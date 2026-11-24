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
#[require(Transform)]
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
}

impl PhysicsBody {
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
