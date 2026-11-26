//! One-shot sounds from the simulation, and sounds placed in the world.
//!
//! **Events.** A module calls [`SoundEvents::play`] from the **simulation** (a command or a
//! `FixedUpdate` system), replays included. The engine plays each `id` once per machine, exactly like
//! [`crate::VisualEffects`] (same memory, same [`EffectTime`]), so a rewind that runs a shot again, or a
//! correction that discovers it late, neither repeats nor loses its sound. Derive the id from what
//! happened (`<projectile name>/boom`, `<shooter>/shot/<count>`). A sound predicted by mistake cannot
//! be taken back. Playing never changes simulation state.
//!
//! The sound of an event is an entity named `sound/<id>` with a [`crate::SoundPlayback`], so
//! `assert.audio_playing sound/<id>` works. Output is muted under command scripts and `--headless`;
//! the browser stays silent until the first click or key press (both: `sound_playback.rs`).
//!
//! **Position.** A sound with a [`SoundPosition`] (set through [`SoundEvent::at`], or added to any
//! looping sound entity and updated as its source moves) is quieter with distance and comes from its
//! direction. The listener is the [`crate::GameCamera`] (the first by name). The numbers are the pure
//! [`spatial_mix`]: full volume up to [`SOUND_FULL_VOLUME_DISTANCE`], falling to silence at `reach`;
//! direction is left/right only (no front/back or height cue).

use bevy::prelude::*;

use crate::EffectTime;

/// Within this distance of the listener a positional sound has its full volume (a third-person
/// camera sits a few units behind its own character).
pub const SOUND_FULL_VOLUME_DISTANCE: f32 = 4.0;
/// A reach that suits a small arena.
pub const DEFAULT_SOUND_REACH: f32 = 50.0;

/// Where a sound comes from.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct SoundPosition {
    pub position: Vec3,
    /// The distance at which it can no longer be heard (above [`SOUND_FULL_VOLUME_DISTANCE`]).
    pub reach: f32,
}

impl SoundPosition {
    /// `Ok` when the numbers make sense, otherwise a message saying what is wrong.
    pub fn validate(&self) -> Result<(), String> {
        if !self.position.is_finite() {
            return Err(format!(
                "sound position must be finite, got {}",
                self.position
            ));
        }
        if !(self.reach.is_finite() && self.reach > SOUND_FULL_VOLUME_DISTANCE) {
            return Err(format!(
                "sound reach must be above {SOUND_FULL_VOLUME_DISTANCE}, got {}",
                self.reach
            ));
        }
        Ok(())
    }
}

/// One sound to play once (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct SoundEvent {
    pub id: String,
    /// As [`crate::SoundPlayback::path`].
    pub path: String,
    /// As [`crate::SoundPlayback::volume`], before distance.
    pub volume: f32,
    /// `None`: heard the same everywhere (the player's own interface sounds).
    pub at: Option<SoundPosition>,
    /// As for effects: [`EffectTime::RemoteView`] for things drawn in the past.
    pub time: EffectTime,
}

/// Sounds asked for since the engine last took them.
#[derive(Resource, Default, Debug)]
pub struct SoundEvents {
    pub requests: Vec<SoundEvent>,
}

impl SoundEvents {
    /// Asks for `event` to be heard (once per id).
    pub fn play(&mut self, event: SoundEvent) {
        self.requests.push(event);
    }
}

/// Where the listener is and which way is its right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundListener {
    pub position: Vec3,
    /// Unit vector towards the listener's right.
    pub right: Vec3,
}

/// How a positional sound reaches the listener.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundMix {
    /// Volume factor from distance, 0 to 1.
    pub gain: f32,
    /// From -1 (fully left) over 0 (ahead, behind or at the listener) to 1 (fully right).
    pub pan: f32,
}

/// The distance and direction rule (pure). Gain falls with the square of the remaining part of the
/// way from [`SOUND_FULL_VOLUME_DISTANCE`] to the reach.
pub fn spatial_mix(listener: &SoundListener, source: &SoundPosition) -> SoundMix {
    let offset = source.position - listener.position;
    let distance = offset.length();
    let span = (source.reach - SOUND_FULL_VOLUME_DISTANCE).max(f32::EPSILON);
    let left_to_go = ((source.reach - distance) / span).clamp(0.0, 1.0);
    let pan = offset.try_normalize().map_or(0.0, |direction| {
        direction.dot(listener.right).clamp(-1.0, 1.0)
    });
    SoundMix {
        gain: left_to_go * left_to_go,
        pan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EARS: SoundListener = SoundListener {
        position: Vec3::ZERO,
        right: Vec3::X,
    };

    fn mix(x: f32, z: f32) -> SoundMix {
        spatial_mix(
            &EARS,
            &SoundPosition {
                position: Vec3::new(x, 0.0, z),
                reach: 24.0,
            },
        )
    }

    #[test]
    fn near_sounds_are_full_and_far_ones_fade_to_silence() {
        assert_eq!(mix(0.0, -1.0).gain, 1.0);
        assert_eq!(mix(0.0, -4.0).gain, 1.0);
        assert_eq!(mix(0.0, -14.0).gain, 0.25);
        assert_eq!(mix(0.0, -24.0).gain, 0.0);
        assert_eq!(mix(0.0, -90.0).gain, 0.0);
    }

    #[test]
    fn direction_is_left_or_right_of_the_listener() {
        assert_eq!(mix(5.0, 0.0).pan, 1.0);
        assert_eq!(mix(-5.0, 0.0).pan, -1.0);
        assert_eq!(mix(0.0, -5.0).pan, 0.0);
        assert_eq!(mix(0.0, 0.0).pan, 0.0);
    }

    #[test]
    fn positions_are_validated() {
        let mut at = SoundPosition {
            position: Vec3::ZERO,
            reach: DEFAULT_SOUND_REACH,
        };
        assert_eq!(at.validate(), Ok(()));
        at.reach = 1.0;
        assert!(at.validate().unwrap_err().contains("reach must be above"));
        at.position.x = f32::NAN;
        assert!(at.validate().unwrap_err().contains("must be finite"));
    }
}
