//! Command-line arguments every Retreat app accepts.

use std::path::PathBuf;

/// The seed used when `--seed` is not given.
pub const DEFAULT_SEED: u64 = 0;

/// Shown after every launch-argument error.
const LAUNCH_USAGE: &str = "usage: <app> [--commands <script.cmds>] [--seed <whole number>] [--headless] [--software-renderer]";

/// What the app was asked to do at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    /// `--commands <file>`: a command script to run, one line per frame.
    pub commands_file: Option<PathBuf>,
    /// `--seed <n>`: the seed of the `SimulationRng`.
    pub seed: u64,
    /// `--headless`: no window; cameras render to an offscreen image (native only).
    pub headless: bool,
    /// `--software-renderer`: use the CPU fallback GPU adapter (WARP on Windows, lavapipe on
    /// Linux), so rendering works on machines without a GPU, such as CI.
    pub software_renderer: bool,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            commands_file: None,
            seed: DEFAULT_SEED,
            headless: false,
            software_renderer: false,
        }
    }
}

impl LaunchOptions {
    /// Reads the process arguments. In the browser there are none, so this gives the defaults.
    pub fn from_env_args() -> Result<Self, String> {
        // The first argument is the program's own path, not an option.
        Self::parse(std::env::args().skip(1))
    }

    /// Parses arguments (without the program path). Pure, so it is tested directly.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--commands" => {
                    let value = next_value(&mut args, "--commands")?;
                    options.commands_file = Some(PathBuf::from(value));
                }
                "--seed" => {
                    let value = next_value(&mut args, "--seed")?;
                    options.seed = value.parse().map_err(|error| {
                        format!("--seed `{value}` is not a whole number from 0 to {}: {error}; {LAUNCH_USAGE}", u64::MAX)
                    })?;
                }
                "--headless" => options.headless = true,
                "--software-renderer" => options.software_renderer = true,
                unknown => {
                    return Err(format!(
                        "unknown launch argument `{unknown}`; {LAUNCH_USAGE}"
                    ));
                }
            }
        }
        Ok(options)
    }
}

/// Takes the value that must follow `flag`.
fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} needs a value; {LAUNCH_USAGE}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<LaunchOptions, String> {
        LaunchOptions::parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn no_arguments_give_the_defaults() {
        assert_eq!(parse(&[]), Ok(LaunchOptions::default()));
    }

    #[test]
    fn reads_commands_file_and_seed_in_any_order() {
        let expected = LaunchOptions {
            commands_file: Some(PathBuf::from("scripts/verify/a.cmds")),
            seed: 42,
            ..LaunchOptions::default()
        };
        assert_eq!(
            parse(&["--commands", "scripts/verify/a.cmds", "--seed", "42"]),
            Ok(expected.clone())
        );
        assert_eq!(
            parse(&["--seed", "42", "--commands", "scripts/verify/a.cmds"]),
            Ok(expected)
        );
    }

    #[test]
    fn reads_headless_and_software_renderer_switches() {
        let options = parse(&["--headless", "--software-renderer"]).expect("valid arguments");
        assert!(options.headless);
        assert!(options.software_renderer);
        assert_eq!(options.commands_file, None);
    }

    #[test]
    fn flag_without_value_is_an_error() {
        let error = parse(&["--commands"]).unwrap_err();
        assert!(error.contains("--commands needs a value"), "{error}");
    }

    #[test]
    fn seed_that_is_not_a_number_is_an_error() {
        let error = parse(&["--seed", "-1"]).unwrap_err();
        assert!(
            error.contains("--seed `-1` is not a whole number"),
            "{error}"
        );
    }

    #[test]
    fn unknown_argument_is_an_error_with_usage() {
        let error = parse(&["--fast"]).unwrap_err();
        assert!(
            error.contains("unknown launch argument `--fast`"),
            "{error}"
        );
        assert!(error.contains("usage:"), "{error}");
    }
}
