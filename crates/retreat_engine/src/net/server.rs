//! The server side, as a resource plus systems inside the simulation's own `World`.
//!
//! [`install_server`] adds a [`NetServer`] resource, a transport (a non-`Send` resource: transports are
//! driven from the game thread) and two systems around every fixed tick:
//! - `server_before_tick` (`FixedFirst`): reads the transport, answers `Hello` with `Welcome` (after the
//!   identity check), buffers inputs and runs the command lines that are due for this tick (only commands
//!   on the allow-list);
//! - `server_after_tick` (`FixedLast`): counts the tick and, every `state_interval_ticks`, broadcasts the
//!   state of the physics bodies to every known client (full or delta, see `state_delta.rs`).
//!
//! **Hardening** (limits in `limits.rs`, counters in [`ServerStats`]): every datagram passes its
//! connection's `ConnectionGuard` (size, rate, mute for repeat offenders) before it is decoded; inputs
//! are checked for line count, line length and a sane tick; connections are capped and a connection that
//! never joins is dropped; who may join is decided by the contract's `NetIdentity`. Nothing a client
//! sends can panic the server: every refusal is counted.
//!
//! Because these run inside `FixedMain`, a real app (Bevy's fixed loop) and the in-process harness
//! (`simulate_tick`) run exactly the same server code.

use std::collections::{BTreeMap, BTreeSet};

use bevy::app::{FixedFirst, FixedLast};
use bevy::prelude::*;
use retreat_engine_api::{
    MAX_REWIND_TICKS, NetAllowedCommands, NetClientJoined, NetClientLeft, NetCommandSender,
    NetIdentity, NetIdentityDecision, NetIdentityRequest, NetInputTick, NetPlayerIds,
    NetStateRegistry, run_command_line,
};

use super::body_state::{BodyState, read_replicated_body_states};
use super::lag_compensation::{PoseHistory, read_net_poses};
use super::limits::{ConnectionGuard, Refusal, ServerLimits, Verdict, check_input_content};
use super::module_state::{MAX_MODULE_STATE_BYTES, ModuleState, encoded_size, read_module_states};
use super::state_delta::{DeltaEncoder, ReplicatedState, split_into_chunks};
use super::tick_pacing::{InputWindow, TIMING_INTERVAL_TICKS, schedule_input};
use super::transport::{ConnectionId, ServerEvent, ServerTransport};
use super::wire;
use super::{ClientMessage, ServerMessage};
use crate::command_script::fail_running_script;

/// How the server behaves.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Command ids clients may run (anything else is refused).
    pub allowed_commands: Vec<String>,
    /// Ticks between state broadcasts (1 = every tick).
    pub state_interval_ticks: u64,
    /// Rate limits, validation caps, connection cap and join timeout.
    pub limits: ServerLimits,
}

/// How often the server refused something. Every counter only grows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServerStats {
    /// Datagrams dropped unread because their connection was muted.
    pub datagrams_muted: u64,
    /// Datagrams larger than `max_datagram_bytes`.
    pub datagrams_too_large: u64,
    /// Datagrams over the per-window limit.
    pub datagrams_rate_limited: u64,
    /// Inputs over the per-window limit.
    pub inputs_rate_limited: u64,
    /// Inputs with too many lines or a line too long.
    pub inputs_bad_shape: u64,
    /// Inputs whose tick was too far ahead of the server's (a violation).
    pub inputs_bad_tick: u64,
    /// Inputs whose tick was too far behind the server's (dropped; not a violation).
    pub inputs_too_old: u64,
    /// Hellos whose token was too long.
    pub tokens_too_long: u64,
    /// Times a connection was muted (see `limits.rs`).
    pub connections_muted: u64,
    /// Connections disconnected for repeated violations.
    pub connections_kicked: u64,
    /// Connections refused because `max_connections` were open.
    pub connections_refused_full: u64,
    /// Connections disconnected because they never joined in time.
    pub join_timeouts: u64,
    /// Hellos the identity check denied.
    pub identity_denied: u64,
    /// Bytes of every datagram received (including refused ones).
    pub bytes_received: u64,
    /// Bytes of every datagram sent (one count per receiving connection).
    pub bytes_sent: u64,
}

