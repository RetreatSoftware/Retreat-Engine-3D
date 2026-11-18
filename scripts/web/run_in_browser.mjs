#!/usr/bin/env node
// Runs a command script in the browser build of the sandbox, in a headless Chrome or Edge,
// and reports the result like a native run: exit code 0 when the script passes, 1 when it fails.
//
//   node scripts/web/run_in_browser.mjs scripts/verify/physics_boxes.cmds [--seed 0]
//
// Before: scripts/web/build.sh (fills target/web). Needs Node 22+ (built-in fetch and WebSocket)
// and Chrome, Chromium or Edge (found automatically, or set RETREAT_BROWSER=<path to binary>).
// No npm packages.
//
// What it does:
// 1. Serves target/web on a free localhost port (its own tiny static server).
// 2. Starts the browser headless with SwiftShader (CPU WebGL2): same pixels on every machine,
//    works without a GPU (CI).
// 3. Opens /?commands=<script>&seed=<n> and polls window.__retreat.status() through the
//    Chrome DevTools Protocol until the script passed or failed.
// 4. Saves every snapshot the script took to target/snapshots/web/<name>.png and prints the
//    page's errors and `state.hash` lines.

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { extname, join, normalize, resolve } from "node:path";

const WEB_DIR = resolve("target/web");
const SNAPSHOT_DIR = resolve("target/snapshots/web");
const POLL_INTERVAL_MS = 500;
const DEFAULT_TIMEOUT_SECONDS = 300;
const MIME_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".png": "image/png",
  ".glb": "model/gltf-binary",
  ".gltf": "model/gltf+json",
};

function usage(message) {
  console.error(`run_in_browser: ${message}`);
  console.error("usage: node scripts/web/run_in_browser.mjs <script.cmds> [--seed <n>] [--timeout <seconds>]");
  process.exit(2);
}

function parseArguments(argv) {
  const options = { script: null, seed: "0", timeoutSeconds: DEFAULT_TIMEOUT_SECONDS };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--seed") options.seed = argv[++index] ?? usage("--seed needs a value");
    else if (argument === "--timeout") options.timeoutSeconds = Number(argv[++index]);
    else if (argument.startsWith("--")) usage(`unknown option ${argument}`);
    else options.script = argument.replaceAll("\\", "/");
  }
  if (!options.script) usage("missing the script path");
  if (!existsSync(join(WEB_DIR, "index.html"))) usage("target/web is empty; run scripts/web/build.sh first");
  if (!existsSync(join(WEB_DIR, options.script))) usage(`${options.script} is not in target/web (build.sh copies scripts/verify)`);
  return options;
}

function startStaticServer() {
  const server = createServer((request, response) => {
    const path = normalize(decodeURIComponent(new URL(request.url, "http://x").pathname)).replace(/^[/\\]+/, "");
    const file = join(WEB_DIR, path === "" ? "index.html" : path);
    if (!file.startsWith(WEB_DIR) || !existsSync(file)) {
      response.writeHead(404).end("not found");
      return;
    }
    response.writeHead(200, { "content-type": MIME_TYPES[extname(file)] ?? "text/plain; charset=utf-8" });
    response.end(readFileSync(file));
  });
  return new Promise((resolveServer) => server.listen(0, "127.0.0.1", () => resolveServer(server)));
}

function findBrowser() {
  const candidates = [
    process.env.RETREAT_BROWSER,
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  ];
  const found = candidates.find((candidate) => candidate && existsSync(candidate));
  if (!found) usage("no Chrome/Chromium/Edge found; set RETREAT_BROWSER=<path to the browser binary>");
  return found;
}

/** Starts the browser and resolves with its DevTools WebSocket URL. */
function launchBrowser(binary) {
  const profile = mkdtempSync(join(tmpdir(), "retreat-browser-"));
  const browser = spawn(binary, [
    "--headless=new",
    "--remote-debugging-port=0",
    `--user-data-dir=${profile}`,
    "--use-angle=swiftshader",
    "--enable-unsafe-swiftshader",
    "--window-size=1280,720",
    "--no-first-run",
    "--no-default-browser-check",
    "about:blank",
  ]);
  return new Promise((resolveUrl, rejectUrl) => {
    let output = "";
    browser.stderr.on("data", (chunk) => {
      output += chunk;
      const match = output.match(/DevTools listening on (ws:\/\/\S+)/);
      if (match) resolveUrl({ browser, devtoolsUrl: match[1] });
    });
    browser.on("exit", (code) => rejectUrl(new Error(`browser exited (${code}) before DevTools started:\n${output}`)));
  });
}

