//! Ray casts and shape casts against the physics bodies, usable inside the fixed tick (a command or
//! an exclusive system): `cast_ray(world, &RayCast { .. })`. The engine answers them
//! (`retreat_engine/src/physics_queries.rs`).
//!
//! A cast sees every live body (not removed ones, and not one spawned in this same tick) where it is
//! **now**, including corrections a client just applied, so the server, a client and a replay get the
//! same answer. Exclude the caster's own entity. Equal distances are won by the first `Name`.
//!
//! **Ground detection** for an upright character: cast a ball a little smaller than the capsule from
//! the center of its lower half-sphere (`position - (0, half_height, 0)`) downwards by a little
//! more than the difference of the radii, excluding the character.

use bevy::prelude::*;

use crate::PhysicsShape;

/// A ray: from `origin` along `direction` (any non-zero length) up to `max_distance`.
#[derive(Clone, Debug, PartialEq)]
pub struct RayCast {
    pub origin: Vec3,
    pub direction: Vec3,
    pub max_distance: f32,
    /// Entities with a `PhysicsBody` to ignore.
    pub exclude: Vec<Entity>,
}

/// A shape swept from `origin` (its center) along `direction` up to `max_distance`.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeCast {
    pub shape: PhysicsShape,
    pub origin: Vec3,
    pub rotation: Quat,
    pub direction: Vec3,
    pub max_distance: f32,
    /// Entities with a `PhysicsBody` to ignore.
    pub exclude: Vec<Entity>,
}

/// The closest thing a cast met.
#[derive(Clone, Debug, PartialEq)]
pub struct CastHit {
    /// The entity with the `PhysicsBody`.
    pub entity: Entity,
    pub name: String,
    /// How far the ray (or the shape's center) travelled. 0 when it started touching or inside.
    pub distance: f32,
    /// Where it touched the body, in world coordinates.
    pub point: Vec3,
    /// The body's surface normal there (unit length). Not meaningful when `distance` is 0.
    pub normal: Vec3,
}

/// The engine's cast functions (a resource the engine installs).
#[derive(Resource, Clone, Copy)]
pub struct PhysicsCaster {
    pub ray: fn(&mut World, &RayCast) -> Option<CastHit>,
    pub shape: fn(&mut World, &ShapeCast) -> Option<CastHit>,
}

fn caster(world: &World) -> PhysicsCaster {
    *world.get_resource::<PhysicsCaster>().expect(
        "no PhysicsCaster: add the engine's PhysicsPlugin (or insert a PhysicsCaster in a test)",
    )
}

/// The closest body the ray meets, or `None` (also for a zero or non-finite ray).
pub fn cast_ray(world: &mut World, cast: &RayCast) -> Option<CastHit> {
    (caster(world).ray)(world, cast)
}

/// The closest body the swept shape meets, or `None` (also for an invalid cast).
pub fn cast_shape(world: &mut World, cast: &ShapeCast) -> Option<CastHit> {
    (caster(world).shape)(world, cast)
}
