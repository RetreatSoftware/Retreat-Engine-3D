//! Physics: Rapier (`bevy_rapier3d`) stepped in `FixedUpdate`, engine-owned physics bodies, and
//! the commands `physics.spawn_box` and `physics.reset`.
//!
//! Determinism: Rapier steps exactly once per fixed tick, by the fixed timestep, after the
//! script line of that tick (`CommandScriptSystems`). While a script waits for pending work
//! no tick runs, so the simulation does not advance either. `bevy_rapier3d` is built with
//! `enhanced-determinism`, so the same steps give bit-identical results on every platform;
//! `state.hash` (in `state_hash.rs`) checks that.
//!
//! **Engine-owned bodies.** Game code never puts Rapier's `RigidBody` / `Collider` on its own
//! entities. It adds a `retreat_engine_api::PhysicsBody` description (library-neutral, so
//! modules never name Rapier); each tick the engine creates one *proxy*
//! entity per new description, **in name order**, and the proxy carries the Rapier components.
//! After each physics step the proxy's pose is copied back to the game entity.
//! Why: Rapier numbers bodies in the order bevy_rapier's queries visit them, which follows
//! Bevy's internal storage layout (each entity's exact set of components and the history of
//! the world). Bodies added in the same tick were numbered differently on Windows and Linux,
//! so contacts resolved in a different order and `state.hash` differed. Proxies all have the
//! same components and are created in a canonical order, so the numbering is the same
//! everywhere. Direct use of `RigidBody` / `Collider` is rejected with an error.

use bevy::prelude::*;
use bevy_rapier3d::plugin::context::{RapierContextColliders, RapierRigidBodySet};
use bevy_rapier3d::prelude::{
    Collider, NoUserData, PhysicsSet, RapierPhysicsPlugin, RigidBody, TimestepMode,
};
use bevy_rapier3d::rapier::dynamics::CoefficientCombineRule;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, InputActionSystems, PhysicsBody,
    PhysicsBodyKind, PhysicsControlSystems, PhysicsShape, PhysicsVelocity, PhysicsVelocityChange,
    PhysicsVelocitySet, Replicated, RetiredEntity,
};

use crate::command_script::{CommandScriptSystems, fail_running_script};
use crate::entity_names::{ensure_name_is_free, find_unique_entity_by_name};
use crate::physics_queries::add_physics_queries;

/// Color of boxes spawned by `physics.spawn_box`.
const PHYSICS_BOX_COLOR: Color = Color::srgb(0.85, 0.35, 0.25);

/// Suffix of a proxy's name: the proxy of `crate` is `crate/physics`.
pub const PHYSICS_PROXY_NAME_SUFFIX: &str = "/physics";

