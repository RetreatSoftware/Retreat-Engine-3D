//! Moving a character over stairs and slopes, and putting a body somewhere else at once. The engine
//! does the geometry (`retreat_engine/src/character_stepping.rs`); the module still decides the
//! velocity, gravity and jumps.
//!
//! **Stepping.** Add [`CharacterStepping`] to a `PhysicsBodyKind::Character` capsule (it brings
//! [`CharacterGround`]). Each tick the module sets the velocity it wants with
//! `PhysicsVelocitySet` from `PhysicsControlSystems`; the engine then, before the physics step:
//! - while the body rises faster than a walk allows (a jump), changes nothing;
//! - on walkable ground (slope up to `max_slope_degrees`) lays the horizontal velocity along the
//!   ground, so the character walks slopes at its speed and does not slide when it stops, and keeps
//!   it on the ground when walking down steps up to `max_step_height`;
//! - climbs a step up to `max_step_height` in front of it;
//! - does not let it walk up anything steeper.
//!
//! The vertical velocity the module asked for is replaced while grounded; in the air it is kept, so
//! the module applies gravity when [`CharacterGround::grounded`] is false:
//! `velocity.y = if ground.grounded { jump_or_zero } else { velocity.y - gravity * dt }`.
//!
//! Both components are synced module state (the engine registers them), so they roll back and a
//! client that creates the entity from the server's state has them.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::NetState;

/// Opts a character body into step and slope handling.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[require(CharacterGround)]
pub struct CharacterStepping {
    /// The highest step (world units) it climbs and walks down while staying on the ground.
    pub max_step_height: f32,
    /// The steepest ground it stands and walks on, below 90.
    pub max_slope_degrees: f32,
}

impl NetState for CharacterStepping {
    const ID: &'static str = "engine.character_stepping";
}

/// What the character stands on, written by the engine each tick (read only).
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharacterGround {
    /// On walkable ground.
    pub grounded: bool,
    /// The ground's surface normal; straight up when not grounded.
    pub normal: Vec3,
}

impl Default for CharacterGround {
    fn default() -> Self {
        Self {
            grounded: false,
            normal: Vec3::Y,
        }
    }
}

impl NetState for CharacterGround {
    const ID: &'static str = "engine.character_ground";
    fn matches(&self, server: &Self) -> bool {
        self.grounded == server.grounded && self.normal.distance(server.normal) < 0.01
    }
}

/// A request to put a body's origin at a position and stop it, once (respawn). Applied before the
/// physics step of the tick (before velocity requests, so a velocity set in the same tick
/// applies), then removed. It waits until the body exists. Insert it in a command or in
/// `PhysicsControlSystems`; the server's teleport reaches clients with the body's state.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct PhysicsTeleport(pub Vec3);
