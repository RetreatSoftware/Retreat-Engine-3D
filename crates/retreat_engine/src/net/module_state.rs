//! Module state (`NetState` components, contract `retreat_engine_api/src/net_state.rs`) in rollback
//! and replication. The one place that moves it between the world, snapshots and messages.
//!
//! - [`capture_module_state`] / [`restore_module_state`] are part of the physics snapshot
//!   (`physics_snapshot.rs`): values are put back, components added since are removed, and entities
//!   spawned since that carry module state are despawned (the replay spawns them again).
//! - [`read_module_states`] lists what a machine shares, sorted by (entity name, type id); retired
//!   entities are left out.
//! - [`apply_module_states`] makes a client's world equal to the server's list: it writes values,
//!   creates missing entities (name and components only), removes components the server lacks, and
//!   despawns an entity without a physics body that the server no longer has at all. An entity with a
//!   body is removed only by the body list (`client.rs`).

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use retreat_engine_api::{NetStateRegistry, PhysicsBody, PredictedSpawn, RetiredEntity};
use serde::{Deserialize, Serialize};

use crate::command_script::fail_running_script;
use crate::physics::{PhysicsProxy, PhysicsProxyOf};

/// Largest encoded size of one [`ModuleState`], so that it always fits a state message.
pub const MAX_MODULE_STATE_BYTES: usize = 1024;

/// One component of one entity, as sent over the network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleState {
    /// `Name` of the entity (the key on every machine).
    pub entity: String,
    /// `NetState::ID` of the component.
    pub id: String,
    /// `NetState::VERSION` of the sender.
    pub version: u8,
    /// The encoded component.
    pub data: Vec<u8>,
}

impl ModuleState {
    /// True when this build has the type, at this version, and can decode the data.
    pub fn is_usable(&self, registry: &NetStateRegistry) -> bool {
        registry
            .get(&self.id)
            .is_some_and(|kind| kind.version == self.version && (kind.decodes)(&self.data))
    }
}

/// Bytes this value takes in a message.
pub fn encoded_size<T: Serialize>(value: &T) -> usize {
    postcard::to_allocvec(value)
        .expect("encoding to a growing Vec cannot run out of room")
        .len()
}

