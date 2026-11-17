//! Retreat engine runtime.
//!
//! A game adds [`RetreatEnginePlugin`] to its Bevy `App`. The plugin owns the
//! base Bevy setup (window, rendering, input, assets), so games and modules do
//! not configure Bevy's `DefaultPlugins` themselves.

use bevy::prelude::*;

/// Sets up the Bevy foundation every Retreat game runs on.
pub struct RetreatEnginePlugin {
    /// Title of the native window (ignored in the browser, where the page title is used).
    pub window_title: String,
}

impl RetreatEnginePlugin {
    pub fn new(window_title: impl Into<String>) -> Self {
        Self {
            window_title: window_title.into(),
        }
    }
}

impl Plugin for RetreatEnginePlugin {
    fn build(&self, app: &mut App) {
        let window_plugin = WindowPlugin {
            primary_window: Some(Window {
                title: self.window_title.clone(),
                ..default()
            }),
            ..default()
        };
        app.add_plugins(DefaultPlugins.set(window_plugin));
    }
}
