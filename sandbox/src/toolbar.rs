//! The sandbox toolbar: a row of buttons along the top of the view. Each button runs one
//! command line, through the same command registry that scripts use.
//!
//! One source of truth: [`SANDBOX_TOOLBAR_ENTRIES`] lists every button, and
//! [`SandboxToolbar::hidden`] says which are hidden. Which buttons show, in what order and
//! where, is computed by the pure functions [`visible_toolbar_entries`] and
//! [`compute_sandbox_toolbar_layout`]; the UI is rebuilt from them whenever the hidden set or
//! the view size changes, so hiding a button re-flows the others.
//!
//! Buttons are Bevy 0.20 `ui_widgets::Button`s: a completed click (or Enter/Space when focused)
//! fires an `Activate` event on the button, and an observer runs the entry's command.
//!
//! Commands: `toolbar.hide <id>`, `toolbar.show <id>`, `toolbar.press <id>`. `toolbar.press`
//! fires the same `Activate` event a click ends with, on the next `Update`, so a scripted press
//! runs the same code as a real one. (It cannot fire it from inside its own handler: the
//! button's command would then run inside another command, which the registry forbids.)

use std::collections::BTreeSet;

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Pressed;
use bevy::ui_widgets::{Activate, Button, observe};
use retreat_engine::dispatch_command_line;
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult};

/// One toolbar button: a stable id (for commands and the entity name), the label shown, and
/// the command line it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolbarEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub command_line: &'static str,
}

/// Every toolbar button, in display order.
pub const SANDBOX_TOOLBAR_ENTRIES: &[ToolbarEntry] = &[
    ToolbarEntry {
        id: "drop_box",
        label: "Drop box",
        command_line: "sandbox.drop_box",
    },
    ToolbarEntry {
        id: "reset",
        label: "Reset boxes",
        command_line: "physics.reset",
    },
    ToolbarEntry {
        id: "stripes",
        label: "Stripes",
        command_line: "shader.apply sandbox_cube stripes",
    },
    ToolbarEntry {
        id: "checker",
        label: "Checker floor",
        command_line: "material.set_texture sandbox_ground textures/checker.png",
    },
];

/// Space between the view's edge and the toolbar, in logical pixels.
pub const TOOLBAR_MARGIN: f32 = 12.0;
/// Space between two buttons.
pub const TOOLBAR_GAP: f32 = 8.0;
/// Height of every button.
pub const TOOLBAR_BUTTON_HEIGHT: f32 = 36.0;
/// Buttons are this wide unless the view is too narrow for all of them; then they shrink.
pub const TOOLBAR_MAX_BUTTON_WIDTH: f32 = 150.0;

const BUTTON_COLOR: Color = Color::srgb(0.16, 0.17, 0.20);
const BUTTON_HOVERED_COLOR: Color = Color::srgb(0.25, 0.27, 0.32);
const BUTTON_PRESSED_COLOR: Color = Color::srgb(0.35, 0.45, 0.65);
const BUTTON_TEXT_SIZE: f32 = 16.0;

/// Where one button goes, in logical pixels from the view's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolbarButtonRect {
    pub id: &'static str,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// The entries that are not hidden, in registry order.
pub fn visible_toolbar_entries(
    entries: &[ToolbarEntry],
    hidden: &BTreeSet<&'static str>,
) -> Vec<ToolbarEntry> {
    entries
        .iter()
        .filter(|entry| !hidden.contains(entry.id))
        .copied()
        .collect()
}

/// Lays the given buttons out in one row along the top: equal widths, at most
/// [`TOOLBAR_MAX_BUTTON_WIDTH`], shrunk so that all of them fit in `viewport_width`. Positions
/// come only from the buttons passed in, so a hidden button leaves no gap.
pub fn compute_sandbox_toolbar_layout(
    visible: &[ToolbarEntry],
    viewport_width: f32,
) -> Vec<ToolbarButtonRect> {
    if visible.is_empty() {
        return Vec::new();
    }
    let count = visible.len() as f32;
    let space_for_buttons = viewport_width - 2.0 * TOOLBAR_MARGIN - TOOLBAR_GAP * (count - 1.0);
    let width = (space_for_buttons / count).clamp(0.0, TOOLBAR_MAX_BUTTON_WIDTH);
    visible
        .iter()
        .enumerate()
        .map(|(index, entry)| ToolbarButtonRect {
            id: entry.id,
            left: TOOLBAR_MARGIN + index as f32 * (width + TOOLBAR_GAP),
            top: TOOLBAR_MARGIN,
            width,
            height: TOOLBAR_BUTTON_HEIGHT,
        })
        .collect()
}

