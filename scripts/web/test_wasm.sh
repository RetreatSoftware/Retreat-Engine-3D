#!/usr/bin/env bash
# Runs the wasm-bindgen tests (crates/retreat_engine/src/platform/wasm_tests.rs) in headless Chrome.
# These cover browser-only logic that run_in_browser.mjs does not check directly: URL parsing through
# the browser's URLSearchParams, the localStorage save backend, the snapshot memory, the status JSON.
# Needs: rustup target wasm32-unknown-unknown, wasm-bindgen-cli at the exact version of Cargo.lock
# (it provides wasm-bindgen-test-runner), and a chromedriver matching the installed Chrome
# (GitHub's ubuntu runners have both; locally set CHROMEDRIVER=<path to chromedriver>).
# Only the #[wasm_bindgen_test] functions run; ordinary #[test] unit tests stay native (cargo test).
set -euo pipefail
cd "$(dirname "$0")/../.."

export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner
# Headless Chrome needs no sandbox in CI containers and no GPU.
export WASM_BINDGEN_TEST_TIMEOUT="${WASM_BINDGEN_TEST_TIMEOUT:-300}"
cargo test -p retreat_engine --lib --target wasm32-unknown-unknown --profile web-dev "$@"
