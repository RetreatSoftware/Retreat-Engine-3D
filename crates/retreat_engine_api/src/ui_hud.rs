//! On-screen readouts a module changes while the game runs: lines of text and a crosshair. The
//! engine draws them (`retreat_engine/src/ui_hud.rs`).
//!
//! A module declares each widget with [`AppHudExt::register_hud_widget`] (an id, a place, a kind)
//! and then only changes the [`UiHud`] resource: [`UiHud::set_text`], [`UiHud::set_visible`]. Set
//! them in an `Update` system from game state (a rewind's replayed ticks never show).
//!
//! **Layout is computed from what is shown** ([`compute_hud_layout`], pure): widgets at one anchor
//! stack in id order, hidden ones and text widgets with no text leave no gap, button panels at a top
//! corner push the text below them, a crosshair sits at the exact center and centered text goes below
//! it.
//!
//! **Looks** ([`HudStyle`], changed with [`UiHud::set_style`]): a color for the text or crosshair, and
//! optionally a background panel behind text. A panel is as wide as its longest line (plus padding).
//! Widgets next to each other at one anchor with the same panel color share one panel, so a table such
//! as a scoreboard is one box; give two neighbors different panel colors to keep them apart.
//!
//! Commands: `ui.set_text <widget> <text...>`, `ui.show_widget|hide_widget <widget>`,
//! `ui.set_text_color <widget> <r> <g> <b>`, `ui.set_panel <widget> <r> <g> <b> <a>`,
//! `ui.clear_panel <widget>` (sRGB numbers 0 to 1), `assert.ui_text <widget> <text...>`,
//! `assert.ui_text_color <widget> <r> <g> <b>`, `assert.ui_panel <widget> <r> <g> <b> <a>`.

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::{PANEL_GAP, PANEL_MARGIN, PanelRect};

/// Width of a text widget's box (narrower in a narrow view). Text is aligned to its anchor side.
pub const HUD_TEXT_WIDTH: f32 = 420.0;
/// A text box is this many times its font size high.
pub const HUD_LINE_HEIGHT: f32 = 1.3;
/// Space between a panel's edge and its text: left and right, and top and bottom.
pub const HUD_PANEL_PADDING: Vec2 = Vec2::new(10.0, 4.0);

/// Where a widget sits in the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudAnchor {
    TopLeft,
    TopCenter,
    TopRight,
    Center,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// What a widget is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HudKind {
    /// One line of text, empty until [`UiHud::set_text`].
    Text { font_size: f32 },
    /// A cross `size` pixels wide at the center of the view (its anchor is ignored).
    Crosshair { size: f32 },
}

/// What a module declares for one widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudWidget {
    /// Unique id, one word (`health`).
    pub id: &'static str,
    pub anchor: HudAnchor,
    pub kind: HudKind,
    /// Whether it is shown from the start.
    pub visible: bool,
}

/// How a widget looks; white without a panel unless a module changes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudStyle {
    /// Color of the text or the crosshair.
    pub color: Color,
    /// Background behind a text widget (usually translucent). A crosshair has none.
    pub panel: Option<Color>,
}

impl Default for HudStyle {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            panel: None,
        }
    }
}

/// A widget and what can change about it.
#[derive(Clone, Debug, PartialEq)]
pub struct HudState {
    pub widget: HudWidget,
    pub text: String,
    pub visible: bool,
    pub style: HudStyle,
}

impl HudState {
    /// Whether it takes a place on screen: visible, and a text widget has text.
    pub fn is_drawn(&self) -> bool {
        self.visible
            && (matches!(self.widget.kind, HudKind::Crosshair { .. }) || !self.text.is_empty())
    }
}

/// Every widget of the app (a resource).
#[derive(Resource, Default, Debug)]
pub struct UiHud {
    widgets: BTreeMap<&'static str, HudState>,
}

impl UiHud {
    /// # Panics
    /// If the id is empty, has spaces or is registered twice.
    pub fn register(&mut self, widget: HudWidget) {
        assert!(
            !widget.id.is_empty() && !widget.id.contains(char::is_whitespace),
            "UiHud::register: invalid widget id `{}`; an id is one word like `health`",
            widget.id
        );
        let state = HudState {
            widget,
            text: String::new(),
            visible: widget.visible,
            style: HudStyle::default(),
        };
        assert!(
            self.widgets.insert(widget.id, state).is_none(),
            "UiHud::register: widget `{}` is registered twice; widget ids must be unique",
            widget.id
        );
    }

