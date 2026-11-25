//! The single integration-test binary of `retreat_engine`.
//!
//! Why one binary: Cargo builds every file directly under `tests/` as its own executable, and
//! each executable statically links the whole engine plus Bevy, wgpu and Rapier. Ten test files
//! meant ten links and ten debug-symbol (.pdb) files of ~1 GB each. Here Cargo finds only
//! `tests/engine/main.rs` (it ignores the other files in this folder), so there is one link.
//!
//! To add a test file: create `tests/engine/<name>.rs` and add `mod <name>;` below. Do NOT put a
//! new `.rs` file directly in `tests/`. Each module has its own scope, so helpers such as
//! `run_script` and `MAX_UPDATES` can repeat across modules.

mod audio_commands;
mod camera_commands;
mod command_script_runner;
mod input_actions_commands;
mod material_commands;
mod model_commands;
mod net_character;
mod net_harness;
mod net_module_state;
mod net_motion;
mod net_pacing;
mod net_server_limits;
mod net_webtransport;
mod perf_commands;
mod physics_commands;
mod physics_rollback;
mod shader_material_commands;
mod storage_commands;
mod ui_commands;
