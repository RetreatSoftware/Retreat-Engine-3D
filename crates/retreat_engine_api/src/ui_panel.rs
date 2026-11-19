//! UI toolkit for modules: a panel of buttons that run command lines.
//!
//! A module declares a panel with [`AppUiExt::register_panel`]: an id, an optional title, where
//! it sits ([`PanelAnchor`]), which way its buttons run ([`PanelDirection`]) and the buttons
//! ([`UiButtonEntry`]: id, label, command line). The engine draws it (today with `bevy_ui`) and a
//! click runs the entry's command line through the command registry, exactly like a script
//! line, so every button is drivable by scripts (`ui.press <panel> <entry>`) and the browser
//! bridge. A module shows and hides its panel with [`UiPanels::set_visible`] or the commands
//! `ui.show_panel` / `ui.hide_panel`.
//!
//! **Layout is computed from what is shown**, in the pure function [`compute_panel_layout`]:
//! sizes and positions come only from the visible entries and the viewport, so hiding a button
//! re-flows its siblings and a narrow view shrinks the buttons. The sandbox toolbar is a panel
//! built with this same toolkit.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;

/// Space between the view's edge and a panel, in logical pixels.
pub const PANEL_MARGIN: f32 = 12.0;
/// Space between two buttons.
pub const PANEL_GAP: f32 = 8.0;
/// Height of a button (it shrinks in a column that does not fit the view, down to
/// [`MIN_BUTTON_HEIGHT`]).
pub const BUTTON_HEIGHT: f32 = 36.0;
/// Smallest button height of a column squeezed into a short view.
pub const MIN_BUTTON_HEIGHT: f32 = 12.0;
/// Buttons of a row are at most this wide; they shrink when the view is too narrow.
pub const ROW_MAX_BUTTON_WIDTH: f32 = 150.0;
/// Buttons of a column are this wide, or as wide as the view allows.
pub const COLUMN_BUTTON_WIDTH: f32 = 220.0;
/// Height of the title line above the buttons, when a panel has a title.
pub const TITLE_HEIGHT: f32 = 28.0;

/// Where a panel sits in the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAnchor {
    TopLeft,
    TopRight,
    Center,
}

/// Which way a panel's buttons run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelDirection {
    Row,
    Column,
}

/// One button: a stable id (for commands and entity names), the label shown, and the command
/// line it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiButtonEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub command_line: &'static str,
}

/// What a module declares for one panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiPanel {
    /// Unique id, one word (`pause_menu`).
    pub id: &'static str,
    /// Heading above the buttons, if any.
    pub title: Option<&'static str>,
    pub anchor: PanelAnchor,
    pub direction: PanelDirection,
    /// Every button, in display order. Entry ids are unique within the panel.
    pub entries: &'static [UiButtonEntry],
    /// Whether the panel is shown from the start.
    pub visible: bool,
}

/// A rectangle in logical pixels from the view's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelRect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// Where one button goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonRect {
    pub id: &'static str,
    pub rect: PanelRect,
}

/// Where everything of one panel goes. Empty (no buttons, no title) when no entry is visible.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelLayout {
    /// The whole panel: title and buttons.
    pub bounds: PanelRect,
    pub title: Option<PanelRect>,
    pub buttons: Vec<ButtonRect>,
}

/// The entries that are not hidden, in panel order.
pub fn visible_entries(
    entries: &[UiButtonEntry],
    hidden: &BTreeSet<&'static str>,
) -> Vec<UiButtonEntry> {
    entries
        .iter()
        .filter(|entry| !hidden.contains(entry.id))
        .copied()
        .collect()
}

