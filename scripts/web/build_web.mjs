#!/usr/bin/env node
// Builds a Retreat game for the browser. Run from the game's repository root:
//
//   node <engine checkout>/scripts/web/build_web.mjs --bin <binary name> --title "<page title>"
//        [--assets <dir>] [--scripts <dir> | --no-scripts] [--out <dir>] [--release]
//
// Writes into --out (default target/web, emptied first): <bin>.js and <bin>_bg.wasm (wasm-bindgen
// output), index.html (the engine's web/index.html with the title, name and size filled in),
// retreat-server.json (`null`; a server started with --serve-web answers with its own), and copies of
// --assets (default assets) as assets/ and --scripts (default scripts/verify) as scripts/verify/, so
// `?commands=` can fetch them (--no-scripts leaves them out). Serve it with the game's server
// (--serve-web <out>) or, on this computer only, any static file server.
//
// Two builds (decided 2026-11-27, package plan P1):
// - default, while developing: Cargo profile `web-dev` (dev settings without debug info), fast to
//   build, about 90 MB.
// - --release, to hand the game to players: Cargo profile `web-release` (size-optimized, link-time
//   optimization, one codegen unit, stripped), then `wasm-opt` (Binaryen) shrinks the module, then
//   every file that compresses by at least COMPRESS_GAIN is also written as `<file>.br` (Brotli), which
//   the game's server sends to browsers that accept it. Much slower to build.
// Both print the sizes (scripts/web/web_sizes.mjs says what counts).
//
// Needs: `rustup target add wasm32-unknown-unknown`, and wasm-bindgen-cli at the version of the
// wasm-bindgen crate in Cargo.lock (cargo install wasm-bindgen-cli --version <x> --locked); --release
// also needs wasm-opt (cargo install wasm-opt --locked). The Cargo profile must exist in the game's
// workspace. No npm packages.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { constants as zlibConstants, brotliCompressSync } from "node:zlib";
import { COMPRESSED_SUFFIX, measureWebFolder, megabytes } from "./web_sizes.mjs";

const TEMPLATE = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "web", "index.html");
const USAGE =
  'usage: node build_web.mjs --bin <binary name> --title "<page title>" [--assets <dir>] [--scripts <dir> | --no-scripts] [--out <dir>] [--release]';
/** A compressed copy is kept only when it is at most this share of the file (PNG and OGG are not). */
const COMPRESS_GAIN = 0.9;
/** The server answers this file itself; its copy in the folder is only for static file servers. */
const SERVER_OFFER_FILE = "retreat-server.json";
/**
 * WebAssembly features rustc enables by default for wasm32-unknown-unknown (Rust 1.82 and later);
 * wasm-opt must be told them or it refuses the module. It adds no feature the module does not use.
 */
const WASM_FEATURES = [
  "--enable-bulk-memory",
  "--enable-multivalue",
  "--enable-mutable-globals",
  "--enable-nontrapping-float-to-int",
  "--enable-reference-types",
  "--enable-sign-ext",
];
/** What wasm-opt optimizes for (measured on Retreat-Arena: see the engine's CLAUDE.md, "Release browser build"). */
const WASM_OPT_LEVEL = "-Oz";

function fail(message) {
  console.error(`build_web: ${message}`);
  process.exit(1);
}

const options = { bin: null, title: null, assets: "assets", scripts: "scripts/verify", out: "target/web" };
const flags = { release: false, "no-scripts": false };
const argv = process.argv.slice(2);
for (let index = 0; index < argv.length; index += 1) {
  const name = argv[index].replace(/^--/, "");
  if (!argv[index].startsWith("--")) fail(`bad argument ${argv[index]}; ${USAGE}`);
  if (name in flags) {
    flags[name] = true;
    continue;
  }
  const value = argv[index + 1];
  if (!(name in options) || value === undefined) fail(`bad argument ${argv[index]}; ${USAGE}`);
  options[name] = value;
  index += 1;
}
if (!options.bin || !options.title) fail(`--bin and --title are required; ${USAGE}`);
if (!/^[A-Za-z0-9_-]+$/.test(options.bin)) fail(`--bin \`${options.bin}\` is not a binary name; ${USAGE}`);
const sources = flags["no-scripts"] ? [options.assets] : [options.assets, options.scripts];
for (const directory of sources) {
  if (!existsSync(directory)) fail(`the directory \`${directory}\` does not exist (run from the game's repository root)`);
}
// The output is deleted first: it must not hold the sources.
const out = resolve(options.out);
for (const kept of [".", ...sources]) {
  if ((resolve(kept) + "/").startsWith(out + "/") || (resolve(kept) + "\\").startsWith(out + "\\")) {
    fail(`--out \`${options.out}\` contains \`${kept}\`; choose a directory of its own`);
  }
}

