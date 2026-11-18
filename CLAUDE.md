# Retreat-Engine-3D

The 3D game engine of the Retreat ecosystem: rendering, game loop, networking, and other fundamental runtime systems. It integrates gameplay modules from `Retreat-Modules` and is the base that `Retreat-Forge` assembles into playable games.

This file contains project facts and rules only. Personal preferences belong in `CLAUDE.local.md`.

The root `../CLAUDE.md` also applies here. This file may narrow its scope (see "Status") but not relax its rules; its verification and infrastructure rules apply only once this repository has code that needs them.

## Status

Early stage: the Engine Sandbox is being built (first window, then commands, snapshots, models, materials, shaders, physics, UI, browser bridge). Do not implement anything beyond what is explicitly requested.

## Ecosystem context

Sibling repositories (cloned next to this one under `Retreat/`):

- `Retreat-CoreSDK` — shared math, data structures, utilities, common abstractions. Has no Retreat dependencies.
- `Retreat-Modules` — independent gameplay crates (inventory, combat, loot, ...).
- `Retreat-Forge` — builder UI: selects modules, resolves dependencies, builds the game, AI customization.
- `Retreat-Website` — web platform: catalog, accounts, rankings, marketplace.

Vision documents live in the `Retreat` repository: [FRAMEWORK.md](https://github.com/RetreatSoftware/Retreat/blob/main/FRAMEWORK.md) (engine/Forge) and [PLATFORM.md](https://github.com/RetreatSoftware/Retreat/blob/main/PLATFORM.md) (platform). Read `FRAMEWORK.md` before making architectural decisions.

## Decided technical direction

- **Language:** Rust. Performance is a hard requirement.
- **Rendering:** `wgpu`. Treat WebGL2 limits as the baseline; WebGPU-only features must be feature-gated.
- **Targets:** native (also used for the authoritative game server) and WebAssembly (`wasm32-unknown-unknown`) for the browser. Games run in the player's browser with no download.
- **Build:** Cargo workspace. Both native and WASM targets must build in CI from the start.
- **Module composition:** compile-time. Each gameplay module is an independent crate with a manifest (capabilities, dependencies, compatibility, configuration) declared in its `Cargo.toml` metadata. Forge resolves and validates; Cargo only compiles. Do not use Cargo features as the plugin mechanism.
- **No dynamic plugin loading, no custom scripting language** for now. Keep game logic data-driven and behind contract traits so a scripting layer stays possible later.
- **Authority:** the server owns game state. Games never mint currency or change balances; economic operations go through the platform adapter to trusted backend services.
- **Engine base: Bevy 0.20.** Do not write a custom ECS, renderer or asset system; extend Bevy instead.
- **Bevy is a direct dependency, pinned once per workspace.** The version lives in `[workspace.dependencies]` of the root `Cargo.toml`; crates declare `bevy.workspace = true`. Do not reach Bevy only through a re-export: Bevy's derive macros (`#[derive(Component)]`, ...) look for `bevy` in the using crate's own `Cargo.toml` and fail with "cannot find `bevy_ecs`" otherwise. Other repositories (Modules) pin the same version in their own workspace.
- **Contract crate `retreat_engine_api`**: the only engine crate modules may depend on. Holds the command layer (`CommandRegistry`, `run_command_line`, `CommandArgs`, `CommandError`, `AppCommandExt::register_command`) and `SimulationRng`.
- **Commands.** Ids are one word, `area.verb` (`assert.position`, later `physics.spawn_box`). A handler is `fn(&mut World, &CommandArgs) -> CommandResult`; it validates its arguments with `CommandArgs::expect_count` / `parse` so every error names the command, the bad value and the usage. Registering an id twice panics. Every entry point (scripts, UI, keys, browser bridge) calls `run_command_line`; never call feature logic directly from UI or input code.
- **Command scripts** (`--commands <file>`, `.cmds`): one command per line, `#` comments and blank lines cost no frame. When a script is loaded, time runs on `TimeUpdateStrategy::FixedTimesteps(1)` (exactly one fixed tick per app update, chosen over `ManualDuration` because it cannot drift from the fixed timestep) and the runner runs in `FixedUpdate`, one line per tick. Bevy's first update starts the clock and runs no tick. The first failing line logs `file:line: message` and exits with code 1. Without a script, time is real (interactive play).
- **Snapshots** (`snapshot <name>` → `target/snapshots/<name>.png`, relative to the working directory). Names are letters, digits, `_`, `-` only. The command starts the GPU capture and registers it in `PendingCommandWork`; the script holds until the PNG is written (or fails after `PENDING_WORK_TIMEOUT`, 30 s of wall-clock time: pending work runs in real time, and a tick lasts 3 ms on a GPU but 250 ms on a software renderer, so a tick-based timeout was too short on one and too long on the other). Any future command whose work finishes on a later frame uses `PendingCommandWork` the same way. Under a script, shaders compile synchronously (`RenderPlugin::synchronous_pipeline_compilation`), so a snapshot never catches a half-compiled scene.
- **Headless** (`--headless`): `DefaultPlugins` without `WinitPlugin`, `ScheduleRunnerPlugin::run_loop(0)`, no primary window, `ExitCondition::DontExit`. Cameras that target the primary window are redirected to one offscreen image (`HEADLESS_RESOLUTION` 1280x720, same as the default window) and marked `ShadowLodOrigin` (keeps Bevy's windowed shadow-LOD choice). Each headless frame waits for the GPU (`RenderDevice::poll`), because without a window nothing paces frames and a slow software GPU would fall minutes behind. Game code never checks headless; it spawns cameras as usual. Measured: windowed and headless PNGs are byte-identical on the same GPU, and repeat runs are byte-identical.
- **Software renderer** (`--software-renderer`): `WgpuSettings::force_fallback_adapter` (WARP on Windows, lavapipe on Linux). Used by CI. Pixels differ slightly from a hardware GPU, so compare PNGs only within one adapter; cross-platform determinism is checked with `state.hash` (later).
- **Models** (`model.spawn <name> <path> <x> <y> <z> <scale>`): loads glTF scene 0 as a `WorldAssetRoot` (Bevy 0.20 renamed `SceneRoot`/`Scene`/`SceneInstanceReady` to `WorldAssetRoot`/`WorldAsset`/`WorldInstanceReady`). Paths are relative to the app's `assets` folder (`sandbox/assets/` under `cargo run`, which sets `CARGO_MANIFEST_DIR`; running the `.exe` directly looks next to the exe). `..` is rejected; absolute paths are native-only and load through `load_builder().override_unapproved()`, because the engine sets `AssetPlugin::unapproved_path_mode = Deny`. The load is `PendingCommandWork`: finished on `WorldInstanceReady`, failed when `recursive_dependency_load_state` is `Failed`. The path rule is shared with textures: `asset_paths.rs` (`resolve_asset_location`, `load_asset`).
- **Materials** (`material.set_color <name> <r> <g> <b>`, sRGB 0–1; `material.set_texture <name> <path>`, PNG only, base color reset to white). Both act on the named entity **and its descendants** with a `StandardMaterial`, so `fox` recolors a whole model. **Copy on write:** each target gets its own edited copy of its material; shared materials (all instances of one glTF) are never edited in place. `set_texture` holds the script until the image loads.
- **Hot reload** (native only): the sandbox enables Bevy's `file_watcher` feature for non-wasm targets. Bevy 0.20 reloads a changed image but existing materials keep drawing the old GPU copy, even when marked changed; `refresh_materials_using_reloaded_images` gives each entity using a reloaded image a fresh copy of its material (new asset id). Checked by `scripts/verify/hot_reload_texture.sh` (swaps a file during the run, uses `texture.wait_for_reload`; fails without the fix). Revisit when Bevy refreshes materials itself.
- **Shader materials** (`shader.apply <name> <material>`): an app registers each custom `Material` type once under a short name with `AppShaderMaterialExt::register_shader_material::<M>(name, make)` (adds Bevy's `MaterialPlugin::<M>` and an entry in `ShaderMaterialRegistry`). `shader.apply` gives the named entity and every `Mesh3d` below it one new instance of that material, removing its previous material component (standard or another registered one: a mesh with two material components is drawn twice), and holds the script until the shader file loads. The shader path is read from `Material::fragment_shader`, which must return a file path. The sandbox registers `stripes` (`sandbox/src/stripes_material.rs`, `sandbox/assets/shaders/stripes.wesl`), animated by `globals.time`.
- **Shader files are `.wesl`** (WGSL plus `import`). Bevy 0.20 moved its own shaders to WESL and passes a plain `.wgsl` file to the GPU unprocessed: no imports, no `constants::MATERIAL_BIND_GROUP`. A `.wesl` file imports Bevy's modules (`import bevy_pbr::render::forward_io::VertexOutput;`) and binds material data at `@group(constants::MATERIAL_BIND_GROUP)`. WebGL2 baseline: uniform buffers only (no storage buffers, no `#[bindless]`).
- **A GPU pipeline that fails to compile fails the script** (`gpu_pipeline_errors.rs`): a render-world system scans `PipelineCache` for failed pipelines and passes the messages (with their causes) to the main world, which fails the running script against its last line. Without it Bevy only logs the error and the mesh disappears, so a check would pass. Pipelines compile a frame or two after the line that caused them, so the reported line may be one after the real cause.
- **Entity names are unique and namespaced.** Commands find entities by `Name` (`entity_names.rs`), and a lookup that finds two fails. Commands that create named entities call `ensure_name_is_free`. glTF nodes inside a model are renamed `<model>/<node>` (`namespaced_node_name`, e.g. `fox/b_Head_05`), because glTF files name nodes freely (Fox.glb has a node named `fox`).
- **Third-party assets** live under `sandbox/assets/` and each one is listed with its license in `sandbox/assets/CREDITS.md`. Prefer small CC0 files; CC-BY needs the credit line. `models/Fox.glb` (163 KB, Khronos sample, CC0 model + CC-BY 4.0 rig/conversion).
- **Pending work costs no tick.** While a script holds on `PendingCommandWork`, the `Last`-schedule system `hold_time_while_script_waits` pauses `Time<Virtual>`: updates keep loading assets and rendering, but no fixed tick runs and simulation time (physics, `globals.time` in shaders) does not advance. So the N-th line runs on the same tick and at the same simulation time on every machine, however long loading or capturing takes. The wall-clock timeout is checked there too. Closed the earlier gap where ticks kept running during a hold (needed once the animated shader made pictures depend on time).
- **Platform layer:** `crates/retreat_engine/src/platform.rs` is the only file with `cfg(target_arch = "wasm32")`; other code reads its constants (e.g. `HAS_FILE_SYSTEM`).
- **Randomness:** everything that changes game state draws from the `SimulationRng` resource, seeded by `--seed` (default `DEFAULT_SEED` = 0). Never create another RNG for simulation.
- **Cross-repository dependencies are git dependencies** on the public GitHub repositories (`retreat_rng = { git = "https://github.com/RetreatSoftware/Retreat-CoreSDK" }` in `[workspace.dependencies]`). `Cargo.lock` pins the commit; `cargo update -p retreat_rng` picks up a new CoreSDK commit. Revisit when crates need versions (private registry or crates.io).
- **Modules are Bevy plugins** (`impl Plugin`) with a manifest in `[package.metadata.retreat]`.
- **Physics: Rapier** (`bevy_rapier3d`) in a fixed timestep; Avian is the fallback.
- **UI: `bevy_ui`.** Buttons and keys dispatch command ids.
- **Models: glTF** through Bevy's `AssetServer`. **Shaders: WGSL syntax in `.wesl` files**, within WebGL2 limits.

## Layout

- `crates/retreat_engine_api/` — contract crate (public API for modules): command registry and parsing, `SimulationRng`.
- `crates/retreat_engine/` — engine runtime: `RetreatEnginePlugin` (Bevy `DefaultPlugins` + `RetreatCommandsPlugin`), `LaunchOptions` (`--commands`, `--seed`, `--headless`, `--software-renderer`), the command-script runner, `PendingCommandWork`, `model.spawn` (`model.rs`), material commands and hot-reload fix (`material.rs`), `shader.apply` and the shader material registry (`shader_material.rs`), shader compile errors to script failures (`gpu_pipeline_errors.rs`), asset path rule (`asset_paths.rs`), name lookups (`entity_names.rs`), built-in commands (`wait`, `quit`, `assert.entity_exists`, `assert.position`), `snapshot` (`snapshot.rs`), headless rendering (`headless.rs`), the platform layer (`platform.rs`). `RetreatCommandsPlugin` works without a window, so tests add it next to `MinimalPlugins`.
- `crates/retreat_engine/tests/assets/` — files the integration tests load (their `AssetPlugin::file_path`).
- `scripts/verify/*.cmds` — one command script per check; `scripts/verify/*.sh` for checks that must act outside the app during a run (hot reload), with their `.cmds` in a subfolder so the CI loop does not run them alone. CI runs every one with `--headless --software-renderer` and uploads `target/snapshots/` as the `snapshots` artifact. `commands_smoke.cmds` also runs as a sandbox unit test. Run them locally headless first (fastest loop) and open every PNG they write.
- `sandbox/` — crate `retreat_sandbox`, the Engine Sandbox app that exercises every engine feature. `sandbox/assets/` is its asset folder (`models/`, `textures/`, `shaders/`), credited in `sandbox/assets/CREDITS.md`.

## Open decisions (do not assume an answer)

- Netcode model (server-authoritative with prediction versus deterministic lockstep).
- Asset pipeline beyond glTF loading (import, compression, packaging).
- Distribution of crates across repositories beyond today's git dependencies (versioned releases, private registry or crates.io).
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

