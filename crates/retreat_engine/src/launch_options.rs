//! Command-line arguments every Retreat app accepts.

use std::path::PathBuf;

/// The seed used when `--seed` is not given.
pub const DEFAULT_SEED: u64 = 0;

/// Shown after every launch-argument error.
const LAUNCH_USAGE: &str = "usage: <app> [--commands <script.cmds>] [--seed <whole number>] [--headless [--no-render]] [--software-renderer] [--real-time] [--sound] [--server <port> | --connect <https://host:port> [--cert-hash <64 hex digits>]]";

/// Whether and how the app takes part in a network game.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NetLaunch {
    /// Single player.
    #[default]
    Off,
    /// `--server <port>`: the authoritative server, listening on that UDP port (0 picks a free one).
    Server { port: u16 },
    /// `--connect <url>`: a predicting client. `cert_hash` pins a self-signed certificate (as printed
    /// by the server); without it the system's certificate authorities are used.
    Client {
        url: String,
        cert_hash: Option<String>,
    },
}

/// What the app was asked to do at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    /// `--commands <file>`: a command script to run, one line per frame.
    pub commands_file: Option<PathBuf>,
    /// `--seed <n>`: the seed of the `SimulationRng`.
    pub seed: u64,
    /// `--headless`: no window; cameras render to an offscreen image (native only).
    pub headless: bool,
    /// `--no-render` (needs `--headless`): no GPU at all, nothing is drawn and `snapshot` fails. For
    /// processes that only simulate (a server, a network-check client): without the render cost
    /// they tick at the full fixed rate even on a machine without a GPU.
    pub no_render: bool,
    /// `--software-renderer`: use the CPU fallback GPU adapter (WARP on Windows, lavapipe on
    /// Linux), so rendering works on machines without a GPU, such as CI.
    pub software_renderer: bool,
    /// `--real-time`: a command script runs one line per fixed tick of *real* time instead of as fast
    /// as frames come (needed when other machines must keep up, as in network checks).
    pub real_time: bool,
    /// `--sound` / `?sound=1`: play real sound even when a script is loaded or `--headless` is set.
    pub sound: bool,
    /// `--server`, `--connect`, `--cert-hash`.
    pub net: NetLaunch,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            commands_file: None,
            seed: DEFAULT_SEED,
            headless: false,
            no_render: false,
            software_renderer: false,
            real_time: false,
            sound: false,
            net: NetLaunch::Off,
        }
    }
}

impl LaunchOptions {
    /// True when the sound device must stay silent: a command script is loaded or `--headless` is
    /// set (verify runs), unless `--sound` asks for real playback. Interactive play is audible.
    /// Only the output is muted; the logical audio state is unchanged. The one rule for this.
    pub fn sound_muted(&self) -> bool {
        !self.sound && (self.commands_file.is_some() || self.headless)
    }

    /// Reads the process arguments. In the browser there are none, so this gives the defaults.
    pub fn from_env_args() -> Result<Self, String> {
        // The first argument is the program's own path, not an option.
        Self::parse(std::env::args().skip(1))
    }

    /// Parses arguments (without the program path). Pure, so it is tested directly.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut options = Self::default();
        let mut cert_hash: Option<String> = None;
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
                "--no-render" => options.no_render = true,
                "--software-renderer" => options.software_renderer = true,
                "--real-time" => options.real_time = true,
                "--sound" => options.sound = true,
                "--server" => {
                    let value = next_value(&mut args, "--server")?;
                    let port = value.parse().map_err(|error| {
                        format!("--server `{value}` is not a UDP port from 0 to 65535: {error}; {LAUNCH_USAGE}")
                    })?;
                    options.net = NetLaunch::Server { port };
                }
                "--connect" => {
                    let url = next_value(&mut args, "--connect")?;
                    options.net = NetLaunch::Client {
                        url,
                        cert_hash: None,
                    };
                }
                "--cert-hash" => cert_hash = Some(next_value(&mut args, "--cert-hash")?),
                unknown => {
                    return Err(format!(
                        "unknown launch argument `{unknown}`; {LAUNCH_USAGE}"
                    ));
                }
            }
        }
        if options.no_render && !options.headless {
            return Err(format!(
                "--no-render only makes sense with --headless (a window needs the GPU); {LAUNCH_USAGE}"
            ));
        }
        match (&mut options.net, cert_hash) {
            (
                NetLaunch::Client {
                    cert_hash: slot, ..
                },
                Some(hash),
            ) => *slot = Some(hash),
            (_, Some(_)) => {
                return Err(format!(
                    "--cert-hash only makes sense with --connect; {LAUNCH_USAGE}"
                ));
            }
            _ => {}
        }
        Ok(options)
    }
}

/// Shown after every URL launch-option error (browser builds).
const URL_USAGE: &str = "usage: page URL `?commands=<path to .cmds>&seed=<whole number>&real_time=1&sound=1&connect=<https://host:port>&cert_hash=<64 hex digits>` (all optional)";

