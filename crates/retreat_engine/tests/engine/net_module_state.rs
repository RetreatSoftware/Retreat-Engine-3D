//! Module state (`NetState`) in rollback and replication: predictions match, server-only decisions
//! correct the clients, and everything converges under lag and loss.

use bevy::prelude::*;
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::link::LinkConditions;
use retreat_engine::net::module_state::{ModuleState, read_module_states};
use retreat_engine::net::server::NetServer;
use retreat_engine::net::simulate_tick;
use retreat_engine::{RollbackError, capture_physics, compute_state_hash, restore_physics};
use retreat_engine_api::{
    AppCommandExt, AppNetStateExt, CommandArgs, CommandOutcome, CommandResult, NetState,
    PhysicsBody, RemoveNetEntity, RetiredEntity, encode_net_state,
};
use serde::{Deserialize, Serialize};

use crate::net_harness::make_app;

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Health {
    points: i32,
}

impl NetState for Health {
    const ID: &'static str = "test.health";
}

/// Counts fixed ticks: state a system changes every tick.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
struct MatchClock {
    ticks: u64,
}

impl NetState for MatchClock {
    const ID: &'static str = "test.match_clock";
}

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Label {
    text: String,
}

impl NetState for Label {
    const ID: &'static str = "test.label";
}

fn find(world: &mut World, name: &str) -> Option<Entity> {
    world
        .query_filtered::<(Entity, &Name), Without<RetiredEntity>>()
        .iter(world)
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity)
}

/// `test.add <name> <amount>`: adds to the entity's health, creating the entity if needed.
fn add_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let name: String = args.parse(0, "name")?;
    let amount: i32 = args.parse(1, "amount")?;
    let entity = match find(world, &name) {
        Some(entity) => entity,
        None => world.spawn(Name::new(name)).id(),
    };
    let points = world
        .get::<Health>(entity)
        .map_or(0, |health| health.points);
    world.entity_mut(entity).insert(Health {
        points: points + amount,
    });
    Ok(CommandOutcome::Continue)
}

/// `test.clock <name>`: starts a match clock.
fn clock_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    world.spawn((Name::new(name), MatchClock { ticks: 0 }));
    Ok(CommandOutcome::Continue)
}

/// `test.forget <name>`: removes an entity; no client may ask a server for it.
fn forget_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    let Some(entity) = find(world, &name) else {
        return Err(args.failure(format!("there is no `{name}`")));
    };
    world.entity_mut(entity).insert(RemoveNetEntity);
    Ok(CommandOutcome::Continue)
}

fn advance_clocks(mut clocks: Query<&mut MatchClock, Without<RetiredEntity>>) {
    for mut clock in &mut clocks {
        clock.ticks += 1;
    }
}

fn module_app() -> App {
    let mut app = make_app();
    app.register_net_state::<Health>()
        .register_net_state::<MatchClock>()
        .register_net_state::<Label>()
        .register_command("test.add", "test.add <name> <amount>", add_command)
        .register_command("test.clock", "test.clock <name>", clock_command)
        .register_command("test.forget", "test.forget <name>", forget_command)
        .add_systems(FixedUpdate, advance_clocks);
    app
}

const ALLOWED: &[&str] = &[
    "test.add",
    "test.clock",
    "net.remove",
    "physics.spawn_box",
    "physics.push",
];

fn link(latency_ticks: u32, jitter_ticks: u32, loss: f32) -> LinkConditions {
    LinkConditions {
        latency_ticks,
        jitter_ticks,
        loss,
    }
}

fn harness_with(margin_ticks: i64, conditions: LinkConditions, make: fn() -> App) -> NetHarness {
    let config = HarnessConfig {
        margin_ticks,
        state_interval_ticks: 2,
        up: conditions,
        down: conditions,
        seed: 5,
    };
    let mut harness = NetHarness::new(config, ALLOWED, 2, make);
    for _ in 0..600 {
        if harness.all_joined() {
            // Let the first full state arrive.
            harness.run_idle(40);
            return harness;
        }
        harness.run_idle(1);
    }
    panic!("the clients did not join within 600 ticks");
}

fn harness(margin_ticks: i64, conditions: LinkConditions) -> NetHarness {
    harness_with(margin_ticks, conditions, module_app)
}

