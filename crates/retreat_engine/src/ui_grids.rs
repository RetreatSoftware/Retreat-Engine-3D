//! Item grid windows, engine side (crawler plan CE2): draws the contract's `UiGrids` with `bevy_ui`
//! and turns the mouse into hover, scroll, drag, rotate and drop. Where everything goes is decided
//! by the contract's pure functions (`compute_grid_layout`, `item_at`, `drop_origin`, `fits_at`,
//! `tooltip_rect`); this file only reads the pointer and builds nodes.
//!
//! **Two layers, each rebuilt as a whole only when what it shows changes:** the windows (views,
//! scroll, which item is being dragged, view size) and the overlay (tooltip, drop preview, the
//! dragged item). A drag that follows the cursor outside every window moves one node instead.
//!
//! **Mouse and commands share one path:** a press on an item, `R` / right button, the pointer over
//! a window and a release change the same `UiGrids` state the `ui.drag_*` commands change, and a
//! drop (either way) is queued and runs the view's command line through `dispatch_command_line` on
//! the next `Update` (never inside another command). The pointer changes hover and target only when
//! it moves, so a script's hover or target stays put under a still mouse. Touch is not handled yet.

use std::collections::BTreeMap;

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, GRID_FOOTER_TEXT_SIZE,
    GRID_TITLE_TEXT_SIZE, GridLayout, GridWindowRect, HudState, PanelRect, UiGridDrag, UiGridItem,
    UiGridShape, UiGridView, UiGrids, UiHud, UiPanels, UiTextFields, cell_at, cells_rect,
    compute_grid_layout, drop_command_line, drop_origin, fits_at, item_at, item_rect, tooltip_rect,
    top_area_bottom,
};

use crate::asset_paths::{load_asset, resolve_asset_location};
use crate::command_dispatch::dispatch_command_line;
use crate::platform::HAS_FILE_SYSTEM;
use crate::ui_panels::UiClickTarget;

const WINDOW_COLOR: Color = Color::srgba(0.08, 0.09, 0.11, 0.92);
const CELL_COLOR: Color = Color::srgb(0.17, 0.18, 0.21);
/// The lines between cells: the cell area's own opaque background, seen in the gaps. Opaque so
/// the lines look the same over any scene (found 2026-11-28: with the see-through window
/// background in the gaps, a row in front of a dark wall or sky showed no lines at all).
const GRID_LINE_COLOR: Color = Color::srgb(0.34, 0.36, 0.41);
const SLOT_COLOR: Color = Color::srgb(0.20, 0.21, 0.25);
const SLOT_LABEL_COLOR: Color = Color::srgb(0.55, 0.56, 0.60);
const TITLE_COLOR: Color = Color::WHITE;
const FOOTER_COLOR: Color = Color::srgb(0.95, 0.85, 0.45);
const SCROLL_TRACK_COLOR: Color = Color::srgb(0.14, 0.15, 0.17);
const SCROLL_THUMB_COLOR: Color = Color::srgb(0.45, 0.47, 0.52);
const TOOLTIP_COLOR: Color = Color::srgba(0.03, 0.03, 0.04, 0.95);
const FITS_COLOR: Color = Color::srgba(0.2, 0.8, 0.3, 0.45);
const BLOCKED_COLOR: Color = Color::srgba(0.9, 0.2, 0.2, 0.45);
/// How much of an item's look stays while it is dragged away from its place.
const DRAGGED_SOURCE_ALPHA: f32 = 0.35;
/// The dragged item's look over its preview.
const GHOST_ALPHA: f32 = 0.8;
const LABEL_TEXT_SIZE: f32 = 13.0;
const TOOLTIP_TEXT_SIZE: f32 = 15.0;
/// Space between neighboring cells (the grid lines).
const CELL_INSET: f32 = 1.0;
/// Space around an equipment slot, so neighboring slots read as separate boxes.
const SLOT_INSET: f32 = 3.0;

/// Adds grid drawing, the pointer handling and the `ui.*` grid commands.
pub struct UiGridsPlugin;

impl Plugin for UiGridsPlugin {
    fn build(&self, app: &mut App) {
        // The wheel is read even in apps without input (tests); adding it twice is harmless.
        app.add_message::<MouseWheel>();
        app.init_resource::<UiGrids>()
            .init_resource::<PendingGridDrops>()
            .init_resource::<GridImages>()
            .register_command("ui.list_grids", "ui.list_grids", list_command)
            .register_command(
                "ui.scroll_grid",
                "ui.scroll_grid <grid> <first row>",
                scroll_command,
            )
            .register_command(
                "ui.drag_start",
                "ui.drag_start <grid> <item>",
                drag_start_command,
            )
            .register_command("ui.drag_rotate", "ui.drag_rotate", drag_rotate_command)
            .register_command(
                "ui.drag_over",
                "ui.drag_over <grid> <x> <y>",
                drag_over_command,
            )
            .register_command(
                "ui.drag_drop",
                "ui.drag_drop [<grid> <x> <y>]",
                drag_drop_command,
            )
            .register_command("ui.drag_cancel", "ui.drag_cancel", drag_cancel_command)
            .register_command(
                "ui.hover_item",
                "ui.hover_item <grid> <item>",
                hover_command,
            )
            .register_command("ui.unhover", "ui.unhover", unhover_command)
            .register_command(
                "assert.ui_grid_item",
                "assert.ui_grid_item <grid> <item> <x> <y> [upright|rotated]",
                assert_item_command,
            )
            .register_command(
                "assert.ui_grid_absent",
                "assert.ui_grid_absent <grid> <item>",
                assert_absent_command,
            )
            .register_command(
                "assert.ui_grid_count",
                "assert.ui_grid_count <grid> <items>",
                assert_count_command,
            )
            .register_command(
                "assert.ui_grid_shown",
                "assert.ui_grid_shown <grid>",
                assert_shown_command,
            )
            .register_command(
                "assert.ui_grid_hidden",
                "assert.ui_grid_hidden <grid>",
                assert_hidden_command,
            )
            .register_command(
                "assert.ui_grid_scroll",
                "assert.ui_grid_scroll <grid> <row>",
                assert_scroll_command,
            )
            .register_command(
                "assert.ui_tooltip",
                "assert.ui_tooltip <text...>",
                assert_tooltip_command,
            )
            .register_command(
                "assert.ui_dragging",
                "assert.ui_dragging <item|nothing>",
                assert_dragging_command,
            )
            .register_command(
                "assert.ui_images_loaded",
                "assert.ui_images_loaded",
                assert_images_loaded_command,
            )
            .register_command(
                "assert.ui_grid_lines",
                "assert.ui_grid_lines <grid>",
                assert_grid_lines_command,
            )
            .add_systems(
                Update,
                (
                    handle_pointer,
                    run_pending_drops,
                    rebuild_windows_when_needed,
                    rebuild_overlay_when_needed,
                    follow_cursor_with_ghost,
                )
                    .chain(),
            );
    }
}