impl ServerStats {
    /// Every refusal counter added up: what `assert.net_refusals_at_most` checks. Byte counters
    /// are not refusals.
    pub fn refusals(&self) -> u64 {
        self.datagrams_muted
            + self.datagrams_too_large
            + self.datagrams_rate_limited
            + self.inputs_rate_limited
            + self.inputs_bad_shape
            + self.inputs_bad_tick
            + self.tokens_too_long
            + self.connections_muted
            + self.connections_kicked
            + self.connections_refused_full
            + self.join_timeouts
            + self.identity_denied
    }
}

/// What the server tracks per open connection.
struct ConnectionState {
    guard: ConnectionGuard,
    connected_at: u64,
    /// Mutes of `guard` already added to `ServerStats::connections_muted`.
    mutes_counted: u32,
    /// How early its inputs arrived since the last timing report.
    timing: InputWindow,
    /// The tick of the last late input scheduled for it (see `schedule_input`).
    spread_end: u64,
    /// The highest input tick run so far (sent back with every state).
    ran_input: u64,
}

/// One input waiting to run: (client id, the client's tick of it, command lines).
type BufferedInput = (u32, u64, Vec<String>);

/// The authoritative simulation's networking state.
#[derive(Resource)]
pub struct NetServer {
    /// The last tick simulated (0 before the first).
    pub tick: u64,
    /// Inputs waiting for their tick: tick -> (client id, input tick, lines), in arrival order.
    buffered: BTreeMap<u64, Vec<BufferedInput>>,
    state_interval: u64,
    /// Connections that said hello and were let in.
    known: BTreeSet<ConnectionId>,
    /// Inputs that arrived after their tick and ran late.
    pub late_inputs: u64,
    /// Lines refused (command not allowed, or the command failed).
    pub rejected_lines: Vec<String>,
    /// The state broadcast most recently.
    pub last_broadcast: Option<(u64, Vec<BodyState>)>,
    /// The module states of the broadcast made most recently.
    pub last_broadcast_states: Vec<ModuleState>,
    /// Module state values too large for a datagram, never sent (each also fails a running script).
    pub oversized_states: u64,
    /// Packets that could not be decoded.
    pub undecodable_packets: u64,
    /// Refusal counters.
    pub stats: ServerStats,
    limits: ServerLimits,
    /// Every open connection, joined or not.
    connections: BTreeMap<ConnectionId, ConnectionState>,
    /// Decides which bodies each broadcast lists (see `state_delta.rs`).
    delta: DeltaEncoder,
}

impl NetServer {
    /// How many clients have said hello (and are still connected).
    pub fn client_count(&self) -> usize {
        self.known.len()
    }

