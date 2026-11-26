//! Measuring how evenly an entity moves **on screen**: where it is drawn in every rendered frame,
//! against real time. Motion that is even in the simulation can still stutter on screen (frame
//! rate against tick rate, network arrival, a camera updated late); this is how an agent sees it.
//!
//!
//! Time is `Time<Virtual>`: real frame time in a game, a fixed step per frame under a deterministic
//! script.
//!
//! - `motion.record <name> <label> <frames>`: records the next `frames` frames of `<name>` (and of
//!   the first camera) into `target/motion/<label>.csv` (native) and logs a summary. It does not
//!   block the script: wait with `wait_until <ticks> assert.motion_recorded <label>`.
//! - `assert.motion_recorded <label>`.
//! - `assert.motion_even <label> <max_error_ms>`: over the part of the recording in which the entity
//!   moved, its drawn position is never further from constant-speed motion than the distance it
//!   covers in `max_error_ms`. For a straight walk at constant speed only.

use std::collections::BTreeMap;

use bevy::animation::AnimationPlayer;
use bevy::prelude::*;
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult, NetViewTick};

use crate::entity_names::find_unique_entity_by_name;

/// The walk is judged between the points this far (units) from where it starts and ends, so that
/// starting and stopping do not count.
const TRIM_DISTANCE: f32 = 0.4;

/// One rendered frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionSample {
    /// `Time<Virtual>` of the frame, seconds.
    pub time: f64,
    /// Where the entity was drawn.
    pub position: Vec3,
    /// Where the first camera was (zero without one).
    pub camera: Vec3,
    /// The camera's view direction.
    pub forward: Vec3,
    /// `NetViewTick` (0 without one).
    pub view_tick: f64,
    /// How far into the next fixed tick the frame was.
    pub overstep: f32,
    /// Where the entity's front points, radians about the up axis (0 is -Z).
    pub yaw: f32,
    /// The pose time of the first animation playing at or below the entity (-1 without one).
    pub animation_time: f32,
}

/// How even a recorded walk was.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionStats {
    /// Frames judged.
    pub frames: usize,
    /// Mean frame rate over them.
    pub frames_per_second: f64,
    /// Units per second of the best constant-speed fit.
    pub speed: f64,
    /// Root mean square and largest distance from that fit, as time at that speed (ms).
    pub rms_error_ms: f64,
    pub max_error_ms: f64,
    /// Frames whose own speed was under half or over one and a half times the fit's.
    pub uneven_frames: usize,
    /// Frames in which the entity went backwards.
    pub backward_frames: usize,
}

/// Judges the moving part of `samples` (see the module docs). `None` when it moved too little.
pub fn motion_stats(samples: &[MotionSample]) -> Option<MotionStats> {
    let (first, last) = (samples.first()?.position, samples.last()?.position);
    let from = samples
        .iter()
        .position(|sample| sample.position.distance(first) > TRIM_DISTANCE)?;
    let to = samples
        .iter()
        .rposition(|sample| sample.position.distance(last) > TRIM_DISTANCE)?;
    let walk = samples.get(from..=to).filter(|walk| walk.len() >= 8)?;
    let direction = (walk.last()?.position - walk.first()?.position).try_normalize()?;
    let points: Vec<(f64, f64)> = walk
        .iter()
        .map(|sample| {
            let along = (sample.position - walk[0].position).dot(direction);
            (sample.time - walk[0].time, f64::from(along))
        })
        .collect();
    let count = points.len() as f64;
    let mean_time = points.iter().map(|point| point.0).sum::<f64>() / count;
    let mean_along = points.iter().map(|point| point.1).sum::<f64>() / count;
    let spread = points
        .iter()
        .map(|point| (point.0 - mean_time).powi(2))
        .sum::<f64>();
    let speed = points
        .iter()
        .map(|point| (point.0 - mean_time) * (point.1 - mean_along))
        .sum::<f64>()
        / spread;
    if !(speed.is_finite() && speed > 0.0) {
        return None;
    }
    let errors: Vec<f64> = points
        .iter()
        .map(|point| (point.1 - mean_along - speed * (point.0 - mean_time)) / speed * 1000.0)
        .collect();
    let steps: Vec<f64> = points
        .windows(2)
        .map(|pair| (pair[1].1 - pair[0].1) / (pair[1].0 - pair[0].0).max(1.0e-6) / speed)
        .collect();
    Some(MotionStats {
        frames: points.len(),
        frames_per_second: (count - 1.0) / points.last()?.0,
        speed,
        rms_error_ms: (errors.iter().map(|error| error * error).sum::<f64>() / count).sqrt(),
        max_error_ms: errors.iter().fold(0.0, |max, error| max.max(error.abs())),
        uneven_frames: steps
            .iter()
            .filter(|step| !(0.5..=1.5).contains(*step))
            .count(),
        backward_frames: steps.iter().filter(|step| **step < -0.01).count(),
    })
}

/// Recordings in progress and finished ones, by label.
#[derive(Resource, Default)]
pub struct MotionRecordings {
    running: BTreeMap<String, (Entity, usize, Vec<MotionSample>)>,
    finished: BTreeMap<String, Vec<MotionSample>>,
}

pub struct MotionRecordPlugin;

