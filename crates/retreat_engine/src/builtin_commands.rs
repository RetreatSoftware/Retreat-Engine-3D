//! Commands every Retreat app has: `wait`, `quit` and the `assert.*` checks.

use bevy::prelude::*;
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult};

use crate::entity_names::find_unique_entity_by_name;

/// How far (in world units) `assert.position` lets an entity be from the expected point.
pub const POSITION_TOLERANCE: f32 = 0.001;

pub(crate) fn register_builtin_commands(app: &mut App) {
    app.register_command("wait", "wait <frames>", wait_command)
        .register_command("quit", "quit", quit_command)
        .register_command(
            "assert.entity_exists",
            "assert.entity_exists <name>",
            assert_entity_exists_command,
        )
        .register_command(
            "assert.position",
            "assert.position <name> <x> <y> <z>",
            assert_position_command,
        );
}

/// `wait <frames>`: a script stays idle for `<frames>` frames after this line.
fn wait_command(_world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let frames: u32 = args.parse(0, "frames")?;
    Ok(CommandOutcome::WaitFrames(frames))
}

/// `quit`: exits the app successfully at the end of this frame.
fn quit_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    world.write_message(AppExit::Success);
    Ok(CommandOutcome::Continue)
}

/// `assert.entity_exists <name>`: fails unless exactly one entity has this `Name`.
fn assert_entity_exists_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    find_unique_entity_by_name(world, args, &name)?;
    Ok(CommandOutcome::Continue)
}

/// `assert.position <name> <x> <y> <z>`: fails unless the named entity's translation is
/// within [`POSITION_TOLERANCE`] of the given point.
fn assert_position_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let name: String = args.parse(0, "name")?;
    let expected = Vec3::new(
        args.parse(1, "x")?,
        args.parse(2, "y")?,
        args.parse(3, "z")?,
    );
    let entity = find_unique_entity_by_name(world, args, &name)?;
    let Some(transform) = world.get::<Transform>(entity) else {
        return Err(args.failure(format!(
            "entity `{name}` has no Transform, so it has no position"
        )));
    };
    let actual = transform.translation;
    if actual.distance(expected) <= POSITION_TOLERANCE {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "entity `{name}` is at {actual}, expected {expected} (tolerance {POSITION_TOLERANCE})"
        )))
    }
}
