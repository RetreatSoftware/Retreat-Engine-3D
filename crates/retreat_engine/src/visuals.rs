//! Draws what the contract's `visuals.rs` describes: `EntityColor` and `CharacterVisual`. Render
//! only: nothing here changes the simulation.
//!
//! - A character's visual is a child entity `<name>/visual`: a glTF model (loaded like
//!   `model.spawn`, so a script waits for it) or the body's shape with a nose block. The owner's
//!   own mesh is removed.
//! - A model that fails to load fails the script and is not retried until the path changes.
//! - Colors are applied copy-on-write to every material at or below the entity, also to parts that
//!   appear later (a model that finishes loading).
//!
//! - `HideVisuals` hides an entity and everything below it.
//!
//! Commands: `visual.set_color|set_color_flat <name> <r> <g> <b>`, `visual.hide|show <name>`,
//! `visual.set_character <name> <model|shape> <scale> <offset_y> <yaw>`,
//! `visual.play <name> <clip> <speed>`, `assert.visual_ready <name>`.

use bevy::prelude::*;
use retreat_engine_api::{
    AnimationPlayback, AppCommandExt, CharacterVisual, ClipPlayback, CommandArgs, CommandOutcome,
    CommandResult, EntityColor, EntityColorFlat, HideVisuals, PhysicsBody, PhysicsShape,
    RetiredEntity,
};

use crate::entity_names::find_unique_entity_by_name;
use crate::model::{is_model_loading, spawn_model};
use crate::platform::HAS_FILE_SYSTEM;

/// Color of a plain shape before any `EntityColor`.
const SHAPE_COLOR: Color = Color::srgb(0.75, 0.75, 0.78);
/// Suffix of a visual's name.
pub const VISUAL_NAME_SUFFIX: &str = "/visual";

/// On the owner: what its visual child was built from.
#[derive(Component)]
struct BuiltVisual {
    model: Option<String>,
    child: Entity,
    clips: Vec<ClipPlayback>,
}

/// On a mesh entity: the color applied to it and the material made for that.
#[derive(Component)]
struct AppliedColor {
    color: Color,
    flat: bool,
    material: Handle<StandardMaterial>,
}

pub struct VisualsPlugin;

impl Plugin for VisualsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                build_character_visuals,
                apply_entity_colors,
                apply_hidden_visuals,
            )
                .chain(),
        )
        .register_command(
            "visual.set_color",
            "visual.set_color <name> <red> <green> <blue>",
            set_color_command,
        )
        .register_command(
            "visual.set_color_flat",
            "visual.set_color_flat <name> <red> <green> <blue>",
            set_color_flat_command,
        )
        .register_command("visual.hide", "visual.hide <name>", hide_command)
        .register_command("visual.show", "visual.show <name>", show_command)
        .register_command(
            "visual.set_character",
            "visual.set_character <name> <model|shape> <scale> <offset_y> <yaw>",
            set_character_command,
        )
        .register_command(
            "visual.play",
            "visual.play <name> <clip> <speed>",
            play_command,
        )
        .register_command(
            "assert.visual_ready",
            "assert.visual_ready <name>",
            assert_ready_command,
        );
    }
}

fn visual_transform(visual: &CharacterVisual) -> Transform {
    Transform {
        translation: visual.offset,
        rotation: Quat::from_rotation_y(visual.yaw),
        scale: Vec3::splat(visual.scale),
    }
}

fn build_character_visuals(world: &mut World) {
    let mut query = world.query::<(Entity, &Name, &CharacterVisual)>();
    let owners: Vec<(Entity, String, CharacterVisual)> = query
        .iter(world)
        .map(|(entity, name, visual)| (entity, name.as_str().to_string(), visual.clone()))
        .collect();
    for (owner, name, visual) in owners {
        let built = world
            .get::<BuiltVisual>(owner)
            .map(|built| (built.model.clone(), built.child));
        let child = match built {
            Some((model, child)) if model == visual.model => child,
            old => {
                if let Some((_, child)) = old
                    && world.get_entity(child).is_ok()
                {
                    world.despawn(child);
                }
                // The visual replaces whatever the entity drew itself.
                world
                    .entity_mut(owner)
                    .remove::<(Mesh3d, MeshMaterial3d<StandardMaterial>)>();
                let child = build_child(world, owner, &name, &visual);
                world.entity_mut(owner).insert(BuiltVisual {
                    model: visual.model.clone(),
                    child,
                    clips: Vec::new(),
                });
                child
            }
        };
        // A model that failed to load is gone.
        if world.get_entity(child).is_err() {
            continue;
        }
        let transform = visual_transform(&visual);
        if let Some(mut current) = world.get_mut::<Transform>(child) {
            current.set_if_neq(transform);
        }
        let changed = world
            .get::<BuiltVisual>(owner)
            .is_some_and(|built| built.clips != visual.clips);
        if changed {
            world.entity_mut(child).insert(AnimationPlayback {
                clips: visual.clips.clone(),
            });
            if let Some(mut built) = world.get_mut::<BuiltVisual>(owner) {
                built.clips = visual.clips;
            }
        }
    }
}

