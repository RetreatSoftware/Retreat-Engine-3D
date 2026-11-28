//! Item grids (crawler plan CE2): windows of cells that hold rectangular items a player drags,
//! rotates and drops, with icons, tooltips and scrolling. The engine draws them and handles the
//! mouse (`retreat_engine/src/ui_grids.rs`); a module only describes what is shown.
//!
//! A module puts one [`UiGridView`] per window into the [`UiGrids`] resource
//! ([`UiGrids::set_view`], [`UiGrids::remove_view`]) from an `Update` system, from its game state.
//! The engine decides nothing about items: a drop runs the view's command line
//! ([`drop_command_line`]: `<drop_command> <item> <x> <y> <0|1>`), and the module decides whether the
//! move is allowed and changes its state; the next view it writes shows the result.
//!
//! **Shapes.** [`UiGridShape::Cells`]: an item covers `size` cells from its top-left cell `(x, y)`
//! (width and height swap when it is rotated). [`UiGridShape::Slots`]: named rectangles (equipment
//! slots) on a canvas of `columns` x `rows` cells; an item sits in a slot (its `x` is the slot index)
//! and is drawn fitted into it, a drop on a slot sends the slot index as `x` and `0` as `y`.
//!
//! **Layout is computed from what is shown** ([`compute_grid_layout`], pure): windows stand side by
//! side in `order` (then id), centered in the view; all windows share one cell size, which shrinks
//! (down to [`MIN_GRID_CELL`]) so they fit. A window with `visible_rows` below `rows` shows that many
//! rows and scrolls (mouse wheel over it, `ui.scroll_grid`).
//!
//! **Dragging** (mouse or commands, the same rules): press on an item starts a drag (the cell of the
//! item that was grabbed stays under the pointer), `R` or the right mouse button rotates it, the
//! pointer over a window picks the target ([`drop_origin`]), release drops (outside every window
//! cancels), `Escape` cancels. The engine previews the target green when the item fits
//! ([`fits_at`]: inside the grid and on no other item; a slot that is empty) and red otherwise; the
//! module still decides. Hovering an item shows its tooltip next to it.
//!
//! Commands: `ui.list_grids`, `ui.scroll_grid <grid> <first row>`, `ui.drag_start <grid> <item>`,
//! `ui.drag_rotate`, `ui.drag_over <grid> <x> <y>`, `ui.drag_drop [<grid> <x> <y>]`,
//! `ui.drag_cancel`, `ui.hover_item <grid> <item>`, `ui.unhover`; asserts
//! `assert.ui_grid_item <grid> <item> <x> <y> [upright|rotated]`, `assert.ui_grid_absent <grid> <item>`,
//! `assert.ui_grid_count <grid> <items>`, `assert.ui_grid_shown|ui_grid_hidden <grid>`,
//! `assert.ui_grid_scroll <grid> <row>`, `assert.ui_tooltip <text...>`, `assert.ui_dragging <item|nothing>`,
//! `assert.ui_images_loaded`.

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::PanelRect;

/// Side of one cell when there is room (pixels).
pub const GRID_CELL: f32 = 48.0;
/// The smallest cell side a crowded view shrinks to.
pub const MIN_GRID_CELL: f32 = 20.0;
/// Space between a window's edge and its cells.
pub const GRID_PADDING: f32 = 8.0;
/// Height of a window's title line.
pub const GRID_TITLE_HEIGHT: f32 = 26.0;
/// Height of a window's footer line (only when it has a footer).
pub const GRID_FOOTER_HEIGHT: f32 = 22.0;
/// Space between windows, and between the windows and the view's edge.
pub const GRID_WINDOW_GAP: f32 = 12.0;
/// Width of the scroll bar of a window that scrolls.
pub const GRID_SCROLLBAR_WIDTH: f32 = 8.0;
/// Width of an item's tooltip box.
pub const TOOLTIP_WIDTH: f32 = 260.0;
/// Height of one tooltip line.
pub const TOOLTIP_LINE_HEIGHT: f32 = 20.0;

/// One line of an item's tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct UiTooltipLine {
    pub text: String,
    pub color: Color,
}

impl UiTooltipLine {
    pub fn new(text: impl Into<String>, color: Color) -> Self {
        Self {
            text: text.into(),
            color,
        }
    }
}

