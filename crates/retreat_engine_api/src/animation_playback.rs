//! How game code and modules ask a model to play animation clips, without naming Bevy's
//! animation types.
//!
//! Put an [`AnimationPlayback`] on the entity of a glTF model (the one `model.spawn` creates).
//! It lists the clips that should play right now, each with a weight, a speed and whether it
//! loops; the engine plays exactly that. One clip is a plain animation; two or more clips with
//! weights are a blend (a walk that fades into a run). A module changes what plays by editing
//! the list, it never touches Bevy's `AnimationPlayer` or `AnimationGraph`.
//!
//! Time is simulation time: the engine advances every clip by the fixed tick (and frozen
//! while a command script holds), so a script gets the same pose on the same tick on every
//! machine and can assert it (`assert.animation_time`).

use bevy::prelude::*;

/// Largest clip speed the engine accepts (1.0 is the speed the clip was made for).
pub const MAX_CLIP_SPEED: f32 = 4.0;

/// One clip of a model's current animation.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipPlayback {
    /// The clip's name in the glTF file (`Walk`). `animation.list <model>` shows them.
    pub clip: String,
    /// How strongly the clip counts against the others, from 0 to 1. A single clip uses 1.
    pub weight: f32,
    /// Playback speed from 0 (frozen) to [`MAX_CLIP_SPEED`].
    pub speed: f32,
    /// Start again when it ends. A clip that does not loop is removed from the list at its
    /// end and the model keeps the last pose.
    pub looping: bool,
}

impl ClipPlayback {
    /// A clip that plays once at full weight and normal speed.
    pub fn once(clip: impl Into<String>) -> Self {
        Self {
            clip: clip.into(),
            weight: 1.0,
            speed: 1.0,
            looping: false,
        }
    }

    /// A clip that loops at full weight and normal speed.
    pub fn looping(clip: impl Into<String>) -> Self {
        Self {
            looping: true,
            ..Self::once(clip)
        }
    }
}

/// The clips a model plays now. An empty list means the model holds its current pose.
/// A clip's own time starts when the clip first appears in the list and keeps running while it
/// stays there, so changing weights every tick (a blend driven by speed) never restarts it.
#[derive(Component, Clone, Debug, PartialEq, Default)]
pub struct AnimationPlayback {
    pub clips: Vec<ClipPlayback>,
}

impl AnimationPlayback {
    /// `Ok` when the list makes sense, otherwise a message saying what is wrong. Whether each
    /// clip exists in the model's file is checked by the engine, which knows the file.
    pub fn validate(&self) -> Result<(), String> {
        for (index, clip) in self.clips.iter().enumerate() {
            if clip.clip.trim().is_empty() {
                return Err("an animation clip name must not be empty".to_string());
            }
            if !(clip.weight.is_finite() && (0.0..=1.0).contains(&clip.weight)) {
                return Err(format!(
                    "weight of clip `{}` must be from 0 to 1, got {}",
                    clip.clip, clip.weight
                ));
            }
            if !(clip.speed.is_finite() && (0.0..=MAX_CLIP_SPEED).contains(&clip.speed)) {
                return Err(format!(
                    "speed of clip `{}` must be from 0 to {MAX_CLIP_SPEED}, got {}",
                    clip.clip, clip.speed
                ));
            }
            if self.clips[..index]
                .iter()
                .any(|other| other.clip == clip.clip)
            {
                return Err(format!(
                    "clip `{}` is listed twice; list each clip once",
                    clip.clip
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_looping_weight_and_speed() {
        let once = ClipPlayback::once("Walk");
        assert!(!once.looping);
        assert_eq!((once.weight, once.speed), (1.0, 1.0));
        let looping = ClipPlayback::looping("Walk");
        assert!(looping.looping);
        assert_eq!(looping.clip, "Walk");
    }

    #[test]
    fn a_valid_blend_passes() {
        let mut blend = AnimationPlayback::default();
        assert_eq!(
            blend.validate(),
            Ok(()),
            "an empty list is valid (hold the pose)"
        );
        blend.clips = vec![
            ClipPlayback {
                weight: 0.25,
                ..ClipPlayback::looping("Walk")
            },
            ClipPlayback {
                weight: 0.75,
                speed: 2.0,
                ..ClipPlayback::looping("Run")
            },
        ];
        assert_eq!(blend.validate(), Ok(()));
    }

    #[test]
    fn bad_weights_speeds_names_and_duplicates_are_rejected() {
        let check = |clips: Vec<ClipPlayback>| AnimationPlayback { clips }.validate().unwrap_err();
        let mut heavy = ClipPlayback::once("Walk");
        heavy.weight = 1.5;
        assert!(check(vec![heavy]).contains("weight of clip `Walk` must be from 0 to 1, got 1.5"));
        let mut fast = ClipPlayback::once("Walk");
        fast.speed = f32::NAN;
        assert!(check(vec![fast]).contains("speed of clip `Walk` must be from 0 to 4"));
        assert!(check(vec![ClipPlayback::once("  ")]).contains("must not be empty"));
        let twice = check(vec![
            ClipPlayback::once("Walk"),
            ClipPlayback::looping("Walk"),
        ]);
        assert!(twice.contains("clip `Walk` is listed twice"), "{twice}");
    }
}
