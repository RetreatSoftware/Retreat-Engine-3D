//! How game code and modules ask for a camera, without naming Bevy's camera components.
//!
//! An entity with a [`GameCamera`] and a unique `Name` is a perspective camera that looks from
//! `position` at `target`. The engine creates the real camera (today Bevy's `Camera3d`, a
//! `Transform` and a `Projection`) and recomputes them whenever the [`GameCamera`] changes, so a
//! module moves a camera by editing this component and never touches `Transform` or `Projection`.
//! There is no camera controller here: orbit, follow and first-person cameras are modules.

use bevy::prelude::*;

/// Smallest field of view the engine accepts, in degrees.
pub const MIN_FOV_DEGREES: f32 = 1.0;
/// Largest field of view the engine accepts, in degrees.
pub const MAX_FOV_DEGREES: f32 = 179.0;
/// Field of view of [`GameCamera::looking_at`] (Bevy's default, so cameras look as before).
pub const DEFAULT_FOV_DEGREES: f32 = 45.0;

/// Smallest distance between `position` and `target`, and smallest sine of the angle between
/// the view direction and `up`: below these the camera has no usable orientation.
const MIN_GEOMETRY_EPSILON: f32 = 1.0e-4;

/// A camera that looks from `position` at `target`. Read again every time it changes.
#[derive(Component, Clone, Debug, PartialEq)]
#[require(Camera3d, Transform)]
pub struct GameCamera {
    /// Where the camera is, in world space.
    pub position: Vec3,
    /// The point the camera looks at.
    pub target: Vec3,
    /// Which way is up for the camera (usually `Vec3::Y`). Must not be parallel to the view.
    pub up: Vec3,
    /// Vertical field of view in degrees, from [`MIN_FOV_DEGREES`] to [`MAX_FOV_DEGREES`].
    pub fov_degrees: f32,
}

impl GameCamera {
    /// A camera at `position` looking at `target`, with `Vec3::Y` up and the default field of view.
    pub fn looking_at(position: Vec3, target: Vec3) -> Self {
        Self {
            position,
            target,
            up: Vec3::Y,
            fov_degrees: DEFAULT_FOV_DEGREES,
        }
    }

    /// `Ok` when the camera has a usable orientation, otherwise a message saying what is wrong.
    /// The engine and its commands check this before applying a change.
    pub fn validate(&self) -> Result<(), String> {
        let all_finite = self.position.is_finite()
            && self.target.is_finite()
            && self.up.is_finite()
            && self.fov_degrees.is_finite();
        if !all_finite {
            return Err(format!(
                "camera values must be finite numbers, got position {}, target {}, up {}, fov {}",
                self.position, self.target, self.up, self.fov_degrees
            ));
        }
        if !(MIN_FOV_DEGREES..=MAX_FOV_DEGREES).contains(&self.fov_degrees) {
            return Err(format!(
                "camera fov must be from {MIN_FOV_DEGREES} to {MAX_FOV_DEGREES} degrees, got {}",
                self.fov_degrees
            ));
        }
        let view = self.target - self.position;
        if view.length() < MIN_GEOMETRY_EPSILON {
            return Err(format!(
                "camera position {} and target {} are the same point; move one of them",
                self.position, self.target
            ));
        }
        if self.up.length() < MIN_GEOMETRY_EPSILON {
            return Err("camera up must not be the zero vector".to_string());
        }
        if view.normalize().cross(self.up.normalize()).length() < MIN_GEOMETRY_EPSILON {
            return Err(format!(
                "camera up {} is parallel to the view direction {view}; pick another up",
                self.up
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> GameCamera {
        GameCamera::looking_at(Vec3::new(0.0, 2.0, 5.0), Vec3::ZERO)
    }

    #[test]
    fn a_normal_camera_is_valid() {
        assert_eq!(camera().validate(), Ok(()));
        assert_eq!(camera().fov_degrees, DEFAULT_FOV_DEGREES);
        assert_eq!(camera().up, Vec3::Y);
    }

    #[test]
    fn position_equal_to_target_is_rejected() {
        let mut bad = camera();
        bad.target = bad.position;
        assert!(bad.validate().unwrap_err().contains("the same point"));
    }

    #[test]
    fn up_parallel_to_the_view_is_rejected() {
        let looking_straight_down = GameCamera::looking_at(Vec3::new(0.0, 5.0, 0.0), Vec3::ZERO);
        let error = looking_straight_down.validate().unwrap_err();
        assert!(error.contains("parallel to the view direction"), "{error}");
        let mut zero_up = camera();
        zero_up.up = Vec3::ZERO;
        assert!(zero_up.validate().unwrap_err().contains("zero vector"));
    }

    #[test]
    fn fov_outside_the_range_is_rejected() {
        for fov in [0.0, 0.5, 180.0, 400.0] {
            let mut bad = camera();
            bad.fov_degrees = fov;
            let error = bad.validate().unwrap_err();
            assert!(error.contains("fov must be from 1 to 179"), "{error}");
        }
    }

    #[test]
    fn non_finite_values_are_rejected() {
        let mut bad = camera();
        bad.position.x = f32::NAN;
        assert!(bad.validate().unwrap_err().contains("finite"));
        let mut bad = camera();
        bad.fov_degrees = f32::INFINITY;
        assert!(bad.validate().unwrap_err().contains("finite"));
    }
}
