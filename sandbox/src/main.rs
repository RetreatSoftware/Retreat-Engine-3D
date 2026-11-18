//! Retreat Engine Sandbox: the app used to see and verify engine features.
//!
//! Launch arguments: `--commands <script.cmds>` runs a command script, `--seed <n>` seeds
//! the simulation RNG (see `retreat_engine::LaunchOptions`).

mod scene;

use bevy::prelude::*;
use retreat_engine::{LaunchOptions, RetreatEnginePlugin};

const WINDOW_TITLE: &str = "Retreat Engine Sandbox";

/// Returns the app's exit status, so a failing command script fails the process.
fn main() -> AppExit {
    let engine_plugin = LaunchOptions::from_env_args()
        .and_then(|options| RetreatEnginePlugin::from_launch_options(WINDOW_TITLE, &options));
    let engine_plugin = match engine_plugin {
        Ok(plugin) => plugin,
        Err(message) => {
            eprintln!("retreat_sandbox: {message}");
            return AppExit::error();
        }
    };
    App::new()
        .add_plugins((engine_plugin, scene::SandboxScenePlugin))
        .run()
}
