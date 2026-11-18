//! Retreat engine runtime.
//!
//! A game adds [`RetreatEnginePlugin`] to its Bevy `App`. The plugin owns the
//! base Bevy setup (window or headless rendering, input, assets) and the command layer
//! (registry, built-in commands, simulation RNG, command scripts, snapshots), so games
//! and modules do not configure these themselves.

mod builtin_commands;
mod command_script;
mod headless;
mod launch_options;
mod pending_command_work;
mod platform;
mod snapshot;

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::render::settings::WgpuSettings;
use bevy::window::ExitCondition;
use bevy::winit::WinitPlugin;
use retreat_engine_api::{CommandRegistry, SimulationRng};

pub use builtin_commands::POSITION_TOLERANCE;
pub use command_script::{CommandScript, PENDING_WORK_TIMEOUT_TICKS};
pub use headless::HEADLESS_RESOLUTION;
pub use launch_options::{DEFAULT_SEED, LaunchOptions};
pub use pending_command_work::PendingCommandWork;
pub use snapshot::{SNAPSHOT_DIRECTORY, SnapshotSource, snapshot_file_path};

/// Sets up the Bevy foundation every Retreat game runs on.
pub struct RetreatEnginePlugin {
    /// Title of the native window (ignored in the browser, where the page title is used).
    pub window_title: String,
    /// No window; cameras render to an offscreen image (`--headless`).
    pub headless: bool,
    /// Use the CPU fallback GPU adapter (`--software-renderer`).
    pub software_renderer: bool,
    pub commands: RetreatCommandsPlugin,
}

impl RetreatEnginePlugin {
    /// A window, the real GPU, no script, default seed: a normal interactive run.
    pub fn new(window_title: impl Into<String>) -> Self {
        Self {
            window_title: window_title.into(),
            headless: false,
            software_renderer: false,
            commands: RetreatCommandsPlugin::default(),
        }
    }

    /// Applies every launch option. Fails if the script file cannot be read.
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
            headless: options.headless,
            software_renderer: options.software_renderer,
            commands: RetreatCommandsPlugin {
                seed: options.seed,
                command_script,
            },
        })
    }

    fn render_plugin(&self) -> RenderPlugin {
        RenderPlugin {
            render_creation: WgpuSettings {
                force_fallback_adapter: self.software_renderer,
                ..default()
            }
            .into(),
            // Background shader compilation finishes after a varying number of frames, so a
            // script's snapshot could catch a half-drawn scene. Under a script, wait instead.
            synchronous_pipeline_compilation: self.commands.command_script.is_some(),
            ..default()
        }
    }
}

impl Plugin for RetreatEnginePlugin {
    fn build(&self, app: &mut App) {
        if self.headless {
            let window_plugin = WindowPlugin {
                primary_window: None,
                // With no window to close, only `quit` (AppExit) ends the app.
                exit_condition: ExitCondition::DontExit,
                ..default()
            };
            app.add_plugins(
                DefaultPlugins
                    .set(window_plugin)
                    .set(self.render_plugin())
                    // Winit runs the window event loop; there is no window.
                    .disable::<WinitPlugin>(),
            );
            // Replaces winit's loop: run frames back to back, with no pause between them.
            // (`DefaultPlugins` only contains this plugin when built without winit.)
            app.add_plugins(ScheduleRunnerPlugin::run_loop(Duration::ZERO));
            app.add_plugins(headless::HeadlessRenderPlugin);
        } else {
            let window_plugin = WindowPlugin {
                primary_window: Some(Window {
                    title: self.window_title.clone(),
                    ..default()
                }),
                ..default()
            };
            app.add_plugins(DefaultPlugins.set(window_plugin).set(self.render_plugin()));
            app.insert_resource(SnapshotSource::PrimaryWindow);
        }
        self.commands.build(app);
        app.add_plugins(snapshot::SnapshotPlugin);
    }
}

/// The command layer without any window or rendering: registry, built-in commands,
/// pending-work tracking, `SimulationRng` and the optional script. `RetreatEnginePlugin`
/// includes it; tests add it on its own next to Bevy's `MinimalPlugins`.
#[derive(Default)]
pub struct RetreatCommandsPlugin {
    pub seed: u64,
    pub command_script: Option<CommandScript>,
}

impl Plugin for RetreatCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CommandRegistry>()
            .init_resource::<PendingCommandWork>()
            .insert_resource(SimulationRng::from_seed(self.seed));
        builtin_commands::register_builtin_commands(app);
        if let Some(script) = &self.command_script {
            app.add_plugins(command_script::CommandScriptPlugin {
                script: script.clone(),
            });
        }
    }
}
