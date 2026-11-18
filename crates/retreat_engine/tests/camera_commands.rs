//! Game cameras in an app without a window or GPU: the engine builds the real camera from a
//! `GameCamera`, commands move it, and mistakes give clear errors. The rendered result is checked
//! by `scripts/verify/camera.cmds`.

use bevy::prelude::*;
use retreat_engine::{CommandScript, GameCameraPlugin, RetreatCommandsPlugin};
use retreat_engine_api::GameCamera;

const MAX_UPDATES: u32 = 200;

fn test_app(text: &str) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RetreatCommandsPlugin {
            seed: 0,
            command_script: Some(CommandScript::from_text("test.cmds", text)),
        })
        .add_plugins(GameCameraPlugin);
    app.world_mut().spawn((
        Name::new("cam"),
        GameCamera::looking_at(Vec3::new(0.0, 2.0, 6.0), Vec3::ZERO),
    ));
    app.finish();
    app.cleanup();
    app
}

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

fn camera_parts(app: &mut App) -> (Transform, f32) {
    let world = app.world_mut();
    let mut query = world.query::<(&Transform, &Projection, &GameCamera)>();
    let (transform, projection, _) = query.single(world).expect("one game camera");
    let Projection::Perspective(perspective) = projection else {
        panic!("a game camera has a perspective projection");
    };
    (*transform, perspective.fov)
}

#[test]
fn spawning_a_game_camera_creates_a_real_camera_looking_at_the_target() {
    let (mut app, failure) = run_script("wait 1\nquit\n");
    assert_eq!(failure, None);
    let world = app.world_mut();
    assert_eq!(
        world.query::<&Camera3d>().iter(world).count(),
        1,
        "GameCamera must require Camera3d"
    );
    let (transform, fov) = camera_parts(&mut app);
    assert_eq!(transform.translation, Vec3::new(0.0, 2.0, 6.0));
    let forward = transform.forward().as_vec3();
    let expected = (Vec3::ZERO - Vec3::new(0.0, 2.0, 6.0)).normalize();
    assert!(forward.distance(expected) < 1e-5, "{forward} vs {expected}");
    assert!((fov - 45f32.to_radians()).abs() < 1e-6);
}

#[test]
fn commands_move_the_camera_and_change_its_target_and_fov() {
    let (mut app, failure) = run_script(
        "camera.set_position cam 3 2 0\ncamera.look_at cam 0 1 0\ncamera.set_fov cam 90\nwait 1\nquit\n",
    );
    assert_eq!(failure, None);
    let (transform, fov) = camera_parts(&mut app);
    assert_eq!(transform.translation, Vec3::new(3.0, 2.0, 0.0));
    let expected = (Vec3::new(0.0, 1.0, 0.0) - Vec3::new(3.0, 2.0, 0.0)).normalize();
    assert!(transform.forward().as_vec3().distance(expected) < 1e-5);
    assert!((fov - 90f32.to_radians()).abs() < 1e-6);
}

#[test]
fn invalid_changes_fail_with_a_clear_message_and_leave_the_camera_alone() {
    let (mut app, failure) = run_script("camera.look_at cam 0 2 6\nquit\n");
    let failure = failure.expect("target equal to position must fail");
    assert!(failure.contains("`camera.look_at`"), "{failure}");
    assert!(failure.contains("the same point"), "{failure}");
    let (transform, _) = camera_parts(&mut app);
    assert_eq!(transform.translation, Vec3::new(0.0, 2.0, 6.0));

    let (_, failure) = run_script("camera.set_fov cam 200\nquit\n");
    assert!(
        failure
            .expect("fov 200 must fail")
            .contains("fov must be from 1 to 179")
    );

    let (_, failure) = run_script("camera.set_position nobody 0 0 0\nquit\n");
    assert!(
        failure
            .expect("unknown name must fail")
            .contains("no entity is named `nobody`")
    );

    let (_, failure) = run_script("camera.set_position cam 0 0\nquit\n");
    assert!(
        failure
            .expect("missing argument must fail")
            .contains("usage: camera.set_position")
    );
}

#[test]
fn a_named_entity_without_a_game_camera_is_rejected() {
    let mut app = test_app("camera.set_fov plain 60\nquit\n");
    app.world_mut()
        .spawn((Name::new("plain"), Transform::IDENTITY));
    for _ in 0..10 {
        app.update();
    }
    let failure = app
        .world()
        .resource::<CommandScript>()
        .failure()
        .map(String::from)
        .expect("an entity that is not a camera must be rejected");
    assert!(failure.contains("is not a game camera"), "{failure}");
}

#[test]
fn editing_the_component_into_an_invalid_state_fails_the_script() {
    let mut app = test_app("wait 5\nquit\n");
    app.update();
    app.update();
    let world = app.world_mut();
    let mut query = world.query::<&mut GameCamera>();
    for mut camera in query.iter_mut(world) {
        camera.up = Vec3::ZERO;
    }
    for _ in 0..4 {
        app.update();
    }
    let failure = app
        .world()
        .resource::<CommandScript>()
        .failure()
        .map(String::from)
        .expect("an invalid direct edit must fail the script");
    assert!(failure.contains("game camera `cam`"), "{failure}");
}
