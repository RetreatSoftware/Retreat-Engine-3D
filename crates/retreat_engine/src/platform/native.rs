//! Native (desktop, server, CI) side of the platform layer. See `mod.rs` for the contract.

use bevy::asset::AssetMetaCheck;
use bevy::prelude::*;

#[path = "native_transport.rs"]
pub mod native_transport;
#[path = "native_web_server.rs"]
mod native_web_server;

use crate::snapshot::snapshot_file_path;
use crate::{CommandScript, LaunchOptions, RetreatEnginePlugin};

/// Native audio output starts at once.
pub(crate) const AUDIO_NEEDS_USER_GESTURE: bool = false;

pub(crate) const HAS_FILE_SYSTEM: bool = true;

pub(crate) const SCRIPT_END_EXITS_APP: bool = true;

/// Reads the process arguments, reads the `--commands` file, builds the app with
/// `RetreatEnginePlugin` and the game's plugins, and runs it. Returns the app's exit status
/// (code 1 when a script fails), so `fn main() -> AppExit` passes it to the shell.
pub fn launch(window_title: &str, add_game_plugins: fn(&mut App)) -> AppExit {
    let prepared = LaunchOptions::from_env_args().and_then(|options| {
        let script = match &options.commands_file {
            Some(path) => Some(CommandScript::read_file(std::path::Path::new(path))?),
            None => None,
        };
        let engine = RetreatEnginePlugin::from_launch_options(window_title, &options, script);
        Ok((engine, options))
    });
    let (engine, options) = match prepared {
        Ok(prepared) => prepared,
        Err(message) => {
            eprintln!("{window_title}: {message}");
            return AppExit::error();
        }
    };
    let mut app = App::new();
    app.add_plugins(engine);
    add_game_plugins(&mut app);
    if let Err(message) = native_transport::install_net(&mut app, &options) {
        eprintln!("{window_title}: {message}");
        return AppExit::error();
    }
    app.run()
}

/// A normal desktop window with the given title.
pub(crate) fn primary_window(title: &str) -> Window {
    Window {
        title: title.to_string(),
        ..default()
    }
}

/// Native reads `.meta` files next to assets when they exist (Bevy's default).
pub(crate) fn asset_meta_check() -> AssetMetaCheck {
    AssetMetaCheck::Always
}

/// Writes `png` to `target/snapshots/<name>.png` (relative to the working directory) and
/// returns that path.
pub(crate) fn store_snapshot(name: &str, png: Vec<u8>) -> Result<String, String> {
    let path = snapshot_file_path(name)?;
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|error| {
            format!(
                "cannot create snapshot directory `{}`: {error}",
                directory.display()
            )
        })?;
    }
    std::fs::write(&path, png)
        .map_err(|error| format!("cannot write snapshot `{}`: {error}", path.display()))?;
    Ok(path.display().to_string())
}

/// Native has no platform-only systems.
pub(crate) struct PlatformPlugin;

impl Plugin for PlatformPlugin {
    fn build(&self, _app: &mut App) {}
}

/// Where native saves go, relative to the working directory: one `<module>.json` per module.
pub(crate) const SAVE_DIRECTORY: &str = "saves";

/// What `save.status` tells the player about where saves live.
pub(crate) const SAVE_LOCATION: &str = "files in the `saves` folder (working directory)";

/// The save backend of this platform: files in [`SAVE_DIRECTORY`].
pub(crate) fn platform_save_backend() -> Box<dyn retreat_engine_api::SaveBackend> {
    Box::new(FileSaveBackend::new(SAVE_DIRECTORY.into()))
}

/// Saves as `<directory>/<key>.json`. Keys are module ids (letters, digits, `_`, `-`), so they
/// are safe file names. A write goes to a temporary file first and is then renamed, so a crash
/// never leaves half a document.
pub(crate) struct FileSaveBackend {
    directory: std::path::PathBuf,
}

impl FileSaveBackend {
    pub(crate) fn new(directory: std::path::PathBuf) -> Self {
        Self { directory }
    }

    fn file_for(&self, key: &str) -> std::path::PathBuf {
        self.directory.join(format!("{key}.json"))
    }
}

impl retreat_engine_api::SaveBackend for FileSaveBackend {
    fn read(&self, key: &str) -> Result<Option<String>, String> {
        let file = self.file_for(key);
        match std::fs::read_to_string(&file) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("cannot read `{}`: {error}", file.display())),
        }
    }

    fn write(&mut self, key: &str, text: &str) -> Result<(), String> {
        std::fs::create_dir_all(&self.directory)
            .map_err(|error| format!("cannot create `{}`: {error}", self.directory.display()))?;
        let file = self.file_for(key);
        let temporary = self.directory.join(format!("{key}.json.tmp"));
        std::fs::write(&temporary, text)
            .and_then(|()| std::fs::rename(&temporary, &file))
            .map_err(|error| format!("cannot write `{}`: {error}", file.display()))
    }

    fn remove(&mut self, key: &str) -> Result<bool, String> {
        let file = self.file_for(key);
        match std::fs::remove_file(&file) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!("cannot delete `{}`: {error}", file.display())),
        }
    }

    fn keys(&self) -> Result<Vec<String>, String> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!(
                    "cannot list `{}`: {error}",
                    self.directory.display()
                ));
            }
        };
        let mut keys: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.strip_suffix(".json").map(String::from)
            })
            .collect();
        keys.sort();
        Ok(keys)
    }
}

/// A native window grants every cursor grab, so there is nothing to ask.
pub(crate) fn pointer_is_locked() -> Option<bool> {
    None
}

#[cfg(test)]
mod save_backend_tests {
    use super::*;
    use retreat_engine_api::SaveBackend;

    /// A fresh empty folder under the system temp folder, removed at the end of the test.
    struct TempDirectory(std::path::PathBuf);

    impl TempDirectory {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("retreat_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn files_round_trip_list_and_delete() {
        let directory = TempDirectory::new("save_files");
        let mut backend = FileSaveBackend::new(directory.0.clone());
        assert_eq!(backend.read("a"), Ok(None), "a missing folder is no error");
        assert_eq!(backend.keys(), Ok(Vec::new()));
        backend.write("b", "second").unwrap();
        backend.write("a", "first").unwrap();
        backend.write("a", "first again").unwrap();
        assert_eq!(backend.read("a"), Ok(Some("first again".to_string())));
        assert_eq!(backend.keys(), Ok(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(backend.remove("a"), Ok(true));
        assert_eq!(backend.remove("a"), Ok(false));
        assert!(
            !directory.0.join("a.json.tmp").exists(),
            "no temporary file is left"
        );
    }

    #[test]
    fn a_save_store_on_files_keeps_its_documents_across_stores() {
        use retreat_engine_api::{SaveStore, serde_json::json};
        let directory = TempDirectory::new("save_store");
        SaveStore::new(Box::new(FileSaveBackend::new(directory.0.clone())))
            .save("score", 1, &json!({"score": 7}))
            .unwrap();
        // A new store on the same folder is a new run of the game.
        let again = SaveStore::new(Box::new(FileSaveBackend::new(directory.0.clone())));
        assert_eq!(
            again.load("score").unwrap().unwrap().data,
            json!({"score": 7})
        );
    }
}
