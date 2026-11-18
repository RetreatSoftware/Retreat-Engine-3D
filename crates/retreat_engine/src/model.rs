//! The `model.spawn` command: loads a glTF model (`.glb` / `.gltf`) and places it in the world.
//!
//! Loading is asynchronous: the file is read and decoded on background tasks (or fetched
//! over HTTP in the browser). The command registers the load in `PendingCommandWork`, so a
//! script's next line runs only once the model is in the world, or the script fails with
//! the load error.

use std::path::{Component, Path, PathBuf};

use bevy::asset::RecursiveDependencyLoadState;
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult};

use crate::PendingCommandWork;
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

/// Where a model file lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSource {
    /// A path inside the app's `assets` folder, e.g. `models/Fox.glb`. Works everywhere
    /// (in the browser the file is fetched over HTTP).
    AssetFolder(String),
    /// An absolute path anywhere on disk, e.g. `C:/art/tree.glb`. Native only.
    AbsoluteFile(PathBuf),
}

/// Decides where `path` points, or explains why it is not allowed. Pure, so it is tested
/// directly; `has_file_system` is `platform::HAS_FILE_SYSTEM` outside tests.
pub fn resolve_model_source(path: &str, has_file_system: bool) -> Result<ModelSource, String> {
    let file = Path::new(path);
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let has_model_extension = extension
        .as_deref()
        .is_some_and(|extension| MODEL_FILE_EXTENSIONS.contains(&extension));
    if !has_model_extension {
        return Err(format!(
            "`{path}` is not a glTF model; the file name must end in .{}",
            MODEL_FILE_EXTENSIONS.join(" or .")
        ));
    }
    if file.is_absolute() {
        return if has_file_system {
            Ok(ModelSource::AbsoluteFile(file.to_path_buf()))
        } else {
            Err(format!(
                "`{path}` is an absolute path, but the browser has no file system; \
                 put the model in the assets folder and use a relative path"
            ))
        };
    }
    if file.components().any(|part| part == Component::ParentDir) {
        return Err(format!(
            "`{path}` leaves the assets folder with `..`; use a path inside the assets folder, \
             or an absolute path (native only)"
        ));
    }
    // Bevy asset paths always use `/`, also on Windows.
    Ok(ModelSource::AssetFolder(path.replace('\\', "/")))
}

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
    let source =
        resolve_model_source(&path, HAS_FILE_SYSTEM).map_err(|message| args.failure(message))?;

    // A cheap clone (the server is shared behind a pointer) that ends the borrow of `world`.
    let asset_server = world.resource::<AssetServer>().clone();
    let scene: Handle<WorldAsset> = match source {
        ModelSource::AssetFolder(asset_path) => {
            asset_server.load(GltfAssetLabel::Scene(0).from_asset(asset_path))
        }
        // Files outside the assets folder are "unapproved" for Bevy; the engine sets
        // `UnapprovedPathMode::Deny`, so only this explicit override may load them.
        ModelSource::AbsoluteFile(file) => asset_server
            .load_builder()
            .override_unapproved()
            .load(GltfAssetLabel::Scene(0).from_asset(file)),
    };

    world.resource_mut::<PendingCommandWork>().start();
    world
        .spawn((
            Name::new(name),
            WorldAssetRoot(scene),
            Transform::from_translation(translation).with_scale(Vec3::splat(scale)),
            LoadingModel { path },
        ))
        .observe(finish_model_load);
    Ok(CommandOutcome::Continue)
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
    fn relative_paths_point_into_the_asset_folder() {
        assert_eq!(
            resolve_model_source("models/Fox.glb", true),
            Ok(ModelSource::AssetFolder("models/Fox.glb".to_string()))
        );
        assert_eq!(
            resolve_model_source("models\\Tree.GLTF", false),
            Ok(ModelSource::AssetFolder("models/Tree.GLTF".to_string()))
        );
    }

    #[test]
    fn absolute_paths_work_only_with_a_file_system() {
        let absolute = if cfg!(windows) {
            "C:/art/tree.glb"
        } else {
            "/art/tree.glb"
        };
        assert_eq!(
            resolve_model_source(absolute, true),
            Ok(ModelSource::AbsoluteFile(PathBuf::from(absolute)))
        );
        let error = resolve_model_source(absolute, false).unwrap_err();
        assert!(error.contains("browser has no file system"), "{error}");
    }

    #[test]
    fn paths_leaving_the_asset_folder_are_rejected() {
        let error = resolve_model_source("../secrets/x.glb", true).unwrap_err();
        assert!(error.contains("leaves the assets folder"), "{error}");
    }

    #[test]
    fn node_names_are_prefixed_with_the_model_name() {
        assert_eq!(namespaced_node_name("fox", "b_Head_05"), "fox/b_Head_05");
    }

    #[test]
    fn non_gltf_files_are_rejected() {
        for path in ["models/Fox.fbx", "models/Fox", "notes.txt"] {
            let error = resolve_model_source(path, true).unwrap_err();
            assert!(error.contains("is not a glTF model"), "{path}: {error}");
        }
    }
}
