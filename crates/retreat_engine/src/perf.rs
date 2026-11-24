//! Performance commands: frame time and scene counts, readable and assertable from a script.
//!
//! - `perf.frame_time`: logs `perf-frame-time frames=<n> avg_ms=<x> max_ms=<y>` over the last
//!   [`FRAME_TIME_WINDOW`] frames.
//! - `assert.frame_time_below <ms>`: fails unless the **average** frame time of that window is below
//!   `ms`. Frame time is wall-clock time, so it is the one non-deterministic number in a script:
//!   it depends on the machine (a software renderer is far slower than a GPU) and on what ran in
//!   the window. The assert is therefore tolerant by design: it checks the average, never one
//!   frame, and it needs [`MIN_FRAMES_FOR_ASSERT`] frames (`wait 60` first). Pending work (a
//!   snapshot or a model load) makes frames slow, so measure in a stretch of plain `wait` frames and
//!   choose a budget with headroom for the slowest machine that runs the script (CI runs a software
//!   renderer; native headless on a GPU is much faster).
//! - `perf.counts`: logs `perf-counts entities=<n> meshes=<n> visible_meshes=<n>`.
//! - `assert.entities_below <n>`, `assert.visible_meshes_below <n>`: exact and deterministic.
//!
//! **Draw calls.** Bevy exposes no draw-call counter to the main world, so the engine reports
//! *visible meshes*: entities with a `Mesh3d` that at least one camera sees this frame
//! (`ViewVisibility`). Each is one draw per view (shadow passes excluded), so the number moves with
//! draw calls and is the same on native and in the browser, but it is not the GPU's own count.
//! Chosen because it is the cheapest deterministic number a budget can be asserted on.

use std::collections::VecDeque;
use std::time::Duration;

use bevy::camera::visibility::ViewVisibility;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult};

/// How many recent frames `perf.frame_time` and `assert.frame_time_below` look at.
pub const FRAME_TIME_WINDOW: usize = 60;

/// `assert.frame_time_below` refuses to judge on fewer frames than this.
pub const MIN_FRAMES_FOR_ASSERT: usize = 10;

/// The wall-clock duration of the most recent frames, oldest first.
#[derive(Resource, Default)]
pub struct FrameTimes {
    recent: VecDeque<Duration>,
    last_frame_end: Option<Instant>,
}

/// What a window of frames came to, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameTimeSummary {
    pub frames: usize,
    pub average_ms: f64,
    pub max_ms: f64,
}

impl FrameTimes {
    /// Records one frame, dropping the oldest beyond [`FRAME_TIME_WINDOW`].
    pub fn record(&mut self, frame: Duration) {
        self.recent.push_back(frame);
        while self.recent.len() > FRAME_TIME_WINDOW {
            self.recent.pop_front();
        }
    }

    /// Average and maximum of the recorded frames; `None` before the first frame.
    pub fn summary(&self) -> Option<FrameTimeSummary> {
        if self.recent.is_empty() {
            return None;
        }
        let milliseconds: Vec<f64> = self
            .recent
            .iter()
            .map(|frame| frame.as_secs_f64() * 1000.0)
            .collect();
        Some(FrameTimeSummary {
            frames: milliseconds.len(),
            average_ms: milliseconds.iter().sum::<f64>() / milliseconds.len() as f64,
            max_ms: milliseconds.iter().copied().fold(0.0, f64::max),
        })
    }
}

/// Adds the frame-time recorder and the `perf.*` commands.
pub struct PerfCommandsPlugin;

impl Plugin for PerfCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameTimes>()
            .add_systems(Last, record_frame_time)
            .register_command("perf.frame_time", "perf.frame_time", frame_time_command)
            .register_command("perf.counts", "perf.counts", counts_command)
            .register_command(
                "assert.frame_time_below",
                "assert.frame_time_below <milliseconds>",
                assert_frame_time_command,
            )
            .register_command(
                "assert.entities_below",
                "assert.entities_below <count>",
                assert_entities_command,
            )
            .register_command(
                "assert.visible_meshes_below",
                "assert.visible_meshes_below <count>",
                assert_visible_meshes_command,
            );
    }
}

