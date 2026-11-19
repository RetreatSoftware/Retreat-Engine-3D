//! Rollback spike: capture the whole physics state and restore it later, so a client can rewind to
//! an earlier tick and re-simulate (client-side prediction, FRAMEWORK.md §12).
//!
//! A snapshot clones Rapier's simulation parts that carry state between steps: the body, collider
//! and joint sets, the island manager, broad phase, narrow phase, CCD solver and integration
//! parameters (all public fields of `bevy_rapier3d`'s context components), plus the `Transform` of
//! every rigid-body entity (the state hash reads those). Restoring writes them back.
//!
//! **Limits of this spike (checked by tests):** the set of bodies must be the same when restoring as when
//! capturing: bodies spawned or despawned inside the rollback window are not handled yet, because
//! `bevy_rapier`'s entity-to-handle maps are rebuilt only for bodies it knows. Restoring is exact
//! when that holds: the same inputs resimulate to the same `state.hash`.

use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy_rapier3d::plugin::context::{
    RapierContextColliders, RapierContextJoints, RapierContextSimulation, RapierRigidBodySet,
};
use bevy_rapier3d::prelude::RigidBody;
use bevy_rapier3d::rapier::dynamics::{CCDSolver, IntegrationParameters, IslandManager};
use bevy_rapier3d::rapier::geometry::{DefaultBroadPhase, NarrowPhase};

/// Everything needed to rewind the physics world to the moment of capture.
pub struct PhysicsSnapshot {
    islands: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    ccd_solver: CCDSolver,
    integration_parameters: IntegrationParameters,
    bodies: RapierRigidBodySet,
    colliders: RapierContextColliders,
    joints: RapierContextJoints,
    transforms: BTreeMap<Entity, Transform>,
}

/// Captures the physics state, or says why that is not possible (no physics context yet).
pub fn capture_physics(world: &mut World) -> Result<PhysicsSnapshot, String> {
    let mut contexts = world.query::<(
        &RapierContextSimulation,
        &RapierRigidBodySet,
        &RapierContextColliders,
        &RapierContextJoints,
    )>();
    let (simulation, bodies, colliders, joints) = contexts
        .single(world)
        .map_err(|error| format!("there is not exactly one Rapier context to capture: {error}"))?;
    let islands = simulation.islands.clone();
    let broad_phase = simulation.broad_phase.clone();
    let narrow_phase = simulation.narrow_phase.clone();
    let ccd_solver = simulation.ccd_solver.clone();
    let integration_parameters = simulation.integration_parameters;
    let bodies = bodies.clone();
    let colliders = colliders.clone();
    let joints = joints.clone();
    let mut transforms = BTreeMap::new();
    let mut bodies_query = world.query_filtered::<(Entity, &Transform), With<RigidBody>>();
    for (entity, transform) in bodies_query.iter(world) {
        transforms.insert(entity, *transform);
    }
    Ok(PhysicsSnapshot {
        islands,
        broad_phase,
        narrow_phase,
        ccd_solver,
        integration_parameters,
        bodies,
        colliders,
        joints,
        transforms,
    })
}

/// Rewinds the physics state to `snapshot`.
pub fn restore_physics(world: &mut World, snapshot: &PhysicsSnapshot) -> Result<(), String> {
    let mut contexts = world.query::<(
        &mut RapierContextSimulation,
        &mut RapierRigidBodySet,
        &mut RapierContextColliders,
        &mut RapierContextJoints,
    )>();
    let (mut simulation, mut bodies, mut colliders, mut joints) = contexts
        .single_mut(world)
        .map_err(|error| format!("there is not exactly one Rapier context to restore: {error}"))?;
    simulation.islands = snapshot.islands.clone();
    simulation.broad_phase = snapshot.broad_phase.clone();
    simulation.narrow_phase = snapshot.narrow_phase.clone();
    simulation.ccd_solver = snapshot.ccd_solver.clone();
    simulation.integration_parameters = snapshot.integration_parameters;
    *bodies = snapshot.bodies.clone();
    *colliders = snapshot.colliders.clone();
    *joints = snapshot.joints.clone();
    for (entity, transform) in &snapshot.transforms {
        if let Some(mut current) = world.get_mut::<Transform>(*entity) {
            *current = *transform;
        }
    }
    Ok(())
}