/// Drops waiting to run their command line (on the next `Update`).
#[derive(Resource, Default)]
struct PendingGridDrops(Vec<String>);

/// Every icon the windows use, by path, kept loaded while the app runs.
#[derive(Resource, Default)]
struct GridImages(BTreeMap<String, Handle<Image>>);

/// Root of the windows layer.
#[derive(Component)]
struct GridWindowsRoot;

/// Root of the overlay layer (tooltip, preview, dragged item).
#[derive(Component)]
struct GridOverlayRoot;

/// The dragged item's node while it follows the cursor (no target).
#[derive(Component)]
struct DragGhost {
    /// Size of the ghost, to center the grabbed cell on the cursor.
    grab_offset: Vec2,
}

/// The view size of the camera bevy_ui draws on.
fn viewport(cameras: &Query<&Camera, With<Camera3d>>) -> Option<Vec2> {
    cameras.iter().find_map(Camera::logical_viewport_size)
}

/// How far down the top of the view is taken (button panels and HUD text at the top): the windows
/// stay below it (the contract's `top_area_bottom`).
fn reserved_top(panels: Option<&UiPanels>, hud: Option<&UiHud>, viewport: Vec2) -> f32 {
    let no_panels = UiPanels::default();
    let widgets: Vec<&HudState> = hud.map(|hud| hud.widgets().collect()).unwrap_or_default();
    top_area_bottom(panels.unwrap_or(&no_panels), &widgets, viewport)
}

fn current_layout(grids: &UiGrids, viewport: Vec2, reserved_top: f32) -> GridLayout {
    let views: Vec<(&str, &UiGridView)> = grids.views().collect();
    compute_grid_layout(&views, viewport, reserved_top)
}

fn node_at(rect: PanelRect) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(rect.left),
        top: px(rect.top),
        width: px(rect.width),
        height: px(rect.height),
        ..default()
    }
}

/// `rect` relative to `origin`'s top-left.
fn relative(rect: PanelRect, origin: PanelRect) -> PanelRect {
    PanelRect {
        left: rect.left - origin.left,
        top: rect.top - origin.top,
        ..rect
    }
}

fn inset(rect: PanelRect, by: f32) -> PanelRect {
    PanelRect {
        left: rect.left + by,
        top: rect.top + by,
        width: (rect.width - 2.0 * by).max(0.0),
        height: (rect.height - 2.0 * by).max(0.0),
    }
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    color.with_alpha(color.alpha() * alpha)
}

/// The icon of `path`, loaded once; `None` (and a warning) for a path that cannot be an icon.
fn image_handle(
    images: &mut GridImages,
    asset_server: &AssetServer,
    path: &str,
) -> Option<Handle<Image>> {
    if let Some(handle) = images.0.get(path) {
        return Some(handle.clone());
    }
    match resolve_asset_location(path, "PNG image", &["png"], HAS_FILE_SYSTEM) {
        Ok(location) => {
            let outside = location.is_outside_asset_folder();
            let handle = load_asset::<Image>(asset_server, location.into_asset_path(), outside);
            images.0.insert(path.to_string(), handle.clone());
            Some(handle)
        }
        Err(problem) => {
            warn!("grid item icon: {problem}");
            None
        }
    }
}

/// Spawns the look of `item` filling `rect` (relative to the parent): background, icon turned with
/// the item, label. `upright` is the icon's unturned size; `None` fills the rect (a slot).
fn spawn_item_look(
    parent: &mut ChildSpawnerCommands,
    item: &UiGridItem,
    rect: PanelRect,
    rotated: bool,
    upright: Option<Vec2>,
    alpha: f32,
    icon: Option<Handle<Image>>,
) {
    let shown = inset(rect, CELL_INSET);
    parent
        .spawn((
            node_at(shown),
            BackgroundColor(with_alpha(item.color, alpha)),
            Pickable::IGNORE,
        ))
        .with_children(|look| {
            if let Some(handle) = icon {
                let size = match upright {
                    Some(size) if rotated => size - Vec2::splat(2.0 * CELL_INSET),
                    _ => Vec2::new(shown.width, shown.height),
                };
                let icon_rect = PanelRect {
                    left: (shown.width - size.x) / 2.0,
                    top: (shown.height - size.y) / 2.0,
                    width: size.x,
                    height: size.y,
                };
                let mut image = ImageNode::new(handle);
                // The icon takes the item's cells whatever its own proportions.
                image.image_mode = bevy::ui::widget::NodeImageMode::Stretch;
                image.color = Color::WHITE.with_alpha(alpha);
                let turn = if rotated && upright.is_some() {
                    UiTransform::from_rotation(Rot2::degrees(90.0))
                } else {
                    UiTransform::IDENTITY
                };
                look.spawn((node_at(icon_rect), image, turn, Pickable::IGNORE));
            }
            if !item.label.is_empty() {
                look.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(3.0),
                        bottom: px(1.0),
                        ..default()
                    },
                    Text::new(item.label.clone()),
                    TextFont::from_font_size(LABEL_TEXT_SIZE),
                    TextColor(Color::WHITE.with_alpha(alpha)),
                    Pickable::IGNORE,
                ));
            }
        });
}

