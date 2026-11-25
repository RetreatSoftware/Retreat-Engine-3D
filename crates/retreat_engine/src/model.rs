//! The `model.spawn` command: loads a glTF model (`.glb` / `.gltf`) and places it in the world.
//!
//! Loading is asynchronous: the file is read and decoded on background tasks (or fetched
//! over HTTP in the browser). The command registers the load in `PendingCommandWork`, so a
//! script's next line runs only once the model is in the world, or the script fails with
//! the load error.

use bevy::asset::RecursiveDependencyLoadState;
use bevy::gltf::{Gltf, GltfAssetLabel};
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult};

use crate::PendingCommandWork;
use crate::asset_paths::{load_asset, resolve_asset_location};
use crate::entity_names::ensure_name_is_free;
use crate::platform::HAS_FILE_SYSTEM;

/// File extensions `model.spawn` accepts (glTF binary and glTF JSON).
pub const MODEL_FILE_EXTENSIONS: [&str; 2] = ["glb", "gltf"];

/// Registers `model.spawn` and the system that reports failed loads. Needs an `AssetServer`
/// and the glTF loader (both in Bevy's `DefaultPlugins`).
pub struct ModelCommandsPlugin;

impl Plugin for ModelCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.register_command(
            "model.spawn",
            "model.spawn <name> <path> <x> <y> <z> <scale>",
            model_spawn_command,
        )
        .add_systems(Update, report_failed_model_loads);
    }
}

/// The glTF file a model was spawned from (the whole file, not only scene 0). Animation reads the
/// file's clips through it. Stays on the model for its whole life.
#[derive(Component, Debug)]
pub(crate) struct ModelGltf(pub Handle<Gltf>);

/// Marks a model entity whose file is still loading. Removed once the model is in the world.
#[derive(Component, Debug)]
struct LoadingModel {
    /// The path as written in the command, for error messages.
    path: String,
}

/// `model.spawn <name> <path> <x> <y> <z> <scale>`: loads the first scene of a glTF file and
/// places it at (x, y, z) with a uniform scale (glTF files are often in centimeters: use 0.01).
fn model_spawn_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(6)?;
    let name: String = args.parse(0, "name")?;
    let path: String = args.parse(1, "path")?;
    let translation = Vec3::new(
        args.parse(2, "x")?,
        args.parse(3, "y")?,
        args.parse(4, "z")?,
    );
    let scale: f32 = args.parse(5, "scale")?;
    if !(scale.is_finite() && scale > 0.0) {
        return Err(args.failure(format!("scale must be a positive number, got {scale}")));
    }
    ensure_name_is_free(world, args, &name)?;
    let transform = Transform::from_translation(translation).with_scale(Vec3::splat(scale));
    spawn_model(world, &name, &path, transform, HAS_FILE_SYSTEM)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

/// True while the model's file is still loading.
pub(crate) fn is_model_loading(world: &World, entity: Entity) -> bool {
    world.get::<LoadingModel>(entity).is_some()
}

/// Spawns a model entity and starts loading its file (pending work until it is in the world).
/// The caller checks that `name` is free.
pub(crate) fn spawn_model(
    world: &mut World,
    name: &str,
    path: &str,
    transform: Transform,
    has_file_system: bool,
) -> Result<Entity, String> {
    let location =
        resolve_asset_location(path, "glTF model", &MODEL_FILE_EXTENSIONS, has_file_system)?;
    let is_outside_asset_folder = location.is_outside_asset_folder();
    let whole_file = location.clone().into_asset_path();
    // `#Scene0`: only the file's first scene, not the whole file.
    let scene_path = GltfAssetLabel::Scene(0).from_asset(location.into_asset_path());
    let asset_server = world.resource::<AssetServer>();
    let scene: Handle<WorldAsset> = load_asset(asset_server, scene_path, is_outside_asset_folder);
    let gltf: Handle<Gltf> = load_asset(asset_server, whole_file, is_outside_asset_folder);

    world.resource_mut::<PendingCommandWork>().start();
    let path = path.to_string();
    Ok(world
        .spawn((
            Name::new(name.to_string()),
            WorldAssetRoot(scene),
            ModelGltf(gltf),
            transform,
            LoadingModel { path },
        ))
        .observe(finish_model_load)
        .id())
}

/// Separates a model's name from the name of a node inside it: `fox/b_Head_05`.
pub const MODEL_NODE_NAME_SEPARATOR: char = '/';

/// The name a glTF node gets inside a model. glTF files name their nodes freely (Fox.glb
/// has a node called `fox`, and two foxes would both have a `root`), so every node name is
/// prefixed with its model's name. Names stay unique, and commands can still address one
/// part of a model.
pub fn namespaced_node_name(model_name: &str, node_name: &str) -> String {
    format!("{model_name}{MODEL_NODE_NAME_SEPARATOR}{node_name}")
}

/// Runs when Bevy has spawned the model's entities under the model entity: prefixes their
/// names (see [`namespaced_node_name`]) and finishes the pending load.
fn finish_model_load(
    ready: On<WorldInstanceReady>,
    loading: Query<&Name, With<LoadingModel>>,
    children: Query<&Children>,
    mut node_names: Query<&mut Name, Without<LoadingModel>>,
    mut commands: Commands,
    mut pending: ResMut<PendingCommandWork>,
) {
    // A model can be re-spawned later (e.g. hot reload); only the first load was pending.
    let Ok(model_name) = loading.get(ready.entity) else {
        return;
    };
    for node in children.iter_descendants(ready.entity) {
        if let Ok(mut node_name) = node_names.get_mut(node) {
            let renamed = namespaced_node_name(model_name.as_str(), node_name.as_str());
            *node_name = Name::new(renamed);
        }
    }
    commands.entity(ready.entity).remove::<LoadingModel>();
    pending.finish();
}

/// Fails the pending load of any model whose file (or a texture it uses) could not load,
/// and removes the empty model entity.
fn report_failed_model_loads(
    loading_models: Query<(Entity, &Name, &WorldAssetRoot, &LoadingModel)>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
    mut pending: ResMut<PendingCommandWork>,
) {
    for (entity, name, root, loading) in &loading_models {
        let state = asset_server.recursive_dependency_load_state(root.0.id());
        if let RecursiveDependencyLoadState::Failed(error) = state {
            pending.fail(format!(
                "model `{name}` could not load `{}` (relative paths start in the app's assets folder): {error}",
                loading.path
            ));
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_names_are_prefixed_with_the_model_name() {
        assert_eq!(namespaced_node_name("fox", "b_Head_05"), "fox/b_Head_05");
    }
}
