//! UI toolkit commands in an app without a window or GPU: showing and hiding panels and entries,
//! errors, and the startup check. Drawing and clicking are checked by `scripts/verify/toolbar.cmds`
//! and `scripts/verify/module_menu.cmds` (they need a camera and rendering for the pictures).

use bevy::prelude::*;
use retreat_engine::{CommandScript, RetreatCommandsPlugin, UiHudPlugin, UiPanelsPlugin};
use retreat_engine_api::{
    AppCommandExt, AppHudExt, AppUiExt, CommandOutcome, HudAnchor, HudKind, HudWidget, PanelAnchor,
    PanelDirection, UiButtonEntry, UiPanel, UiPanels,
};

const MAX_UPDATES: u32 = 200;

const ENTRIES: &[UiButtonEntry] = &[
    UiButtonEntry {
        id: "one",
        label: "One",
        command_line: "test.noop",
    },
    UiButtonEntry {
        id: "two",
        label: "Two",
        command_line: "test.noop",
    },
];

fn panel() -> UiPanel {
    UiPanel {
        id: "menu",
        title: Some("Menu"),
        anchor: PanelAnchor::Center,
        direction: PanelDirection::Column,
        entries: ENTRIES,
        visible: false,
    }
}

fn run_script(text: &str) -> (App, Option<String>) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RetreatCommandsPlugin {
            seed: 0,
            command_script: Some(CommandScript::from_text("test.cmds", text)),
        })
        .add_plugins((UiPanelsPlugin, UiHudPlugin))
        .register_command("test.noop", "test.noop", |_world, _args| {
            Ok(CommandOutcome::Continue)
        })
        .register_panel(panel())
        .register_hud_widget(HudWidget {
            id: "status",
            anchor: HudAnchor::TopLeft,
            kind: HudKind::Text { font_size: 20.0 },
            visible: true,
        });
    app.finish();
    app.cleanup();
    for _ in 0..MAX_UPDATES {
        app.update();
        if app.should_exit().is_some() {
            break;
        }
    }
    let failure = app
        .world()
        .resource::<CommandScript>()
        .failure()
        .map(String::from);
    (app, failure)
}

fn failure_of(text: &str) -> String {
    run_script(text).1.expect("the script must fail")
}

#[test]
fn show_and_hide_change_the_panel_and_its_entries() {
    let (app, failure) = run_script(
        "ui.show_panel menu\nui.hide_entry menu one\nui.hide_entry menu one\nui.list\nquit\n",
    );
    assert_eq!(failure, None);
    let panels = app.world().resource::<UiPanels>();
    assert!(panels.is_visible("menu"));
    let shown: Vec<&str> = panels
        .visible_entries("menu")
        .iter()
        .map(|entry| entry.id)
        .collect();
    assert_eq!(shown, ["two"]);

    let (app, failure) = run_script(
        "ui.show_panel menu\nui.hide_entry menu one\nui.show_entry menu one\nui.hide_panel menu\nquit\n",
    );
    assert_eq!(failure, None);
    let panels = app.world().resource::<UiPanels>();
    assert!(!panels.is_visible("menu"));
    assert_eq!(panels.visible_entries("menu").len(), 2);
}

#[test]
fn errors_name_the_command_the_bad_value_and_the_choices() {
    let failure = failure_of("ui.show_panel menue\nquit\n");
    assert!(
        failure.contains("no panel `menue`; panels: menu"),
        "{failure}"
    );
    let failure = failure_of("ui.hide_entry menu three\nquit\n");
    assert!(
        failure.contains("panel `menu` has no entry `three`; entries: one, two"),
        "{failure}"
    );
    let failure = failure_of("ui.show_panel\nquit\n");
    assert!(
        failure.contains("usage: ui.show_panel <panel>"),
        "{failure}"
    );
}

#[test]
fn pressing_a_button_that_is_not_shown_says_why() {
    let failure = failure_of("ui.press menu one\nquit\n");
    assert!(
        failure.contains("button `one` of panel `menu` is not shown"),
        "{failure}"
    );
    let failure = failure_of("ui.press menu nine\nquit\n");
    assert!(failure.contains("has no entry `nine`"), "{failure}");
}

#[test]
#[should_panic(expected = "UI panel entry `menu/one` runs command `test.noop`")]
fn a_panel_entry_with_an_unregistered_command_panics_at_startup() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RetreatCommandsPlugin::default())
        .add_plugins((UiPanelsPlugin, UiHudPlugin))
        .register_panel(panel())
        .register_hud_widget(HudWidget {
            id: "status",
            anchor: HudAnchor::TopLeft,
            kind: HudKind::Text { font_size: 20.0 },
            visible: true,
        });
    app.finish();
}

#[test]
fn a_widget_text_is_set_asserted_and_hidden_by_command() {
    let (_, failure) =
        run_script("ui.set_text status round 2 of 3\nassert.ui_text status round 2 of 3\nquit\n");
    assert_eq!(failure, None);
    let hidden = failure_of("ui.hide_widget status\nassert.ui_text status\nquit\n");
    assert!(hidden.contains("is hidden"), "{hidden}");
    let unknown = failure_of("ui.set_text nope x\nquit\n");
    assert!(unknown.contains("status"), "lists the widgets: {unknown}");
}

#[test]
fn a_panels_state_is_asserted_without_rendering() {
    let (_, failure) = run_script(
        "assert.ui_panel_state menu hidden\nui.show_panel menu\nassert.ui_panel_state menu shown\nquit\n",
    );
    assert_eq!(failure, None);
    let failure = failure_of("assert.ui_panel_state menu shown\n");
    assert!(failure.contains("panel `menu` is hidden"), "{failure}");
    let failure = failure_of("assert.ui_panel_state nothing shown\n");
    assert!(failure.contains("there is no panel `nothing`"), "{failure}");
}