    /// Every widget, in id order.
    pub fn widgets(&self) -> impl Iterator<Item = &HudState> {
        self.widgets.values()
    }

    pub fn widget(&self, id: &str) -> Option<&HudState> {
        self.widgets.get(id)
    }

    /// Whether setting this text would change anything (so a system can avoid marking the
    /// resource changed every frame).
    pub fn has_text(&self, id: &str, text: &str) -> bool {
        self.widget(id).is_some_and(|state| state.text == text)
    }

    pub fn set_text(&mut self, id: &str, text: impl Into<String>) -> Result<(), String> {
        self.state_mut(id)?.text = text.into();
        Ok(())
    }

    pub fn set_visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
        self.state_mut(id)?.visible = visible;
        Ok(())
    }

    /// Like [`UiHud::has_text`], for the looks.
    pub fn has_style(&self, id: &str, style: HudStyle) -> bool {
        self.widget(id).is_some_and(|state| state.style == style)
    }

    pub fn set_style(&mut self, id: &str, style: HudStyle) -> Result<(), String> {
        self.state_mut(id)?.style = style;
        Ok(())
    }

    fn state_mut(&mut self, id: &str) -> Result<&mut HudState, String> {
        let ids: Vec<&str> = self.widgets.keys().copied().collect();
        self.widgets
            .get_mut(id)
            .ok_or_else(|| format!("no HUD widget `{id}`; widgets: {}", ids.join(", ")))
    }
}

/// How much height the button panels take at the top corners, margin excluded.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudOccupied {
    pub top_left: f32,
    pub top_right: f32,
}

/// Widgets drawn together: one crosshair, one text line, or the lines sharing a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct HudBlock {
    pub anchor: HudAnchor,
    /// The space the block takes. Text blocks are [`HUD_TEXT_WIDTH`] wide boxes; the panel drawn in
    /// one is only as wide as its text, on the anchor's side.
    pub rect: PanelRect,
    pub panel: Option<Color>,
    /// Each widget's line inside `rect`, from top to bottom.
    pub lines: Vec<(&'static str, PanelRect)>,
}