/// Adds Rapier, engine-owned bodies and the physics commands. Needs Bevy's `TimePlugin` (for
/// the fixed timestep) and `TransformPlugin` to be added first; `RetreatEnginePlugin` and
/// tests do that.
pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        let fixed_timestep = app
            .world()
            .get_resource::<Time<Fixed>>()
            .expect("PhysicsPlugin needs Bevy's TimePlugin first (it owns the fixed timestep)")
            .timestep();
        add_physics_queries(app);
        app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default().in_fixed_schedule())
            // One physics step per fixed tick, of exactly the fixed timestep: one source of
            // truth for "how long is a tick".
            .insert_resource(TimestepMode::Fixed {
                dt: fixed_timestep.as_secs_f32(),
                substeps: 1,
            })
            .configure_sets(
                FixedUpdate,
                CommandScriptSystems.before(PhysicsSet::SyncBackend),
            )
            .add_systems(
                FixedUpdate,
                (
                    reject_rapier_components_outside_proxies,
                    crate::retired_bodies::retire_requested_bodies,
                    crate::retired_bodies::purge_retired_bodies,
                    create_physics_proxies,
                )
                    .chain()
                    .after(CommandScriptSystems)
                    .before(PhysicsControlSystems),
            )
            .configure_sets(
                FixedUpdate,
                PhysicsControlSystems
                    .after(CommandScriptSystems)
                    .after(InputActionSystems)
                    .before(PhysicsSet::SyncBackend),
            )
            .add_systems(
                FixedUpdate,
                (
                    apply_velocity_requests
                        .after(PhysicsControlSystems)
                        .before(PhysicsSet::SyncBackend),
                    configure_character_bodies
                        .after(PhysicsSet::SyncBackend)
                        .before(PhysicsSet::StepSimulation),
                    (copy_proxy_poses_to_owners, sync_body_readouts)
                        .chain()
                        .after(PhysicsSet::Writeback),
                ),
            )
            .register_command(
                "physics.spawn_box",
                "physics.spawn_box <name> <x> <y> <z> <size>",
                spawn_box_command,
            )
            .register_command("physics.reset", "physics.reset", reset_command)
            .register_command(
                "physics.spawn_character",
                "physics.spawn_character <name> <x> <y> <z> <half_height> <radius>",
                spawn_character_command,
            )
            .register_command(
                "physics.spawn_wall",
                "physics.spawn_wall <name> <x> <y> <z> <size_x> <size_y> <size_z>",
                spawn_wall_command,
            )
            .register_command(
                "physics.set_velocity",
                "physics.set_velocity <name> <vx> <vy> <vz>",
                set_velocity_command,
            )
            .register_command(
                "assert.velocity",
                "assert.velocity <name> <vx> <vy> <vz> <tolerance>",
                assert_velocity_command,
            )
            .register_command(
                "physics.push",
                "physics.push <name> <vx> <vy> <vz>",
                push_command,
            );
        crate::retired_bodies::register_remove_commands(app);
    }
}

/// The Rapier body and collider for a contract-crate [`PhysicsBody`]. The only place that maps
/// the library-neutral description onto Rapier.
fn rapier_components(physics_body: &PhysicsBody) -> (RigidBody, Collider) {
    let body = match physics_body.kind {
        PhysicsBodyKind::Dynamic | PhysicsBodyKind::Character => RigidBody::Dynamic,
        PhysicsBodyKind::Fixed => RigidBody::Fixed,
    };
    let shape = match physics_body.shape {
        PhysicsShape::Cuboid { half_extents } => {
            Collider::cuboid(half_extents.x, half_extents.y, half_extents.z)
        }
        PhysicsShape::Ball { radius } => Collider::ball(radius),
        PhysicsShape::Capsule {
            half_height,
            radius,
        } => Collider::capsule_y(half_height, radius),
    };
    let collider = if physics_body.shape_offset == Vec3::ZERO {
        shape
    } else {
        Collider::compound(vec![(physics_body.shape_offset, Quat::IDENTITY, shape)])
    };
    (body, collider)
}

/// On a proxy: the game entity it simulates. Despawning that entity despawns the proxy.
#[derive(Component, Debug)]
#[relationship(relationship_target = PhysicsProxy)]
pub struct PhysicsProxyOf(pub Entity);

/// On a game entity with a [`PhysicsBody`]: its proxy (set by the engine).
#[derive(Component, Debug)]
#[relationship_target(relationship = PhysicsProxyOf, linked_spawn)]
pub struct PhysicsProxy(Entity);

impl PhysicsProxy {
    /// The proxy entity that carries the Rapier components.
    pub fn entity(&self) -> Entity {
        self.0
    }
}

/// Marks a box created by `physics.spawn_box`, so `physics.reset` removes exactly those.
#[derive(Component)]
pub struct PhysicsBox;

/// Fails the running script (and logs an error) for every entity that got Rapier's
/// `RigidBody` or `Collider` directly instead of through a [`PhysicsBody`].
fn reject_rapier_components_outside_proxies(world: &mut World) {
    let mut query = world.query_filtered::<(Entity, Option<&Name>), (
        Or<(Added<RigidBody>, Added<Collider>)>,
        Without<PhysicsProxyOf>,
    )>();
    let offenders: Vec<String> = query
        .iter(world)
        .map(|(entity, name)| describe_entity(entity, name))
        .collect();
    if let Some(first) = offenders.first() {
        report_physics_error(
            world,
            format!(
                "entity {first} has Rapier's RigidBody/Collider directly; add a \
                 PhysicsBody (retreat_engine_api) instead, so the engine registers bodies in a \
                 deterministic order (see retreat_engine physics.rs)"
            ),
        );
    }
}

