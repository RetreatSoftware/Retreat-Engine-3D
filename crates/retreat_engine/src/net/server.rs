//! The server side, as a resource plus systems inside the simulation's own `World`.
//!
//! [`install_server`] adds a [`NetServer`] resource, a transport (a non-`Send` resource: transports are
//! driven from the game thread) and two systems around every fixed tick:
//! - `server_before_tick` (`FixedFirst`): reads the transport, answers `Hello` with `Welcome`, buffers
//!   inputs and runs the command lines that are due for this tick (only commands on the allow-list);
//! - `server_after_tick` (`FixedLast`): counts the tick and, every `state_interval_ticks`, broadcasts the
//!   state of every physics body to every known client.
//!
//! Because these run inside `FixedMain`, a real app (Bevy's fixed loop) and the in-process harness
//! (`simulate_tick`) run exactly the same server code.

use std::collections::{BTreeMap, BTreeSet};

use bevy::app::{FixedFirst, FixedLast};
use bevy::prelude::*;
use retreat_engine_api::run_command_line;

use super::body_state::{BodyState, read_body_states};
use super::transport::{ConnectionId, ServerEvent, ServerTransport};
use super::wire;
use super::{ClientMessage, ServerMessage};

/// How the server behaves.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Command ids clients may run (anything else is refused).
    pub allowed_commands: Vec<String>,
    /// Ticks between state broadcasts (1 = every tick).
    pub state_interval_ticks: u64,
}

/// The authoritative simulation's networking state.
#[derive(Resource)]
pub struct NetServer {
    /// The last tick simulated (0 before the first).
    pub tick: u64,
    allowed_commands: BTreeSet<String>,
    /// Inputs waiting for their tick: tick -> (client id, lines), in arrival order.
    buffered: BTreeMap<u64, Vec<(u32, Vec<String>)>>,
    state_interval: u64,
    /// Connections that said hello.
    known: BTreeSet<ConnectionId>,
    /// Inputs that arrived after their tick and ran late.
    pub late_inputs: u64,
    /// Lines refused (command not allowed, or the command failed).
    pub rejected_lines: Vec<String>,
    /// The state broadcast most recently.
    pub last_broadcast: Option<(u64, Vec<BodyState>)>,
    /// Packets that could not be decoded.
    pub undecodable_packets: u64,
}

impl NetServer {
    /// How many clients have said hello (and are still connected).
    pub fn client_count(&self) -> usize {
        self.known.len()
    }
}

/// The server's transport (not `Send`: kept as a non-send resource).
struct ServerTransportBox(Box<dyn ServerTransport>);

/// Adds the server to `app`: the resource, the transport and the two tick systems.
pub fn install_server(
    app: &mut App,
    config: ServerConfig,
    transport: impl ServerTransport + 'static,
) {
    app.insert_resource(NetServer {
        tick: 0,
        allowed_commands: config.allowed_commands.into_iter().collect(),
        buffered: BTreeMap::new(),
        state_interval: config.state_interval_ticks.max(1),
        known: BTreeSet::new(),
        late_inputs: 0,
        rejected_lines: Vec::new(),
        last_broadcast: None,
        undecodable_packets: 0,
    })
    .insert_non_send(ServerTransportBox(Box::new(transport)))
    .add_systems(FixedFirst, server_before_tick)
    .add_systems(FixedLast, server_after_tick);
}

fn server_before_tick(world: &mut World) {
    let Some(mut transport) = world.remove_non_send::<ServerTransportBox>() else {
        return;
    };
    world.resource_scope(|world, mut server: Mut<NetServer>| {
        let next = server.tick + 1;
        for event in transport.0.poll() {
            match event {
                ServerEvent::Connected(_) => {}
                ServerEvent::Disconnected(id) => {
                    server.known.remove(&id);
                }
                ServerEvent::Packet { from, bytes } => match wire::decode_client_message(&bytes) {
                    Ok(ClientMessage::Hello { nonce }) => {
                        server.known.insert(from);
                        let welcome = ServerMessage::Welcome {
                            nonce,
                            client_id: from,
                            tick: server.tick,
                        };
                        transport
                            .0
                            .send(from, wire::encode_server_message(&welcome));
                    }
                    Ok(ClientMessage::Input { tick, lines }) => {
                        // Input from a connection that never said hello is ignored.
                        if server.known.contains(&from) {
                            // An input for a tick that is already running or past runs now, late.
                            let target = if tick < next {
                                server.late_inputs += 1;
                                next
                            } else {
                                tick
                            };
                            server
                                .buffered
                                .entry(target)
                                .or_default()
                                .push((from, lines));
                        }
                    }
                    Err(_) => server.undecodable_packets += 1,
                },
            }
        }
        let mut due = server.buffered.remove(&next).unwrap_or_default();
        due.sort_by_key(|(client_id, _)| *client_id);
        for (_, lines) in due {
            for line in lines {
                run_allowed_line(world, &mut server, &line);
            }
        }
    });
    world.insert_non_send(transport);
}

fn run_allowed_line(world: &mut World, server: &mut NetServer, line: &str) {
    let command_id = line.split_whitespace().next().unwrap_or("");
    if !server.allowed_commands.contains(command_id) {
        server.rejected_lines.push(format!(
            "`{line}`: command `{command_id}` is not allowed from clients"
        ));
        return;
    }
    if let Err(error) = run_command_line(world, line) {
        server.rejected_lines.push(format!("`{line}`: {error}"));
    }
}

fn server_after_tick(world: &mut World) {
    let Some(mut transport) = world.remove_non_send::<ServerTransportBox>() else {
        return;
    };
    let (due, tick, known) = {
        let mut server = world.resource_mut::<NetServer>();
        server.tick += 1;
        (
            server.tick.is_multiple_of(server.state_interval),
            server.tick,
            server.known.clone(),
        )
    };
    if due {
        let bodies = read_body_states(world);
        let bytes = wire::encode_server_message(&ServerMessage::State {
            tick,
            bodies: bodies.clone(),
        });
        for id in known {
            transport.0.send(id, bytes.clone());
        }
        world.resource_mut::<NetServer>().last_broadcast = Some((tick, bodies));
    }
    world.insert_non_send(transport);
}
