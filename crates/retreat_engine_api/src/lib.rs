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
//! Camera: an entity with a [`GameCamera`] is a perspective camera; the engine maps it onto
//! Bevy's camera components, modules only edit the description.
//!
//! Audio: an entity with a [`SoundPlayback`] is a sound that plays; whether it is playing is
//! decided by simulation time, so scripts can assert it without hearing it.
//!
//! Physics: an entity with a [`PhysicsBody`] becomes a rigid body; the types name no physics
//! library, so modules do not depend on one.

mod command_args;
mod command_error;
mod command_line;
mod command_registry;
mod game_camera;
mod input_actions;
mod physics_body;
mod simulation_rng;
mod sound_playback;

pub use command_args::CommandArgs;
pub use command_error::CommandError;
pub use command_line::{ParsedCommandLine, parse_command_line};
pub use command_registry::{
    AppCommandExt, CommandOutcome, CommandRegistry, CommandResult, run_command_line,
};
pub use game_camera::{DEFAULT_FOV_DEGREES, GameCamera, MAX_FOV_DEGREES, MIN_FOV_DEGREES};
pub use input_actions::{
    AppInputExt, HELD_ONLY, INPUT_KEY_NAMES, InputAction, InputActionSystems, InputActions,
    InputKey,
};
pub use physics_body::{PhysicsBody, PhysicsBodyKind, PhysicsShape};
pub use simulation_rng::SimulationRng;
pub use sound_playback::{MAX_SOUND_VOLUME, SoundKind, SoundPlayback};
