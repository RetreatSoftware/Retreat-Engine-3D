//! How long another player takes to see what a player does: the time from the first frame the
//! actor draws its walker moving to the first frame an observer draws it moving, on a localhost-like
//! link, after the things that disturb a real session (a slow join, a frozen window on either side,
//! a late joiner). The bound is `LOCAL_LATENCY_BUDGET_MS`; the delay must also **return** to it.

use std::time::Duration;

use bevy::prelude::*;
use retreat_engine::RenderSmoothingPlugin;
use retreat_engine::net::client::NetClient;
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::link::LinkConditions;
use retreat_engine::net::tick_pacing::{
    DRAIN_DEADBAND_TICKS, LOCAL_LATENCY_BUDGET_MS, TARGET_SLACK_TICKS,
};
use retreat_engine_api::{
    AppCommandExt, AppNetStateExt, CommandArgs, CommandOutcome, CommandResult, NetState,
    PhysicsBody, PhysicsControlSystems, PhysicsVelocitySet, RemoteInterpolation, RenderSmoothing,
};
use serde::{Deserialize, Serialize};

use crate::net_harness::make_app;

/// 75 frames per second.
const FRAME: Duration = Duration::from_micros(13_333);
const SECOND: u64 = 75;

/// The speed the walker was told to walk at: synced, set by its player's input.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Go(f32);

impl NetState for Go {
    const ID: &'static str = "test.go";
}

/// `test.go <speed>`.
fn go_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let speed: f32 = args.parse(0, "speed")?;
    for mut go in world.query::<&mut Go>().iter_mut(world) {
        go.0 = speed;
    }
    Ok(CommandOutcome::Continue)
}

fn walk(walkers: Query<(Entity, &Go)>, mut commands: Commands) {
    for (entity, go) in &walkers {
        commands
            .entity(entity)
            .insert(PhysicsVelocitySet(Vec3::X * go.0));
    }
}

fn latency_app() -> App {
    let mut app = make_app();
    app.add_plugins(RenderSmoothingPlugin)
        .register_net_state::<Go>()
        .register_command("test.go", "test.go <speed>", go_command)
        .add_systems(FixedUpdate, walk.in_set(PhysicsControlSystems));
    app
}

/// A server, the actor (client 0) and the observer (client 1) as two windows on one machine.
fn session() -> NetHarness {
    let link = LinkConditions {
        latency_ticks: 0,
        jitter_ticks: 0,
        loss: 0.0,
    };
    let config = HarnessConfig {
        // What a real client uses (`platform/native_transport.rs`).
        margin_ticks: 6,
        state_interval_ticks: 2,
        up: link,
        down: link,
        seed: 4,
    };
    let mut harness = NetHarness::new(config, &["test.go"], 2, latency_app);
    harness.server_app.world_mut().spawn((
        Name::new("runner"),
        PhysicsBody::character(0.5, 0.4),
        Transform::from_xyz(0.0, 0.9, 0.0),
        Go::default(),
    ));
    harness
}

fn run(harness: &mut NetHarness, seconds: u64) {
    harness.run_frames(FRAME, seconds * SECOND, |_, _| {});
}

fn runner(harness: &mut NetHarness, client: usize) -> Entity {
    let world = harness.client_apps[client].world_mut();
    world
        .query_filtered::<(Entity, &Name), With<Go>>()
        .iter(world)
        .find(|(_, name)| name.as_str() == "runner")
        .expect("the runner is replicated")
        .0
}

fn drawn(harness: &NetHarness, client: usize, entity: Entity) -> f32 {
    let world = harness.client_apps[client].world();
    world
        .get::<GlobalTransform>(entity)
        .expect("drawn")
        .translation()
        .x
}

/// The actor starts walking; returns how long after the actor's screen the observer's shows it.
/// The walker stops again and both settle.
fn seen_after(harness: &mut NetHarness) -> Duration {
    let (own, remote) = (runner(harness, 0), runner(harness, 1));
    harness.client_apps[0]
        .world_mut()
        .entity_mut(own)
        .insert(RenderSmoothing::default());
    harness.client_apps[1]
        .world_mut()
        .entity_mut(remote)
        .insert(RemoteInterpolation);
    run(harness, 1);
    let start = [drawn(harness, 0, own), drawn(harness, 1, remote)];
    harness.client_apps[0]
        .world_mut()
        .resource_mut::<NetClient>()
        .queue_input("test.go 3");
    let mut moved = [None, None];
    harness.run_frames(FRAME, 4 * SECOND, |harness, frame| {
        for (who, entity) in [own, remote].into_iter().enumerate() {
            if moved[who].is_none() && (drawn(harness, who, entity) - start[who]).abs() > 0.004 {
                moved[who] = Some(frame);
            }
        }
    });
    harness.client_apps[0]
        .world_mut()
        .resource_mut::<NetClient>()
        .queue_input("test.go 0");
    run(harness, 1);
    let own_frame = moved[0].expect("the actor sees itself move");
    let seen = moved[1].expect("the observer sees the actor move within 4 s");
    FRAME * u32::try_from(seen.saturating_sub(own_frame)).expect("small")
}

