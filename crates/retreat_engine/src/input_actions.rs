//! Input mapping, engine side: connects Bevy's keyboard and mouse state to the actions of
//! `retreat_engine_api::InputActions`. The contract names no input library; this file is the one
//! place that maps `InputKey` onto Bevy's `KeyCode` / `MouseButton` ([`to_bevy_input`]).
//!
//! Flow of one key press:
//! 1. A real key event (or `input.key_down` from a script) sets the key down in Bevy's
//!    `ButtonInput<KeyCode>` / `ButtonInput<MouseButton>`. Scripts use the same resources a real
//!    keyboard does, so a scripted press tests the bindings too.
//! 2. `dispatch_pressed_actions` (`Update`) finds every action bound to a key that went down this
//!    frame (`just_pressed`) and runs its command line through `dispatch_command_line`.
//! 3. `update_held_actions` (`FixedUpdate`, in `InputActionSystems`, after the script line of the
//!    tick) recomputes `InputActions::is_held` from `pressed`, so held state is per fixed tick.
//!
//! A mouse button press while the pointer is over a UI button or text field (`UiClickTarget`)
//! runs no action: the click belongs to the UI.
//!
//! While a text field has the keyboard focus (`UiTextFields::is_typing`), no action runs and none
//! is held: the keys belong to the field.
//!
//! Mouse look, axes and cursor capture are in `analog_input.rs`.
//!
//! Commands: `input.key_down <key>`, `input.key_up <key>`, `input.tap <key>` (down for exactly one
//! tick), `input.bind <action> <key>`, `input.unbind <action> <key>`, `input.list`,
//! `assert.input_held <action>`, `assert.input_not_held <action>`.
//!
//! Startup check: every action's command must be registered once all plugins are built
//! (`Plugin::finish`), otherwise the app panics naming the action and the missing command.

use std::collections::BTreeSet;

use crate::UiClickTarget;
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandRegistry, CommandResult, INPUT_KEY_NAMES,
    InputActionSystems, InputActions, InputKey, UiTextFields,
};

use crate::CommandScriptSystems;
use crate::analog_input::{add_analog_input, check_axis_actions, update_analog_input};
use crate::command_dispatch::dispatch_command_line;

/// What an [`InputKey`] is in Bevy: a keyboard key or a mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BevyInput {
    Key(KeyCode),
    Mouse(MouseButton),
}

