//! Retreat engine runtime.
//!
//! A game adds [`RetreatEnginePlugin`] to its Bevy `App`. The plugin owns the
//! base Bevy setup (window or headless rendering, input, assets) and the command layer
//! (registry, built-in commands, simulation RNG, command scripts, snapshots), so games
//! and modules do not configure these themselves.

mod analog_input;
mod animation;
mod asset_paths;
mod audio;
mod bridge_status;
mod builtin_commands;
mod camera;
mod character_stepping;
mod command_dispatch;
mod command_script;
mod entity_names;
mod gpu_pipeline_errors;
mod headless;
mod input_actions;
mod intangibility;
mod launch_options;
mod material;
mod model;
mod motion_record;
pub mod net;
mod net_pose;
mod once_queue;
mod pending_command_work;
mod perf;
mod physics;
mod physics_queries;
mod physics_snapshot;
mod platform;
mod render_smoothing;
mod retired_bodies;
mod shader_material;
mod snapshot;
mod sound_events;
mod state_hash;
mod storage;
mod ui_hud;
mod ui_panels;
mod visual_effects;
mod visuals;

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::UnapprovedPathMode;
use bevy::audio::{AudioPlugin, Volume};
use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::render::settings::WgpuSettings;
use bevy::window::ExitCondition;
use bevy::winit::{WinitPlugin, WinitSettings};
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
    CommandScript, CommandScriptSystems, PENDING_WORK_TIMEOUT, RealTimeScript, ScriptProgress,
};
pub use headless::HEADLESS_RESOLUTION;
pub use input_actions::InputActionsPlugin;
pub use launch_options::{DEFAULT_SEED, LaunchOptions, NetLaunch, ServerOffer, WebServeLaunch};
pub use material::{MaterialCommandsPlugin, TEXTURE_FILE_EXTENSIONS};
pub use model::{
    MODEL_FILE_EXTENSIONS, MODEL_NODE_NAME_SEPARATOR, ModelCommandsPlugin, namespaced_node_name,
};
pub use motion_record::{
    MotionRecordPlugin, MotionRecordings, MotionSample, MotionStats, motion_stats,
};
pub use net_pose::NetPosePlugin;
pub use pending_command_work::PendingCommandWork;
pub use perf::{
    FRAME_TIME_WINDOW, FrameTimeSummary, FrameTimes, MIN_FRAMES_FOR_ASSERT, PerfCommandsPlugin,
    SceneCounts, count_scene,
};
pub use physics::{
    PHYSICS_PROXY_NAME_SUFFIX, PhysicsBox, PhysicsPlugin, PhysicsProxy, PhysicsProxyOf,
    spawn_body_from_description, spawn_physics_box,
};
pub use physics_snapshot::{PhysicsSnapshot, RollbackError, capture_physics, restore_physics};
pub use platform::launch;
#[cfg(not(target_arch = "wasm32"))]
pub use platform::{WebTransportClient, WebTransportServer, parse_certificate_hash};
pub use render_smoothing::RenderSmoothingPlugin;
pub use retired_bodies::ROLLBACK_WINDOW_TICKS;
pub use shader_material::{
    AppShaderMaterialExt, ShaderMaterialCommandsPlugin, ShaderMaterialRegistry,
};
pub use snapshot::{SNAPSHOT_DIRECTORY, SnapshotSource, snapshot_file_path};
pub use sound_events::{SOUND_EVENT_NAME_PREFIX, SoundEventLog, SoundEventsPlugin};
pub use state_hash::{
    LastStateHash, StateHashPlugin, compute_state_hash, format_state_hash, parse_state_hash,
};
pub use storage::StorageCommandsPlugin;
pub use ui_hud::UiHudPlugin;
pub use ui_panels::UiPanelsPlugin;
pub use visual_effects::{EFFECT_NAME_PREFIX, EffectLog, VisualEffectsPlugin};
pub use visuals::{VISUAL_NAME_SUFFIX, VisualsPlugin};

/// Sets up the Bevy foundation every Retreat game runs on.
pub struct RetreatEnginePlugin {
    /// Title of the native window (ignored in the browser, where the page title is used).
    pub window_title: String,
    /// No window; cameras render to an offscreen image (`--headless`).
    pub headless: bool,
    /// Headless without any GPU or drawing (`--no-render`).
    pub no_render: bool,
    /// Use the CPU fallback GPU adapter (`--software-renderer`).
    pub software_renderer: bool,
    /// `--real-time`: scripts run in real time (see `RealTimeScript`).
    pub real_time: bool,
    /// Mute the sound device (`LaunchOptions::sound_muted`); logical audio state is unchanged.
    pub sound_muted: bool,
    pub commands: RetreatCommandsPlugin,
}

