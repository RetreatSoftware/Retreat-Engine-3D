//! The networking core over real WebTransport on localhost: a server and two clients, each in its own
//! simulation, exchange datagrams through QUIC with a pinned self-signed certificate, join through the
//! handshake, predict, correct and converge. Everything runs in real time in one process.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use retreat_engine::net::body_state::{BodyState, read_body_states};
use retreat_engine::net::client::{
    ClientConfig, ClientPhase, NetClient, install_client, process_client_states,
};
use retreat_engine::net::server::{ServerConfig, install_server};
use retreat_engine::net::{prepare_simulation_app, simulate_tick};
use retreat_engine::{WebTransportClient, WebTransportServer};

use super::net_harness::{ALLOWED, make_app};

/// Real time between steps: short, so the test is quick, but long enough for packets to arrive.
const STEP_PAUSE: Duration = Duration::from_millis(3);

fn positions(states: &[BodyState]) -> Vec<(String, [f32; 3])> {
    states
        .iter()
        .map(|state| (state.name.clone(), state.translation.to_array()))
        .collect()
}

fn joined(app: &App) -> bool {
    app.world().resource::<NetClient>().phase == ClientPhase::Running
}

#[test]
fn two_clients_play_through_a_real_webtransport_server_and_converge() {
    let transport = WebTransportServer::start(0).expect("server");
    let url = format!("https://127.0.0.1:{}", transport.port);
    let hash = transport.certificate_hash_hex().to_string();
    let mut server = make_app();
    install_server(
        &mut server,
        ServerConfig {
            allowed_commands: ALLOWED.iter().map(|id| id.to_string()).collect(),
            state_interval_ticks: 2,
        },
        transport,
    );
    prepare_simulation_app(&mut server);
    let mut clients: Vec<App> = (0..2)
        .map(|id| {
            let transport = WebTransportClient::connect(&url, Some(&hash)).expect("client");
            let mut app = make_app();
            install_client(
                &mut app,
                ClientConfig {
                    margin_ticks: 6,
                    nonce: 500 + id,
                },
                transport,
            );
            prepare_simulation_app(&mut app);
            app
        })
        .collect();

    let started = Instant::now();
    let mut step = 0u32;
    let mut spawned = [false, false];
    let spawn_lines = [
        "physics.spawn_box net_a 0 3 0 1",
        "physics.spawn_box net_b 2 4 0 1",
    ];
    while started.elapsed() < Duration::from_secs(20) {
        step += 1;
        simulate_tick(server.world_mut());
        let all_joined = clients.iter().all(joined);
        for (index, app) in clients.iter_mut().enumerate() {
            // Once everybody has joined, each client spawns its own box (once).
            if all_joined && !spawned[index] {
                spawned[index] = true;
                app.world_mut()
                    .resource_mut::<NetClient>()
                    .queue_input(spawn_lines[index]);
            }
            process_client_states(app.world_mut());
            simulate_tick(app.world_mut());
        }
        // Done when the server has both boxes and every client agrees with it.
        if spawned.iter().all(|done| *done) && step > 400 {
            let server_bodies = positions(&read_body_states(server.world_mut()));
            let settled = clients.iter_mut().all(|app| {
                let own = positions(&read_body_states(app.world_mut()));
                own.len() == server_bodies.len()
                    && own.iter().zip(&server_bodies).all(|(a, b)| {
                        a.0 == b.0 && a.1.iter().zip(b.1).all(|(x, y)| (x - y).abs() < 0.05)
                    })
            });
            if settled && server_bodies.len() == 3 {
                break;
            }
        }
        std::thread::sleep(STEP_PAUSE);
    }

    let names: Vec<String> = read_body_states(server.world_mut())
        .into_iter()
        .map(|state| state.name)
        .collect();
    assert_eq!(
        names,
        ["ground", "net_a", "net_b"],
        "the server has every box"
    );
    let server_bodies = positions(&read_body_states(server.world_mut()));
    for (index, app) in clients.iter_mut().enumerate() {
        let own = positions(&read_body_states(app.world_mut()));
        let own_names: Vec<&String> = own.iter().map(|(name, _)| name).collect();
        assert_eq!(
            own_names,
            names.iter().collect::<Vec<_>>(),
            "client {index}"
        );
        let stats = app.world().resource::<NetClient>().stats;
        assert!(stats.states_received > 10, "client {index}: {stats:?}");
        for (a, b) in own.iter().zip(&server_bodies) {
            for (x, y) in a.1.iter().zip(b.1) {
                assert!(
                    (x - y).abs() < 0.05,
                    "client {index}: {} is at {:?}, the server has it at {:?}",
                    a.0,
                    a.1,
                    b.1
                );
            }
        }
    }
}