/** A minimal Chrome DevTools Protocol client over the browser's WebSocket. */
async function connectDevtools(url) {
  const socket = new WebSocket(url);
  await new Promise((resolveOpen, rejectOpen) => {
    socket.onopen = resolveOpen;
    socket.onerror = rejectOpen;
  });
  let nextId = 1;
  const pending = new Map();
  const listeners = [];
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) {
      const { resolveCall, rejectCall } = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) rejectCall(new Error(JSON.stringify(message.error)));
      else resolveCall(message.result);
    } else if (message.method) {
      for (const listener of listeners) listener(message);
    }
  };
  return {
    call(method, params = {}, sessionId = undefined) {
      const id = nextId++;
      socket.send(JSON.stringify({ id, method, params, sessionId }));
      return new Promise((resolveCall, rejectCall) => pending.set(id, { resolveCall, rejectCall }));
    },
    onEvent(listener) {
      listeners.push(listener);
    },
    close() {
      socket.close();
    },
  };
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const server = await startStaticServer();
  const port = server.address().port;
  const { browser, devtoolsUrl } = await launchBrowser(findBrowser());
  const devtools = await connectDevtools(devtoolsUrl);

  const pageUrl = `http://127.0.0.1:${port}/?commands=${encodeURIComponent(options.script)}&seed=${options.seed}`;
  const { targetId } = await devtools.call("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await devtools.call("Target.attachToTarget", { targetId, flatten: true });
  devtools.onEvent((message) => {
    if (message.sessionId !== sessionId || message.method !== "Runtime.consoleAPICalled") return;
    const text = message.params.args.map((arg) => arg.value ?? arg.description ?? "").join(" ");
    if (/ERROR|state\.hash|panicked/.test(text)) console.log(`[page] ${text.replace(/%c/g, "")}`);
  });
  await devtools.call("Runtime.enable", {}, sessionId);
  await devtools.call("Page.navigate", { url: pageUrl }, sessionId);
  console.log(`run_in_browser: ${pageUrl}`);

  const evaluate = async (expression) => {
    const { result, exceptionDetails } = await devtools.call(
      "Runtime.evaluate",
      { expression, awaitPromise: true, returnByValue: true },
      sessionId,
    );
    if (exceptionDetails) throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
    return result.value;
  };

  const deadline = Date.now() + options.timeoutSeconds * 1000;
  let status = { script: "loading" };
  while (Date.now() < deadline) {
    status = await evaluate("window.__retreat ? window.__retreat.status() : { script: 'loading' }").catch(() => status);
    if (status.script === "passed" || status.script === "failed" || status.script === "none") break;
    await new Promise((wait) => setTimeout(wait, POLL_INTERVAL_MS));
  }

  const names = (await evaluate("window.__retreat ? window.__retreat.snapshotNames() : []").catch(() => [])) ?? [];
  mkdirSync(SNAPSHOT_DIR, { recursive: true });
  for (const name of names) {
    const dataUrl = await evaluate(`window.__retreat.snapshotPng(${JSON.stringify(name)})`);
    const file = join(SNAPSHOT_DIR, `${name}.png`);
    writeFileSync(file, Buffer.from(dataUrl.split(",")[1], "base64"));
    console.log(`run_in_browser: snapshot ${file}`);
  }

  // Browser audio waits for a user gesture: report the audio contexts before and after a real
  // key press (a trusted input event), so the page's resume-on-gesture handling is visible.
  const audioBefore = await evaluate("window.__retreat.audioState()").catch(() => []);
  if (audioBefore.length > 0) {
    for (const type of ["keyDown", "keyUp"]) {
      await devtools.call("Input.dispatchKeyEvent", { type, key: "a", code: "KeyA", text: "a" }, sessionId);
    }
    await new Promise((wait) => setTimeout(wait, 300));
    const audioAfter = await evaluate("window.__retreat.audioState()").catch(() => []);
    console.log(
      `run_in_browser: audio contexts before a key press ${JSON.stringify(audioBefore)}, after ${JSON.stringify(audioAfter)}`,
    );
  }

  devtools.close();
  browser.kill();
  server.close();

  if (status.script === "passed") {
    console.log(`run_in_browser: PASS ${options.script}`);
    process.exit(0);
  }
  const reason =
    status.script === "failed"
      ? status.failure
      : status.script === "none"
        ? "the page started without the script (check the URL)"
        : `timed out after ${options.timeoutSeconds} s, status ${JSON.stringify(status)}`;
  console.error(`run_in_browser: FAIL ${options.script}: ${reason}`);
  process.exit(1);
}

main().catch((error) => {
  console.error(`run_in_browser: ${error.stack ?? error}`);
  process.exit(1);
});