/// The one mapping from the contract's keys to Bevy's. No `_ =>` arm: a new `InputKey` does not
/// compile until it is mapped here.
pub(crate) fn to_bevy_input(key: InputKey) -> BevyInput {
    match key {
        InputKey::A => BevyInput::Key(KeyCode::KeyA),
        InputKey::B => BevyInput::Key(KeyCode::KeyB),
        InputKey::C => BevyInput::Key(KeyCode::KeyC),
        InputKey::D => BevyInput::Key(KeyCode::KeyD),
        InputKey::E => BevyInput::Key(KeyCode::KeyE),
        InputKey::F => BevyInput::Key(KeyCode::KeyF),
        InputKey::G => BevyInput::Key(KeyCode::KeyG),
        InputKey::H => BevyInput::Key(KeyCode::KeyH),
        InputKey::I => BevyInput::Key(KeyCode::KeyI),
        InputKey::J => BevyInput::Key(KeyCode::KeyJ),
        InputKey::K => BevyInput::Key(KeyCode::KeyK),
        InputKey::L => BevyInput::Key(KeyCode::KeyL),
        InputKey::M => BevyInput::Key(KeyCode::KeyM),
        InputKey::N => BevyInput::Key(KeyCode::KeyN),
        InputKey::O => BevyInput::Key(KeyCode::KeyO),
        InputKey::P => BevyInput::Key(KeyCode::KeyP),
        InputKey::Q => BevyInput::Key(KeyCode::KeyQ),
        InputKey::R => BevyInput::Key(KeyCode::KeyR),
        InputKey::S => BevyInput::Key(KeyCode::KeyS),
        InputKey::T => BevyInput::Key(KeyCode::KeyT),
        InputKey::U => BevyInput::Key(KeyCode::KeyU),
        InputKey::V => BevyInput::Key(KeyCode::KeyV),
        InputKey::W => BevyInput::Key(KeyCode::KeyW),
        InputKey::X => BevyInput::Key(KeyCode::KeyX),
        InputKey::Y => BevyInput::Key(KeyCode::KeyY),
        InputKey::Z => BevyInput::Key(KeyCode::KeyZ),
        InputKey::Space => BevyInput::Key(KeyCode::Space),
        InputKey::Enter => BevyInput::Key(KeyCode::Enter),
        InputKey::Escape => BevyInput::Key(KeyCode::Escape),
        InputKey::Tab => BevyInput::Key(KeyCode::Tab),
        InputKey::ArrowLeft => BevyInput::Key(KeyCode::ArrowLeft),
        InputKey::ArrowRight => BevyInput::Key(KeyCode::ArrowRight),
        InputKey::ArrowUp => BevyInput::Key(KeyCode::ArrowUp),
        InputKey::ArrowDown => BevyInput::Key(KeyCode::ArrowDown),
        InputKey::ShiftLeft => BevyInput::Key(KeyCode::ShiftLeft),
        InputKey::ControlLeft => BevyInput::Key(KeyCode::ControlLeft),
        InputKey::MouseLeft => BevyInput::Mouse(MouseButton::Left),
        InputKey::MouseRight => BevyInput::Mouse(MouseButton::Right),
        InputKey::MouseMiddle => BevyInput::Mouse(MouseButton::Middle),
    }
}

/// Whether `key` is down right now.
fn is_key_down(
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    key: InputKey,
) -> bool {
    match to_bevy_input(key) {
        BevyInput::Key(code) => keys.pressed(code),
        BevyInput::Mouse(button) => mouse.pressed(button),
    }
}

/// Whether `key` went down since the last frame's start.
fn was_key_pressed_this_frame(
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    key: InputKey,
) -> bool {
    match to_bevy_input(key) {
        BevyInput::Key(code) => keys.just_pressed(code),
        BevyInput::Mouse(button) => mouse.just_pressed(button),
    }
}

/// Keys that `input.tap` pressed and `release_tapped_keys` must release after this frame's
/// dispatch.
#[derive(Resource, Default)]
struct PendingKeyReleases(BTreeSet<InputKey>);

/// Adds the input mapping: held state, one-shot dispatch, the `input.*` commands.
pub struct InputActionsPlugin;

impl Plugin for InputActionsPlugin {
    fn build(&self, app: &mut App) {
        // `init_resource` keeps the real ones when `InputPlugin` (DefaultPlugins) added them
        // first, and adds empty ones in tests that run on `MinimalPlugins`.
        app.init_resource::<InputActions>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<PendingKeyReleases>()
            .register_command("input.key_down", "input.key_down <key>", key_down_command)
            .register_command("input.key_up", "input.key_up <key>", key_up_command)
            .register_command("input.tap", "input.tap <key>", tap_command)
            .register_command("input.bind", "input.bind <action> <key>", bind_command)
            .register_command(
                "input.unbind",
                "input.unbind <action> <key>",
                unbind_command,
            )
            .register_command("input.list", "input.list", list_command)
            .register_command(
                "assert.input_held",
                "assert.input_held <action>",
                assert_held_command,
            )
            .register_command(
                "assert.input_not_held",
                "assert.input_not_held <action>",
                assert_not_held_command,
            )
            .add_systems(
                FixedUpdate,
                (update_held_actions, update_analog_input)
                    .chain()
                    .in_set(InputActionSystems)
                    .after(CommandScriptSystems),
            )
            .add_systems(
                Update,
                (dispatch_pressed_actions, release_tapped_keys).chain(),
            );
        add_analog_input(app);
    }

