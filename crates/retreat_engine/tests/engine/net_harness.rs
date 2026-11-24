//! The server-authoritative harness: clients predict, the server decides, corrections converge.
//! Every scenario runs a server and clients in one process over simulated links; nothing here
//! touches a real network.

use std::collections::BTreeMap;

use bevy::prelude::*;
use retreat_engine::net::body_state::{BodyState, read_body_states};
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::link::LinkConditions;
use retreat_engine::net::prepare_simulation_app;
use retreat_engine::{PhysicsPlugin, RetreatCommandsPlugin, StateHashPlugin};
use retreat_engine_api::{PhysicsBody, PhysicsBodyKind, PhysicsShape, Replicated, SyncedKind};

/// One simulation app: physics, commands and a ground slab. The same on server and clients.
pub(crate) fn make_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, AssetPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .add_plugins(RetreatCommandsPlugin::default())
        .add_plugins((PhysicsPlugin, StateHashPlugin))
        .add_plugins(retreat_engine::net::commands::NetCommandsPlugin);
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
    app
}

pub(crate) const ALLOWED: &[&str] = &["physics.spawn_box", "physics.push", "physics.remove"];

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
        if harness.all_joined() {
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
    let server = positions(&read_body_states(harness.server_app.world_mut()));
    for index in 0..harness.client_apps.len() {
        let client_positions = positions(&read_body_states(harness.client_apps[index].world_mut()));
        assert_eq!(
            client_positions.keys().collect::<Vec<_>>(),
            server.keys().collect::<Vec<_>>(),
            "client {index} must have exactly the bodies of the server"
        );
        for (name, server_position) in &server {
            let distance = client_positions[name].distance(*server_position);
            assert!(
                distance <= tolerance,
                "client {index}: `{name}` is {distance} away from the server (tolerance {tolerance})"
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
    let stats = harness.client(0).stats;
    assert_eq!(harness.server().late_inputs, 0);
    assert_eq!(stats.corrections, 0, "{stats:?}");
    assert!(stats.states_matching > 50, "{stats:?}");
    // The client runs ahead of the server, so compare once everything has come to rest.
    harness.run_idle(300);
    assert!(
        harness.server().rejected_lines.is_empty(),
        "{:?}",
        harness.server().rejected_lines
    );
    assert_clients_converged(&mut harness, 1e-3);
}

#[test]
fn with_latency_and_enough_lead_prediction_still_matches() {
    let mut harness = harness(config(10, link(5, 0, 0.0), link(5, 0, 0.0)), 1);
    run_plan(&mut harness, &two_box_plan(), 300);
    let stats = harness.client(0).stats;
    assert_eq!(harness.server().late_inputs, 0);
    assert_eq!(stats.corrections, 0, "{stats:?}");
    assert!(stats.states_matching > 50, "{stats:?}");
    harness.run_idle(300);
    assert_eq!(harness.client(0).stats.corrections, 0);
    assert_clients_converged(&mut harness, 1e-3);
}

#[test]
fn a_late_input_is_corrected_and_the_client_converges_to_the_server() {
    // A margin of -5 starts the client five ticks too close to the server: inputs arrive late.
    let mut harness = harness(config(-5, link(6, 0, 0.0), link(2, 0, 0.0)), 1);
    run_plan(&mut harness, &two_box_plan(), 400);
    assert!(harness.server().late_inputs > 0);
    let stats = harness.client(0).stats;
    assert!(stats.corrections > 0, "{stats:?}");
    assert!(stats.replayed_ticks > 0, "{stats:?}");
    assert_eq!(stats.refused_rollbacks, 0, "{stats:?}");
    assert_clients_converged(&mut harness, 0.05);
}

#[test]
fn a_body_removed_during_a_lag_window_with_corrections_pending_converges() {
    // Late inputs keep the client correcting while the removal is predicted, sent and confirmed.
    let mut harness = harness(config(-5, link(6, 0, 0.0), link(2, 0, 0.0)), 1);
    let plan: Plan = vec![
        (5, 0, "physics.spawn_box a 0 3 0 1"),
        (6, 0, "physics.spawn_box b 2 5 0 1"),
        (20, 0, "physics.push a 3 0 0"),
        (24, 0, "physics.remove b"),
        (26, 0, "physics.push a -1 0 1"),
        (30, 0, "physics.spawn_box c -2 5 0 1"),
        (34, 0, "physics.push c 1 0 0"),
    ];
    run_plan(&mut harness, &plan, 300);
    let stats = harness.client(0).stats;
    assert!(stats.corrections > 0, "{stats:?}");
    assert_eq!(stats.refused_rollbacks, 0, "{stats:?}");
    let names: Vec<String> = read_body_states(harness.server_app.world_mut())
        .into_iter()
        .map(|state| state.name)
        .collect();
    assert_eq!(names, vec!["a", "c", "ground"], "b left the server's state");
    assert_clients_converged(&mut harness, 0.05);
}

#[test]
fn a_lost_input_is_undone_on_the_client_because_the_server_never_saw_it() {
    let mut harness = harness(config(8, link(2, 0, 0.0), link(2, 0, 0.0)), 1);
    // Every message from the client to the server is lost from now on (the client has joined).
    harness.set_links(link(2, 0, 1.0), link(2, 0, 0.0));
    run_plan(&mut harness, &two_box_plan(), 300);
    let server_bodies = read_body_states(harness.server_app.world_mut());
    assert_eq!(
        server_bodies.len(),
        1,
        "only the ground exists on the server"
    );
    assert!(harness.client(0).stats.corrections > 0);
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
    let names: Vec<String> = read_body_states(harness.client_apps[0].world_mut())
        .into_iter()
        .map(|state| state.name)
        .collect();
    assert_eq!(names, ["from_a", "from_b", "ground"]);
    assert!(
        harness.client(0).stats.corrections > 0,
        "client 0 learns about from_b"
    );
    assert!(
        harness.client(1).stats.corrections > 0,
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
    let rejected = &harness.server().rejected_lines;
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
        harness.server().rejected_lines[0].contains("no entity is named `nobody`"),
        "{:?}",
        harness.server().rejected_lines
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
            (0..2)
                .map(|index| harness.client(index).stats)
                .collect::<Vec<_>>(),
            read_body_states(harness.server_app.world_mut())
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
    assert!(harness.server().late_inputs > 0);
    assert!(harness.client(0).stats.corrections > 0);
    assert_clients_converged(&mut harness, 0.05);
}

#[test]
fn physics_push_changes_a_bodys_velocity_and_refuses_fixed_or_unsimulated_bodies() {
    use retreat_engine_api::run_command_line;
    let mut app = make_app();
    prepare_simulation_app(&mut app);
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
    prepare_simulation_app(&mut by_update);
    setup(&mut by_update);
    for _ in 0..150 {
        by_update.update();
    }
    let mut by_tick = make_app();
    prepare_simulation_app(&mut by_tick);
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
        let round_trip = harness.client(0).measured_round_trip.expect("joined");
        let expected = u64::from(up + down);
        assert!(
            (expected..=expected + 2).contains(&round_trip),
            "latencies {up} and {down}: measured {round_trip}"
        );
        assert_eq!(harness.server().tick, harness.global_tick);
    }
    // With heavy loss the hello is repeated until a welcome gets through.
    let lossy = harness(config(2, link(2, 0, 0.7), link(2, 0, 0.7)), 2);
    assert!(lossy.all_joined());
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
        read_body_states(harness.server_app.world_mut())
            .iter()
            .all(|state| state.name != "early"),
        "input made before joining must be dropped"
    );
}

#[test]
fn net_commands_send_input_report_status_and_check_roles() {
    use retreat_engine_api::run_command_line;
    let mut harness = harness(
        config(1, LinkConditions::PERFECT, LinkConditions::PERFECT),
        2,
    );
    // The client sends a command line through `net.send` (everything after it is the line).
    let client_world = harness.client_apps[0].world_mut();
    run_command_line(client_world, "assert.net_joined").unwrap();
    run_command_line(client_world, "net.send physics.spawn_box via_net 0 3 0 1").unwrap();
    run_command_line(client_world, "net.status").unwrap();
    let error = run_command_line(client_world, "net.send").unwrap_err();
    assert!(
        error.to_string().contains("give the command line to send"),
        "{error}"
    );
    let error = run_command_line(client_world, "assert.net_clients 2").unwrap_err();
    assert!(
        error.to_string().contains("not a network server"),
        "{error}"
    );
    harness.run_idle(20);
    assert!(
        read_body_states(harness.server_app.world_mut())
            .iter()
            .any(|state| state.name == "via_net"),
        "the server ran the line"
    );
    // Server side.
    let server_world = harness.server_app.world_mut();
    run_command_line(server_world, "assert.net_clients 2").unwrap();
    let error = run_command_line(server_world, "assert.net_clients 3").unwrap_err();
    assert!(
        error.to_string().contains("knows 2 clients, expected 3"),
        "{error}"
    );
    run_command_line(server_world, "net.status").unwrap();
    run_command_line(server_world, "net.bodies").unwrap();
    let error = run_command_line(server_world, "assert.net_joined").unwrap_err();
    assert!(
        error.to_string().contains("not a network client"),
        "{error}"
    );
    let error = run_command_line(server_world, "net.send physics.push a 1 0 0").unwrap_err();
    assert!(
        error.to_string().contains("not a network client"),
        "{error}"
    );
}

#[test]
fn a_world_with_many_bodies_is_replicated_in_chunks() {
    let mut harness = harness(config(8, link(2, 1, 0.0), link(2, 1, 0.0)), 2);
    // 60 boxes: far more than one state message holds.
    let lines: Vec<String> = (0..60)
        .map(|index| {
            let x = (index % 10) as f32 * 1.2 - 5.0;
            let z = (index / 10) as f32 * 1.2 - 3.0;
            format!(
                "physics.spawn_box many_{index:02} {x} {} {z} 0.5",
                1.0 + (index % 3) as f32
            )
        })
        .collect();
    // A player may send at most `max_lines_per_input` lines per tick, so it takes a few ticks.
    for batch in lines.chunks(10) {
        harness.step(&[(0, batch.to_vec())]);
    }
    harness.run_idle(500);
    let names = |harness: &mut NetHarness, index: usize| -> usize {
        read_body_states(harness.client_apps[index].world_mut())
            .iter()
            .filter(|state| state.name.starts_with("many_"))
            .count()
    };
    assert_eq!(names(&mut harness, 0), 60);
    assert_eq!(
        names(&mut harness, 1),
        60,
        "client 1 learned all 60 through chunked states"
    );
    assert_clients_converged(&mut harness, 0.05);
    assert_eq!(harness.undecodable_packets(), 0);
}

/// Spawns `count` boxes in one tick from client 0 and lets them settle to rest.
fn spawn_resting_boxes(harness: &mut NetHarness, count: u32) {
    let lines: Vec<String> = (0..count)
        .map(|index| {
            let x = (index % 10) as f32 * 1.2 - 5.0;
            let z = (index / 10) as f32 * 1.2 - 3.0;
            format!("physics.spawn_box rest_{index:02} {x} 1 {z} 0.5")
        })
        .collect();
    harness.step(&[(0, lines)]);
    harness.run_idle(600);
}

#[test]
fn a_world_at_rest_costs_a_small_fraction_of_a_full_state_per_broadcast() {
    let mut harness = harness(config(8, link(2, 1, 0.0), link(2, 1, 0.0)), 2);
    spawn_resting_boxes(&mut harness, 60);
    let before = harness.bytes().1;
    harness.run_idle(640); // 10 seconds
    let per_client = (harness.bytes().1 - before) / 2 / 10;
    // Before deltas every broadcast listed all 60 bodies: about 60 * 90 bytes * 30 broadcasts/s.
    let full_state_every_broadcast = 60 * 90 * 30;
    println!(
        "resting 60 boxes: {per_client} B/s per client (was about {full_state_every_broadcast})"
    );
    assert!(
        per_client * 10 < full_state_every_broadcast,
        "{per_client} B/s per client for a resting world"
    );
    assert_clients_converged(&mut harness, 0.05);
}

#[test]
fn deltas_converge_with_loss_a_moving_body_among_resting_ones_and_a_removal() {
    let mut harness = harness(config(8, link(2, 1, 0.1), link(2, 1, 0.1)), 2);
    spawn_resting_boxes(&mut harness, 20);
    harness.step(&[(0, line("physics.push rest_03 4 6 0"))]);
    harness.step(&[(1, line("physics.remove rest_07"))]);
    harness.run_idle(900);
    assert_clients_converged(&mut harness, 0.05);
    assert_eq!(harness.undecodable_packets(), 0);
}

/// Records the join and leave messages the server writes, and the sender of every `test.whoami`.
#[derive(Resource, Default)]
struct Observed {
    joined: Vec<u32>,
    left: Vec<u32>,
    senders: Vec<Option<u32>>,
}

fn record_membership(
    mut joined: MessageReader<retreat_engine_api::NetClientJoined>,
    mut left: MessageReader<retreat_engine_api::NetClientLeft>,
    mut observed: ResMut<Observed>,
) {
    observed
        .joined
        .extend(joined.read().map(|message| message.client_id));
    observed
        .left
        .extend(left.read().map(|message| message.client_id));
}

#[test]
fn the_server_reports_joins_leaves_and_who_sent_each_command() {
    use retreat_engine_api::{AppCommandExt, AppNetExt, CommandOutcome, NetCommandSender};
    let make = || {
        let mut app = make_app();
        app.init_resource::<Observed>()
            .add_systems(FixedUpdate, record_membership)
            .allow_client_command("test.whoami")
            .register_command("test.whoami", "test.whoami", |world, _args| {
                let sender = world.resource::<NetCommandSender>().0;
                world.resource_mut::<Observed>().senders.push(sender);
                Ok(CommandOutcome::Continue)
            });
        app
    };
    let config = config(1, LinkConditions::PERFECT, LinkConditions::PERFECT);
    let mut harness = NetHarness::new(config, ALLOWED, 2, make);
    for _ in 0..50 {
        harness.run_idle(1);
    }
    assert!(harness.all_joined());
    harness.step(&[(1, line("test.whoami"))]);
    harness.run_idle(10);
    harness.disconnect_client(0);
    harness.run_idle(5);

    let server = harness.server_app.world().resource::<Observed>();
    assert_eq!(server.joined, [0, 1], "both clients joined");
    assert_eq!(server.left, [0], "client 0's connection ended");
    // The server saw the command as sent by client 1; client 1 predicted it as itself.
    assert_eq!(server.senders, [Some(1)]);
    assert_eq!(
        harness.client_apps[1]
            .world()
            .resource::<Observed>()
            .senders,
        [Some(1)]
    );
    // After the command the sender is reset.
    assert_eq!(
        harness.server_app.world().resource::<NetCommandSender>().0,
        None
    );
}

#[test]
fn a_velocity_change_waits_for_the_body_then_applies_once_and_is_removed() {
    use retreat_engine_api::PhysicsVelocityChange;
    let mut app = make_app();
    prepare_simulation_app(&mut app);
    let body =
        retreat_engine::spawn_physics_box(app.world_mut(), "mover", Vec3::new(0.0, 30.0, 0.0), 1.0);
    // Requested in the same tick the entity is spawned: the body does not exist yet.
    app.world_mut()
        .entity_mut(body)
        .insert(PhysicsVelocityChange(Vec3::new(5.0, 0.0, 0.0)));
    retreat_engine::net::simulate_tick(app.world_mut());
    assert!(
        app.world().entity(body).contains::<PhysicsVelocityChange>(),
        "the request waits until the body exists"
    );
    retreat_engine::net::simulate_tick(app.world_mut());
    retreat_engine::net::simulate_tick(app.world_mut());
    assert!(!app.world().entity(body).contains::<PhysicsVelocityChange>());
    let state = read_body_states(app.world_mut())
        .into_iter()
        .find(|state| state.name == "mover")
        .expect("mover");
    assert!((state.linear_velocity.x - 5.0).abs() < 1e-3, "{state:?}");
}

// --- Replication contract: every shape, and `Replicated` choosing what is synced ---

/// Spawns a dynamic ball directly in `world` (not through a command), as a module would.
fn spawn_ball(world: &mut World, name: &str, y: f32, replicated: Replicated) {
    world.spawn((
        Name::new(name.to_string()),
        PhysicsBody {
            kind: PhysicsBodyKind::Dynamic,
            shape: PhysicsShape::Ball { radius: 0.5 },
            shape_offset: Vec3::ZERO,
        },
        replicated,
        Transform::from_xyz(0.0, y, 0.0),
    ));
}

fn state_named(world: &mut World, name: &str) -> BodyState {
    read_body_states(world)
        .into_iter()
        .find(|state| state.name == name)
        .unwrap_or_else(|| panic!("no body named `{name}`"))
}

#[test]
fn a_ball_the_server_has_is_created_on_every_client_and_converges() {
    let mut harness = harness(config(8, link(2, 1, 0.0), link(2, 1, 0.0)), 2);
    spawn_ball(
        harness.server_app.world_mut(),
        "ball",
        6.0,
        Replicated::full(),
    );
    run_plan(&mut harness, &Plan::new(), 150);
    harness.run_idle(300);
    for index in 0..2 {
        let ball = state_named(harness.client_apps[index].world_mut(), "ball");
        assert_eq!(
            ball.description.shape,
            PhysicsShape::Ball { radius: 0.5 },
            "client {index} built the ball with the server's shape"
        );
    }
    assert_clients_converged(&mut harness, 0.05);
}

#[test]
fn replicated_lists_what_is_sent_and_a_pose_only_body_still_converges() {
    let mut harness = harness(config(8, link(2, 0, 0.0), link(2, 0, 0.0)), 1);
    spawn_ball(
        harness.server_app.world_mut(),
        "light",
        6.0,
        Replicated::of(&[SyncedKind::Pose]),
    );
    run_plan(&mut harness, &Plan::new(), 100);
    let server = state_named(harness.server_app.world_mut(), "light");
    assert_eq!(server.synced, vec![SyncedKind::Pose]);
    assert_eq!(server.linear_velocity, Vec3::ZERO, "velocity is not sent");
    harness.run_idle(300);
    let client = state_named(harness.client_apps[0].world_mut(), "light");
    assert_eq!(client.synced, vec![SyncedKind::Pose]);
    assert_clients_converged(&mut harness, 0.05);
}

#[test]
fn a_local_only_body_is_neither_sent_nor_removed_by_corrections() {
    let mut harness = harness(config(8, link(2, 0, 0.0), link(2, 0, 0.0)), 1);
    spawn_ball(
        harness.client_apps[0].world_mut(),
        "debris",
        4.0,
        Replicated::local_only(),
    );
    harness.run_idle(20);
    // A box from the server forces a correction on the client.
    run_plan(&mut harness, &two_box_plan(), 120);
    assert!(harness.client(0).stats.corrections > 0);
    let client_names: Vec<String> = read_body_states(harness.client_apps[0].world_mut())
        .into_iter()
        .map(|state| state.name)
        .collect();
    assert!(
        client_names.contains(&"debris".to_string()),
        "{client_names:?}"
    );
    let server_names: Vec<String> = read_body_states(harness.server_app.world_mut())
        .into_iter()
        .map(|state| state.name)
        .collect();
    assert!(!server_names.contains(&"debris".to_string()));
}