fn spawn_text(
    parent: &mut ChildSpawnerCommands,
    rect: PanelRect,
    text: &str,
    size: f32,
    color: Color,
) {
    parent.spawn((
        node_at(rect),
        Text::new(text.to_string()),
        TextFont::from_font_size(size),
        TextColor(color),
        Pickable::IGNORE,
    ));
}

/// What the windows layer shows: rebuilt when any of it changes.
#[derive(PartialEq, Clone, Default)]
struct WindowsShown {
    views: Vec<(String, UiGridView)>,
    scroll: Vec<u32>,
    dragged: Option<(String, String)>,
    viewport: Vec2,
    reserved_top: f32,
}

/// What the overlay layer shows.
#[derive(PartialEq, Clone, Default)]
struct OverlayShown {
    hovered: Option<(String, String)>,
    drag: Option<UiGridDrag>,
    windows: Option<WindowsShown>,
}

fn windows_shown(grids: &UiGrids, viewport: Vec2, reserved_top: f32) -> WindowsShown {
    WindowsShown {
        views: grids
            .views()
            .map(|(id, view)| (id.to_string(), view.clone()))
            .collect(),
        scroll: grids.views().map(|(id, _)| grids.scroll(id)).collect(),
        dragged: grids
            .drag()
            .map(|drag| (drag.grid.clone(), drag.item.clone())),
        viewport,
        reserved_top,
    }
}

