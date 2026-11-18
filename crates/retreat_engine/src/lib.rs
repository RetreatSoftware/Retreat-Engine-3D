//! Retreat engine runtime.
//!
//! A game adds [`RetreatEnginePlugin`] to its Bevy `App`. The plugin owns the
//! base Bevy setup (window, rendering, input, assets) and the command layer
//! (registry, built-in commands, simulation RNG, command scripts), so games and
//! modules do not configure these themselves.

mod builtin_commands;
mod command_script;
mod launch_options;

use bevy::prelude::*;
use retreat_engine_api::{CommandRegistry, SimulationRng};

pub use builtin_commands::POSITION_TOLERANCE;
pub use command_script::CommandScript;
pub use launch_options::{DEFAULT_SEED, LaunchOptions};

/// Sets up the Bevy foundation every Retreat game runs on.
pub struct RetreatEnginePlugin {
    /// Title of the native window (ignored in the browser, where the page title is used).
    pub window_title: String,
    pub commands: RetreatCommandsPlugin,
}

impl RetreatEnginePlugin {
    /// No script, default seed: a normal interactive run.
    pub fn new(window_title: impl Into<String>) -> Self {
        Self {
            window_title: window_title.into(),
            commands: RetreatCommandsPlugin::default(),
        }
    }

    /// Applies `--commands` and `--seed`. Fails if the script file cannot be read.
    pub fn from_launch_options(
        window_title: impl Into<String>,
        options: &LaunchOptions,
    ) -> Result<Self, String> {
        let command_script = match &options.commands_file {
            Some(path) => Some(CommandScript::read_file(path)?),
            None => None,
        };
        Ok(Self {
            window_title: window_title.into(),
            commands: RetreatCommandsPlugin {
                seed: options.seed,
                command_script,
            },
        })
    }
}

impl Plugin for RetreatEnginePlugin {
    fn build(&self, app: &mut App) {
        let window_plugin = WindowPlugin {
            primary_window: Some(Window {
                title: self.window_title.clone(),
                ..default()
            }),
            ..default()
        };
        app.add_plugins(DefaultPlugins.set(window_plugin));
        self.commands.build(app);
    }
}

/// The command layer without any window or rendering: registry, built-in commands,
/// `SimulationRng` and the optional script. `RetreatEnginePlugin` includes it; tests add it
/// on its own next to Bevy's `MinimalPlugins`.
#[derive(Default)]
pub struct RetreatCommandsPlugin {
    pub seed: u64,
    pub command_script: Option<CommandScript>,
}

impl Plugin for RetreatCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CommandRegistry>()
            .insert_resource(SimulationRng::from_seed(self.seed));
        builtin_commands::register_builtin_commands(app);
        if let Some(script) = &self.command_script {
            app.add_plugins(command_script::CommandScriptPlugin {
                script: script.clone(),
            });
        }
    }
}
