//! Draws the contract's `UiHud` (`retreat_engine_api/src/ui_hud.rs`) with `bevy_ui`, and registers
//! the widget commands. Where each widget goes is decided by the contract's pure
//! `compute_hud_layout`; the widgets are rebuilt when the HUD, the panels or the view size change.

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, HudAnchor, HudKind, HudOccupied,
    PANEL_MARGIN, PanelAnchor, PanelRect, UiHud, UiPanels, compute_hud_layout,
    compute_panel_layout,
};

const CROSSHAIR_COLOR: Color = Color::srgb(0.95, 0.95, 0.95);
const CROSSHAIR_THICKNESS: f32 = 2.0;

pub struct UiHudPlugin;

impl Plugin for UiHudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiHud>()
            .init_resource::<UiPanels>()
            .add_systems(Update, rebuild_hud_when_needed)
            .register_command(
                "ui.set_text",
                "ui.set_text <widget> <text...>",
                set_text_command,
            )
            .register_command("ui.show_widget", "ui.show_widget <widget>", show_command)
            .register_command("ui.hide_widget", "ui.hide_widget <widget>", hide_command)
            .register_command(
                "assert.ui_text",
                "assert.ui_text <widget> <text...>",
                assert_text_command,
            );
    }
}

/// The root node of one widget.
#[derive(Component)]
struct HudRoot;

/// The height the shown button panels take at the top corners.
fn occupied_by_panels(panels: &UiPanels, viewport: Vec2) -> HudOccupied {
    let mut occupied = HudOccupied::default();
    for id in panels.ids() {
        let Some(panel) = panels.panel(id).filter(|_| panels.is_visible(id)) else {
            continue;
        };
        let layout = compute_panel_layout(
            &panels.visible_entries(id),
            panel.title.is_some(),
            panel.anchor,
            panel.direction,
            viewport,
        );
        if layout.buttons.is_empty() {
            continue;
        }
        let height = layout.bounds.top + layout.bounds.height - PANEL_MARGIN;
        match panel.anchor {
            PanelAnchor::TopLeft => occupied.top_left = occupied.top_left.max(height),
            PanelAnchor::TopRight => occupied.top_right = occupied.top_right.max(height),
            PanelAnchor::Center => {}
        }
    }
    occupied
}

fn node_at(rect: PanelRect, justify: JustifyContent) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(rect.left),
        top: px(rect.top),
        width: px(rect.width),
        height: px(rect.height),
        justify_content: justify,
        align_items: AlignItems::Center,
        ..default()
    }
}

fn justify(anchor: HudAnchor) -> JustifyContent {
    match anchor {
        HudAnchor::TopLeft | HudAnchor::BottomLeft => JustifyContent::FlexStart,
        HudAnchor::TopRight | HudAnchor::BottomRight => JustifyContent::FlexEnd,
        HudAnchor::TopCenter | HudAnchor::Center | HudAnchor::BottomCenter => {
            JustifyContent::Center
        }
    }
}

fn rebuild_hud_when_needed(
    mut commands: Commands,
    hud: Res<UiHud>,
    panels: Res<UiPanels>,
    cameras: Query<&Camera, With<Camera3d>>,
    roots: Query<Entity, With<HudRoot>>,
    mut last_viewport: Local<Option<Vec2>>,
) {
    let Some(viewport) = cameras.iter().find_map(Camera::logical_viewport_size) else {
        return;
    };
    if !hud.is_changed() && !panels.is_changed() && *last_viewport == Some(viewport) {
        return;
    }
    *last_viewport = Some(viewport);
    for root in &roots {
        commands.entity(root).despawn();
    }
    let shown: Vec<_> = hud.widgets().filter(|state| state.visible).collect();
    let widgets: Vec<_> = shown.iter().map(|state| state.widget).collect();
    let layout = compute_hud_layout(&widgets, viewport, occupied_by_panels(&panels, viewport));
    for (state, (id, rect)) in shown.iter().zip(layout) {
        let mut root = commands.spawn((
            Name::new(format!("hud/{id}")),
            HudRoot,
            node_at(rect, justify(state.widget.anchor)),
            Pickable::IGNORE,
        ));
        match state.widget.kind {
            HudKind::Text { font_size } => {
                root.with_child((
                    Text::new(state.text.clone()),
                    TextFont::from_font_size(font_size),
                    TextColor(Color::WHITE),
                ));
            }
            HudKind::Crosshair { size } => {
                let bar = |width: f32, height: f32| {
                    let rect = PanelRect {
                        left: (size - width) / 2.0,
                        top: (size - height) / 2.0,
                        width,
                        height,
                    };
                    (
                        node_at(rect, JustifyContent::Center),
                        BackgroundColor(CROSSHAIR_COLOR),
                        Pickable::IGNORE,
                    )
                };
                root.with_child(bar(size, CROSSHAIR_THICKNESS));
                root.with_child(bar(CROSSHAIR_THICKNESS, size));
            }
        }
    }
}

fn set_text_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let id: String = args.parse(0, "widget")?;
    let text = args.rest(1).join(" ");
    world
        .resource_mut::<UiHud>()
        .set_text(&id, text)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

fn set_widget_visible(world: &mut World, args: &CommandArgs, visible: bool) -> CommandResult {
    args.expect_count(1)?;
    let id: String = args.parse(0, "widget")?;
    world
        .resource_mut::<UiHud>()
        .set_visible(&id, visible)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

fn show_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    set_widget_visible(world, args, true)
}

fn hide_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    set_widget_visible(world, args, false)
}

/// `assert.ui_text <widget> <text...>`: the widget is shown with exactly this text.
fn assert_text_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let id: String = args.parse(0, "widget")?;
    let expected = args.rest(1).join(" ");
    match world.resource::<UiHud>().widget(&id) {
        Some(state) if state.visible && state.text == expected => Ok(CommandOutcome::Continue),
        Some(state) => Err(args.failure(format!(
            "widget `{id}` is {} with `{}`, expected it shown with `{expected}`",
            if state.visible { "shown" } else { "hidden" },
            state.text
        ))),
        None => Err(args.failure(format!("there is no HUD widget `{id}`"))),
    }
}
