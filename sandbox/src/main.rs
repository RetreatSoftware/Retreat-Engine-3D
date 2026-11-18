//! Retreat Engine Sandbox: the app used to see and verify engine features.
//!
//! Launch options: native `--commands <script.cmds>` / `--seed <n>` / `--headless` /
//! `--software-renderer`; in the browser the page URL `?commands=<path>&seed=<n>` (see
//! `retreat_engine::LaunchOptions`). The same `main` serves both: `retreat_engine::launch`
//! hides the platform difference.

mod drop_box;
mod scene;
mod stripes_material;
mod toolbar;

use bevy::prelude::*;

const WINDOW_TITLE: &str = "Retreat Engine Sandbox";

/// Returns the app's exit status, so a failing command script fails the process (native).
fn main() -> AppExit {
    retreat_engine::launch(WINDOW_TITLE, add_sandbox_plugins)
}

/// Everything the sandbox adds on top of the engine.
fn add_sandbox_plugins(app: &mut App) {
    app.add_plugins((
        scene::SandboxScenePlugin,
        stripes_material::StripesMaterialPlugin,
        drop_box::DropBoxPlugin,
        toolbar::SandboxToolbarPlugin,
        // The first gameplay module (Retreat-Modules): `spawner.rain <count>`.
        retreat_module_spawner::SpawnerModulePlugin,
        // The orbit camera module: `orbit.start`, arrow keys held rotate the sandbox camera.
        retreat_module_orbit_camera::OrbitCameraModulePlugin,
        // The sfx module: `sfx.play`, `sfx.music`, key P beeps.
        retreat_module_sfx::SfxModulePlugin,
    ));
}
