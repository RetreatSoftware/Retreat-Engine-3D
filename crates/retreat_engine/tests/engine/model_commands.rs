//! `model.spawn` error paths in an app without a window or GPU. Loading a real model is
//! checked by `scripts/verify/model_spawn.cmds` (it needs rendering for the snapshot).

use bevy::prelude::*;
use retreat_engine::{CommandScript, ModelCommandsPlugin, RetreatCommandsPlugin};

/// Generous: a failed file read is reported within a few updates.
const MAX_UPDATES: u32 = 200;

fn run_script(text: &str, prepare: impl FnOnce(&mut World)) -> (AppExit, Option<String>) {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<WorldAsset>()
        .init_asset::<bevy::gltf::Gltf>()
        .add_plugins(RetreatCommandsPlugin {
            seed: 0,
            command_script: Some(CommandScript::from_text("test.cmds", text)),
        })
        .add_plugins(ModelCommandsPlugin);
    prepare(app.world_mut());
    for _ in 0..MAX_UPDATES {
        app.update();
        if let Some(exit) = app.should_exit() {
            let failure = app
                .world()
                .resource::<CommandScript>()
                .failure()
                .map(String::from);
            return (exit, failure);
        }
    }
    panic!("the script did not finish within {MAX_UPDATES} updates");
}

#[test]
fn a_missing_model_file_fails_the_line_that_spawned_it() {
    let (exit, failure) = run_script(
        "model.spawn ghost models/missing.glb 0 0 0 1\nquit\n",
        |_| {},
    );
    assert_eq!(exit, AppExit::error());
    let failure = failure.expect("the load failure must be recorded");
    assert!(
        failure.starts_with("test.cmds:1: model `ghost` could not load `models/missing.glb`"),
        "{failure}"
    );
}

#[test]
fn a_name_that_is_already_used_is_rejected() {
    let (exit, failure) = run_script("model.spawn taken models/Fox.glb 0 0 0 1\n", |world| {
        world.spawn(Name::new("taken"));
    });
    assert_eq!(exit, AppExit::error());
    let failure = failure.expect("the duplicate name must fail");
    assert!(
        failure.contains("an entity is already named `taken`"),
        "{failure}"
    );
}

#[test]
fn a_scale_that_is_not_positive_is_rejected() {
    let (exit, failure) = run_script("model.spawn fox models/Fox.glb 0 0 0 -1\n", |_| {});
    assert_eq!(exit, AppExit::error());
    let failure = failure.expect("a negative scale must fail");
    assert!(
        failure.contains("scale must be a positive number, got -1"),
        "{failure}"
    );
}