/// Rebuilds the windows when views, scroll, the dragged item, the view size or the corner panels
/// changed.
#[allow(clippy::too_many_arguments)]
fn rebuild_windows_when_needed(
    mut commands: Commands,
    grids: Res<UiGrids>,
    panels: Option<Res<UiPanels>>,
    hud: Option<Res<UiHud>>,
    cameras: Query<&Camera, With<Camera3d>>,
    roots: Query<Entity, With<GridWindowsRoot>>,
    asset_server: Option<Res<AssetServer>>,
    mut images: ResMut<GridImages>,
    mut last: Local<Option<WindowsShown>>,
) {
    let Some(viewport) = viewport(&cameras) else {
        return;
    };
    let panels_changed = panels.as_ref().is_some_and(|panels| panels.is_changed())
        || hud.as_ref().is_some_and(|hud| hud.is_changed());
    if !grids.is_changed()
        && !panels_changed
        && last
            .as_ref()
            .is_some_and(|shown| shown.viewport == viewport)
    {
        return;
    }
    let reserved_top = reserved_top(panels.as_deref(), hud.as_deref(), viewport);
    let shown = windows_shown(&grids, viewport, reserved_top);
    if last.as_ref() == Some(&shown) {
        return;
    }
    *last = Some(shown);
    for root in &roots {
        commands.entity(root).despawn();
    }
    let layout = current_layout(&grids, viewport, reserved_top);
    if layout.windows.is_empty() {
        return;
    }
    let dragged = grids
        .drag()
        .map(|drag| (drag.grid.as_str(), drag.item.as_str()));
    commands
        .spawn((
            Name::new("ui/grids"),
            GridWindowsRoot,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|root| {
            for window in &layout.windows {
                let view = grids
                    .view(&window.grid)
                    .expect("laid out from the shown views");
                let scroll = grids.scroll(&window.grid);
                let mut icon = |path: &Option<String>| {
                    let server = asset_server.as_ref()?;
                    image_handle(&mut images, server, path.as_ref()?)
                };
                let icons: Vec<Option<Handle<Image>>> =
                    view.items.iter().map(|item| icon(&item.image)).collect();
                spawn_window(root, view, window, layout.cell, scroll, dragged, icons);
            }
        });
}

fn spawn_window(
    root: &mut ChildSpawnerCommands,
    view: &UiGridView,
    window: &GridWindowRect,
    cell: f32,
    scroll: u32,
    dragged: Option<(&str, &str)>,
    icons: Vec<Option<Handle<Image>>>,
) {
    root.spawn((
        Name::new(format!("ui/grid/{}", window.grid)),
        node_at(window.rect),
        BackgroundColor(WINDOW_COLOR),
        // The window takes clicks: a press on it runs no input action (no cursor capture).
        UiClickTarget,
    ))
    .with_children(|frame| {
        let at = |rect: PanelRect| relative(rect, window.rect);
        spawn_text(
            frame,
            at(window.title),
            &view.title,
            GRID_TITLE_TEXT_SIZE,
            TITLE_COLOR,
        );
        if let Some(footer) = window.footer {
            spawn_text(
                frame,
                at(footer),
                &view.footer,
                GRID_FOOTER_TEXT_SIZE,
                FOOTER_COLOR,
            );
        }
        if let Some(track) = window.scrollbar {
            frame.spawn((
                node_at(at(track)),
                BackgroundColor(SCROLL_TRACK_COLOR),
                Pickable::IGNORE,
            ));
            let rows = view.rows as f32;
            let thumb = PanelRect {
                top: track.top + track.height * scroll as f32 / rows,
                height: (track.height * view.shown_rows() as f32 / rows).max(4.0),
                ..track
            };
            frame.spawn((
                node_at(at(thumb)),
                BackgroundColor(SCROLL_THUMB_COLOR),
                Pickable::IGNORE,
            ));
        }
        // Cells are drawn inset on an opaque line color; slots stand on the window itself.
        let lines = match view.shape {
            UiGridShape::Cells => GRID_LINE_COLOR,
            UiGridShape::Slots(_) => Color::NONE,
        };
        frame
            .spawn((
                Name::new(format!("ui/grid/{}/cells", window.grid)),
                Node {
                    overflow: Overflow::clip(),
                    ..node_at(at(window.cells))
                },
                BackgroundColor(lines),
                Pickable::IGNORE,
            ))
            .with_children(|cells| {
                let inside = |rect: PanelRect| relative(rect, window.cells);
                match &view.shape {
                    UiGridShape::Cells => {
                        for row in scroll..scroll + view.shown_rows() {
                            for column in 0..view.columns {
                                let rect =
                                    cells_rect(window, cell, scroll, column, row, UVec2::ONE);
                                cells.spawn((
                                    node_at(inset(inside(rect), CELL_INSET)),
                                    BackgroundColor(CELL_COLOR),
                                    Pickable::IGNORE,
                                ));
                            }
                        }
                    }
                    UiGridShape::Slots(slots) => {
                        for slot in slots {
                            let rect = cells_rect(window, cell, scroll, slot.x, slot.y, slot.size);
                            cells
                                .spawn((
                                    node_at(inset(inside(rect), SLOT_INSET)),
                                    BackgroundColor(SLOT_COLOR),
                                    Pickable::IGNORE,
                                ))
                                .with_child((
                                    Text::new(slot.label.clone()),
                                    TextFont::from_font_size(LABEL_TEXT_SIZE),
                                    TextColor(SLOT_LABEL_COLOR),
                                    Pickable::IGNORE,
                                ));
                        }
                    }
                }
                let first = scroll;
                let last = scroll + view.shown_rows();
                for (item, icon) in view.items.iter().zip(icons) {
                    if matches!(view.shape, UiGridShape::Cells)
                        && (item.y >= last || item.y + item.drawn_size().y <= first)
                    {
                        continue;
                    }
                    let rect = inside(item_rect(view, window, cell, scroll, item));
                    let alpha = if dragged == Some((window.grid.as_str(), item.id.as_str())) {
                        DRAGGED_SOURCE_ALPHA
                    } else {
                        1.0
                    };
                    let upright = matches!(view.shape, UiGridShape::Cells)
                        .then(|| item.size.as_vec2() * cell);
                    spawn_item_look(cells, item, rect, item.rotated, upright, alpha, icon);
                }
            });
    });
}

/// Rebuilds the tooltip, drop preview and dragged item when one of them changed.
#[allow(clippy::too_many_arguments)]
fn rebuild_overlay_when_needed(
    mut commands: Commands,
    grids: Res<UiGrids>,
    panels: Option<Res<UiPanels>>,
    hud: Option<Res<UiHud>>,
    cameras: Query<&Camera, With<Camera3d>>,
    roots: Query<Entity, With<GridOverlayRoot>>,
    asset_server: Option<Res<AssetServer>>,
    mut images: ResMut<GridImages>,
    mut last: Local<Option<OverlayShown>>,
) {
    let Some(viewport) = viewport(&cameras) else {
        return;
    };
    let panels_changed = panels.as_ref().is_some_and(|panels| panels.is_changed())
        || hud.as_ref().is_some_and(|hud| hud.is_changed());
    if !grids.is_changed() && !panels_changed && last.is_some() {
        return;
    }
    let reserved_top = reserved_top(panels.as_deref(), hud.as_deref(), viewport);
    let shown = OverlayShown {
        hovered: grids.hovered().map(|(g, i)| (g.to_string(), i.to_string())),
        drag: grids.drag().cloned(),
        windows: Some(windows_shown(&grids, viewport, reserved_top)),
    };
    if last.as_ref() == Some(&shown) {
        return;
    }
    *last = Some(shown);
    for root in &roots {
        commands.entity(root).despawn();
    }
    let layout = current_layout(&grids, viewport, reserved_top);
    let mut root = commands.spawn((
        Name::new("ui/grids_overlay"),
        GridOverlayRoot,
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            ..default()
        },
        // Above the windows.
        GlobalZIndex(10),
        Pickable::IGNORE,
    ));
    if let Some(drag) = grids.drag() {
        let Some(item) = grids
            .view(&drag.grid)
            .and_then(|view| view.item(&drag.item))
        else {
            return;
        };
        let icon = asset_server
            .as_ref()
            .zip(item.image.as_ref())
            .and_then(|(server, path)| image_handle(&mut images, server, path));
        let size = drag.drawn_size();
        let upright = drag.size.as_vec2() * layout.cell;
        let target = drag.target.as_ref().and_then(|(grid, origin)| {
            let view = grids.view(grid)?;
            let window = layout.window(grid)?;
            let scroll = grids.scroll(grid);
            let rect = match &view.shape {
                UiGridShape::Cells => {
                    cells_rect(window, layout.cell, scroll, origin.x, origin.y, size)
                }
                UiGridShape::Slots(slots) => {
                    let slot = slots.get(origin.x as usize)?;
                    cells_rect(window, layout.cell, scroll, slot.x, slot.y, slot.size)
                }
            };
            let fits = fits_at(view, &drag.item, size, origin.x, origin.y);
            Some((rect, fits, matches!(view.shape, UiGridShape::Slots(_))))
        });
        root.with_children(|overlay| match target {
            Some((rect, fits, is_slot)) => {
                overlay.spawn((
                    node_at(rect),
                    BackgroundColor(if fits { FITS_COLOR } else { BLOCKED_COLOR }),
                    Pickable::IGNORE,
                ));
                let upright = (!is_slot).then_some(upright);
                spawn_item_look(
                    overlay,
                    item,
                    rect,
                    drag.rotated,
                    upright,
                    GHOST_ALPHA,
                    icon,
                );
            }
            None => {
                let rect = PanelRect {
                    left: -1000.0,
                    top: -1000.0,
                    width: size.x as f32 * layout.cell,
                    height: size.y as f32 * layout.cell,
                };
                let grab_offset = (drag.grab.as_vec2() + Vec2::splat(0.5)) * layout.cell;
                overlay
                    .spawn((node_at(rect), DragGhost { grab_offset }, Pickable::IGNORE))
                    .with_children(|ghost| {
                        let own = PanelRect {
                            left: 0.0,
                            top: 0.0,
                            ..rect
                        };
                        spawn_item_look(
                            ghost,
                            item,
                            own,
                            drag.rotated,
                            Some(upright),
                            GHOST_ALPHA,
                            icon,
                        );
                    });
            }
        });
        return;
    }
    let Some((grid, item_id)) = grids.hovered() else {
        return;
    };
    let (Some(view), Some(window)) = (grids.view(grid), layout.window(grid)) else {
        return;
    };
    let Some(item) = view.item(item_id).filter(|item| !item.tooltip.is_empty()) else {
        return;
    };
    let anchor = item_rect(view, window, layout.cell, grids.scroll(grid), item);
    let rect = tooltip_rect(anchor, item.tooltip.len(), viewport);
    root.with_children(|overlay| {
        overlay
            .spawn((
                Name::new("ui/grid_tooltip"),
                node_at(rect),
                BackgroundColor(TOOLTIP_COLOR),
                Pickable::IGNORE,
            ))
            .with_children(|tooltip| {
                for (index, line) in item.tooltip.iter().enumerate() {
                    let line_rect = PanelRect {
                        left: retreat_engine_api::GRID_PADDING,
                        top: retreat_engine_api::GRID_PADDING
                            + index as f32 * retreat_engine_api::TOOLTIP_LINE_HEIGHT,
                        width: rect.width - 2.0 * retreat_engine_api::GRID_PADDING,
                        height: retreat_engine_api::TOOLTIP_LINE_HEIGHT,
                    };
                    spawn_text(
                        tooltip,
                        line_rect,
                        &line.text,
                        TOOLTIP_TEXT_SIZE,
                        line.color,
                    );
                }
            });
    });
}