/// Which toolbar entries are hidden. Changing it rebuilds the toolbar.
#[derive(Resource, Default, Debug)]
pub struct SandboxToolbar {
    pub hidden: BTreeSet<&'static str>,
}

/// The toolbar's root UI node (rebuilt as a whole).
#[derive(Component)]
struct ToolbarRoot;

/// A toolbar button and the entry it runs.
#[derive(Component)]
struct ToolbarButton {
    entry: ToolbarEntry,
}

/// Entity name of the button for entry `id`.
fn toolbar_button_name(id: &str) -> String {
    format!("toolbar/{id}")
}

pub struct SandboxToolbarPlugin;

impl Plugin for SandboxToolbarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SandboxToolbar>()
            .register_command("toolbar.hide", "toolbar.hide <id>", hide_command)
            .register_command("toolbar.show", "toolbar.show <id>", show_command)
            .register_command("toolbar.press", "toolbar.press <id>", press_command)
            .init_resource::<PendingToolbarPresses>()
            .add_systems(
                Update,
                (
                    rebuild_toolbar_when_needed,
                    activate_pending_toolbar_presses,
                    color_toolbar_buttons,
                )
                    .chain(),
            );
    }
}

/// Rebuilds the toolbar when the hidden set changes or the view is resized (or on the first
/// frame the camera has a size).
fn rebuild_toolbar_when_needed(
    mut commands: Commands,
    toolbar: Res<SandboxToolbar>,
    cameras: Query<&Camera, With<Camera3d>>,
    roots: Query<Entity, With<ToolbarRoot>>,
    mut last_width: Local<Option<f32>>,
) {
    let Some(viewport) = cameras.iter().find_map(Camera::logical_viewport_size) else {
        return;
    };
    let width_changed = *last_width != Some(viewport.x);
    if !toolbar.is_changed() && !width_changed {
        return;
    }
    *last_width = Some(viewport.x);
    for root in &roots {
        commands.entity(root).despawn();
    }
    let visible = visible_toolbar_entries(SANDBOX_TOOLBAR_ENTRIES, &toolbar.hidden);
    let layout = compute_sandbox_toolbar_layout(&visible, viewport.x);
    commands
        .spawn((
            Name::new("toolbar"),
            ToolbarRoot,
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .with_children(|root| {
            for (entry, rect) in visible.iter().zip(&layout) {
                root.spawn((
                    Name::new(toolbar_button_name(entry.id)),
                    ToolbarButton { entry: *entry },
                    Button,
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(rect.left),
                        top: px(rect.top),
                        width: px(rect.width),
                        height: px(rect.height),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(BUTTON_COLOR),
                    Hovered::default(),
                    observe(run_activated_toolbar_button),
                ))
                .with_child((
                    Text::new(entry.label),
                    TextFont::from_font_size(BUTTON_TEXT_SIZE),
                    TextColor(Color::WHITE),
                ));
            }
        });
}

/// Buttons that `toolbar.press` asked to activate, handled on the next `Update`.
#[derive(Resource, Default)]
struct PendingToolbarPresses(Vec<Entity>);

/// Fires `Activate` on each button `toolbar.press` asked for: the event a click ends with.
fn activate_pending_toolbar_presses(
    mut pending: ResMut<PendingToolbarPresses>,
    mut commands: Commands,
) {
    for button in pending.0.drain(..) {
        commands.trigger(Activate { entity: button });
    }
}

/// Observer on every toolbar button: runs the entry's command when the button is activated.
/// The command runs as a queued world command, after the current system, never inside another
/// command's handler.
fn run_activated_toolbar_button(
    activate: On<Activate>,
    buttons: Query<&ToolbarButton>,
    mut commands: Commands,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    let entry = button.entry;
    commands.queue(move |world: &mut World| {
        let source = format!("toolbar button `{}`", entry.id);
        dispatch_command_line(world, &source, entry.command_line);
    });
}

/// Hover and press feedback (four buttons: cheap enough to set every frame).
fn color_toolbar_buttons(
    mut buttons: Query<(Has<Pressed>, &Hovered, &mut BackgroundColor), With<ToolbarButton>>,
) {
    for (is_pressed, hovered, mut color) in &mut buttons {
        color.0 = if is_pressed {
            BUTTON_PRESSED_COLOR
        } else if hovered.get() {
            BUTTON_HOVERED_COLOR
        } else {
            BUTTON_COLOR
        };
    }
}

/// The registered entry with this id, or an error listing the valid ids.
fn find_entry(
    args: &CommandArgs,
    id: &str,
) -> Result<ToolbarEntry, retreat_engine_api::CommandError> {
    SANDBOX_TOOLBAR_ENTRIES
        .iter()
        .find(|entry| entry.id == id)
        .copied()
        .ok_or_else(|| {
            let ids: Vec<&str> = SANDBOX_TOOLBAR_ENTRIES
                .iter()
                .map(|entry| entry.id)
                .collect();
            args.failure(format!(
                "no toolbar entry `{id}`; entries: {}",
                ids.join(", ")
            ))
        })
}

/// `toolbar.hide <id>`: hides a button; the others move up to fill its place.
fn hide_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let id: String = args.parse(0, "id")?;
    let entry = find_entry(args, &id)?;
    world
        .resource_mut::<SandboxToolbar>()
        .hidden
        .insert(entry.id);
    Ok(CommandOutcome::Continue)
}

/// `toolbar.show <id>`: shows a hidden button again, in its registry position.
fn show_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let id: String = args.parse(0, "id")?;
    let entry = find_entry(args, &id)?;
    world
        .resource_mut::<SandboxToolbar>()
        .hidden
        .remove(entry.id);
    Ok(CommandOutcome::Continue)
}