    /// Runs after every plugin's `build`, so an action may name a command that a plugin added
    /// later than its own.
    fn finish(&self, app: &mut App) {
        check_axis_actions(app.world());
        let Some(registry) = app.world().get_resource::<CommandRegistry>() else {
            return;
        };
        let known = registry.ids();
        let unknown = app
            .world()
            .resource::<InputActions>()
            .actions_with_unknown_commands(|id| known.contains(&id));
        if let Some((action, command)) = unknown.first() {
            panic!(
                "input action `{action}` runs command `{command}`, which no plugin registered; \
                 register the command (AppCommandExt::register_command) or fix the action's \
                 command_line. All such actions: {unknown:?}"
            );
        }
    }
}

/// Recomputes `InputActions::is_held` from the keys that are down. Read `pressed`, never
/// `just_pressed`: a fixed tick can run twice or not at all in one frame.
/// While a text field has the keyboard focus nothing is held: typing `w` must not walk.
fn update_held_actions(
    mut actions: ResMut<InputActions>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    fields: Option<Res<UiTextFields>>,
) {
    let typing = fields.is_some_and(|fields| fields.is_typing());
    actions.update_held(|key| !typing && is_key_down(&keys, &mouse, key));
}

/// Runs the command of every action whose bound key went down this frame, once per action, in
/// action-id order.
fn dispatch_pressed_actions(world: &mut World) {
    // Keys pressed while typing in a text field belong to the field (see `ui_panel.rs`).
    if world
        .get_resource::<UiTextFields>()
        .is_some_and(UiTextFields::is_typing)
    {
        return;
    }
    // A mouse press on a UI button or text field belongs to the UI, not to the game (a click on
    // "Log in" must not also capture the cursor or fire).
    let mouse_on_ui = mouse_is_over_ui(world);
    let keys = world.resource::<ButtonInput<KeyCode>>();
    let mouse = world.resource::<ButtonInput<MouseButton>>();
    let actions = world.resource::<InputActions>();
    let mut to_run: Vec<(&'static str, &'static str)> = Vec::new();
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    for (_, key) in INPUT_KEY_NAMES {
        if !was_key_pressed_this_frame(keys, mouse, *key) {
            continue;
        }
        if mouse_on_ui && matches!(to_bevy_input(*key), BevyInput::Mouse(_)) {
            continue;
        }
        for id in actions.actions_for_key(*key) {
            if seen.insert(id)
                && let Some(command_line) = actions.command_line(id)
            {
                to_run.push((id, command_line));
            }
        }
    }
    to_run.sort_unstable();
    for (id, command_line) in to_run {
        dispatch_command_line(world, &format!("input action `{id}`"), command_line);
    }
}

/// True when the mouse pointer is over a UI element that takes clicks (`UiClickTarget`).
fn mouse_is_over_ui(world: &World) -> bool {
    let Some(hits) = world
        .get_resource::<HoverMap>()
        .and_then(|hover| hover.get(&PointerId::Mouse))
    else {
        return false;
    };
    hits.keys()
        .any(|entity| world.get::<UiClickTarget>(*entity).is_some())
}

/// Releases the keys `input.tap` pressed, after `dispatch_pressed_actions` saw them go down.
fn release_tapped_keys(
    mut pending: ResMut<PendingKeyReleases>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
) {
    for key in std::mem::take(&mut pending.0) {
        match to_bevy_input(key) {
            BevyInput::Key(code) => keys.release(code),
            BevyInput::Mouse(button) => mouse.release(button),
        }
    }
}

fn parse_key(
    args: &CommandArgs,
    index: usize,
) -> Result<InputKey, retreat_engine_api::CommandError> {
    let name: String = args.parse(index, "key")?;
    InputKey::parse(&name).map_err(|message| args.failure(message))
}

fn set_key_down(world: &mut World, key: InputKey) {
    match to_bevy_input(key) {
        BevyInput::Key(code) => world.resource_mut::<ButtonInput<KeyCode>>().press(code),
        BevyInput::Mouse(button) => world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(button),
    }
}

