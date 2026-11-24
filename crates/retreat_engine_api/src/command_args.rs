//! The arguments of one command call, with helpers that turn mistakes into clear errors.

use std::fmt::Display;
use std::str::FromStr;

use crate::CommandError;

/// What a command handler receives: its arguments as text, plus its id and usage so that
/// every error it returns can say which command failed and how to call it correctly.
#[derive(Debug, Clone)]
pub struct CommandArgs {
    id: String,
    usage: &'static str,
    values: Vec<String>,
}

impl CommandArgs {
    pub fn new(id: impl Into<String>, usage: &'static str, values: Vec<String>) -> Self {
        Self {
            id: id.into(),
            usage,
            values,
        }
    }

    /// The id of the command being run, e.g. `wait`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Fails unless the command got exactly `expected` arguments. Call it first in a handler.
    pub fn expect_count(&self, expected: usize) -> Result<(), CommandError> {
        if self.values.len() == expected {
            Ok(())
        } else {
            Err(self.wrong_count(expected))
        }
    }

    /// Reads argument number `index` (0-based) as type `T`. `name` is the placeholder shown
    /// in the usage (`frames` for `wait <frames>`) and appears in the error message.
    pub fn parse<T>(&self, index: usize, name: &str) -> Result<T, CommandError>
    where
        T: FromStr,
        T::Err: Display,
    {
        let Some(value) = self.values.get(index) else {
            return Err(self.wrong_count(index + 1));
        };
        value
            .parse::<T>()
            .map_err(|error| CommandError::InvalidArgument {
                id: self.id.clone(),
                usage: self.usage,
                name: name.to_string(),
                value: value.clone(),
                reason: error.to_string(),
            })
    }

    /// Every argument from `from` on (empty when there are fewer). For commands whose last argument
    /// is a whole command line (`net.send physics.push a 1 0 0`).
    pub fn rest(&self, from: usize) -> &[String] {
        self.values.get(from..).unwrap_or(&[])
    }

    /// Builds the error for a command that ran but did not succeed.
    pub fn failure(&self, message: impl Into<String>) -> CommandError {
        CommandError::Failed {
            id: self.id.clone(),
            message: message.into(),
        }
    }

    fn wrong_count(&self, expected: usize) -> CommandError {
        CommandError::WrongArgumentCount {
            id: self.id.clone(),
            usage: self.usage,
            expected,
            found: self.values.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_args(values: &[&str]) -> CommandArgs {
        let values = values.iter().map(|value| value.to_string()).collect();
        CommandArgs::new("wait", "wait <frames>", values)
    }

    #[test]
    fn parse_reads_typed_values() {
        let args = wait_args(&["12"]);
        assert_eq!(args.parse::<u32>(0, "frames"), Ok(12));
    }

    #[test]
    fn parse_reports_the_bad_value_and_the_usage() {
        let error = wait_args(&["soon"]).parse::<u32>(0, "frames").unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("<frames> = `soon` is invalid"),
            "{message}"
        );
        assert!(message.contains("usage: wait <frames>"), "{message}");
    }

    #[test]
    fn parse_of_a_missing_argument_is_a_count_error() {
        let error = wait_args(&[]).parse::<u32>(0, "frames").unwrap_err();
        assert!(matches!(
            error,
            CommandError::WrongArgumentCount {
                expected: 1,
                found: 0,
                ..
            }
        ));
    }

    #[test]
    fn expect_count_accepts_the_right_count_only() {
        assert_eq!(wait_args(&["1"]).expect_count(1), Ok(()));
        assert!(wait_args(&["1", "2"]).expect_count(1).is_err());
    }
}
