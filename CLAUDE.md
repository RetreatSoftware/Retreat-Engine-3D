# Retreat-Engine-3D

The 3D game engine of the Retreat ecosystem: rendering, game loop, networking, and other fundamental runtime systems. It integrates gameplay modules from `Retreat-Modules` and is the base that `Retreat-Forge` assembles into playable games.

This file contains project facts and rules only. Personal preferences belong in `CLAUDE.local.md`.

The root `../CLAUDE.md` also applies here. This file may narrow its scope (see "Status") but not relax its rules; its verification and infrastructure rules apply only once this repository has code that needs them.

## Status

Early stage. No code exists yet. Do not implement anything beyond what is explicitly requested.

## Ecosystem context

Sibling repositories (cloned next to this one under `Retreat/`):

- `Retreat-CoreSDK` — shared math, data structures, utilities, common abstractions. Has no Retreat dependencies.
- `Retreat-Modules` — independent gameplay crates (inventory, combat, loot, ...).
- `Retreat-Forge` — builder UI: selects modules, resolves dependencies, builds the game, AI customization.
- `Retreat-Website` — web platform: catalog, accounts, rankings, marketplace.

Vision documents live in the parent folder: `FRAMEWORK.en.md` (engine/Forge) and `PLATFORM.en.md` (platform). Read `FRAMEWORK.en.md` before making architectural decisions. The originals (`FRAMEWORK.md`, `PLATFORM.md`) are in Portuguese.

## Decided technical direction

- **Language:** Rust. Performance is a hard requirement.
- **Rendering:** `wgpu`. Treat WebGL2 limits as the baseline; WebGPU-only features must be feature-gated.
- **Targets:** native (also used for the authoritative game server) and WebAssembly (`wasm32-unknown-unknown`) for the browser. Games run in the player's browser with no download.
- **Build:** Cargo workspace. Both native and WASM targets must build in CI from the start.
- **Module composition:** compile-time. Each gameplay module is an independent crate with a manifest (capabilities, dependencies, compatibility, configuration) declared in its `Cargo.toml` metadata. Forge resolves and validates; Cargo only compiles. Do not use Cargo features as the plugin mechanism.
- **No dynamic plugin loading, no custom scripting language** for now. Keep game logic data-driven and behind contract traits so a scripting layer stays possible later.
- **Authority:** the server owns game state. Games never mint currency or change balances; economic operations go through the platform adapter to trusted backend services.

## Open decisions (do not assume an answer)

- Build on Bevy (plugins, ECS, renderer on wgpu) versus a custom engine layer. A short spike is planned to decide this. Do not write a custom ECS unless a technical need is demonstrated.
- Netcode model (server-authoritative with prediction versus deterministic lockstep).
- Asset formats and pipeline.
- Distribution of crates across repositories (private registry versus git dependencies).
- 2D/3D scope beyond 3D, and the exact minimum combination that always yields a playable game.

## Architecture rules

- Dependency direction: `Retreat-CoreSDK` → contract crates → engine and modules → generated game. Never reverse it.
- **Shared generic helpers:** check `Retreat-CoreSDK` first. A helper stays in the engine until a second consumer (a module) needs it; then move it to CoreSDK instead of copying it. What belongs in CoreSDK, and what does not, is defined in `../Retreat-CoreSDK/CLAUDE.md`. Shared gameplay concepts go in the engine's public API / contract crates, not in CoreSDK.
- Modules depend on contract crates (traits, events, component types), never on each other and never on engine internals.
- Expose a small, stable, documented public API. Keep renderer and networking internals private.
- Isolate all `cfg(target_arch = "wasm32")` code in a single platform layer. Do not assume native capabilities (threads, filesystem, sockets) exist in the browser; assume single-threaded WASM as the baseline.
- Configuration must be reproducible: the same module selection must rebuild the same project.
- Generated project structures must be predictable and documented so humans and AI agents can customize them safely.
- Distinguish trusted first-party modules from untrusted third-party or AI-generated code. Rust memory safety is not sandboxing.

## Working rules

- Keep the initial implementation as simple as reasonably possible.
- Do not modify sibling repositories from this one without being asked.
- Write documentation and code comments in English.

## Verification

Applies once the engine has a first loop; build the missing pieces in this order when a task needs them.

1. **Command registry** (`snapshot <png>`, `wait <frames>`, `quit`, `assert.*`). Input and UI dispatch through it. Scripts run one line per frame, fixed timestep, seeded RNG.
2. **Headless snapshot:** `cargo run --example <x> -- --headless --commands <file>` renders to an offscreen wgpu texture and writes a PNG. Open it. CI uses a software adapter (lavapipe/WARP).
3. **Browser bridge:** serve the WASM build (`trunk serve` or wasm-bindgen) and expose `window.__retreat.command()` / `snapshot()`. Run the same scripts to check WASM-only behavior. `wasm-bindgen-test` (headless Chrome) covers WASM-only logic in CI.
4. **Per change:** `cargo check` for native and `wasm32-unknown-unknown`, plus `cargo nextest`.
5. **Determinism:** the same script must give the same state hash on native and WASM. Mismatch means platform drift.
6. **Netcode:** test server plus N fake clients in one process with simulated latency, not in browsers.
7. **Performance:** frame time and draw-call count are readable via commands so budgets can be asserted.