/// Keeps a dragged item that is over no window under the cursor.
fn follow_cursor_with_ghost(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut ghosts: Query<(&DragGhost, &mut Node)>,
) {
    let Some(cursor) = windows.iter().next().and_then(Window::cursor_position) else {
        return;
    };
    for (ghost, mut node) in &mut ghosts {
        let left = px(cursor.x - ghost.grab_offset.x);
        let top = px(cursor.y - ghost.grab_offset.y);
        if node.left != left || node.top != top {
            node.left = left;
            node.top = top;
        }
    }
}

/// What the pointer is over: a window, the cell under it and the item there.
struct PointerHit {
    grid: String,
    cell: UVec2,
    item: Option<(String, UVec2)>,
}

fn hit_at(grids: &UiGrids, layout: &GridLayout, point: Vec2) -> Option<PointerHit> {
    for window in &layout.windows {
        let view = grids.view(&window.grid)?;
        let scroll = grids.scroll(&window.grid);
        let Some(cell) = cell_at(window, layout.cell, scroll, point) else {
            continue;
        };
        let item = item_at(view, window, layout.cell, scroll, point).map(|item| {
            let grab = match view.shape {
                UiGridShape::Cells => cell.saturating_sub(UVec2::new(item.x, item.y)),
                UiGridShape::Slots(_) => UVec2::ZERO,
            };
            (item.id.clone(), grab)
        });
        return Some(PointerHit {
            grid: window.grid.clone(),
            cell,
            item,
        });
    }
    None
}

/// The window whose frame (title, cells, scroll bar) is under `point`.
fn window_under(layout: &GridLayout, point: Vec2) -> Option<&GridWindowRect> {
    layout.windows.iter().find(|window| {
        let rect = window.rect;
        point.x >= rect.left
            && point.x < rect.left + rect.width
            && point.y >= rect.top
            && point.y < rect.top + rect.height
    })
}

/// The drop target of `drag` when the pointer is over `hit`.
fn target_for(
    grids: &UiGrids,
    drag: &UiGridDrag,
    hit: Option<&PointerHit>,
) -> Option<(String, UVec2)> {
    let hit = hit?;
    let view = grids.view(&hit.grid)?;
    drop_origin(view, hit.cell, drag.grab, drag.drawn_size())
        .map(|origin| (hit.grid.clone(), origin))
}

