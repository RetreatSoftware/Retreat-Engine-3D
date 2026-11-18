//! Why a command could not run. Every message says what went wrong and how to fix it,
//! so an agent can correct a script from the message alone.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum CommandError {
    /// No command is registered under this id.
    UnknownCommand { id: String, known_ids: Vec<String> },
    /// The command got more or fewer arguments than it takes.
    WrongArgumentCount {
        id: String,
        usage: &'static str,
        expected: usize,
        found: usize,
    },
    /// An argument could not be read as the type the command needs (e.g. `abc` as a number).
    InvalidArgument {
        id: String,
        usage: &'static str,
        name: String,
        value: String,
        reason: String,
    },
    /// The command ran but did not succeed (e.g. an `assert.*` command whose check failed).
    Failed { id: String, message: String },
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::UnknownCommand { id, known_ids } => write!(
                f,
                "unknown command `{id}`; known commands: {}. \
                 Check the spelling, or register the command with `app.register_command(...)`",
                known_ids.join(", ")
            ),
            CommandError::WrongArgumentCount {
                id,
                usage,
                expected,
                found,
            } => write!(
                f,
                "`{id}` takes {expected} argument(s) but got {found}; usage: {usage}"
            ),
            CommandError::InvalidArgument {
                id,
                usage,
                name,
                value,
                reason,
            } => write!(
                f,
                "`{id}`: argument <{name}> = `{value}` is invalid ({reason}); usage: {usage}"
            ),
            CommandError::Failed { id, message } => write!(f, "`{id}` failed: {message}"),
        }
    }
}

impl std::error::Error for CommandError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_message_lists_known_ids() {
        let error = CommandError::UnknownCommand {
            id: "wiat".to_string(),
            known_ids: vec!["quit".to_string(), "wait".to_string()],
        };
        let message = error.to_string();
        assert!(message.contains("unknown command `wiat`"), "{message}");
        assert!(message.contains("known commands: quit, wait"), "{message}");
    }

    #[test]
    fn wrong_argument_count_message_shows_usage() {
        let error = CommandError::WrongArgumentCount {
            id: "wait".to_string(),
            usage: "wait <frames>",
            expected: 1,
            found: 0,
        };
        assert_eq!(
            error.to_string(),
            "`wait` takes 1 argument(s) but got 0; usage: wait <frames>"
        );
    }
}
