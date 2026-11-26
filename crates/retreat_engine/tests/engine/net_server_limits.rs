//! Server hardening: rate limits, input validation, repeat offenders, connection cap, join timeout and the
//! identity hook. Misbehaving clients are raw loopback connections that send hand-made datagrams; honest
//! clients are the harness's real ones. The server must count every refusal and never panic.

use retreat_engine::net::ClientMessage;
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::limits::ServerLimits;
use retreat_engine::net::link::LinkConditions;
use retreat_engine::net::transport::{ClientTransport, LoopbackClient};
use retreat_engine::net::wire::encode_client_message;
use retreat_engine_api::{
    NetIdentity, NetIdentityCheck, NetIdentityDecision, NetIdentityRequest, NetPlayerIds,
};

use super::net_harness::{ALLOWED, make_app};

const PERFECT: LinkConditions = LinkConditions {
    latency_ticks: 0,
    jitter_ticks: 0,
    loss: 0.0,
};

/// Small numbers so a test needs few ticks; everything else is the default.
fn small_limits() -> ServerLimits {
    ServerLimits {
        rate_window_ticks: 8,
        max_datagrams_per_window: 64,
        max_datagram_burst: 64,
        max_inputs_per_window: 16,
        max_input_burst: 16,
        violations_before_mute: 4,
        mute_ticks: 10,
        mutes_before_disconnect: 2,
        join_timeout_ticks: 30,
        ..ServerLimits::default()
    }
}

fn harness_with(limits: ServerLimits, clients: u32) -> NetHarness {
    let config = HarnessConfig {
        margin_ticks: 2,
        state_interval_ticks: 2,
        up: PERFECT,
        down: PERFECT,
        seed: 5,
    };
    let mut harness = NetHarness::with_limits(config, limits, ALLOWED, clients, make_app);
    for _ in 0..100 {
        if harness.all_joined() {
            break;
        }
        harness.run_idle(1);
    }
    assert!(harness.all_joined(), "the honest clients must join");
    harness
}

fn hello_bytes(token: Option<&str>) -> Vec<u8> {
    encode_client_message(&ClientMessage::Hello {
        nonce: 9,
        token: token.map(str::to_string),
    })
}

fn input_bytes(tick: u64, lines: &[&str]) -> Vec<u8> {
    encode_client_message(&ClientMessage::Input {
        tick,
        lines: lines.iter().map(|line| line.to_string()).collect(),
    })
}

/// A raw connection that has said hello and been let in.
fn joined_raw(harness: &mut NetHarness) -> LoopbackClient {
    let mut raw = harness.network().connect();
    raw.send(hello_bytes(None));
    harness.run_idle(2);
    raw
}

#[test]
fn an_oversized_datagram_is_counted_and_dropped() {
    let mut harness = harness_with(small_limits(), 0);
    let mut raw = harness.network().connect();
    raw.send(vec![7; 5000]);
    harness.run_idle(2);
    assert_eq!(harness.server().stats.datagrams_too_large, 1);
    assert_eq!(harness.server().connection_count(), 1);
}

#[test]
fn garbage_gets_a_connection_muted_and_then_disconnected_without_a_panic() {
    let mut harness = harness_with(small_limits(), 1);
    let mut raw = harness.network().connect();
    for tick in 0..40u8 {
        for variant in 0..6u8 {
            // Empty, wrong version, right version with junk, and plain noise.
            let bytes = match variant % 4 {
                0 => vec![],
                1 => vec![250, 1, 2, 3],
                2 => vec![6, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 1],
                _ => vec![tick; 20],
            };
            raw.send(bytes);
        }
        harness.run_idle(1);
    }
    let stats = harness.server().stats;
    assert_eq!(stats.connections_muted, 2, "muted twice, then kicked");
    assert_eq!(stats.connections_kicked, 1);
    assert!(
        stats.datagrams_muted > 0,
        "muted datagrams are dropped unread"
    );
    assert!(harness.server().undecodable_packets >= 8);
    // Only the honest client is left, and it still runs.
    assert_eq!(harness.server().connection_count(), 1);
    assert!(harness.all_joined());
    // More garbage from the kicked connection changes nothing.
    raw.send(vec![1, 2, 3]);
    harness.run_idle(2);
    assert_eq!(harness.server().stats.connections_kicked, 1);
}

#[test]
fn flooding_inputs_is_rate_limited_and_does_not_disturb_an_honest_client() {
    let mut harness = harness_with(small_limits(), 1);
    let mut raw = joined_raw(&mut harness);
    for _ in 0..30 {
        let tick = harness.server().tick + 3;
        for _ in 0..5 {
            raw.send(input_bytes(tick, &["physics.push nothing 0 0 0"]));
        }
        harness.run_idle(1);
    }
    let stats = harness.server().stats;
    assert!(stats.inputs_rate_limited > 0, "{stats:?}");
    assert!(harness.all_joined(), "the honest client keeps playing");
    assert_eq!(harness.client(0).stats.corrections, 0);
}

