//! An honest client that is slow or stalls is never refused: after a stall it catches up by sending
//! many inputs at once, which the server's rate limits must allow for.

use std::time::Duration;

use retreat_engine::net::harness::NetHarness;
use retreat_engine::net::link::LinkConditions;

use crate::net_character::{harness, link};

/// 75 frames per second.
const FRAME: Duration = Duration::from_micros(13_333);

/// Client 0 stalls for `frozen` frames, then draws `awake` frames, `times` times over.
fn stall(harness: &mut NetHarness, frozen: u64, awake: u64, times: u32) {
    for _ in 0..times {
        harness.set_client_frozen(0, true);
        harness.run_frames(FRAME, frozen, |_, _| {});
        harness.set_client_frozen(0, false);
        harness.run_frames(FRAME, awake, |_, _| {});
    }
}

fn session(conditions: LinkConditions) -> NetHarness {
    let mut harness = harness(2, conditions);
    harness.run_frames(FRAME, 300, |_, _| {});
    harness
}

fn assert_never_refused(harness: &NetHarness, what: &str) {
    let stats = harness.server().stats;
    assert_eq!(
        stats.refusals(),
        0,
        "{what}: refused {}",
        stats.refusal_breakdown()
    );
}

#[test]
fn a_client_that_stalls_for_seconds_is_never_refused() {
    // From half a second to six: below, at and beyond what a client replays instead of skipping.
    for seconds in [0.5, 1.5, 2.5, 3.5, 4.5, 6.0_f64] {
        let mut harness = session(link(2, 1, 0.0));
        let frozen = (seconds * 75.0) as u64;
        stall(&mut harness, frozen, 150, 4);
        assert_never_refused(&harness, &format!("{seconds} s stalls"));
    }
}

#[test]
fn a_client_that_draws_a_few_frames_between_long_stalls_is_never_refused() {
    // A slow software-rendered browser: a frame every 0.4 s to 2 s, sometimes a quick run of them.
    for (frozen, awake) in [(30, 1), (75, 1), (150, 2), (110, 8), (260, 3)] {
        let mut harness = session(link(3, 1, 0.0));
        stall(&mut harness, frozen, awake, 40);
        harness.run_frames(FRAME, 600, |_, _| {});
        assert_never_refused(&harness, &format!("{frozen} frames frozen, {awake} drawn"));
    }
}

#[test]
fn a_client_whose_datagrams_arrive_all_at_once_is_never_refused() {
    // The network holds everything back for five seconds, as a stalled server or link does.
    let mut harness = session(link(2, 0, 0.0));
    let held = LinkConditions {
        latency_ticks: 320,
        jitter_ticks: 0,
        loss: 0.0,
    };
    let normal = link(2, 0, 0.0);
    harness.set_links(held, normal);
    harness.run_frames(FRAME, 5 * 75, |_, _| {});
    harness.set_links(normal, normal);
    harness.run_frames(FRAME, 600, |_, _| {});
    assert_never_refused(&harness, "a held-back link");
}
