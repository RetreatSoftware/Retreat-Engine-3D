//! Retreat Engine Sandbox: the app used to see and verify engine features.
//!
//! Launch options: native `--commands <script.cmds>` / `--seed <n>` / `--headless` /
//! `--software-renderer`; in the browser the page URL `?commands=<path>&seed=<n>` (see
//! `retreat_engine::LaunchOptions`). The same `main` serves both: `retreat_engine::launch`
//! hides the platform difference.

mod drop_box;
mod greeting;
mod pickup;
mod scene;
mod stripes_material;
mod tally;
mod toolbar;
mod walker;
mod walker_combat;
mod walker_rocket;

use bevy::prelude::*;
use retreat_engine_api::AppNetExt;

const WINDOW_TITLE: &str = "Retreat Engine Sandbox";

/// Returns the app's exit status, so a failing command script fails the process (native).
fn main() -> AppExit {
    retreat_engine::launch(WINDOW_TITLE, add_sandbox_plugins)
}

/// Everything the sandbox adds on top of the engine.
fn add_sandbox_plugins(app: &mut App) {
    // What a client may ask a server of this app to do (`--server`): spawn and push boxes.
    app.allow_client_command("physics.spawn_box")
        .allow_client_command("physics.push");
    // A demo pickup without a body (`sandbox.pickup_spawn`); its own call because Bevy's plugin
    // tuples hold at most 15 entries.
    app.add_plugins(pickup::PickupPlugin);
    // A demo text field (`greeting` panel, `sandbox.greet`).
    app.add_plugins(greeting::GreetingPlugin);
    app.add_plugins((
        scene::SandboxScenePlugin,
        stripes_material::StripesMaterialPlugin,
        drop_box::DropBoxPlugin,
        toolbar::SandboxToolbarPlugin,
        // A demo of module state: `sandbox.tally_add`, rolled back and replicated.
        tally::TallyPlugin,
        // A demo character: analog input, ground cast, character body (`sandbox.walker_spawn`).
        walker::WalkerPlugin,
        // A demo shot: lag compensation, health, hiding, readouts (`sandbox.walker_fire`).
        walker_combat::WalkerCombatPlugin,
        walker_rocket::WalkerRocketPlugin,
        // The first gameplay module (Retreat-Modules): `spawner.rain <count>`.
        retreat_module_spawner::SpawnerModulePlugin,
        // The orbit camera module: `orbit.start`, arrow keys held rotate the sandbox camera.
        retreat_module_orbit_camera::OrbitCameraModulePlugin,
        // The sfx module: `sfx.play`, `sfx.music`, key P beeps.
        retreat_module_sfx::SfxModulePlugin,
        // The fox walk module: `fox.set_speed <model> <speed>` picks and blends clips.
        retreat_module_fox_walk::FoxWalkModulePlugin,
        // The highscore module: `score.add|save|load`, a versioned save through SaveStore.
        retreat_module_highscore::HighscoreModulePlugin,
        // The menu module: a pause menu (UI toolkit panel) toggled with Escape.
        retreat_module_menu::MenuModulePlugin,
        // The players module: a body per connected client, moved with held arrow keys (networked play).
        retreat_module_players::PlayersModulePlugin,
    ));
}
