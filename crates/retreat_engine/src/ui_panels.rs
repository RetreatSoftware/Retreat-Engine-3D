//! UI toolkit, engine side: draws the contract's `UiPanels` with `bevy_ui` (Bevy 0.20 widgets) and
//! runs a button's command line when it is activated. Text fields are made in `ui_text_fields.rs`. This is the one place that turns panel
//! descriptions into UI nodes; where each button goes is decided by the contract's pure
//! `compute_panel_layout`, stacked per corner by `UiPanels::layouts`.
//!
//! The UI is rebuilt as a whole when the `UiPanels` resource changes (show, hide a panel or an
//! entry) or when the view size changes, so hiding anything re-flows what is left.
//!
//! Buttons are `bevy::ui_widgets::Button`s: a completed click (or Enter/Space when focused) fires
//! an `Activate` event, and one app-wide observer runs the entry's command through
//! `dispatch_command_line` (per-button observers were lost on a rebuild in the browser, found by
//! the crawler's network check, 2026-11-27).
//! `ui.press` fires the same `Activate` on the next `Update`, so a scripted press runs the same code
//! as a real one. (It cannot fire it from inside its own handler: the button's command would then
//! run inside another command, which the registry forbids.)
//!
//! Commands: `ui.show_panel <panel>`, `ui.hide_panel <panel>`, `ui.hide_entry <panel> <entry>`,
//! `ui.show_entry <panel> <entry>`, `ui.press <panel> <entry>`, `ui.list`.
//!
//! Startup check (`Plugin::finish`): every entry's command must be registered; otherwise the app
//! panics naming the panel entry and the missing command.

use bevy::input_focus::{FocusCause, InputFocus};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Pressed;
use bevy::ui_widgets::{Activate, Button};
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandRegistry, CommandResult, PanelRect,
    UiButtonEntry, UiPanels, UiTextFields,
};

use crate::command_dispatch::dispatch_command_line;
use crate::ui_text_fields::{PanelTextField, UiTextFieldsPlugin, spawn_text_field, widget_text};
use bevy::ecs::system::SystemParam;
use bevy::text::EditableText;

const BUTTON_COLOR: Color = Color::srgb(0.16, 0.17, 0.20);
const BUTTON_HOVERED_COLOR: Color = Color::srgb(0.25, 0.27, 0.32);
const BUTTON_PRESSED_COLOR: Color = Color::srgb(0.35, 0.45, 0.65);
const BUTTON_TEXT_SIZE: f32 = 16.0;
const TITLE_TEXT_SIZE: f32 = 20.0;

/// Adds panel drawing and the `ui.*` commands.
pub struct UiPanelsPlugin;

impl Plugin for UiPanelsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(UiTextFieldsPlugin);
        app.init_resource::<UiPanels>()
            .init_resource::<PendingPresses>()
            .register_command("ui.show_panel", "ui.show_panel <panel>", show_panel_command)
            .register_command("ui.hide_panel", "ui.hide_panel <panel>", hide_panel_command)
            .register_command(
                "ui.hide_entry",
                "ui.hide_entry <panel> <entry>",
                hide_entry_command,
            )
            .register_command(
                "ui.show_entry",
                "ui.show_entry <panel> <entry>",
                show_entry_command,
            )
            .register_command("ui.press", "ui.press <panel> <entry>", press_command)
            .register_command("ui.list", "ui.list", list_command)
            // One app-wide observer instead of one per button: buttons are rebuilt with the
            // panels, and an entity observer did not survive every rebuild in the browser.
            .add_observer(run_activated_button)
            .add_systems(
                Update,
                (
                    rebuild_panels_when_needed,
                    activate_pending_presses,
                    color_buttons,
                )
                    .chain(),
            );
    }

    /// Runs after every plugin's `build`, so an entry may run a command registered later.
    fn finish(&self, app: &mut App) {
        let Some(registry) = app.world().get_resource::<CommandRegistry>() else {
            return;
        };
        let known = registry.ids();
        let unknown = app
            .world()
            .resource::<UiPanels>()
            .entries_with_unknown_commands(|id| known.contains(&id));
        if let Some((entry, command)) = unknown.first() {
            panic!(
                "UI panel entry `{entry}` runs command `{command}`, which no plugin registered; \
                 register the command (AppCommandExt::register_command) or fix the entry's \
                 command_line. All such entries: {unknown:?}"
            );
        }
    }
}

