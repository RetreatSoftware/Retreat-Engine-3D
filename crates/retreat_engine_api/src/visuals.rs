//! How a module changes what an entity looks like, without naming render types: color, a character's
//! model, facing and animation, and smoothing of what is drawn. The engine maps them
//! (`retreat_engine/src/visuals.rs`, `render_smoothing.rs`). **All are render only:** they change
//! nothing in the simulation, are not synced and are not rewound. A module derives them from synced
//! state on each machine, for example in a system on `Added<Player>` (a client creates replicated
//! entities from the server's state with no visuals but a plain shape).

use bevy::prelude::*;

use crate::ClipPlayback;

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
