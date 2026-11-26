//! How a module changes what an entity looks like, without naming render types: color, a character's
//! model, facing and animation, and smoothing of what is drawn. The engine maps them
//! (`retreat_engine/src/visuals.rs`, `render_smoothing.rs`). **All are render only:** they change
//! nothing in the simulation, are not synced and are not rewound. A module derives them from synced
//! state on each machine, for example in a system on `Added<Player>` (a client creates replicated
//! entities from the server's state with no visuals but a plain shape).
//!
//! **Things without a physics body** (a pickup, a prop): give the synced entity a
//! [`crate::NetPose`] (its place, replicated, also to late joiners), then dress it on each machine with
//! [`BoxVisual`], [`SphereVisual`] or a model ([`CharacterVisual`] with `model: Some(..)`),
//! [`EntityColor`], [`VisualMotion`] to turn or bob, and [`HideVisuals`] while it is taken.

use bevy::prelude::*;

use crate::ClipPlayback;

/// A plain sphere drawn at the entity (a projectile without a physics body or a model). Render
/// only. [`EntityColor`] on the same entity overrides `color`.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
#[require(Transform, Visibility)]
pub struct SphereVisual {
    pub radius: f32,
    pub color: Color,
    /// Light the sphere gives off itself: 0 is lit by the scene only, 1 and more looks glowing.
    pub glow: f32,
}

/// A plain box drawn at the entity (a pickup). Render only. [`EntityColor`] on the same entity
/// overrides `color`.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
#[require(Transform, Visibility)]
pub struct BoxVisual {
    /// Width, height and depth.
    pub size: Vec3,
    pub color: Color,
    /// As [`SphereVisual::glow`].
    pub glow: f32,
}

/// Turns and bobs what is drawn for an entity ([`BoxVisual`], [`SphereVisual`], [`CharacterVisual`]),
/// around the entity's up axis. The entity itself does not move: casts, distances and the camera see
/// it where it is. Time is the fixed clock, so a script's picture is the same every run; machines need
/// not be in phase.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct VisualMotion {
    /// Radians per second about the up axis.
    pub turn: f32,
    /// How far above and below its place it moves.
    pub bob_height: f32,
    /// Seconds for one bob up and down; 0 does not bob.
    pub bob_seconds: f32,
}

impl VisualMotion {
    /// `Ok` when the numbers make sense, otherwise a message saying what is wrong.
    pub fn validate(&self) -> Result<(), String> {
        let numbers = [self.turn, self.bob_height, self.bob_seconds];
        if numbers.iter().all(|number| number.is_finite()) && self.bob_seconds >= 0.0 {
            Ok(())
        } else {
            Err(format!(
                "visual motion needs finite numbers and bob seconds of 0 or more, got {self:?}"
            ))
        }
    }

    /// Where `base` (relative to the entity) is `seconds` into the motion. Pure.
    pub fn applied(&self, base: Transform, seconds: f64) -> Transform {
        let full = std::f64::consts::TAU;
        let angle = (f64::from(self.turn) * seconds).rem_euclid(full) as f32;
        let lift = if self.bob_seconds > 0.0 {
            let phase = (seconds / f64::from(self.bob_seconds)).fract();
            self.bob_height * (full * phase).sin() as f32
        } else {
            0.0
        };
        let turn = Quat::from_rotation_y(angle);
        Transform {
            translation: turn * base.translation + Vec3::Y * lift,
            rotation: turn * base.rotation,
            scale: base.scale,
        }
    }
}

/// With [`EntityColor`]: replace the look instead of tinting it, by dropping the color texture (an
/// orange model can then be blue). Removing it does not bring the texture back.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntityColorFlat;

/// Not drawn (with everything below it) while present: a dead player. It stays in the simulation
/// and casts still hit it, so exclude it there.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HideVisuals;

/// On a client, draws **another** player's entity from the server's own past states, at
/// [`crate::NetViewTick`] (about 100 ms behind), instead of this machine's prediction: exactly
/// what [`crate::cast_ray_at`] judges shots against. The simulation is untouched. Without server
/// history (a server, single player) it does nothing.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[require(DrawnPose)]
pub struct RemoteInterpolation;

