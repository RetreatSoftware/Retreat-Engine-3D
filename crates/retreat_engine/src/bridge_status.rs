//! The status text the browser bridge returns from `__retreat.status()`: a small JSON object
//! that says whether the launch script is running, passed or failed. Pure, so it is tested on
//! native; the browser side (`platform/wasm.rs`) only stores and returns the string.
//!
//! Shapes:
//! - `{"script":"none"}`: launched without `?commands=`.
//! - `{"script":"running","line":7}`: line 7 ran last.
//! - `{"script":"passed"}`: every line ran and all started work finished.
//! - `{"script":"failed","failure":"scripts/verify/x.cmds:4: ..."}`.

use crate::command_script::ScriptProgress;

/// The JSON for a script's progress (`None` when no script was launched).
pub fn script_status_json(progress: Option<&ScriptProgress>) -> String {
    match progress {
        None => r#"{"script":"none"}"#.to_string(),
        Some(ScriptProgress::Running { last_line }) => {
            format!(r#"{{"script":"running","line":{last_line}}}"#)
        }
        Some(ScriptProgress::Passed) => r#"{"script":"passed"}"#.to_string(),
        Some(ScriptProgress::Failed(failure)) => {
            format!(
                r#"{{"script":"failed","failure":"{}"}}"#,
                json_escape(failure)
            )
        }
    }
}

/// Escapes text for a JSON string: quotes, backslashes and control characters.
fn json_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_progress_has_its_json_shape() {
        assert_eq!(script_status_json(None), r#"{"script":"none"}"#);
        assert_eq!(
            script_status_json(Some(&ScriptProgress::Running { last_line: 7 })),
            r#"{"script":"running","line":7}"#
        );
        assert_eq!(
            script_status_json(Some(&ScriptProgress::Passed)),
            r#"{"script":"passed"}"#
        );
    }

    #[test]
    fn failures_are_escaped_into_valid_json_strings() {
        let failure = ScriptProgress::Failed("a.cmds:2: `x` said \"no\"\\\nnext".to_string());
        assert_eq!(
            script_status_json(Some(&failure)),
            r#"{"script":"failed","failure":"a.cmds:2: `x` said \"no\"\\\nnext"}"#
        );
    }
}
