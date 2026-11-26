//! One-shot looks: an explosion that shows once, however often the simulation replays it.
//!
//! A module calls [`VisualEffects::play`] from the **simulation** (a command or a `FixedUpdate`
//! system), replays included. The engine shows each `id` once per machine (it remembers ids for
//! [`EFFECT_MEMORY_TICKS`] live ticks), so a rewind that runs the explosion again, or a correction
//! that discovers it late, does not repeat or lose it. Derive the id from what exploded
//! (`<projectile name>/boom`). An effect predicted by mistake cannot be taken back.
//!
//! An effect is a glowing sphere that grows from nothing to `radius` over `ticks` ticks and
//! vanishes. It is render only: not synced, not hashed, hits nothing. Its sound is a
//! [`crate::SoundEvent`] played from the same place, which follows the same once-per-id rule.

use bevy::prelude::*;

/// How long an effect id is remembered (longer than any rewind).
pub const EFFECT_MEMORY_TICKS: u64 = 1024;

/// When a client shows an effect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EffectTime {
    /// At once: the player's own actions, drawn at the predicted present.
    #[default]
    Present,
    /// When the view time of [`crate::RemoteInterpolation`] reaches the tick that asked for it:
    /// for explosions of entities drawn that way. Without a view time it shows at once.
    RemoteView,
}

/// One effect (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct VisualEffect {
    pub id: String,
    pub position: Vec3,
    /// The radius it grows to.
    pub radius: f32,
    pub color: Color,
    /// As [`crate::SphereVisual::glow`].
    pub glow: f32,
    /// How long it lasts, in fixed ticks.
    pub ticks: u32,
    pub time: EffectTime,
}

/// Effects asked for since the engine last took them.
#[derive(Resource, Default, Debug)]
pub struct VisualEffects {
    pub requests: Vec<VisualEffect>,
}

impl VisualEffects {
    /// Asks for `effect` to be shown (once per id).
    pub fn play(&mut self, effect: VisualEffect) {
        self.requests.push(effect);
    }
}
