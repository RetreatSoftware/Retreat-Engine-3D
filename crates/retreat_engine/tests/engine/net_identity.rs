//! Client identity, one connection per player id, rejoins and where server records exist (crawler
//! plan CE1): a client that waits for a token says hello only once it has one, the token reaches the
//! server's identity check, a second connection with the same player id takes over the first, a
//! player who comes back is announced as a rejoin, and a client never has a `RecordStore`.

use bevy::prelude::*;
use retreat_engine::net::ClientMessage;
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::link::LinkConditions;
use retreat_engine::net::transport::ClientTransport;
use retreat_engine::net::wire::encode_client_message;
use retreat_engine::{RecordLocation, RecordsPlugin};
use retreat_engine_api::{
    NetClientIdentity, NetClientJoined, NetClientLeft, NetClientRejoined, NetIdentity,
    NetIdentityCheck, NetIdentityDecision, NetIdentityRequest, NetPlayerIds, RecordStore,
    run_command_line,
};

use super::net_harness::{ALLOWED, make_app};

const PERFECT: LinkConditions = LinkConditions {
    latency_ticks: 0,
    jitter_ticks: 0,
    loss: 0.0,
};

/// The token is the player id (a dev login by name); no token is denied.
struct NameIsPlayer;

impl NetIdentityCheck for NameIsPlayer {
    fn check(&self, request: NetIdentityRequest) -> NetIdentityDecision {
        match request.token {
            Some(name) => NetIdentityDecision::Allow {
                player_id: name.to_string(),
            },
            None => NetIdentityDecision::Deny {
                reason: "log in with a name".to_string(),
            },
        }
    }
}

/// What the server's modules heard, in order.
#[derive(Resource, Default, Debug)]
struct Heard {
    joined: Vec<u32>,
    left: Vec<u32>,
    rejoined: Vec<(u32, u32)>,
}

fn listen(
    mut heard: ResMut<Heard>,
    mut joined: MessageReader<NetClientJoined>,
    mut left: MessageReader<NetClientLeft>,
    mut rejoined: MessageReader<NetClientRejoined>,
) {
    heard.joined.extend(joined.read().map(|m| m.client_id));
    heard.left.extend(left.read().map(|m| m.client_id));
    heard
        .rejoined
        .extend(rejoined.read().map(|m| (m.client_id, m.previous_client_id)));
}

fn identity_app() -> App {
    let mut app = make_app();
    app.insert_resource(NetIdentity::new(NameIsPlayer))
        .init_resource::<Heard>()
        .add_plugins(RecordsPlugin {
            location: RecordLocation::Memory,
        })
        .add_systems(FixedUpdate, listen);
    app
}

/// A harness whose clients each wait for a token (none set yet).
fn waiting_harness(clients: u32) -> NetHarness {
    let config = HarnessConfig {
        margin_ticks: 2,
        state_interval_ticks: 2,
        up: PERFECT,
        down: PERFECT,
        seed: 3,
    };
    let mut harness = NetHarness::new(config, ALLOWED, clients, identity_app);
    for client in &mut harness.client_apps {
        client.insert_resource(NetClientIdentity {
            wait_for_token: true,
            token: None,
        });
    }
    harness
}

fn identify(harness: &mut NetHarness, client: usize, name: &str) {
    run_command_line(
        harness.client_apps[client].world_mut(),
        &format!("net.identify {name}"),
    )
    .expect("net.identify runs on a client");
}

fn run_until(harness: &mut NetHarness, done: impl Fn(&NetHarness) -> bool) {
    for _ in 0..200 {
        if done(harness) {
            return;
        }
        harness.run_idle(1);
    }
    panic!("the condition did not hold within 200 ticks");
}

fn heard(harness: &NetHarness) -> &Heard {
    harness.server_app.world().resource::<Heard>()
}

fn client_id(harness: &NetHarness, client: usize) -> u32 {
    harness.client(client).id
}

#[test]
fn a_client_waiting_for_a_token_joins_only_once_it_has_one_and_as_that_player() {
    let mut harness = waiting_harness(1);
    harness.run_idle(60);
    assert!(!harness.all_joined(), "no hello without a token");
    assert_eq!(harness.server().client_count(), 0);
    identify(&mut harness, 0, "alice");
    run_until(&mut harness, NetHarness::all_joined);
    let id = client_id(&harness, 0);
    let ids = harness.server_app.world().resource::<NetPlayerIds>();
    assert_eq!(ids.player_of(id), Some("alice"));
    assert_eq!(heard(&harness).rejoined, [], "a first join is no rejoin");
    run_command_line(
        harness.server_app.world_mut(),
        &format!("assert.net_player {id} alice"),
    )
    .unwrap();
    let wrong = run_command_line(
        harness.server_app.world_mut(),
        &format!("assert.net_player {id} bob"),
    )
    .unwrap_err();
    assert!(
        wrong.to_string().contains("joined as player `alice`"),
        "{wrong}"
    );
}

#[test]
fn a_second_connection_of_the_same_player_takes_over_and_is_announced_as_a_rejoin() {
    let mut harness = waiting_harness(2);
    identify(&mut harness, 0, "alice");
    run_until(&mut harness, |h| h.server().client_count() == 1);
    let first = client_id(&harness, 0);
    identify(&mut harness, 1, "alice");
    run_until(&mut harness, |h| !heard(h).rejoined.is_empty());
    let second = client_id(&harness, 1);
    assert_ne!(first, second);
    assert_eq!(
        harness.server().client_count(),
        1,
        "one connection per player"
    );
    assert_eq!(harness.server().stats.connections_replaced, 1);
    assert_eq!(
        harness.server().stats.refusals(),
        0,
        "a takeover is no refusal"
    );
    let heard = heard(&harness);
    assert_eq!(heard.joined, [first, second]);
    assert_eq!(heard.left, [first], "the older connection left");
    assert_eq!(heard.rejoined, [(second, first)]);
    let ids = harness.server_app.world().resource::<NetPlayerIds>();
    assert_eq!(ids.client_of("alice"), Some(second));
    assert_eq!(ids.player_of(first), None);
}

#[test]
fn a_player_who_comes_back_after_leaving_is_announced_as_a_rejoin() {
    let mut harness = waiting_harness(1);
    identify(&mut harness, 0, "bob");
    run_until(&mut harness, NetHarness::all_joined);
    let first = client_id(&harness, 0);
    harness.disconnect_client(0);
    run_until(&mut harness, |h| h.server().client_count() == 0);
    // A new connection (a reload of the page) with the same login.
    let mut again = harness.network().connect();
    again.send(encode_client_message(&ClientMessage::Hello {
        nonce: 77,
        token: Some("bob".to_string()),
    }));
    run_until(&mut harness, |h| h.server().client_count() == 1);
    let heard = heard(&harness);
    assert_eq!(heard.left, [first]);
    let (new, previous) = heard.rejoined[0];
    assert_eq!(previous, first);
    assert_ne!(new, first);
    assert_eq!(
        harness.server().stats.connections_replaced,
        0,
        "nothing to take over"
    );
}

#[test]
fn only_the_server_keeps_records() {
    let harness = waiting_harness(1);
    assert!(
        harness
            .server_app
            .world()
            .contains_resource::<RecordStore>()
    );
    assert!(
        !harness.client_apps[0]
            .world()
            .contains_resource::<RecordStore>(),
        "a client never has server records"
    );
}
