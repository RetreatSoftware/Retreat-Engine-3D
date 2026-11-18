//! Material commands: `material.set_color` and `material.set_texture`.
//!
//! Both act on a named entity **and everything below it** that has a `StandardMaterial`, so
//! `fox` recolors a whole model and `fox/fox` only that part. Materials are often shared (every
//! fox loaded from one file uses the same material asset), so a command never edits a material
//! in place: it gives each target its own edited copy ("copy on write"). Only the named entity
//! changes.

use bevy::asset::{LoadState, UntypedAssetId, VisitAssetDependencies};
use bevy::prelude::*;
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandError, CommandOutcome, CommandResult};

use crate::PendingCommandWork;
use crate::asset_paths::{load_asset, resolve_asset_location};
use crate::entity_names::find_unique_entity_by_name;
use crate::platform::HAS_FILE_SYSTEM;

/// File extensions `material.set_texture` accepts. Bevy decodes PNG by default; other formats
/// (JPEG, ...) need extra Bevy features.
pub const TEXTURE_FILE_EXTENSIONS: [&str; 1] = ["png"];

/// Registers the material commands and the system that tracks texture loads.
pub struct MaterialCommandsPlugin;

impl Plugin for MaterialCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingTextureLoads>()
            .init_resource::<PendingTextureReloads>()
            .register_command(
                "material.set_color",
                "material.set_color <name> <red> <green> <blue>",
                set_color_command,
            )
            .register_command(
                "material.set_texture",
                "material.set_texture <name> <path>",
                set_texture_command,
            )
            .register_command(
                "texture.wait_for_reload",
                "texture.wait_for_reload <path>",
                wait_for_reload_command,
            )
            .add_systems(
                Update,
                (
                    report_texture_loads,
                    refresh_materials_using_reloaded_images,
                    finish_texture_reload_waits,
                ),
            );
    }
}

/// `texture.wait_for_reload <path>`: holds the script until the already-loaded texture at
/// `path` is reloaded because its file changed on disk (native hot reload). It lets a check
/// prove hot reload works: change the file, wait for the reload, then take a snapshot.
/// The reload must happen after this line runs; an earlier one is not seen (timeout).
fn wait_for_reload_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let path: String = args.parse(0, "path")?;
    let location = resolve_asset_location(
        &path,
        "PNG texture",
        &TEXTURE_FILE_EXTENSIONS,
        HAS_FILE_SYSTEM,
    )
    .map_err(|message| args.failure(message))?;
    let asset_server = world.resource::<AssetServer>();
    let Some(image) = asset_server.get_handle::<Image>(location.into_asset_path()) else {
        return Err(args.failure(format!(
            "texture `{path}` is not loaded, so it cannot be reloaded; \
             use it first (e.g. material.set_texture)"
        )));
    };
    world.resource_mut::<PendingCommandWork>().start();
    world
        .resource_mut::<PendingTextureReloads>()
        .0
        .push(image.id());
    Ok(CommandOutcome::Continue)
}

/// Textures that `texture.wait_for_reload` is waiting on.
#[derive(Resource, Default)]
struct PendingTextureReloads(Vec<AssetId<Image>>);

/// Finishes each reload wait when its image is reported as modified (reloaded).
fn finish_texture_reload_waits(
    mut image_events: MessageReader<AssetEvent<Image>>,
    mut waits: ResMut<PendingTextureReloads>,
    mut pending: ResMut<PendingCommandWork>,
) {
    for event in image_events.read() {
        if let AssetEvent::Modified { id } = event
            && let Some(index) = waits.0.iter().position(|waiting| waiting == id)
        {
            waits.0.remove(index);
            pending.finish();
        }
    }
}

/// Hot reload fix. When an image file changes on disk, Bevy 0.20 uploads the new image, but a
/// material that already used it keeps drawing the old one, even when the material is marked
/// as changed. A *new* material asset does pick up the new image (both seen in snapshots).
/// So every entity whose material uses a reloaded image gets a fresh copy of that material.
/// Revisit when Bevy refreshes existing materials on image reload by itself.
fn refresh_materials_using_reloaded_images(
    mut image_events: MessageReader<AssetEvent<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut users: Query<&mut MeshMaterial3d<StandardMaterial>>,
) {
    let reloaded_images: Vec<UntypedAssetId> = image_events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } => Some(id.untyped()),
            _ => None,
        })
        .collect();
    if reloaded_images.is_empty() {
        return;
    }
    for mut material in &mut users {
        let Some(current) = materials.get(&material.0) else {
            continue;
        };
        if uses_any_image(current, &reloaded_images) {
            let fresh_copy = current.clone();
            material.0 = materials.add(fresh_copy);
        }
    }
}

/// True if any texture slot of `material` holds one of `images`. Bevy lists every texture
/// slot (base color, normal map, ...) as an asset dependency, so no slot is missed.
fn uses_any_image(material: &StandardMaterial, images: &[UntypedAssetId]) -> bool {
    let mut found = false;
    material.visit_dependencies(&mut |dependency| {
        if images.contains(&dependency) {
            found = true;
        }
    });
    found
}