impl LaunchOptions {
    /// Reads the browser page's URL query, already split into decoded `(key, value)` pairs
    /// (`?commands=scripts/verify/a.cmds&seed=3`). The script path is fetched relative to the
    /// page. `--headless` and `--software-renderer` have no browser meaning and are rejected.
    pub fn from_url_query_pairs(
        pairs: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, String> {
        let mut options = Self::default();
        let mut cert_hash: Option<String> = None;
        for (key, value) in pairs {
            match key.as_str() {
                "commands" => options.commands_file = Some(PathBuf::from(value)),
                "real_time" => options.real_time = matches!(value.as_str(), "1" | "true"),
                "sound" => options.sound = matches!(value.as_str(), "1" | "true"),
                "connect" => {
                    options.net = NetLaunch::Client {
                        url: value,
                        cert_hash: None,
                    };
                }
                "cert_hash" => cert_hash = Some(value),
                "seed" => {
                    options.seed = value.parse().map_err(|error| {
                        format!("seed `{value}` is not a whole number from 0 to {}: {error}; {URL_USAGE}", u64::MAX)
                    })?;
                }
                unknown => {
                    return Err(format!(
                        "unknown URL launch option `{unknown}`; {URL_USAGE}"
                    ));
                }
            }
        }
        match (&mut options.net, cert_hash) {
            (
                NetLaunch::Client {
                    cert_hash: slot, ..
                },
                Some(hash),
            ) => *slot = Some(hash),
            (_, Some(_)) => {
                return Err(format!(
                    "cert_hash only makes sense with connect; {URL_USAGE}"
                ));
            }
            _ => {}
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
    fn no_render_needs_headless() {
        let options = parse(&["--headless", "--no-render"]).expect("valid arguments");
        assert!(options.no_render);
        let error = parse(&["--no-render"]).unwrap_err();
        assert!(
            error.contains("--no-render only makes sense with --headless"),
            "{error}"
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

    fn url(pairs: &[(&str, &str)]) -> Result<LaunchOptions, String> {
        LaunchOptions::from_url_query_pairs(
            pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string())),
        )
    }

    #[test]
    fn url_query_gives_commands_and_seed() {
        let options =
            url(&[("commands", "scripts/verify/a.cmds"), ("seed", "7")]).expect("valid query");
        assert_eq!(
            options.commands_file,
            Some(PathBuf::from("scripts/verify/a.cmds"))
        );
        assert_eq!(options.seed, 7);
        assert_eq!(url(&[]), Ok(LaunchOptions::default()));
    }

    #[test]
    fn url_query_rejects_unknown_keys_and_bad_seeds() {
        let error = url(&[("headless", "1")]).unwrap_err();
        assert!(
            error.contains("unknown URL launch option `headless`"),
            "{error}"
        );
        let error = url(&[("seed", "x")]).unwrap_err();
        assert!(error.contains("seed `x` is not a whole number"), "{error}");
    }

    #[test]
    fn sound_is_muted_for_scripts_and_headless_unless_asked() {
        assert!(!parse(&[]).unwrap().sound_muted());
        assert!(parse(&["--commands", "a.cmds"]).unwrap().sound_muted());
        assert!(parse(&["--headless"]).unwrap().sound_muted());
        assert!(
            !parse(&["--commands", "a.cmds", "--sound"])
                .unwrap()
                .sound_muted()
        );
        assert!(!parse(&["--headless", "--sound"]).unwrap().sound_muted());
        assert!(parse(&["--sound"]).unwrap().sound);
    }

    #[test]
    fn url_sound_option_turns_real_playback_on() {
        let muted = url(&[("commands", "a.cmds")]).unwrap();
        assert!(muted.sound_muted());
        let loud = url(&[("commands", "a.cmds"), ("sound", "1")]).unwrap();
        assert!(loud.sound && !loud.sound_muted());
        assert!(!url(&[("sound", "0")]).unwrap().sound);
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

    #[test]
    fn reads_the_network_options() {
        let server = parse(&["--server", "4433", "--real-time"]).unwrap();
        assert_eq!(server.net, NetLaunch::Server { port: 4433 });
        assert!(server.real_time);
        let client = parse(&["--connect", "https://127.0.0.1:4433", "--cert-hash", "ab"]).unwrap();
        assert_eq!(
            client.net,
            NetLaunch::Client {
                url: "https://127.0.0.1:4433".to_string(),
                cert_hash: Some("ab".to_string())
            }
        );
        // The order of --connect and --cert-hash does not matter.
        let reversed = parse(&["--cert-hash", "ab", "--connect", "https://h:1"]).unwrap();
        assert!(matches!(
            reversed.net,
            NetLaunch::Client {
                cert_hash: Some(_),
                ..
            }
        ));
        assert_eq!(parse(&[]).unwrap().net, NetLaunch::Off);
    }

    #[test]
    fn network_option_mistakes_are_explained() {
        let error = parse(&["--cert-hash", "ab"]).unwrap_err();
        assert!(
            error.contains("--cert-hash only makes sense with --connect"),
            "{error}"
        );
        let error = parse(&["--server", "port"]).unwrap_err();
        assert!(error.contains("not a UDP port"), "{error}");
        let error = parse(&["--connect"]).unwrap_err();
        assert!(error.contains("--connect needs a value"), "{error}");
    }

    #[test]
    fn reads_the_network_options_from_the_url() {
        let pairs = [
            ("connect", "https://127.0.0.1:4433"),
            ("cert_hash", "ab"),
            ("real_time", "1"),
        ]
        .map(|(key, value)| (key.to_string(), value.to_string()));
        let options = LaunchOptions::from_url_query_pairs(pairs).unwrap();
        assert!(options.real_time);
        assert_eq!(
            options.net,
            NetLaunch::Client {
                url: "https://127.0.0.1:4433".to_string(),
                cert_hash: Some("ab".to_string())
            }
        );
        let lonely = [("cert_hash".to_string(), "ab".to_string())];
        let error = LaunchOptions::from_url_query_pairs(lonely).unwrap_err();
        assert!(
            error.contains("cert_hash only makes sense with connect"),
            "{error}"
        );
    }
}