/// Creates one proxy per newly added [`PhysicsBody`], in name order, with the same set of
/// components on every proxy.
fn create_physics_proxies(world: &mut World) {
    let mut query = world.query_filtered::<(
        Entity,
        Option<&Name>,
        &PhysicsBody,
        &Transform,
        Has<ChildOf>,
    ), (
        Added<PhysicsBody>,
        Without<retreat_engine_api::RetiredEntity>,
    )>();
    let mut new_bodies: Vec<(String, Entity, PhysicsBody, Transform)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (owner, name, physics_body, transform, has_parent) in query.iter(world) {
        let Some(name) = name else {
            errors.push(format!(
                "entity {owner} has a PhysicsBody but no Name; physics bodies need a unique \
                 Name, which sets their deterministic order"
            ));
            continue;
        };
        if has_parent {
            errors.push(format!(
                "`{name}` has a PhysicsBody and a parent; physics bodies must be root entities \
                 (their Transform is their world pose)"
            ));
            continue;
        }
        new_bodies.push((
            name.as_str().to_string(),
            owner,
            physics_body.clone(),
            *transform,
        ));
    }
    for error in errors {
        report_physics_error(world, error);
    }
    // The canonical order: by name (names are unique), never by storage order.
    new_bodies.sort_by(|(left, ..), (right, ..)| left.cmp(right));
    for (name, owner, physics_body, transform) in new_bodies {
        let (body, collider) = rapier_components(&physics_body);
        world.spawn((
            Name::new(format!("{name}{PHYSICS_PROXY_NAME_SUFFIX}")),
            PhysicsProxyOf(owner),
            body,
            collider,
            transform,
        ));
    }
}

/// One entity's pending velocity requests.
struct VelocityRequest {
    entity: Entity,
    set: Option<Vec3>,
    change: Option<Vec3>,
    proxy: Option<Entity>,
}

/// Applies every [`PhysicsVelocitySet`] and then [`PhysicsVelocityChange`] whose body exists in
/// the physics world, and removes the requests. A request on an entity whose body does not exist yet
/// waits for the tick it appears; one on a body that cannot move is dropped with a logged warning.
fn apply_velocity_requests(world: &mut World) {
    let mut requests = world.query_filtered::<(
        Entity,
        Option<&PhysicsVelocitySet>,
        Option<&PhysicsVelocityChange>,
        Option<&PhysicsProxy>,
    ), Or<(With<PhysicsVelocitySet>, With<PhysicsVelocityChange>)>>();
    let pending: Vec<VelocityRequest> = requests
        .iter(world)
        .map(|(entity, set, change, proxy)| VelocityRequest {
            entity,
            set: set.map(|set| set.0),
            change: change.map(|change| change.0),
            proxy: proxy.map(PhysicsProxy::entity),
        })
        .collect();
    if pending.is_empty() {
        return;
    }
    let mut applied: Vec<Entity> = Vec::new();
    {
        let mut sets = world.query::<&mut RapierRigidBodySet>();
        let Ok(mut set) = sets.single_mut(world) else {
            return;
        };
        for request in &pending {
            let handle = request
                .proxy
                .and_then(|proxy| set.entity2body().get(&proxy).copied());
            let Some(body) = handle.and_then(|handle| set.bodies.get_mut(handle)) else {
                continue;
            };
            if body.is_dynamic() {
                let velocity = request.set.unwrap_or_else(|| body.linvel())
                    + request.change.unwrap_or(Vec3::ZERO);
                body.set_linvel(velocity, true);
            } else {
                warn!(
                    "a velocity request on a fixed body was ignored (entity {})",
                    request.entity
                );
            }
            applied.push(request.entity);
        }
    }
    for entity in applied {
        world
            .entity_mut(entity)
            .remove::<(PhysicsVelocitySet, PhysicsVelocityChange)>();
    }
}