impl RetreatEnginePlugin {
    /// A window, the real GPU, no script, default seed: a normal interactive run.
    pub fn new(window_title: impl Into<String>) -> Self {
        Self {
            window_title: window_title.into(),
            headless: false,
            no_render: false,
            software_renderer: false,
            real_time: false,
            sound_muted: false,
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
            no_render: options.no_render,
            software_renderer: options.software_renderer,
            real_time: options.real_time,
            sound_muted: options.sound_muted(),
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

    /// Bevy's audio plugin; muted means global volume 0, so sounds still load, have a length and
    /// end on the fixed clock, but nothing reaches the speakers (`audio.rs` applies it to every volume).
    fn audio_plugin(&self) -> AudioPlugin {
        AudioPlugin {
            global_volume: if self.sound_muted {
                GlobalVolume::new(Volume::SILENT)
            } else {
                GlobalVolume::default()
            },
            ..default()
        }
    }

    /// Without a renderer Bevy's camera plugin logs an error that means nothing (it looks for the
    /// render sub-app), so that one source is silenced then.
    fn log_plugin(&self) -> LogPlugin {
        let mut plugin = LogPlugin::default();
        if self.no_render {
            plugin
                .filter
                .push_str(",bevy_extract::extract_resource=off");
        }
        plugin
    }

    fn render_plugin(&self) -> RenderPlugin {
        RenderPlugin {
            render_creation: WgpuSettings {
                force_fallback_adapter: self.software_renderer,
                // No backends: Bevy starts no GPU and no render sub-app (`--no-render`).
                backends: if self.no_render {
                    None
                } else {
                    WgpuSettings::default().backends
                },
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
                    .set(self.audio_plugin())
                    .set(self.log_plugin())
                    // Winit runs the window event loop; there is no window.
                    .disable::<WinitPlugin>(),
            );
            // Replaces winit's loop: frames run back to back under a deterministic script, and
            // about once per tick otherwise (see `headless_frame_wait`).
            // (`DefaultPlugins` only contains this plugin when built without winit.)
            let wait = LaunchOptions::headless_frame_wait(
                self.commands.command_script.is_some(),
                self.real_time,
            );
            app.add_plugins(ScheduleRunnerPlugin::run_loop(wait));
            if !self.no_render {
                app.add_plugins(headless::HeadlessRenderPlugin);
            }
        } else {
            let window_plugin = WindowPlugin {
                primary_window: Some(platform::primary_window(&self.window_title)),
                ..default()
            };
            app.add_plugins(
                DefaultPlugins
                    .set(window_plugin)
                    .set(self.asset_plugin())
                    .set(self.render_plugin())
                    .set(self.audio_plugin()),
            );
            app.insert_resource(SnapshotSource::PrimaryWindow);
            // A window that is not focused keeps drawing at full rate (Bevy's default caps it at
            // 60 per second with uneven gaps): with two clients on one machine, the one being
            // watched is usually not the one being played. Vsync still limits the rate.
            app.insert_resource(WinitSettings::continuous());
        }
        if self.real_time {
            app.insert_resource(RealTimeScript);
        }
        self.commands.build(app);
        app.add_plugins((
            snapshot::SnapshotPlugin,
            ModelCommandsPlugin,
            MaterialCommandsPlugin,
            ShaderMaterialCommandsPlugin,
            PhysicsPlugin,
            StateHashPlugin,
            InputActionsPlugin,
            GameCameraPlugin,
            AudioCommandsPlugin,
            AnimationCommandsPlugin,
            StorageCommandsPlugin,
            net::commands::NetCommandsPlugin,
            UiPanelsPlugin,
            platform::PlatformPlugin,
        ));
        // Bevy's plugin tuples hold at most 15 entries, so this one has its own call.
        app.add_plugins((
            PerfCommandsPlugin,
            RenderSmoothingPlugin,
            VisualsPlugin,
            UiHudPlugin,
            MotionRecordPlugin,
            NetPosePlugin,
            VisualEffectsPlugin,
            SoundEventsPlugin,
        ));
        // Scans the render world for failed pipelines, so it needs one.
        if !self.no_render {
            app.add_plugins(gpu_pipeline_errors::GpuPipelineErrorsPlugin);
        }
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