    /// How many connections are open, joined or not.
    pub fn connection_count(&self) -> usize {
        self.connections.len()
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
    app.init_resource::<NetIdentity>();
    app.init_resource::<NetPlayerIds>();
    app.init_resource::<NetStateRegistry>();
    app.init_resource::<PoseHistory>();
    app.insert_resource(NetServer {
        tick: 0,
        buffered: BTreeMap::new(),
        state_interval: config.state_interval_ticks.max(1),
        known: BTreeSet::new(),
        late_inputs: 0,
        rejected_lines: Vec::new(),
        last_broadcast: None,
        last_broadcast_states: Vec::new(),
        oversized_states: 0,
        undecodable_packets: 0,
        stats: ServerStats::default(),
        limits: config.limits,
        connections: BTreeMap::new(),
        delta: DeltaEncoder::default(),
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
                ServerEvent::Connected(id) => {
                    if server.connections.len() >= server.limits.max_connections {
                        server.stats.connections_refused_full += 1;
                        warn!(
                            "net server: connection {id} refused: {} connections are open (`max_connections`)",
                            server.connections.len()
                        );
                        transport.0.disconnect(id);
                    } else {
                        server.connections.insert(
                            id,
                            ConnectionState {
                                guard: ConnectionGuard::new(next),
                                connected_at: next,
                                mutes_counted: 0,
                                timing: InputWindow::default(),
                                spread_end: 0,
                                ran_input: 0,
                            },
                        );
                    }
                }
                ServerEvent::Disconnected(id) => forget_connection(world, &mut server, id),
                ServerEvent::Packet { from, bytes } => {
                    server.stats.bytes_received += bytes.len() as u64;
                    handle_packet(world, &mut server, &mut *transport.0, next, from, &bytes);
                }
            }
        }
        drop_connections_that_never_joined(world, &mut server, &mut *transport.0, next);
        let mut due = server.buffered.remove(&next).unwrap_or_default();
        due.sort_by_key(|(client_id, ..)| *client_id);
        for (client_id, input_tick, lines) in due {
            // A client that was disconnected after sending runs nothing.
            if !server.known.contains(&client_id) {
                continue;
            }
            if let Some(state) = server.connections.get_mut(&client_id) {
                state.ran_input = state.ran_input.max(input_tick);
            }
            world
                .get_resource_or_insert_with(NetInputTick::default)
                .begin(Some(input_tick));
            for line in lines {
                run_allowed_line(world, &mut server, client_id, &line);
            }
            world.resource_mut::<NetInputTick>().begin(None);
        }
    });
    world.insert_non_send(transport);
}

/// Forgets a connection: tells modules the client left when it had joined.
fn forget_connection(world: &mut World, server: &mut NetServer, id: ConnectionId) {
    server.connections.remove(&id);
    if server.known.remove(&id) {
        world.write_message(NetClientLeft { client_id: id });
        world.resource_mut::<NetPlayerIds>().0.remove(&id);
    }
}

/// Ends a connection from the server's side and forgets it.
fn drop_connection(
    world: &mut World,
    server: &mut NetServer,
    transport: &mut dyn ServerTransport,
    id: ConnectionId,
) {
    transport.disconnect(id);
    forget_connection(world, server, id);
}

/// Counts a refusal in the server's stats.
fn count_refusal(stats: &mut ServerStats, refusal: Refusal) {
    match refusal {
        Refusal::Muted => stats.datagrams_muted += 1,
        Refusal::DatagramTooLarge => stats.datagrams_too_large += 1,
        Refusal::DatagramRate => stats.datagrams_rate_limited += 1,
        Refusal::InputRate => stats.inputs_rate_limited += 1,
        // Counted in `NetServer::undecodable_packets`.
        Refusal::Undecodable => {}
        Refusal::InputShape => stats.inputs_bad_shape += 1,
        Refusal::InputTick => stats.inputs_bad_tick += 1,
        Refusal::InputTooOld => stats.inputs_too_old += 1,
        Refusal::TokenTooLong => stats.tokens_too_long += 1,
    }
}

/// Applies a guard's verdict: counts the refusal and disconnects an offender. Returns true when the
/// datagram or input may be used.
fn apply_verdict(
    world: &mut World,
    server: &mut NetServer,
    transport: &mut dyn ServerTransport,
    id: ConnectionId,
    verdict: Verdict,
) -> bool {
    if let Some(state) = server.connections.get_mut(&id) {
        let mutes = state.guard.mutes();
        server.stats.connections_muted += u64::from(mutes - state.mutes_counted);
        state.mutes_counted = mutes;
    }
    match verdict {
        Verdict::Accept => true,
        Verdict::Refuse(refusal) => {
            count_refusal(&mut server.stats, refusal);
            false
        }
        Verdict::Disconnect(refusal) => {
            count_refusal(&mut server.stats, refusal);
            server.stats.connections_kicked += 1;
            warn!(
                "net server: connection {id} disconnected after repeated violations (last: {refusal:?}); \
                 limits are `ServerLimits` in net/limits.rs"
            );
            drop_connection(world, server, transport, id);
            false
        }
    }
}

