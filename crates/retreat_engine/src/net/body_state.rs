//! The replicated state of physics bodies: what a server sends and a client applies.
//!
//! A [`BodyState`] is library-neutral data keyed by the game entity's `Name`: its description (kind
//! and shape, so a client can create a body it has not seen), pose, velocities and whether it is
//! asleep. [`read_body_states`] collects it from the world; [`apply_body_states`] writes it onto the
//! bodies of a client. Both are the only places in the net code that touch Rapier's body set.

use bevy::prelude::*;
use bevy_rapier3d::plugin::context::RapierRigidBodySet;
use retreat_engine_api::{PhysicsBody, PhysicsBodyKind};
use serde::{Deserialize, Serialize};

use crate::RetiredBody;
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
}

/// The state of every physics body, sorted by name.
pub fn read_body_states(world: &mut World) -> Vec<BodyState> {
    let mut owners =
        world.query_filtered::<(&Name, &PhysicsBody, &PhysicsProxy), Without<RetiredBody>>();
    let owner_list: Vec<(String, PhysicsBody, Entity)> = owners
        .iter(world)
        .map(|(name, body, proxy)| (name.as_str().to_string(), body.clone(), proxy.entity()))
        .collect();
    let mut sets = world.query::<&RapierRigidBodySet>();
    let Ok(set) = sets.single(world) else {
        return Vec::new();
    };
    let mut states: Vec<BodyState> = owner_list
        .into_iter()
        .filter_map(|(name, description, proxy)| {
            let handle = *set.entity2body().get(&proxy)?;
            let body = set.bodies.get(handle)?;
            Some(BodyState {
                name,
                description,
                translation: body.translation(),
                rotation: *body.rotation(),
                linear_velocity: body.linvel(),
                angular_velocity: body.angvel(),
                sleeping: body.is_sleeping(),
            })
        })
        .collect();
    states.sort_by(|left, right| left.name.cmp(&right.name));
    states
}

/// The state of every body that moves (`PhysicsBodyKind::Dynamic`), sorted by name: what is replicated.
/// Fixed bodies never move and every machine has them from its own scene, so they are not sent.
pub fn read_replicated_body_states(world: &mut World) -> Vec<BodyState> {
    let mut states = read_body_states(world);
    states.retain(|state| state.description.kind == PhysicsBodyKind::Dynamic);
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
    let mut owners =
        world.query_filtered::<(&Name, &PhysicsBody, &PhysicsProxy), Without<RetiredBody>>();
    let retired_names: Vec<String> = world
        .query_filtered::<&Name, With<RetiredBody>>()
        .iter(world)
        .map(|name| name.as_str().to_string())
        .collect();
    // Only dynamic bodies are replicated, so only they can be missing from a state list.
    let proxies: Vec<(String, Entity)> = owners
        .iter(world)
        .filter(|(_, body, _)| body.kind == PhysicsBodyKind::Dynamic)
        .map(|(name, _, proxy)| (name.as_str().to_string(), proxy.entity()))
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
        let found = proxies.iter().find(|(name, _)| *name == state.name);
        let Some(handle) = found.and_then(|(_, proxy)| set.entity2body().get(proxy).copied())
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
        body.set_translation(state.translation, false);
        body.set_rotation(state.rotation, false);
        body.set_linvel(state.linear_velocity, false);
        body.set_angvel(state.angular_velocity, false);
        if state.sleeping {
            body.sleep();
        } else {
            body.wake_up(false);
        }
    }
    for (name, _) in &proxies {
        if !states.iter().any(|state| state.name == *name) {
            report.missing_from_states.push(name.clone());
        }
    }
    report.missing_from_states.sort();
    report.unknown_to_world.sort();
    report
}