fn send(harness: &mut NetHarness, client: usize, line: &str) {
    harness.step(&[(client, vec![line.to_string()])]);
}

/// The server changes state by itself (a decision clients cannot predict).
fn server_set_health(harness: &mut NetHarness, name: &str, points: i32) {
    let world = harness.server_app.world_mut();
    let entity = find(world, name).expect("on the server");
    world.entity_mut(entity).insert(Health { points });
}

fn corrections(harness: &NetHarness) -> Vec<u64> {
    (0..harness.client_apps.len())
        .map(|index| harness.client(index).stats.corrections)
        .collect()
}

/// Every client holds the server's module state: the same values now (for state at rest).
fn assert_converged(harness: &mut NetHarness) {
    let server = read_module_states(harness.server_app.world_mut());
    for index in 0..harness.client_apps.len() {
        let client = read_module_states(harness.client_apps[index].world_mut());
        assert_eq!(client, server, "client {index}");
        assert_eq!(harness.client(index).stats.refused_rollbacks, 0);
        assert_eq!(harness.client(index).stats.unusable_module_states, 0);
    }
}

fn health(world: &mut World, name: &str) -> Option<i32> {
    let entity = find(world, name)?;
    world.get::<Health>(entity).map(|health| health.points)
}

#[test]
fn predicted_state_matches_the_server_without_corrections() {
    let mut harness = harness(10, link(5, 0, 0.0));
    send(&mut harness, 0, "test.clock match");
    harness.run_idle(60);
    // Client 1 learns of the clock from the server, and from then on both clients predict it.
    let before = corrections(&harness);
    send(&mut harness, 0, "test.add hero 5");
    harness.run_idle(200);
    assert_eq!(harness.client(0).stats.corrections, before[0]);
    let (tick, _) = harness.server().last_broadcast.clone().expect("broadcast");
    harness.run_idle(1);
    assert!(tick > 0);
    for index in 0..2 {
        assert_eq!(
            health(harness.client_apps[index].world_mut(), "hero"),
            Some(5)
        );
    }
}

#[test]
fn a_ticking_value_is_predicted_exactly_at_the_tick_the_server_sends() {
    let mut harness = harness(1, LinkConditions::PERFECT);
    send(&mut harness, 0, "test.clock match");
    harness.run_idle(100);
    let before = corrections(&harness);
    harness.run_idle(100);
    assert_eq!(corrections(&harness), before, "no more corrections");
    let server_states = harness.server().last_broadcast_states.clone();
    let (tick, _) = harness.server().last_broadcast.clone().expect("broadcast");
    for index in 0..2 {
        let own: &[ModuleState] = harness
            .client(index)
            .module_states_at(tick)
            .expect("the client runs ahead of the server");
        assert_eq!(own, server_states, "client {index} at tick {tick}");
    }
}

#[test]
fn server_only_decisions_correct_the_clients_and_converge_despite_lag_and_loss() {
    let mut harness = harness(10, link(6, 3, 0.0));
    send(&mut harness, 0, "test.add hero 100");
    send(&mut harness, 1, "test.add villain 100");
    harness.run_idle(100);
    // Inputs are not resent, so loss starts after the players made them.
    let lossy = link(6, 3, 0.2);
    harness.set_links(lossy, lossy);
    let before = corrections(&harness);
    for points in [80, 55, 20] {
        server_set_health(&mut harness, "hero", points);
        harness.run_idle(17);
    }
    harness.run_idle(300);
    for (index, now) in corrections(&harness).into_iter().enumerate() {
        assert!(now > before[index], "client {index} was corrected");
        assert_eq!(
            health(harness.client_apps[index].world_mut(), "hero"),
            Some(20)
        );
    }
    assert_converged(&mut harness);
    let settled = corrections(&harness);
    harness.run_idle(200);
    assert_eq!(corrections(&harness), settled, "converged for good");
}

#[test]
fn late_inputs_are_corrected_and_converge() {
    let mut harness = harness(-4, link(5, 2, 0.0));
    send(&mut harness, 0, "test.clock match");
    harness.run_idle(30);
    for _ in 0..10 {
        send(&mut harness, 0, "test.add hero 1");
        send(&mut harness, 1, "test.add hero 10");
    }
    harness.run_idle(400);
    assert!(harness.server().late_inputs > 0);
    let server = health(harness.server_app.world_mut(), "hero");
    for index in 0..2 {
        assert_eq!(
            health(harness.client_apps[index].world_mut(), "hero"),
            server
        );
        assert_eq!(harness.client(index).stats.refused_rollbacks, 0);
    }
}