/// Queues the drop of the current drag at its target and ends the drag. Nothing runs when there is
/// no target or the item would land exactly where it is.
fn drop_dragged(grids: &mut UiGrids, pending: &mut PendingGridDrops) {
    let Some(drag) = grids.cancel_drag() else {
        return;
    };
    let Some((grid, origin)) = drag.target.clone() else {
        return;
    };
    let Some(view) = grids.view(&grid) else {
        return;
    };
    let unchanged = grid == drag.grid
        && view.item(&drag.item).is_some_and(|item| {
            item.x == origin.x && item.y == origin.y && item.rotated == drag.rotated
        });
    if !unchanged {
        pending.0.push(drop_command_line(
            view,
            &drag.item,
            origin.x,
            origin.y,
            drag.rotated,
        ));
    }
}

/// Mouse and keys: hover, wheel scroll, press to drag, `R` / right button to rotate, release to
/// drop, `Escape` to cancel.
#[allow(clippy::too_many_arguments)]
fn handle_pointer(
    mut grids: ResMut<UiGrids>,
    mut pending: ResMut<PendingGridDrops>,
    panels: Option<Res<UiPanels>>,
    hud: Option<Res<UiHud>>,
    cameras: Query<&Camera, With<Camera3d>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    fields: Option<Res<UiTextFields>>,
    mut wheel: MessageReader<MouseWheel>,
    mut last_cursor: Local<Option<Vec2>>,
) {
    let scrolled: f32 = wheel
        .read()
        .map(|event| match event.unit {
            MouseScrollUnit::Line => event.y,
            MouseScrollUnit::Pixel => event.y / 40.0,
        })
        .sum();
    let typing = fields.is_some_and(|fields| fields.is_typing());
    let key_pressed =
        |key: KeyCode| !typing && keys.as_ref().is_some_and(|keys| keys.just_pressed(key));
    let mouse_pressed =
        |button: MouseButton| mouse.as_ref().is_some_and(|m| m.just_pressed(button));
    let mouse_released =
        |button: MouseButton| mouse.as_ref().is_some_and(|m| m.just_released(button));
    // Keys act on a drag whether it came from the mouse or a script.
    if grids.drag().is_some() {
        if key_pressed(KeyCode::Escape) {
            grids.cancel_drag();
        } else if key_pressed(KeyCode::KeyR) || mouse_pressed(MouseButton::Right) {
            grids.drag_mut().expect("checked").rotate();
        }
    }
    if grids.views().next().is_none() {
        *last_cursor = None;
        return;
    }
    let (Some(viewport), Some(cursor)) = (
        viewport(&cameras),
        windows.iter().next().and_then(Window::cursor_position),
    ) else {
        return;
    };
    let layout = current_layout(
        &grids,
        viewport,
        reserved_top(panels.as_deref(), hud.as_deref(), viewport),
    );
    let hit = hit_at(&grids, &layout, cursor);
    if scrolled != 0.0
        && let Some(window) = window_under(&layout, cursor)
    {
        let grid = window.grid.clone();
        let rows = scrolled.round() as i64;
        let row = (i64::from(grids.scroll(&grid)) - rows).max(0) as u32;
        if row != grids.scroll(&grid) {
            grids
                .set_scroll(&grid, row)
                .expect("a laid out window is shown");
        }
    }
    if mouse_pressed(MouseButton::Left)
        && grids.drag().is_none()
        && let Some(PointerHit {
            grid,
            item: Some((item, grab)),
            ..
        }) = &hit
    {
        grids
            .start_drag(grid, item, *grab)
            .expect("the item under the pointer is shown");
    }
    let moved = *last_cursor != Some(cursor);
    *last_cursor = Some(cursor);
    if grids.drag().is_some() {
        let drag = grids.drag().expect("checked").clone();
        let target = target_for(&grids, &drag, hit.as_ref());
        // Only a moving (or pressing) pointer picks the target: a script's target stays under a
        // still mouse.
        if (moved || mouse_pressed(MouseButton::Left)) && drag.target != target {
            grids.drag_mut().expect("checked").target = target;
        }
        if mouse_released(MouseButton::Left) {
            drop_dragged(&mut grids, &mut pending);
        }
        if grids.hovered().is_some() {
            grids.set_hovered(None);
        }
        return;
    }
    if moved {
        let hovered = hit.and_then(|hit| hit.item.map(|(item, _)| (hit.grid, item)));
        let current = grids.hovered().map(|(g, i)| (g.to_string(), i.to_string()));
        if current != hovered {
            grids.set_hovered(hovered);
        }
    }
}

/// Runs the queued drops' command lines.
fn run_pending_drops(world: &mut World) {
    let lines = std::mem::take(&mut world.resource_mut::<PendingGridDrops>().0);
    for line in lines {
        dispatch_command_line(world, "grid drop", &line);
    }
}

fn view_or_error<'a>(
    grids: &'a UiGrids,
    args: &CommandArgs,
    grid: &str,
) -> Result<&'a UiGridView, retreat_engine_api::CommandError> {
    grids
        .view(grid)
        .ok_or_else(|| args.failure(grids.unknown(grid)))
}

/// `ui.list_grids`: logs every shown window and its items.
fn list_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let grids = world.resource::<UiGrids>();
    for (id, view) in grids.views() {
        let items: Vec<String> = view
            .items
            .iter()
            .map(|item| {
                format!(
                    "{} at ({}, {}){}",
                    item.id,
                    item.x,
                    item.y,
                    if item.rotated { " rotated" } else { "" }
                )
            })
            .collect();
        info!(
            "ui grid `{id}` `{}`: {}x{} cells, first row {}, items [{}]",
            view.title,
            view.columns,
            view.rows,
            grids.scroll(id),
            items.join(", ")
        );
    }
    Ok(CommandOutcome::Continue)
}