/// Lays a panel out in `viewport` (width, height). Pure: positions come only from the entries
/// passed in and the viewport size, so a hidden entry leaves no gap. A row shares the width
/// equally (at most [`ROW_MAX_BUTTON_WIDTH`] each); a column has buttons of
/// [`COLUMN_BUTTON_WIDTH`] whose height shrinks if the column would not fit the view.
pub fn compute_panel_layout(
    entries: &[UiButtonEntry],
    has_title: bool,
    anchor: PanelAnchor,
    direction: PanelDirection,
    viewport: Vec2,
) -> PanelLayout {
    if entries.is_empty() {
        return PanelLayout {
            bounds: PanelRect {
                left: 0.0,
                top: 0.0,
                width: 0.0,
                height: 0.0,
            },
            title: None,
            buttons: Vec::new(),
        };
    }
    let count = entries.len() as f32;
    let title_block = if has_title {
        TITLE_HEIGHT + PANEL_GAP
    } else {
        0.0
    };
    let (button_width, button_height) = match direction {
        PanelDirection::Row => {
            let room = viewport.x - 2.0 * PANEL_MARGIN - PANEL_GAP * (count - 1.0);
            (
                (room / count).clamp(0.0, ROW_MAX_BUTTON_WIDTH),
                BUTTON_HEIGHT,
            )
        }
        PanelDirection::Column => {
            let room = viewport.y - 2.0 * PANEL_MARGIN - title_block - PANEL_GAP * (count - 1.0);
            (
                (viewport.x - 2.0 * PANEL_MARGIN).clamp(0.0, COLUMN_BUTTON_WIDTH),
                (room / count).clamp(MIN_BUTTON_HEIGHT, BUTTON_HEIGHT),
            )
        }
    };
    let (buttons_width, buttons_height) = match direction {
        PanelDirection::Row => (
            count * button_width + PANEL_GAP * (count - 1.0),
            button_height,
        ),
        PanelDirection::Column => (
            button_width,
            count * button_height + PANEL_GAP * (count - 1.0),
        ),
    };
    let size = Vec2::new(buttons_width, title_block + buttons_height);
    let origin = match anchor {
        PanelAnchor::TopLeft => Vec2::new(PANEL_MARGIN, PANEL_MARGIN),
        PanelAnchor::TopRight => Vec2::new(viewport.x - PANEL_MARGIN - size.x, PANEL_MARGIN),
        PanelAnchor::Center => (viewport - size) / 2.0,
    };
    let buttons = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let step = index as f32;
            let (left, top) = match direction {
                PanelDirection::Row => (
                    origin.x + step * (button_width + PANEL_GAP),
                    origin.y + title_block,
                ),
                PanelDirection::Column => (
                    origin.x,
                    origin.y + title_block + step * (button_height + PANEL_GAP),
                ),
            };
            ButtonRect {
                id: entry.id,
                rect: PanelRect {
                    left,
                    top,
                    width: button_width,
                    height: button_height,
                },
            }
        })
        .collect();
    PanelLayout {
        bounds: PanelRect {
            left: origin.x,
            top: origin.y,
            width: size.x,
            height: size.y,
        },
        title: has_title.then_some(PanelRect {
            left: origin.x,
            top: origin.y,
            width: size.x,
            height: TITLE_HEIGHT,
        }),
        buttons,
    }
}

/// A panel and what can change about it while the game runs.
#[derive(Debug, Clone)]
struct PanelState {
    panel: UiPanel,
    visible: bool,
    hidden_entries: BTreeSet<&'static str>,
}

/// Every panel of the app (a Bevy resource). Changing it (show, hide) makes the engine rebuild the
/// UI. Created by [`AppUiExt::register_panel`].
#[derive(Resource, Default, Debug)]
pub struct UiPanels {
    panels: BTreeMap<&'static str, PanelState>,
}

impl UiPanels {
    /// Adds a panel.
    ///
    /// # Panics
    /// If the id is empty, has whitespace, or is registered twice; or if two entries of the panel
    /// share an id or an entry id is empty/has whitespace (bugs to fix in code, like command ids).
    pub fn register(&mut self, panel: UiPanel) {
        let is_word = |id: &str| !id.is_empty() && !id.contains(char::is_whitespace);
        assert!(
            is_word(panel.id),
            "UiPanels::register: invalid panel id `{}`; an id is one word like `pause_menu`",
            panel.id
        );
        assert!(
            !self.panels.contains_key(panel.id),
            "UiPanels::register: panel `{}` is registered twice; panel ids must be unique",
            panel.id
        );
        let mut seen = BTreeSet::new();
        for entry in panel.entries {
            assert!(
                is_word(entry.id),
                "UiPanels::register: panel `{}` has an invalid entry id `{}`",
                panel.id,
                entry.id
            );
            assert!(
                seen.insert(entry.id),
                "UiPanels::register: panel `{}` lists entry `{}` twice; entry ids must be unique",
                panel.id,
                entry.id
            );
        }
        self.panels.insert(
            panel.id,
            PanelState {
                panel,
                visible: panel.visible,
                hidden_entries: BTreeSet::new(),
            },
        );
    }

