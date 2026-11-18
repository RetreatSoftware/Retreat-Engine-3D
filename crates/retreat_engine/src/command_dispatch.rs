//! The entry point for commands that come from the UI, keys or other input (not from a
//! script line): run the command, and report a failure the same way everywhere.

use bevy::prelude::*;
use retreat_engine_api::run_command_line;

use crate::command_script::fail_running_script;

/// Runs `line` like a script line would. `source` says where it came from (e.g.
/// `toolbar button "drop_box"`) and prefixes the error. On failure it logs the error and, if
/// a script is running (a script pressed the button), fails the script too, so a broken button
/// fails its check. `wait` outcomes are ignored: only scripts wait.
pub fn dispatch_command_line(world: &mut World, source: &str, line: &str) {
    if let Err(error) = run_command_line(world, line) {
        let message = format!("{source}: {error}");
        error!("{message}");
        fail_running_script(world, message);
    }
}