/// `ui.scroll_grid <grid> <first row>` (kept within the window's rows).
fn scroll_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let grid: String = args.parse(0, "grid")?;
    let row: u32 = args.parse(1, "first row")?;
    world
        .resource_mut::<UiGrids>()
        .set_scroll(&grid, row)
        .map_err(|problem| args.failure(problem))?;
    Ok(CommandOutcome::Continue)
}

/// `ui.drag_start <grid> <item>`: takes the item as a press on its top-left cell would.
fn drag_start_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let grid: String = args.parse(0, "grid")?;
    let item: String = args.parse(1, "item")?;
    world
        .resource_mut::<UiGrids>()
        .start_drag(&grid, &item, UVec2::ZERO)
        .map_err(|problem| args.failure(problem))?;
    Ok(CommandOutcome::Continue)
}

fn no_drag(args: &CommandArgs) -> retreat_engine_api::CommandError {
    args.failure("nothing is being dragged; start with `ui.drag_start <grid> <item>`")
}

/// `ui.drag_rotate`: turns the dragged item 90 degrees.
fn drag_rotate_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let mut grids = world.resource_mut::<UiGrids>();
    grids.drag_mut().ok_or_else(|| no_drag(args))?.rotate();
    Ok(CommandOutcome::Continue)
}

/// Sets the drag target to `(x, y)` of window `grid` from arguments 0 to 2.
fn set_target(world: &mut World, args: &CommandArgs) -> CommandResult {
    let grid: String = args.parse(0, "grid")?;
    let x: u32 = args.parse(1, "x")?;
    let y: u32 = args.parse(2, "y")?;
    let mut grids = world.resource_mut::<UiGrids>();
    view_or_error(&grids, args, &grid)?;
    grids.drag_mut().ok_or_else(|| no_drag(args))?.target = Some((grid, UVec2::new(x, y)));
    Ok(CommandOutcome::Continue)
}

/// `ui.drag_over <grid> <x> <y>`: the dragged item would land with its top-left at `(x, y)` (a
/// slot window: slot `x`); the preview shows whether it fits.
fn drag_over_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    set_target(world, args)
}

/// `ui.drag_drop [<grid> <x> <y>]`: drops the dragged item at its target (or there); the window's
/// drop command runs on this frame's `Update`.
fn drag_drop_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    match args.rest(0).len() {
        0 => {}
        3 => {
            set_target(world, args)?;
        }
        _ => return Err(args.failure("expected no arguments, or <grid> <x> <y>")),
    }
    let has_target = world
        .resource::<UiGrids>()
        .drag()
        .ok_or_else(|| no_drag(args))?
        .target
        .is_some();
    if !has_target {
        return Err(
            args.failure("the dragged item is over no window; use `ui.drag_over <grid> <x> <y>`")
        );
    }
    world.resource_scope(|world, mut grids: Mut<UiGrids>| {
        drop_dragged(&mut grids, &mut world.resource_mut::<PendingGridDrops>());
    });
    Ok(CommandOutcome::Continue)
}

/// `ui.drag_cancel`.
fn drag_cancel_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    world
        .resource_mut::<UiGrids>()
        .cancel_drag()
        .ok_or_else(|| no_drag(args))?;
    Ok(CommandOutcome::Continue)
}

/// `ui.hover_item <grid> <item>`: shows its tooltip as the pointer over it would.
fn hover_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let grid: String = args.parse(0, "grid")?;
    let item: String = args.parse(1, "item")?;
    let mut grids = world.resource_mut::<UiGrids>();
    view_or_error(&grids, args, &grid)?
        .item(&item)
        .ok_or_else(|| args.failure(format!("window `{grid}` shows no item `{item}`")))?;
    grids.set_hovered(Some((grid, item)));
    Ok(CommandOutcome::Continue)
}

/// `ui.unhover`.
fn unhover_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    world.resource_mut::<UiGrids>().set_hovered(None);
    Ok(CommandOutcome::Continue)
}

/// `assert.ui_grid_item <grid> <item> <x> <y> [upright|rotated]`.
fn assert_item_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let count = args.rest(0).len();
    if count != 4 && count != 5 {
        return Err(args.failure("expected <grid> <item> <x> <y> [upright|rotated]"));
    }
    let grid: String = args.parse(0, "grid")?;
    let id: String = args.parse(1, "item")?;
    let x: u32 = args.parse(2, "x")?;
    let y: u32 = args.parse(3, "y")?;
    let rotated = match args.rest(4).first().map(String::as_str) {
        None => None,
        Some("upright") => Some(false),
        Some("rotated") => Some(true),
        Some(other) => return Err(args.failure(format!("`{other}` is not upright or rotated"))),
    };
    let grids = world.resource::<UiGrids>();
    let view = view_or_error(grids, args, &grid)?;
    let item = view
        .item(&id)
        .ok_or_else(|| args.failure(format!("window `{grid}` shows no item `{id}`")))?;
    if item.x != x || item.y != y || rotated.is_some_and(|rotated| rotated != item.rotated) {
        return Err(args.failure(format!(
            "item `{id}` of `{grid}` is at ({}, {}) {}, expected ({x}, {y}){}",
            item.x,
            item.y,
            if item.rotated { "rotated" } else { "upright" },
            rotated.map_or("", |r| if r { " rotated" } else { " upright" })
        )));
    }
    Ok(CommandOutcome::Continue)
}

/// `assert.ui_grid_absent <grid> <item>`.
fn assert_absent_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let grid: String = args.parse(0, "grid")?;
    let id: String = args.parse(1, "item")?;
    let grids = world.resource::<UiGrids>();
    if let Some(item) = view_or_error(grids, args, &grid)?.item(&id) {
        return Err(args.failure(format!(
            "window `{grid}` shows item `{id}` at ({}, {})",
            item.x, item.y
        )));
    }
    Ok(CommandOutcome::Continue)
}

