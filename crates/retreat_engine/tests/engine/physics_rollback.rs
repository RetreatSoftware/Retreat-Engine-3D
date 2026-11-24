//! Rollback spike: capture the physics state at tick N, run on to tick M, restore, re-run the same
//! ticks, and require the same `state.hash`. This is the go/no-go check for client-side prediction
//! on Rapier (FRAMEWORK.md §12: server-authoritative with prediction). It also prints what a
//! snapshot and restore cost, to judge whether a Rapier refactor would pay off.

use std::time::Instant;

use bevy::prelude::*;
use retreat_engine::{
    CommandScript, PhysicsPlugin, RetreatCommandsPlugin, RollbackError, StateHashPlugin,
    capture_physics, compute_state_hash, restore_physics, spawn_physics_box,
};
use retreat_engine_api::{PhysicsBody, PhysicsBodyKind, PhysicsShape};

fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .add_plugins(RetreatCommandsPlugin {
            seed: 0,
            // A script switches the clock to exactly one fixed tick per update.
            command_script: Some(CommandScript::from_text("rollback.cmds", "wait 100000\n")),
        })
        .add_plugins((PhysicsPlugin, StateHashPlugin));
    app.finish();
    app.cleanup();
    app.world_mut().spawn((
        Name::new("ground"),
        PhysicsBody {
            kind: PhysicsBodyKind::Fixed,
            shape: PhysicsShape::Cuboid {
                half_extents: Vec3::new(20.0, 0.5, 20.0),
            },
            shape_offset: Vec3::ZERO,
        },
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    app
}

/// Drops `count` boxes in a staggered grid so they collide with each other and the ground.
fn drop_boxes(app: &mut App, count: u32) {
    for index in 0..count {
        let column = (index % 8) as f32;
        let row = ((index / 8) % 8) as f32;
        let layer = (index / 64) as f32;
        spawn_physics_box(
            app.world_mut(),
            &format!("box_{index:04}"),
            Vec3::new(column * 1.1 - 4.0, 2.0 + layer * 1.3, row * 1.1 - 4.0),
            0.8,
        );
    }
}

fn run_ticks(app: &mut App, ticks: u32) {
    for _ in 0..ticks {
        app.update();
    }
}

fn hash(app: &mut App) -> u64 {
    compute_state_hash(app.world_mut())
        .expect("every body is named")
        .0
}

/// Capture at tick N, run M-N more ticks, restore, run the same M-N ticks: the hash must match.
fn assert_rollback_is_exact(box_count: u32, warmup: u32, window: u32) {
    let mut app = test_app();
    drop_boxes(&mut app, box_count);
    run_ticks(&mut app, warmup);
    let start = Instant::now();
    let snapshot = capture_physics(app.world_mut()).expect("capture");
    let capture_time = start.elapsed();
    let hash_at_capture = hash(&mut app);

    run_ticks(&mut app, window);
    let hash_after_first_run = hash(&mut app);
    assert_ne!(
        hash_at_capture, hash_after_first_run,
        "the bodies must still be moving, or the test proves nothing"
    );

    let start = Instant::now();
    restore_physics(app.world_mut(), &snapshot).expect("restore");
    let restore_time = start.elapsed();
    assert_eq!(
        hash(&mut app),
        hash_at_capture,
        "right after restoring, the state is the captured one"
    );

    let start = Instant::now();
    run_ticks(&mut app, window);
    let resimulate_time = start.elapsed();
    assert_eq!(
        hash(&mut app),
        hash_after_first_run,
        "re-running the same ticks after a restore must give the same state"
    );
    println!(
        "rollback spike: {box_count} boxes, window {window} ticks: capture {capture_time:?}, \
         restore {restore_time:?}, resimulate {resimulate_time:?} ({:?} per tick)",
        resimulate_time / window
    );
}

#[test]
fn rolling_back_a_few_boxes_resimulates_to_the_same_hash() {
    assert_rollback_is_exact(8, 10, 40);
}

#[test]
fn rolling_back_many_boxes_mid_collision_resimulates_to_the_same_hash() {
    assert_rollback_is_exact(128, 30, 60);
}

#[test]
fn a_long_window_still_matches() {
    assert_rollback_is_exact(32, 5, 200);
}

#[test]
fn rolling_back_twice_to_the_same_snapshot_gives_the_same_result_both_times() {
    let mut app = test_app();
    drop_boxes(&mut app, 16);
    run_ticks(&mut app, 12);
    let snapshot = capture_physics(app.world_mut()).expect("capture");
    run_ticks(&mut app, 50);
    let first = hash(&mut app);
    restore_physics(app.world_mut(), &snapshot).expect("restore");
    run_ticks(&mut app, 50);
    assert_eq!(hash(&mut app), first);
    restore_physics(app.world_mut(), &snapshot).expect("restore again");
    run_ticks(&mut app, 50);
    assert_eq!(hash(&mut app), first);
}

#[test]
fn bodies_spawned_inside_the_window_are_removed_and_the_replay_spawns_them_again() {
    let mut app = test_app();
    drop_boxes(&mut app, 8);
    run_ticks(&mut app, 10);
    let snapshot = capture_physics(app.world_mut()).expect("capture");
    let hash_at_capture = hash(&mut app);

    // The window: five ticks, then a new body, then twenty more.
    let play_window = |app: &mut App| {
        run_ticks(app, 5);
        spawn_physics_box(app.world_mut(), "late_box", Vec3::new(0.0, 4.0, 0.0), 0.8);
        run_ticks(app, 20);
    };
    play_window(&mut app);
    let hash_after_first_run = hash(&mut app);

    restore_physics(app.world_mut(), &snapshot).expect("restore");
    assert_eq!(
        hash(&mut app),
        hash_at_capture,
        "the body spawned in the window is gone after the restore"
    );
    // The replay of the same inputs spawns it again, under the same name.
    play_window(&mut app);
    assert_eq!(hash(&mut app), hash_after_first_run);
}

#[test]
fn bodies_despawned_inside_the_window_refuse_the_restore_and_change_nothing() {
    let mut app = test_app();
    drop_boxes(&mut app, 8);
    run_ticks(&mut app, 10);
    let snapshot = capture_physics(app.world_mut()).expect("capture");
    run_ticks(&mut app, 5);

    let mut query = app.world_mut().query::<(Entity, &Name)>();
    let doomed = query
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "box_0003")
        .map(|(entity, _)| entity)
        .expect("box_0003 exists");
    app.world_mut().despawn(doomed);
    run_ticks(&mut app, 2);
    let hash_before = hash(&mut app);

    let error = restore_physics(app.world_mut(), &snapshot).unwrap_err();
    assert_eq!(
        error,
        RollbackError::BodiesDespawned(vec![
            "box_0003".to_string(),
            "box_0003/physics".to_string()
        ])
    );
    assert!(
        error.to_string().contains("delay despawning bodies"),
        "{error}"
    );
    assert_eq!(
        hash(&mut app),
        hash_before,
        "a refused restore changes nothing"
    );
}

