//! Custom shader materials and the `shader.apply <name> <material>` command.
//!
//! An app registers each custom material type once, under a short name, with
//! [`AppShaderMaterialExt::register_shader_material`]. `shader.apply` then replaces the material
//! of the named entity **and every mesh below it** with a new instance of that material, and
//! holds the script until the material's shader file has loaded.
//!
//! The material's shader file is read from the type itself (`Material::fragment_shader`), so
//! the path is written once, next to the material.

use std::collections::BTreeMap;
use std::hash::Hash;

use bevy::asset::LoadState;
use bevy::prelude::*;
use bevy::shader::ShaderRef;
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandError, CommandOutcome, CommandResult};

use crate::PendingCommandWork;
use crate::entity_names::find_unique_entity_by_name;

/// Registers `shader.apply` and the system that tracks shader loads. Added by
/// `RetreatEnginePlugin`; materials are added with [`AppShaderMaterialExt`].
pub struct ShaderMaterialCommandsPlugin;

impl Plugin for ShaderMaterialCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShaderMaterialRegistry>()
            .init_resource::<PendingShaderLoads>()
            .register_command(
                "shader.apply",
                "shader.apply <name> <material>",
                apply_shader_command,
            )
            .add_systems(Update, report_shader_loads);
    }
}

/// Adds a custom material to the app under a name that `shader.apply` accepts.
pub trait AppShaderMaterialExt {
    /// Adds Bevy's `MaterialPlugin::<M>` (which renders `M`) and registers `make` under
    /// `name`. `make` builds the material each time `shader.apply` uses it. `M` must return a
    /// file path from `Material::fragment_shader`. Panics if `name` is already registered.
    /// The `where` clause repeats the bound Bevy puts on `MaterialPlugin<M>` being a plugin.
    fn register_shader_material<M>(&mut self, name: &'static str, make: fn() -> M) -> &mut Self
    where
        M: Material,
        M::Data: PartialEq + Eq + Hash + Clone;
}

impl AppShaderMaterialExt for App {
    fn register_shader_material<M>(&mut self, name: &'static str, make: fn() -> M) -> &mut Self
    where
        M: Material,
        M::Data: PartialEq + Eq + Hash + Clone,
    {
        self.add_plugins(MaterialPlugin::<M>::default());
        self.world_mut()
            .get_resource_or_init::<ShaderMaterialRegistry>()
            .register(name, make);
        self
    }
}

/// Every material `shader.apply` can use, by name. One entry per material type.
#[derive(Resource, Default)]
pub struct ShaderMaterialRegistry {
    entries: BTreeMap<&'static str, ShaderMaterialEntry>,
}

/// Creates one new material asset and puts it on every target entity.
type InsertNewMaterial = Box<dyn Fn(&mut World, &[Entity]) + Send + Sync>;

/// Removes one material type's component from an entity (no-op if absent).
type RemoveMaterial = Box<dyn Fn(&mut World, Entity) + Send + Sync>;

/// What `shader.apply` needs to know about one material type, with the type erased so that
/// entries of different types fit in one map.
struct ShaderMaterialEntry {
    /// The fragment shader file, relative to the assets folder.
    shader_path: String,
    insert_new_material: InsertNewMaterial,
    remove_material: RemoveMaterial,
}

impl ShaderMaterialRegistry {
    /// Registers material type `M` under `name`; see [`AppShaderMaterialExt`]. The type's
    /// assets (`Assets<M>`) must exist when `shader.apply` runs. Panics on a duplicate name or
    /// when `M` has no fragment shader file.
    pub fn register<M: Material>(&mut self, name: &'static str, make: fn() -> M) {
        let shader_path = match M::fragment_shader() {
            ShaderRef::Path(path) => path.to_string(),
            ShaderRef::Default | ShaderRef::Handle(_) => panic!(
                "shader material `{name}`: Material::fragment_shader must return a file path \
                 (ShaderRef::Path), so shader.apply can wait for the file to load"
            ),
        };
        let entry = ShaderMaterialEntry {
            shader_path,
            insert_new_material: Box::new(move |world, targets| {
                let material = world.resource_mut::<Assets<M>>().add(make());
                for &target in targets {
                    world
                        .entity_mut(target)
                        .insert(MeshMaterial3d(material.clone()));
                }
            }),
            remove_material: Box::new(|world, entity| {
                world.entity_mut(entity).remove::<MeshMaterial3d<M>>();
            }),
        };
        if self.entries.insert(name, entry).is_some() {
            panic!("shader material `{name}` is registered twice");
        }
    }

