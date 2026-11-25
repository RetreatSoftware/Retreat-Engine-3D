//! The engine side of `retreat_engine_api/src/analog_input.rs`: mouse movement into
//! `AnalogInput`, axes from held actions, and cursor capture on the primary window.
//!
//! - Mouse movement is collected in `PreUpdate` while the cursor is captured and handed to the next
//!   fixed tick (`update_analog_input`, in `InputActionSystems`, after the held actions).
//! - A client replaying ticks (`NetReplaying`) leaves `AnalogInput` alone, so movement collected
//!   this frame is not swallowed by the replay.
//! - Cursor capture: [`next_cursor_state`] is the rule; `apply_cursor_capture` applies it to the
//!   window. Whether the browser really granted the lock comes from the platform layer.
//!
//! Commands: `input.look <dx> <dy>`, `input.set_axis <axis> <value>`, `input.clear_axis <axis>`,
//! `input.capture_cursor`, `input.release_cursor`, `assert.input_look <dx> <dy>`,
//! `assert.input_axis <axis> <value>`, `assert.cursor_wanted`, `assert.cursor_not_wanted`.

use bevy::input::InputSystems;
use bevy::input::mouse::MouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowFocused};
use retreat_engine_api::{
    AnalogInput, AppCommandExt, CommandArgs, CommandOutcome, CommandResult, CursorCapture,
    InputActions, NetReplaying,
};

use crate::platform;

/// How close a value must be in the assert commands.
const ASSERT_TOLERANCE: f32 = 1.0e-3;

/// Adds analog input and cursor capture (part of `InputActionsPlugin`).
pub(crate) fn add_analog_input(app: &mut App) {
    app.init_resource::<AnalogInput>()
        .init_resource::<CursorCapture>()
        .add_message::<MouseMotion>()
        .add_message::<WindowFocused>()
        .add_systems(
            PreUpdate,
            (apply_cursor_capture, collect_mouse_motion)
                .chain()
                .after(InputSystems),
        )
        .add_systems(Last, apply_cursor_capture)
        .register_command("input.look", "input.look <dx> <dy>", look_command)
        .register_command(
            "input.set_axis",
            "input.set_axis <axis> <value>",
            set_axis_command,
        )
        .register_command(
            "input.clear_axis",
            "input.clear_axis <axis>",
            clear_axis_command,
        )
        .register_command(
            "input.capture_cursor",
            "input.capture_cursor",
            capture_command,
        )
        .register_command(
            "input.release_cursor",
            "input.release_cursor",
            release_command,
        )
        .register_command(
            "assert.input_look",
            "assert.input_look <dx> <dy>",
            assert_look_command,
        )
        .register_command(
            "assert.input_axis",
            "assert.input_axis <axis> <value>",
            assert_axis_command,
        )
        .register_command(
            "assert.cursor_wanted",
            "assert.cursor_wanted",
            assert_wanted_command,
        )
        .register_command(
            "assert.cursor_not_wanted",
            "assert.cursor_not_wanted",
            assert_not_wanted_command,
        );
}

/// Panics when an axis names an action nobody registered.
pub(crate) fn check_axis_actions(world: &World) {
    let actions = world.resource::<InputActions>();
    for axis in world.resource::<AnalogInput>().declared_axes() {
        for action in [axis.negative_action, axis.positive_action] {
            assert!(
                actions.contains(action),
                "input axis `{}` uses action `{action}`, which no plugin registered; register it \
                 (AppInputExt::register_action) or fix the axis",
                axis.id
            );
        }
    }
}

/// Starts the tick's analog input.
pub(crate) fn update_analog_input(
    mut analog: ResMut<AnalogInput>,
    actions: Res<InputActions>,
    replaying: Option<Res<NetReplaying>>,
) {
    if replaying.is_some_and(|replaying| replaying.0) {
        return;
    }
    analog.update(|action| actions.is_held(action));
}

fn collect_mouse_motion(
    mut motion: MessageReader<MouseMotion>,
    cursor: Res<CursorCapture>,
    mut analog: ResMut<AnalogInput>,
) {
    let total: Vec2 = motion.read().map(|motion| motion.delta).sum();
    if cursor.captured && total != Vec2::ZERO {
        analog.add_look(total);
    }
}

/// What to do about the cursor.
#[derive(Debug, PartialEq, Eq)]
struct CursorStep {
    state: CursorCapture,
    /// Ask the platform for the lock again (it needs a click).
    ask_again: bool,
}

