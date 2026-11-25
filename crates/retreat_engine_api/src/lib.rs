//! Retreat engine contract: the only engine crate that gameplay modules depend on.
//!
//! Commands: every user-visible action is a command with a unique id (`area.verb`),
//! registered in [`CommandRegistry`] and run through [`run_command_line`]. Command
//! scripts, UI buttons, keys and the browser bridge all dispatch through that one function,
//! which is what makes every feature drivable by an agent.
//!
//! Randomness: anything a command script can observe draws from [`SimulationRng`].
//!
//! Input: named actions bound to keys in data ([`InputAction`], [`InputActions`]); a key press runs
//! the action's command, and [`InputActions::is_held`] serves continuous input.
//!
//! Analog input: [`AnalogInput`] (mouse look and axes per fixed tick) and [`CursorCapture`].
//!
//! Camera: an entity with a [`GameCamera`] is a perspective camera; the engine maps it onto
//! Bevy's camera components, modules only edit the description.
//!
//! Audio: an entity with a [`SoundPlayback`] is a sound that plays; whether it is playing is
//! decided by simulation time, so scripts can assert it without hearing it.
//!
//! Animation: an entity with an [`AnimationPlayback`] plays the listed glTF clips (one clip, or a
//! weighted blend) on simulation time; the engine maps it onto Bevy's animation.
//!
//! Networking (what a module sees): [`NetCommandSender`], [`NetClientJoined`], [`NetClientLeft`],
//! [`NetAllowedCommands`], [`NetInputQueue`].
//!
//! Module state: a component that implements [`NetState`] and is registered with
//! [`AppNetStateExt::register_net_state`] is rolled back and replicated.
//!
//! Saving: [`SaveStore`] keeps one versioned JSON document per module; the engine decides where
//! it lives (file, browser storage, memory).
//!
//! UI: [`UiPanel`] declares a panel of buttons that run command lines; its layout is the pure
//! [`compute_panel_layout`].
//!
//! Physics: an entity with a [`PhysicsBody`] becomes a rigid body; the types name no physics
//! library, so modules do not depend on one. [`PhysicsVelocity`] reads a body's velocity,
//! [`cast_ray`] and [`cast_shape`] query the bodies, and systems that drive bodies run in
//! [`PhysicsControlSystems`].

mod analog_input;
mod animation_playback;
mod command_args;
mod command_error;
mod command_line;
mod command_registry;
mod game_camera;
mod input_actions;
mod net;
mod net_state;
mod physics_body;
mod physics_queries;
mod save_store;
mod simulation_rng;
mod sound_playback;
mod ui_panel;

pub use analog_input::{AnalogInput, AppAxisExt, CursorCapture, InputAxis};
pub use animation_playback::{AnimationPlayback, ClipPlayback, MAX_CLIP_SPEED};
pub use command_args::CommandArgs;
pub use command_error::CommandError;
pub use command_line::{ParsedCommandLine, parse_command_line};
pub use command_registry::{
    AppCommandExt, CommandOutcome, CommandRegistry, CommandResult, run_command_line,
};
pub use game_camera::{
    CameraProjection, DEFAULT_FOV_DEGREES, GameCamera, MAX_FOV_DEGREES, MIN_FOV_DEGREES,
};
pub use input_actions::{
    AppInputExt, HELD_ONLY, INPUT_KEY_NAMES, InputAction, InputActionSystems, InputActions,
    InputKey,
};
pub use net::{
    AcceptEveryone, AppNetExt, NetAllowedCommands, NetClientJoined, NetClientLeft,
    NetCommandSender, NetIdentity, NetIdentityCheck, NetIdentityDecision, NetIdentityRequest,
    NetInputQueue, NetLocalClient, NetPlayerIds, NetReplaying, Replicated, SyncedKind,
    add_net_contract,
};
pub use net_state::{
    AppNetStateExt, NetState, NetStateRegistry, NetStateType, RemoveNetEntity, RetiredEntity,
    decode_net_state, encode_net_state,
};
pub use physics_body::{
    PhysicsBody, PhysicsBodyKind, PhysicsControlSystems, PhysicsShape, PhysicsVelocity,
    PhysicsVelocityChange, PhysicsVelocitySet, RemovePhysicsBody,
};
pub use physics_queries::{CastHit, PhysicsCaster, RayCast, ShapeCast, cast_ray, cast_shape};
pub use save_store::{
    MemorySaveBackend, SAVE_FORMAT_VERSION, SaveBackend, SaveError, SaveStore, SavedData,
    decode_save, encode_save, is_valid_module_id, serde_json,
};
pub use simulation_rng::SimulationRng;
pub use sound_playback::{MAX_SOUND_VOLUME, SoundKind, SoundPlayback};
pub use ui_panel::{
    AppUiExt, BUTTON_HEIGHT, ButtonRect, COLUMN_BUTTON_WIDTH, MIN_BUTTON_HEIGHT, PANEL_GAP,
    PANEL_MARGIN, PanelAnchor, PanelDirection, PanelLayout, PanelRect, ROW_MAX_BUTTON_WIDTH,
    TITLE_HEIGHT, UiButtonEntry, UiPanel, UiPanels, compute_panel_layout, visible_entries,
};