    /// All panel ids, sorted.
    pub fn ids(&self) -> Vec<&'static str> {
        self.panels.keys().copied().collect()
    }

    /// The declaration of a panel.
    pub fn panel(&self, id: &str) -> Option<UiPanel> {
        self.panels.get(id).map(|state| state.panel)
    }

    /// Whether the panel is shown. `false` for an unknown id.
    pub fn is_visible(&self, id: &str) -> bool {
        self.panels.get(id).is_some_and(|state| state.visible)
    }

    /// Shows or hides a panel.
    pub fn set_visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
        let state = self.state_mut(id)?;
        if state.visible != visible {
            state.visible = visible;
        }
        Ok(())
    }

    /// The entries of the panel that are not hidden, in order. Empty for an unknown id.
    pub fn visible_entries(&self, id: &str) -> Vec<UiButtonEntry> {
        self.panels
            .get(id)
            .map(|state| visible_entries(state.panel.entries, &state.hidden_entries))
            .unwrap_or_default()
    }

    /// Hides one entry (the others re-flow). Hiding a hidden entry is fine.
    pub fn hide_entry(&mut self, id: &str, entry: &str) -> Result<(), String> {
        let entry_id = self.entry_id(id, entry)?;
        self.state_mut(id)?.hidden_entries.insert(entry_id);
        Ok(())
    }

    /// Shows a hidden entry again, in its place.
    pub fn show_entry(&mut self, id: &str, entry: &str) -> Result<(), String> {
        let entry_id = self.entry_id(id, entry)?;
        self.state_mut(id)?.hidden_entries.remove(entry_id);
        Ok(())
    }

    /// The entry called `entry` of panel `id`, or an error listing the valid ids.
    pub fn entry(&self, id: &str, entry: &str) -> Result<UiButtonEntry, String> {
        let state = self.state(id)?;
        state
            .panel
            .entries
            .iter()
            .find(|candidate| candidate.id == entry)
            .copied()
            .ok_or_else(|| {
                let ids: Vec<&str> = state.panel.entries.iter().map(|e| e.id).collect();
                format!(
                    "panel `{id}` has no entry `{entry}`; entries: {}",
                    ids.join(", ")
                )
            })
    }

    /// Command ids that panel entries run and `is_registered` rejects, with the panel and entry:
    /// the engine's startup check.
    pub fn entries_with_unknown_commands(
        &self,
        is_registered: impl Fn(&str) -> bool,
    ) -> Vec<(String, String)> {
        let mut unknown = Vec::new();
        for state in self.panels.values() {
            for entry in state.panel.entries {
                let command_id = entry.command_line.split_whitespace().next().unwrap_or("");
                if !is_registered(command_id) {
                    unknown.push((
                        format!("{}/{}", state.panel.id, entry.id),
                        command_id.to_string(),
                    ));
                }
            }
        }
        unknown
    }

    fn entry_id(&self, id: &str, entry: &str) -> Result<&'static str, String> {
        self.entry(id, entry).map(|found| found.id)
    }

    fn state(&self, id: &str) -> Result<&PanelState, String> {
        self.panels.get(id).ok_or_else(|| {
            let ids: Vec<&str> = self.panels.keys().copied().collect();
            format!("no panel `{id}`; panels: {}", ids.join(", "))
        })
    }

    fn state_mut(&mut self, id: &str) -> Result<&mut PanelState, String> {
        if !self.panels.contains_key(id) {
            return Err(self.state(id).err().unwrap_or_default());
        }
        Ok(self.panels.get_mut(id).expect("checked above"))
    }
}

/// Adds `register_panel` to Bevy's `App`, so a module's plugin can declare its panels.
pub trait AppUiExt {
    /// Registers a panel, creating [`UiPanels`] if needed. See [`UiPanels::register`].
    fn register_panel(&mut self, panel: UiPanel) -> &mut Self;
}