fn set_key_up(world: &mut World, key: InputKey) {
    match to_bevy_input(key) {
        BevyInput::Key(code) => world.resource_mut::<ButtonInput<KeyCode>>().release(code),
        BevyInput::Mouse(button) => world
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(button),
    }
}

fn key_is_down(world: &World, key: InputKey) -> bool {
    is_key_down(
        world.resource::<ButtonInput<KeyCode>>(),
        world.resource::<ButtonInput<MouseButton>>(),
        key,
    )
}

/// `input.key_down <key>`: presses a key like a keyboard would. Stays down until `input.key_up`.
fn key_down_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let key = parse_key(args, 0)?;
    set_key_down(world, key);
    Ok(CommandOutcome::Continue)
}

/// `input.key_up <key>`: releases a key.
fn key_up_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let key = parse_key(args, 0)?;
    set_key_up(world, key);
    Ok(CommandOutcome::Continue)
}

/// `input.tap <key>`: key down for exactly one tick: actions bound to it run once and it counts
/// as held for this tick only.
fn tap_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let key = parse_key(args, 0)?;
    if key_is_down(world, key) {
        return Err(args.failure(format!(
            "key `{}` is already down (use `input.key_up {}` first); a tap needs a key that is up",
            key.name(),
            key.name()
        )));
    }
    set_key_down(world, key);
    world.resource_mut::<PendingKeyReleases>().0.insert(key);
    Ok(CommandOutcome::Continue)
}

fn parse_action(
    args: &CommandArgs,
    world: &World,
) -> Result<String, retreat_engine_api::CommandError> {
    let id: String = args.parse(0, "action")?;
    if !world.resource::<InputActions>().contains(&id) {
        let ids = world.resource::<InputActions>().ids();
        return Err(args.failure(format!("no action `{id}`; actions: {}", ids.join(", "))));
    }
    Ok(id)
}

/// `input.bind <action> <key>`: binds one more key to an action.
fn bind_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let id = parse_action(args, world)?;
    let key = parse_key(args, 1)?;
    world
        .resource_mut::<InputActions>()
        .bind(&id, key)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

/// `input.unbind <action> <key>`: removes a binding; fails if it is not bound.
fn unbind_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let id = parse_action(args, world)?;
    let key = parse_key(args, 1)?;
    world
        .resource_mut::<InputActions>()
        .unbind(&id, key)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

/// `input.list`: logs every action with its keys and command.
fn list_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let actions = world.resource::<InputActions>();
    for id in actions.ids() {
        info!(
            "input action `{id}`: keys [{}] -> `{}`",
            actions.key_names(id),
            actions.command_line(id).unwrap_or("(held only)")
        );
    }
    Ok(CommandOutcome::Continue)
}

/// `assert.input_held <action>`: fails unless a key of the action was down at the last tick.
fn assert_held_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let id = parse_action(args, world)?;
    if world.resource::<InputActions>().is_held(&id) {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "action `{id}` is not held (keys: {}); hold one with `input.key_down <key>`",
            world.resource::<InputActions>().key_names(&id)
        )))
    }
}

/// `assert.input_not_held <action>`: fails if a key of the action was down at the last tick.
fn assert_not_held_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let id = parse_action(args, world)?;
    if world.resource::<InputActions>().is_held(&id) {
        Err(args.failure(format!(
            "action `{id}` is held (keys: {})",
            world.resource::<InputActions>().key_names(&id)
        )))
    } else {
        Ok(CommandOutcome::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_contract_key_maps_to_a_distinct_bevy_input() {
        let mut seen = Vec::new();
        for (name, key) in INPUT_KEY_NAMES {
            let mapped = to_bevy_input(*key);
            assert!(
                !seen.contains(&mapped),
                "{name} maps to a duplicate {mapped:?}"
            );
            seen.push(mapped);
        }
    }
}
