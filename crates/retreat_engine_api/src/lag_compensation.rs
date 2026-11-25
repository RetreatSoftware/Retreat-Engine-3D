//! Lag compensation: judging a shot against what the shooter saw. The engine implements it
//! (`retreat_engine/src/net/lag_compensation.rs`); it knows no weapons.
//!
//! **How it fits together.**
//! 1. A client draws the other players from the server's own past states: give their entities
//!    [`crate::RemoteInterpolation`]. [`NetViewTick`] says which server tick they are drawn at.
//! 2. The client's fire input carries that number:
//!    `rifle.fire <yaw> <pitch> <view_tick>`.
//! 3. The command (the same line on the server and in the shooter's prediction) casts with
//!    [`cast_ray_at`]: every moving body is where it was at that server tick; fixed bodies are
//!    where they are. Exclude the shooter, and anything that cannot be hit now (a dead player).
//!
//! The view tick is a client's claim. The server keeps [`MAX_REWIND_TICKS`] ticks of history and
//! treats an older (or newer) number as the nearest it has, so a client cannot shoot further into
//! the past than that. Only poses are rewound: check health and the like against the present.

use bevy::prelude::*;

use crate::{CastHit, RayCast, ShapeCast};

/// How far back the server can look: one second at 64 ticks per second.
pub const MAX_REWIND_TICKS: u64 = 64;

/// On a client, the server tick (with a fraction) at which entities with
/// [`crate::RemoteInterpolation`] are drawn now; `None` on a server, without a network and before
/// the first state. Written by the engine every live fixed tick.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct NetViewTick(pub Option<f64>);

impl NetViewTick {
    /// The number to put in an input line: the view tick, or 0 (= "now") without one.
    pub fn for_input(&self) -> f64 {
        self.0.unwrap_or(0.0)
    }
}

/// The engine's lag-compensated cast functions (a resource the engine installs).
#[derive(Resource, Clone, Copy)]
pub struct LagCaster {
    pub ray: fn(&mut World, &RayCast, f64) -> Option<CastHit>,
    pub shape: fn(&mut World, &ShapeCast, f64) -> Option<CastHit>,
}

fn caster(world: &World) -> LagCaster {
    *world
        .get_resource::<LagCaster>()
        .expect("no LagCaster: add the engine's PhysicsPlugin (or insert a LagCaster in a test)")
}

/// [`crate::cast_ray`] against the moving bodies as they were at server tick `view_tick`. A tick of
/// 0 or less, or no history (single player), casts against the present. A hit on a body that has
/// since been removed reports its (retired) entity and name.
pub fn cast_ray_at(world: &mut World, cast: &RayCast, view_tick: f64) -> Option<CastHit> {
    (caster(world).ray)(world, cast, view_tick)
}

/// [`crate::cast_shape`] against the moving bodies as they were at server tick `view_tick`.
pub fn cast_shape_at(world: &mut World, cast: &ShapeCast, view_tick: f64) -> Option<CastHit> {
    (caster(world).shape)(world, cast, view_tick)
}
