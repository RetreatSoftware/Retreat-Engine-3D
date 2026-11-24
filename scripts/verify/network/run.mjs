// Network check: one server and two clients, three real processes of the sandbox talking over real
// WebTransport on localhost (a self-signed certificate pinned by its hash). Each process runs a
// command script in real time; at the end each prints the bodies it sees, and this script checks that
// all three worlds agree.
//
// Run from Retreat-Engine-3D after `cargo build -p retreat_sandbox`:
//   node scripts/verify/network/run.mjs [--software-renderer]
// Exit code 0 = every process exited cleanly and the three worlds agree (positions within 0.05).

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..", "..", "..");
const exe = join(root, "target", "debug", process.platform === "win32" ? "retreat_sandbox.exe" : "retreat_sandbox");
if (!existsSync(exe)) {
  console.error(`network check: ${exe} does not exist; run \`cargo build -p retreat_sandbox\` first`);
  process.exit(1);
}
const extra = process.argv.includes("--software-renderer") ? ["--software-renderer"] : [];
const TOLERANCE = 0.05;
const TIMEOUT_MS = 90_000;

function start(name, args) {
  const child = spawn(exe, ["--headless", "--real-time", ...extra, ...args], { cwd: root });
  const process_ = { name, child, output: "", exited: null };
  const collect = (chunk) => {
    process_.output += chunk.toString();
  };
  child.stdout.on("data", collect);
  child.stderr.on("data", collect);
  process_.done = new Promise((done) => {
    child.on("exit", (code) => {
      process_.exited = code;
      done(code);
    });
  });
  return process_;
}

function waitFor(what, check) {
  return new Promise((resolvePromise, reject) => {
    const started = Date.now();
    const timer = setInterval(() => {
      const found = check();
      if (found !== undefined) {
        clearInterval(timer);
        resolvePromise(found);
      } else if (Date.now() - started > TIMEOUT_MS) {
        clearInterval(timer);
        reject(new Error(`timed out waiting for ${what}`));
      }
    }, 100);
  });
}

function bodies(output) {
  const found = new Map();
  for (const match of output.matchAll(/net-body (\S+) (-?[\d.]+) (-?[\d.]+) (-?[\d.]+)/g)) {
    found.set(match[1], [Number(match[2]), Number(match[3]), Number(match[4])]);
  }
  return found;
}

const processes = [];
let failure = null;
try {
  const server = start("server", ["--server", "0", "--commands", join(here, "server.cmds")]);
  processes.push(server);
  const listening = await waitFor("the server to listen", () => {
    const match = server.output.match(/retreat-net: listening on UDP port (\d+) certificate-hash ([0-9a-f]{64})/);
    if (server.exited !== null) throw new Error(`the server exited early:\n${server.output}`);
    return match ? { port: match[1], hash: match[2] } : undefined;
  });
  console.log(`network check: server listening on port ${listening.port}`);
  for (const name of ["a", "b"]) {
    processes.push(
      start(`client ${name}`, [
        "--connect", `https://127.0.0.1:${listening.port}`,
        "--cert-hash", listening.hash,
        "--commands", join(here, `client_${name}.cmds`),
      ]),
    );
  }
  const codes = await Promise.race([
    Promise.all(processes.map((p) => p.done)),
    new Promise((_, reject) => setTimeout(() => reject(new Error("the processes did not finish in time")), TIMEOUT_MS)),
  ]);
  processes.forEach((p, index) => {
    if (codes[index] !== 0) {
      failure = `${p.name} exited with code ${codes[index]}:\n${p.output.split("\n").filter((l) => /ERROR|error|panicked/.test(l)).join("\n")}`;
    }
  });
  if (!failure) {
    const views = processes.map((p) => ({ name: p.name, bodies: bodies(p.output) }));
    const reference = views[0].bodies;
    const expectedNames = ["net_a", "net_b", "sandbox_cube", "sandbox_ground"];
    for (const view of views) {
      const names = [...view.bodies.keys()].sort();
      if (JSON.stringify(names) !== JSON.stringify(expectedNames)) {
        failure = `${view.name} sees bodies ${JSON.stringify(names)}, expected ${JSON.stringify(expectedNames)}`;
        break;
      }
      for (const [name, position] of view.bodies) {
        const distance = Math.hypot(...position.map((value, axis) => value - reference.get(name)[axis]));
        if (distance > TOLERANCE) {
          failure = `${view.name} has ${name} at ${position}, the server at ${reference.get(name)} (distance ${distance.toFixed(3)})`;
          break;
        }
      }
      if (failure) break;
    }
    if (!failure) {
      for (const [name, position] of reference) console.log(`network check: ${name} at ${position.join(" ")} on all three`);
    }
  }
} catch (error) {
  failure = String(error.message ?? error);
} finally {
  processes.forEach((p) => p.exited === null && p.child.kill());
}
if (failure) {
  console.error(`network check: FAIL: ${failure}`);
  process.exit(1);
}
console.log("network check: PASS");
