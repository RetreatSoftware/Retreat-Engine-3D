//! Material commands in an app without a window or GPU: copy-on-write, whole-subtree
//! targets, and errors. The rendered result is checked by `scripts/verify/material_change.cmds`.

use bevy::prelude::*;
use retreat_engine::{CommandScript, MaterialCommandsPlugin, RetreatCommandsPlugin};

/// Generous: a failed file read is reported within a few updates.
const MAX_UPDATES: u32 = 200;

const START_COLOR: Color = Color::srgb(0.2, 0.2, 0.2);

/// Two entities `left` and `right` that share ONE material asset, and a parent `group`
/// (no material) whose child `group_child` has its own material.
fn spawn_test_scene(world: &mut World) {
    let shared = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::from_color(START_COLOR));
    world.spawn((Name::new("left"), MeshMaterial3d(shared.clone())));
    world.spawn((Name::new("right"), MeshMaterial3d(shared)));
    let own = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::from_color(START_COLOR));
    world
        .spawn(Name::new("group"))
        .with_child((Name::new("group_child"), MeshMaterial3d(own)));
    world.spawn(Name::new("no_material"));
}

/// Runs `text` to the end and returns the app (to inspect) and the script's failure, if any.
fn run_script(text: &str) -> (App, Option<String>) {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<StandardMaterial>()
        .init_asset::<Image>()
        .add_plugins(RetreatCommandsPlugin {
            seed: 0,
            command_script: Some(CommandScript::from_text("test.cmds", text)),
        })
        .add_plugins(MaterialCommandsPlugin);
    spawn_test_scene(app.world_mut());
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

/// The material of the entity named `name`: its handle and its base color.
fn material_of(app: &mut App, name: &str) -> (Handle<StandardMaterial>, Color) {
    let world = app.world_mut();
    let mut query = world.query::<(&Name, &MeshMaterial3d<StandardMaterial>)>();
    let handle = query
        .iter(world)
        .find(|(entity_name, _)| entity_name.as_str() == name)
        .map(|(_, material)| material.0.clone())
        .unwrap_or_else(|| panic!("no entity `{name}` with a material"));
    let color = world
        .resource::<Assets<StandardMaterial>>()
        .get(&handle)
        .expect("material asset exists")
        .base_color;
    (handle, color)
}

#[test]
fn set_color_changes_only_the_named_entity_even_if_the_material_was_shared() {
    let (mut app, failure) = run_script("material.set_color left 1 0 0\nquit\n");
    assert_eq!(failure, None);
    let (left_handle, left_color) = material_of(&mut app, "left");
    let (right_handle, right_color) = material_of(&mut app, "right");
    assert_eq!(left_color, Color::srgb(1.0, 0.0, 0.0));
    assert_eq!(
        right_color, START_COLOR,
        "the shared original must not change"
    );
    assert_ne!(left_handle, right_handle, "left must get its own copy");
}

#[test]
fn set_color_reaches_materials_below_the_named_entity() {
    let (mut app, failure) = run_script("material.set_color group 0 1 0\nquit\n");
    assert_eq!(failure, None);
    let (_, child_color) = material_of(&mut app, "group_child");
    assert_eq!(child_color, Color::srgb(0.0, 1.0, 0.0));
}

#[test]
fn set_color_rejects_channels_outside_zero_to_one() {
    let (_, failure) = run_script("material.set_color left 2 0 0\n");
    let failure = failure.expect("2 is not a valid channel");
    assert!(
        failure.contains("<red> must be between 0 and 1 (sRGB), got 2"),
        "{failure}"
    );
}

#[test]
fn an_entity_without_any_material_is_an_error() {
    let (_, failure) = run_script("material.set_color no_material 1 1 1\n");
    let failure = failure.expect("there is nothing to recolor");
    assert!(
        failure.contains("entity `no_material` and its children have no StandardMaterial"),
        "{failure}"
    );
}

#[test]
fn a_texture_that_cannot_load_fails_the_line() {
    let (mut app, failure) = run_script("material.set_texture left textures/missing.png\nquit\n");
    let failure = failure.expect("the missing texture must fail the script");
    assert!(
        failure.starts_with("test.cmds:1: texture `textures/missing.png` could not load"),
        "{failure}"
    );
    // The command still applied the (broken) texture to its own copy only.
    let (_, right_color) = material_of(&mut app, "right");
    assert_eq!(right_color, START_COLOR);
}

#[test]
fn only_png_textures_are_accepted() {
    let (_, failure) = run_script("material.set_texture left textures/photo.jpg\n");
    let failure = failure.expect("jpg is not accepted");
    assert!(failure.contains("is not a PNG texture"), "{failure}");
}
