//! Retreat engine contract: the only engine crate that gameplay modules depend on.
//!
//! Commands: every user-visible action is a command with a unique id (`area.verb`),
//! registered in [`CommandRegistry`] and run through [`run_command_line`]. Command
//! scripts, UI buttons, keys and the browser bridge all dispatch through that one function,
//! which is what makes every feature drivable by an agent.
//!
//! Randomness: anything a command script can observe draws from [`SimulationRng`].
//!
//! Physics: an entity with a [`PhysicsBody`] becomes a rigid body; the types name no physics
//! library, so modules do not depend on one.

mod command_args;
mod command_error;
mod command_line;
mod command_registry;
mod physics_body;
mod simulation_rng;

pub use command_args::CommandArgs;
pub use command_error::CommandError;
pub use command_line::{ParsedCommandLine, parse_command_line};
pub use command_registry::{
    AppCommandExt, CommandOutcome, CommandRegistry, CommandResult, run_command_line,
};
pub use physics_body::{PhysicsBody, PhysicsBodyKind, PhysicsShape};
pub use simulation_rng::SimulationRng;