/// Text widgets that go into one block.
struct TextGroup {
    anchor: HudAnchor,
    panel: Option<Color>,
    /// (widget id, line height), in id order.
    lines: Vec<(&'static str, f32)>,
}

/// Where every drawn widget ([`HudState::is_drawn`]) goes, in blocks. Pure: from the widgets (in id
/// order), the view size and the space button panels take.
pub fn compute_hud_layout(
    widgets: &[&HudState],
    viewport: Vec2,
    occupied: HudOccupied,
) -> Vec<HudBlock> {
    let width = HUD_TEXT_WIDTH.min((viewport.x - 2.0 * PANEL_MARGIN).max(0.0));
    let mut crosshair = 0.0f32;
    let mut blocks = Vec::new();
    let mut groups: Vec<TextGroup> = Vec::new();
    for state in widgets.iter().filter(|state| state.is_drawn()) {
        let id = state.widget.id;
        match state.widget.kind {
            HudKind::Crosshair { size } => {
                crosshair = crosshair.max(size);
                let corner = (viewport - Vec2::splat(size)) / 2.0;
                let rect = PanelRect {
                    left: corner.x,
                    top: corner.y,
                    width: size,
                    height: size,
                };
                blocks.push(HudBlock {
                    anchor: HudAnchor::Center,
                    rect,
                    panel: None,
                    lines: vec![(id, rect)],
                });
            }
            HudKind::Text { font_size } => {
                let (anchor, panel) = (state.widget.anchor, state.style.panel);
                let line = (id, font_size * HUD_LINE_HEIGHT);
                let neighbor = groups.iter_mut().rev().find(|group| group.anchor == anchor);
                match neighbor {
                    Some(group) if panel.is_some() && group.panel == panel => {
                        group.lines.push(line)
                    }
                    _ => groups.push(TextGroup {
                        anchor,
                        panel,
                        lines: vec![line],
                    }),
                }
            }
        }
    }
    // The next free y of each anchor: downwards from the top, upwards from the bottom.
    let below = |taken: f32| PANEL_MARGIN + if taken > 0.0 { taken + PANEL_GAP } else { 0.0 };
    let mut top_left = below(occupied.top_left);
    let mut top_center = PANEL_MARGIN;
    let mut top_right = below(occupied.top_right);
    let mut center = viewport.y / 2.0
        + if crosshair > 0.0 {
            crosshair / 2.0 + PANEL_GAP
        } else {
            0.0
        };
    let mut bottom_left = viewport.y - PANEL_MARGIN;
    let mut bottom_center = bottom_left;
    let mut bottom_right = bottom_left;
    let left = PANEL_MARGIN;
    let middle = (viewport.x - width) / 2.0;
    let right = viewport.x - PANEL_MARGIN - width;
    for group in groups {
        let TextGroup {
            anchor,
            panel,
            mut lines,
        } = group;
        let padding = if panel.is_some() {
            HUD_PANEL_PADDING.y
        } else {
            0.0
        };
        let tall = lines.iter().map(|(_, height)| height).sum::<f32>() + 2.0 * padding;
        let down = |cursor: &mut f32| {
            let top = *cursor;
            *cursor += tall + PANEL_GAP;
            top
        };
        let up = |cursor: &mut f32| {
            *cursor -= tall;
            let top = *cursor;
            *cursor -= PANEL_GAP;
            top
        };
        let (x, y) = match anchor {
            HudAnchor::TopLeft => (left, down(&mut top_left)),
            HudAnchor::TopCenter => (middle, down(&mut top_center)),
            HudAnchor::TopRight => (right, down(&mut top_right)),
            HudAnchor::Center => (middle, down(&mut center)),
            HudAnchor::BottomLeft => (left, up(&mut bottom_left)),
            HudAnchor::BottomCenter => (middle, up(&mut bottom_center)),
            HudAnchor::BottomRight => (right, up(&mut bottom_right)),
        };
        // At the bottom the first id sits lowest, as single lines do.
        if matches!(
            anchor,
            HudAnchor::BottomLeft | HudAnchor::BottomCenter | HudAnchor::BottomRight
        ) {
            lines.reverse();
        }
        let mut line_top = y + padding;
        let lines = lines
            .into_iter()
            .map(|(id, height)| {
                let rect = PanelRect {
                    left: x,
                    top: line_top,
                    width,
                    height,
                };
                line_top += height;
                (id, rect)
            })
            .collect();
        let rect = PanelRect {
            left: x,
            top: y,
            width,
            height: tall,
        };
        blocks.push(HudBlock {
            anchor,
            rect,
            panel,
            lines,
        });
    }
    blocks
}

/// Adds `register_hud_widget` to Bevy's `App`.
pub trait AppHudExt {
    fn register_hud_widget(&mut self, widget: HudWidget) -> &mut Self;
}

impl AppHudExt for App {
    fn register_hud_widget(&mut self, widget: HudWidget) -> &mut Self {
        self.init_resource::<UiHud>();
        self.world_mut().resource_mut::<UiHud>().register(widget);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: Vec2 = Vec2::new(1280.0, 720.0);

    fn text(id: &'static str, anchor: HudAnchor) -> HudState {
        HudState {
            widget: HudWidget {
                id,
                anchor,
                kind: HudKind::Text { font_size: 20.0 },
                visible: true,
            },
            text: id.to_string(),
            visible: true,
            style: HudStyle::default(),
        }
    }

    fn paneled(id: &'static str, anchor: HudAnchor, shade: f32) -> HudState {
        let mut state = text(id, anchor);
        state.style.panel = Some(Color::srgba(shade, shade, shade, 0.5));
        state
    }

    fn cross() -> HudState {
        let mut state = text("cross", HudAnchor::TopLeft);
        state.widget.kind = HudKind::Crosshair { size: 24.0 };
        state.text.clear();
        state
    }

    fn layout(widgets: &[HudState], viewport: Vec2, occupied: HudOccupied) -> Vec<HudBlock> {
        compute_hud_layout(&widgets.iter().collect::<Vec<_>>(), viewport, occupied)
    }

    fn rect(blocks: &[HudBlock], id: &str) -> PanelRect {
        blocks
            .iter()
            .flat_map(|block| &block.lines)
            .find(|(found, _)| *found == id)
            .unwrap()
            .1
    }

    #[test]
    fn the_crosshair_is_centered_and_center_text_goes_below_it() {
        let none = HudOccupied::default();
        let blocks = layout(&[cross(), text("note", HudAnchor::Center)], VIEW, none);
        let cross = rect(&blocks, "cross");
        assert_eq!((cross.left, cross.top), (628.0, 348.0));
        let note = rect(&blocks, "note");
        assert!(note.top >= cross.top + cross.height);
        assert_eq!(note.left + note.width / 2.0, 640.0);
        let alone = layout(&[text("note", HudAnchor::Center)], VIEW, none);
        assert_eq!(
            rect(&alone, "note").top,
            360.0,
            "no gap without a crosshair"
        );
    }

    #[test]
    fn widgets_stack_from_their_corner_and_panels_push_them_down() {
        let widgets = [
            text("a", HudAnchor::BottomLeft),
            text("b", HudAnchor::BottomLeft),
            text("c", HudAnchor::TopRight),
        ];
        let occupied = HudOccupied {
            top_left: 0.0,
            top_right: 36.0,
        };
        let blocks = layout(&widgets, VIEW, occupied);
        let (a, b) = (rect(&blocks, "a"), rect(&blocks, "b"));
        assert_eq!(a.top + a.height, 720.0 - PANEL_MARGIN);
        assert_eq!(b.top + b.height, a.top - PANEL_GAP);
        let c = rect(&blocks, "c");
        assert_eq!(c.top, PANEL_MARGIN + 36.0 + PANEL_GAP);
        assert_eq!(c.left + c.width, 1280.0 - PANEL_MARGIN);
        // Without `a`, `b` takes its place.
        let blocks = layout(&widgets[1..], VIEW, occupied);
        assert_eq!(rect(&blocks, "b").top, a.top);
    }

    #[test]
    fn hidden_and_empty_widgets_take_no_space() {
        let mut empty = text("a", HudAnchor::TopLeft);
        empty.text.clear();
        let mut hidden = text("b", HudAnchor::TopLeft);
        hidden.visible = false;
        let blocks = layout(
            &[empty, hidden, text("c", HudAnchor::TopLeft)],
            VIEW,
            HudOccupied::default(),
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(rect(&blocks, "c").top, PANEL_MARGIN);
    }

    #[test]
    fn neighbors_with_the_same_panel_share_one_block_with_padding() {
        let widgets = [
            paneled("a", HudAnchor::TopLeft, 0.0),
            // Another anchor in between does not split the run.
            paneled("b", HudAnchor::TopRight, 0.0),
            paneled("c", HudAnchor::TopLeft, 0.0),
            paneled("d", HudAnchor::TopLeft, 0.5),
            text("e", HudAnchor::TopLeft),
            text("f", HudAnchor::TopLeft),
        ];
        let blocks = layout(&widgets, VIEW, HudOccupied::default());
        let ids: Vec<Vec<&str>> = blocks
            .iter()
            .map(|block| block.lines.iter().map(|line| line.0).collect())
            .collect();
        assert_eq!(
            ids,
            [vec!["a", "c"], vec!["b"], vec!["d"], vec!["e"], vec!["f"]]
        );
        let shared = &blocks[0];
        let (a, c) = (rect(&blocks, "a"), rect(&blocks, "c"));
        assert_eq!(a.top, shared.rect.top + HUD_PANEL_PADDING.y);
        assert_eq!(c.top, a.top + a.height, "no gap inside a panel");
        assert_eq!(
            shared.rect.height,
            a.height + c.height + 2.0 * HUD_PANEL_PADDING.y
        );
        assert_eq!(
            blocks[2].rect.top,
            shared.rect.top + shared.rect.height + PANEL_GAP
        );
        assert_eq!(blocks[3].rect.height, 20.0 * HUD_LINE_HEIGHT);
    }

    #[test]
    fn a_bottom_panel_keeps_the_first_id_lowest() {
        let widgets = [
            paneled("a", HudAnchor::BottomRight, 0.0),
            paneled("b", HudAnchor::BottomRight, 0.0),
        ];
        let blocks = layout(&widgets, VIEW, HudOccupied::default());
        assert_eq!(blocks.len(), 1);
        assert!(rect(&blocks, "b").top < rect(&blocks, "a").top);
        let bottom = blocks[0].rect.top + blocks[0].rect.height;
        assert_eq!(bottom, 720.0 - PANEL_MARGIN);
    }

    #[test]
    fn a_narrow_view_shrinks_text_boxes() {
        let blocks = layout(
            &[text("a", HudAnchor::TopCenter)],
            Vec2::new(300.0, 200.0),
            HudOccupied::default(),
        );
        assert_eq!(rect(&blocks, "a").width, 300.0 - 2.0 * PANEL_MARGIN);
    }
}