/// An item drawn in a grid window.
#[derive(Clone, Debug, PartialEq)]
pub struct UiGridItem {
    /// Unique among all shown items (it is the first argument of the drop command), one word.
    pub id: String,
    /// Top-left cell ([`UiGridShape::Cells`]) or slot index in `x` ([`UiGridShape::Slots`]).
    pub x: u32,
    pub y: u32,
    /// Width and height in cells, upright.
    pub size: UVec2,
    /// Turned 90 degrees: covers `size.y` x `size.x` cells.
    pub rotated: bool,
    /// Background of its cells.
    pub color: Color,
    /// An icon (PNG in the game's assets), drawn over the background, turned with the item.
    pub image: Option<String>,
    /// Short text drawn on it (a name, a stack count).
    pub label: String,
    /// Lines shown while it is hovered (none: no tooltip).
    pub tooltip: Vec<UiTooltipLine>,
}

impl UiGridItem {
    /// The cells it covers as drawn (width and height swapped when rotated).
    pub fn drawn_size(&self) -> UVec2 {
        drawn_size(self.size, self.rotated)
    }
}

/// The cells an item of `size` covers when `rotated` or not.
pub fn drawn_size(size: UVec2, rotated: bool) -> UVec2 {
    if rotated {
        UVec2::new(size.y, size.x)
    } else {
        size
    }
}

/// A named rectangle of a [`UiGridShape::Slots`] window, in cells.
#[derive(Clone, Debug, PartialEq)]
pub struct UiGridSlot {
    /// Shown in the empty slot (`Head`).
    pub label: String,
    pub x: u32,
    pub y: u32,
    pub size: UVec2,
}

/// How a window's cells are used.
#[derive(Clone, Debug, PartialEq)]
pub enum UiGridShape {
    /// Every cell holds part of an item; items cover several cells.
    Cells,
    /// Items sit in these slots (equipment).
    Slots(Vec<UiGridSlot>),
}

/// One grid window as a module wants it shown.
#[derive(Clone, Debug, PartialEq)]
pub struct UiGridView {
    pub title: String,
    pub columns: u32,
    pub rows: u32,
    /// Rows shown at once; fewer than `rows` makes the window scroll. `0` shows every row.
    pub visible_rows: u32,
    pub shape: UiGridShape,
    pub items: Vec<UiGridItem>,
    /// The command a drop on this window runs: `<drop_command> <item> <x> <y> <0|1>`.
    pub drop_command: String,
    /// Windows stand left to right by `order`, then by id.
    pub order: i32,
    /// A line under the cells (empty: none), e.g. a balance.
    pub footer: String,
}

impl UiGridView {
    /// The rows shown at once.
    pub fn shown_rows(&self) -> u32 {
        if self.visible_rows == 0 || self.visible_rows >= self.rows {
            self.rows
        } else {
            self.visible_rows
        }
    }

    /// Whether the window scrolls.
    pub fn scrolls(&self) -> bool {
        self.shown_rows() < self.rows
    }

    /// The largest first row a scroll may show.
    pub fn max_scroll(&self) -> u32 {
        self.rows - self.shown_rows()
    }

    pub fn item(&self, id: &str) -> Option<&UiGridItem> {
        self.items.iter().find(|item| item.id == id)
    }

    /// Checks what the engine cannot draw (a module bug): an empty window, an item outside it,
    /// ids with spaces or twice.
    pub fn validate(&self) -> Result<(), String> {
        if self.columns == 0 || self.rows == 0 {
            return Err("a grid window needs at least one column and one row".to_string());
        }
        if self.drop_command.trim().is_empty() {
            return Err("a grid window needs a drop command".to_string());
        }
        let mut seen: Vec<&str> = Vec::new();
        for item in &self.items {
            if item.id.is_empty() || item.id.contains(char::is_whitespace) {
                return Err(format!("item id `{}` must be one word", item.id));
            }
            if seen.contains(&item.id.as_str()) {
                return Err(format!("item `{}` is listed twice", item.id));
            }
            seen.push(&item.id);
            if item.size.x == 0 || item.size.y == 0 {
                return Err(format!("item `{}` has an empty size", item.id));
            }
            match &self.shape {
                UiGridShape::Cells => {
                    let size = item.drawn_size();
                    if item.x + size.x > self.columns || item.y + size.y > self.rows {
                        return Err(format!(
                            "item `{}` at ({}, {}) covering {}x{} leaves the {}x{} grid",
                            item.id, item.x, item.y, size.x, size.y, self.columns, self.rows
                        ));
                    }
                }
                UiGridShape::Slots(slots) => {
                    if item.x as usize >= slots.len() {
                        return Err(format!(
                            "item `{}` is in slot {}, but the window has {} slots",
                            item.id,
                            item.x,
                            slots.len()
                        ));
                    }
                }
            }
        }
        if let UiGridShape::Slots(slots) = &self.shape {
            for slot in slots {
                if slot.x + slot.size.x > self.columns || slot.y + slot.size.y > self.rows {
                    return Err(format!("slot `{}` leaves the window", slot.label));
                }
            }
        }
        Ok(())
    }
}

