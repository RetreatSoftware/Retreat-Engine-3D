//! Item grid commands in an app without a window or GPU: a drag ends in the window's drop command
//! line (rotation included), an unchanged drop runs nothing, scroll and hover, and errors. The
//! pictures are checked by `scripts/verify/item_grids.cmds`.

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use retreat_engine::{CommandScript, RetreatCommandsPlugin, UiGridsPlugin, UiPanelsPlugin};
use retreat_engine_api::{
    AppCommandExt, CommandOutcome, UiGridItem, UiGridShape, UiGridView, UiGrids, UiTooltipLine,
};

const MAX_UPDATES: u32 = 200;

fn bag() -> UiGridView {
    UiGridView {
        title: "Bag".to_string(),
        columns: 4,
        rows: 6,
        visible_rows: 3,
        shape: UiGridShape::Cells,
        items: vec![UiGridItem {
            id: "sword".to_string(),
            x: 0,
            y: 0,
            size: UVec2::new(1, 3),
            rotated: false,
            color: Color::WHITE,
            image: None,
            label: "Sword".to_string(),
            tooltip: vec![UiTooltipLine::new("Rusty sword", Color::WHITE)],
        }],
        drop_command: "test.put bag".to_string(),
        order: 0,
        footer: String::new(),
    }
}

/// Runs `text` with a `bag` window shown; returns the lines `test.put` received and the failure.
fn run_script(text: &str) -> (Vec<String>, Option<String>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RetreatCommandsPlugin {
            seed: 0,
            command_script: Some(CommandScript::from_text("test.cmds", text)),
        })
        .add_plugins((UiPanelsPlugin, UiGridsPlugin))
        .register_command("test.put", "test.put <args...>", move |_world, args| {
            sink.lock().unwrap().push(args.rest(0).join(" "));
            Ok(CommandOutcome::Continue)
        });
    app.finish();
    app.cleanup();
    app.world_mut()
        .resource_mut::<UiGrids>()
        .set_view("bag", bag());
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
    let lines = received.lock().unwrap().clone();
    (lines, failure)
}

#[test]
fn a_rotated_drag_runs_the_drop_command_with_the_place_and_the_turn() {
    let (lines, failure) = run_script(
        "ui.drag_start bag sword\nassert.ui_dragging sword\nui.drag_rotate\nui.drag_over bag 1 4\nui.drag_drop\nwait 2\nassert.ui_dragging nothing\nquit\n",
    );
    assert_eq!(failure, None);
    assert_eq!(lines, vec!["bag sword 1 4 1".to_string()]);
}

#[test]
fn a_drop_where_the_item_already_is_runs_nothing_and_cancel_drops_nothing() {
    let (lines, failure) = run_script(
        "ui.drag_start bag sword\nui.drag_drop bag 0 0\nui.drag_start bag sword\nui.drag_cancel\nwait 2\nquit\n",
    );
    assert_eq!(failure, None);
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn scroll_stays_within_the_rows_and_hover_shows_the_tooltip() {
    let (_, failure) = run_script(
        "ui.scroll_grid bag 50\nassert.ui_grid_scroll bag 3\nui.hover_item bag sword\nassert.ui_tooltip Rusty sword\nassert.ui_grid_item bag sword 0 0 upright\nassert.ui_grid_count bag 1\nquit\n",
    );
    assert_eq!(failure, None);
}

#[test]
fn errors_name_the_window_the_item_and_what_to_do() {
    let failure = run_script("ui.drag_start chest sword\n").1.unwrap();
    assert!(
        failure.contains("`chest` is not shown") && failure.contains("bag"),
        "{failure}"
    );
    let failure = run_script("ui.drag_drop\n").1.unwrap();
    assert!(failure.contains("nothing is being dragged"), "{failure}");
    let failure = run_script("ui.drag_start bag sword\nui.drag_drop\n")
        .1
        .unwrap();
    assert!(failure.contains("ui.drag_over"), "{failure}");
    let failure = run_script("assert.ui_grid_item bag sword 2 0\n").1.unwrap();
    assert!(failure.contains("is at (0, 0) upright"), "{failure}");
}
