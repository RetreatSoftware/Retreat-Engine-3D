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
scripts/verify/hot_reload_texture.sh
```

The second line runs a command script (one command per frame) and exits with code 0 if every line passed.
The third renders without a window and writes `target/snapshots/sandbox_basic.png`.
The fourth loads a glTF model (`sandbox/assets/models/Fox.glb`) and writes `target/snapshots/model_spawn.png`.
The fifth changes colors and textures (`material_before.png`, `material_after.png`); the last checks texture hot reload.
Add `--software-renderer` to render on the CPU (no GPU needed).

Third-party assets and their licenses are listed in [sandbox/assets/CREDITS.md](sandbox/assets/CREDITS.md).

## Part of the Retreat ecosystem

- [Retreat-CoreSDK](https://github.com/RetreatSoftware/Retreat-CoreSDK) — shared SDK
- [Retreat-Website](https://github.com/RetreatSoftware/Retreat-Website) — web platform
- [Retreat-Forge](https://github.com/RetreatSoftware/Retreat-Forge) — game creation tool
- [Retreat-Modules](https://github.com/RetreatSoftware/Retreat-Modules) — pluggable gameplay systems