#[test]
fn components_added_and_removed_by_the_server_reach_the_clients() {
    let mut harness = harness(10, link(4, 0, 0.0));
    send(&mut harness, 0, "test.add hero 1");
    harness.run_idle(100);
    let hero = find(harness.server_app.world_mut(), "hero").expect("hero");
    harness
        .server_app
        .world_mut()
        .entity_mut(hero)
        .insert(Label {
            text: "winner".to_string(),
        });
    harness.run_idle(60);
    assert_converged(&mut harness);
    assert_eq!(read_module_states(harness.server_app.world_mut()).len(), 2);
    harness
        .server_app
        .world_mut()
        .entity_mut(hero)
        .remove::<Label>();
    // A removal makes the next broadcast full, so it arrives within a few round trips.
    harness.run_idle(30);
    assert_converged(&mut harness);
    assert_eq!(
        read_module_states(harness.client_apps[1].world_mut()).len(),
        1
    );
}

#[test]
fn removed_entities_are_retired_and_clients_follow_without_refusing_a_rollback() {
    let mut harness = harness(10, link(6, 2, 0.1));
    send(&mut harness, 0, "test.add hero 1");
    send(&mut harness, 0, "test.add villain 2");
    harness.run_idle(100);
    // One removal predicted by a client, one decided by the server alone.
    send(&mut harness, 0, "net.remove hero");
    let villain = find(harness.server_app.world_mut(), "villain").expect("villain");
    harness
        .server_app
        .world_mut()
        .entity_mut(villain)
        .insert(RemoveNetEntity);
    harness.run_idle(300);
    assert_converged(&mut harness);
    assert!(read_module_states(harness.server_app.world_mut()).is_empty());
    for index in 0..2 {
        let world = harness.client_apps[index].world_mut();
        assert_eq!(find(world, "hero"), None);
        assert_eq!(find(world, "villain"), None);
    }
    // After the rollback window the retired entities are gone.
    harness.run_idle(600);
    let server = harness.server_app.world_mut();
    assert!(server.get_entity(villain).is_err());
}

#[test]
fn a_removal_the_server_refuses_is_undone_on_the_client() {
    let mut harness = harness(10, link(4, 0, 0.0));
    send(&mut harness, 0, "test.add hero 7");
    harness.run_idle(200);
    // No client may ask the server for this, so only the client removes the entity.
    send(&mut harness, 0, "test.forget hero");
    harness.run_idle(200);
    assert_eq!(harness.server().rejected_lines.len(), 1);
    assert_converged(&mut harness);
    let world = harness.client_apps[0].world_mut();
    assert_eq!(health(world, "hero"), Some(7));
    let settled = corrections(&harness);
    harness.run_idle(100);
    assert_eq!(corrections(&harness), settled);
}

#[test]
fn state_on_a_physics_body_reaches_clients_that_lack_the_body() {
    let mut harness = harness(10, link(4, 0, 0.0));
    let server = harness.server_app.world_mut();
    server.spawn((
        Name::new("player"),
        PhysicsBody::dynamic_cube(1.0),
        Transform::from_xyz(0.0, 3.0, 0.0),
        Health { points: 42 },
    ));
    harness.run_idle(300);
    assert_converged(&mut harness);
    for index in 0..2 {
        let world = harness.client_apps[index].world_mut();
        let player = find(world, "player").expect("created from the state");
        assert!(world.get::<PhysicsBody>(player).is_some());
        assert_eq!(world.get::<Health>(player), Some(&Health { points: 42 }));
    }
}

#[test]
fn many_values_are_chunked_and_converge_with_loss() {
    let mut harness = harness(10, link(4, 0, 0.1));
    let server = harness.server_app.world_mut();
    for index in 0..60 {
        server.spawn((
            Name::new(format!("sign_{index:02}")),
            Label {
                text: "x".repeat(200),
            },
        ));
    }
    harness.run_idle(900);
    assert_converged(&mut harness);
    assert_eq!(
        read_module_states(harness.client_apps[0].world_mut()).len(),
        60
    );
    assert_eq!(harness.server().oversized_states, 0);
    assert_eq!(harness.undecodable_packets(), 0);
}

