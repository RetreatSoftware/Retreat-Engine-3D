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
//! entities. It adds a [`PhysicsBody`] description; each tick the engine creates one *proxy*
//! entity per new description, **in name order**, and the proxy carries the Rapier components.
//! After each physics step the proxy's pose is copied back to the game entity.
//! Why: Rapier numbers bodies in the order bevy_rapier's queries visit them, which follows
//! Bevy's internal storage layout (each entity's exact set of components and the history of
//! the world). Bodies added in the same tick were numbered differently on Windows and Linux,
//! so contacts resolved in a different order and `state.hash` differed. Proxies all have the
//! same components and are created in a canonical order, so the numbering is the same
//! everywhere. Direct use of `RigidBody` / `Collider` is rejected with an error.

use bevy::prelude::*;
use bevy_rapier3d::prelude::{
    Collider, NoUserData, PhysicsSet, RapierPhysicsPlugin, RigidBody, TimestepMode,
};
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult};

use crate::command_script::{CommandScriptSystems, fail_running_script};
use crate::entity_names::ensure_name_is_free;

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
                    create_physics_proxies,
                )
                    .chain()
                    .after(CommandScriptSystems)
                    .before(PhysicsSet::SyncBackend),
            )
            .add_systems(
                FixedUpdate,
                copy_proxy_poses_to_owners.after(PhysicsSet::Writeback),
            )
            .register_command(
                "physics.spawn_box",
                "physics.spawn_box <name> <x> <y> <z> <size>",
                spawn_box_command,
            )
            .register_command("physics.reset", "physics.reset", reset_command);
    }
}

/// Makes an entity physical. The entity must have a unique `Name` (it sets the body's place in
/// the canonical order) and must not have a parent (its `Transform` is its world pose). The
/// engine reads this once, in the tick it is added; later changes are not applied.
#[derive(Component, Clone, Debug)]
#[require(Transform)]
pub struct PhysicsBody {
    pub body: RigidBody,
    pub collider: Collider,
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
                 PhysicsBody {{ body, collider }} instead, so the engine registers bodies in a \
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
    ), Added<PhysicsBody>>();
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
        world.spawn((
            Name::new(format!("{name}{PHYSICS_PROXY_NAME_SUFFIX}")),
            PhysicsProxyOf(owner),
            physics_body.body,
            physics_body.collider,
            transform,
        ));
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
    let half_size = size / 2.0;
    world
        .spawn((
            Name::new(name.to_string()),
            PhysicsBox,
            PhysicsBody {
                body: RigidBody::Dynamic,
                collider: Collider::cuboid(half_size, half_size, half_size),
            },
            Transform::from_translation(center),
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
