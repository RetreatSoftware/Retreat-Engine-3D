//! Networking core: server-authoritative simulation with client-side prediction (FRAMEWORK.md §12).
//!
//! **Model.** Server and clients run the same fixed-tick simulation. A *tick number* labels the n-th
//! simulated tick on every machine. A client starts `round trip + margin` ticks ahead of the server, so
//! an input it makes for tick `n` reaches the server before the server simulates tick `n` (if the link
//! allows; `server.rs` counts inputs that arrive late). Inputs are command lines (everything is a
//! command), run just before the tick; the server only runs commands it has been told to allow.
//!
//! **Where the code lives.** The server and the client are resources plus systems inside the simulation's
//! own `World` (`server.rs`, `client.rs`: `install_server`, `install_client`), running in `FixedFirst` and
//! `FixedLast`, so a real app and the in-process harness (`harness.rs`, driven by `simulate_tick`) run the
//! same code. Messages are encoded by `wire.rs` (a version byte plus postcard) and move over a transport
//! (`transport.rs`: the traits and an in-process loopback; native WebTransport lives in the platform layer).
//!
//! **Prediction.** A client applies its own inputs at once and keeps a history (physics snapshot, body
//! states and inputs) per tick. When the server's state for tick `s` arrives, the client compares it with
//! its own state at `s`. On a mismatch it restores its snapshot of `s`, writes the server's body states
//! over it, creates bodies it did not know, and replays its inputs for ticks `s+1..` up to its present.
//! The server is always right; clients only guess ahead.
//!
//! **Limits.** Bodies created from server state start with zero velocity until the next correction; a body
//! despawned directly inside the history window refuses the rollback (remove bodies with
//! `RemovePhysicsBody`, see `retired_bodies.rs`);
//! a state message must fit one datagram (about 1200 bytes).

pub mod body_state;
pub mod client;
pub mod commands;
pub mod harness;
pub mod limits;
pub mod link;
pub mod server;
pub mod state_delta;
pub mod transport;
pub mod wire;

use bevy::app::FixedMain;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use body_state::BodyState;
use serde::{Deserialize, Serialize};

/// Largest distance (world units) between predicted and server positions that still counts as
/// "the same".
pub const POSITION_TOLERANCE: f32 = 1.0e-3;
/// Largest velocity difference that still counts as "the same".
pub const VELOCITY_TOLERANCE: f32 = 1.0e-2;
/// Most bodies in one state message: a body takes 85 to 100 bytes (more with very long names), a datagram
/// holds about 1200, so 8 leaves room for names up to about 40 characters.
pub const MAX_BODIES_PER_STATE_MESSAGE: usize = 8;
/// A client keeps this many ticks of history at most.
pub const MAX_HISTORY_TICKS: u64 = crate::ROLLBACK_WINDOW_TICKS;

/// What a client sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Asks to join; the server answers with `Welcome` carrying the same `nonce`. `token` is whatever
    /// the platform gave the client to prove who it is; only the server's `NetIdentityCheck` reads it.
    Hello { nonce: u32, token: Option<String> },
    /// Command lines to run at server tick `tick`. (Who sent it is the connection, never a field the
    /// client could forge.)
    Input { tick: u64, lines: Vec<String> },
}

/// What the server sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// The answer to `Hello`: the client's id and the server's last simulated tick.
    Welcome {
        nonce: u32,
        client_id: u32,
        tick: u64,
    },
    /// The state of the physics bodies after the server simulated `tick`: every body when `full`,
    /// otherwise only the bodies that are not resting (see `state_delta.rs`).
    ///
    /// A state is cut into chunks of at most [`MAX_BODIES_PER_STATE_MESSAGE`] bodies so that every
    /// message fits one datagram: chunk `chunk` of `chunks` (both counted from 0 and 1). The client
    /// uses a tick only when all of its chunks arrived.
    State {
        tick: u64,
        full: bool,
        chunk: u16,
        chunks: u16,
        bodies: Vec<BodyState>,
    },
}

/// Makes `app` step exactly one fixed tick per `update`, and runs the first update (which starts
/// Bevy's clock and simulates no tick). Call it after adding every plugin.
pub fn prepare_simulation_app(app: &mut App) {
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app.finish();
    app.cleanup();
    app.update();
}

/// Simulates exactly one fixed tick of `world` without running a frame: advances the fixed clock by one
/// timestep and runs the `FixedMain` schedule (the same schedules a frame runs for a tick). This is how
/// the net code steps a simulation, so the server, the clients and a client's replays all use one code
/// path, and so a correction can replay ticks from inside a system (a full `App::update` cannot nest).
///
/// Do not call it from inside `FixedMain` itself (the schedule is already running).
pub fn simulate_tick(world: &mut World) {
    let timestep = world.resource::<Time<Fixed>>().timestep();
    world.resource_mut::<Time<Fixed>>().advance_by(timestep);
    let fixed_time = world.resource::<Time<Fixed>>().as_generic();
    *world.resource_mut::<Time>() = fixed_time;
    world.run_schedule(FixedMain);
    let virtual_time = world.resource::<Time<Virtual>>().as_generic();
    *world.resource_mut::<Time>() = virtual_time;
}

/// True when two body states are the same within the tolerances (name and description must match;
/// sleeping is ignored: it follows from the motion).
pub fn states_match(left: &BodyState, right: &BodyState) -> bool {
    left.name == right.name
        && left.description == right.description
        && left.synced == right.synced
        && left.translation.distance(right.translation) <= POSITION_TOLERANCE
        && left.rotation.angle_between(right.rotation) <= POSITION_TOLERANCE
        && left.linear_velocity.distance(right.linear_velocity) <= VELOCITY_TOLERANCE
        && left.angular_velocity.distance(right.angular_velocity) <= VELOCITY_TOLERANCE
}

/// True when two lists (sorted by name) describe the same bodies within the tolerances.
pub fn state_lists_match(left: &[BodyState], right: &[BodyState]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| states_match(left, right))
}