/// The command line a drop of `item` at `(x, y)` runs.
pub fn drop_command_line(view: &UiGridView, item: &str, x: u32, y: u32, rotated: bool) -> String {
    format!("{} {item} {x} {y} {}", view.drop_command, u8::from(rotated))
}

/// Whether an item `item` covering `size` cells fits at `(x, y)` of `view` (the drag preview; the
/// module decides the real move): a cell grid needs the cells inside and free of other items, a
/// slot window needs the slot `x` to exist and hold no other item.
pub fn fits_at(view: &UiGridView, item: &str, size: UVec2, x: u32, y: u32) -> bool {
    match &view.shape {
        UiGridShape::Cells => {
            if x + size.x > view.columns || y + size.y > view.rows {
                return false;
            }
            view.items
                .iter()
                .filter(|other| other.id != item)
                .all(|other| {
                    let other_size = other.drawn_size();
                    x + size.x <= other.x
                        || other.x + other_size.x <= x
                        || y + size.y <= other.y
                        || other.y + other_size.y <= y
                })
        }
        UiGridShape::Slots(slots) => {
            (x as usize) < slots.len()
                && view
                    .items
                    .iter()
                    .all(|other| other.id == item || other.x != x)
        }
    }
}

/// Where a window and its parts are on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct GridWindowRect {
    pub grid: String,
    /// The whole window.
    pub rect: PanelRect,
    pub title: PanelRect,
    /// The shown cells (scrolled rows are outside it).
    pub cells: PanelRect,
    /// The footer line, when the window has a footer.
    pub footer: Option<PanelRect>,
    /// The scroll bar track, when the window scrolls.
    pub scrollbar: Option<PanelRect>,
}

/// Every shown window and the shared cell size.
#[derive(Clone, Debug, PartialEq)]
pub struct GridLayout {
    pub cell: f32,
    pub windows: Vec<GridWindowRect>,
}

impl GridLayout {
    pub fn window(&self, grid: &str) -> Option<&GridWindowRect> {
        self.windows.iter().find(|window| window.grid == grid)
    }
}

fn window_width(view: &UiGridView, cell: f32) -> f32 {
    let bar = if view.scrolls() {
        GRID_SCROLLBAR_WIDTH + GRID_PADDING / 2.0
    } else {
        0.0
    };
    view.columns as f32 * cell + 2.0 * GRID_PADDING + bar
}

fn window_height(view: &UiGridView, cell: f32) -> f32 {
    let footer = if view.footer.is_empty() {
        0.0
    } else {
        GRID_FOOTER_HEIGHT
    };
    GRID_TITLE_HEIGHT + view.shown_rows() as f32 * cell + 2.0 * GRID_PADDING + footer
}