/// A UI element that takes mouse clicks (panel buttons and text fields): a mouse press on it runs no
/// input action (`input_actions.rs`), so a click on a button does not also capture the cursor.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct UiClickTarget;

/// The root node of one panel (rebuilt as a whole).
#[derive(Component)]
struct PanelRoot;

/// A panel button and the entry it runs.
#[derive(Component)]
struct PanelButton {
    panel: &'static str,
    entry: UiButtonEntry,
}

/// Entity name of a panel's root.
fn panel_root_name(panel: &str) -> String {
    format!("ui/{panel}")
}

/// Entity name of a panel's button.
fn panel_button_name(panel: &str, entry: &str) -> String {
    format!("ui/{panel}/{entry}")
}

/// An absolutely positioned node at `rect`.
fn node_at(rect: PanelRect) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(rect.left),
        top: px(rect.top),
        width: px(rect.width),
        height: px(rect.height),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    }
}

/// What a rebuild needs of the text fields: their mirrored state, the keyboard focus and the
/// widgets being replaced.
#[derive(SystemParam)]
struct FieldsToRebuild<'w, 's> {
    texts: Res<'w, UiTextFields>,
    focus: ResMut<'w, InputFocus>,
    old: Query<'w, 's, (Entity, &'static PanelTextField, &'static EditableText)>,
}

/// Rebuilds every visible panel when `UiPanels` changed or the view was resized (or on the first
/// frame a camera has a size). A text field keeps its text and its keyboard focus across a rebuild.
fn rebuild_panels_when_needed(
    mut commands: Commands,
    panels: Res<UiPanels>,
    mut fields: FieldsToRebuild,
    cameras: Query<&Camera, With<Camera3d>>,
    roots: Query<Entity, With<PanelRoot>>,
    mut last_viewport: Local<Option<Vec2>>,
) {
    let Some(viewport) = cameras.iter().find_map(Camera::logical_viewport_size) else {
        return;
    };
    let viewport_changed = *last_viewport != Some(viewport);
    if !panels.is_changed() && !viewport_changed {
        return;
    }
    *last_viewport = Some(viewport);
    // The widgets being replaced are fresher than `UiTextFields` (mirrored after this frame's
    // edits), so a rebuild in the frame something was typed keeps it, and keeps the focus.
    let mut current_texts = fields.texts.texts.clone();
    let mut focused = fields.texts.focused;
    for (entity, field, editable) in &fields.old {
        current_texts.insert((field.panel, field.field.id), widget_text(editable));
        if fields.focus.get() == Some(entity) {
            focused = Some((field.panel, field.field.id));
        }
    }
    for root in &roots {
        commands.entity(root).despawn();
    }
    // Panels at one corner stack (`UiPanels::layouts`).
    for (id, layout) in panels.layouts(viewport) {
        let Some(panel) = panels.panel(id) else {
            continue;
        };
        let entries = panels.visible_entries(id);
        commands
            .spawn((
                Name::new(panel_root_name(id)),
                PanelRoot,
                Node {
                    width: percent(100),
                    height: percent(100),
                    ..default()
                },
                // The full-size root must not swallow clicks meant for the scene.
                Pickable::IGNORE,
            ))
            .with_children(|root| {
                if let (Some(title), Some(rect)) = (panel.title, layout.title) {
                    root.spawn((node_at(rect), Pickable::IGNORE)).with_child((
                        Text::new(title),
                        TextFont::from_font_size(TITLE_TEXT_SIZE),
                        TextColor(Color::WHITE),
                    ));
                }
                for (field, placed) in panels.text_fields(id).iter().zip(&layout.fields) {
                    let text = current_texts
                        .get(&(id, field.id))
                        .map_or("", String::as_str);
                    let entity = spawn_text_field(root, id, *field, placed.rect, text);
                    if focused == Some((id, field.id)) {
                        fields.focus.set(entity, FocusCause::Navigated);
                    }
                }
                for (entry, button) in entries.iter().zip(&layout.buttons) {
                    root.spawn((
                        Name::new(panel_button_name(id, entry.id)),
                        PanelButton {
                            panel: id,
                            entry: *entry,
                        },
                        UiClickTarget,
                        Button,
                        node_at(button.rect),
                        BackgroundColor(BUTTON_COLOR),
                        Hovered::default(),
                    ))
                    .with_child((
                        Text::new(entry.label),
                        TextFont::from_font_size(BUTTON_TEXT_SIZE),
                        TextColor(Color::WHITE),
                    ));
                }
            });
    }
}