/// The delay is within the budget, and so is what the actor's inputs wait in the server.
fn assert_within_budget(harness: &mut NetHarness, when: &str) {
    let wait = harness.client(0).stats.input_wait_ticks.expect("reports");
    assert!(
        wait <= TARGET_SLACK_TICKS + DRAIN_DEADBAND_TICKS + 1,
        "{when}: the actor's inputs wait {wait} ticks in the server"
    );
    for attempt in 0..3 {
        let delay = seen_after(harness);
        assert!(
            delay <= Duration::from_millis(LOCAL_LATENCY_BUDGET_MS),
            "{when}, walk {attempt}: the observer sees the walk {delay:?} late"
        );
    }
}

#[test]
fn another_player_sees_an_action_within_the_budget_and_keeps_doing_so() {
    let mut harness = session();
    run(&mut harness, 6);
    assert_within_budget(&mut harness, "after joining");
    run(&mut harness, 60);
    assert_within_budget(&mut harness, "a minute later");
}

#[test]
fn a_join_during_slow_first_frames_does_not_leave_a_delay() {
    let mut harness = session();
    // The actor's window stalls between its hello and the answer: the measured round trip, and so
    // its starting lead, are far too large.
    harness.run_frames(FRAME, 1, |_, _| {});
    for _ in 0..4 {
        harness.set_client_frozen(0, true);
        harness.run_frames(FRAME, SECOND / 2, |_, _| {});
        harness.set_client_frozen(0, false);
        harness.run_frames(FRAME, 1, |_, _| {});
    }
    run(&mut harness, 6);
    assert_within_budget(&mut harness, "after a stalling join");
}

#[test]
fn the_delay_recovers_after_either_window_freezes_and_after_a_late_join() {
    let mut harness = session();
    // The observer joins ten seconds after the actor.
    harness.set_client_frozen(1, true);
    run(&mut harness, 10);
    harness.set_client_frozen(1, false);
    run(&mut harness, 6);
    assert_within_budget(&mut harness, "after a late join");
    for (who, client) in [("actor", 0), ("observer", 1)] {
        // A short stall, a dragged window, and one long enough to lose simulated time.
        for seconds in [1, 3] {
            harness.set_client_frozen(client, true);
            run(&mut harness, seconds);
            harness.set_client_frozen(client, false);
            run(&mut harness, 8);
            assert_within_budget(&mut harness, &format!("after the {who} froze {seconds} s"));
        }
    }
}

#[test]
fn the_other_player_moves_evenly_while_the_observer_drains_its_lead() {
    let mut harness = session();
    run(&mut harness, 6);
    let remote = runner(&mut harness, 1);
    harness.client_apps[1]
        .world_mut()
        .entity_mut(remote)
        .insert(RemoteInterpolation);
    harness.client_apps[0]
        .world_mut()
        .resource_mut::<NetClient>()
        .queue_input("test.go 3");
    harness.set_client_frozen(1, true);
    run(&mut harness, 3);
    harness.set_client_frozen(1, false);
    // Catching up takes well under a second; after that the observer only drains.
    run(&mut harness, 1);
    let mut positions = Vec::new();
    let mut slowed = false;
    harness.run_frames(FRAME, 4 * SECOND, |harness, _| {
        positions.push(drawn(harness, 1, remote));
        slowed |= harness.client(1).pacing.time_scale < 1.0;
    });
    assert!(
        slowed,
        "the observer drained nothing: the check proves nothing"
    );
    let expected = 3.0 * FRAME.as_secs_f32();
    let uneven: Vec<f32> = positions
        .windows(2)
        .map(|pair| (pair[1] - pair[0]) / expected)
        .filter(|ratio| !(0.75..=1.25).contains(ratio))
        .collect();
    assert!(
        uneven.is_empty(),
        "{} uneven frames: {:?}",
        uneven.len(),
        &uneven[..uneven.len().min(12)]
    );
}