/// `toolbar.press <id>`: activates a visible button as a click would. The button's command runs
/// on this frame's `Update`, so later lines see its effect.
fn press_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let id: String = args.parse(0, "id")?;
    let entry = find_entry(args, &id)?;
    let button_name = toolbar_button_name(entry.id);
    let mut buttons = world.query_filtered::<(Entity, &Name), With<ToolbarButton>>();
    let Some(button) = buttons
        .iter(world)
        .find(|(_, name)| name.as_str() == button_name)
        .map(|(entity, _)| entity)
    else {
        return Err(args.failure(format!(
            "toolbar button `{id}` is not shown (hidden, or the toolbar is not built yet)"
        )));
    };
    world.resource_mut::<PendingToolbarPresses>().0.push(button);
    Ok(CommandOutcome::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE_VIEW: f32 = 1280.0;

    fn hidden(ids: &[&'static str]) -> BTreeSet<&'static str> {
        ids.iter().copied().collect()
    }

    #[test]
    fn all_entries_show_in_registry_order_at_full_width() {
        let visible = visible_toolbar_entries(SANDBOX_TOOLBAR_ENTRIES, &hidden(&[]));
        let layout = compute_sandbox_toolbar_layout(&visible, WIDE_VIEW);
        let ids: Vec<&str> = layout.iter().map(|rect| rect.id).collect();
        assert_eq!(ids, ["drop_box", "reset", "stripes", "checker"]);
        assert!(
            layout
                .iter()
                .all(|rect| rect.width == TOOLBAR_MAX_BUTTON_WIDTH)
        );
        assert_eq!(layout[0].left, TOOLBAR_MARGIN);
    }

    #[test]
    fn hiding_an_entry_reflows_the_others_without_a_gap() {
        let visible = visible_toolbar_entries(SANDBOX_TOOLBAR_ENTRIES, &hidden(&["reset"]));
        let layout = compute_sandbox_toolbar_layout(&visible, WIDE_VIEW);
        let ids: Vec<&str> = layout.iter().map(|rect| rect.id).collect();
        assert_eq!(ids, ["drop_box", "stripes", "checker"]);
        for pair in layout.windows(2) {
            assert_eq!(
                pair[1].left,
                pair[0].left + pair[0].width + TOOLBAR_GAP,
                "buttons must sit next to each other: {pair:?}"
            );
        }
    }

    #[test]
    fn buttons_shrink_to_fit_a_narrow_view() {
        let narrow = 400.0;
        let visible = visible_toolbar_entries(SANDBOX_TOOLBAR_ENTRIES, &hidden(&[]));
        let layout = compute_sandbox_toolbar_layout(&visible, narrow);
        let last = layout.last().expect("four buttons");
        assert!(layout[0].width < TOOLBAR_MAX_BUTTON_WIDTH);
        assert_eq!(last.left + last.width + TOOLBAR_MARGIN, narrow);
    }

    #[test]
    fn hiding_every_entry_gives_an_empty_layout() {
        let all: Vec<&'static str> = SANDBOX_TOOLBAR_ENTRIES
            .iter()
            .map(|entry| entry.id)
            .collect();
        let visible = visible_toolbar_entries(SANDBOX_TOOLBAR_ENTRIES, &hidden(&all));
        assert!(compute_sandbox_toolbar_layout(&visible, WIDE_VIEW).is_empty());
    }

    #[test]
    fn entry_ids_are_unique() {
        let ids: BTreeSet<&str> = SANDBOX_TOOLBAR_ENTRIES
            .iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(ids.len(), SANDBOX_TOOLBAR_ENTRIES.len());
    }
}
