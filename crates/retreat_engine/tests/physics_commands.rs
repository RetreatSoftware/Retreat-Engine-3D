//! Physics commands and the state hash in an app without a window or GPU: boxes fall and come
//! to rest, the same script gives the same hash, and errors. The rendered result is checked by
//! `scripts/verify/physics_boxes.cmds`.

use bevy::prelude::*;
use bevy_rapier3d::prelude::{Collider, RigidBody};
use retreat_engine::{
    CommandScript, LastStateHash, PhysicsBox, PhysicsPlugin, RetreatCommandsPlugin, StateHashPlugin,
};

/// More updates than any test script needs.
const MAX_UPDATES: u32 = 2000;

/// Half the height of the test ground slab; its top face is at y = 0.
const GROUND_HALF_THICKNESS: f32 = 0.5;

/// How far a resting box may sink into the ground or hover above it. Rapier keeps a small
/// contact margin, so a resting box is not at the exact mathematical height.
const RESTING_HEIGHT_TOLERANCE: f32 = 0.02;

fn test_app(text: &str) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .add_plugins(RetreatCommandsPlugin {
            seed: 0,
            command_script: Some(CommandScript::from_text("test.cmds", text)),
        })
        .add_plugins((PhysicsPlugin, StateHashPlugin));
    // Plugins with work to do after every plugin is added (Rapier) need these two calls;
    // `App::run` makes them, but tests drive `update` by hand.
    app.finish();
    app.cleanup();
    app.world_mut().spawn((
        Name::new("ground"),
        RigidBody::Fixed,
        Collider::cuboid(10.0, GROUND_HALF_THICKNESS, 10.0),
        Transform::from_xyz(0.0, -GROUND_HALF_THICKNESS, 0.0),
    ));
    app
}

/// Runs `text` to the end; returns the app and the script's failure, if any.
fn run_script(text: &str) -> (App, Option<String>) {
    let mut app = test_app(text);
    for _ in 0..MAX_UPDATES {
        app.update();
        if app.should_exit().is_some() {
            let failure = app
                .world()
                .resource::<CommandScript>()
                .failure()
                .map(String::from);
            return (app, failure);
        }
    }
    panic!("the script did not finish within {MAX_UPDATES} updates");
}

fn position_of(app: &mut App, name: &str) -> Vec3 {
    let world = app.world_mut();
    let mut query = world.query::<(&Name, &Transform)>();
    query
        .iter(world)
        .find(|(entity_name, _)| entity_name.as_str() == name)
        .map(|(_, transform)| transform.translation)
        .unwrap_or_else(|| panic!("no entity named `{name}`"))
}

fn last_hash(app: &App) -> u64 {
    app.world()
        .resource::<LastStateHash>()
        .0
        .expect("the script must have run state.hash")
}

#[test]
fn a_box_falls_and_comes_to_rest_on_the_ground() {
    // 1-unit box dropped from 3 units up; 4 s (256 ticks) is plenty to land and settle.
    let (mut app, failure) = run_script("physics.spawn_box crate 0 3 0 1\nwait 256\nquit\n");
    assert_eq!(failure, None);
    let settled = position_of(&mut app, "crate");
    assert!(
        (settled.y - 0.5).abs() < RESTING_HEIGHT_TOLERANCE,
        "a 1-unit box must rest with its center 0.5 above the ground, got y = {}",
        settled.y
    );
    // Resting means it no longer moves: one more simulated second changes nothing visible.
    for _ in 0..64 {
        app.update();
    }
    let later = position_of(&mut app, "crate");
    assert!(
        settled.distance(later) < 1e-4,
        "the box kept moving: {settled} then {later}"
    );
}

#[test]
fn the_same_script_gives_the_same_state_hash() {
    let script = "physics.spawn_box a 0 3 0 1\n\
                  physics.spawn_box b 0.3 5 0.2 0.8\n\
                  wait 120\n\
                  state.hash\n\
                  quit\n";
    let (first, first_failure) = run_script(script);
    let (second, second_failure) = run_script(script);
    assert_eq!(first_failure, None);
    assert_eq!(second_failure, None);
    assert_eq!(last_hash(&first), last_hash(&second));
}

#[test]
fn a_different_start_gives_a_different_state_hash() {
    let (first, _) = run_script("physics.spawn_box a 0 3 0 1\nwait 60\nstate.hash\nquit\n");
    let (second, _) = run_script("physics.spawn_box a 0 3.5 0 1\nwait 60\nstate.hash\nquit\n");
    assert_ne!(last_hash(&first), last_hash(&second));
}

#[test]
fn assert_state_hash_fails_on_a_different_hash() {
    let (_, failure) =
        run_script("physics.spawn_box a 0 3 0 1\nassert.state_hash 0x0000000000000001\nquit\n");
    let failure = failure.expect("a wrong hash must fail");
    assert!(
        failure.starts_with("test.cmds:2: `assert.state_hash` failed: state hash is 0x"),
        "{failure}"
    );
    assert!(
        failure.contains("expected 0x0000000000000001 (2 rigid bodies)"),
        "{failure}"
    );
}

#[test]
fn reset_removes_spawned_boxes_and_keeps_the_scene() {
    let (mut app, failure) = run_script(
        "physics.spawn_box a 0 3 0 1\nphysics.spawn_box b 2 3 0 1\nwait 10\nphysics.reset\nquit\n",
    );
    assert_eq!(failure, None);
    let world = app.world_mut();
    let boxes = world
        .query_filtered::<(), With<PhysicsBox>>()
        .iter(world)
        .count();
    assert_eq!(boxes, 0);
    assert_eq!(position_of(&mut app, "ground").y, -GROUND_HALF_THICKNESS);
}

#[test]
fn a_reset_name_can_be_used_again() {
    let (_, failure) = run_script(
        "physics.spawn_box a 0 3 0 1\nphysics.reset\nphysics.spawn_box a 0 3 0 1\nquit\n",
    );
    assert_eq!(failure, None);
}

#[test]
fn spawn_box_rejects_a_zero_size_and_a_taken_name() {
    let (_, failure) = run_script("physics.spawn_box a 0 3 0 0\nquit\n");
    let failure = failure.expect("size 0 must fail");
    assert!(
        failure.contains("<size> must be greater than 0"),
        "{failure}"
    );

    let (_, failure) = run_script("physics.spawn_box ground 0 3 0 1\nquit\n");
    let failure = failure.expect("a taken name must fail");
    assert!(failure.contains("ground"), "{failure}");
}
