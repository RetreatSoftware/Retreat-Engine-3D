//! How game code and modules ask for a sound, without naming the audio library.
//!
//! An entity with a [`SoundPlayback`] and a unique `Name` is a sound that is playing: the engine
//! starts it (today with Bevy's audio, through the browser's Web Audio on the web) and removes
//! the entity when a one-shot sound has finished, so "the entity exists" is the logical
//! answer to "is it playing?". That answer is computed from simulation time, never from the
//! sound card, so a command script can assert what is playing (`assert.audio_playing <name>`)
//! on a machine with no speakers and get the same result on every platform. To stop a sound,
//! despawn its entity (or run `audio.stop <name>`).
//!
//! Browser note: browsers keep audio silent until the player clicks or presses a key on the page.
//! Nothing here changes; sounds are still "playing" logically and become audible after the
//! first gesture.

use bevy::prelude::*;

/// Largest volume the engine accepts (1.0 is the file's own level).
pub const MAX_SOUND_VOLUME: f32 = 2.0;

/// What a sound is for. The engine plays at most one [`SoundKind::Music`] at a time: starting
/// music stops the music that was playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundKind {
    /// A short sound that plays once (a click, a hit) and may overlap with others.
    Effect,
    /// A long track that replaces the previous music.
    Music,
}

/// A sound to play. Read once, in the frame it is added, except `volume`, which can be changed
/// while it plays.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct SoundPlayback {
    /// File inside the app's assets folder (`sounds/beep.wav`); `.wav` and `.ogg` are accepted.
    pub path: String,
    /// Linear volume from 0.0 (silent) to [`MAX_SOUND_VOLUME`]; 1.0 is the file's own level.
    pub volume: f32,
    /// Play again from the start when it ends, until stopped.
    pub looping: bool,
    pub kind: SoundKind,
}

impl SoundPlayback {
    /// A one-shot effect at full volume.
    pub fn effect(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            volume: 1.0,
            looping: false,
            kind: SoundKind::Effect,
        }
    }

    /// Looping music at full volume.
    pub fn music(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            volume: 1.0,
            looping: true,
            kind: SoundKind::Music,
        }
    }

    /// `Ok` when the request makes sense, otherwise a message saying what is wrong.
    pub fn validate(&self) -> Result<(), String> {
        if self.path.trim().is_empty() {
            return Err("sound path must not be empty".to_string());
        }
        if !(self.volume.is_finite() && (0.0..=MAX_SOUND_VOLUME).contains(&self.volume)) {
            return Err(format!(
                "sound volume must be from 0 to {MAX_SOUND_VOLUME}, got {}",
                self.volume
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_kind_and_looping() {
        let effect = SoundPlayback::effect("sounds/beep.wav");
        assert_eq!(effect.kind, SoundKind::Effect);
        assert!(!effect.looping);
        let music = SoundPlayback::music("sounds/tone.wav");
        assert_eq!(music.kind, SoundKind::Music);
        assert!(music.looping);
        assert_eq!(music.volume, 1.0);
    }

    #[test]
    fn volume_and_path_are_validated() {
        assert_eq!(SoundPlayback::effect("a.wav").validate(), Ok(()));
        let mut sound = SoundPlayback::effect("a.wav");
        for volume in [-0.1, 2.5, f32::NAN, f32::INFINITY] {
            sound.volume = volume;
            let error = sound.validate().unwrap_err();
            assert!(error.contains("volume must be from 0 to 2"), "{error}");
        }
        assert!(
            SoundPlayback::effect("  ")
                .validate()
                .unwrap_err()
                .contains("path must not be empty")
        );
    }
}