/// Where the engine draws the entity this frame (after smoothing or remote interpolation; the
/// plain `Transform` without them). Add it to read the position a camera should follow, in an
/// `Update` system `.after(DrawnPoseSystems)`. Read only.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct DrawnPose {
    pub translation: Vec3,
    pub rotation: Quat,
}

/// The `Update` set in which [`DrawnPose`] is written.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DrawnPoseSystems;

/// Colors the entity and everything below it (a model's parts too). A texture is tinted. Removing
/// the component keeps the last color.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct EntityColor(pub Color);

/// How a character looks: the engine draws it as a child of the entity, named `<name>/visual`,
/// instead of the entity's own mesh.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct CharacterVisual {
    /// A glTF model path in the app's assets (`models/Fox.glb`), or `None` for a plain shape: the
    /// body's shape with a nose block towards -Z that shows the facing.
    pub model: Option<String>,
    /// Uniform scale of the model (1 for the plain shape).
    pub scale: f32,
    /// Where the visual's origin sits relative to the entity (a model with its origin at the feet
    /// goes down by half the body's height).
    pub offset: Vec3,
    /// Rotation about the up axis, radians: the facing, plus whatever turns the model's front to
    /// -Z.
    pub yaw: f32,
    /// The model's animation (as `AnimationPlayback::clips`); empty holds the pose.
    pub clips: Vec<ClipPlayback>,
}

impl Default for CharacterVisual {
    fn default() -> Self {
        Self {
            model: None,
            scale: 1.0,
            offset: Vec3::ZERO,
            yaw: 0.0,
            clips: Vec::new(),
        }
    }
}

/// Draws the entity (and its children) smoothly: between fixed ticks it is interpolated, and a jump
/// caused by a client's correction fades out instead of snapping. Give it to **other** players'
/// entities; the simulation `Transform` is untouched. The picture is up to one tick behind.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
#[require(DrawnPose)]
pub struct RenderSmoothing {
    /// Time in which half of a correction's jump disappears, seconds.
    pub half_life: f32,
    /// A jump longer than this (a respawn) is shown at once.
    pub snap_distance: f32,
}

impl Default for RenderSmoothing {
    fn default() -> Self {
        Self {
            half_life: 0.08,
            snap_distance: 2.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_turns_around_the_entity_and_bobs_above_and_below() {
        let motion = VisualMotion {
            turn: std::f32::consts::FRAC_PI_2,
            bob_height: 0.5,
            bob_seconds: 4.0,
        };
        let base = Transform::from_xyz(1.0, 0.2, 0.0);
        let start = motion.applied(base, 0.0);
        assert!(start.translation.distance(base.translation) < 1.0e-6);
        // A quarter turn and the top of the bob.
        let quarter = motion.applied(base, 1.0);
        assert!(quarter.translation.distance(Vec3::new(0.0, 0.7, -1.0)) < 1.0e-5);
        let low = motion.applied(base, 3.0);
        assert!((low.translation.y + 0.3).abs() < 1.0e-5);
        assert_eq!(VisualMotion::default().applied(base, 123.4), base);
        // The bob keeps its height after a long session (the angle drifts with `turn`'s rounding).
        let late = motion.applied(base, 4.0 * 1_000_000.0 + 1.0);
        assert!((late.translation.y - 0.7).abs() < 1.0e-4);
    }

    #[test]
    fn motion_numbers_are_validated() {
        assert_eq!(VisualMotion::default().validate(), Ok(()));
        for bad in [
            (f32::NAN, 0.0, 1.0),
            (0.0, f32::INFINITY, 1.0),
            (0.0, 0.0, -1.0),
        ] {
            let motion = VisualMotion {
                turn: bad.0,
                bob_height: bad.1,
                bob_seconds: bad.2,
            };
            assert!(motion.validate().is_err(), "{motion:?}");
        }
    }
}