/// Places the windows in `views` (already only the shown ones) in a view of `viewport` logical
/// pixels below `reserved_top` (what the top corners hold: button panels), side by side in `order`
/// then id, centered in the space left, one shared cell size as large as fits (at most
/// [`GRID_CELL`], at least [`MIN_GRID_CELL`]). No windows: an empty layout. Pure.
pub fn compute_grid_layout(
    views: &[(&str, &UiGridView)],
    viewport: Vec2,
    reserved_top: f32,
) -> GridLayout {
    let room_top = reserved_top.clamp(0.0, viewport.y);
    let room_height = viewport.y - room_top;
    let mut ordered: Vec<&(&str, &UiGridView)> = views.iter().collect();
    ordered.sort_by(|a, b| a.1.order.cmp(&b.1.order).then(a.0.cmp(b.0)));
    if ordered.is_empty() {
        return GridLayout {
            cell: GRID_CELL,
            windows: Vec::new(),
        };
    }
    // Width and height are linear in the cell size: solve for the largest that fits.
    let gaps = GRID_WINDOW_GAP * (ordered.len() as f32 + 1.0);
    let fixed_width: f32 = ordered
        .iter()
        .map(|(_, view)| window_width(view, 0.0))
        .sum();
    let columns: f32 = ordered.iter().map(|(_, view)| view.columns as f32).sum();
    let by_width = (viewport.x - gaps - fixed_width) / columns;
    let by_height = ordered
        .iter()
        .map(|(_, view)| {
            (room_height - 2.0 * GRID_WINDOW_GAP - window_height(view, 0.0))
                / view.shown_rows() as f32
        })
        .fold(f32::INFINITY, f32::min);
    let cell = by_width
        .min(by_height)
        .clamp(MIN_GRID_CELL, GRID_CELL)
        .floor();
    let total_width: f32 = ordered
        .iter()
        .map(|(_, view)| window_width(view, cell))
        .sum::<f32>()
        + GRID_WINDOW_GAP * (ordered.len() as f32 - 1.0);
    let mut left = ((viewport.x - total_width) / 2.0)
        .max(GRID_WINDOW_GAP)
        .floor();
    // Tops line up: the tallest window is centered vertically in the room below the panels.
    let tallest = ordered
        .iter()
        .map(|(_, view)| window_height(view, cell))
        .fold(0.0, f32::max);
    let top = (room_top + ((room_height - tallest) / 2.0).max(GRID_WINDOW_GAP)).floor();
    let mut windows = Vec::new();
    for (id, view) in ordered {
        let width = window_width(view, cell);
        let height = window_height(view, cell);
        let rect = PanelRect {
            left,
            top,
            width,
            height,
        };
        let title = PanelRect {
            left: left + GRID_PADDING,
            top,
            width: width - 2.0 * GRID_PADDING,
            height: GRID_TITLE_HEIGHT,
        };
        let cells = PanelRect {
            left: left + GRID_PADDING,
            top: top + GRID_TITLE_HEIGHT + GRID_PADDING,
            width: view.columns as f32 * cell,
            height: view.shown_rows() as f32 * cell,
        };
        let footer = (!view.footer.is_empty()).then(|| PanelRect {
            left: left + GRID_PADDING,
            top: cells.top + cells.height + GRID_PADDING / 2.0,
            width: width - 2.0 * GRID_PADDING,
            height: GRID_FOOTER_HEIGHT,
        });
        let scrollbar = view.scrolls().then(|| PanelRect {
            left: cells.left + cells.width + GRID_PADDING / 2.0,
            top: cells.top,
            width: GRID_SCROLLBAR_WIDTH,
            height: cells.height,
        });
        windows.push(GridWindowRect {
            grid: (*id).to_string(),
            rect,
            title,
            cells,
            footer,
            scrollbar,
        });
        left += width + GRID_WINDOW_GAP;
    }
    GridLayout { cell, windows }
}

/// The rectangle of cells `(x, y)` to `(x + size.x, y + size.y)` of a window scrolled to `scroll`,
/// on screen (it may reach outside the shown cells; the drawing clips it).
pub fn cells_rect(
    window: &GridWindowRect,
    cell: f32,
    scroll: u32,
    x: u32,
    y: u32,
    size: UVec2,
) -> PanelRect {
    PanelRect {
        left: window.cells.left + x as f32 * cell,
        top: window.cells.top + (y as f32 - scroll as f32) * cell,
        width: size.x as f32 * cell,
        height: size.y as f32 * cell,
    }
}

/// Where `item` of `view` is drawn on screen: its cells, or centered in its slot at its own
/// proportions (as large as the slot allows, never larger than its cells).
pub fn item_rect(
    view: &UiGridView,
    window: &GridWindowRect,
    cell: f32,
    scroll: u32,
    item: &UiGridItem,
) -> PanelRect {
    match &view.shape {
        UiGridShape::Cells => cells_rect(window, cell, scroll, item.x, item.y, item.drawn_size()),
        UiGridShape::Slots(slots) => {
            let slot = &slots[item.x as usize];
            let room = cells_rect(window, cell, scroll, slot.x, slot.y, slot.size);
            let size = item.drawn_size().as_vec2() * cell;
            let scale = (room.width / size.x).min(room.height / size.y).min(1.0);
            let (width, height) = (size.x * scale, size.y * scale);
            PanelRect {
                left: room.left + (room.width - width) / 2.0,
                top: room.top + (room.height - height) / 2.0,
                width,
                height,
            }
        }
    }
}

fn contains(rect: PanelRect, point: Vec2) -> bool {
    point.x >= rect.left
        && point.x < rect.left + rect.width
        && point.y >= rect.top
        && point.y < rect.top + rect.height
}

/// The cell of `window` (a grid cell, counting scrolled rows) under `point`, if it is over the
/// shown cells.
pub fn cell_at(window: &GridWindowRect, cell: f32, scroll: u32, point: Vec2) -> Option<UVec2> {
    if !contains(window.cells, point) {
        return None;
    }
    let x = ((point.x - window.cells.left) / cell).floor() as u32;
    let y = ((point.y - window.cells.top) / cell).floor() as u32 + scroll;
    Some(UVec2::new(x, y))
}

/// The slot of a slot window that covers `cell`.
pub fn slot_at(view: &UiGridView, cell: UVec2) -> Option<u32> {
    let UiGridShape::Slots(slots) = &view.shape else {
        return None;
    };
    slots
        .iter()
        .position(|slot| {
            cell.x >= slot.x
                && cell.x < slot.x + slot.size.x
                && cell.y >= slot.y
                && cell.y < slot.y + slot.size.y
        })
        .map(|index| index as u32)
}