/// Records a content violation of connection `id` (its datagram was accepted but is wrong).
fn refuse_content(
    world: &mut World,
    server: &mut NetServer,
    transport: &mut dyn ServerTransport,
    now: u64,
    id: ConnectionId,
    refusal: Refusal,
) {
    let limits = server.limits;
    let Some(state) = server.connections.get_mut(&id) else {
        return;
    };
    let verdict = state.guard.violation(&limits, now, refusal);
    apply_verdict(world, server, transport, id, verdict);
}

fn handle_packet(
    world: &mut World,
    server: &mut NetServer,
    transport: &mut dyn ServerTransport,
    next: u64,
    from: ConnectionId,
    bytes: &[u8],
) {
    let limits = server.limits;
    // A packet from a connection the server does not track (refused or already dropped) is ignored.
    let Some(state) = server.connections.get_mut(&from) else {
        return;
    };
    let verdict = state.guard.admit_datagram(&limits, next, bytes.len());
    if !apply_verdict(world, server, transport, from, verdict) {
        return;
    }
    match wire::decode_client_message(bytes) {
        Ok(ClientMessage::Hello { nonce, token }) => {
            if token
                .as_ref()
                .is_some_and(|text| text.len() > limits.max_token_bytes)
            {
                refuse_content(world, server, transport, next, from, Refusal::TokenTooLong);
                return;
            }
            if !server.known.contains(&from) {
                let request = NetIdentityRequest {
                    connection_id: from,
                    token: token.as_deref(),
                };
                let decision = world.resource::<NetIdentity>().0.check(request);
                match decision {
                    NetIdentityDecision::Allow { player_id } => {
                        server.known.insert(from);
                        server.delta.request_full_state();
                        world
                            .resource_mut::<NetPlayerIds>()
                            .0
                            .insert(from, player_id);
                        world.write_message(NetClientJoined { client_id: from });
                    }
                    NetIdentityDecision::Deny { reason } => {
                        server.stats.identity_denied += 1;
                        warn!(
                            "net server: connection {from} denied by the identity check: {reason}"
                        );
                        drop_connection(world, server, transport, from);
                        return;
                    }
                }
            }
            let welcome = ServerMessage::Welcome {
                nonce,
                client_id: from,
                tick: server.tick,
            };
            let welcome_bytes = wire::encode_server_message(&welcome);
            server.stats.bytes_sent += welcome_bytes.len() as u64;
            transport.send(from, welcome_bytes);
        }
        Ok(ClientMessage::Input { tick, lines }) => {
            // Input from a connection that never said hello is ignored.
            if !server.known.contains(&from) {
                return;
            }
            let Some(state) = server.connections.get_mut(&from) else {
                return;
            };
            let verdict = state.guard.admit_input(&limits, next);
            if !apply_verdict(world, server, transport, from, verdict) {
                return;
            }
            let refusal = check_input_content(&limits, next, tick, &lines);
            // Refused for its tick or not, the client needs to hear how far off it is.
            if matches!(
                refusal,
                None | Some(Refusal::InputTooOld) | Some(Refusal::InputTick)
            ) && let Some(state) = server.connections.get_mut(&from)
            {
                state.timing.record(next, tick);
            }
            if let Some(refusal) = refusal {
                if refusal == Refusal::InputTooOld {
                    server.stats.inputs_too_old += 1;
                    return;
                }
                refuse_content(world, server, transport, next, from, refusal);
                return;
            }
            // An input for a tick that is already running or past runs late: in the next ticks,
            // one per tick, so a burst does not collapse into one tick.
            if tick < next {
                server.late_inputs += 1;
            }
            let Some(state) = server.connections.get_mut(&from) else {
                return;
            };
            let target = schedule_input(next, tick, &mut state.spread_end);
            server
                .buffered
                .entry(target)
                .or_default()
                .push((from, tick, lines));
        }
        Err(_) => {
            server.undecodable_packets += 1;
            refuse_content(world, server, transport, next, from, Refusal::Undecodable);
        }
    }
}