#[test]
fn inputs_with_too_many_lines_a_long_line_or_a_wild_tick_are_refused_and_counted() {
    let limits = small_limits();
    let mut harness = harness_with(limits, 0);
    let mut raw = joined_raw(&mut harness);
    let now = harness.server().tick;
    let spawn = "physics.spawn_box evil 0 3 0 1";
    let many = vec![spawn; limits.max_lines_per_input + 1];
    raw.send(input_bytes(now + 2, &many));
    let long_line = format!("{spawn} {}", "x".repeat(limits.max_line_bytes));
    raw.send(input_bytes(now + 2, &[&long_line]));
    raw.send(input_bytes(now + limits.max_ticks_ahead + 100, &[spawn]));
    raw.send(input_bytes(u64::MAX, &[spawn]));
    harness.run_idle(2);
    let stats = harness.server().stats;
    assert_eq!(stats.inputs_bad_shape, 2, "{stats:?}");
    assert_eq!(stats.inputs_bad_tick, 2, "{stats:?}");
    harness.run_idle(limits.max_ticks_ahead + 200);
    assert!(
        harness.server().rejected_lines.is_empty(),
        "a refused input never reaches the command layer"
    );
    // An input that is only a little late still runs (late, not refused).
    raw.send(input_bytes(harness.server().tick, &[spawn]));
    harness.run_idle(3);
    assert_eq!(harness.server().late_inputs, 1);
}

#[test]
fn connections_beyond_the_cap_are_refused() {
    let limits = ServerLimits {
        max_connections: 2,
        ..small_limits()
    };
    let mut harness = harness_with(limits, 1);
    let _second = harness.network().connect();
    let mut third = harness.network().connect();
    harness.run_idle(2);
    assert_eq!(harness.server().connection_count(), 2);
    assert_eq!(harness.server().stats.connections_refused_full, 1);
    // The refused connection is ignored even if it talks.
    third.send(hello_bytes(None));
    harness.run_idle(2);
    assert_eq!(harness.server().client_count(), 1);
}

#[test]
fn a_connection_that_never_says_hello_is_dropped_after_the_join_timeout() {
    let limits = small_limits();
    let mut harness = harness_with(limits, 1);
    let _silent = harness.network().connect();
    harness.run_idle(2);
    assert_eq!(harness.server().connection_count(), 2);
    harness.run_idle(limits.join_timeout_ticks + 2);
    assert_eq!(harness.server().stats.join_timeouts, 1);
    assert_eq!(harness.server().connection_count(), 1);
    assert!(harness.all_joined(), "a joined client is never timed out");
}

/// Allows the token `good-<name>` as player `<name>` and denies everything else.
struct TokenCheck;

impl NetIdentityCheck for TokenCheck {
    fn check(&self, request: NetIdentityRequest) -> NetIdentityDecision {
        match request.token.and_then(|token| token.strip_prefix("good-")) {
            Some(name) => NetIdentityDecision::Allow {
                player_id: name.to_string(),
            },
            None => NetIdentityDecision::Deny {
                reason: "missing or unknown token".to_string(),
            },
        }
    }
}

#[test]
fn the_identity_hook_allows_with_a_stable_player_id_or_denies() {
    let mut harness = harness_with(small_limits(), 0);
    harness
        .server_app
        .insert_resource(NetIdentity::new(TokenCheck));
    let mut bad = harness.network().connect();
    let mut good = harness.network().connect();
    let mut no_token = harness.network().connect();
    bad.send(hello_bytes(Some("forged-admin")));
    good.send(hello_bytes(Some("good-ana")));
    no_token.send(hello_bytes(None));
    harness.run_idle(3);
    assert_eq!(harness.server().stats.identity_denied, 2);
    assert_eq!(harness.server().client_count(), 1);
    let ids = harness.server_app.world().resource::<NetPlayerIds>();
    assert_eq!(ids.0.values().collect::<Vec<_>>(), ["ana"]);
    // A denied connection is gone: the second hello changes nothing.
    bad.send(hello_bytes(Some("good-root")));
    harness.run_idle(3);
    assert_eq!(harness.server().client_count(), 1);
    // When the allowed client leaves, its player id goes too.
    harness.disconnect_client(1);
    harness.run_idle(3);
    assert_eq!(harness.server().client_count(), 0);
    assert!(
        harness
            .server_app
            .world()
            .resource::<NetPlayerIds>()
            .0
            .is_empty()
    );
}

#[test]
fn the_default_identity_check_accepts_everyone_and_a_long_token_is_refused() {
    let limits = small_limits();
    let mut harness = harness_with(limits, 1);
    let ids = harness.server_app.world().resource::<NetPlayerIds>();
    assert_eq!(ids.0.values().collect::<Vec<_>>(), ["guest-0"]);
    let mut raw = harness.network().connect();
    raw.send(hello_bytes(Some(&"t".repeat(limits.max_token_bytes + 1))));
    harness.run_idle(2);
    assert_eq!(harness.server().stats.tokens_too_long, 1);
    assert_eq!(harness.server().client_count(), 1);
}

#[test]
fn a_client_far_behind_real_time_is_dropped_input_by_input_but_never_kicked() {
    let limits = small_limits();
    let mut harness = harness_with(limits, 0);
    let mut raw = joined_raw(&mut harness);
    // Run the server far past the old tick, then send many inputs stamped with it.
    harness.run_idle(limits.max_ticks_behind + 50);
    for _ in 0..(limits.violations_before_mute * 3) {
        raw.send(input_bytes(1, &["physics.spawn_box slow 0 3 0 1"]));
        harness.run_idle(1);
    }
    let stats = harness.server().stats;
    assert_eq!(
        stats.inputs_too_old,
        u64::from(limits.violations_before_mute * 3)
    );
    assert_eq!(stats.connections_muted, 0);
    assert_eq!(stats.connections_kicked, 0);
    assert_eq!(harness.server().client_count(), 1);
}
