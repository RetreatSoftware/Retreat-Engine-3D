//! The `snapshot <name>` command: captures what the main camera shows (the window, or with
//! `--headless` the offscreen image), encodes it as PNG and hands it to the platform layer:
//! native writes `target/snapshots/<name>.png`; the browser keeps it in memory, where
//! `__retreat.snapshot(name)` returns it.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use image::{DynamicImage, ImageFormat};
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult};

use crate::PendingCommandWork;
use crate::platform::store_snapshot;

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
/// later frame; a script waits (through [`PendingCommandWork`]) until it is stored.
fn snapshot_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    // Checks the name on every platform, so a script valid in the browser is valid natively.
    snapshot_file_path(&name).map_err(|message| args.failure(message))?;

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
        .observe(store_snapshot_and_report(name));
    Ok(CommandOutcome::Continue)
}

/// The observer that runs when the GPU capture is ready: encodes PNG, stores it through the
/// platform layer, then reports success or failure to [`PendingCommandWork`].
fn store_snapshot_and_report(
    name: String,
) -> impl FnMut(On<ScreenshotCaptured>, ResMut<PendingCommandWork>) {
    move |captured: On<ScreenshotCaptured>, mut pending: ResMut<PendingCommandWork>| {
        match encode_png(&captured.image).and_then(|png| store_snapshot(&name, png)) {
            Ok(location) => {
                info!("snapshot saved: {location}");
                pending.finish();
            }
            Err(message) => pending.fail(format!("snapshot `{name}`: {message}")),
        }
    }
}

/// PNG bytes of a captured frame, as 8-bit RGB (the alpha channel is dropped, as Bevy's own
/// `save_to_disk` does, because with HDR it holds brightness, not transparency).
fn encode_png(image: &Image) -> Result<Vec<u8>, String> {
    let dynamic = image
        .clone()
        .try_into_dynamic()
        .map_err(|error| format!("cannot convert the captured frame to an image: {error}"))?;
    if matches!(
        dynamic,
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_)
    ) {
        return Err(
            "the captured frame is floating-point (HDR); snapshots support 8-bit frames only"
                .to_string(),
        );
    }
    let mut png = Vec::new();
    DynamicImage::ImageRgb8(dynamic.to_rgb8())
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|error| format!("cannot encode PNG: {error}"))?;
    Ok(png)
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
