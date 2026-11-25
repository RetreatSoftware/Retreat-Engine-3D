//! The replicated state of physics bodies: what a server sends and a client applies.
//!
//! A [`BodyState`] is library-neutral data keyed by the game entity's `Name`: its description (kind
//! and shape, so a client can create a body it has not seen), pose, velocities and whether it is
//! asleep. [`read_body_states`] collects it from the world; [`apply_body_states`] writes it onto the
//! bodies of a client. Both are the only places in the net code that touch Rapier's body set.

use bevy::prelude::*;
use bevy_rapier3d::plugin::context::RapierRigidBodySet;
use retreat_engine_api::{PhysicsBody, PhysicsBodyKind, Replicated, RetiredEntity, SyncedKind};
use serde::{Deserialize, Serialize};

use crate::physics::PhysicsProxy;

/// One body's replicated state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodyState {
    /// `Name` of the game entity (unique; the key on every machine).
    pub name: String,
    /// Kind, shape and offset: enough to create the body on a client that lacks it.
    pub description: PhysicsBody,
    pub translation: Vec3,
    pub rotation: Quat,
    pub linear_velocity: Vec3,
    pub angular_velocity: Vec3,
    pub sleeping: bool,
    /// The kinds this body replicates (sorted, from its `Replicated`). A field whose kind is not
    /// listed is zero here (identity rotation, `false`) and is never applied.
    pub synced: Vec<SyncedKind>,
}

/// The state of every physics body, sorted by name.
pub fn read_body_states(world: &mut World) -> Vec<BodyState> {
    let mut owners = world.query_filtered::<(
        &Name,
        &PhysicsBody,
        &PhysicsProxy,
        Option<&Replicated>,
    ), Without<RetiredEntity>>();
    let owner_list: Vec<(String, PhysicsBody, Entity, Replicated)> = owners
        .iter(world)
        .map(|(name, body, proxy, replicated)| {
            (
                name.as_str().to_string(),
                body.clone(),
                proxy.entity(),
                replicated.cloned().unwrap_or_default(),
            )
        })
        .collect();
    let mut sets = world.query::<&RapierRigidBodySet>();
    let Ok(set) = sets.single(world) else {
        return Vec::new();
    };
    let mut states: Vec<BodyState> = owner_list
        .into_iter()
        .filter_map(|(name, description, proxy, replicated)| {
            let handle = *set.entity2body().get(&proxy)?;
            let body = set.bodies.get(handle)?;
            let mut state = BodyState {
                name,
                description,
                translation: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                linear_velocity: Vec3::ZERO,
                angular_velocity: Vec3::ZERO,
                sleeping: false,
                synced: replicated.kinds.iter().copied().collect(),
            };
            if replicated.syncs(SyncedKind::Pose) {
                state.translation = body.translation();
                state.rotation = *body.rotation();
            }
            if replicated.syncs(SyncedKind::Velocity) {
                state.linear_velocity = body.linvel();
                state.angular_velocity = body.angvel();
            }
            if replicated.syncs(SyncedKind::Sleeping) {
                state.sleeping = body.is_sleeping();
            }
            Some(state)
        })
        .collect();
    states.sort_by(|left, right| left.name.cmp(&right.name));
    states
}

/// True when a body is part of the replicated state: it moves (`PhysicsBodyKind::Dynamic`) and its
/// `Replicated` lists at least one kind. This is the one rule for "is replicated": the server's
/// state, the client's history and the client's apply step all use it. Fixed bodies never move and
/// every machine has them from its own scene, so they are not sent.
fn is_replicated_body(kind: PhysicsBodyKind, synced: &[SyncedKind]) -> bool {
    kind == PhysicsBodyKind::Dynamic && !synced.is_empty()
}

/// The state of every replicated body (see `is_replicated_body`), sorted by name.
pub fn read_replicated_body_states(world: &mut World) -> Vec<BodyState> {
    let mut states = read_body_states(world);
    states.retain(|state| is_replicated_body(state.description.kind, &state.synced));
    states
}

/// Names in a state list that no body in the world has, and names in the world that the list lacks.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// In the state list, not in the world: the caller may create them.
    pub unknown_to_world: Vec<String>,
    /// In the world, not in the state list.
    pub missing_from_states: Vec<String>,
}

/// Writes `states` onto the matching bodies (pose, velocities, sleeping) and reports the names that
/// did not match. Fixed bodies are left alone (they never move).
pub fn apply_body_states(world: &mut World, states: &[BodyState]) -> ApplyReport {
    // Retired bodies are not part of the shared state (see `retired_bodies.rs`): not matched and
    // never created again from a state.
    let mut owners = world.query_filtered::<(
        &Name,
        &PhysicsBody,
        &PhysicsProxy,
        Option<&Replicated>,
    ), Without<RetiredEntity>>();
    let retired_names: Vec<String> = world
        .query_filtered::<&Name, With<RetiredEntity>>()
        .iter(world)
        .map(|name| name.as_str().to_string())
        .collect();
    // Only replicated bodies can be missing from a state list.
    let proxies: Vec<(String, Entity, Replicated)> = owners
        .iter(world)
        .filter_map(|(name, body, proxy, replicated)| {
            let replicated = replicated.cloned().unwrap_or_default();
            let kinds: Vec<SyncedKind> = replicated.kinds.iter().copied().collect();
            is_replicated_body(body.kind, &kinds)
                .then(|| (name.as_str().to_string(), proxy.entity(), replicated))
        })
        .collect();
    let mut report = ApplyReport::default();
    let mut sets = world.query::<&mut RapierRigidBodySet>();
    let Ok(mut set) = sets.single_mut(world) else {
        report.unknown_to_world = states.iter().map(|state| state.name.clone()).collect();
        return report;
    };
    for state in states {
        if retired_names.contains(&state.name) {
            continue;
        }
        let found = proxies.iter().find(|(name, _, _)| *name == state.name);
        let Some(handle) = found.and_then(|(_, proxy, _)| set.entity2body().get(proxy).copied())
        else {
            report.unknown_to_world.push(state.name.clone());
            continue;
        };
        let Some(body) = set.bodies.get_mut(handle) else {
            report.unknown_to_world.push(state.name.clone());
            continue;
        };
        if !body.is_dynamic() {
            continue;
        }
        // The server's declaration decides what it sent, so apply exactly the kinds it listed.
        if state.synced.contains(&SyncedKind::Pose) {
            body.set_translation(state.translation, false);
            body.set_rotation(state.rotation, false);
        }
        if state.synced.contains(&SyncedKind::Velocity) {
            body.set_linvel(state.linear_velocity, false);
            body.set_angvel(state.angular_velocity, false);
        }
        if state.synced.contains(&SyncedKind::Sleeping) {
            if state.sleeping {
                body.sleep();
            } else {
                body.wake_up(false);
            }
        }
    }
    for (name, _, _) in &proxies {
        if !states.iter().any(|state| state.name == *name) {
            report.missing_from_states.push(name.clone());
        }
    }
    report.missing_from_states.sort();
    report.unknown_to_world.sort();
    report
}
