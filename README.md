# Retreat-Engine-3D

The 3D game engine of the Retreat ecosystem, responsible for rendering, the game loop, networking, and other fundamental runtime systems.

It supports integrating gameplay modules from [Retreat-Modules](https://github.com/RetreatSoftware/Retreat-Modules).

## Technical direction

- Written in Rust, rendering with `wgpu`.
- Browser-first: games compile to WebAssembly and run in the player's browser with no download. The same engine also builds natively, for example for authoritative game servers.
- Gameplay modules are independent crates composed at compile time.

This is the initial direction and is still subject to change. Status: early stage; the Engine Sandbox (`sandbox/`) is being built on Bevy.

## Run the sandbox

```bash
cargo run -p retreat_sandbox
cargo run -p retreat_sandbox -- --commands scripts/verify/commands_smoke.cmds --seed 0
cargo run -p retreat_sandbox -- --headless --commands scripts/verify/sandbox_basic.cmds
cargo run -p retreat_sandbox -- --headless --commands scripts/verify/model_spawn.cmds
cargo run -p retreat_sandbox -- --headless --commands scripts/verify/material_change.cmds
cargo run -p retreat_sandbox -- --headless --commands scripts/verify/shader_stripes.cmds
cargo run -p retreat_sandbox -- --headless --commands scripts/verify/physics_boxes.cmds
cargo run -p retreat_sandbox -- --headless --commands scripts/verify/toolbar.cmds
cargo run -p retreat_sandbox -- --headless --commands scripts/verify/module_spawner.cmds
scripts/verify/hot_reload_texture.sh
```

The second line runs a command script (one command per frame) and exits with code 0 if every line passed.
The third renders without a window and writes `target/snapshots/sandbox_basic.png`.
The fourth loads a glTF model (`sandbox/assets/models/Fox.glb`) and writes `target/snapshots/model_spawn.png`.
The fifth changes colors and textures (`material_before.png`, `material_after.png`); the last checks texture hot reload.
Add `--software-renderer` to render on the CPU (no GPU needed).

Third-party assets and their licenses are listed in [sandbox/assets/CREDITS.md](sandbox/assets/CREDITS.md).


## Run the sandbox in the browser

Needs the `wasm32-unknown-unknown` target, `wasm-bindgen-cli` at the version `Cargo.lock` pins
(`cargo install wasm-bindgen-cli --version 0.2.130 --locked`), Python (to serve) or Node 22+
(to run scripts headless), and Chrome or Edge.

```bash
scripts/web/build.sh
python -m http.server 8090 --directory target/web
node scripts/web/run_in_browser.mjs scripts/verify/physics_boxes.cmds
```

Open `http://localhost:8090/` to play, or `http://localhost:8090/?commands=scripts/verify/physics_boxes.cmds`
to watch a script. In the browser console, `__retreat.command("sandbox.drop_box")` runs a command and
`__retreat.status()` reports the script.

## Play over the local network

```bash
scripts/web/build.sh
cargo run -p retreat_sandbox -- --headless --no-render --server 5000 --serve-web target/web
```

The server prints the address to open on other devices (`https://<this computer>:8443/`). The browser
warns about the server's own certificate: choose Advanced, then Proceed. On Windows, allow the app
through the firewall on private networks.

## Part of the Retreat ecosystem

- [Retreat-CoreSDK](https://github.com/RetreatSoftware/Retreat-CoreSDK) — shared SDK
- [Retreat-Website](https://github.com/RetreatSoftware/Retreat-Website) — web platform
- [Retreat-Forge](https://github.com/RetreatSoftware/Retreat-Forge) — game creation tool
- [Retreat-Modules](https://github.com/RetreatSoftware/Retreat-Modules) — pluggable gameplay systems
