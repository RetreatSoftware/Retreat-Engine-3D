//! Physics: Rapier (`bevy_rapier3d`) stepped in `FixedUpdate`, plus `physics.spawn_box` and
//! `physics.reset`.
//!
//! Determinism: Rapier steps exactly once per fixed tick, by the fixed timestep, after the
//! script line of that tick (`CommandScriptSystems`). While a script waits for pending work
//! no tick runs, so the simulation does not advance either. `bevy_rapier3d` is built with
//! `enhanced-determinism`, so the same steps give bit-identical results on every platform;
//! `state.hash` (in `state_hash.rs`) checks that.

use bevy::prelude::*;
use bevy_rapier3d::prelude::{
    Collider, NoUserData, PhysicsSet, RapierPhysicsPlugin, RigidBody, TimestepMode,
};
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult};

use crate::command_script::CommandScriptSystems;
use crate::entity_names::ensure_name_is_free;

/// Color of boxes spawned by `physics.spawn_box`.
const PHYSICS_BOX_COLOR: Color = Color::srgb(0.85, 0.35, 0.25);

/// Adds Rapier and the physics commands. Needs Bevy's `TimePlugin` (for the fixed timestep)
/// and `TransformPlugin` to be added first; `RetreatEnginePlugin` and tests do that.
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
            .register_command(
                "physics.spawn_box",
                "physics.spawn_box <name> <x> <y> <z> <size>",
                spawn_box_command,
            )
            .register_command("physics.reset", "physics.reset", reset_command);
    }
}

/// Marks a box created by `physics.spawn_box`, so `physics.reset` removes exactly those.
#[derive(Component)]
pub struct PhysicsBox;

/// `physics.spawn_box <name> <x> <y> <z> <size>`: a dynamic cube with edge `size`, centered at
/// (x, y, z), that falls under gravity and collides with every other collider.
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

    let mesh = world
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(size, size, size));
    let material = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(PHYSICS_BOX_COLOR);
    let half_size = size / 2.0;
    world.spawn((
        Name::new(name),
        PhysicsBox,
        RigidBody::Dynamic,
        Collider::cuboid(half_size, half_size, half_size),
        Transform::from_xyz(x, y, z),
        Mesh3d(mesh),
        MeshMaterial3d(material),
    ));
    Ok(CommandOutcome::Continue)
}

/// `physics.reset`: removes every box made by `physics.spawn_box`. The rest of the scene
/// (ground, fixed colliders) stays.
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