/// The cursor rule. `locked` is what the platform reports (`None`: it grants every request),
/// `lost_focus` that the window lost focus, `clicked` that a mouse button went down.
fn next_cursor_state(
    before: CursorCapture,
    has_window: bool,
    locked: Option<bool>,
    lost_focus: bool,
    clicked: bool,
) -> CursorStep {
    if !has_window {
        let state = CursorCapture {
            wanted: before.wanted,
            captured: before.wanted,
        };
        return CursorStep {
            state,
            ask_again: false,
        };
    }
    let lock_taken_away = before.captured && locked == Some(false);
    let wanted = before.wanted && !lost_focus && !lock_taken_away;
    let captured = wanted && locked.unwrap_or(true);
    CursorStep {
        state: CursorCapture { wanted, captured },
        ask_again: wanted && !captured && clicked,
    }
}

/// Applies [`next_cursor_state`] to the primary window.
fn apply_cursor_capture(
    mut cursor: ResMut<CursorCapture>,
    mut windows: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut focus: MessageReader<WindowFocused>,
    mouse: Res<ButtonInput<MouseButton>>,
) {
    let lost_focus = focus.read().any(|message| !message.focused);
    let window = windows.single_mut().ok();
    let step = next_cursor_state(
        *cursor,
        window.is_some(),
        platform::pointer_is_locked(),
        lost_focus,
        mouse.get_just_pressed().next().is_some(),
    );
    cursor.set_if_neq(step.state);
    let Some(mut options) = window else {
        return;
    };
    let grab_mode = if step.state.wanted {
        CursorGrabMode::Locked
    } else {
        CursorGrabMode::None
    };
    if options.grab_mode != grab_mode || step.ask_again {
        options.grab_mode = grab_mode;
        options.visible = !step.state.wanted;
    }
}

fn look_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let delta = Vec2::new(args.parse(0, "dx")?, args.parse(1, "dy")?);
    if !delta.is_finite() {
        return Err(args.failure(format!("the movement must be finite, got {delta}")));
    }
    world.resource_mut::<AnalogInput>().add_look(delta);
    Ok(CommandOutcome::Continue)
}

fn set_axis_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let id: String = args.parse(0, "axis")?;
    let value: f32 = args.parse(1, "value")?;
    world
        .resource_mut::<AnalogInput>()
        .force_axis(&id, Some(value))
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

fn clear_axis_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let id: String = args.parse(0, "axis")?;
    world
        .resource_mut::<AnalogInput>()
        .force_axis(&id, None)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

fn capture_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    world.resource_mut::<CursorCapture>().wanted = true;
    Ok(CommandOutcome::Continue)
}

fn release_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    world.resource_mut::<CursorCapture>().wanted = false;
    Ok(CommandOutcome::Continue)
}

fn assert_look_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let expected = Vec2::new(args.parse(0, "dx")?, args.parse(1, "dy")?);
    let actual = world.resource::<AnalogInput>().look();
    if actual.distance(expected) <= ASSERT_TOLERANCE {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "look of the last tick is {actual}, expected {expected}"
        )))
    }
}

fn assert_axis_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let id: String = args.parse(0, "axis")?;
    let expected: f32 = args.parse(1, "value")?;
    let actual = world.resource::<AnalogInput>().axis(&id);
    if (actual - expected).abs() <= ASSERT_TOLERANCE {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!("axis `{id}` is {actual}, expected {expected}")))
    }
}

fn assert_wanted_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    if world.resource::<CursorCapture>().wanted {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure("cursor capture is not wanted"))
    }
}

fn assert_not_wanted_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    if world.resource::<CursorCapture>().wanted {
        Err(args.failure("cursor capture is wanted"))
    } else {
        Ok(CommandOutcome::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(wanted: bool, captured: bool) -> CursorCapture {
        CursorCapture { wanted, captured }
    }

    #[test]
    fn without_a_window_capture_follows_the_wish() {
        let step = next_cursor_state(state(true, false), false, None, false, false);
        assert_eq!(step.state, state(true, true));
    }

    #[test]
    fn a_browser_lock_is_asked_for_again_on_a_click_until_granted() {
        let waiting = next_cursor_state(state(true, false), true, Some(false), false, false);
        assert_eq!(waiting.state, state(true, false));
        assert!(!waiting.ask_again);
        let click = next_cursor_state(waiting.state, true, Some(false), false, true);
        assert!(click.ask_again);
        let granted = next_cursor_state(click.state, true, Some(true), false, false);
        assert_eq!(granted.state, state(true, true));
    }

    #[test]
    fn a_lost_lock_or_lost_focus_ends_the_wish() {
        let lost = next_cursor_state(state(true, true), true, Some(false), false, false);
        assert_eq!(lost.state, state(false, false));
        let unfocused = next_cursor_state(state(true, true), true, None, true, false);
        assert_eq!(unfocused.state, state(false, false));
    }
}
