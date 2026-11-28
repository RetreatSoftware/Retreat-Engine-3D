//! Draws the contract's `UiHud` (`retreat_engine_api/src/ui_hud.rs`) with `bevy_ui`, and registers
//! the widget commands. Where each widget goes is decided by the contract's pure
//! `compute_hud_layout`; the widgets are rebuilt when the HUD, the panels or the view size change.
//! A panel's width is Bevy's own layout of the text inside it.

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, HUD_PANEL_PADDING, HudAnchor,
    HudBlock, HudKind, HudOccupied, HudState, HudStyle, PANEL_MARGIN, PanelAnchor, PanelRect,
    UiHud, UiPanels, compute_hud_layout, compute_panel_layout,
};

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
            )
            .register_command(
                "ui.set_text_color",
                "ui.set_text_color <widget> <r> <g> <b>",
                set_color_command,
            )
            .register_command(
                "ui.set_panel",
                "ui.set_panel <widget> <r> <g> <b> <a>",
                set_panel_command,
            )
            .register_command(
                "ui.clear_panel",
                "ui.clear_panel <widget>",
                clear_panel_command,
            )
            .register_command(
                "assert.ui_text_color",
                "assert.ui_text_color <widget> <r> <g> <b>",
                assert_color_command,
            )
            .register_command(
                "assert.ui_panel",
                "assert.ui_panel <widget> <r> <g> <b> <a>",
                assert_panel_command,
            );
    }
}

/// The root node of one block.
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
            panels.text_fields(id),
            &panels.visible_entries(id),
            panel.title.is_some(),
            panel.anchor,
            panel.direction,
            viewport,
        );
        if layout.is_empty() {
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
    let widgets: Vec<&HudState> = hud.widgets().collect();
    let occupied = occupied_by_panels(&panels, viewport);
    for block in compute_hud_layout(&widgets, viewport, occupied) {
        spawn_block(&mut commands, &hud, &block);
    }
}

fn align(anchor: HudAnchor) -> AlignItems {
    match justify(anchor) {
        JustifyContent::FlexStart => AlignItems::FlexStart,
        JustifyContent::FlexEnd => AlignItems::FlexEnd,
        _ => AlignItems::Center,
    }
}

/// One block: its box, in it the background (as wide as its text), in that the lines.
fn spawn_block(commands: &mut Commands, hud: &UiHud, block: &HudBlock) {
    let Some(&(first, _)) = block.lines.first() else {
        return;
    };
    let mut root = commands.spawn((
        Name::new(format!("hud_block/{first}")),
        HudRoot,
        Node {
            align_items: AlignItems::Stretch,
            ..node_at(block.rect, justify(block.anchor))
        },
        Pickable::IGNORE,
    ));
    root.with_children(|root| {
        let padding = if block.panel.is_some() {
            HUD_PANEL_PADDING
        } else {
            Vec2::ZERO
        };
        let mut background = root.spawn((
            Node {
                // A line longer than the box widens the panel instead of running out of it.
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                align_items: align(block.anchor),
                padding: UiRect::axes(px(padding.x), px(padding.y)),
                ..default()
            },
            Pickable::IGNORE,
        ));
        if let Some(color) = block.panel {
            background.insert(BackgroundColor(color));
        }
        background.with_children(|background| {
            for &(id, rect) in &block.lines {
                let Some(state) = hud.widget(id) else {
                    continue;
                };
                let name = Name::new(format!("hud/{id}"));
                match state.widget.kind {
                    HudKind::Text { font_size } => {
                        background
                            .spawn((
                                name,
                                Node {
                                    height: px(rect.height),
                                    align_items: AlignItems::Center,
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ))
                            .with_child((
                                Text::new(state.text.clone()),
                                TextFont::from_font_size(font_size),
                                TextColor(state.style.color),
                                TextLayout::no_wrap(),
                            ));
                    }
                    HudKind::Crosshair { size } => {
                        let color = state.style.color;
                        let bar = |width: f32, height: f32| {
                            let rect = PanelRect {
                                left: (size - width) / 2.0,
                                top: (size - height) / 2.0,
                                width,
                                height,
                            };
                            (
                                node_at(rect, JustifyContent::Center),
                                BackgroundColor(color),
                                Pickable::IGNORE,
                            )
                        };
                        background
                            .spawn((
                                name,
                                Node {
                                    width: px(size),
                                    height: px(size),
                                    ..default()
                                },
                                Pickable::IGNORE,
                            ))
                            .with_children(|cross| {
                                cross.spawn(bar(size, CROSSHAIR_THICKNESS));
                                cross.spawn(bar(CROSSHAIR_THICKNESS, size));
                            });
                    }
                }
            }
        });
    });
}

/// Reads `count` numbers from 0 to 1 starting at argument 1.
fn color_numbers(
    args: &CommandArgs,
    count: usize,
) -> Result<Vec<f32>, retreat_engine_api::CommandError> {
    args.expect_count(count + 1)?;
    (1..=count)
        .map(|index| {
            let value: f32 = args.parse(index, "color")?;
            if (0.0..=1.0).contains(&value) {
                Ok(value)
            } else {
                Err(args.failure(format!("color numbers go from 0 to 1, got {value}")))
            }
        })
        .collect()
}

/// Whether two colors look the same, whichever color space each was made in (`Color::WHITE` is
/// linear, a command's numbers are sRGB).
fn same_color(left: Color, right: Color) -> bool {
    let (left, right) = (left.to_srgba(), right.to_srgba());
    [
        left.red - right.red,
        left.green - right.green,
        left.blue - right.blue,
        left.alpha - right.alpha,
    ]
    .iter()
    .all(|difference| difference.abs() < 1.0e-3)
}

fn state_of(
    world: &World,
    args: &CommandArgs,
) -> Result<HudState, retreat_engine_api::CommandError> {
    let id: String = args.parse(0, "widget")?;
    world
        .resource::<UiHud>()
        .widget(&id)
        .cloned()
        .ok_or_else(|| args.failure(format!("there is no HUD widget `{id}`")))
}

fn change_style(
    world: &mut World,
    args: &CommandArgs,
    change: impl FnOnce(&mut HudStyle),
) -> CommandResult {
    let state = state_of(world, args)?;
    let mut style = state.style;
    change(&mut style);
    world
        .resource_mut::<UiHud>()
        .set_style(state.widget.id, style)
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}

fn set_color_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let n = color_numbers(args, 3)?;
    change_style(world, args, |style| {
        style.color = Color::srgb(n[0], n[1], n[2])
    })
}

fn set_panel_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let n = color_numbers(args, 4)?;
    change_style(world, args, |style| {
        style.panel = Some(Color::srgba(n[0], n[1], n[2], n[3]));
    })
}

fn clear_panel_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    change_style(world, args, |style| style.panel = None)
}

fn assert_color_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let n = color_numbers(args, 3)?;
    let state = state_of(world, args)?;
    if same_color(state.style.color, Color::srgb(n[0], n[1], n[2])) {
        return Ok(CommandOutcome::Continue);
    }
    Err(args.failure(format!(
        "widget `{}` has color {:?}",
        state.widget.id, state.style.color
    )))
}

fn assert_panel_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let n = color_numbers(args, 4)?;
    let state = state_of(world, args)?;
    let expected = Color::srgba(n[0], n[1], n[2], n[3]);
    if state
        .style
        .panel
        .is_some_and(|panel| same_color(panel, expected))
    {
        return Ok(CommandOutcome::Continue);
    }
    Err(args.failure(format!(
        "widget `{}` has panel {:?}",
        state.widget.id, state.style.panel
    )))
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
