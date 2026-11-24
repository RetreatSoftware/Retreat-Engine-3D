//! Deferred destruction of physics bodies (`RemovePhysicsBody`), so a rollback can always restore.
//!
//! A despawned body cannot be brought back by `restore_physics`, so a client that rewinds past the
//! despawn would refuse the correction. Instead of despawning, a body is **retired**:
//! - `retire_requested_bodies` (a tick system) turns a `RemovePhysicsBody` request into a
//!   [`RetiredBody`] on the game entity and its proxy, hides the entity and disables its Rapier body
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

use std::time::Duration;

use bevy::prelude::*;
use bevy_rapier3d::plugin::context::RapierRigidBodySet;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, PhysicsBody, RemovePhysicsBody,
};

use crate::entity_names::find_unique_entity_by_name;
use crate::physics::PhysicsProxy;

/// Fixed ticks a retired body is kept: the most a client can rewind (its history length).
/// `net::MAX_HISTORY_TICKS` is this value, so there is one source of truth.
pub const ROLLBACK_WINDOW_TICKS: u64 = 512;

/// On a retired game entity and on its proxy: the fixed time at which it was retired.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetiredBody {
    pub since: Duration,
}

/// Registers `physics.remove`.
pub(crate) fn register_remove_command(app: &mut App) {
    app.register_command("physics.remove", "physics.remove <name>", remove_command);
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

/// Retires every body that has a [`RemovePhysicsBody`] request.
pub(crate) fn retire_requested_bodies(world: &mut World) {
    let mut requests = world.query_filtered::<(Entity, Option<&PhysicsProxy>, Has<RetiredBody>), With<RemovePhysicsBody>>();
    let requested: Vec<(Entity, Option<Entity>, bool)> = requests
        .iter(world)
        .map(|(entity, proxy, retired)| (entity, proxy.map(PhysicsProxy::entity), retired))
        .collect();
    let since = world.resource::<Time<Fixed>>().elapsed();
    for (entity, proxy, already_retired) in requested {
        world.entity_mut(entity).remove::<RemovePhysicsBody>();
        if already_retired {
            continue;
        }
        let retired = RetiredBody { since };
        world
            .entity_mut(entity)
            .insert((retired, Visibility::Hidden));
        let Some(proxy) = proxy else {
            // No Rapier body yet: `create_physics_proxies` skips retired owners.
            continue;
        };
        world.entity_mut(proxy).insert(retired);
        let mut sets = world.query::<&mut RapierRigidBodySet>();
        if let Ok(mut set) = sets.single_mut(world)
            && let Some(handle) = set.entity2body().get(&proxy).copied()
            && let Some(body) = set.bodies.get_mut(handle)
        {
            body.set_enabled(false);
        }
    }
}

/// Despawns bodies retired more than the rollback window ago (their proxy goes with them).
pub(crate) fn purge_retired_bodies(world: &mut World) {
    let timestep = world.resource::<Time<Fixed>>().timestep();
    let now = world.resource::<Time<Fixed>>().elapsed();
    let keep_for = timestep * (ROLLBACK_WINDOW_TICKS as u32 + 1);
    let expired: Vec<Entity> = world
        .query_filtered::<(Entity, &RetiredBody), With<PhysicsBody>>()
        .iter(world)
        .filter(|(_, retired)| now >= retired.since + keep_for)
        .map(|(entity, _)| entity)
        .collect();
    for entity in expired {
        world.despawn(entity);
    }
}
