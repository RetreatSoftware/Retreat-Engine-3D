//! Command scripts: text files of commands that run one line per frame.
//!
//! Determinism contract: when a script is loaded, every app update runs exactly one fixed
//! tick (`TimeUpdateStrategy::FixedTimesteps(1)`), except the very first update, where Bevy
//! starts its clock and runs none. The runner runs in `FixedUpdate`, so the N-th command
//! line always runs on fixed tick N (plus any `wait`), however fast the machine is.
//! A line that starts work finishing later (`snapshot`) holds the script through
//! [`PendingCommandWork`] until the work is done.

use std::path::Path;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use retreat_engine_api::{CommandOutcome, parse_command_line, run_command_line};

use crate::PendingCommandWork;

/// How many ticks a script waits for [`PendingCommandWork`] (e.g. a snapshot) before it
/// fails. A capture normally takes 1-3 ticks; reaching this means the work never finishes.
pub const PENDING_WORK_TIMEOUT_TICKS: u32 = 300;

/// A loaded script and how far it has run. Present as a resource only while the app
/// was launched with `--commands`.
#[derive(Resource, Debug, Clone)]
pub struct CommandScript {
    /// Shown in front of every error (`scripts/verify/a.cmds:3: ...`).
    source_name: String,
    lines: Vec<String>,
    next_line_index: usize,
    frames_to_wait: u32,
    /// 1-based number of the last line that ran (0 before the first).
    last_line_number: usize,
    /// Ticks spent waiting for [`PendingCommandWork`] since it became busy.
    ticks_waiting_for_work: u32,
    /// The first error, with its `file:line` prefix. Once set, the script stops.
    failure: Option<String>,
}

impl CommandScript {
    pub fn from_text(source_name: impl Into<String>, text: &str) -> Self {
        Self {
            source_name: source_name.into(),
            lines: text.lines().map(String::from).collect(),
            next_line_index: 0,
            frames_to_wait: 0,
            last_line_number: 0,
            ticks_waiting_for_work: 0,
            failure: None,
        }
    }

    /// Reads a script file. Native only: in the browser there is no file system.
    pub fn read_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read command script `{}`: {error}", path.display()))?;
        Ok(Self::from_text(path.display().to_string(), &text))
    }

    /// The first error the script hit, if any (`file:line: message`).
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Returns the next line that is a command, with its 1-based line number,
    /// skipping blank and comment lines (they cost no frame).
    fn take_next_command_line(&mut self) -> Option<(usize, String)> {
        while self.next_line_index < self.lines.len() {
            let index = self.next_line_index;
            self.next_line_index += 1;
            let text = &self.lines[index];
            if parse_command_line(text).is_some() {
                return Some((index + 1, text.clone()));
            }
        }
        None
    }

    fn stop_with_failure(&mut self, message: String) {
        self.failure = Some(message);
        self.next_line_index = self.lines.len();
        self.frames_to_wait = 0;
    }
}

/// Runs a script: one line per fixed tick, on deterministic time.
pub(crate) struct CommandScriptPlugin {
    pub(crate) script: CommandScript,
}

impl Plugin for CommandScriptPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.script.clone())
            .insert_resource(TimeUpdateStrategy::FixedTimesteps(1))
            .add_systems(FixedUpdate, run_next_script_line);
    }
}

/// Runs at most one command line per fixed tick, and none while [`PendingCommandWork`] is
/// busy. On the first error it logs `file:line: message`, stops the script and exits the
/// app with an error code, so a failing check fails the process (and CI).
fn run_next_script_line(world: &mut World) {
    world.resource_scope(|world, mut script: Mut<CommandScript>| {
        if script.failure.is_some() {
            return;
        }
        // Failures and timeouts of pending work are reported against the line that started it.
        let started_by_line = script.last_line_number;
        let pending_failure = world.resource_mut::<PendingCommandWork>().take_failure();
        if let Some(failure) = pending_failure {
            fail_script(world, &mut script, started_by_line, failure);
            return;
        }
        if world.resource::<PendingCommandWork>().is_busy() {
            script.ticks_waiting_for_work += 1;
            if script.ticks_waiting_for_work > PENDING_WORK_TIMEOUT_TICKS {
                let message = format!(
                    "the work this line started did not finish within {PENDING_WORK_TIMEOUT_TICKS} \
                     ticks (for `snapshot`: is a camera rendering to the window or offscreen image?)"
                );
                fail_script(world, &mut script, started_by_line, message);
            }
            return;
        }
        script.ticks_waiting_for_work = 0;
        if script.frames_to_wait > 0 {
            script.frames_to_wait -= 1;
            return;
        }
        let Some((line_number, text)) = script.take_next_command_line() else {
            return;
        };
        script.last_line_number = line_number;
        match run_command_line(world, &text) {
            Ok(CommandOutcome::Continue) => {}
            Ok(CommandOutcome::WaitFrames(frames)) => script.frames_to_wait = frames,
            Err(error) => fail_script(world, &mut script, line_number, error),
        }
    });
}

/// Logs `file:line: message`, stops the script and asks the app to exit with an error code.
fn fail_script(
    world: &mut World,
    script: &mut CommandScript,
    line_number: usize,
    message: impl std::fmt::Display,
) {
    let located = format!("{}:{line_number}: {message}", script.source_name);
    error!("{located}");
    script.stop_with_failure(located);
    world.write_message(AppExit::error());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_blank_and_comment_lines_and_keeps_line_numbers() {
        let mut script = CommandScript::from_text("t.cmds", "# header\n\nwait 1\n  # note\nquit\n");
        assert_eq!(
            script.take_next_command_line(),
            Some((3, "wait 1".to_string()))
        );
        assert_eq!(
            script.take_next_command_line(),
            Some((5, "quit".to_string()))
        );
        assert_eq!(script.take_next_command_line(), None);
    }

    #[test]
    fn read_file_names_the_missing_file() {
        let error = CommandScript::read_file(Path::new("does/not/exist.cmds")).unwrap_err();
        assert!(
            error.contains("cannot read command script `does/not/exist.cmds`"),
            "{error}"
        );
    }
}
