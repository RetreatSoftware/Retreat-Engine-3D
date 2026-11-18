//! Command scripts: text files of commands that run one line per frame.
//!
//! Determinism contract: when a script is loaded, every app update runs exactly one fixed
//! tick (`TimeUpdateStrategy::FixedTimesteps(1)`), except the very first update, where Bevy
//! starts its clock and runs none. The runner runs in `FixedUpdate`, so the N-th command
//! line always runs on fixed tick N (plus any `wait`), however fast the machine is.

use std::path::Path;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use retreat_engine_api::{CommandOutcome, parse_command_line, run_command_line};

/// A loaded script and how far it has run. Present as a resource only while the app
/// was launched with `--commands`.
#[derive(Resource, Debug, Clone)]
pub struct CommandScript {
    /// Shown in front of every error (`scripts/verify/a.cmds:3: ...`).
    source_name: String,
    lines: Vec<String>,
    next_line_index: usize,
    frames_to_wait: u32,
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

/// Runs at most one command line per fixed tick. On the first error it logs
/// `file:line: message`, stops the script and exits the app with an error code,
/// so a failing check fails the process (and CI).
fn run_next_script_line(world: &mut World) {
    world.resource_scope(|world, mut script: Mut<CommandScript>| {
        if script.frames_to_wait > 0 {
            script.frames_to_wait -= 1;
            return;
        }
        let Some((line_number, text)) = script.take_next_command_line() else {
            return;
        };
        match run_command_line(world, &text) {
            Ok(CommandOutcome::Continue) => {}
            Ok(CommandOutcome::WaitFrames(frames)) => script.frames_to_wait = frames,
            Err(error) => {
                let message = format!("{}:{line_number}: {error}", script.source_name);
                error!("{message}");
                script.stop_with_failure(message);
                world.write_message(AppExit::error());
            }
        }
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
    fn read_file_names_the_missing_file() {
        let error = CommandScript::read_file(Path::new("does/not/exist.cmds")).unwrap_err();
        assert!(
            error.contains("cannot read command script `does/not/exist.cmds`"),
            "{error}"
        );
    }
}