/// The item of `view` under `point` (shown cells only).
pub fn item_at<'a>(
    view: &'a UiGridView,
    window: &GridWindowRect,
    cell: f32,
    scroll: u32,
    point: Vec2,
) -> Option<&'a UiGridItem> {
    if !contains(window.cells, point) {
        return None;
    }
    view.items
        .iter()
        .find(|item| contains(item_rect(view, window, cell, scroll, item), point))
}

/// Where a dragged item lands when the pointer is over `cell` of `view`: for cells, the item's
/// top-left cell such that the grabbed cell `grab` (counted from the item's top-left, as drawn)
/// stays under the pointer, kept inside the grid when it can be; for slots, the slot under the
/// pointer and `0`. `None` over a slot window's empty canvas.
pub fn drop_origin(view: &UiGridView, cell: UVec2, grab: UVec2, size: UVec2) -> Option<UVec2> {
    match &view.shape {
        UiGridShape::Cells => {
            let grab = grab.min(size.saturating_sub(UVec2::ONE));
            let mut origin = cell.saturating_sub(grab);
            origin.x = origin.x.min(view.columns.saturating_sub(size.x));
            origin.y = origin.y.min(view.rows.saturating_sub(size.y));
            Some(origin)
        }
        UiGridShape::Slots(_) => slot_at(view, cell).map(|slot| UVec2::new(slot, 0)),
    }
}

/// Where an item's tooltip box goes: right of the item, or left of it when there is no room, kept
/// inside the view. Pure.
pub fn tooltip_rect(item: PanelRect, lines: usize, viewport: Vec2) -> PanelRect {
    let height = lines as f32 * TOOLTIP_LINE_HEIGHT + 2.0 * GRID_PADDING;
    let mut left = item.left + item.width + GRID_PADDING;
    if left + TOOLTIP_WIDTH > viewport.x {
        left = item.left - GRID_PADDING - TOOLTIP_WIDTH;
    }
    let left = left.clamp(0.0, (viewport.x - TOOLTIP_WIDTH).max(0.0));
    let top = item.top.clamp(0.0, (viewport.y - height).max(0.0));
    PanelRect {
        left,
        top,
        width: TOOLTIP_WIDTH,
        height,
    }
}

/// An item being dragged.
#[derive(Clone, Debug, PartialEq)]
pub struct UiGridDrag {
    /// The window it was taken from.
    pub grid: String,
    pub item: String,
    /// Upright size in cells.
    pub size: UVec2,
    /// Turned now (starts as the item was).
    pub rotated: bool,
    /// The grabbed cell, counted from the item's top-left as drawn now.
    pub grab: UVec2,
    /// Where it would land: window and origin (see [`drop_origin`]).
    pub target: Option<(String, UVec2)>,
}

impl UiGridDrag {
    pub fn drawn_size(&self) -> UVec2 {
        drawn_size(self.size, self.rotated)
    }

    /// Turns the item; the grabbed cell turns with it.
    pub fn rotate(&mut self) {
        let size = self.drawn_size();
        // Clockwise: the cell at (x, y) of a w x h item goes to (h - 1 - y, x).
        self.grab = UVec2::new(
            size.y.saturating_sub(1).saturating_sub(self.grab.y),
            self.grab.x,
        );
        self.rotated = !self.rotated;
    }
}

/// Every grid window a module shows, and what the pointer does with them (a resource). Modules
/// write views; the engine writes scroll, drag and hover.
#[derive(Resource, Default, Debug)]
pub struct UiGrids {
    views: BTreeMap<String, UiGridView>,
    scroll: BTreeMap<String, u32>,
    drag: Option<UiGridDrag>,
    hovered: Option<(String, String)>,
}

impl UiGrids {
    /// Shows `view` as window `grid` (or replaces it). Call only when it differs from
    /// [`UiGrids::view`] (a write marks the resource changed, which rebuilds the windows).
    ///
    /// # Panics
    /// If the grid id is not one word, or the view is invalid ([`UiGridView::validate`]): a module bug.
    pub fn set_view(&mut self, grid: &str, view: UiGridView) {
        assert!(
            !grid.is_empty() && !grid.contains(char::is_whitespace),
            "UiGrids::set_view: grid id `{grid}` must be one word"
        );
        if let Err(problem) = view.validate() {
            panic!("UiGrids::set_view: window `{grid}`: {problem}");
        }
        let max = view.max_scroll();
        if let Some(scroll) = self.scroll.get_mut(grid) {
            *scroll = (*scroll).min(max);
        }
        self.views.insert(grid.to_string(), view);
        self.forget_missing();
    }