/// Spawns the visual child; a bad model path is reported and gives an empty child.
fn build_child(world: &mut World, owner: Entity, name: &str, visual: &CharacterVisual) -> Entity {
    let child_name = format!("{name}{VISUAL_NAME_SUFFIX}");
    let transform = visual_transform(visual);
    let child = match &visual.model {
        Some(path) => match spawn_model(world, &child_name, path, transform, HAS_FILE_SYSTEM) {
            Ok(child) => child,
            Err(message) => {
                error!("the visual of `{name}`: {message}");
                crate::command_script::fail_running_script(
                    world,
                    format!("the visual of `{name}`: {message}"),
                );
                world.spawn((Name::new(child_name), transform)).id()
            }
        },
        None => spawn_shape(world, owner, child_name, transform),
    };
    world.entity_mut(owner).insert_if_new(Visibility::default());
    world.entity_mut(child).insert(ChildOf(owner));
    child
}

/// The body's shape (a capsule without a body), with a nose towards -Z.
fn spawn_shape(world: &mut World, owner: Entity, name: String, transform: Transform) -> Entity {
    let shape = world.get::<PhysicsBody>(owner).map_or(
        PhysicsShape::Capsule {
            half_height: 0.5,
            radius: 0.4,
        },
        |body| body.shape,
    );
    let (mesh, front, height): (Mesh, f32, f32) = match shape {
        PhysicsShape::Cuboid { half_extents } => (
            Cuboid::from_size(half_extents * 2.0).into(),
            half_extents.z,
            half_extents.y,
        ),
        PhysicsShape::Ball { radius } => (Sphere::new(radius).into(), radius, 0.0),
        PhysicsShape::Capsule {
            half_height,
            radius,
        } => (
            Capsule3d::new(radius, half_height * 2.0).into(),
            radius,
            half_height,
        ),
    };
    let nose = Cuboid::new(front * 0.8, front * 0.4, front * 0.5);
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    let (mesh, nose) = (meshes.add(mesh), meshes.add(nose));
    let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
    let material = materials.add(SHAPE_COLOR);
    let nose_material = materials.add(Color::srgb(0.15, 0.15, 0.18));
    let body = world
        .spawn((
            Name::new(name.clone()),
            Mesh3d(mesh),
            MeshMaterial3d(material),
            transform,
        ))
        .id();
    world.spawn((
        Name::new(format!("{name}/nose")),
        Mesh3d(nose),
        MeshMaterial3d(nose_material),
        Transform::from_xyz(0.0, height * 0.6, -front),
        ChildOf(body),
        // The nose keeps its color, so the facing stays visible.
        KeepsColor,
    ));
    body
}

/// A part `EntityColor` leaves alone.
#[derive(Component)]
struct KeepsColor;

fn apply_entity_colors(
    colored: Query<(Entity, &EntityColor, Has<EntityColorFlat>)>,
    children: Query<&Children>,
    mut parts: Query<
        (
            &mut MeshMaterial3d<StandardMaterial>,
            Option<&mut AppliedColor>,
        ),
        Without<KeepsColor>,
    >,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for (owner, color, flat) in &colored {
        for part in std::iter::once(owner).chain(children.iter_descendants(owner)) {
            let Ok((mut material, applied)) = parts.get_mut(part) else {
                continue;
            };
            if applied.as_ref().is_some_and(|applied| {
                applied.color == color.0 && applied.flat == flat && applied.material == material.0
            }) {
                continue;
            }
            let Some(mut copy) = materials.get(&material.0).cloned() else {
                continue;
            };
            copy.base_color = color.0;
            if flat {
                copy.base_color_texture = None;
            }
            material.0 = materials.add(copy);
            let record = AppliedColor {
                color: color.0,
                flat,
                material: material.0.clone(),
            };
            commands.entity(part).insert(record);
        }
    }
}