#[test]
fn a_value_too_large_for_a_datagram_is_counted_and_not_sent() {
    let mut harness = harness(10, LinkConditions::PERFECT);
    harness.server_app.world_mut().spawn((
        Name::new("huge"),
        Label {
            text: "x".repeat(2000),
        },
    ));
    harness.run_idle(40);
    assert!(harness.server().oversized_states > 0);
    assert!(read_module_states(harness.client_apps[0].world_mut()).is_empty());
}

/// A client built without the module's `Health` registration.
fn app_without_health_on_clients() -> App {
    let mut app = make_app();
    app.register_net_state::<MatchClock>();
    app
}

#[test]
fn state_a_client_cannot_read_is_counted_and_does_not_force_corrections() {
    let mut harness = harness_with(10, LinkConditions::PERFECT, app_without_health_on_clients);
    // Only the server learns the type (as if the client's build were older).
    harness.server_app.register_net_state::<Health>();
    harness
        .server_app
        .world_mut()
        .spawn((Name::new("hero"), Health { points: 1 }));
    harness.run_idle(100);
    assert!(harness.server().tick > 0);
    for index in 0..2 {
        let stats = harness.client(index).stats;
        assert!(stats.unusable_module_states > 0, "{stats:?}");
        assert_eq!(stats.corrections, 0, "{stats:?}");
    }
    assert!(harness.server_app.world().contains_resource::<NetServer>());
}

fn prepared() -> App {
    let mut app = module_app();
    retreat_engine::net::prepare_simulation_app(&mut app);
    simulate_tick(app.world_mut());
    app
}

#[test]
fn a_restore_puts_module_state_back_exactly_and_a_replay_repeats_it() {
    let mut app = prepared();
    let world = app.world_mut();
    let hero = world.spawn((Name::new("hero"), Health { points: 10 })).id();
    let bystander = world.spawn(Name::new("bystander")).id();
    world.spawn((Name::new("match"), MatchClock { ticks: 0 }));
    simulate_tick(world);
    let snapshot = capture_physics(world).expect("capture");
    let hash_at_capture = compute_state_hash(world).expect("hash").0;

    let advance = |world: &mut World| {
        for _ in 0..5 {
            simulate_tick(world);
        }
    };
    advance(world);
    let hash_after = compute_state_hash(world).expect("hash").0;
    assert_ne!(hash_after, hash_at_capture, "module state is hashed");
    // Changes after the capture: a value, a new component, a new entity, a removal.
    world.entity_mut(hero).insert(Health { points: 1 });
    world.entity_mut(bystander).insert(Health { points: 3 });
    let late = world.spawn((Name::new("late"), Health { points: 4 })).id();
    world.entity_mut(hero).insert(RemoveNetEntity);
    simulate_tick(world);
    assert!(world.get::<RetiredEntity>(hero).is_some());

    restore_physics(world, &snapshot).expect("restore");
    assert_eq!(world.get::<Health>(hero), Some(&Health { points: 10 }));
    assert!(world.get::<RetiredEntity>(hero).is_none(), "brought back");
    assert!(world.get::<Health>(bystander).is_none());
    assert!(world.get_entity(bystander).is_ok(), "it existed before");
    assert!(world.get_entity(late).is_err(), "spawned after the capture");
    assert_eq!(compute_state_hash(world).expect("hash").0, hash_at_capture);
    advance(world);
    assert_eq!(compute_state_hash(world).expect("hash").0, hash_after);
}

#[test]
fn a_directly_despawned_state_entity_refuses_the_restore_and_changes_nothing() {
    let mut app = prepared();
    let world = app.world_mut();
    let hero = world.spawn((Name::new("hero"), Health { points: 10 })).id();
    let other = world.spawn((Name::new("other"), Health { points: 2 })).id();
    let snapshot = capture_physics(world).expect("capture");
    world.despawn(hero);
    world.entity_mut(other).insert(Health { points: 9 });
    assert_eq!(
        restore_physics(world, &snapshot),
        Err(RollbackError::StateEntitiesDespawned(vec![
            "hero".to_string()
        ]))
    );
    assert_eq!(world.get::<Health>(other), Some(&Health { points: 9 }));
    assert_eq!(
        read_module_states(world)[0].data,
        encode_net_state(&Health { points: 9 })
    );
}
