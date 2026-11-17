//! Retreat Engine Sandbox: the app used to see and verify engine features.

mod scene;

use retreat_engine::RetreatEnginePlugin;
use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins((
            RetreatEnginePlugin::new("Retreat Engine Sandbox"),
            scene::SandboxScenePlugin,
        ))
        .run();
}