/// Records the wall-clock duration between this frame's end and the previous frame's end.
///
/// Reads `Instant` directly, never `Time<Real>`: a command script runs on
/// `TimeUpdateStrategy::FixedTimesteps`, which makes every Bevy clock (`Time<Real>` too) advance by
/// exactly one tick per update, so Bevy's clocks cannot measure how long a frame really took.
fn record_frame_time(mut frame_times: ResMut<FrameTimes>) {
    let now = Instant::now();
    // The first frame has no previous frame to measure against.
    if let Some(previous) = frame_times.last_frame_end.replace(now) {
        frame_times.record(now - previous);
    }
}

/// Entity counts of the scene right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneCounts {
    pub entities: usize,
    pub meshes: usize,
    pub visible_meshes: usize,
}

/// Counts all entities, `Mesh3d` entities, and the `Mesh3d` entities some camera sees.
pub fn count_scene(world: &mut World) -> SceneCounts {
    let entities = world.query::<Entity>().iter(world).count();
    let mut meshes = 0;
    let mut visible_meshes = 0;
    let mut query = world.query_filtered::<&ViewVisibility, With<Mesh3d>>();
    for visibility in query.iter(world) {
        meshes += 1;
        if visibility.get() {
            visible_meshes += 1;
        }
    }
    SceneCounts {
        entities,
        meshes,
        visible_meshes,
    }
}

/// `perf.frame_time`.
fn frame_time_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    match world.resource::<FrameTimes>().summary() {
        Some(summary) => info!(
            "perf-frame-time frames={} avg_ms={:.3} max_ms={:.3}",
            summary.frames, summary.average_ms, summary.max_ms
        ),
        None => info!("perf-frame-time frames=0 (no frame has been measured yet)"),
    }
    Ok(CommandOutcome::Continue)
}

/// `perf.counts`.
fn counts_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let counts = count_scene(world);
    info!(
        "perf-counts entities={} meshes={} visible_meshes={}",
        counts.entities, counts.meshes, counts.visible_meshes
    );
    Ok(CommandOutcome::Continue)
}

/// `assert.frame_time_below <milliseconds>`.
fn assert_frame_time_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let budget_ms: f64 = args.parse(0, "milliseconds")?;
    let summary = world.resource::<FrameTimes>().summary();
    match summary {
        Some(summary) if summary.frames >= MIN_FRAMES_FOR_ASSERT => {
            if summary.average_ms < budget_ms {
                Ok(CommandOutcome::Continue)
            } else {
                Err(args.failure(format!(
                    "average frame time over the last {} frames is {:.3} ms, budget {budget_ms} ms (max {:.3} ms); frame time depends on the machine, so raise the budget only after checking the scene did not get heavier",
                    summary.frames, summary.average_ms, summary.max_ms
                )))
            }
        }
        _ => Err(args.failure(format!(
            "fewer than {MIN_FRAMES_FOR_ASSERT} frames measured; put `wait 60` before this line"
        ))),
    }
}

/// `assert.entities_below <count>`.
fn assert_entities_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let limit: usize = args.parse(0, "count")?;
    let actual = count_scene(world).entities;
    if actual < limit {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!("{actual} entities, expected fewer than {limit}")))
    }
}

/// `assert.visible_meshes_below <count>`.
fn assert_visible_meshes_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let limit: usize = args.parse(0, "count")?;
    let actual = count_scene(world).visible_meshes;
    if actual < limit {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "{actual} visible meshes, expected fewer than {limit}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_averages_and_finds_the_maximum() {
        let mut times = FrameTimes::default();
        assert_eq!(times.summary(), None);
        times.record(Duration::from_millis(10));
        times.record(Duration::from_millis(30));
        let summary = times.summary().unwrap();
        assert_eq!(summary.frames, 2);
        assert!((summary.average_ms - 20.0).abs() < 1e-9);
        assert!((summary.max_ms - 30.0).abs() < 1e-9);
    }

    #[test]
    fn the_window_keeps_only_the_latest_frames() {
        let mut times = FrameTimes::default();
        times.record(Duration::from_secs(1));
        for _ in 0..FRAME_TIME_WINDOW {
            times.record(Duration::from_millis(5));
        }
        let summary = times.summary().unwrap();
        assert_eq!(summary.frames, FRAME_TIME_WINDOW);
        assert!(
            (summary.max_ms - 5.0).abs() < 1e-9,
            "the old slow frame left the window"
        );
    }
}