/// Every module state of the world that is shared between machines, sorted by (entity, id).
/// An entity without a `Name`, or two with the same name, are errors (logged, and they fail a
/// running script).
pub fn read_module_states(world: &mut World) -> Vec<ModuleState> {
    let Some(registry) = world.get_resource::<NetStateRegistry>().cloned() else {
        return Vec::new();
    };
    let mut states: BTreeMap<(String, &'static str), ModuleState> = BTreeMap::new();
    let mut errors = BTreeSet::new();
    for kind in registry.types() {
        for entity in (kind.entities)(world) {
            if world.get::<RetiredEntity>(entity).is_some() || awaits_its_body(world, entity) {
                continue;
            }
            let Some(name) = world.get::<Name>(entity) else {
                errors.insert(format!(
                    "an entity with net state `{}` has no Name; give it a unique Name when it is \
                     spawned",
                    kind.id
                ));
                continue;
            };
            let name = name.as_str().to_string();
            let Some(data) = (kind.read)(world, entity) else {
                continue;
            };
            let state = ModuleState {
                entity: name.clone(),
                id: kind.id.to_string(),
                version: kind.version,
                data,
            };
            if states.insert((name.clone(), kind.id), state).is_some() {
                errors.insert(format!(
                    "two entities named `{name}` have net state `{}`; names must be unique",
                    kind.id
                ));
            }
        }
    }
    for message in errors {
        error!("{message}");
        fail_running_script(world, message);
    }
    states.into_values().collect()
}

/// True for a body spawned too late in this tick to be simulated in it: the body list does not
/// have it yet, so its state must wait too. A client that heard of the state alone would make an
/// entity without a body and, a broadcast later, a second one with it.
fn awaits_its_body(world: &World, entity: Entity) -> bool {
    world.get::<PhysicsBody>(entity).is_some() && world.get::<PhysicsProxy>(entity).is_none()
}

/// Marks an entity a client made from module state alone.
#[derive(Component)]
pub(crate) struct MadeFromState;

/// Removes the stand-in made from state alone for `name`, because its body arrived: the body is
/// spawned as a new entity (a body is added with its entity, never to an older one) and takes the
/// state in the same correction.
pub(crate) fn replace_stand_in(world: &mut World, name: &str) {
    let stand_ins: Vec<Entity> = world
        .query_filtered::<(Entity, &Name), (With<MadeFromState>, Without<PhysicsBody>)>()
        .iter(world)
        .filter(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity)
        .collect();
    for entity in stand_ins {
        world.despawn(entity);
    }
}

/// True when two sorted lists hold the same values, each by its type's `NetState::matches`.
pub fn module_state_lists_match(
    registry: &NetStateRegistry,
    predicted: &[ModuleState],
    server: &[ModuleState],
) -> bool {
    predicted.len() == server.len()
        && predicted.iter().zip(server).all(|(predicted, server)| {
            predicted.entity == server.entity
                && predicted.id == server.id
                && predicted.version == server.version
                && registry
                    .get(&server.id)
                    .is_some_and(|kind| (kind.matches)(&predicted.data, &server.data))
        })
}

/// The module state part of a snapshot.
#[derive(Default)]
pub struct ModuleStateSnapshot {
    /// Every named entity that existed (to tell "spawned since" from "got a component since").
    existing: BTreeSet<Entity>,
    /// Entities with module state: their name and values by type id.
    values: BTreeMap<Entity, (String, BTreeMap<&'static str, Vec<u8>>)>,
}

impl ModuleStateSnapshot {
    /// Names (sorted) of captured entities that no longer exist, so a restore cannot be exact.
    pub fn despawned_entities(&self, world: &World) -> Vec<String> {
        let mut names: Vec<String> = self
            .values
            .iter()
            .filter(|(entity, _)| world.get_entity(**entity).is_err())
            .map(|(_, (name, _))| name.clone())
            .collect();
        names.sort();
        names
    }
}

/// Captures every registered component, including those of retired entities (a restore can bring
/// them back).
pub fn capture_module_state(world: &mut World) -> ModuleStateSnapshot {
    let mut snapshot = ModuleStateSnapshot::default();
    let Some(registry) = world.get_resource::<NetStateRegistry>().cloned() else {
        return snapshot;
    };
    if registry.is_empty() {
        return snapshot;
    }
    snapshot.existing = world
        .query_filtered::<Entity, With<Name>>()
        .iter(world)
        .collect();
    for kind in registry.types() {
        for entity in (kind.entities)(world) {
            let Some(data) = (kind.read)(world, entity) else {
                continue;
            };
            let name = world
                .get::<Name>(entity)
                .map_or_else(|| entity.to_string(), |name| name.as_str().to_string());
            snapshot
                .values
                .entry(entity)
                .or_insert_with(|| (name, BTreeMap::new()))
                .1
                .insert(kind.id, data);
        }
    }
    snapshot
}

/// Retires a predicted spawn made after the restored tick.
pub(crate) fn set_aside(world: &mut World, entity: Entity) {
    if world.get::<RetiredEntity>(entity).is_some() {
        return;
    }
    let since = world
        .get_resource::<Time<Fixed>>()
        .map_or_else(Default::default, |time| time.elapsed());
    let mut found = world.entity_mut(entity);
    found.insert(RetiredEntity { since });
    if found.contains::<Visibility>() {
        found.insert(Visibility::Hidden);
    }
}

/// Puts the captured values back. The caller has checked
/// [`ModuleStateSnapshot::despawned_entities`].
pub fn restore_module_state(world: &mut World, snapshot: &ModuleStateSnapshot) {
    let Some(registry) = world.get_resource::<NetStateRegistry>().cloned() else {
        return;
    };
    for kind in registry.types() {
        for entity in (kind.entities)(world) {
            // An earlier type may have despawned it.
            if world.get_entity(entity).is_err() {
                continue;
            }
            let captured = snapshot.values.get(&entity);
            if captured.is_none() && !snapshot.existing.contains(&entity) {
                // A predicted spawn is set aside, not destroyed: the replay's spawn takes the
                // same entity again (`spawn_predicted`), so it never restarts on screen.
                if world.get::<PredictedSpawn>(entity).is_some() {
                    set_aside(world, entity);
                } else {
                    world.despawn(entity);
                }
            } else if !captured.is_some_and(|(_, values)| values.contains_key(kind.id)) {
                (kind.remove)(world, entity);
            }
        }
    }
    for (entity, (name, values)) in &snapshot.values {
        for (id, data) in values {
            if let Some(kind) = registry.get(id)
                && let Err(message) = (kind.write)(world, *entity, data)
            {
                let message = format!("restoring `{name}`: {message}");
                error!("{message}");
                fail_running_script(world, message);
            }
        }
    }
}

/// Makes the world's module state equal to `server` (a whole list of usable values, sorted; see
/// the module docs). Called after the body states were applied.
/// `keep` names predicted spawns the server cannot list yet: they are left alone.
pub fn apply_module_states(world: &mut World, server: &[ModuleState], keep: &BTreeSet<String>) {
    let Some(registry) = world.get_resource::<NetStateRegistry>().cloned() else {
        return;
    };
    // The entity of each name that can take state: not a physics proxy. A retired entity keeps its
    // name until it is purged and is not part of the shared state (a client revives the ones without
    // a body before calling this).
    let mut by_name: BTreeMap<String, Entity> = BTreeMap::new();
    let mut retired: BTreeSet<String> = BTreeSet::new();
    let mut named =
        world.query_filtered::<(Entity, &Name, Has<RetiredEntity>), Without<PhysicsProxyOf>>();
    for (entity, name, is_retired) in named.iter(world) {
        if is_retired {
            retired.insert(name.as_str().to_string());
        } else {
            by_name.insert(name.as_str().to_string(), entity);
        }
    }
    // An entity that already carries state wins over another one that happens to share its name.
    for kind in registry.types() {
        for entity in (kind.entities)(world) {
            if world.get::<RetiredEntity>(entity).is_none()
                && let Some(name) = world.get::<Name>(entity)
            {
                by_name.insert(name.as_str().to_string(), entity);
            }
        }
    }
    let wanted: BTreeSet<(&str, &str)> = server
        .iter()
        .map(|state| (state.entity.as_str(), state.id.as_str()))
        .collect();
    let server_names: BTreeSet<&str> = server.iter().map(|state| state.entity.as_str()).collect();
    for kind in registry.types() {
        for entity in (kind.entities)(world) {
            if world.get_entity(entity).is_err() || world.get::<RetiredEntity>(entity).is_some() {
                continue;
            }
            let Some(name) = world.get::<Name>(entity) else {
                continue;
            };
            let name = name.as_str().to_string();
            if wanted.contains(&(name.as_str(), kind.id))
                || (keep.contains(&name) && !server_names.contains(name.as_str()))
            {
                continue;
            }
            if !server_names.contains(name.as_str()) && world.get::<PhysicsBody>(entity).is_none() {
                world.despawn(entity);
                by_name.remove(&name);
            } else {
                (kind.remove)(world, entity);
            }
        }
    }
    // `server` is sorted, so missing entities are created in name order.
    for state in server {
        if retired.contains(&state.entity) && !by_name.contains_key(&state.entity) {
            continue;
        }
        let Some(kind) = registry.get(&state.id) else {
            continue;
        };
        let entity = *by_name.entry(state.entity.clone()).or_insert_with(|| {
            world
                .spawn((Name::new(state.entity.clone()), MadeFromState))
                .id()
        });
        if let Err(message) = (kind.write)(world, entity, &state.data) {
            error!("applying `{}`: {message}", state.entity);
        }
    }
}
