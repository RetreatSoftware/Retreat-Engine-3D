//! Native (desktop, server, CI) side of the platform layer. See `mod.rs` for the contract.

use bevy::asset::AssetMetaCheck;
use bevy::prelude::*;

use crate::snapshot::snapshot_file_path;
use crate::{CommandScript, LaunchOptions, RetreatEnginePlugin};

/// Native audio output starts at once.
pub(crate) const AUDIO_NEEDS_USER_GESTURE: bool = false;

pub(crate) const HAS_FILE_SYSTEM: bool = true;

pub(crate) const SCRIPT_END_EXITS_APP: bool = true;

/// Reads the process arguments, reads the `--commands` file, builds the app with
/// `RetreatEnginePlugin` and the game's plugins, and runs it. Returns the app's exit status
/// (code 1 when a script fails), so `fn main() -> AppExit` passes it to the shell.
pub fn launch(window_title: &str, add_game_plugins: fn(&mut App)) -> AppExit {
    let engine = LaunchOptions::from_env_args().and_then(|options| {
        let script = match &options.commands_file {
            Some(path) => Some(CommandScript::read_file(std::path::Path::new(path))?),
            None => None,
        };
        Ok(RetreatEnginePlugin::from_launch_options(
            window_title,
            &options,
            script,
        ))
    });
    let engine = match engine {
        Ok(engine) => engine,
        Err(message) => {
            eprintln!("{window_title}: {message}");
            return AppExit::error();
        }
    };
    let mut app = App::new();
    app.add_plugins(engine);
    add_game_plugins(&mut app);
    app.run()
}

/// A normal desktop window with the given title.
pub(crate) fn primary_window(title: &str) -> Window {
    Window {
        title: title.to_string(),
        ..default()
    }
}

/// Native reads `.meta` files next to assets when they exist (Bevy's default).
pub(crate) fn asset_meta_check() -> AssetMetaCheck {
    AssetMetaCheck::Always
}

/// Writes `png` to `target/snapshots/<name>.png` (relative to the working directory) and
/// returns that path.
pub(crate) fn store_snapshot(name: &str, png: Vec<u8>) -> Result<String, String> {
    let path = snapshot_file_path(name)?;
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|error| {
            format!(
                "cannot create snapshot directory `{}`: {error}",
                directory.display()
            )
        })?;
    }
    std::fs::write(&path, png)
        .map_err(|error| format!("cannot write snapshot `{}`: {error}", path.display()))?;
    Ok(path.display().to_string())
}

/// Native has no platform-only systems.
pub(crate) struct PlatformPlugin;

impl Plugin for PlatformPlugin {
    fn build(&self, _app: &mut App) {}
}