/// Gives a new character body its properties (see `PhysicsBodyKind::Character`) before its first
/// step. They are set on the physics body itself, not as extra components, because every proxy
/// must have the same components (see the module docs).
fn configure_character_bodies(world: &mut World) {
    let mut owners =
        world.query_filtered::<(&PhysicsBody, &PhysicsProxy), Without<RetiredEntity>>();
    let characters: Vec<Entity> = owners
        .iter(world)
        .filter(|(body, _)| body.kind == PhysicsBodyKind::Character)
        .map(|(_, proxy)| proxy.entity())
        .collect();
    if characters.is_empty() {
        return;
    }
    let mut contexts = world.query::<(&mut RapierRigidBodySet, &mut RapierContextColliders)>();
    let Ok((mut set, mut colliders)) = contexts.single_mut(world) else {
        return;
    };
    for proxy in characters {
        let Some(handle) = set.entity2body().get(&proxy).copied() else {
            continue;
        };
        let Some(body) = set.bodies.get_mut(handle) else {
            continue;
        };
        if body.gravity_scale() == 0.0 {
            continue;
        }
        body.set_gravity_scale(0.0, false);
        body.lock_rotations(true, false);
        body.enable_ccd(true);
        body.activation_mut().normalized_linear_threshold = -1.0;
        body.activation_mut().angular_threshold = -1.0;
        let attached = body.colliders().to_vec();
        for collider in attached {
            if let Some(collider) = colliders.colliders.get_mut(collider) {
                collider.set_friction(0.0);
                collider.set_friction_combine_rule(CoefficientCombineRule::Min);
            }
        }
    }
}

/// Copies every live body's pose and velocity from the physics body set to its game entity
/// (`Transform`, [`PhysicsVelocity`]). Runs after each step, and after a rewind or a server
/// correction, so game code never reads a pose from the future.
pub(crate) fn sync_body_readouts(world: &mut World) {
    let mut owners = world.query_filtered::<(Entity, &PhysicsProxy), Without<RetiredEntity>>();
    let owners: Vec<(Entity, Entity)> = owners
        .iter(world)
        .map(|(owner, proxy)| (owner, proxy.entity()))
        .collect();
    let mut sets = world.query::<&RapierRigidBodySet>();
    let Ok(set) = sets.single(world) else {
        return;
    };
    let readouts: Vec<(Entity, Vec3, Quat, PhysicsVelocity)> = owners
        .into_iter()
        .filter_map(|(owner, proxy)| {
            let body = set.bodies.get(*set.entity2body().get(&proxy)?)?;
            let velocity = PhysicsVelocity {
                linear: body.linvel(),
                angular: body.angvel(),
            };
            Some((owner, body.translation(), *body.rotation(), velocity))
        })
        .collect();
    for (owner, translation, rotation, velocity) in readouts {
        let Ok(mut entity) = world.get_entity_mut(owner) else {
            continue;
        };
        if let Some(mut transform) = entity.get_mut::<Transform>()
            && (transform.translation != translation || transform.rotation != rotation)
        {
            transform.translation = translation;
            transform.rotation = rotation;
        }
        if let Some(mut current) = entity.get_mut::<PhysicsVelocity>() {
            current.set_if_neq(velocity);
        }
    }
}

/// After each physics step: the game entity takes the pose Rapier computed for its proxy.
fn copy_proxy_poses_to_owners(
    proxies: Query<(&PhysicsProxyOf, &Transform), Changed<Transform>>,
    mut owners: Query<&mut Transform, Without<PhysicsProxyOf>>,
) {
    for (proxy_of, proxy_transform) in &proxies {
        if let Ok(mut owner_transform) = owners.get_mut(proxy_of.0) {
            *owner_transform = *proxy_transform;
        }
    }
}

fn describe_entity(entity: Entity, name: Option<&Name>) -> String {
    match name {
        Some(name) => format!("`{name}`"),
        None => format!("{entity}"),
    }
}

/// Logs a physics setup error and fails the running script, if any.
fn report_physics_error(world: &mut World, message: String) {
    error!("{message}");
    fail_running_script(world, message);
}

