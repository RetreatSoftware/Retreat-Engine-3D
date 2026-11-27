// Sizes of a browser build folder, as a player gets it. Used by build_web.mjs (its report) and by
// the package checks of games (they assert the numbers).
//
// What counts: every file a player's browser may download, so not the `.br` copies themselves and not
// `scripts/` (verify scripts are fetched only by `?commands=`). `downloadBytes` takes a file's `.br`
// copy when it has one (a server that serves compressed files sends that), `uncompressedBytes` the
// file itself.
import { existsSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

/** The suffix of a file's compressed copy, written by build_web.mjs --release. */
export const COMPRESSED_SUFFIX = ".br";

function listFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? listFiles(path) : [path];
  });
}

/** `{ wasmBytes, uncompressedBytes, downloadBytes, files }` of the build in `folder`. */
export function measureWebFolder(folder) {
  let wasmBytes = 0;
  let uncompressedBytes = 0;
  let downloadBytes = 0;
  let files = 0;
  for (const path of listFiles(folder)) {
    const name = relative(folder, path).replaceAll("\\", "/");
    if (name.endsWith(COMPRESSED_SUFFIX) || name.startsWith("scripts/")) continue;
    const size = statSync(path).size;
    const compressed = path + COMPRESSED_SUFFIX;
    files += 1;
    uncompressedBytes += size;
    downloadBytes += existsSync(compressed) ? statSync(compressed).size : size;
    if (name.endsWith(".wasm")) wasmBytes += size;
  }
  return { wasmBytes, uncompressedBytes, downloadBytes, files };
}

/** Bytes as megabytes with one decimal (1 MB = 1,000,000 bytes, like download sizes are stated). */
export function megabytes(bytes) {
  return `${(bytes / 1e6).toFixed(1)} MB`;
}
