# Retreat-Engine-3D

The 3D game engine of the Retreat ecosystem, responsible for rendering, the game loop, networking, and other fundamental runtime systems.

It supports integrating gameplay modules from [Retreat-Modules](https://github.com/RetreatSoftware/Retreat-Modules).

## Technical direction

- Written in Rust, rendering with `wgpu`.
- Browser-first: games compile to WebAssembly and run in the player's browser with no download. The same engine also builds natively, for example for authoritative game servers.
- Gameplay modules are independent crates composed at compile time.

This is the initial direction and is still subject to change. Status: early stage; the Engine Sandbox (`sandbox/`) is being built on Bevy.

## Part of the Retreat ecosystem

- [Retreat-CoreSDK](https://github.com/RetreatSoftware/Retreat-CoreSDK) — shared SDK
- [Retreat-Website](https://github.com/RetreatSoftware/Retreat-Website) — web platform
- [Retreat-Forge](https://github.com/RetreatSoftware/Retreat-Forge) — game creation tool
- [Retreat-Modules](https://github.com/RetreatSoftware/Retreat-Modules) — pluggable gameplay systems
