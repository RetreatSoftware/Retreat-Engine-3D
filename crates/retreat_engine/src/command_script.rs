//! Command scripts: text files of commands that run one line per frame.
//!
//! Determinism contract: when a script is loaded, every app update runs exactly one fixed
//! tick (`TimeUpdateStrategy::FixedTimesteps(1)`), except the very first update, where Bevy
//! starts its clock and runs none. The runner runs in `FixedUpdate`, so the N-th command
//! line always runs on fixed tick N (plus any `wait`), however fast the machine is.
//! A line that starts work finishing later (`snapshot`, `model.spawn`) holds the script through
//! [`PendingCommandWork`] until the work is done. While it holds, simulation time is paused
//! (`Time<Virtual>`), so no fixed tick runs and nothing that reads time (physics, animated
//! shaders) moves: pending work costs zero ticks, however long it takes on this machine.

use std::path::Path;
use std::time::Duration;

use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use retreat_engine_api::{CommandOutcome, parse_command_line, run_command_line};

use crate::PendingCommandWork;

/// How long a script waits for [`PendingCommandWork`] before it fails. Wall-clock time, not
/// ticks: pending work (a GPU capture, a file read, a file watcher) runs in real time, and a
/// tick takes 3 ms on a GPU but 250 ms on a software renderer. The timeout only decides when a
/// stuck script gives up, so it does not affect what a passing script does.
pub const PENDING_WORK_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// When the script started waiting for [`PendingCommandWork`]; `None` while not waiting.
    waiting_for_work_since: Option<Instant>,
    /// See [`PENDING_WORK_TIMEOUT`]; tests shorten it with [`Self::with_pending_work_timeout`].
    pending_work_timeout: Duration,
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
            waiting_for_work_since: None,
            pending_work_timeout: PENDING_WORK_TIMEOUT,
            failure: None,
        }
    }

    /// Reads a script file. Native only: in the browser there is no file system.
    pub fn read_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read command script `{}`: {error}", path.display()))?;
        Ok(Self::from_text(path.display().to_string(), &text))
    }

    /// Replaces [`PENDING_WORK_TIMEOUT`] for this script (tests use a short one).
    pub fn with_pending_work_timeout(mut self, timeout: Duration) -> Self {
        self.pending_work_timeout = timeout;
        self
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
            .add_systems(FixedUpdate, run_next_script_line)
            .add_systems(Last, hold_time_while_script_waits);
    }
}

/// Runs at most one command line per fixed tick. On the first error it logs
/// `file:line: message`, stops the script and exits the app with an error code, so a failing
/// check fails the process (and CI). Waiting for [`PendingCommandWork`] is done by
/// [`hold_time_while_script_waits`], which stops ticks entirely while work is in flight.
fn run_next_script_line(world: &mut World) {
    world.resource_scope(|world, mut script: Mut<CommandScript>| {
        if script.failure.is_some() {
            return;
        }
        // Failures of pending work are reported against the line that started it.
        let pending_failure = world.resource_mut::<PendingCommandWork>().take_failure();
        if let Some(failure) = pending_failure {
            let started_by_line = script.last_line_number;
            fail_script(world, &mut script, started_by_line, failure);
            return;
        }
        if world.resource::<PendingCommandWork>().is_busy() {
            // Work started outside the script; the hold system pauses time until it is done.
            return;
        }
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

/// Runs at the end of every update (`Last`). While [`PendingCommandWork`] is busy it pauses
/// `Time<Virtual>`: the next updates still load assets and render, but run no fixed tick and
/// advance no simulation time. When the work is done it unpauses, and the next update runs the
/// next tick. Fails the script if the work takes longer than the (wall-clock) timeout.
fn hold_time_while_script_waits(world: &mut World) {
    world.resource_scope(|world, mut script: Mut<CommandScript>| {
        if script.failure.is_some() {
            return;
        }
        let is_busy = world.resource::<PendingCommandWork>().is_busy();
        let mut virtual_time = world.resource_mut::<Time<Virtual>>();
        if !is_busy {
            virtual_time.unpause();
            script.waiting_for_work_since = None;
            return;
        }
        virtual_time.pause();
        let waiting_since = *script
            .waiting_for_work_since
            .get_or_insert_with(Instant::now);
        if waiting_since.elapsed() > script.pending_work_timeout {
            let started_by_line = script.last_line_number;
            let message = format!(
                "the work this line started did not finish within {:?} \
                 (for `snapshot`: is a camera rendering to the window or offscreen image?)",
                script.pending_work_timeout
            );
            fail_script(world, &mut script, started_by_line, message);
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

/// Fails the running script against its last line, for errors found outside a command (a
/// shader that fails to compile on the GPU). Does nothing without a script, or when the script
/// has already failed (only the first error is reported).
pub(crate) fn fail_running_script(world: &mut World, message: impl std::fmt::Display) {
    if !world.contains_resource::<CommandScript>() {
        return;
    }
    world.resource_scope(|world, mut script: Mut<CommandScript>| {
        if script.failure.is_some() {
            return;
        }
        let line_number = script.last_line_number;
        fail_script(world, &mut script, line_number, message);
    });
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
    fn fail_running_script_reports_against_the_last_line_once() {
        let mut world = World::new();
        world.init_resource::<Messages<AppExit>>();
        let mut script = CommandScript::from_text("t.cmds", "wait 1\nquit\n");
        script.last_line_number = 1;
        world.insert_resource(script);
        fail_running_script(&mut world, "shader broke");
        fail_running_script(&mut world, "second error is ignored");
        assert_eq!(
            world.resource::<CommandScript>().failure(),
            Some("t.cmds:1: shader broke")
        );
    }

    #[test]
    fn fail_running_script_without_a_script_does_nothing() {
        let mut world = World::new();
        fail_running_script(&mut world, "shader broke");
        assert!(!world.contains_resource::<CommandScript>());
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
