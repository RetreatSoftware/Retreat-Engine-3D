//! The `snapshot <name>` command: saves what the main camera shows as
//! `target/snapshots/<name>.png`, from the window or (with `--headless`) from the offscreen image.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult};

use crate::PendingCommandWork;
use crate::platform::HAS_FILE_SYSTEM;

/// Where snapshots are written, relative to the working directory. `cargo run` from the
/// workspace root puts them next to the build output, which git ignores.
pub const SNAPSHOT_DIRECTORY: &str = "target/snapshots";

/// What `snapshot` captures.
#[derive(Resource, Debug, Clone)]
pub enum SnapshotSource {
    /// The game window (normal runs).
    PrimaryWindow,
    /// The image cameras render to with `--headless`.
    OffscreenImage(Handle<Image>),
}

/// Registers `snapshot`. Needs Bevy's rendering (`DefaultPlugins`) and a [`SnapshotSource`].
pub(crate) struct SnapshotPlugin;

impl Plugin for SnapshotPlugin {
    fn build(&self, app: &mut App) {
        app.register_command("snapshot", "snapshot <name>", snapshot_command);
    }
}

/// Turns a snapshot name into its file path. A name is letters, digits, `_` and `-` only,
/// so a script can never write outside the snapshot directory.
pub fn snapshot_file_path(name: &str) -> Result<PathBuf, String> {
    let is_valid_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    if name.is_empty() || !name.chars().all(is_valid_char) {
        return Err(format!(
            "snapshot name `{name}` is invalid; use only letters, digits, `_` and `-` \
             (the file becomes {SNAPSHOT_DIRECTORY}/<name>.png)"
        ));
    }
    Ok(Path::new(SNAPSHOT_DIRECTORY).join(format!("{name}.png")))
}

/// `snapshot <name>`: requests a capture of the [`SnapshotSource`]. The GPU captures it on a
/// later frame; a script waits (through [`PendingCommandWork`]) until the file is written.
fn snapshot_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    if !HAS_FILE_SYSTEM {
        return Err(args.failure(
            "snapshot files need a file system; in the browser use `__retreat.snapshot()` instead",
        ));
    }
    let path = snapshot_file_path(&name).map_err(|message| args.failure(message))?;
    prepare_snapshot_file(&path).map_err(|message| args.failure(message))?;

    let screenshot = match world.get_resource::<SnapshotSource>() {
        Some(SnapshotSource::PrimaryWindow) => Screenshot::primary_window(),
        Some(SnapshotSource::OffscreenImage(image)) => Screenshot::image(image.clone()),
        None => {
            return Err(args.failure(
                "no SnapshotSource; snapshots need RetreatEnginePlugin (rendering), not only RetreatCommandsPlugin",
            ));
        }
    };
    world.resource_mut::<PendingCommandWork>().start();
    world
        .spawn(screenshot)
        .observe(save_snapshot_and_report(path));
    Ok(CommandOutcome::Continue)
}

/// Creates the snapshot directory and removes an old file with the same name, so that
/// "the file exists afterwards" proves this run wrote it.
fn prepare_snapshot_file(path: &Path) -> Result<(), String> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|error| {
            format!(
                "cannot create snapshot directory `{}`: {error}",
                directory.display()
            )
        })?;
    }
    if path.exists() {
        std::fs::remove_file(path).map_err(|error| {
            format!("cannot replace old snapshot `{}`: {error}", path.display())
        })?;
    }
    Ok(())
}

/// The observer that runs when the GPU capture is ready: saves the PNG with Bevy's
/// `save_to_disk`, then reports success or failure to [`PendingCommandWork`].
fn save_snapshot_and_report(
    path: PathBuf,
) -> impl FnMut(On<ScreenshotCaptured>, ResMut<PendingCommandWork>) {
    let mut save = save_to_disk(path.clone());
    move |captured: On<ScreenshotCaptured>, mut pending: ResMut<PendingCommandWork>| {
        save(captured);
        if path.is_file() {
            info!("snapshot saved: {}", path.display());
            pending.finish();
        } else {
            pending.fail(format!(
                "snapshot `{}` was captured but could not be saved (see the error logged above)",
                path.display()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_name_maps_into_the_snapshot_directory() {
        let path = snapshot_file_path("sandbox_basic-1").expect("valid name");
        assert_eq!(path, Path::new("target/snapshots/sandbox_basic-1.png"));
    }

    #[test]
    fn names_that_could_escape_the_directory_are_rejected() {
        for name in ["", "../x", "a/b", "a b", "C:x", "shot.png"] {
            let error = snapshot_file_path(name).unwrap_err();
            assert!(error.contains("is invalid"), "{name:?}: {error}");
        }
    }
}