/// `material.set_color <name> <red> <green> <blue>`: sets the base color (sRGB, each 0 to 1).
/// A texture, if any, is multiplied by this color, so it tints a textured model.
fn set_color_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let name: String = args.parse(0, "name")?;
    let red = parse_color_channel(args, 1, "red")?;
    let green = parse_color_channel(args, 2, "green")?;
    let blue = parse_color_channel(args, 3, "blue")?;
    let color = Color::srgb(red, green, blue);
    let targets = find_material_targets(world, args, &name)?;
    edit_materials_copy_on_write(world, args, &targets, |material| {
        material.base_color = color;
    })?;
    Ok(CommandOutcome::Continue)
}

/// `material.set_texture <name> <path>`: uses a PNG as the base color texture and resets the
/// base color to white, so the texture shows its own colors. The script holds until the image
/// has loaded, and fails if it cannot load.
fn set_texture_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let name: String = args.parse(0, "name")?;
    let path: String = args.parse(1, "path")?;
    let location = resolve_asset_location(
        &path,
        "PNG texture",
        &TEXTURE_FILE_EXTENSIONS,
        HAS_FILE_SYSTEM,
    )
    .map_err(|message| args.failure(message))?;
    let targets = find_material_targets(world, args, &name)?;

    let is_outside_asset_folder = location.is_outside_asset_folder();
    let asset_server = world.resource::<AssetServer>();
    let image: Handle<Image> = load_asset(
        asset_server,
        location.into_asset_path(),
        is_outside_asset_folder,
    );
    edit_materials_copy_on_write(world, args, &targets, |material| {
        material.base_color_texture = Some(image.clone());
        material.base_color = Color::WHITE;
    })?;

    world.resource_mut::<PendingCommandWork>().start();
    world
        .resource_mut::<PendingTextureLoads>()
        .0
        .push(PendingTexture { image, path });
    Ok(CommandOutcome::Continue)
}

/// Reads a color channel and checks it is between 0 and 1.
fn parse_color_channel(args: &CommandArgs, index: usize, name: &str) -> Result<f32, CommandError> {
    let value: f32 = args.parse(index, name)?;
    if (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(args.failure(format!(
            "<{name}> must be between 0 and 1 (sRGB), got {value}"
        )))
    }
}

/// The named entity and all its descendants that have a `StandardMaterial`, in hierarchy
/// order (parent first). Fails if the name is unknown or nothing there has a material.
fn find_material_targets(
    world: &mut World,
    args: &CommandArgs,
    name: &str,
) -> Result<Vec<Entity>, CommandError> {
    let root = find_unique_entity_by_name(world, args, name)?;
    let mut targets = Vec::new();
    // Depth-first walk with an explicit stack: start at the root, then visit children.
    let mut to_visit = vec![root];
    while let Some(entity) = to_visit.pop() {
        if world
            .get::<MeshMaterial3d<StandardMaterial>>(entity)
            .is_some()
        {
            targets.push(entity);
        }
        if let Some(children) = world.get::<Children>(entity) {
            // Reversed, so the first child is popped (visited) first.
            to_visit.extend(children.iter().rev());
        }
    }
    if targets.is_empty() {
        return Err(args.failure(format!(
            "entity `{name}` and its children have no StandardMaterial to change"
        )));
    }
    Ok(targets)
}

/// Gives every target its own copy of its material with `edit` applied. Editing the shared
/// asset instead would also change every other entity that uses it.
fn edit_materials_copy_on_write(
    world: &mut World,
    args: &CommandArgs,
    targets: &[Entity],
    edit: impl Fn(&mut StandardMaterial),
) -> Result<(), CommandError> {
    for &target in targets {
        let current = world
            .get::<MeshMaterial3d<StandardMaterial>>(target)
            .map(|material| material.0.clone())
            .expect("find_material_targets only returns entities with a material");
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        let Some(original) = materials.get(&current) else {
            return Err(args.failure(
                "a target's material has not finished loading; wait for the model before changing it",
            ));
        };
        let mut edited = original.clone();
        edit(&mut edited);
        let copy = materials.add(edited);
        world.entity_mut(target).insert(MeshMaterial3d(copy));
    }
    Ok(())
}

/// Texture loads started by `material.set_texture` that a script is waiting for.
#[derive(Resource, Default)]
struct PendingTextureLoads(Vec<PendingTexture>);

struct PendingTexture {
    image: Handle<Image>,
    /// The path as written in the command, for error messages.
    path: String,
}

/// Finishes (or fails) the pending work of each texture once its load state is final.
fn report_texture_loads(
    mut loads: ResMut<PendingTextureLoads>,
    asset_server: Res<AssetServer>,
    mut pending: ResMut<PendingCommandWork>,
) {
    loads.0.retain(|load| match asset_server.load_state(load.image.id()) {
        LoadState::Loaded => {
            pending.finish();
            false
        }
        LoadState::Failed(error) => {
            pending.fail(format!(
                "texture `{}` could not load (relative paths start in the app's assets folder): {error}",
                load.path
            ));
            false
        }
        // Still loading: keep waiting.
        _ => true,
    });
}
