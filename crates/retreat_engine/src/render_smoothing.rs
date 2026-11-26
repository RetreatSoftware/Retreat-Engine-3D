//! Where entities are drawn, without touching the simulation (contract `visuals.rs`:
//! `RenderSmoothing`, `RemoteInterpolation`, `DrawnPose`):
//! - `remember_previous_pose` (`FixedFirst`) keeps the pose before the tick;
//! - a client's correction reports how far it moved the entity ([`poses_before_correction`],
//!   [`absorb_correction`]); that jump becomes an offset that fades out;
//! - `compute_drawn_poses` (`Update`, `DrawnPoseSystems`) decides the pose to draw: the server's past
//!   at the view tick for remote-interpolated entities (`net/lag_compensation.rs`), else between
//!   the two ticks plus the offset; game cameras read it from `DrawnPose`;
//! - in `PostUpdate`, just for transform propagation, `Transform` holds the pose to draw (between
//!   the two ticks, plus the offset); right after, the simulation pose is put back. So
//!   `GlobalTransform` (what is drawn, children included) is smooth and every simulation system
//!   still reads the true `Transform`.
//!
//! Under a command script the clock has no overstep, so the picture is exactly one tick behind.
//!
//! Commands: `render.smooth <name> <half_life> <snap_distance>`,
//! `assert.drawn_position <name> <x> <y> <z> <tolerance>`.

use std::collections::BTreeMap;

use bevy::app::FixedFirst;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, DrawnPose, DrawnPoseSystems,
    HideVisuals, NetPose, NetViewTick, RemoteInterpolation, RenderSmoothing,
};

use crate::entity_names::find_unique_entity_by_name;
use crate::net::lag_compensation::PoseHistory;

/// What the engine keeps for a smoothed entity.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct SmoothingState {
    previous: Option<Transform>,
    /// Added to the drawn position; fades out.
    offset: Vec3,
    /// The pose to draw this frame.
    drawn: Option<Transform>,
    /// The simulation pose, while `Transform` holds the drawn one.
    simulation: Option<Transform>,
}

pub struct RenderSmoothingPlugin;

impl Plugin for RenderSmoothingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedFirst, remember_previous_pose)
            .init_resource::<PoseHistory>()
            .add_systems(Update, compute_drawn_poses.in_set(DrawnPoseSystems))
            .add_systems(
                PostUpdate,
                (
                    draw_smoothed_pose.before(TransformSystems::Propagate),
                    restore_simulation_pose.after(TransformSystems::Propagate),
                ),
            )
            .register_command(
                "render.smooth",
                "render.smooth <name> <half_life> <snap_distance>",
                smooth_command,
            )
            .register_command(
                "assert.drawn_position",
                "assert.drawn_position <name> <x> <y> <z> <tolerance>",
                assert_drawn_command,
            );
    }
}

fn remember_previous_pose(
    mut commands: Commands,
    mut smoothed: Query<(Entity, &Transform, Option<&mut SmoothingState>), With<DrawnPose>>,
) {
    for (entity, transform, state) in &mut smoothed {
        match state {
            Some(mut state) => state.previous = Some(*transform),
            None => {
                commands.entity(entity).insert(SmoothingState {
                    previous: Some(*transform),
                    ..SmoothingState::default()
                });
            }
        }
    }
}

/// The part of the offset left after `seconds`.
fn faded(offset: Vec3, seconds: f32, half_life: f32) -> Vec3 {
    if half_life <= 0.0 {
        return Vec3::ZERO;
    }
    offset * 0.5_f32.powf(seconds / half_life)
}

/// The offset after the simulation jumped by `jump`: what was drawn stays where it was, unless
/// the jump is too long to hide.
fn absorbed(offset: Vec3, jump: Vec3, snap_distance: f32) -> Vec3 {
    if jump.length() > snap_distance {
        return Vec3::ZERO;
    }
    offset - jump
}

/// The pose to draw between two ticks.
fn between(previous: Transform, current: Transform, fraction: f32) -> Transform {
    Transform {
        translation: previous.translation.lerp(current.translation, fraction),
        rotation: previous.rotation.slerp(current.rotation, fraction),
        scale: current.scale,
    }
}

/// What deciding an entity's drawn pose reads and writes.
type Drawn = (
    &'static Name,
    &'static Transform,
    &'static mut DrawnPose,
    Option<&'static RenderSmoothing>,
    Has<RemoteInterpolation>,
    Option<&'static mut SmoothingState>,
    Entity,
    Has<NetPose>,
    Has<HideVisuals>,
    Option<&'static Visibility>,
);

