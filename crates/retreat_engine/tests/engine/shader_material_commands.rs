//! `shader.apply` in an app without a window or GPU: which entities get the material, that
//! it replaces the previous one, and errors. The rendered result is checked by
//! `scripts/verify/shader_stripes.cmds`.

use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::{ShaderLoader, ShaderRef};
use retreat_engine::{
    CommandScript, RetreatCommandsPlugin, ShaderMaterialCommandsPlugin, ShaderMaterialRegistry,
};

/// Generous: loading a small file takes a few updates (see `UPDATE_INTERVAL`).
const MAX_UPDATES: u32 = 2000;

/// Real time between updates. The file loads on another thread in real time, and an update
/// of an app with no window or GPU takes microseconds, so without a pause 2000 updates can
/// finish before the file is read.
const UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1);

/// Test assets live in `crates/retreat_engine/tests/assets/`.
const TEST_ASSET_FOLDER: &str = "tests/assets";

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct FirstTestMaterial {}

impl Material for FirstTestMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/test.wesl".into()
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct SecondTestMaterial {}

impl Material for SecondTestMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/test.wesl".into()
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct MissingShaderMaterial {}

impl Material for MissingShaderMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/missing.wesl".into()
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct NoShaderFileMaterial {}

impl Material for NoShaderFileMaterial {}

/// `model` (no mesh) with children `model/body` (mesh) and `model/socket` (no mesh), plus a
/// separate `bystander` mesh. Every mesh starts with a standard material.
fn spawn_test_scene(world: &mut World) {
    let standard = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    world.spawn(Name::new("model")).with_children(|model| {
        model.spawn((
            Name::new("model/body"),
            Mesh3d(Handle::default()),
            MeshMaterial3d(standard.clone()),
        ));
        model.spawn(Name::new("model/socket"));
    });
    world.spawn((
        Name::new("bystander"),
        Mesh3d(Handle::default()),
        MeshMaterial3d(standard),
    ));
    world.spawn(Name::new("empty"));
}

fn test_app(text: &str) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            file_path: TEST_ASSET_FOLDER.to_string(),
            ..default()
        },
    ))
    .init_asset::<StandardMaterial>()
    .init_asset::<Shader>()
    .register_asset_loader(ShaderLoader)
    .init_asset::<FirstTestMaterial>()
    .init_asset::<SecondTestMaterial>()
    .init_asset::<MissingShaderMaterial>()
    .add_plugins(RetreatCommandsPlugin {
        seed: 0,
        command_script: Some(CommandScript::from_text("test.cmds", text)),
    })
    .add_plugins(ShaderMaterialCommandsPlugin);
    // Registered on the registry directly: `register_shader_material` would also add Bevy's
    // MaterialPlugin, which needs the renderer.
    let mut registry = app.world_mut().resource_mut::<ShaderMaterialRegistry>();
    registry.register("first", || FirstTestMaterial {});
    registry.register("second", || SecondTestMaterial {});
    registry.register("missing", || MissingShaderMaterial {});
    spawn_test_scene(app.world_mut());
    app
}

/// Runs `text` to the end; returns the app (to inspect) and the script's failure, if any.
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
        std::thread::sleep(UPDATE_INTERVAL);
    }
    panic!("the script did not finish within {MAX_UPDATES} updates");
}

fn entity_named(app: &mut App, name: &str) -> Entity {
    let world = app.world_mut();
    let mut query = world.query::<(Entity, &Name)>();
    query
        .iter(world)
        .find(|(_, entity_name)| entity_name.as_str() == name)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("no entity named `{name}`"))
}

fn has<C: Component>(app: &mut App, name: &str) -> bool {
    let entity = entity_named(app, name);
    app.world().get::<C>(entity).is_some()
}

#[test]
fn apply_replaces_the_material_of_every_mesh_below_the_named_entity() {
    let (mut app, failure) = run_script("shader.apply model first\nquit\n");
    assert_eq!(failure, None);
    assert!(has::<MeshMaterial3d<FirstTestMaterial>>(
        &mut app,
        "model/body"
    ));
    assert!(
        !has::<MeshMaterial3d<StandardMaterial>>(&mut app, "model/body"),
        "a mesh must keep exactly one material, or it is drawn twice"
    );
    assert!(
        !has::<MeshMaterial3d<FirstTestMaterial>>(&mut app, "model/socket"),
        "entities without a mesh get no material"
    );
    assert!(
        has::<MeshMaterial3d<StandardMaterial>>(&mut app, "bystander"),
        "other entities are untouched"
    );
}

#[test]
fn applying_another_shader_material_replaces_the_previous_one() {
    let (mut app, failure) =
        run_script("shader.apply model first\nshader.apply model second\nquit\n");
    assert_eq!(failure, None);
    assert!(has::<MeshMaterial3d<SecondTestMaterial>>(
        &mut app,
        "model/body"
    ));
    assert!(!has::<MeshMaterial3d<FirstTestMaterial>>(
        &mut app,
        "model/body"
    ));
}

#[test]
fn unknown_material_lists_the_registered_ones() {
    let (_, failure) = run_script("shader.apply model sparkles\nquit\n");
    let failure = failure.expect("an unknown material must fail");
    assert!(
        failure.contains("unknown shader material `sparkles`; registered: first, missing, second"),
        "{failure}"
    );
}

#[test]
fn an_entity_without_any_mesh_is_an_error() {
    let (_, failure) = run_script("shader.apply empty first\nquit\n");
    let failure = failure.expect("no mesh must fail");
    assert!(
        failure.contains("entity `empty` and its children have no mesh"),
        "{failure}"
    );
}

#[test]
fn a_missing_shader_file_fails_the_line() {
    let (_, failure) = run_script("shader.apply model missing\nquit\n");
    let failure = failure.expect("a missing shader file must fail");
    assert!(
        failure.starts_with("test.cmds:1: shader `shaders/missing.wesl` could not load"),
        "{failure}"
    );
}

#[test]
#[should_panic(expected = "shader material `first` is registered twice")]
fn registering_a_name_twice_panics() {
    let mut registry = ShaderMaterialRegistry::default();
    registry.register("first", || FirstTestMaterial {});
    registry.register("first", || SecondTestMaterial {});
}

#[test]
#[should_panic(expected = "must return a file path")]
fn a_material_without_a_shader_file_cannot_be_registered() {
    let mut registry = ShaderMaterialRegistry::default();
    registry.register("plain", || NoShaderFileMaterial {});
}
