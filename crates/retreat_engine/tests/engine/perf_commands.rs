//! Performance commands in an app without a window: scene counts are exact, frame time is
//! measured and asserted tolerantly, and every failure message says what to do.

use bevy::camera::visibility::ViewVisibility;
use bevy::prelude::*;
use retreat_engine::{
    CommandScript, MIN_FRAMES_FOR_ASSERT, PerfCommandsPlugin, RetreatCommandsPlugin, count_scene,
};
use retreat_engine_api::run_command_line;

fn app_with_perf() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RetreatCommandsPlugin::default())
        .add_plugins(PerfCommandsPlugin);
    app
}

/// Spawns `visible` meshes a camera sees and `hidden` meshes nothing sees.
fn spawn_meshes(app: &mut App, visible: usize, hidden: usize) {
    for index in 0..visible + hidden {
        let view_visibility = if index < visible {
            ViewVisibility::VISIBLE
        } else {
            ViewVisibility::HIDDEN
        };
        app.world_mut()
            .spawn((Mesh3d(Handle::default()), view_visibility));
    }
}

#[test]
fn scene_counts_split_visible_from_hidden_meshes() {
    let mut app = app_with_perf();
    let before = count_scene(app.world_mut());
    spawn_meshes(&mut app, 3, 2);
    let after = count_scene(app.world_mut());
    assert_eq!(after.meshes, 5);
    assert_eq!(after.visible_meshes, 3);
    assert_eq!(after.entities, before.entities + 5);
    run_command_line(app.world_mut(), "perf.counts").unwrap();
}

#[test]
fn visible_mesh_and_entity_budgets_are_asserted_exactly() {
    let mut app = app_with_perf();
    spawn_meshes(&mut app, 3, 2);
    let world = app.world_mut();
    run_command_line(world, "assert.visible_meshes_below 4").unwrap();
    let error = run_command_line(world, "assert.visible_meshes_below 3").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("3 visible meshes, expected fewer than 3"),
        "{error}"
    );
    run_command_line(world, "assert.entities_below 1000").unwrap();
    let error = run_command_line(world, "assert.entities_below 1").unwrap_err();
    assert!(
        error.to_string().contains("expected fewer than 1"),
        "{error}"
    );
}

#[test]
fn frame_time_needs_frames_then_checks_the_average() {
    let mut app = app_with_perf();
    let error = run_command_line(app.world_mut(), "assert.frame_time_below 1000").unwrap_err();
    assert!(error.to_string().contains("`wait 60`"), "{error}");
    for _ in 0..MIN_FRAMES_FOR_ASSERT + 2 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let world = app.world_mut();
    run_command_line(world, "perf.frame_time").unwrap();
    // A generous budget passes; an impossible one fails with the measurement in the message.
    run_command_line(world, "assert.frame_time_below 5000").unwrap();
    let error = run_command_line(world, "assert.frame_time_below 0.000001").unwrap_err();
    assert!(error.to_string().contains("average frame time"), "{error}");
}

#[test]
fn perf_commands_work_in_a_script() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RetreatCommandsPlugin {
            seed: 0,
            command_script: Some(CommandScript::from_text(
                "perf.cmds",
                "wait 15\nperf.frame_time\nperf.counts\nassert.frame_time_below 5000\nassert.entities_below 10000\nquit\n",
            )),
        })
        .add_plugins(PerfCommandsPlugin);
    for _ in 0..200 {
        app.update();
        if app.should_exit().is_some() {
            break;
        }
    }
    assert_eq!(app.world().resource::<CommandScript>().failure(), None);
}