const run = (command, args) => execFileSync(command, args, { stdio: "inherit" });
const runOrExplain = (command, args, install) => {
  try {
    run(command, args);
  } catch (error) {
    if (error.code === "ENOENT") fail(`\`${command}\` is not installed; install it with: ${install}`);
    fail(`\`${command} ${args.join(" ")}\` failed (exit ${error.status}); its messages are above`);
  }
};
const WASM_OPT_INSTALL = "cargo install wasm-opt --locked";
if (flags.release) {
  // Before the long build: say at once what is missing.
  try {
    execFileSync("wasm-opt", ["--version"], { stdio: "ignore" });
  } catch {
    fail(`--release needs wasm-opt (Binaryen), which is not installed; install it with: ${WASM_OPT_INSTALL}`);
  }
}

const profile = flags.release ? "web-release" : "web-dev";
const targetDirectory = process.env.CARGO_TARGET_DIR ?? "target";
const cargoToml = readFileSync("Cargo.toml", "utf8");
if (!cargoToml.includes(`[profile.${profile}]`)) {
  fail(`the game's Cargo.toml has no [profile.${profile}]; add it (see the engine's Cargo.toml for the settings)`);
}
runOrExplain("cargo", ["build", "--bin", options.bin, "--target", "wasm32-unknown-unknown", "--profile", profile], "https://rustup.rs");
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
runOrExplain(
  "wasm-bindgen",
  [
    "--target", "web", "--no-typescript", "--remove-name-section", "--remove-producers-section", "--out-dir", out,
    join(targetDirectory, "wasm32-unknown-unknown", profile, `${options.bin}.wasm`),
  ],
  "cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock> --locked",
);
const wasmFile = join(out, `${options.bin}_bg.wasm`);
const boundBytes = statSync(wasmFile).size;
if (flags.release) {
  runOrExplain("wasm-opt", [WASM_OPT_LEVEL, ...WASM_FEATURES, "--strip-debug", "--strip-producers", wasmFile, "-o", wasmFile], WASM_OPT_INSTALL);
}

const escaped = options.title.replace(/&/g, "&amp;").replace(/</g, "&lt;");
const page = readFileSync(TEMPLATE, "utf8")
  .replaceAll("{{TITLE}}", () => escaped)
  .replaceAll("{{WASM_NAME}}", () => options.bin)
  .replaceAll("{{WASM_BYTES}}", () => String(statSync(wasmFile).size));
writeFileSync(join(out, "index.html"), page);
writeFileSync(join(out, SERVER_OFFER_FILE), "null\n");
cpSync(options.assets, join(out, "assets"), { recursive: true });
if (!flags["no-scripts"]) {
  mkdirSync(join(out, "scripts"), { recursive: true });
  cpSync(options.scripts, join(out, "scripts", "verify"), { recursive: true });
}

function listFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? listFiles(path) : [path];
  });
}

if (flags.release) {
  for (const file of listFiles(out)) {
    if (file.endsWith(SERVER_OFFER_FILE) || file.startsWith(join(out, "scripts"))) continue;
    const bytes = readFileSync(file);
    const compressed = brotliCompressSync(bytes, {
      params: {
        [zlibConstants.BROTLI_PARAM_QUALITY]: zlibConstants.BROTLI_MAX_QUALITY,
        // The largest window browsers decode (16 MiB).
        [zlibConstants.BROTLI_PARAM_LGWIN]: zlibConstants.BROTLI_MAX_WINDOW_BITS,
        [zlibConstants.BROTLI_PARAM_SIZE_HINT]: bytes.length,
      },
    });
    if (compressed.length <= bytes.length * COMPRESS_GAIN) writeFileSync(file + COMPRESSED_SUFFIX, compressed);
  }
}

const sizes = measureWebFolder(out);
const optimized = flags.release ? ` (${megabytes(boundBytes)} before wasm-opt)` : "";
console.log(`build_web: built ${options.out} with the ${profile} profile`);
console.log(
  `build_web: sizes: wasm ${megabytes(sizes.wasmBytes)}${optimized}, ${sizes.files} files ${megabytes(sizes.uncompressedBytes)}, download ${megabytes(sizes.downloadBytes)}`,
);