/// `assert.ui_grid_count <grid> <items>`.
fn assert_count_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let grid: String = args.parse(0, "grid")?;
    let expected: usize = args.parse(1, "items")?;
    let grids = world.resource::<UiGrids>();
    let actual = view_or_error(grids, args, &grid)?.items.len();
    if actual != expected {
        return Err(args.failure(format!(
            "window `{grid}` shows {actual} items, expected {expected}"
        )));
    }
    Ok(CommandOutcome::Continue)
}

fn assert_shown_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let grid: String = args.parse(0, "grid")?;
    view_or_error(world.resource::<UiGrids>(), args, &grid)?;
    Ok(CommandOutcome::Continue)
}

fn assert_hidden_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let grid: String = args.parse(0, "grid")?;
    if world.resource::<UiGrids>().view(&grid).is_some() {
        return Err(args.failure(format!("grid window `{grid}` is shown")));
    }
    Ok(CommandOutcome::Continue)
}

/// `assert.ui_grid_scroll <grid> <row>`.
fn assert_scroll_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let grid: String = args.parse(0, "grid")?;
    let expected: u32 = args.parse(1, "row")?;
    let grids = world.resource::<UiGrids>();
    view_or_error(grids, args, &grid)?;
    let actual = grids.scroll(&grid);
    if actual != expected {
        return Err(args.failure(format!(
            "window `{grid}` shows row {actual} first, expected {expected}"
        )));
    }
    Ok(CommandOutcome::Continue)
}

/// `assert.ui_tooltip <text...>`: the hovered item's tooltip has a line that is exactly this text.
fn assert_tooltip_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let expected = args.rest(0).join(" ");
    if expected.is_empty() {
        return Err(args.failure("expected the text of a tooltip line"));
    }
    let grids = world.resource::<UiGrids>();
    let Some((grid, id)) = grids.hovered() else {
        return Err(args.failure("no item is hovered; use `ui.hover_item <grid> <item>`"));
    };
    let lines: Vec<&str> = grids
        .view(grid)
        .and_then(|view| view.item(id))
        .map(|item| item.tooltip.iter().map(|line| line.text.as_str()).collect())
        .unwrap_or_default();
    if lines.contains(&expected.as_str()) {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "the tooltip of `{id}` has lines {lines:?}, none is `{expected}`"
        )))
    }
}

/// `assert.ui_dragging <item|nothing>`.
fn assert_dragging_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let expected: String = args.parse(0, "item")?;
    let actual = world
        .resource::<UiGrids>()
        .drag()
        .map_or("nothing".to_string(), |drag| drag.item.clone());
    if actual != expected {
        return Err(args.failure(format!("dragging {actual}, expected {expected}")));
    }
    Ok(CommandOutcome::Continue)
}

/// `assert.ui_images_loaded`: every icon the windows use has loaded (fails naming a failed load).
/// Whether lines drawn in `line` between cells of `cell` show the same over any scene: the line
/// color is opaque and differs clearly from the cells. Pure.
fn grid_lines_always_show(line: Color, cell: Color) -> bool {
    let (line, cell) = (line.to_srgba(), cell.to_srgba());
    let brightness = |c: Srgba| (c.red + c.green + c.blue) / 3.0;
    line.alpha >= 1.0 && cell.alpha >= 1.0 && (brightness(line) - brightness(cell)).abs() >= 0.1
}

/// `assert.ui_grid_lines <grid>`: the drawn cell area of a cell window has an opaque line color
/// that differs from its cells, so every row shows its lines whatever is behind the window.
fn assert_grid_lines_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let grid: String = args.parse(0, "grid")?;
    let view = view_or_error(world.resource::<UiGrids>(), args, &grid)?;
    if !matches!(view.shape, UiGridShape::Cells) {
        return Err(args.failure(format!("`{grid}` has slots, not cells: it draws no lines")));
    }
    let name = format!("ui/grid/{grid}/cells");
    let background = world
        .query::<(&Name, &BackgroundColor)>()
        .iter(world)
        .find(|(found, _)| found.as_str() == name)
        .map(|(_, background)| background.0)
        .ok_or_else(|| {
            args.failure(format!(
                "`{grid}` is not drawn yet (no `{name}`; windows are built only where there is a                  camera: wait a frame, and do not use --no-render)"
            ))
        })?;
    if grid_lines_always_show(background, CELL_COLOR) {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "the lines of `{grid}` are {background:?} on cells {CELL_COLOR:?}: not opaque or too              close to the cells, so they vanish over some scenes"
        )))
    }
}

fn assert_images_loaded_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let Some(server) = world.get_resource::<AssetServer>() else {
        return Ok(CommandOutcome::Continue);
    };
    let mut waiting = Vec::new();
    for (path, handle) in &world.resource::<GridImages>().0 {
        match server.get_load_state(handle.id()) {
            Some(bevy::asset::LoadState::Loaded) => {}
            Some(bevy::asset::LoadState::Failed(error)) => {
                return Err(args.failure(format!("icon `{path}` failed to load: {error}")));
            }
            _ => waiting.push(path.clone()),
        }
    }
    if waiting.is_empty() {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!("icons still loading: {waiting:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_lines_show_over_any_scene_only_when_opaque_and_distinct() {
        assert!(grid_lines_always_show(GRID_LINE_COLOR, CELL_COLOR));
        // The old look: the see-through window color in the gaps.
        assert!(!grid_lines_always_show(WINDOW_COLOR, CELL_COLOR));
        assert!(!grid_lines_always_show(CELL_COLOR, CELL_COLOR));
    }
}
