#!/usr/bin/env bash
# Builds the sandbox for the browser into target/web/:
#   retreat_sandbox.js + retreat_sandbox_bg.wasm (wasm-bindgen output), index.html,
#   assets/ (the sandbox's asset folder) and scripts/verify/ (so ?commands= can fetch them).
# Needs: rustup target wasm32-unknown-unknown, and wasm-bindgen-cli at the exact version of the
# wasm-bindgen crate in Cargo.lock (cargo install wasm-bindgen-cli --version <x> --locked).
# Serve with: python -m http.server 8090 --directory target/web
# Then open:  http://localhost:8090/?commands=scripts/verify/physics_boxes.cmds
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo build -p retreat_sandbox --target wasm32-unknown-unknown --profile web-dev

rm -rf target/web
mkdir -p target/web/scripts
wasm-bindgen --target web --no-typescript --remove-name-section --out-dir target/web \
  target/wasm32-unknown-unknown/web-dev/retreat_sandbox.wasm
cp web/index.html target/web/
cp -r sandbox/assets target/web/assets
cp -r scripts/verify target/web/scripts/verify
echo "Built target/web. Serve: python -m http.server 8090 --directory target/web"