#[test]
fn the_simulation_rng_is_rewound_with_the_physics() {
    use retreat_engine_api::SimulationRng;
    let mut app = test_app();
    drop_boxes(&mut app, 4);
    run_ticks(&mut app, 3);
    let snapshot = capture_physics(app.world_mut()).expect("capture");
    let draw = |app: &mut App| app.world_mut().resource_mut::<SimulationRng>().0.next_u64();
    let first = draw(&mut app);
    let second = draw(&mut app);
    restore_physics(app.world_mut(), &snapshot).expect("restore");
    assert_eq!(
        draw(&mut app),
        first,
        "the RNG replays the same numbers after a restore"
    );
    assert_eq!(draw(&mut app), second);
}

#[test]
fn the_fixed_clock_is_rewound_with_the_physics() {
    let mut app = test_app();
    drop_boxes(&mut app, 4);
    run_ticks(&mut app, 7);
    let snapshot = capture_physics(app.world_mut()).expect("capture");
    let captured = app.world().resource::<Time<Fixed>>().elapsed();
    run_ticks(&mut app, 30);
    assert!(app.world().resource::<Time<Fixed>>().elapsed() > captured);
    restore_physics(app.world_mut(), &snapshot).expect("restore");
    assert_eq!(app.world().resource::<Time<Fixed>>().elapsed(), captured);
    run_ticks(&mut app, 1);
    assert_eq!(
        app.world().resource::<Time<Fixed>>().elapsed(),
        captured + app.world().resource::<Time<Fixed>>().timestep()
    );
}
