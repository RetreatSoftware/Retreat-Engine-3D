//! Deferred destruction of replicated entities (`RemovePhysicsBody`, `RemoveNetEntity`), so a
//! rollback can always restore.
//!
//! A despawned entity cannot be brought back by `restore_physics`, so a client that rewinds past the
//! despawn would refuse the correction. Instead of despawning, a body is **retired**:
//! - `retire_requested_bodies` (a tick system) turns a `RemovePhysicsBody` request into a
//!   [`RetiredEntity`] on the game entity and its proxy, hides the entity and disables its Rapier body
//!   (the body set is part of the physics snapshot, so a restore re-enables it by itself);
//! - a retired body is invisible to everything that compares machines: `state.hash`, replication
//!   (`read_body_states`) and state application skip it, so server and client agree no matter when
//!   each of them retired it;
//! - `purge_retired_bodies` despawns it once [`ROLLBACK_WINDOW_TICKS`] fixed ticks have passed (the
//!   longest a client can rewind), measured on `Time<Fixed>`, which the snapshot rewinds, so the
//!   purge happens on the same tick everywhere;
//! - `restore_physics` un-retires bodies that were retired after the snapshot; the replay runs the
//!   same input lines and retires them again.
//!
//! Limit: the entity keeps its `Name` until the purge, so a new body cannot reuse the name of a
//! retired one inside the window (the players module uses a fresh id per connection).

use std::collections::BTreeSet;

use bevy::prelude::*;
use bevy_rapier3d::plugin::context::RapierRigidBodySet;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, PhysicsBody, RemoveNetEntity,
    RemovePhysicsBody, RetiredEntity,
};

use crate::entity_names::find_unique_entity_by_name;
use crate::physics::{PhysicsProxy, PhysicsProxyOf};

/// Fixed ticks a retired body is kept: the most a client can rewind (its history length).
/// `net::MAX_HISTORY_TICKS` is this value, so there is one source of truth.
pub const ROLLBACK_WINDOW_TICKS: u64 = 512;

/// Registers `physics.remove` and `net.remove`.
pub(crate) fn register_remove_commands(app: &mut App) {
    app.register_command("physics.remove", "physics.remove <name>", remove_command)
        .register_command("net.remove", "net.remove <name>", remove_net_entity_command);
}

/// `net.remove <name>`: asks for a replicated entity (with or without a body) to be removed safely.
fn remove_net_entity_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    world.entity_mut(entity).insert(RemoveNetEntity);
    Ok(CommandOutcome::Continue)
}

/// `physics.remove <name>`: asks for the body to be removed safely (see the module docs).
fn remove_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    if !world.entity(entity).contains::<PhysicsBody>() {
        return Err(args.failure(format!("`{name}` is not a physics body")));
    }
    world.entity_mut(entity).insert(RemovePhysicsBody);
    Ok(CommandOutcome::Continue)
}

/// Retires every entity that has a [`RemovePhysicsBody`] or [`RemoveNetEntity`] request.
pub(crate) fn retire_requested_bodies(world: &mut World) {
    let mut requests = world.query_filtered::<
        (Entity, Option<&PhysicsProxy>, Has<RetiredEntity>),
        Or<(With<RemovePhysicsBody>, With<RemoveNetEntity>)>,
    >();
    let requested: Vec<(Entity, Option<Entity>, bool)> = requests
        .iter(world)
        .map(|(entity, proxy, retired)| (entity, proxy.map(PhysicsProxy::entity), retired))
        .collect();
    let since = world.resource::<Time<Fixed>>().elapsed();
    for (entity, proxy, already_retired) in requested {
        world
            .entity_mut(entity)
            .remove::<(RemovePhysicsBody, RemoveNetEntity)>();
        if already_retired {
            continue;
        }
        let retired = RetiredEntity { since };
        world
            .entity_mut(entity)
            .insert((retired, Visibility::Hidden));
        let Some(proxy) = proxy else {
            // No Rapier body yet: `create_physics_proxies` skips retired owners.
            continue;
        };
        world.entity_mut(proxy).insert(retired);
        set_body_enabled(world, proxy, false);
    }
}

/// Brings back retired entities **without a physics body** that have one of these names, unless a
/// live entity already has it: a client calls it for what the server still lists (the server refused
/// a removal the client predicted, or has not made it yet). Bodies are not revived this way: a
/// re-enabled Rapier body kept falling through its contacts (found by a harness test), so a body is
/// only brought back by restoring a snapshot taken before its removal.
pub(crate) fn revive_retired_entities(world: &mut World, names: &BTreeSet<&str>) {
    let mut named = world.query_filtered::<
        (Entity, &Name, Has<RetiredEntity>),
        (Without<PhysicsProxyOf>, Without<PhysicsBody>),
    >();
    let mut live: BTreeSet<String> = BTreeSet::new();
    let mut retired: Vec<(String, Entity)> = Vec::new();
    for (entity, name, is_retired) in named.iter(world) {
        if !names.contains(name.as_str()) {
            continue;
        }
        if is_retired {
            retired.push((name.as_str().to_string(), entity));
        } else {
            live.insert(name.as_str().to_string());
        }
    }
    for (name, entity) in retired {
        if live.contains(&name) {
            continue;
        }
        let mut owner = world.entity_mut(entity);
        owner.remove::<RetiredEntity>();
        if owner.contains::<Visibility>() {
            owner.insert(Visibility::Inherited);
        }
    }
}

/// Switches the simulation of a proxy's Rapier body on or off.
fn set_body_enabled(world: &mut World, proxy: Entity, enabled: bool) {
    let mut sets = world.query::<&mut RapierRigidBodySet>();
    if let Ok(mut set) = sets.single_mut(world)
        && let Some(handle) = set.entity2body().get(&proxy).copied()
        && let Some(body) = set.bodies.get_mut(handle)
    {
        body.set_enabled(enabled);
    }
}

/// Despawns entities retired more than the rollback window ago (a body's proxy goes with it).
pub(crate) fn purge_retired_bodies(world: &mut World) {
    let timestep = world.resource::<Time<Fixed>>().timestep();
    let now = world.resource::<Time<Fixed>>().elapsed();
    let keep_for = timestep * (ROLLBACK_WINDOW_TICKS as u32 + 1);
    let expired: Vec<Entity> = world
        .query_filtered::<(Entity, &RetiredEntity), Without<PhysicsProxyOf>>()
        .iter(world)
        .filter(|(_, retired)| now >= retired.since + keep_for)
        .map(|(entity, _)| entity)
        .collect();
    for entity in expired {
        world.despawn(entity);
    }
}
