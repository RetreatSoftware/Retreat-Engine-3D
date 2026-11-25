// Network check: one server and two clients, three real processes of the sandbox talking over real
// WebTransport on localhost (a self-signed certificate pinned by its hash). Each process runs a
// command script in real time; at the end each prints the bodies it sees, and this script checks that
// all three worlds agree.
//
// Run from Retreat-Engine-3D after `cargo build -p retreat_sandbox`:
//   node scripts/verify/network/run.mjs [--software-renderer] [--browser] [--scenario players|preset_players|walkers]
// The default scenario spawns and pushes boxes with `net.send`; `--scenario players` uses the players
// module: each client holds a movement key and the engine predicts and sends the movement.
// `--scenario preset_players` is the first tested preset (engine + players + orbit_camera + menu): one
// client that renders (so it writes PNGs) moves its player; the check asserts the player moved.
// `--scenario walkers`: each client drives a character with analog input (axis, mouse look, jump); the check
// also compares the module state and requires both characters to have walked.
// With --browser, client B is the browser build (headless Chrome through scripts/web/run_in_browser.mjs,
// WebTransport from the page): run scripts/web/build.sh first.
// Exit code 0 = every process exited cleanly and the three worlds agree (positions within 0.05).

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..", "..", "..");
const exe = join(root, "target", "debug", process.platform === "win32" ? "retreat_sandbox.exe" : "retreat_sandbox");
// A GitHub annotation shows on the run's page and through the API without a login, which the raw log
// does not; outside CI this prints nothing extra.
function annotate(report) {
  if (process.env.GITHUB_ACTIONS) {
    const text = report.replaceAll("%", "%25").replaceAll("\r", "%0D").replaceAll("\n", "%0A").slice(0, 60000);
    console.log(`::error title=Network check failed::${text}`);
  }
}
function die(message) {
  console.error(`network check: ${message}`);
  annotate(message);
  process.exit(1);
}
process.on("uncaughtException", (error) => die(`unexpected error: ${error.stack ?? error}`));
if (!existsSync(exe)) {
  die(`${exe} does not exist; run \`cargo build -p retreat_sandbox\` first`);
}
const extra = process.argv.includes("--software-renderer") ? ["--software-renderer"] : [];
const browserClient = process.argv.includes("--browser");
const scenario = process.argv.includes("--scenario") ? process.argv[process.argv.indexOf("--scenario") + 1] : "boxes";
if (!["boxes", "players", "preset_players", "walkers"].includes(scenario)) die(`unknown scenario ${scenario}; use boxes, players, preset_players or walkers`);
const isPreset = scenario === "preset_players";
const TOLERANCE = 0.05;
// The browser variant waits much longer (the page needs many seconds to load) and its scripts too.
const TIMEOUT_MS = browserClient ? 240_000 : 90_000;
const scenarioFolder = scenario !== "boxes" ? scenario : browserClient ? "browser" : "";
const scriptDir = join(here, scenarioFolder);
const webScriptDir = `scripts/verify/network${scenarioFolder ? `/${scenarioFolder}` : ""}`;