/// Decides where each entity with a `DrawnPose` is drawn this frame.
fn compute_drawn_poses(
    time: Res<Time<Real>>,
    fixed: Res<Time<Fixed>>,
    view: Option<Res<NetViewTick>>,
    history: Res<PoseHistory>,
    mut entities: Query<Drawn>,
    mut commands: Commands,
) {
    let view = view.and_then(|view| view.0);
    // Only look the past up when something is drawn from it.
    let past = view
        .filter(|_| entities.iter().any(|entity| entity.4))
        .and_then(|view| history.at(view + f64::from(fixed.overstep_fraction())));
    for (
        name,
        transform,
        mut pose,
        smoothing,
        remote,
        state,
        entity,
        bodyless,
        hidden,
        visibility,
    ) in &mut entities
    {
        let mut drawn = *transform;
        let server = past
            .as_ref()
            .filter(|_| remote)
            .and_then(|bodies| bodies.get(name.as_str()));
        // A bodyless entity drawn in the past exists on screen exactly while it existed then:
        // not before the view reaches its birth, and until the view passes its removal.
        if remote && bodyless && past.is_some() {
            let wanted = if server.is_some() && !hidden {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if visibility != Some(&wanted) {
                commands.entity(entity).insert(wanted);
            }
        }
        if let Some(mut state) = state {
            if let Some(body) = server {
                drawn.translation = body.translation;
                drawn.rotation = body.rotation;
                state.offset = Vec3::ZERO;
            } else if let Some(settings) = smoothing {
                state.offset = faded(state.offset, time.delta_secs(), settings.half_life);
                let previous = state.previous.unwrap_or(drawn);
                // A teleport is not walked through.
                let previous =
                    if previous.translation.distance(drawn.translation) > settings.snap_distance {
                        drawn
                    } else {
                        previous
                    };
                drawn = between(previous, drawn, fixed.overstep_fraction());
                drawn.translation += state.offset;
            }
            state.drawn = Some(drawn);
        }
        pose.set_if_neq(DrawnPose {
            translation: drawn.translation,
            rotation: drawn.rotation,
        });
    }
}

fn draw_smoothed_pose(mut entities: Query<(&mut Transform, &mut SmoothingState)>) {
    for (mut transform, mut state) in &mut entities {
        let Some(drawn) = state.drawn.take() else {
            continue;
        };
        if drawn != *transform {
            state.simulation = Some(*transform);
            *transform = drawn;
        }
    }
}

fn restore_simulation_pose(mut smoothed: Query<(&mut Transform, &mut SmoothingState)>) {
    for (mut transform, mut state) in &mut smoothed {
        if let Some(simulation) = state.simulation.take() {
            *transform = simulation;
        }
    }
}

/// Where every smoothed entity is before a client rewinds.
pub(crate) fn poses_before_correction(world: &mut World) -> BTreeMap<Entity, Vec3> {
    world
        .query_filtered::<(Entity, &Transform), With<RenderSmoothing>>()
        .iter(world)
        .map(|(entity, transform)| (entity, transform.translation))
        .collect()
}

/// After the replay: turns each entity's jump into a fading offset.
pub(crate) fn absorb_correction(world: &mut World, before: &BTreeMap<Entity, Vec3>) {
    let mut smoothed = world.query::<(Entity, &Transform, &RenderSmoothing, &mut SmoothingState)>();
    for (entity, transform, settings, mut state) in smoothed.iter_mut(world) {
        if let Some(old) = before.get(&entity) {
            let jump = transform.translation - *old;
            state.offset = absorbed(state.offset, jump, settings.snap_distance);
        }
    }
}

/// `render.smooth <name> <half_life> <snap_distance>`.
fn smooth_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let name: String = args.parse(0, "name")?;
    let settings = RenderSmoothing {
        half_life: args.parse(1, "half_life")?,
        snap_distance: args.parse(2, "snap_distance")?,
    };
    let entity = find_unique_entity_by_name(world, args, &name)?;
    world.entity_mut(entity).insert(settings);
    Ok(CommandOutcome::Continue)
}

/// `assert.drawn_position <name> <x> <y> <z> <tolerance>`: where the last frame drew it.
fn assert_drawn_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(5)?;
    let name: String = args.parse(0, "name")?;
    let expected = Vec3::new(
        args.parse(1, "x")?,
        args.parse(2, "y")?,
        args.parse(3, "z")?,
    );
    let tolerance: f32 = args.parse(4, "tolerance")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    let Some(drawn) = world.get::<GlobalTransform>(entity) else {
        return Err(args.failure(format!("`{name}` has no transform")));
    };
    let drawn = drawn.translation();
    if drawn.distance(expected) <= tolerance {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "`{name}` is drawn at {drawn}, expected {expected} within {tolerance}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jump_is_hidden_then_fades_and_a_long_one_shows_at_once() {
        let offset = absorbed(Vec3::ZERO, Vec3::X, 2.0);
        assert_eq!(offset, Vec3::NEG_X, "drawn where it was");
        let half = faded(offset, 0.1, 0.1);
        assert!(half.distance(Vec3::new(-0.5, 0.0, 0.0)) < 1e-6);
        assert_eq!(faded(offset, 1.0, 0.0), Vec3::ZERO);
        assert_eq!(absorbed(half, Vec3::X * 5.0, 2.0), Vec3::ZERO);
    }

    #[test]
    fn drawing_interpolates_between_the_ticks() {
        let previous = Transform::from_xyz(0.0, 0.0, 0.0);
        let current = Transform::from_xyz(2.0, 0.0, 0.0);
        assert_eq!(between(previous, current, 0.25).translation.x, 0.5);
    }
}
