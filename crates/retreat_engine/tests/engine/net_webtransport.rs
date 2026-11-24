//! The networking core over real WebTransport on localhost: a server and two clients, each in its own
//! simulation, exchange datagrams through QUIC with a pinned self-signed certificate, join through the
//! handshake, predict, correct and converge. Everything runs in real time in one process.

use std::time::{Duration, Instant};

use retreat_engine::net::body_state::{BodyState, read_body_states};
use retreat_engine::net::runner::{ClientPhase, ClientRunner, ServerRunner};
use retreat_engine::net::{ClientNode, ServerNode};
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

#[test]
fn two_clients_play_through_a_real_webtransport_server_and_converge() {
    let transport = WebTransportServer::start(0).expect("server");
    let url = format!("https://127.0.0.1:{}", transport.port);
    let hash = transport.certificate_hash_hex().to_string();
    let mut server = ServerRunner::new(ServerNode::new(make_app(), ALLOWED, 2), transport);
    let mut clients: Vec<ClientRunner<WebTransportClient>> = (0..2)
        .map(|id| {
            let transport = WebTransportClient::connect(&url, Some(&hash)).expect("client");
            ClientRunner::new(ClientNode::new(id, make_app(), 0), transport, 500 + id, 6)
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
        server.step();
        let joined = clients.iter().all(|c| c.phase == ClientPhase::Running);
        for (index, client) in clients.iter_mut().enumerate() {
            let mut lines = Vec::new();
            // Once everybody has joined, each client spawns its own box (once).
            if joined && !spawned[index] {
                spawned[index] = true;
                lines.push(spawn_lines[index].to_string());
            }
            client.step(lines);
        }
        let plan_done = spawned.iter().all(|done| *done);
        // Done when the server has both boxes, both clients have them, and they all rest in place.
        if plan_done && step > 400 {
            let server_bodies = positions(&read_body_states(server.node.app.world_mut()));
            let settled = clients.iter_mut().all(|client| {
                let own = positions(&read_body_states(client.node.app.world_mut()));
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

    let names: Vec<String> = read_body_states(server.node.app.world_mut())
        .into_iter()
        .map(|state| state.name)
        .collect();
    assert_eq!(
        names,
        ["ground", "net_a", "net_b"],
        "the server has every box"
    );
    for client in &mut clients {
        let own: Vec<String> = read_body_states(client.node.app.world_mut())
            .into_iter()
            .map(|state| state.name)
            .collect();
        assert_eq!(own, names, "client {} sees every box", client.node.id);
        assert!(
            client.node.stats.states_received > 10,
            "{:?}",
            client.node.stats
        );
    }
    let server_bodies = positions(&read_body_states(server.node.app.world_mut()));
    for client in &mut clients {
        let own = positions(&read_body_states(client.node.app.world_mut()));
        for (a, b) in own.iter().zip(&server_bodies) {
            for (x, y) in a.1.iter().zip(b.1) {
                assert!(
                    (x - y).abs() < 0.05,
                    "client {}: {} is at {:?}, the server has it at {:?}",
                    client.node.id,
                    a.0,
                    a.1,
                    b.1
                );
            }
        }
    }
}
