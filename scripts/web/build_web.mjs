#!/usr/bin/env node
// Builds a Retreat game for the browser. Run from the game's repository root:
//
//   node <engine checkout>/scripts/web/build_web.mjs --bin <binary name> --title "<page title>"
//        [--assets <dir>] [--scripts <dir>] [--out <dir>]
//
// Writes into --out (default target/web, emptied first): <bin>.js and <bin>_bg.wasm (wasm-bindgen
// output), index.html (the engine's web/index.html with the title and name filled in),
// retreat-server.json (`null`; a server started with --serve-web answers with its own), and copies of
// --assets (default assets) as assets/ and --scripts (default scripts/verify) as scripts/verify/, so
// `?commands=` can fetch them. Serve it with the game's server (--serve-web <out>) or, on this
// computer only, any static file server.
//
// Needs: `rustup target add wasm32-unknown-unknown`, and wasm-bindgen-cli at the version of the
// wasm-bindgen crate in Cargo.lock (cargo install wasm-bindgen-cli --version <x> --locked).
// Cargo's `web-dev` profile must exist in the game's workspace. No npm packages.
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const TEMPLATE = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "web", "index.html");
const USAGE = 'usage: node build_web.mjs --bin <binary name> --title "<page title>" [--assets <dir>] [--scripts <dir>] [--out <dir>]';

function fail(message) {
  console.error(`build_web: ${message}`);
  process.exit(1);
}

const options = { bin: null, title: null, assets: "assets", scripts: "scripts/verify", out: "target/web" };
const argv = process.argv.slice(2);
for (let index = 0; index < argv.length; index += 2) {
  const name = argv[index].replace(/^--/, "");
  const value = argv[index + 1];
  if (!argv[index].startsWith("--") || !(name in options) || value === undefined) fail(`bad argument ${argv[index]}; ${USAGE}`);
  options[name] = value;
}
if (!options.bin || !options.title) fail(`--bin and --title are required; ${USAGE}`);
if (!/^[A-Za-z0-9_-]+$/.test(options.bin)) fail(`--bin \`${options.bin}\` is not a binary name; ${USAGE}`);
for (const directory of [options.assets, options.scripts]) {
  if (!existsSync(directory)) fail(`the directory \`${directory}\` does not exist (run from the game's repository root)`);
}
// The output is deleted first: it must not hold the sources.
const out = resolve(options.out);
for (const kept of [".", options.assets, options.scripts]) {
  if ((resolve(kept) + "/").startsWith(out + "/") || (resolve(kept) + "\\").startsWith(out + "\\")) {
    fail(`--out \`${options.out}\` contains \`${kept}\`; choose a directory of its own`);
  }
}

const run = (command, args) => execFileSync(command, args, { stdio: "inherit" });
run("cargo", ["build", "--bin", options.bin, "--target", "wasm32-unknown-unknown", "--profile", "web-dev"]);
rmSync(out, { recursive: true, force: true });
mkdirSync(join(out, "scripts"), { recursive: true });
run("wasm-bindgen", [
  "--target", "web", "--no-typescript", "--remove-name-section", "--out-dir", out,
  join("target", "wasm32-unknown-unknown", "web-dev", `${options.bin}.wasm`),
]);

const escaped = options.title.replace(/&/g, "&amp;").replace(/</g, "&lt;");
const page = readFileSync(TEMPLATE, "utf8")
  .replaceAll("{{TITLE}}", () => escaped)
  .replaceAll("{{WASM_NAME}}", () => options.bin);
writeFileSync(join(out, "index.html"), page);
writeFileSync(join(out, "retreat-server.json"), "null\n");
cpSync(options.assets, join(out, "assets"), { recursive: true });
cpSync(options.scripts, join(out, "scripts", "verify"), { recursive: true });
console.log(`build_web: built ${options.out}`);
