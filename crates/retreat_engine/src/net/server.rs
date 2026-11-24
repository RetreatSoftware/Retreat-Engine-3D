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
use retreat_engine_api::{
    NetAllowedCommands, NetClientJoined, NetClientLeft, NetCommandSender, run_command_line,
};

use super::body_state::{BodyState, read_replicated_body_states};
use super::transport::{ConnectionId, ServerEvent, ServerTransport};
use super::wire;
use super::{ClientMessage, MAX_BODIES_PER_STATE_MESSAGE, ServerMessage};

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
    // The allow-list is a contract resource that modules add to; the config's ids are added to it.
    app.init_resource::<NetAllowedCommands>();
    app.world_mut()
        .resource_mut::<NetAllowedCommands>()
        .0
        .extend(config.allowed_commands);
    app.insert_resource(NetServer {
        tick: 0,
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
                    if server.known.remove(&id) {
                        world.write_message(NetClientLeft { client_id: id });
                    }
                }
                ServerEvent::Packet { from, bytes } => match wire::decode_client_message(&bytes) {
                    Ok(ClientMessage::Hello { nonce }) => {
                        if server.known.insert(from) {
                            world.write_message(NetClientJoined { client_id: from });
                        }
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
        for (client_id, lines) in due {
            for line in lines {
                run_allowed_line(world, &mut server, client_id, &line);
            }
        }
    });
    world.insert_non_send(transport);
}

fn run_allowed_line(world: &mut World, server: &mut NetServer, client_id: u32, line: &str) {
    let command_id = line.split_whitespace().next().unwrap_or("");
    let allowed = world
        .resource::<NetAllowedCommands>()
        .0
        .contains(command_id);
    if !allowed {
        server.rejected_lines.push(format!(
            "`{line}`: command `{command_id}` is not allowed from clients"
        ));
        return;
    }
    world.insert_resource(NetCommandSender(Some(client_id)));
    let result = run_command_line(world, line);
    world.insert_resource(NetCommandSender(None));
    if let Err(error) = result {
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
        let bodies = read_replicated_body_states(world);
        // At least one message even when there are no bodies, so the client still hears the tick.
        let pieces: Vec<&[BodyState]> = if bodies.is_empty() {
            vec![&[]]
        } else {
            bodies.chunks(MAX_BODIES_PER_STATE_MESSAGE).collect()
        };
        for (index, piece) in pieces.iter().enumerate() {
            let bytes = wire::encode_server_message(&ServerMessage::State {
                tick,
                chunk: index as u16,
                chunks: pieces.len() as u16,
                bodies: piece.to_vec(),
            });
            for id in &known {
                transport.0.send(*id, bytes.clone());
            }
        }
        world.resource_mut::<NetServer>().last_broadcast = Some((tick, bodies));
    }
    world.insert_non_send(transport);
}
