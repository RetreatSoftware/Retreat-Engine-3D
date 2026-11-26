#!/usr/bin/env bash
# Builds the sandbox for the browser into target/web/ with scripts/web/build_web.mjs (the build
# every game uses).
# Needs: rustup target wasm32-unknown-unknown, and wasm-bindgen-cli at the exact version of the
# wasm-bindgen crate in Cargo.lock (cargo install wasm-bindgen-cli --version <x> --locked).
# Serve with: python -m http.server 8090 --directory target/web
# Then open:  http://localhost:8090/?commands=scripts/verify/physics_boxes.cmds
set -euo pipefail
cd "$(dirname "$0")/../.."

node scripts/web/build_web.mjs --bin retreat_sandbox --title "Retreat Engine Sandbox" \
  --assets sandbox/assets --scripts scripts/verify --out target/web
echo "Serve: python -m http.server 8090 --directory target/web"