impl AppUiExt for App {
    fn register_panel(&mut self, panel: UiPanel) -> &mut Self {
        self.init_resource::<UiPanels>();
        self.world_mut().resource_mut::<UiPanels>().register(panel);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRIES: &[UiButtonEntry] = &[
        UiButtonEntry {
            id: "a",
            label: "A",
            command_line: "x.a",
        },
        UiButtonEntry {
            id: "b",
            label: "B",
            command_line: "x.b",
        },
        UiButtonEntry {
            id: "c",
            label: "C",
            command_line: "x.c",
        },
    ];
    const WIDE: Vec2 = Vec2::new(1280.0, 720.0);

    fn panel(direction: PanelDirection, anchor: PanelAnchor) -> UiPanel {
        UiPanel {
            id: "menu",
            title: Some("Menu"),
            anchor,
            direction,
            entries: ENTRIES,
            visible: false,
        }
    }

    fn ids(layout: &PanelLayout) -> Vec<&'static str> {
        layout.buttons.iter().map(|button| button.id).collect()
    }

    #[test]
    fn a_row_shows_entries_in_order_at_full_width_from_the_corner() {
        let layout = compute_panel_layout(
            ENTRIES,
            false,
            PanelAnchor::TopLeft,
            PanelDirection::Row,
            WIDE,
        );
        assert_eq!(ids(&layout), ["a", "b", "c"]);
        assert!(
            layout
                .buttons
                .iter()
                .all(|b| b.rect.width == ROW_MAX_BUTTON_WIDTH)
        );
        assert_eq!(layout.buttons[0].rect.left, PANEL_MARGIN);
        assert_eq!(layout.buttons[0].rect.top, PANEL_MARGIN);
        assert_eq!(layout.title, None);
    }

    #[test]
    fn hiding_an_entry_reflows_the_others_without_a_gap() {
        let hidden = BTreeSet::from(["b"]);
        let shown = visible_entries(ENTRIES, &hidden);
        let layout = compute_panel_layout(
            &shown,
            false,
            PanelAnchor::TopLeft,
            PanelDirection::Row,
            WIDE,
        );
        assert_eq!(ids(&layout), ["a", "c"]);
        for pair in layout.buttons.windows(2) {
            assert_eq!(
                pair[1].rect.left,
                pair[0].rect.left + pair[0].rect.width + PANEL_GAP,
                "buttons must sit next to each other: {pair:?}"
            );
        }
    }

    #[test]
    fn row_buttons_shrink_to_fit_a_narrow_view() {
        let narrow = Vec2::new(300.0, 720.0);
        let layout = compute_panel_layout(
            ENTRIES,
            false,
            PanelAnchor::TopLeft,
            PanelDirection::Row,
            narrow,
        );
        let last = layout.buttons.last().expect("three buttons");
        assert!(layout.buttons[0].rect.width < ROW_MAX_BUTTON_WIDTH);
        assert_eq!(last.rect.left + last.rect.width + PANEL_MARGIN, narrow.x);
    }

    #[test]
    fn a_top_right_row_ends_at_the_right_margin() {
        let layout = compute_panel_layout(
            ENTRIES,
            false,
            PanelAnchor::TopRight,
            PanelDirection::Row,
            WIDE,
        );
        let last = layout.buttons.last().unwrap();
        assert_eq!(last.rect.left + last.rect.width, WIDE.x - PANEL_MARGIN);
    }

    #[test]
    fn a_centered_column_is_centered_with_a_title_above_its_buttons() {
        let layout = compute_panel_layout(
            ENTRIES,
            true,
            PanelAnchor::Center,
            PanelDirection::Column,
            WIDE,
        );
        let bounds = layout.bounds;
        assert!((bounds.left + bounds.width / 2.0 - WIDE.x / 2.0).abs() < 1e-3);
        assert!((bounds.top + bounds.height / 2.0 - WIDE.y / 2.0).abs() < 1e-3);
        let title = layout.title.expect("the panel has a title");
        assert_eq!(title.top, bounds.top);
        assert_eq!(
            layout.buttons[0].rect.top,
            bounds.top + TITLE_HEIGHT + PANEL_GAP
        );
        assert!(
            layout
                .buttons
                .iter()
                .all(|b| b.rect.width == COLUMN_BUTTON_WIDTH)
        );
        assert_eq!(
            layout.buttons[1].rect.top,
            layout.buttons[0].rect.top + BUTTON_HEIGHT + PANEL_GAP
        );
    }

