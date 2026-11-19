//! Retreat engine runtime.
//!
//! A game adds [`RetreatEnginePlugin`] to its Bevy `App`. The plugin owns the
//! base Bevy setup (window or headless rendering, input, assets) and the command layer
//! (registry, built-in commands, simulation RNG, command scripts, snapshots), so games
//! and modules do not configure these themselves.

mod animation;
mod asset_paths;
mod audio;
mod bridge_status;
mod builtin_commands;
mod camera;
mod command_dispatch;
mod command_script;
mod entity_names;
mod gpu_pipeline_errors;
mod headless;
mod input_actions;
mod launch_options;
mod material;
mod model;
mod pending_command_work;
mod physics;
mod physics_snapshot;
mod platform;
mod shader_material;
mod snapshot;
mod state_hash;
mod storage;
mod ui_panels;

use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::UnapprovedPathMode;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::render::settings::WgpuSettings;
use bevy::window::ExitCondition;
use bevy::winit::WinitPlugin;
use retreat_engine_api::{CommandRegistry, SimulationRng};

pub use animation::AnimationCommandsPlugin;
pub use asset_paths::{AssetLocation, resolve_asset_location};
pub use audio::{
    AudioCommandsPlugin, SOUND_FILE_EXTENSIONS, add_sound_assets_without_audio_device,
};
pub use bridge_status::script_status_json;
pub use builtin_commands::POSITION_TOLERANCE;
pub use camera::GameCameraPlugin;
pub use command_dispatch::dispatch_command_line;
pub use command_script::{
    CommandScript, CommandScriptSystems, PENDING_WORK_TIMEOUT, ScriptProgress,
};
pub use headless::HEADLESS_RESOLUTION;
pub use input_actions::InputActionsPlugin;
pub use launch_options::{DEFAULT_SEED, LaunchOptions};
pub use material::{MaterialCommandsPlugin, TEXTURE_FILE_EXTENSIONS};
pub use model::{
    MODEL_FILE_EXTENSIONS, MODEL_NODE_NAME_SEPARATOR, ModelCommandsPlugin, namespaced_node_name,
};
pub use pending_command_work::PendingCommandWork;
pub use physics::{
    PHYSICS_PROXY_NAME_SUFFIX, PhysicsBox, PhysicsPlugin, PhysicsProxy, PhysicsProxyOf,
    spawn_physics_box,
};
pub use physics_snapshot::{PhysicsSnapshot, capture_physics, restore_physics};
pub use platform::launch;
pub use shader_material::{
    AppShaderMaterialExt, ShaderMaterialCommandsPlugin, ShaderMaterialRegistry,
};
pub use snapshot::{SNAPSHOT_DIRECTORY, SnapshotSource, snapshot_file_path};
pub use state_hash::{
    LastStateHash, StateHashPlugin, compute_state_hash, format_state_hash, parse_state_hash,
};
pub use storage::StorageCommandsPlugin;
pub use ui_panels::UiPanelsPlugin;

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

    /// Applies every launch option. `command_script` is the already loaded `--commands` /
    /// `?commands=` script: the platform layer reads it (a file natively, an HTTP fetch in the
    /// browser) because only it knows how.
    pub fn from_launch_options(
        window_title: impl Into<String>,
        options: &LaunchOptions,
        command_script: Option<CommandScript>,
    ) -> Self {
        Self {
            window_title: window_title.into(),
            headless: options.headless,
            software_renderer: options.software_renderer,
            commands: RetreatCommandsPlugin {
                seed: options.seed,
                command_script,
            },
        }
    }

    /// Assets load from the app's `assets` folder. Paths outside it are refused unless a
    /// command explicitly overrides that (only `model.spawn` with an absolute path does).
    fn asset_plugin(&self) -> AssetPlugin {
        AssetPlugin {
            unapproved_path_mode: UnapprovedPathMode::Deny,
            meta_check: platform::asset_meta_check(),
            ..default()
        }
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
                    .set(self.asset_plugin())
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
                primary_window: Some(platform::primary_window(&self.window_title)),
                ..default()
            };
            app.add_plugins(
                DefaultPlugins
                    .set(window_plugin)
                    .set(self.asset_plugin())
                    .set(self.render_plugin()),
            );
            app.insert_resource(SnapshotSource::PrimaryWindow);
        }
        self.commands.build(app);
        app.add_plugins((
            snapshot::SnapshotPlugin,
            ModelCommandsPlugin,
            MaterialCommandsPlugin,
            ShaderMaterialCommandsPlugin,
            gpu_pipeline_errors::GpuPipelineErrorsPlugin,
            PhysicsPlugin,
            StateHashPlugin,
            InputActionsPlugin,
            GameCameraPlugin,
            AudioCommandsPlugin,
            AnimationCommandsPlugin,
            StorageCommandsPlugin,
            UiPanelsPlugin,
            platform::PlatformPlugin,
        ));
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
