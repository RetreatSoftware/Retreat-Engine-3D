//! On-screen readouts a module changes while the game runs: lines of text and a crosshair. The
//! engine draws them (`retreat_engine/src/ui_hud.rs`).
//!
//! A module declares each widget with [`AppHudExt::register_hud_widget`] (an id, a place, a kind)
//! and then only changes the [`UiHud`] resource: [`UiHud::set_text`], [`UiHud::set_visible`]. Set
//! them in an `Update` system from game state (a rewind's replayed ticks never show).
//!
//! **Layout is computed from what is shown** ([`compute_hud_layout`], pure): widgets at one anchor
//! stack in id order, hidden ones leave no gap, button panels at a top corner push the text below
//! them, a crosshair sits at the exact center and centered text goes below it.
//!
//! Commands: `ui.set_text <widget> <text...>`, `ui.show_widget|hide_widget <widget>`,
//! `assert.ui_text <widget> <text...>`.

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::{PANEL_GAP, PANEL_MARGIN, PanelRect};

/// Width of a text widget's box (narrower in a narrow view). Text is aligned to its anchor side.
pub const HUD_TEXT_WIDTH: f32 = 420.0;
/// A text box is this many times its font size high.
pub const HUD_LINE_HEIGHT: f32 = 1.3;

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

/// A widget and what can change about it.
#[derive(Clone, Debug, PartialEq)]
pub struct HudState {
    pub widget: HudWidget,
    pub text: String,
    pub visible: bool,
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

/// Where every **visible** widget goes, in the order given. Pure: from the widgets, the view size
/// and the space panels take.
pub fn compute_hud_layout(
    widgets: &[HudWidget],
    viewport: Vec2,
    occupied: HudOccupied,
) -> Vec<(&'static str, PanelRect)> {
    let width = HUD_TEXT_WIDTH.min((viewport.x - 2.0 * PANEL_MARGIN).max(0.0));
    let height = |kind: HudKind| match kind {
        HudKind::Text { font_size } => font_size * HUD_LINE_HEIGHT,
        HudKind::Crosshair { size } => size,
    };
    let crosshair = widgets
        .iter()
        .filter(|widget| matches!(widget.kind, HudKind::Crosshair { .. }))
        .map(|widget| height(widget.kind))
        .fold(0.0, f32::max);
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
    widgets
        .iter()
        .map(|widget| {
            let tall = height(widget.kind);
            if let HudKind::Crosshair { size } = widget.kind {
                let corner = (viewport - Vec2::splat(size)) / 2.0;
                let rect = PanelRect {
                    left: corner.x,
                    top: corner.y,
                    width: size,
                    height: size,
                };
                return (widget.id, rect);
            }
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
            let (x, y) = match widget.anchor {
                HudAnchor::TopLeft => (left, down(&mut top_left)),
                HudAnchor::TopCenter => (middle, down(&mut top_center)),
                HudAnchor::TopRight => (right, down(&mut top_right)),
                HudAnchor::Center => (middle, down(&mut center)),
                HudAnchor::BottomLeft => (left, up(&mut bottom_left)),
                HudAnchor::BottomCenter => (middle, up(&mut bottom_center)),
                HudAnchor::BottomRight => (right, up(&mut bottom_right)),
            };
            let rect = PanelRect {
                left: x,
                top: y,
                width,
                height: tall,
            };
            (widget.id, rect)
        })
        .collect()
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

    fn text(id: &'static str, anchor: HudAnchor) -> HudWidget {
        HudWidget {
            id,
            anchor,
            kind: HudKind::Text { font_size: 20.0 },
            visible: true,
        }
    }

    fn cross() -> HudWidget {
        HudWidget {
            id: "cross",
            anchor: HudAnchor::TopLeft,
            kind: HudKind::Crosshair { size: 24.0 },
            visible: true,
        }
    }

    fn rect(layout: &[(&str, PanelRect)], id: &str) -> PanelRect {
        layout.iter().find(|(found, _)| *found == id).unwrap().1
    }

    #[test]
    fn the_crosshair_is_centered_and_center_text_goes_below_it() {
        let layout = compute_hud_layout(
            &[cross(), text("note", HudAnchor::Center)],
            VIEW,
            HudOccupied::default(),
        );
        let cross = rect(&layout, "cross");
        assert_eq!((cross.left, cross.top), (628.0, 348.0));
        let note = rect(&layout, "note");
        assert!(note.top >= cross.top + cross.height);
        assert_eq!(note.left + note.width / 2.0, 640.0);
        let alone = compute_hud_layout(
            &[text("note", HudAnchor::Center)],
            VIEW,
            HudOccupied::default(),
        );
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
        let layout = compute_hud_layout(&widgets, VIEW, occupied);
        let (a, b) = (rect(&layout, "a"), rect(&layout, "b"));
        assert_eq!(a.top + a.height, 720.0 - PANEL_MARGIN);
        assert_eq!(b.top + b.height, a.top - PANEL_GAP);
        let c = rect(&layout, "c");
        assert_eq!(c.top, PANEL_MARGIN + 36.0 + PANEL_GAP);
        assert_eq!(c.left + c.width, 1280.0 - PANEL_MARGIN);
        // Without `a`, `b` takes its place.
        let layout = compute_hud_layout(&widgets[1..], VIEW, occupied);
        assert_eq!(rect(&layout, "b").top, a.top);
    }

    #[test]
    fn a_narrow_view_shrinks_text_boxes() {
        let layout = compute_hud_layout(
            &[text("a", HudAnchor::TopCenter)],
            Vec2::new(300.0, 200.0),
            HudOccupied::default(),
        );
        assert_eq!(rect(&layout, "a").width, 300.0 - 2.0 * PANEL_MARGIN);
    }
}