// The native processes only simulate, so they draw nothing (`--no-render`): rendering on a software
// GPU made them tick slower than the 64 Hz of the browser client, and processes whose tick numbers
// drift apart get refused by the server (see CLAUDE.md, Networking). `--software-renderer` is still
// passed, but unused without a renderer; it stays so that a changed flag set cannot reintroduce a GPU.
function start(name, args, command = exe, fixedArgs = ["--headless", "--no-render", "--real-time", ...extra]) {
  const child = spawn(command, [...fixedArgs, ...args], { cwd: root });
  child.on("error", (error) => die(`cannot start ${name} (${command}): ${error.message}`));
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

// The module state values a process printed with `net.states`, as sorted lines.
function moduleStates(output) {
  return [...output.matchAll(/net-state (\S+ \S+ v\d+ [0-9a-f]*)/g)].map((match) => match[1]).sort();
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
  const server = start("server", ["--server", "0", "--commands", join(scriptDir, "server.cmds")]);
  processes.push(server);
  const listening = await waitFor("the server to listen", () => {
    const match = server.output.match(/retreat-net: listening on UDP port (\d+) certificate-hash ([0-9a-f]{64})/);
    if (server.exited !== null) throw new Error(`the server exited early:\n${server.output}`);
    return match ? { port: match[1], hash: match[2] } : undefined;
  });
  console.log(`network check: server listening on port ${listening.port}`);
  // Client b is the browser with --browser. In the preset it also renders natively (PNGs), while client a
  // (the mover) does not.
  for (const name of ["a", "b"]) {
    const scriptName = `client_${name}`;
    if (browserClient && name === "b") {
      const query = `connect=${encodeURIComponent(`https://127.0.0.1:${listening.port}`)}&cert_hash=${listening.hash}&real_time=1`;
      processes.push(
        start(
          "browser client b",
          [`${webScriptDir}/${scriptName}.cmds`, "--query", query],
          process.execPath,
          [join(root, "scripts", "web", "run_in_browser.mjs")],
        ),
      );
      continue;
    }
    processes.push(
      start(
        `client ${name}`,
        [
          "--connect", `https://127.0.0.1:${listening.port}`,
          "--cert-hash", listening.hash,
          "--commands", join(scriptDir, `${scriptName}.cmds`),
        ],
        exe,
        isPreset && name === "b" ? ["--headless", "--real-time", ...extra] : undefined,
      ),
    );
  }
  // The timer is cleared when the processes finish, otherwise it keeps node alive for TIMEOUT_MS after a pass.
  let giveUp;
  const codes = await Promise.race([
    Promise.all(processes.map((p) => p.done)),
    new Promise((_, reject) => {
      giveUp = setTimeout(() => reject(new Error("the processes did not finish in time")), TIMEOUT_MS);
    }),
  ]).finally(() => clearTimeout(giveUp));
  processes.forEach((p, index) => {
    if (codes[index] !== 0) {
      failure = `${p.name} exited with code ${codes[index]}:\n${p.output.split("\n").filter((l) => /ERROR|error|panicked/.test(l)).join("\n")}`;
    }
  });
  if (!failure) {
    const views = processes.map((p) => ({ name: p.name, bodies: bodies(p.output) }));
    const reference = views[0].bodies;
    // The sandbox always has the players module, so each joined client has its player body.
    const sceneNames = ["player_0", "player_1", "sandbox_cube", "sandbox_ground"];
    const extraNames = { boxes: ["net_a", "net_b"], walkers: ["walker_0", "walker_1"] }[scenario] ?? [];
    const expectedNames = [...sceneNames, ...extraNames].sort();
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
    if (!failure && scenario === "walkers") {
      // Both characters spawn at x -4 and walk east, the browser's too: a client that ticks slower
      // than the server adjusts (tick pacing), so its inputs take effect.
      for (const name of ["walker_0", "walker_1"]) {
        const x = reference.get(name)[0];
        if (x < -3) failure = `${name} did not walk east: x ${x}`;
      }
    }
    if (!failure && (scenario === "boxes" || scenario === "walkers")) {
      const reference = JSON.stringify(moduleStates(processes[0].output));
      const different = processes.find((p) => JSON.stringify(moduleStates(p.output)) !== reference);
      if (reference === "[]" || different) {
        failure = `module state differs: server ${reference}, ${different?.name ?? "server"} ${JSON.stringify(moduleStates((different ?? processes[0]).output))}`;
      } else {
        console.log(`network check: module state on every process: ${reference}`);
      }
    }
    if (!failure && isPreset) {
      // Exactly one player must have moved forward (client a held ArrowUp, which is -z), the idle one stays at
      // its spawn. Forward, not sideways: players spawn in a row along x and a sideways mover pushes its neighbour.
      const SPAWN_Z = 2;
      const MIN_MOVE = 0.5;
      const IDLE_MAX = 0.05;
      for (const view of views) {
        const moves = ["player_0", "player_1"].map((name) => [name, SPAWN_Z - view.bodies.get(name)[2]]);
        const movers = moves.filter(([, delta]) => delta > MIN_MOVE);
        const idle = moves.filter(([, delta]) => Math.abs(delta) < IDLE_MAX);
        if (movers.length !== 1 || idle.length !== 1) {
          failure = `${view.name}: expected one player moved more than ${MIN_MOVE} forward of its spawn and one idle, got ${JSON.stringify(moves)}`;
        }
      }
    }
    if (!failure) {
      // The traffic counters each process printed with `net.stats` (their limits are asserted in the scripts).
      for (const p of processes) {
        const stats = [...p.output.matchAll(/net-stats (role=\S+ .*)/g)].pop();
        if (p.name === "server") console.log((p.output.match(/net-stats-detail .*/g) ?? []).pop());
        console.log(`network check: ${p.name}: ${stats ? stats[1].replace(/\[[0-9;]*m/g, "").trim() : "no net-stats line"}`);
      }
      for (const [name, position] of reference) console.log(`network check: ${name} at ${position.join(" ")} on every process`);
    }
  }
} catch (error) {
  failure = String(error.message ?? error);
} finally {
  processes.forEach((p) => p.exited === null && p.child.kill());
}
if (failure) {
  // The tail of every process's output, so a failure on a machine without a terminal (CI) explains itself.
  const tails = processes.map(
    (p) => `----- ${p.name} (exit ${p.exited}) -----\n${p.output.split("\n").slice(-40).join("\n")}`,
  );
  console.error(`${tails.join("\n")}\nnetwork check: FAIL: ${failure}`);
  annotate([failure, ...tails].join("\n"));
  process.exit(1);
}
console.log("network check: PASS");