    /// Registered names, sorted (for error messages).
    pub fn names(&self) -> Vec<&'static str> {
        self.entries.keys().copied().collect()
    }
}

/// `shader.apply <name> <material>`: gives the named entity and every mesh below it a new
/// instance of the registered material `<material>`, replacing their current material (the
/// standard one or another shader material). Holds the script until the shader file loads.
fn apply_shader_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let name: String = args.parse(0, "name")?;
    let material_name: String = args.parse(1, "material")?;
    let targets = find_mesh_targets(world, args, &name)?;

    world.resource_scope(|world, registry: Mut<ShaderMaterialRegistry>| {
        let Some(entry) = registry.entries.get(material_name.as_str()) else {
            return Err(args.failure(format!(
                "unknown shader material `{material_name}`; registered: {}",
                registry.names().join(", ")
            )));
        };
        // An entity draws once per material component, so it must keep exactly one.
        for &target in &targets {
            world
                .entity_mut(target)
                .remove::<MeshMaterial3d<StandardMaterial>>();
            for other in registry.entries.values() {
                (other.remove_material)(world, target);
            }
        }
        (entry.insert_new_material)(world, &targets);

        let shader: Handle<Shader> = world.resource::<AssetServer>().load(&entry.shader_path);
        world.resource_mut::<PendingCommandWork>().start();
        world
            .resource_mut::<PendingShaderLoads>()
            .0
            .push(PendingShader {
                shader,
                path: entry.shader_path.clone(),
            });
        Ok(CommandOutcome::Continue)
    })
}

/// The named entity and all its descendants that have a mesh. Fails if the name is unknown or
/// nothing there has a mesh.
fn find_mesh_targets(
    world: &mut World,
    args: &CommandArgs,
    name: &str,
) -> Result<Vec<Entity>, CommandError> {
    let root = find_unique_entity_by_name(world, args, name)?;
    let mut targets = Vec::new();
    // Depth-first walk with an explicit stack, as in `material.rs`.
    let mut to_visit = vec![root];
    while let Some(entity) = to_visit.pop() {
        if world.get::<Mesh3d>(entity).is_some() {
            targets.push(entity);
        }
        if let Some(children) = world.get::<Children>(entity) {
            to_visit.extend(children.iter().rev());
        }
    }
    if targets.is_empty() {
        return Err(args.failure(format!(
            "entity `{name}` and its children have no mesh to put a shader on"
        )));
    }
    Ok(targets)
}

/// Shader files that `shader.apply` is waiting for.
#[derive(Resource, Default)]
struct PendingShaderLoads(Vec<PendingShader>);

struct PendingShader {
    shader: Handle<Shader>,
    /// For error messages.
    path: String,
}

/// Finishes (or fails) the pending work of each shader once its load state is final.
fn report_shader_loads(
    mut loads: ResMut<PendingShaderLoads>,
    asset_server: Res<AssetServer>,
    mut pending: ResMut<PendingCommandWork>,
) {
    loads
        .0
        .retain(|load| match asset_server.load_state(load.shader.id()) {
            LoadState::Loaded => {
                pending.finish();
                false
            }
            LoadState::Failed(error) => {
                pending.fail(format!(
                    "shader `{}` could not load (paths start in the app's assets folder): {error}",
                    load.path
                ));
                false
            }
            // Still loading: keep waiting.
            _ => true,
        });
}
