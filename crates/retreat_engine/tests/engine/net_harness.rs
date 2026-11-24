//! The server-authoritative harness: clients predict, the server decides, corrections converge.
//! Every scenario runs a server and clients in one process over simulated links; nothing here
//! touches a real network.

use std::collections::BTreeMap;

use bevy::prelude::*;
use retreat_engine::net::body_state::{BodyState, read_body_states};
use retreat_engine::net::link::LinkConditions;
use retreat_engine::net::runner::ClientPhase;
use retreat_engine::net::{HarnessConfig, NetHarness, prepare_simulation_app};
use retreat_engine::{PhysicsPlugin, RetreatCommandsPlugin, StateHashPlugin};
use retreat_engine_api::{PhysicsBody, PhysicsBodyKind, PhysicsShape};

/// One simulation app: physics, commands and a ground slab. The same on server and clients.
pub(crate) fn make_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .add_plugins(RetreatCommandsPlugin::default())
        .add_plugins((PhysicsPlugin, StateHashPlugin));
    app.world_mut().spawn((
        Name::new("ground"),
        PhysicsBody {
            kind: PhysicsBodyKind::Fixed,
            shape: PhysicsShape::Cuboid {
                half_extents: Vec3::new(30.0, 0.5, 30.0),
            },
            shape_offset: Vec3::ZERO,
        },
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    prepare_simulation_app(&mut app);
    app
}

pub(crate) const ALLOWED: &[&str] = &["physics.spawn_box", "physics.push"];

fn config(margin_ticks: i64, up: LinkConditions, down: LinkConditions) -> HarnessConfig {
    HarnessConfig {
        margin_ticks,
        state_interval_ticks: 2,
        up,
        down,
        seed: 11,
    }
}

fn link(latency_ticks: u32, jitter_ticks: u32, loss: f32) -> LinkConditions {
    LinkConditions {
        latency_ticks,
        jitter_ticks,
        loss,
    }
}

/// A harness whose clients have all joined (the handshake is done), so a plan starts on a running game.
fn harness(config: HarnessConfig, clients: u32) -> NetHarness {
    let mut harness = NetHarness::new(config, ALLOWED, clients, make_app);
    for _ in 0..300 {
        if harness
            .clients
            .iter()
            .all(|client| client.phase == ClientPhase::Running)
        {
            return harness;
        }
        harness.run_idle(1);
    }
    panic!("the clients did not join within 300 ticks");
}

fn line(text: &str) -> Vec<String> {
    vec![text.to_string()]
}

/// A script of what players do: `(tick, client, line)`.
type Plan = Vec<(u64, usize, &'static str)>;

fn run_plan(harness: &mut NetHarness, plan: &Plan, ticks: u64) {
    for tick in 1..=ticks {
        let inputs: Vec<(usize, Vec<String>)> = plan
            .iter()
            .filter(|(at, _, _)| *at == tick)
            .map(|(_, client, text)| (*client, line(text)))
            .collect();
        harness.step(&inputs);
    }
}

fn positions(states: &[BodyState]) -> BTreeMap<String, Vec3> {
    states
        .iter()
        .map(|state| (state.name.clone(), state.translation))
        .collect()
}

/// Every client has exactly the bodies of the server, and each is within `tolerance` of it.
fn assert_clients_converged(harness: &mut NetHarness, tolerance: f32) {
    let server = positions(&read_body_states(harness.server.app.world_mut()));
    for client in &mut harness.clients {
        let client_positions = positions(&read_body_states(client.app.world_mut()));
        assert_eq!(
            client_positions.keys().collect::<Vec<_>>(),
            server.keys().collect::<Vec<_>>(),
            "client {} must have exactly the bodies of the server",
            client.id
        );
        for (name, server_position) in &server {
            let distance = client_positions[name].distance(*server_position);
            assert!(
                distance <= tolerance,
                "client {}: `{name}` is {distance} away from the server (tolerance {tolerance})",
                client.id
            );
        }
    }
}

fn two_box_plan() -> Plan {
    vec![
        (5, 0, "physics.spawn_box a 0 3 0 1"),
        (12, 0, "physics.push a 3 0 0"),
        (30, 0, "physics.spawn_box b 2 5 0 1"),
    ]
}

#[test]
fn with_perfect_links_prediction_always_matches_the_server() {
    // The client measures the round trip when it joins and starts that far ahead of the server, plus
    // the margin (1 tick covers the one tick any message needs to arrive).
    let mut harness = harness(
        config(1, LinkConditions::PERFECT, LinkConditions::PERFECT),
        1,
    );
    run_plan(&mut harness, &two_box_plan(), 200);
    let stats = harness.clients[0].stats;
    assert_eq!(harness.server.late_inputs, 0);
    assert_eq!(stats.corrections, 0, "{stats:?}");
    assert!(stats.states_matching > 50, "{stats:?}");
    // The client runs ahead of the server, so compare once everything has come to rest.
    harness.run_idle(300);
    assert!(
        harness.server.rejected_lines.is_empty(),
        "{:?}",
        harness.server.rejected_lines
    );
    assert_clients_converged(&mut harness, 1e-3);
}

#[test]
fn with_latency_and_enough_lead_prediction_still_matches() {
    let mut harness = harness(config(10, link(5, 0, 0.0), link(5, 0, 0.0)), 1);
    run_plan(&mut harness, &two_box_plan(), 300);
    let stats = harness.clients[0].stats;
    assert_eq!(harness.server.late_inputs, 0);
    assert_eq!(stats.corrections, 0, "{stats:?}");
    assert!(stats.states_matching > 50, "{stats:?}");
    harness.run_idle(300);
    assert_eq!(harness.clients[0].stats.corrections, 0);
    assert_clients_converged(&mut harness, 1e-3);
}

#[test]
fn a_late_input_is_corrected_and_the_client_converges_to_the_server() {
    // A margin of -5 starts the client five ticks too close to the server: inputs arrive late.
    let mut harness = harness(config(-5, link(6, 0, 0.0), link(2, 0, 0.0)), 1);
    run_plan(&mut harness, &two_box_plan(), 400);
    assert!(harness.server.late_inputs > 0);
    let stats = harness.clients[0].stats;
    assert!(stats.corrections > 0, "{stats:?}");
    assert!(stats.replayed_ticks > 0, "{stats:?}");
    assert_eq!(stats.refused_rollbacks, 0, "{stats:?}");
    assert_clients_converged(&mut harness, 0.05);
}

#[test]
fn a_lost_input_is_undone_on_the_client_because_the_server_never_saw_it() {
    let mut harness = harness(config(8, link(2, 0, 0.0), link(2, 0, 0.0)), 1);
    // Every message from the client to the server is lost from now on (the client has joined).
    harness.set_links(link(2, 0, 1.0), link(2, 0, 0.0));
    run_plan(&mut harness, &two_box_plan(), 300);
    let server_bodies = read_body_states(harness.server.app.world_mut());
    assert_eq!(
        server_bodies.len(),
        1,
        "only the ground exists on the server"
    );
    assert!(harness.clients[0].stats.corrections > 0);
    assert_clients_converged(&mut harness, 1e-3);
}

#[test]
fn loss_and_jitter_on_both_directions_still_converge_after_things_settle() {
    let mut harness = harness(config(6, link(3, 4, 0.2), link(3, 4, 0.2)), 1);
    run_plan(&mut harness, &two_box_plan(), 200);
    // No player input and a clean network: the bodies come to rest and the client catches up.
    harness.set_links(LinkConditions::PERFECT, LinkConditions::PERFECT);
    harness.run_idle(400);
    assert_clients_converged(&mut harness, 0.05);
}

#[test]
fn two_clients_see_each_others_bodies_through_the_server() {
    let mut harness = harness(config(8, link(3, 1, 0.0), link(3, 1, 0.0)), 2);
    let plan: Plan = vec![
        (5, 0, "physics.spawn_box from_a 0 3 0 1"),
        (9, 1, "physics.spawn_box from_b 3 4 0 1"),
        (20, 0, "physics.push from_a 2 0 0"),
        (24, 1, "physics.push from_b -2 0 0"),
    ];
    run_plan(&mut harness, &plan, 250);
    harness.run_idle(300);
    let names: Vec<String> = read_body_states(harness.clients[0].app.world_mut())
        .into_iter()
        .map(|state| state.name)
        .collect();
    assert_eq!(names, ["from_a", "from_b", "ground"]);
    assert!(
        harness.clients[0].stats.corrections > 0,
        "client 0 learns about from_b"
    );
    assert!(
        harness.clients[1].stats.corrections > 0,
        "client 1 learns about from_a"
    );
    assert_clients_converged(&mut harness, 0.05);
}

#[test]
fn the_server_refuses_commands_clients_may_not_run() {
    let mut harness = harness(
        config(2, LinkConditions::PERFECT, LinkConditions::PERFECT),
        1,
    );
    let plan: Plan = vec![(3, 0, "physics.reset")];
    run_plan(&mut harness, &plan, 20);
    let rejected = &harness.server.rejected_lines;
    assert_eq!(rejected.len(), 1, "{rejected:?}");
    assert!(rejected[0].contains("command `physics.reset` is not allowed from clients"));
}

#[test]
fn a_failing_input_is_reported_not_fatal() {
    let mut harness = harness(
        config(2, LinkConditions::PERFECT, LinkConditions::PERFECT),
        1,
    );
    let plan: Plan = vec![(3, 0, "physics.push nobody 1 0 0")];
    run_plan(&mut harness, &plan, 20);
    assert!(
        harness.server.rejected_lines[0].contains("no entity is named `nobody`"),
        "{:?}",
        harness.server.rejected_lines
    );
}

#[test]
fn the_same_setup_gives_the_same_outcome_twice() {
    let outcome = || {
        let mut harness = harness(config(2, link(6, 2, 0.1), link(2, 2, 0.1)), 2);
        let plan: Plan = vec![
            (5, 0, "physics.spawn_box a 0 3 0 1"),
            (7, 1, "physics.spawn_box b 1 4 0 1"),
            (15, 0, "physics.push a 2 0 0"),
        ];
        run_plan(&mut harness, &plan, 250);
        (
            harness
                .clients
                .iter()
                .map(|client| client.stats)
                .collect::<Vec<_>>(),
            read_body_states(harness.server.app.world_mut())
                .into_iter()
                .map(|state| state.translation.to_array().map(f32::to_bits))
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(outcome(), outcome());
}

#[test]
fn a_margin_below_the_need_makes_every_input_late_even_on_a_perfect_link() {
    let mut harness = harness(
        config(-1, LinkConditions::PERFECT, LinkConditions::PERFECT),
        1,
    );
    run_plan(&mut harness, &two_box_plan(), 200);
    assert!(harness.server.late_inputs > 0);
    assert!(harness.clients[0].stats.corrections > 0);
    assert_clients_converged(&mut harness, 0.05);
}

#[test]
fn physics_push_changes_a_bodys_velocity_and_refuses_fixed_or_unsimulated_bodies() {
    use retreat_engine_api::run_command_line;
    let mut app = make_app();
    run_command_line(app.world_mut(), "physics.spawn_box a 0 20 0 1").unwrap();
    // Before its first tick the body does not exist in the physics world yet.
    let error = run_command_line(app.world_mut(), "physics.push a 1 0 0").unwrap_err();
    assert!(error.to_string().contains("no physics body yet"), "{error}");
    app.update();
    app.update();
    run_command_line(app.world_mut(), "physics.push a 4 0 0").unwrap();
    let state = read_body_states(app.world_mut())
        .into_iter()
        .find(|state| state.name == "a")
        .expect("body a");
    assert!((state.linear_velocity.x - 4.0).abs() < 1e-4, "{state:?}");
    let error = run_command_line(app.world_mut(), "physics.push ground 1 0 0").unwrap_err();
    assert!(error.to_string().contains("fixed body"), "{error}");
    let error = run_command_line(app.world_mut(), "physics.push a 1 0").unwrap_err();
    assert!(error.to_string().contains("usage: physics.push"), "{error}");
}

#[test]
fn messages_cross_the_links_as_bytes_and_the_bandwidth_is_small() {
    let mut harness = harness(config(8, link(3, 1, 0.0), link(3, 1, 0.0)), 2);
    run_plan(&mut harness, &two_box_plan(), 640);
    assert_eq!(harness.undecodable_packets(), 0);
    // 640 ticks = 10 seconds of game time, two clients, a few bodies, a state every second tick.
    let seconds = 10;
    let down_per_client = harness.bytes().1 / 2 / seconds;
    let up_per_client = harness.bytes().0 / 2 / seconds;
    println!(
        "bandwidth: server to each client {down_per_client} B/s, each client to server {up_per_client} B/s"
    );
    assert!(
        down_per_client < 20_000,
        "{down_per_client} B/s down per client"
    );
    assert!(up_per_client < 2_000, "{up_per_client} B/s up per client");
    assert!(harness.bytes().0 > 0 && harness.bytes().1 > 0);
}

#[test]
fn simulate_tick_gives_the_same_state_as_a_normal_update() {
    use retreat_engine::compute_state_hash;
    use retreat_engine::net::simulate_tick;
    use retreat_engine_api::run_command_line;
    let setup = |app: &mut App| {
        run_command_line(app.world_mut(), "physics.spawn_box a 0 3 0 1").unwrap();
        run_command_line(app.world_mut(), "physics.spawn_box b 0.4 5 0 1").unwrap();
    };
    let mut by_update = make_app();
    setup(&mut by_update);
    for _ in 0..150 {
        by_update.update();
    }
    let mut by_tick = make_app();
    setup(&mut by_tick);
    for _ in 0..150 {
        simulate_tick(by_tick.world_mut());
    }
    assert_eq!(
        compute_state_hash(by_update.world_mut()).unwrap(),
        compute_state_hash(by_tick.world_mut()).unwrap()
    );
}

#[test]
fn the_handshake_measures_the_round_trip_and_survives_loss() {
    for (up, down) in [(0u32, 0u32), (3, 5), (8, 2)] {
        let harness = harness(config(1, link(up, 0, 0.0), link(down, 0, 0.0)), 1);
        let round_trip = harness.clients[0].measured_round_trip.expect("joined");
        let expected = u64::from(up + down);
        assert!(
            (expected..=expected + 2).contains(&round_trip),
            "latencies {up} and {down}: measured {round_trip}"
        );
        assert_eq!(harness.server.tick, harness.global_tick);
    }
    // With heavy loss the hello is repeated until a welcome gets through.
    let lossy = harness(config(2, link(2, 0, 0.7), link(2, 0, 0.7)), 2);
    assert!(
        lossy
            .clients
            .iter()
            .all(|c| c.phase == ClientPhase::Running)
    );
}

#[test]
fn input_before_the_handshake_is_never_applied() {
    let mut harness = NetHarness::new(
        config(1, link(5, 0, 0.0), link(5, 0, 0.0)),
        ALLOWED,
        1,
        make_app,
    );
    // The player acts at once, long before the client has joined.
    harness.step(&[(0, line("physics.spawn_box early 0 3 0 1"))]);
    harness.run_idle(200);
    assert!(
        read_body_states(harness.server.app.world_mut())
            .iter()
            .all(|state| state.name != "early"),
        "input made before joining must be dropped"
    );
}