    /// Hides window `grid` (nothing when it is not shown).
    pub fn remove_view(&mut self, grid: &str) {
        self.views.remove(grid);
        self.scroll.remove(grid);
        self.forget_missing();
    }

    /// A drag or hover of an item that is no longer shown ends.
    fn forget_missing(&mut self) {
        let shown = |views: &BTreeMap<String, UiGridView>, grid: &str, item: &str| {
            views
                .get(grid)
                .is_some_and(|view| view.item(item).is_some())
        };
        if let Some(drag) = &self.drag
            && !shown(&self.views, &drag.grid, &drag.item)
        {
            self.drag = None;
        }
        if let Some((grid, item)) = &self.hovered
            && !shown(&self.views, grid, item)
        {
            self.hovered = None;
        }
    }

    pub fn view(&self, grid: &str) -> Option<&UiGridView> {
        self.views.get(grid)
    }

    /// Every shown window, by id.
    pub fn views(&self) -> impl Iterator<Item = (&str, &UiGridView)> {
        self.views.iter().map(|(id, view)| (id.as_str(), view))
    }

    /// The first row window `grid` shows.
    pub fn scroll(&self, grid: &str) -> u32 {
        self.scroll.get(grid).copied().unwrap_or(0)
    }

    /// Scrolls window `grid` to show `row` first (kept within its rows).
    pub fn set_scroll(&mut self, grid: &str, row: u32) -> Result<(), String> {
        let view = self.views.get(grid).ok_or_else(|| self.unknown(grid))?;
        let row = row.min(view.max_scroll());
        self.scroll.insert(grid.to_string(), row);
        Ok(())
    }

    pub fn drag(&self) -> Option<&UiGridDrag> {
        self.drag.as_ref()
    }

    pub fn drag_mut(&mut self) -> Option<&mut UiGridDrag> {
        self.drag.as_mut()
    }

    /// Starts dragging `item` of `grid`, grabbed at cell `grab` of it.
    pub fn start_drag(&mut self, grid: &str, item: &str, grab: UVec2) -> Result<(), String> {
        let view = self.views.get(grid).ok_or_else(|| self.unknown(grid))?;
        let found = view
            .item(item)
            .ok_or_else(|| format!("window `{grid}` shows no item `{item}`"))?;
        self.drag = Some(UiGridDrag {
            grid: grid.to_string(),
            item: item.to_string(),
            size: found.size,
            rotated: found.rotated,
            grab: grab.min(found.drawn_size().saturating_sub(UVec2::ONE)),
            target: None,
        });
        Ok(())
    }

    /// Ends the drag without a drop.
    pub fn cancel_drag(&mut self) -> Option<UiGridDrag> {
        self.drag.take()
    }

    pub fn hovered(&self) -> Option<(&str, &str)> {
        self.hovered
            .as_ref()
            .map(|(grid, item)| (grid.as_str(), item.as_str()))
    }

    pub fn set_hovered(&mut self, hovered: Option<(String, String)>) {
        self.hovered = hovered;
    }