fn drop_connections_that_never_joined(
    world: &mut World,
    server: &mut NetServer,
    transport: &mut dyn ServerTransport,
    now: u64,
) {
    let timeout = server.limits.join_timeout_ticks;
    let late: Vec<ConnectionId> = server
        .connections
        .iter()
        .filter(|(id, state)| {
            !server.known.contains(id) && now.saturating_sub(state.connected_at) >= timeout
        })
        .map(|(id, _)| *id)
        .collect();
    for id in late {
        server.stats.join_timeouts += 1;
        warn!(
            "net server: connection {id} dropped: no accepted hello within {timeout} ticks (`join_timeout_ticks`)"
        );
        drop_connection(world, server, transport, id);
    }
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
    if tick.is_multiple_of(TIMING_INTERVAL_TICKS) {
        send_timing_reports(world, &mut *transport.0);
    }
    // Every tick, for lag-compensated casts.
    let bodies = read_replicated_body_states(world);
    let poses = read_net_poses(world);
    world
        .resource_mut::<PoseHistory>()
        .record(tick, &bodies, &poses, MAX_REWIND_TICKS);
    if due {
        let state = ReplicatedState {
            bodies,
            states: sendable_module_states(world),
        };
        let (full, listed) = world.resource_mut::<NetServer>().delta.encode(&state);
        // At least one message even when there is nothing to list, so the client hears the tick.
        let pieces = split_into_chunks(listed);
        let chunks = pieces.len() as u16;
        for (index, piece) in pieces.into_iter().enumerate() {
            // Encoded per client: each hears how far its own inputs were run.
            for id in &known {
                let mut server = world.resource_mut::<NetServer>();
                let input_tick = server
                    .connections
                    .get(id)
                    .map_or(0, |state| state.ran_input);
                let bytes = wire::encode_server_message(&ServerMessage::State {
                    tick,
                    input_tick,
                    full,
                    chunk: index as u16,
                    chunks,
                    bodies: piece.bodies.clone(),
                    states: piece.states.clone(),
                });
                server.stats.bytes_sent += bytes.len() as u64;
                transport.0.send(*id, bytes);
            }
        }
        let mut server = world.resource_mut::<NetServer>();
        server.last_broadcast_states = state.states;
        server.last_broadcast = Some((tick, state.bodies));
    }
    world.insert_non_send(transport);
}

/// The module states to broadcast: all of them, except values too large for one datagram, which
/// are reported (an error and a failed script) and never sent.
fn sendable_module_states(world: &mut World) -> Vec<ModuleState> {
    let mut states = read_module_states(world);
    let mut too_large = Vec::new();
    states.retain(|state| {
        let size = encoded_size(state);
        if size > MAX_MODULE_STATE_BYTES {
            too_large.push(format!(
                "net state `{}` of `{}` takes {size} bytes, more than {MAX_MODULE_STATE_BYTES}, \
                 and is not sent; split it over several components or entities",
                state.id, state.entity
            ));
        }
        size <= MAX_MODULE_STATE_BYTES
    });
    world.resource_mut::<NetServer>().oversized_states += too_large.len() as u64;
    for message in too_large {
        error!("{message}");
        fail_running_script(world, message);
    }
    states
}

/// Tells each client that sent inputs how early they arrived.
fn send_timing_reports(world: &mut World, transport: &mut dyn ServerTransport) {
    let mut server = world.resource_mut::<NetServer>();
    let server = &mut *server;
    for (id, state) in &mut server.connections {
        let Some(report) = state.timing.take() else {
            continue;
        };
        if !server.known.contains(id) {
            continue;
        }
        let bytes = wire::encode_server_message(&ServerMessage::Timing {
            slack: report.slack,
            oldest_tick: report.oldest_tick,
        });
        server.stats.bytes_sent += bytes.len() as u64;
        transport.send(*id, bytes);
    }
}