    #[test]
    fn a_column_in_a_short_view_shrinks_its_buttons_but_not_below_the_minimum() {
        let short = Vec2::new(800.0, 140.0);
        let layout = compute_panel_layout(
            ENTRIES,
            false,
            PanelAnchor::TopLeft,
            PanelDirection::Column,
            short,
        );
        let height = layout.buttons[0].rect.height;
        assert!(
            (MIN_BUTTON_HEIGHT..BUTTON_HEIGHT).contains(&height),
            "{height}"
        );
        let last = layout.buttons.last().unwrap();
        assert!(last.rect.top + last.rect.height + PANEL_MARGIN <= short.y + 1e-3);
        let tiny = Vec2::new(800.0, 20.0);
        let layout = compute_panel_layout(
            ENTRIES,
            false,
            PanelAnchor::TopLeft,
            PanelDirection::Column,
            tiny,
        );
        assert_eq!(layout.buttons[0].rect.height, MIN_BUTTON_HEIGHT);
    }

    #[test]
    fn no_visible_entry_gives_an_empty_layout_with_no_title() {
        let layout =
            compute_panel_layout(&[], true, PanelAnchor::Center, PanelDirection::Column, WIDE);
        assert!(layout.buttons.is_empty());
        assert_eq!(layout.title, None);
    }

    #[test]
    fn registering_and_changing_panels() {
        let mut panels = UiPanels::default();
        panels.register(panel(PanelDirection::Column, PanelAnchor::Center));
        assert!(
            !panels.is_visible("menu"),
            "hidden unless the panel says visible"
        );
        panels.set_visible("menu", true).unwrap();
        assert!(panels.is_visible("menu"));
        panels.hide_entry("menu", "b").unwrap();
        let shown: Vec<&str> = panels
            .visible_entries("menu")
            .iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(shown, ["a", "c"]);
        panels.show_entry("menu", "b").unwrap();
        assert_eq!(panels.visible_entries("menu").len(), 3);
        assert_eq!(panels.entry("menu", "c").unwrap().command_line, "x.c");
    }

    #[test]
    fn mistakes_list_the_valid_choices() {
        let mut panels = UiPanels::default();
        panels.register(panel(PanelDirection::Row, PanelAnchor::TopLeft));
        let error = panels.set_visible("menue", true).unwrap_err();
        assert!(error.contains("no panel `menue`; panels: menu"), "{error}");
        let error = panels.hide_entry("menu", "z").unwrap_err();
        assert!(
            error.contains("panel `menu` has no entry `z`; entries: a, b, c"),
            "{error}"
        );
        assert!(!panels.is_visible("nope"));
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn a_panel_id_cannot_be_registered_twice() {
        let mut panels = UiPanels::default();
        panels.register(panel(PanelDirection::Row, PanelAnchor::TopLeft));
        panels.register(panel(PanelDirection::Row, PanelAnchor::TopLeft));
    }

    #[test]
    #[should_panic(expected = "lists entry `a` twice")]
    fn an_entry_id_cannot_repeat_inside_a_panel() {
        const TWICE: &[UiButtonEntry] = &[ENTRIES[0], ENTRIES[0]];
        let mut panels = UiPanels::default();
        panels.register(UiPanel {
            entries: TWICE,
            ..panel(PanelDirection::Row, PanelAnchor::TopLeft)
        });
    }

    #[test]
    fn unknown_commands_are_reported_with_their_panel_and_entry() {
        let mut panels = UiPanels::default();
        panels.register(panel(PanelDirection::Row, PanelAnchor::TopLeft));
        let unknown = panels.entries_with_unknown_commands(|id| id == "x.a" || id == "x.c");
        assert_eq!(unknown, vec![("menu/b".to_string(), "x.b".to_string())]);
    }
}