    /// The error for a window that is not shown, naming those that are.
    pub fn unknown(&self, grid: &str) -> String {
        let shown: Vec<&str> = self.views.keys().map(String::as_str).collect();
        format!("grid window `{grid}` is not shown; shown windows: {shown:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, x: u32, y: u32, w: u32, h: u32) -> UiGridItem {
        UiGridItem {
            id: id.to_string(),
            x,
            y,
            size: UVec2::new(w, h),
            rotated: false,
            color: Color::WHITE,
            image: None,
            label: String::new(),
            tooltip: Vec::new(),
        }
    }

    fn bag(items: Vec<UiGridItem>) -> UiGridView {
        UiGridView {
            title: "Bag".to_string(),
            columns: 6,
            rows: 4,
            visible_rows: 0,
            shape: UiGridShape::Cells,
            items,
            drop_command: "bag.put".to_string(),
            order: 1,
            footer: String::new(),
        }
    }

    fn slots() -> UiGridView {
        UiGridView {
            title: "Equipment".to_string(),
            columns: 4,
            rows: 4,
            visible_rows: 0,
            shape: UiGridShape::Slots(vec![
                UiGridSlot {
                    label: "Head".to_string(),
                    x: 1,
                    y: 0,
                    size: UVec2::new(2, 2),
                },
                UiGridSlot {
                    label: "Hand".to_string(),
                    x: 0,
                    y: 2,
                    size: UVec2::new(2, 2),
                },
            ]),
            items: vec![item("helm", 0, 0, 2, 2)],
            drop_command: "equipment.put".to_string(),
            order: 0,
            footer: String::new(),
        }
    }

    #[test]
    fn an_item_fits_on_free_cells_inside_the_grid_only() {
        let view = bag(vec![item("sword", 0, 0, 1, 3), item("ring", 3, 1, 1, 1)]);
        assert!(fits_at(&view, "new", UVec2::new(2, 2), 1, 0));
        assert!(
            !fits_at(&view, "new", UVec2::new(2, 2), 2, 0),
            "overlaps the ring"
        );
        assert!(
            !fits_at(&view, "new", UVec2::new(2, 2), 5, 0),
            "leaves the grid"
        );
        assert!(
            fits_at(&view, "sword", UVec2::new(1, 3), 0, 1),
            "an item never blocks itself"
        );
    }

    #[test]
    fn a_slot_takes_one_item() {
        let view = slots();
        assert!(
            !fits_at(&view, "other", UVec2::ONE, 0, 0),
            "the head slot holds the helm"
        );
        assert!(fits_at(&view, "helm", UVec2::ONE, 0, 0));
        assert!(fits_at(&view, "other", UVec2::new(1, 3), 1, 0));
        assert!(!fits_at(&view, "other", UVec2::ONE, 2, 0), "no third slot");
    }

    #[test]
    fn rotation_swaps_the_covered_cells_and_turns_the_grab() {
        assert_eq!(drawn_size(UVec2::new(1, 3), true), UVec2::new(3, 1));
        let mut drag = UiGridDrag {
            grid: "bag".to_string(),
            item: "sword".to_string(),
            size: UVec2::new(1, 3),
            rotated: false,
            grab: UVec2::new(0, 2),
            target: None,
        };
        drag.rotate();
        assert_eq!(drag.drawn_size(), UVec2::new(3, 1));
        assert_eq!(
            drag.grab,
            UVec2::new(0, 0),
            "the bottom cell turns to the left end"
        );
        drag.rotate();
        assert_eq!(drag.drawn_size(), UVec2::new(1, 3));
    }

    #[test]
    fn the_grabbed_cell_stays_under_the_pointer_and_the_item_stays_inside() {
        let view = bag(Vec::new());
        let size = UVec2::new(2, 3);
        assert_eq!(
            drop_origin(&view, UVec2::new(3, 2), UVec2::new(1, 1), size),
            Some(UVec2::new(2, 1))
        );
        assert_eq!(
            drop_origin(&view, UVec2::new(0, 0), UVec2::new(1, 2), size),
            Some(UVec2::new(0, 0))
        );
        assert_eq!(
            drop_origin(&view, UVec2::new(5, 3), UVec2::ZERO, size),
            Some(UVec2::new(4, 1))
        );
        let equipment = slots();
        assert_eq!(
            drop_origin(&equipment, UVec2::new(1, 3), UVec2::ZERO, size),
            Some(UVec2::new(1, 0))
        );
        assert_eq!(
            drop_origin(&equipment, UVec2::new(3, 3), UVec2::ZERO, size),
            None
        );
    }

    #[test]
    fn windows_stand_side_by_side_in_order_and_reflow_when_one_hides() {
        let bag_view = bag(Vec::new());
        let equipment = slots();
        let viewport = Vec2::new(1280.0, 720.0);
        let both = compute_grid_layout(
            &[("bag", &bag_view), ("equipment", &equipment)],
            viewport,
            0.0,
        );
        assert_eq!(both.cell, GRID_CELL);
        assert_eq!(both.windows[0].grid, "equipment", "lower order goes left");
        assert!(both.windows[0].rect.left + both.windows[0].rect.width < both.windows[1].rect.left);
        let alone = compute_grid_layout(&[("bag", &bag_view)], viewport, 0.0);
        let window = &alone.windows[0];
        let center = window.rect.left + window.rect.width / 2.0;
        assert!(
            (center - 640.0).abs() <= 1.0,
            "the bag alone is centered, at {center}"
        );
        assert!(compute_grid_layout(&[], viewport, 0.0).windows.is_empty());
    }

    #[test]
    fn windows_stay_below_the_corner_panels_and_shrink_to_fit_there() {
        let bag_view = bag(Vec::new());
        let viewport = Vec2::new(1280.0, 720.0);
        let layout = compute_grid_layout(&[("bag", &bag_view)], viewport, 300.0);
        let window = &layout.windows[0];
        assert!(window.rect.top >= 300.0);
        assert!(window.rect.top + window.rect.height <= 720.0);
        let mut tall = bag(Vec::new());
        tall.rows = 12;
        let squeezed = compute_grid_layout(&[("bag", &tall)], viewport, 300.0);
        assert!(squeezed.cell < GRID_CELL);
        assert!(squeezed.windows[0].rect.top >= 300.0);
    }

    #[test]
    fn a_slot_item_keeps_its_proportions_centered_in_the_slot() {
        let mut view = slots();
        // A 1x3 sword in the 2x3 hand slot (x 0..2, y 2..4): one cell wide, centered.
        view.items = vec![item("sword", 1, 0, 1, 3)];
        let layout = compute_grid_layout(&[("gear", &view)], Vec2::new(1280.0, 720.0), 0.0);
        let window = &layout.windows[0];
        let rect = item_rect(&view, window, layout.cell, 0, &view.items[0]);
        let slot = cells_rect(window, layout.cell, 0, 0, 2, UVec2::new(2, 2));
        assert_eq!(
            rect.height, slot.height,
            "as tall as the 2-high slot allows"
        );
        assert!(
            (rect.width - rect.height / 3.0).abs() < 0.01,
            "a third as wide as tall"
        );
        assert!((rect.left + rect.width / 2.0 - (slot.left + slot.width / 2.0)).abs() < 0.01);
    }

    #[test]
    fn a_narrow_view_shrinks_the_cells() {
        let bag_view = bag(Vec::new());
        let layout = compute_grid_layout(&[("bag", &bag_view)], Vec2::new(200.0, 720.0), 0.0);
        assert!(layout.cell < GRID_CELL && layout.cell >= MIN_GRID_CELL);
        let window = &layout.windows[0];
        assert!(window.rect.left + window.rect.width <= 200.0);
    }

    #[test]
    fn a_window_with_fewer_visible_rows_scrolls_and_hit_tests_count_the_scroll() {
        let mut chest = bag(vec![item("deep", 0, 7, 1, 1)]);
        chest.rows = 10;
        chest.visible_rows = 4;
        assert!(chest.scrolls());
        assert_eq!(chest.max_scroll(), 6);
        let layout = compute_grid_layout(&[("chest", &chest)], Vec2::new(1280.0, 720.0), 0.0);
        let window = &layout.windows[0];
        assert!(window.scrollbar.is_some());
        assert_eq!(window.cells.height, 4.0 * layout.cell);
        let point = Vec2::new(window.cells.left + 1.0, window.cells.top + 1.0);
        assert_eq!(
            cell_at(window, layout.cell, 7, point),
            Some(UVec2::new(0, 7))
        );
        assert_eq!(
            item_at(&chest, window, layout.cell, 7, point).map(|i| i.id.as_str()),
            Some("deep")
        );
        assert!(item_at(&chest, window, layout.cell, 0, point).is_none());
    }

    #[test]
    fn scroll_is_kept_within_the_rows_and_a_removed_item_ends_its_drag() {
        let mut grids = UiGrids::default();
        let mut chest = bag(vec![item("deep", 0, 7, 1, 1)]);
        chest.rows = 10;
        chest.visible_rows = 4;
        grids.set_view("chest", chest.clone());
        grids.set_scroll("chest", 99).unwrap();
        assert_eq!(grids.scroll("chest"), 6);
        grids.start_drag("chest", "deep", UVec2::ZERO).unwrap();
        chest.items.clear();
        grids.set_view("chest", chest);
        assert!(grids.drag().is_none());
        assert!(
            grids
                .set_scroll("nothing", 0)
                .unwrap_err()
                .contains("not shown")
        );
    }

    #[test]
    fn a_tooltip_goes_right_of_the_item_or_left_near_the_edge() {
        let viewport = Vec2::new(1000.0, 600.0);
        let item = PanelRect {
            left: 100.0,
            top: 50.0,
            width: 48.0,
            height: 48.0,
        };
        assert!(tooltip_rect(item, 3, viewport).left > 148.0);
        let edge = PanelRect {
            left: 900.0,
            top: 580.0,
            width: 48.0,
            height: 48.0,
        };
        let rect = tooltip_rect(edge, 3, viewport);
        assert!(rect.left + rect.width <= 900.0);
        assert!(rect.top + rect.height <= 600.0);
    }

    #[test]
    fn invalid_views_name_the_problem() {
        let outside = bag(vec![item("big", 5, 0, 2, 1)]);
        assert!(
            outside
                .validate()
                .unwrap_err()
                .contains("leaves the 6x4 grid")
        );
        let twice = bag(vec![item("a", 0, 0, 1, 1), item("a", 1, 0, 1, 1)]);
        assert!(twice.validate().unwrap_err().contains("twice"));
        assert_eq!(
            drop_command_line(&bag(Vec::new()), "a", 2, 3, true),
            "bag.put a 2 3 1"
        );
    }
}
