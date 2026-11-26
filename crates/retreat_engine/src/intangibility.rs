//! Collision filtering for the contract's [`Intangible`] (`intangible.rs`): fixed bodies are in
//! one group, moving bodies in another, and an intangible body only meets the fixed group.
//!
//! The groups are written onto the physics library's colliders directly, every tick where they
//! differ: the colliders are rewound with the physics snapshot while `Intangible` is rewound with
//! the module state, and comparing against the collider itself keeps them in step.
//!
//! Command: `physics.set_intangible <name> <0|1>`.

use bevy::prelude::*;
use bevy_rapier3d::plugin::context::{RapierContextColliders, RapierRigidBodySet};
use bevy_rapier3d::rapier::geometry::{Group, InteractionGroups, InteractionTestMode};
use retreat_engine_api::{
    CommandArgs, CommandOutcome, CommandResult, Intangible, PhysicsBody, PhysicsBodyKind,
};

use crate::entity_names::find_unique_entity_by_name;
use crate::physics::PhysicsProxy;

const FIXED_GROUP: Group = Group::GROUP_1;
const MOVING_GROUP: Group = Group::GROUP_2;
const INTANGIBLE_GROUP: Group = Group::GROUP_3;

/// The collision groups of a body.
fn groups(kind: PhysicsBodyKind, intangible: bool) -> InteractionGroups {
    let (memberships, filter) = if intangible {
        (INTANGIBLE_GROUP, FIXED_GROUP)
    } else if kind == PhysicsBodyKind::Fixed {
        (FIXED_GROUP, Group::ALL)
    } else {
        (MOVING_GROUP, Group::ALL)
    };
    InteractionGroups::new(memberships, filter, InteractionTestMode::And)
}

/// Gives every body's colliders the groups its kind and [`Intangible`] ask for.
pub(crate) fn apply_collision_groups(
    owners: Query<(&PhysicsBody, &PhysicsProxy, Has<Intangible>)>,
    mut contexts: Query<(&RapierRigidBodySet, &mut RapierContextColliders)>,
) {
    let Ok((bodies, mut context)) = contexts.single_mut() else {
        return;
    };
    for (body, proxy, intangible) in &owners {
        let wanted = groups(body.kind, intangible);
        let Some(handle) = bodies.entity2body().get(&proxy.entity()) else {
            continue;
        };
        let Some(rigid_body) = bodies.bodies.get(*handle) else {
            continue;
        };
        for collider in rigid_body.colliders() {
            let differs = context
                .colliders
                .get(*collider)
                .is_some_and(|collider| collider.collision_groups() != wanted);
            if differs && let Some(collider) = context.colliders.get_mut(*collider) {
                collider.set_collision_groups(wanted);
            }
        }
    }
}

/// `physics.set_intangible <name> <0|1>`.
pub(crate) fn set_intangible_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let name: String = args.parse(0, "name")?;
    let on: u8 = args.parse(1, "on")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    if !world.entity(entity).contains::<PhysicsBody>() {
        return Err(args.failure(format!("`{name}` is not a physics body")));
    }
    if on == 0 {
        world.entity_mut(entity).remove::<Intangible>();
    } else {
        world.entity_mut(entity).insert(Intangible);
    }
    Ok(CommandOutcome::Continue)
}
