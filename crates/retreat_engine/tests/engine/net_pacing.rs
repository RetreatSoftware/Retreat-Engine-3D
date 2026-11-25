//! Clients that tick slower than, faster than or in bursts against the server keep their inputs:
//! the server reports how early they arrive, the client adjusts, and late bursts are spread.

use retreat_engine::net::harness::{ClientFrames, NetHarness};
use retreat_engine::net::link::LinkConditions;
use retreat_engine::net::tick_pacing::{MAX_SLACK_TICKS, SLOW_TIME_SCALE};

use crate::net_character::{harness, link, mover, run_on_client};

/// Walks client 0's character east for `ticks` server ticks and returns how far the server moved it.
fn walk_west(harness: &mut NetHarness, ticks: u64) -> f32 {
    let before = mover(harness.server_app.world_mut(), "mover_0").1;
    run_on_client(harness, 0, "input.set_axis move_x -1");
    harness.run_idle(ticks);
    run_on_client(harness, 0, "input.clear_axis move_x");
    harness.run_idle(300);
    let after = mover(harness.server_app.world_mut(), "mover_0").1;
    before.translation.x - after.translation.x
}

/// Late inputs counted while the game runs for `ticks` more ticks.
fn late_during(harness: &mut NetHarness, ticks: u64) -> u64 {
    let before = harness.server().late_inputs;
    harness.run_idle(ticks);
    harness.server().late_inputs - before
}

fn with_frames(frames: ClientFrames, conditions: LinkConditions) -> NetHarness {
    let mut harness = harness(2, conditions);
    harness.set_client_frames(0, frames);
    // Time for a few timing reports.
    harness.run_idle(600);
    harness
}

/// At 4 units per second and 64 ticks per second.
fn expected_distance(ticks: u64) -> f32 {
    4.0 * ticks as f32 / 64.0
}

fn assert_walks_its_full_distance(harness: &mut NetHarness) {
    let walked = walk_west(harness, 96);
    let expected = expected_distance(96);
    // Frames quantize when the axis change is seen by up to one frame.
    assert!(
        (walked - expected).abs() < 1.9,
        "walked {walked}, expected about {expected}"
    );
    assert!(walked > expected * 0.7, "walked {walked} of {expected}");
}

#[test]
fn a_slow_renderer_that_loses_ticks_every_frame_keeps_its_inputs() {
    // 400 ms frames of which at most 250 ms are simulated, as Bevy does.
    let mut harness = with_frames(
        ClientFrames {
            every_ticks: 26,
            ..ClientFrames::default()
        },
        link(3, 1, 0.0),
    );
    assert!(harness.client(0).stats.catch_up_ticks > 0);
    assert_eq!(harness.client(0).stats.resyncs, 0);
    let late = late_during(&mut harness, 640);
    assert!(late < 64, "{late} inputs late out of about 640");
    assert_walks_its_full_distance(&mut harness);
    assert_eq!(harness.server().stats.refusals(), 0);
}

#[test]
fn a_clock_that_drifts_slow_is_caught_up() {
    let mut harness = with_frames(
        ClientFrames {
            speed: 0.8,
            ..ClientFrames::default()
        },
        link(4, 1, 0.0),
    );
    assert!(harness.client(0).stats.catch_up_ticks > 50);
    let late = late_during(&mut harness, 640);
    assert!(late < 64, "{late} inputs late");
    assert_walks_its_full_distance(&mut harness);
}

#[test]
fn a_clock_that_runs_fast_is_slowed_before_the_server_refuses_it() {
    let mut harness = with_frames(
        ClientFrames {
            speed: 1.05,
            ..ClientFrames::default()
        },
        link(2, 0, 0.0),
    );
    harness.run_idle(2000);
    let lead = harness.client(0).tick as i64 - harness.server().tick as i64;
    assert!(lead < MAX_SLACK_TICKS + 30, "the lead is {lead}");
    assert!(harness.client(0).pacing.time_scale >= SLOW_TIME_SCALE);
    assert_eq!(harness.server().stats.refusals(), 0);
    assert_walks_its_full_distance(&mut harness);
}

#[test]
fn a_hopeless_gap_jumps_instead_of_replaying_it() {
    let mut harness = harness(2, link(2, 0, 0.0));
    // The client's app stops for 20 seconds (a hidden browser tab).
    harness.set_client_frames(
        0,
        ClientFrames {
            every_ticks: 1,
            max_ticks_per_frame: 0,
            speed: 1.0,
        },
    );
    harness.run_idle(1280);
    harness.set_client_frames(0, ClientFrames::default());
    harness.run_idle(300);
    assert!(harness.client(0).stats.resyncs > 0);
    assert!(harness.client(0).tick >= harness.server().tick);
    assert_walks_its_full_distance(&mut harness);
}

#[test]
fn a_late_burst_is_spread_over_ticks_so_every_input_takes_effect() {
    // A negative margin makes the first inputs late; before they were run together and only the
    // last one counted.
    let mut harness = harness(-12, link(3, 0, 0.0));
    assert_walks_its_full_distance(&mut harness);
    let late = late_during(&mut harness, 320);
    assert_eq!(late, 0, "the lead was corrected");
}
