#!/usr/bin/env bash
# Hot reload check (native): a texture file that changes on disk while the app runs must
# show up in the picture. Starts the sandbox with a checkerboard probe texture on the
# ground, replaces the file with a red one after the first snapshot, and fails unless the
# second snapshot differs from the first. Extra arguments go to the sandbox, e.g.
#   scripts/verify/hot_reload_texture.sh --software-renderer
set -euo pipefail
cd "$(dirname "$0")/../.."

probe=sandbox/assets/textures/hot_reload_probe.png
before=target/snapshots/hot_reload_before.png
after=target/snapshots/hot_reload_after.png
log=target/snapshots/hot_reload.log

cp sandbox/assets/textures/checker.png "$probe"
trap 'rm -f "$probe"' EXIT
rm -f "$before" "$after"
mkdir -p target/snapshots
# Build first, so the file swap below is timed against the running app, not the compiler.
cargo build -q -p retreat_sandbox

# Replace the probe as soon as the "before" snapshot exists (polls for up to 120 s).
(
  for _ in $(seq 1 1200); do
    if [ -f "$before" ]; then
      cp scripts/verify/hot_reload/probe_red.png "$probe"
      exit 0
    fi
    sleep 0.1
  done
  echo "hot_reload_texture.sh: the before-snapshot never appeared" >&2
) &
swapper=$!

cargo run -q -p retreat_sandbox -- --headless "$@" \
  --commands scripts/verify/hot_reload/texture.cmds > "$log" 2>&1 \
  || { echo "FAIL: the sandbox exited with an error; see $log"; tail -5 "$log"; exit 1; }
wait "$swapper"

if cmp -s "$before" "$after"; then
  echo "FAIL: $after is identical to $before; the reloaded texture was not drawn"
  exit 1
fi
echo "PASS: the texture reloaded and the picture changed ($before -> $after)"
