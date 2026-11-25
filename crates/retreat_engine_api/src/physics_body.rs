//! How game code and modules ask for a physics body, without naming the physics library.
//!
//! An entity with a [`PhysicsBody`], a unique `Name` and no parent becomes a rigid body: the
//! engine creates the real body (today in Rapier) in a deterministic order and copies its pose
//! back to the entity's `Transform` every tick. Modules use only these types, so they keep
//! working if the engine changes physics library (Avian is the recorded fallback).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Makes an entity physical. Read once, in the tick it is added; later edits are ignored.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[require(Transform, crate::Replicated, PhysicsVelocity)]
pub struct PhysicsBody {
    pub kind: PhysicsBodyKind,
    pub shape: PhysicsShape,
    /// Where the shape's center sits relative to the entity's origin (e.g. a ground slab whose
    /// top face is the entity's origin has offset `(0, -half_height, 0)`).
    pub shape_offset: Vec3,
}

/// Whether the body moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhysicsBodyKind {
    /// Moved by gravity and collisions.
    Dynamic,
    /// An upright character: moved by collisions and by the velocity the game sets. Its rotation is
    /// locked, it feels **no engine gravity** (the controller adds its own), has no friction, never
    /// sleeps and uses continuous collision detection. Replicated like a dynamic body.
    Character,
    /// Never moves; others collide with it.
    Fixed,
}

/// The collision shape, in the entity's local units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum PhysicsShape {
    /// A box; `half_extents` go from the center to each face.
    Cuboid { half_extents: Vec3 },
    /// A sphere.
    Ball { radius: f32 },
    /// An upright (Y axis) capsule: a cylinder of height `2 * half_height` with a half-sphere of
    /// `radius` on each end, so it is `2 * (half_height + radius)` tall.
    Capsule { half_height: f32, radius: f32 },
}

impl PhysicsShape {
    /// Whether every size is a positive finite number.
    pub fn is_valid(&self) -> bool {
        let positive = |value: f32| value.is_finite() && value > 0.0;
        match *self {
            PhysicsShape::Cuboid { half_extents } => {
                half_extents.to_array().into_iter().all(positive)
            }
            PhysicsShape::Ball { radius } => positive(radius),
            PhysicsShape::Capsule {
                half_height,
                radius,
            } => positive(half_height) && positive(radius),
        }
    }
}

/// A body's velocity (units and radians per second), kept by the engine: current at the start of a
/// tick, also right after a client's rewind. Read only; writing it changes nothing (use
/// [`PhysicsVelocitySet`] or [`PhysicsVelocityChange`]).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicsVelocity {
    pub linear: Vec3,
    pub angular: Vec3,
}

/// A request to set a moving body's linear velocity once. Applied before the physics step of the
/// tick when inserted from [`PhysicsControlSystems`] (before any [`PhysicsVelocityChange`]), then
/// removed. It waits until the body exists.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct PhysicsVelocitySet(pub Vec3);

/// The `FixedUpdate` set for systems that drive bodies (character controllers): it runs after the
/// tick's commands and input and before velocity requests and the physics step, so the server, a
/// client and a replay act in the same order.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhysicsControlSystems;

/// A request to change a dynamic body's velocity once: the engine adds `0` (units per second) to its
/// linear velocity and wakes it, then removes this component. It stays on the entity until the body
/// exists in the physics world (a body appears on the tick after its entity is spawned).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct PhysicsVelocityChange(pub Vec3);

/// A request to remove a body (and its entity) safely. Insert it instead of despawning an entity that
/// has a [`PhysicsBody`]: a despawn cannot be rolled back, so a client that corrects a prediction
/// would refuse the rewind. The engine *retires* the body at once (it stops simulating, is hidden and
/// is no longer replicated or hashed) and despawns the entity only after the rollback window, so a
/// rewind and replay can bring it back deterministically. Needs no network code in the caller.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemovePhysicsBody;

impl PhysicsBody {
    /// An upright character capsule centered on the entity.
    pub fn character(half_height: f32, radius: f32) -> Self {
        Self {
            kind: PhysicsBodyKind::Character,
            shape: PhysicsShape::Capsule {
                half_height,
                radius,
            },
            shape_offset: Vec3::ZERO,
        }
    }

    /// A dynamic box with edge length `size`, centered on the entity.
    pub fn dynamic_cube(size: f32) -> Self {
        Self {
            kind: PhysicsBodyKind::Dynamic,
            shape: PhysicsShape::Cuboid {
                half_extents: Vec3::splat(size / 2.0),
            },
            shape_offset: Vec3::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_cube_has_half_the_size_as_half_extents() {
        let body = PhysicsBody::dynamic_cube(0.5);
        assert_eq!(body.kind, PhysicsBodyKind::Dynamic);
        assert_eq!(
            body.shape,
            PhysicsShape::Cuboid {
                half_extents: Vec3::splat(0.25)
            }
        );
        assert_eq!(body.shape_offset, Vec3::ZERO);
    }
}