fn set_color_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let name: String = args.parse(0, "name")?;
    let channel = |index, label| args.parse::<f32>(index, label);
    let (red, green, blue) = (
        channel(1, "red")?,
        channel(2, "green")?,
        channel(3, "blue")?,
    );
    if ![red, green, blue]
        .iter()
        .all(|value| (0.0..=1.0).contains(value))
    {
        return Err(args.failure("colors are between 0 and 1"));
    }
    let entity = find_unique_entity_by_name(world, args, &name)?;
    world
        .entity_mut(entity)
        .insert(EntityColor(Color::srgb(red, green, blue)));
    Ok(CommandOutcome::Continue)
}

fn set_character_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(5)?;
    let name: String = args.parse(0, "name")?;
    let model: String = args.parse(1, "model")?;
    let scale: f32 = args.parse(2, "scale")?;
    let offset_y: f32 = args.parse(3, "offset_y")?;
    let yaw: f32 = args.parse(4, "yaw")?;
    if !(scale.is_finite() && scale > 0.0 && offset_y.is_finite() && yaw.is_finite()) {
        return Err(args.failure("the scale is positive and every number finite"));
    }
    let entity = find_unique_entity_by_name(world, args, &name)?;
    let clips = world
        .get::<CharacterVisual>(entity)
        .map(|visual| visual.clips.clone())
        .unwrap_or_default();
    world.entity_mut(entity).insert(CharacterVisual {
        model: (model != "shape").then_some(model),
        scale,
        offset: Vec3::new(0.0, offset_y, 0.0),
        yaw,
        clips,
    });
    Ok(CommandOutcome::Continue)
}

fn play_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let name: String = args.parse(0, "name")?;
    let clip: String = args.parse(1, "clip")?;
    let speed: f32 = args.parse(2, "speed")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    let Some(mut visual) = world.get_mut::<CharacterVisual>(entity) else {
        return Err(args.failure(format!(
            "`{name}` has no character visual; run `visual.set_character` first"
        )));
    };
    visual.clips = vec![ClipPlayback {
        speed,
        ..ClipPlayback::looping(clip)
    }];
    Ok(CommandOutcome::Continue)
}

/// Hides entities with `HideVisuals` and shows them again when it is removed (a retired entity
/// stays hidden).
fn apply_hidden_visuals(
    mut commands: Commands,
    hidden: Query<(Entity, Option<&Visibility>), With<HideVisuals>>,
    mut shown_again: RemovedComponents<HideVisuals>,
    retired: Query<(), With<RetiredEntity>>,
    alive: Query<()>,
) {
    for entity in shown_again.read() {
        if alive.contains(entity) && !hidden.contains(entity) && !retired.contains(entity) {
            commands.entity(entity).insert(Visibility::Inherited);
        }
    }
    for (entity, visibility) in &hidden {
        if visibility != Some(&Visibility::Hidden) {
            commands.entity(entity).insert(Visibility::Hidden);
        }
    }
}

fn hide_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    world.entity_mut(entity).insert(HideVisuals);
    Ok(CommandOutcome::Continue)
}

fn show_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    world.entity_mut(entity).remove::<HideVisuals>();
    Ok(CommandOutcome::Continue)
}

/// `visual.set_color_flat <name> <r> <g> <b>`: like `visual.set_color`, without the texture.
fn set_color_flat_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    set_color_command(world, args)?;
    let name: String = args.parse(0, "name")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    world.entity_mut(entity).insert(EntityColorFlat);
    Ok(CommandOutcome::Continue)
}

/// `assert.visual_ready <name>`: its visual is built and its model loaded.
fn assert_ready_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    let child = world.get::<BuiltVisual>(entity).map(|built| built.child);
    match child {
        Some(child) if world.get_entity(child).is_ok() && !is_model_loading(world, child) => {
            Ok(CommandOutcome::Continue)
        }
        _ => Err(args.failure(format!("the visual of `{name}` is not ready"))),
    }
}
