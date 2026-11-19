//! The sandbox toolbar: a row of buttons along the top of the view, declared with the engine's UI
//! toolkit (`retreat_engine_api::UiPanel`). It is the same mechanism a module uses for its own
//! panels: the engine draws it, lays it out with the pure `compute_panel_layout` and runs each
//! button's command line through the command registry.
//!
//! Drive it from a script with `ui.press toolbar <id>`, `ui.hide_entry toolbar <id>` and
//! `ui.show_entry toolbar <id>` (see `scripts/verify/toolbar.cmds`).

use bevy::prelude::*;
use retreat_engine_api::{AppUiExt, PanelAnchor, PanelDirection, UiButtonEntry, UiPanel};

/// Id of the toolbar panel in `ui.*` commands.
pub const TOOLBAR_PANEL_ID: &str = "toolbar";

/// Every toolbar button, in display order.
pub const SANDBOX_TOOLBAR_ENTRIES: &[UiButtonEntry] = &[
    UiButtonEntry {
        id: "drop_box",
        label: "Drop box",
        command_line: "sandbox.drop_box",
    },
    UiButtonEntry {
        id: "reset",
        label: "Reset boxes",
        command_line: "physics.reset",
    },
    UiButtonEntry {
        id: "stripes",
        label: "Stripes",
        command_line: "shader.apply sandbox_cube stripes",
    },
    UiButtonEntry {
        id: "checker",
        label: "Checker floor",
        command_line: "material.set_texture sandbox_ground textures/checker.png",
    },
];

/// Declares the toolbar panel: a row at the top left, shown from the start.
pub struct SandboxToolbarPlugin;

impl Plugin for SandboxToolbarPlugin {
    fn build(&self, app: &mut App) {
        app.register_panel(UiPanel {
            id: TOOLBAR_PANEL_ID,
            title: None,
            anchor: PanelAnchor::TopLeft,
            direction: PanelDirection::Row,
            entries: SANDBOX_TOOLBAR_ENTRIES,
            visible: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use retreat_engine_api::UiPanels;

    #[test]
    fn the_toolbar_registers_as_a_visible_panel_with_every_entry() {
        let mut app = App::new();
        app.add_plugins(SandboxToolbarPlugin);
        let panels = app.world().resource::<UiPanels>();
        assert!(panels.is_visible(TOOLBAR_PANEL_ID));
        let ids: Vec<&str> = panels
            .visible_entries(TOOLBAR_PANEL_ID)
            .iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(ids, ["drop_box", "reset", "stripes", "checker"]);
    }
}
