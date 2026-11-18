//! The one rule for file paths that commands load assets from (`model.spawn`,
//! `material.set_texture`, ...): relative paths start in the app's `assets` folder, `..` may
//! not leave it, and absolute paths work only where there is a file system (native).

use std::path::{Component, Path, PathBuf};

use bevy::asset::{Asset, AssetPath};
use bevy::prelude::*;

/// Where an asset file lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetLocation {
    /// A path inside the app's `assets` folder, e.g. `models/Fox.glb`. Works everywhere
    /// (in the browser the file is fetched over HTTP).
    AssetFolder(String),
    /// An absolute path anywhere on disk, e.g. `C:/art/tree.glb`. Native only.
    AbsoluteFile(PathBuf),
}

impl AssetLocation {
    /// The path in the form Bevy's `AssetServer` takes.
    pub fn into_asset_path(self) -> AssetPath<'static> {
        match self {
            AssetLocation::AssetFolder(path) => AssetPath::from(path),
            AssetLocation::AbsoluteFile(file) => AssetPath::from(file),
        }
    }

    /// True for files outside the assets folder, which Bevy calls "unapproved".
    pub fn is_outside_asset_folder(&self) -> bool {
        matches!(self, AssetLocation::AbsoluteFile(_))
    }
}

/// Decides where `path` points, or explains why it is not allowed. `kind` names the expected
/// file in errors ("glTF model"); `extensions` are the accepted endings, lowercase. Pure, so it
/// is tested directly; `has_file_system` is `platform::HAS_FILE_SYSTEM` outside tests.
pub fn resolve_asset_location(
    path: &str,
    kind: &str,
    extensions: &[&str],
    has_file_system: bool,
) -> Result<AssetLocation, String> {
    let file = Path::new(path);
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let has_expected_extension = extension
        .as_deref()
        .is_some_and(|extension| extensions.contains(&extension));
    if !has_expected_extension {
        return Err(format!(
            "`{path}` is not a {kind}; the file name must end in .{}",
            extensions.join(" or .")
        ));
    }
    if file.is_absolute() {
        return if has_file_system {
            Ok(AssetLocation::AbsoluteFile(file.to_path_buf()))
        } else {
            Err(format!(
                "`{path}` is an absolute path, but the browser has no file system; \
                 put the file in the assets folder and use a relative path"
            ))
        };
    }
    if file.components().any(|part| part == Component::ParentDir) {
        return Err(format!(
            "`{path}` leaves the assets folder with `..`; use a path inside the assets folder, \
             or an absolute path (native only)"
        ));
    }
    // Bevy asset paths always use `/`, also on Windows.
    Ok(AssetLocation::AssetFolder(path.replace('\\', "/")))
}

/// Starts loading `path`. Files outside the assets folder are "unapproved" for Bevy; the
/// engine sets `UnapprovedPathMode::Deny`, so only this explicit override may load them.
pub(crate) fn load_asset<A: Asset>(
    asset_server: &AssetServer,
    path: AssetPath<'static>,
    is_outside_asset_folder: bool,
) -> Handle<A> {
    if is_outside_asset_folder {
        asset_server.load_builder().override_unapproved().load(path)
    } else {
        asset_server.load(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &[&str] = &["glb", "gltf"];

    #[test]
    fn relative_paths_point_into_the_asset_folder() {
        assert_eq!(
            resolve_asset_location("models/Fox.glb", "glTF model", MODEL, true),
            Ok(AssetLocation::AssetFolder("models/Fox.glb".to_string()))
        );
        assert_eq!(
            resolve_asset_location("models\\Tree.GLTF", "glTF model", MODEL, false),
            Ok(AssetLocation::AssetFolder("models/Tree.GLTF".to_string()))
        );
    }

    #[test]
    fn absolute_paths_work_only_with_a_file_system() {
        let absolute = if cfg!(windows) {
            "C:/art/tree.glb"
        } else {
            "/art/tree.glb"
        };
        let location = resolve_asset_location(absolute, "glTF model", MODEL, true);
        assert_eq!(
            location,
            Ok(AssetLocation::AbsoluteFile(PathBuf::from(absolute)))
        );
        assert!(location.unwrap().is_outside_asset_folder());
        let error = resolve_asset_location(absolute, "glTF model", MODEL, false).unwrap_err();
        assert!(error.contains("browser has no file system"), "{error}");
    }

    #[test]
    fn paths_leaving_the_asset_folder_are_rejected() {
        let error =
            resolve_asset_location("../secrets/x.glb", "glTF model", MODEL, true).unwrap_err();
        assert!(error.contains("leaves the assets folder"), "{error}");
    }

    #[test]
    fn wrong_extensions_are_rejected_with_the_kind_and_the_list() {
        let error = resolve_asset_location("a/b.jpg", "PNG texture", &["png"], true).unwrap_err();
        assert_eq!(
            error,
            "`a/b.jpg` is not a PNG texture; the file name must end in .png"
        );
        for path in ["models/Fox.fbx", "models/Fox", "notes.txt"] {
            let error = resolve_asset_location(path, "glTF model", MODEL, true).unwrap_err();
            assert!(error.contains("is not a glTF model"), "{path}: {error}");
        }
    }
}
