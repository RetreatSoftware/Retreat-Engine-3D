//! Physics commands and the state hash in an app without a window or GPU: boxes fall and come
//! to rest, the same script gives the same hash, and errors. The rendered result is checked by
//! `scripts/verify/physics_boxes.cmds`.

use bevy::prelude::*;
use bevy_rapier3d::prelude::{Collider, RapierRigidBodySet, RigidBody};
use retreat_engine::{
    CommandScript, LastStateHash, PhysicsBody, PhysicsBox, PhysicsPlugin, RetreatCommandsPlugin,
    StateHashPlugin,
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
        PhysicsBody {
            body: RigidBody::Fixed,
            collider: Collider::cuboid(10.0, GROUND_HALF_THICKNESS, 10.0),
        },
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
fn a_box_starts_simulating_at_its_spawn_point() {
    // Rapier builds a body from its GlobalTransform. A box spawned by a script line, in the
    // middle of a fixed tick, must still start from its spawn point, not the world origin.
    let mut app = test_app(
        "physics.spawn_box crate 0 3 0 1
",
    );
    // Update 1: no tick. Update 2: tick 1 runs the line, then one physics step.
    app.update();
    app.update();
    let after_first_step = position_of(&mut app, "crate");
    assert!(
        (after_first_step.y - 3.0).abs() < 0.01,
        "after one step the box must be just below its spawn height 3, got y = {}",
        after_first_step.y
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
    let rigid_bodies = world
        .query_filtered::<(), With<RigidBody>>()
        .iter(world)
        .count();
    assert_eq!(
        rigid_bodies, 1,
        "despawning a box must despawn its proxy too"
    );
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

/// Names of the Rapier bodies in Rapier's own numbering order (the order that decides how
/// contacts are solved).
fn rapier_body_order(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut sets = world.query::<&RapierRigidBodySet>();
    let set = sets.single(world).expect("one Rapier context");
    let entities: Vec<Entity> = set
        .bodies
        .iter()
        .map(|(_, body)| Entity::from_bits(body.user_data as u64))
        .collect();
    entities
        .into_iter()
        .map(|entity| {
            world
                .get::<Name>(entity)
                .expect("proxies are named")
                .to_string()
        })
        .collect()
}

#[derive(Component)]
struct MarkerA;

#[derive(Component)]
struct MarkerB;

#[test]
fn bodies_added_in_one_tick_are_numbered_in_name_order_whatever_their_components() {
    // Regression: Rapier used to number bodies in Bevy's storage order, which depends on each
    // entity's components and differed between Windows and Linux.
    let mut app = test_app(
        "wait 1
quit
",
    );
    let body = || PhysicsBody {
        body: RigidBody::Dynamic,
        collider: Collider::ball(0.5),
    };
    // Spawned in reverse name order, each with a different set of components.
    app.world_mut().spawn((
        Name::new("c"),
        body(),
        Transform::from_xyz(4.0, 3.0, 0.0),
        MarkerA,
    ));
    app.world_mut().spawn((
        Name::new("b"),
        body(),
        Transform::from_xyz(2.0, 3.0, 0.0),
        MarkerB,
    ));
    app.world_mut()
        .spawn((Name::new("a"), body(), Transform::from_xyz(0.0, 3.0, 0.0)));
    app.update();
    app.update();
    assert_eq!(
        rapier_body_order(&mut app),
        vec!["a/physics", "b/physics", "c/physics", "ground/physics"]
    );
}

#[test]
fn rapier_components_on_a_game_entity_fail_the_script() {
    let mut app = test_app(
        "wait 3
quit
",
    );
    app.world_mut().spawn((
        Name::new("rogue"),
        RigidBody::Dynamic,
        Collider::ball(0.5),
        Transform::IDENTITY,
    ));
    for _ in 0..10 {
        app.update();
    }
    let failure = app
        .world()
        .resource::<CommandScript>()
        .failure()
        .map(String::from)
        .expect("direct RigidBody/Collider must fail the script");
    assert!(
        failure.contains("entity `rogue` has Rapier's RigidBody/Collider directly"),
        "{failure}"
    );
}

#[test]
fn a_physics_body_without_a_name_fails_the_script() {
    let mut app = test_app(
        "wait 3
quit
",
    );
    app.world_mut().spawn(PhysicsBody {
        body: RigidBody::Dynamic,
        collider: Collider::ball(0.5),
    });
    for _ in 0..10 {
        app.update();
    }
    let failure = app
        .world()
        .resource::<CommandScript>()
        .failure()
        .map(String::from)
        .expect("an unnamed PhysicsBody must fail the script");
    assert!(
        failure.contains("has a PhysicsBody but no Name"),
        "{failure}"
    );
}