/// `physics.spawn_box <name> <x> <y> <z> <size>`: a dynamic cube with edge `size`, centered at
/// (x, y, z), that falls under gravity and collides with every other body.
fn spawn_box_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(5)?;
    let name: String = args.parse(0, "name")?;
    let x: f32 = args.parse(1, "x")?;
    let y: f32 = args.parse(2, "y")?;
    let z: f32 = args.parse(3, "z")?;
    let size: f32 = args.parse(4, "size")?;
    if size <= 0.0 {
        return Err(args.failure(format!("<size> must be greater than 0, got {size}")));
    }
    ensure_name_is_free(world, args, &name)?;
    ensure_name_is_free(world, args, &format!("{name}{PHYSICS_PROXY_NAME_SUFFIX}"))?;

    spawn_physics_box(world, &name, Vec3::new(x, y, z), size);
    Ok(CommandOutcome::Continue)
}

/// Spawns a dynamic box (game entity with mesh and [`PhysicsBody`], marked [`PhysicsBox`]).
/// The caller checks that `name` and `<name>/physics` are free and that `size` is positive;
/// `physics.spawn_box` does, and so must any other caller (e.g. a toolbar button).
pub fn spawn_physics_box(world: &mut World, name: &str, center: Vec3, size: f32) -> Entity {
    let mesh = world
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(size, size, size));
    let material = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(PHYSICS_BOX_COLOR);
    world
        .spawn((
            Name::new(name.to_string()),
            PhysicsBox,
            PhysicsBody::dynamic_cube(size),
            Transform::from_translation(center),
            Mesh3d(mesh),
            MeshMaterial3d(material),
        ))
        .id()
}

/// Spawns a body from its replicated description (any [`PhysicsShape`]), marked [`PhysicsBox`] so
/// `physics.reset` finds it. The mesh follows the shape; a `shape_offset` moves the collider only.
/// Same caller duties as [`spawn_physics_box`].
pub fn spawn_body_from_description(
    world: &mut World,
    name: &str,
    description: &PhysicsBody,
    replicated: Replicated,
    transform: Transform,
) -> Entity {
    let mesh: Mesh = match description.shape {
        PhysicsShape::Cuboid { half_extents } => Cuboid::new(
            half_extents.x * 2.0,
            half_extents.y * 2.0,
            half_extents.z * 2.0,
        )
        .into(),
        PhysicsShape::Ball { radius } => Sphere::new(radius).into(),
        PhysicsShape::Capsule {
            half_height,
            radius,
        } => Capsule3d::new(radius, half_height * 2.0).into(),
    };
    let mesh = world.resource_mut::<Assets<Mesh>>().add(mesh);
    let material = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(PHYSICS_BOX_COLOR);
    world
        .spawn((
            Name::new(name.to_string()),
            PhysicsBox,
            description.clone(),
            replicated,
            transform,
            Mesh3d(mesh),
            MeshMaterial3d(material),
        ))
        .id()
}

/// `physics.reset`: removes every box made by `physics.spawn_box` (and so their proxies). The
/// rest of the scene (ground, fixed bodies) stays.
fn reset_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let boxes: Vec<Entity> = world
        .query_filtered::<Entity, With<PhysicsBox>>()
        .iter(world)
        .collect();
    for entity in boxes {
        world.despawn(entity);
    }
    Ok(CommandOutcome::Continue)
}

/// `physics.push <name> <vx> <vy> <vz>`: adds a velocity change (units per second) to a dynamic
/// body and wakes it. It is the engine's simplest "player acts on the world" command, and what the
/// networking harness uses as an input.
fn push_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let name: String = args.parse(0, "name")?;
    let change = Vec3::new(
        args.parse(1, "vx")?,
        args.parse(2, "vy")?,
        args.parse(3, "vz")?,
    );
    if !change.is_finite() {
        return Err(args.failure(format!("the velocity change must be finite, got {change}")));
    }
    let owner = find_unique_entity_by_name(world, args, &name)?;
    let Some(proxy) = world.get::<PhysicsProxy>(owner).map(PhysicsProxy::entity) else {
        return Err(args.failure(format!(
            "`{name}` has no physics body yet (a body is created on the tick after it is spawned)"
        )));
    };
    let mut sets = world.query::<&mut RapierRigidBodySet>();
    let mut set = sets
        .single_mut(world)
        .map_err(|error| args.failure(format!("there is no physics world: {error}")))?;
    let handle = set.entity2body().get(&proxy).copied();
    let Some(body) = handle.and_then(|handle| set.bodies.get_mut(handle)) else {
        return Err(args.failure(format!(
            "`{name}` is not simulated yet (its body appears on the next tick)"
        )));
    };
    if !body.is_dynamic() {
        return Err(args.failure(format!("`{name}` is a fixed body and cannot be pushed")));
    }
    let velocity = body.linvel() + change;
    body.set_linvel(velocity, true);
    sync_body_readouts(world);
    Ok(CommandOutcome::Continue)
}

