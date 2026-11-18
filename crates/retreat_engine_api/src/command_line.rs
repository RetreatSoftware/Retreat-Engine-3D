//! Splits one line of command text into a command id and its arguments.

/// One command line after splitting: `wait 3` becomes id `wait` and args `["3"]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommandLine {
    pub id: String,
    pub args: Vec<String>,
}

/// A line whose first non-space character is this is a comment.
const COMMENT_PREFIX: char = '#';

/// Splits `line` on whitespace. Returns `None` for a blank line or a comment line,
/// which are not commands. Arguments cannot contain spaces (there is no quoting).
pub fn parse_command_line(line: &str) -> Option<ParsedCommandLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with(COMMENT_PREFIX) {
        return None;
    }
    let mut words = trimmed.split_whitespace().map(String::from);
    let id = words.next()?;
    let args = words.collect();
    Some(ParsedCommandLine { id, args })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_id_and_arguments() {
        let parsed = parse_command_line("assert.position cube 0 0.5 0");
        let expected = ParsedCommandLine {
            id: "assert.position".to_string(),
            args: vec!["cube".into(), "0".into(), "0.5".into(), "0".into()],
        };
        assert_eq!(parsed, Some(expected));
    }

    #[test]
    fn command_without_arguments_has_empty_args() {
        let parsed = parse_command_line("quit").expect("`quit` is a command");
        assert_eq!(parsed.id, "quit");
        assert!(parsed.args.is_empty());
    }

    #[test]
    fn extra_whitespace_is_ignored() {
        let parsed = parse_command_line("   wait \t  3   ").expect("`wait 3` is a command");
        assert_eq!(parsed.id, "wait");
        assert_eq!(parsed.args, vec!["3".to_string()]);
    }

    #[test]
    fn blank_and_comment_lines_are_not_commands() {
        for line in ["", "   ", "# a comment", "   # indented comment"] {
            assert_eq!(
                parse_command_line(line),
                None,
                "line {line:?} must not be a command"
            );
        }
    }
}
