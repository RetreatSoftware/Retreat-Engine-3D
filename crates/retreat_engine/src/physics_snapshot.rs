//! Rollback spike: capture the whole physics state and restore it later, so a client can rewind to
//! an earlier tick and re-simulate (client-side prediction, FRAMEWORK.md §12).
//!
//! A snapshot clones Rapier's simulation parts that carry state between steps: the body, collider
//! and joint sets, the island manager, broad phase, narrow phase, CCD solver and integration
//! parameters (all public fields of `bevy_rapier3d`'s context components), plus the `Transform` of
//! every rigid-body entity (the state hash reads those). Restoring writes them back.
//!
//! **Bodies that appear or disappear inside the rollback window.**
//! - A body spawned after the capture is despawned by `restore_physics` (with its proxy): the replay
//!   of the same inputs spawns it again, under the same `Name`.
//! - A body despawned after the capture cannot be brought back (its game entity's other components are
//!   not ours to recreate), so `restore_physics` refuses with [`RollbackError::BodiesDespawned`] and
//!   changes nothing. Whoever opens a rollback window must therefore delay destroying bodies until the
//!   window has passed (deferred destruction, the netcode layer's job).
//!
//! Restoring is exact when those rules hold: the same inputs resimulate to the same `state.hash`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use bevy::prelude::*;
use bevy_rapier3d::plugin::context::{
    RapierContextColliders, RapierContextJoints, RapierContextSimulation, RapierRigidBodySet,
};
use bevy_rapier3d::prelude::RigidBody;
use bevy_rapier3d::rapier::dynamics::{CCDSolver, IntegrationParameters, IslandManager};
use bevy_rapier3d::rapier::geometry::{DefaultBroadPhase, NarrowPhase};
use retreat_engine_api::{PhysicsBody, SimulationRng};

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
    /// The simulation RNG: a replay that spawns or rolls random values must see the same numbers.
    rng: Option<SimulationRng>,
    /// How much fixed time had passed (`Time<Fixed>`): sounds, animation and the like measure time
    /// with it, so a replay must not run it twice.
    fixed_elapsed: Duration,
    /// Game entities with a `PhysicsBody` at capture time.
    game_bodies: BTreeSet<Entity>,
    /// Their names, for error messages after they are gone.
    names: BTreeMap<Entity, String>,
    /// Entities with a Rapier `RigidBody` (the proxies) at capture time.
    proxies: BTreeSet<Entity>,
}

/// Why a snapshot could not be restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackError {
    /// There is not exactly one Rapier context.
    NoPhysicsContext(String),
    /// Bodies that existed at capture time are gone (names, sorted).
    BodiesDespawned(Vec<String>),
}

impl std::fmt::Display for RollbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RollbackError::NoPhysicsContext(reason) => {
                write!(f, "there is not exactly one Rapier context: {reason}")
            }
            RollbackError::BodiesDespawned(names) => write!(
                f,
                "physics bodies {} were despawned after the snapshot was captured and cannot be                  brought back; delay despawning bodies until the rollback window has passed",
                names.join(", ")
            ),
        }
    }
}

impl std::error::Error for RollbackError {}

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
    let mut game_query = world.query_filtered::<Entity, With<PhysicsBody>>();
    let game_bodies: BTreeSet<Entity> = game_query.iter(world).collect();
    let mut proxy_query = world.query_filtered::<Entity, With<RigidBody>>();
    let proxies: BTreeSet<Entity> = proxy_query.iter(world).collect();
    let mut names = BTreeMap::new();
    let mut name_query = world.query::<(Entity, &Name)>();
    for (entity, name) in name_query.iter(world) {
        if game_bodies.contains(&entity) || proxies.contains(&entity) {
            names.insert(entity, name.as_str().to_string());
        }
    }
    let rng = world.get_resource::<SimulationRng>().cloned();
    let fixed_elapsed = world.resource::<Time<Fixed>>().elapsed();
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
        rng,
        fixed_elapsed,
        game_bodies,
        names,
        proxies,
    })
}

/// Rewinds the physics state to `snapshot`.
pub fn restore_physics(world: &mut World, snapshot: &PhysicsSnapshot) -> Result<(), RollbackError> {
    let mut game_query = world.query_filtered::<Entity, With<PhysicsBody>>();
    let game_now: BTreeSet<Entity> = game_query.iter(world).collect();
    let mut proxy_query = world.query_filtered::<Entity, With<RigidBody>>();
    let proxies_now: BTreeSet<Entity> = proxy_query.iter(world).collect();
    let missing: Vec<Entity> = snapshot
        .game_bodies
        .difference(&game_now)
        .chain(snapshot.proxies.difference(&proxies_now))
        .copied()
        .collect();
    if !missing.is_empty() {
        let mut names: Vec<String> = missing
            .iter()
            .map(|entity| {
                snapshot
                    .names
                    .get(entity)
                    .cloned()
                    .unwrap_or_else(|| format!("{entity}"))
            })
            .collect();
        names.sort();
        names.dedup();
        return Err(RollbackError::BodiesDespawned(names));
    }
    // Spawned after the capture: remove them (a proxy goes with its game entity; a proxy whose
    // game entity is already gone is removed itself).
    for entity in game_now.difference(&snapshot.game_bodies) {
        world.despawn(*entity);
    }
    for entity in proxies_now.difference(&snapshot.proxies) {
        if world.get_entity(*entity).is_ok() {
            world.despawn(*entity);
        }
    }
    let mut contexts = world.query::<(
        &mut RapierContextSimulation,
        &mut RapierRigidBodySet,
        &mut RapierContextColliders,
        &mut RapierContextJoints,
    )>();
    let (mut simulation, mut bodies, mut colliders, mut joints) = contexts
        .single_mut(world)
        .map_err(|error| RollbackError::NoPhysicsContext(error.to_string()))?;
    simulation.islands = snapshot.islands.clone();
    simulation.broad_phase = snapshot.broad_phase.clone();
    simulation.narrow_phase = snapshot.narrow_phase.clone();
    simulation.ccd_solver = snapshot.ccd_solver.clone();
    simulation.integration_parameters = snapshot.integration_parameters;
    *bodies = snapshot.bodies.clone();
    *colliders = snapshot.colliders.clone();
    *joints = snapshot.joints.clone();
    // Rewind the fixed clock: a fresh clock with the same step, advanced to the captured time.
    let timestep = world.resource::<Time<Fixed>>().timestep();
    let mut clock = Time::<Fixed>::from_duration(timestep);
    clock.advance_by(snapshot.fixed_elapsed);
    clock.advance_by(Duration::ZERO);
    *world.resource_mut::<Time<Fixed>>() = clock;
    if let Some(rng) = &snapshot.rng {
        world.insert_resource(rng.clone());
    }
    for (entity, transform) in &snapshot.transforms {
        if let Some(mut current) = world.get_mut::<Transform>(*entity) {
            *current = *transform;
        }
    }
    Ok(())
}
