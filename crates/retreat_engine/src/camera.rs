//! Cameras, engine side: maps the contract's `GameCamera` onto Bevy's camera components, in this
//! one file. `GameCamera` requires `Camera3d` and `Transform`, so a camera exists (and the
//! headless offscreen redirect finds it) the moment it is spawned; `sync_game_cameras` then keeps
//! its `Transform` and `Projection` equal to the description whenever the description changes.
//!
//! Commands (the camera is found by its unique `Name`): `camera.set_position <name> <x> <y> <z>`,
//! `camera.look_at <name> <x> <y> <z>` (sets the target), `camera.set_fov <name> <degrees>`.
//! Each validates the result with `GameCamera::validate` before changing anything. A module that
//! edits `GameCamera` directly into an invalid state fails the running script from
//! `sync_game_cameras`.

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandError, CommandOutcome, CommandResult, GameCamera,
};

use crate::command_script::fail_running_script;
use crate::entity_names::find_unique_entity_by_name;

/// Adds the camera mapping and the `camera.*` commands.
pub struct GameCameraPlugin;

impl Plugin for GameCameraPlugin {
    fn build(&self, app: &mut App) {
        app.register_command(
            "camera.set_position",
            "camera.set_position <name> <x> <y> <z>",
            set_position_command,
        )
        .register_command(
            "camera.look_at",
            "camera.look_at <name> <x> <y> <z>",
            look_at_command,
        )
        .register_command(
            "camera.set_fov",
            "camera.set_fov <name> <degrees>",
            set_fov_command,
        )
        // Before transform propagation (PostUpdate), so the new pose is drawn this frame.
        .add_systems(Update, sync_game_cameras);
    }
}

/// Copies every changed `GameCamera` into its `Transform` and `Projection`. This is the one
/// place that knows how the contract's camera becomes Bevy's.
fn sync_game_cameras(
    mut commands: Commands,
    mut cameras: Query<(&Name, &GameCamera, &mut Transform, &mut Projection), Changed<GameCamera>>,
) {
    for (name, camera, mut transform, mut projection) in &mut cameras {
        if let Err(message) = camera.validate() {
            let message = format!("game camera `{name}`: {message}");
            error!("{message}");
            commands.queue(move |world: &mut World| fail_running_script(world, message));
            continue;
        }
        *transform =
            Transform::from_translation(camera.position).looking_at(camera.target, camera.up);
        let fov = camera.fov_degrees.to_radians();
        match projection.as_mut() {
            Projection::Perspective(perspective) => perspective.fov = fov,
            other => *other = Projection::Perspective(PerspectiveProjection { fov, ..default() }),
        }
    }
}

/// The `GameCamera` of the entity named by argument 0, as a mutable reference, plus its name.
fn camera_named_by_first_argument<'w>(
    world: &'w mut World,
    args: &CommandArgs,
) -> Result<(String, Mut<'w, GameCamera>), CommandError> {
    let name: String = args.parse(0, "name")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    match world.get_mut::<GameCamera>(entity) {
        Some(camera) => Ok((name, camera)),
        None => Err(args.failure(format!(
            "entity `{name}` is not a game camera (it has no GameCamera component)"
        ))),
    }
}

/// Applies `change` to a copy of the camera, validates the copy, and only then stores it, so a
/// rejected command leaves the camera as it was.
fn change_camera(
    world: &mut World,
    args: &CommandArgs,
    change: impl FnOnce(&mut GameCamera),
) -> CommandResult {
    let (_, mut camera) = camera_named_by_first_argument(world, args)?;
    let mut changed = camera.clone();
    change(&mut changed);
    changed
        .validate()
        .map_err(|message| args.failure(message))?;
    *camera = changed;
    Ok(CommandOutcome::Continue)
}

fn parse_vec3(args: &CommandArgs, first_index: usize) -> Result<Vec3, CommandError> {
    Ok(Vec3::new(
        args.parse(first_index, "x")?,
        args.parse(first_index + 1, "y")?,
        args.parse(first_index + 2, "z")?,
    ))
}

/// `camera.set_position <name> <x> <y> <z>`: moves the camera; it keeps looking at its target.
fn set_position_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let position = parse_vec3(args, 1)?;
    change_camera(world, args, |camera| camera.position = position)
}

/// `camera.look_at <name> <x> <y> <z>`: sets the point the camera looks at.
fn look_at_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let target = parse_vec3(args, 1)?;
    change_camera(world, args, |camera| camera.target = target)
}

/// `camera.set_fov <name> <degrees>`: sets the vertical field of view.
fn set_fov_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let degrees: f32 = args.parse(1, "degrees")?;
    change_camera(world, args, |camera| camera.fov_degrees = degrees)
}