/// Buttons that `ui.press` asked to activate, handled on the next `Update`: by entity name, not by
/// entity, because the panel may be rebuilt in between (a view resize; found in the browser).
#[derive(Resource, Default)]
struct PendingPresses(Vec<String>);

/// Fires `Activate` on each button `ui.press` asked for: the event a click ends with.
fn activate_pending_presses(
    mut pending: ResMut<PendingPresses>,
    buttons: Query<(Entity, &Name), With<PanelButton>>,
    mut commands: Commands,
) {
    for wanted in pending.0.drain(..) {
        match buttons.iter().find(|(_, name)| name.as_str() == wanted) {
            Some((button, _)) => commands.trigger(Activate { entity: button }),
            None => warn!("ui.press: button `{wanted}` is no longer shown; the press is dropped"),
        }
    }
}

/// Observer on every panel button: runs the entry's command when the button is activated. The
/// command runs as a queued world command, after the current system, never inside another
/// command's handler.
fn run_activated_button(
    activate: On<Activate>,
    buttons: Query<&PanelButton>,
    mut commands: Commands,
) {
    let Ok(button) = buttons.get(activate.entity) else {
        return;
    };
    let (panel, entry) = (button.panel, button.entry);
    commands.queue(move |world: &mut World| {
        let source = format!("button `{}` of panel `{panel}`", entry.id);
        dispatch_command_line(world, &source, entry.command_line);
    });
}

/// Hover and press feedback (a handful of buttons: cheap enough to set every frame).
fn color_buttons(
    mut buttons: Query<(Has<Pressed>, &Hovered, &mut BackgroundColor), With<PanelButton>>,
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

/// `ui.show_panel <panel>`.
fn show_panel_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let panel: String = args.parse(0, "panel")?;
    world
        .resource_mut::<UiPanels>()
        .set_visible(&panel, true)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

/// `ui.hide_panel <panel>`.
fn hide_panel_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let panel: String = args.parse(0, "panel")?;
    world
        .resource_mut::<UiPanels>()
        .set_visible(&panel, false)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

/// `ui.hide_entry <panel> <entry>`: the other buttons re-flow to fill its place.
fn hide_entry_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let panel: String = args.parse(0, "panel")?;
    let entry: String = args.parse(1, "entry")?;
    world
        .resource_mut::<UiPanels>()
        .hide_entry(&panel, &entry)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

/// `ui.show_entry <panel> <entry>`: shows a hidden button again, in its place.
fn show_entry_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let panel: String = args.parse(0, "panel")?;
    let entry: String = args.parse(1, "entry")?;
    world
        .resource_mut::<UiPanels>()
        .show_entry(&panel, &entry)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

/// `ui.press <panel> <entry>`: activates a shown button as a click would. The button's command
/// runs on this frame's `Update`, so later lines see its effect.
fn press_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let panel: String = args.parse(0, "panel")?;
    let entry: String = args.parse(1, "entry")?;
    world
        .resource::<UiPanels>()
        .entry(&panel, &entry)
        .map_err(|message| args.failure(message))?;
    let button_name = panel_button_name(&panel, &entry);
    let mut buttons = world.query_filtered::<&Name, With<PanelButton>>();
    let shown = buttons.iter(world).any(|name| name.as_str() == button_name);
    if !shown {
        return Err(args.failure(format!(
            "button `{entry}` of panel `{panel}` is not shown (the panel or the entry is hidden, \
             or the UI is not built yet)"
        )));
    }
    world.resource_mut::<PendingPresses>().0.push(button_name);
    Ok(CommandOutcome::Continue)
}

/// `ui.list`: logs every panel, whether it is shown, and its visible entries.
fn list_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let panels = world.resource::<UiPanels>();
    for id in panels.ids() {
        let entries: Vec<&str> = panels
            .visible_entries(id)
            .iter()
            .map(|entry| entry.id)
            .collect();
        info!(
            "ui panel `{id}`: {}, entries [{}]",
            if panels.is_visible(id) {
                "shown"
            } else {
                "hidden"
            },
            entries.join(", ")
        );
    }
    Ok(CommandOutcome::Continue)
}