/// `physics.spawn_character <name> <x> <y> <z> <half_height> <radius>`: an upright character
/// capsule centered at (x, y, z).
fn spawn_character_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(6)?;
    let name: String = args.parse(0, "name")?;
    let center = Vec3::new(
        args.parse(1, "x")?,
        args.parse(2, "y")?,
        args.parse(3, "z")?,
    );
    let body = PhysicsBody::character(args.parse(4, "half_height")?, args.parse(5, "radius")?);
    spawn_described(world, args, &name, body, center)
}

/// `physics.spawn_wall <name> <x> <y> <z> <size_x> <size_y> <size_z>`: a fixed box.
fn spawn_wall_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(7)?;
    let name: String = args.parse(0, "name")?;
    let center = Vec3::new(
        args.parse(1, "x")?,
        args.parse(2, "y")?,
        args.parse(3, "z")?,
    );
    let size = Vec3::new(
        args.parse(4, "size_x")?,
        args.parse(5, "size_y")?,
        args.parse(6, "size_z")?,
    );
    let body = PhysicsBody {
        kind: PhysicsBodyKind::Fixed,
        shape: PhysicsShape::Cuboid {
            half_extents: size / 2.0,
        },
        shape_offset: Vec3::ZERO,
    };
    spawn_described(world, args, &name, body, center)
}

fn spawn_described(
    world: &mut World,
    args: &CommandArgs,
    name: &str,
    body: PhysicsBody,
    center: Vec3,
) -> CommandResult {
    if !body.shape.is_valid() || !center.is_finite() {
        return Err(args.failure("sizes must be greater than 0 and every number finite"));
    }
    ensure_name_is_free(world, args, name)?;
    ensure_name_is_free(world, args, &format!("{name}{PHYSICS_PROXY_NAME_SUFFIX}"))?;
    spawn_body_from_description(
        world,
        name,
        &body,
        Replicated::full(),
        Transform::from_translation(center),
    );
    Ok(CommandOutcome::Continue)
}

/// `physics.set_velocity <name> <vx> <vy> <vz>`: asks for the body's linear velocity to be set
/// (applied in this tick's physics, or when the body exists).
fn set_velocity_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let name: String = args.parse(0, "name")?;
    let velocity = Vec3::new(
        args.parse(1, "vx")?,
        args.parse(2, "vy")?,
        args.parse(3, "vz")?,
    );
    if !velocity.is_finite() {
        return Err(args.failure(format!("the velocity must be finite, got {velocity}")));
    }
    let owner = find_unique_entity_by_name(world, args, &name)?;
    if !world.entity(owner).contains::<PhysicsBody>() {
        return Err(args.failure(format!("`{name}` is not a physics body")));
    }
    world.entity_mut(owner).insert(PhysicsVelocitySet(velocity));
    Ok(CommandOutcome::Continue)
}

/// `assert.velocity <name> <vx> <vy> <vz> <tolerance>`: the body's linear velocity.
fn assert_velocity_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(5)?;
    let name: String = args.parse(0, "name")?;
    let expected = Vec3::new(
        args.parse(1, "vx")?,
        args.parse(2, "vy")?,
        args.parse(3, "vz")?,
    );
    let tolerance: f32 = args.parse(4, "tolerance")?;
    let owner = find_unique_entity_by_name(world, args, &name)?;
    let Some(velocity) = world.get::<PhysicsVelocity>(owner) else {
        return Err(args.failure(format!("`{name}` is not a physics body")));
    };
    if velocity.linear.distance(expected) <= tolerance {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "`{name}` moves at {}, expected {expected} within {tolerance}",
            velocity.linear
        )))
    }
}