impl Plugin for MotionRecordPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MotionRecordings>()
            // After transform propagation: `GlobalTransform` is what is drawn.
            .add_systems(Last, record_frames)
            .register_command(
                "motion.record",
                "motion.record <name> <label> <frames>",
                record_command,
            )
            .register_command(
                "assert.motion_recorded",
                "assert.motion_recorded <label>",
                assert_recorded_command,
            )
            .register_command(
                "assert.motion_even",
                "assert.motion_even <label> <max_error_ms>",
                assert_even_command,
            );
    }
}

fn record_frames(
    time: Res<Time<Virtual>>,
    fixed: Res<Time<Fixed>>,
    view: Option<Res<NetViewTick>>,
    mut recordings: ResMut<MotionRecordings>,
    entities: Query<&GlobalTransform>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    (children, players): (Query<&Children>, Query<&AnimationPlayer>),
) {
    if recordings.running.is_empty() {
        return;
    }
    let camera = cameras.iter().next().copied().unwrap_or_default();
    let mut done = Vec::new();
    for (label, (entity, frames, samples)) in &mut recordings.running {
        if let Ok(transform) = entities.get(*entity) {
            samples.push(MotionSample {
                time: time.elapsed_secs_f64(),
                position: transform.translation(),
                camera: camera.translation(),
                forward: camera.forward().as_vec3(),
                view_tick: view.as_ref().and_then(|view| view.0).unwrap_or(0.0),
                overstep: fixed.overstep_fraction(),
                yaw: transform.rotation().to_euler(EulerRot::YXZ).0,
                animation_time: std::iter::once(*entity)
                    .chain(children.iter_descendants(*entity))
                    .filter_map(|part| players.get(part).ok())
                    .find_map(|player| player.playing_animations().next())
                    .map_or(-1.0, |(_, active)| active.seek_time()),
            });
        }
        if samples.len() >= *frames || entities.get(*entity).is_err() {
            done.push(label.clone());
        }
    }
    for label in done {
        let Some((_, _, samples)) = recordings.running.remove(&label) else {
            continue;
        };
        report(&label, &samples);
        recordings.finished.insert(label, samples);
    }
}

fn report(label: &str, samples: &[MotionSample]) {
    match motion_stats(samples) {
        Some(stats) => info!("motion {label}: {stats:?}"),
        None => info!(
            "motion {label}: {} frames, the entity barely moved",
            samples.len()
        ),
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut text =
            "time,x,y,z,camera_x,camera_y,camera_z,forward_x,forward_y,forward_z,view_tick,overstep,yaw,animation_time\n"
                .to_string();
        for sample in samples {
            let (at, camera, forward) = (sample.position, sample.camera, sample.forward);
            text += &format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                sample.time,
                at.x,
                at.y,
                at.z,
                camera.x,
                camera.y,
                camera.z,
                forward.x,
                forward.y,
                forward.z,
                sample.view_tick,
                sample.overstep,
                sample.yaw,
                sample.animation_time
            );
        }
        let path = format!("target/motion/{label}.csv");
        if let Err(error) =
            std::fs::create_dir_all("target/motion").and_then(|()| std::fs::write(&path, text))
        {
            warn!("motion {label}: cannot write {path}: {error}");
        }
    }
}

fn record_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let name: String = args.parse(0, "name")?;
    let label: String = args.parse(1, "label")?;
    let frames: usize = args.parse(2, "frames")?;
    if !label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(args.failure("the label is a file name: letters, digits, `_` and `-`"));
    }
    let entity = find_unique_entity_by_name(world, args, &name)?;
    let mut recordings = world.resource_mut::<MotionRecordings>();
    recordings.finished.remove(&label);
    recordings
        .running
        .insert(label, (entity, frames, Vec::new()));
    Ok(CommandOutcome::Continue)
}

fn assert_recorded_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let label: String = args.parse(0, "label")?;
    if world
        .resource::<MotionRecordings>()
        .finished
        .contains_key(&label)
    {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!("`{label}` is not recorded yet")))
    }
}

fn assert_even_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let label: String = args.parse(0, "label")?;
    let max_error_ms: f64 = args.parse(1, "max_error_ms")?;
    let Some(samples) = world.resource::<MotionRecordings>().finished.get(&label) else {
        return Err(args.failure(format!("`{label}` is not recorded")));
    };
    match motion_stats(samples) {
        Some(stats) if stats.max_error_ms <= max_error_ms => Ok(CommandOutcome::Continue),
        Some(stats) => Err(args.failure(format!("`{label}` moved unevenly: {stats:?}"))),
        None => Err(args.failure(format!("`{label}` barely moved"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(position: impl Fn(usize) -> f32) -> Vec<MotionSample> {
        (0..100)
            .map(|frame| MotionSample {
                time: frame as f64 / 60.0,
                position: Vec3::X * position(frame),
                ..MotionSample::default()
            })
            .collect()
    }

    #[test]
    fn constant_speed_is_even_and_a_freeze_is_not() {
        let even = motion_stats(&walk(|frame| frame as f32 * 0.1)).expect("moved");
        assert!(even.max_error_ms < 0.01, "{even:?}");
        assert!((even.speed - 6.0).abs() < 1e-3);
        assert_eq!(even.uneven_frames, 0);
        // Every tenth frame stands still, the next one jumps.
        let uneven = motion_stats(&walk(|frame| {
            let shown = if frame % 10 == 9 { frame - 1 } else { frame };
            shown as f32 * 0.1
        }))
        .expect("moved");
        assert!(uneven.max_error_ms > 10.0, "{uneven:?}");
        assert!(uneven.uneven_frames >= 14, "{uneven:?}");
        assert!(motion_stats(&walk(|_| 1.0)).is_none());
    }
}
