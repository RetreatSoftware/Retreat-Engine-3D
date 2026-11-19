//! UI toolkit, engine side: draws the contract's `UiPanels` with `bevy_ui` (Bevy 0.20 widgets) and
//! runs a button's command line when it is activated. This is the one place that turns panel
//! descriptions into UI nodes; where each button goes is decided by the contract's pure
//! `compute_panel_layout`.
//!
//! The UI is rebuilt as a whole when the `UiPanels` resource changes (show, hide a panel or an
//! entry) or when the view size changes, so hiding anything re-flows what is left.
//!
//! Buttons are `bevy::ui_widgets::Button`s: a completed click (or Enter/Space when focused) fires
//! an `Activate` event, and an observer runs the entry's command through `dispatch_command_line`.
//! `ui.press` fires the same `Activate` on the next `Update`, so a scripted press runs the same code
//! as a real one. (It cannot fire it from inside its own handler: the button's command would then
//! run inside another command, which the registry forbids.)
//!
//! Commands: `ui.show_panel <panel>`, `ui.hide_panel <panel>`, `ui.hide_entry <panel> <entry>`,
//! `ui.show_entry <panel> <entry>`, `ui.press <panel> <entry>`, `ui.list`.
//!
//! Startup check (`Plugin::finish`): every entry's command must be registered; otherwise the app
//! panics naming the panel entry and the missing command.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Pressed;
use bevy::ui_widgets::{Activate, Button, observe};
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandRegistry, CommandResult, PanelRect,
    UiButtonEntry, UiPanels, compute_panel_layout,
};

use crate::command_dispatch::dispatch_command_line;

const BUTTON_COLOR: Color = Color::srgb(0.16, 0.17, 0.20);
const BUTTON_HOVERED_COLOR: Color = Color::srgb(0.25, 0.27, 0.32);
const BUTTON_PRESSED_COLOR: Color = Color::srgb(0.35, 0.45, 0.65);
const BUTTON_TEXT_SIZE: f32 = 16.0;
const TITLE_TEXT_SIZE: f32 = 20.0;

/// Adds panel drawing and the `ui.*` commands.
pub struct UiPanelsPlugin;

impl Plugin for UiPanelsPlugin {
    fn build(&self, app: &mut App) {
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

/// Rebuilds every visible panel when `UiPanels` changed or the view was resized (or on the first
/// frame a camera has a size).
fn rebuild_panels_when_needed(
    mut commands: Commands,
    panels: Res<UiPanels>,
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
    for root in &roots {
        commands.entity(root).despawn();
    }
    for id in panels.ids() {
        if !panels.is_visible(id) {
            continue;
        }
        let Some(panel) = panels.panel(id) else {
            continue;
        };
        let entries = panels.visible_entries(id);
        let layout = compute_panel_layout(
            &entries,
            panel.title.is_some(),
            panel.anchor,
            panel.direction,
            viewport,
        );
        if layout.buttons.is_empty() {
            continue;
        }
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
                for (entry, button) in entries.iter().zip(&layout.buttons) {
                    root.spawn((
                        Name::new(panel_button_name(id, entry.id)),
                        PanelButton {
                            panel: id,
                            entry: *entry,
                        },
                        Button,
                        node_at(button.rect),
                        BackgroundColor(BUTTON_COLOR),
                        Hovered::default(),
                        observe(run_activated_button),
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

/// Buttons that `ui.press` asked to activate, handled on the next `Update`.
#[derive(Resource, Default)]
struct PendingPresses(Vec<Entity>);

/// Fires `Activate` on each button `ui.press` asked for: the event a click ends with.
fn activate_pending_presses(mut pending: ResMut<PendingPresses>, mut commands: Commands) {
    for button in pending.0.drain(..) {
        commands.trigger(Activate { entity: button });
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
    let mut buttons = world.query_filtered::<(Entity, &Name), With<PanelButton>>();
    let Some(button) = buttons
        .iter(world)
        .find(|(_, name)| name.as_str() == button_name)
        .map(|(entity, _)| entity)
    else {
        return Err(args.failure(format!(
            "button `{entry}` of panel `{panel}` is not shown (the panel or the entry is hidden, \
             or the UI is not built yet)"
        )));
    };
    world.resource_mut::<PendingPresses>().0.push(button);
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
