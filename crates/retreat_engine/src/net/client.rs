//! The predicting client, as a resource plus systems inside the simulation's own `World`.
//!
//! [`install_client`] adds a [`NetClient`] resource, a transport (non-send) and:
//! - `client_before_tick` (`FixedFirst`): reads the transport (the handshake and server states), and runs
//!   the command lines the player queued with [`NetClient::queue_input`] (during a replay: the lines
//!   recorded for that tick);
//! - `client_after_tick` (`FixedLast`): counts the tick, records the history (physics snapshot, body
//!   states, inputs) and sends the tick's input to the server (not during a replay);
//! - [`process_client_states`] (before the fixed loop of each frame): compares server states with the
//!   client's own history and, on a mismatch, rewinds, applies the server's state and replays the inputs.
//!   It runs outside `FixedMain` because a replay runs `FixedMain` itself.
//!
//! **Join.** The client sends `Hello { nonce }` every [`HELLO_RESEND_TICKS`] ticks until the server's
//! `Welcome` arrives, measures the round trip in ticks and starts simulating at
//! `welcome.tick + round trip + margin_ticks`: where the server will be when the client's inputs arrive,
//! plus a margin for jitter (negative margins make inputs late on purpose, in tests).

use std::collections::BTreeMap;

use bevy::app::{FixedFirst, FixedLast, RunFixedMainLoop, RunFixedMainLoopSystems};
use bevy::prelude::*;
use retreat_engine_api::{NetCommandSender, NetInputQueue, PhysicsShape, run_command_line};

use super::body_state::{BodyState, apply_body_states, read_replicated_body_states};
use super::transport::{ClientEvent, ClientTransport};
use super::wire;
use super::{ClientMessage, MAX_HISTORY_TICKS, ServerMessage, simulate_tick, state_lists_match};
use crate::physics::{PHYSICS_PROXY_NAME_SUFFIX, PhysicsBox, spawn_physics_box};
use crate::physics_snapshot::{PhysicsSnapshot, capture_physics, restore_physics};

/// A partly received state is kept for this many ticks while newer ones arrive.
const PARTIAL_STATE_TICKS: u64 = 64;

/// A client repeats its `Hello` this often while it waits for a `Welcome`.
pub const HELLO_RESEND_TICKS: u64 = 16;

/// How the client behaves.
#[derive(Clone, Copy, Debug)]
pub struct ClientConfig {
    /// Ticks added to the measured round trip when choosing how far ahead of the server to run.
    pub margin_ticks: i64,
    /// Identifies this `Hello` (the server echoes it).
    pub nonce: u32,
}

/// Where a client is in joining.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientPhase {
    /// Waiting for a `Welcome`.
    Connecting,
    /// Simulating and sending input.
    Running,
}

/// What a client did about server states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClientStats {
    /// Server states received.
    pub states_received: u64,
    /// States that matched the prediction.
    pub states_matching: u64,
    /// Rollbacks and replays done.
    pub corrections: u64,
    /// States dropped because the client had no history for their tick.
    pub states_without_history: u64,
    /// Corrections that could not restore (a body was despawned inside the window).
    pub refused_rollbacks: u64,
    /// Ticks replayed in all corrections.
    pub replayed_ticks: u64,
}

/// One tick of a client's history.
struct HistoryEntry {
    /// Physics state after the client simulated this tick.
    snapshot: PhysicsSnapshot,
    /// Body states after this tick.
    bodies: Vec<BodyState>,
    /// The command lines the client ran just before this tick.
    inputs: Vec<String>,
}

/// The client's networking state.
#[derive(Resource)]
pub struct NetClient {
    pub id: u32,
    pub phase: ClientPhase,
    /// The tick this client has simulated up to (it runs ahead of the server).
    pub tick: u64,
    history: BTreeMap<u64, HistoryEntry>,
    pub stats: ClientStats,
    /// Lines that failed on this client (a prediction that the rules refuse).
    pub rejected_lines: Vec<String>,
    nonce: u32,
    margin_ticks: i64,
    /// Ticks seen so far (the clock before the client has a tick of its own).
    steps: u64,
    hello_first_sent_at: Option<u64>,
    /// The round trip measured at the handshake, in ticks.
    pub measured_round_trip: Option<u64>,
    pub undecodable_packets: u64,
    /// Lines the player made, run at the start of the next tick.
    pending_inputs: Vec<String>,
    /// The lines run in the tick being simulated (recorded into the history).
    current_inputs: Vec<String>,
    /// True while a correction replays ticks.
    replaying: bool,
    replay_lines: Vec<String>,
    /// Server states received and not processed yet.
    pending_states: Vec<(u64, Vec<BodyState>)>,
    /// State chunks of ticks that are not complete yet: tick -> chunks received so far (by index).
    partial_states: BTreeMap<u64, Vec<Option<Vec<BodyState>>>>,
}

impl NetClient {
    /// Queues a command line the player makes; it runs at the start of the next tick and is sent to
    /// the server. Lines queued before the client has joined are dropped when it joins.
    pub fn queue_input(&mut self, line: impl Into<String>) {
        self.pending_inputs.push(line.into());
    }

    /// Body states this client had after `tick`, if still in its history.
    pub fn states_at(&self, tick: u64) -> Option<&[BodyState]> {
        self.history.get(&tick).map(|entry| entry.bodies.as_slice())
    }

    /// Collects one chunk of a state; when every chunk of the tick has arrived, the whole state
    /// (chunks in order, so still sorted by name) becomes ready for `process_client_states`.
    fn add_state_chunk(&mut self, tick: u64, chunk: u16, chunks: u16, bodies: Vec<BodyState>) {
        let (chunk, chunks) = (usize::from(chunk), usize::from(chunks));
        if chunks == 0 || chunk >= chunks {
            self.undecodable_packets += 1;
            return;
        }
        let slots = self
            .partial_states
            .entry(tick)
            .or_insert_with(|| vec![None; chunks]);
        if slots.len() != chunks {
            self.undecodable_packets += 1;
            return;
        }
        slots[chunk] = Some(bodies);
        if slots.iter().all(Option::is_some) {
            let complete: Vec<BodyState> = self
                .partial_states
                .remove(&tick)
                .into_iter()
                .flatten()
                .flatten()
                .flatten()
                .collect();
            self.pending_states.push((tick, complete));
        }
        // A tick whose chunks never all arrive is dropped once newer ticks are being assembled.
        let newest = self
            .partial_states
            .keys()
            .next_back()
            .copied()
            .unwrap_or(tick);
        self.partial_states
            .retain(|old, _| newest.saturating_sub(*old) <= PARTIAL_STATE_TICKS);
    }

    fn start_at(&mut self, tick: u64) {
        self.tick = tick;
        self.history.clear();
        self.pending_inputs.clear();
    }
}

struct ClientTransportBox(Box<dyn ClientTransport>);

/// Adds the client to `app`: the resource, the transport, the tick systems and the per-frame
/// correction.
pub fn install_client(
    app: &mut App,
    config: ClientConfig,
    transport: impl ClientTransport + 'static,
) {
    app.init_resource::<NetInputQueue>();
    app.world_mut().resource_mut::<NetInputQueue>().accepting = true;
    app.insert_resource(NetClient {
        id: 0,
        phase: ClientPhase::Connecting,
        tick: 0,
        history: BTreeMap::new(),
        stats: ClientStats::default(),
        rejected_lines: Vec::new(),
        nonce: config.nonce,
        margin_ticks: config.margin_ticks,
        steps: 0,
        hello_first_sent_at: None,
        measured_round_trip: None,
        undecodable_packets: 0,
        pending_inputs: Vec::new(),
        current_inputs: Vec::new(),
        replaying: false,
        replay_lines: Vec::new(),
        pending_states: Vec::new(),
        partial_states: BTreeMap::new(),
    })
    .insert_non_send(ClientTransportBox(Box::new(transport)))
    .add_systems(FixedFirst, client_before_tick)
    .add_systems(FixedLast, client_after_tick)
    .add_systems(
        RunFixedMainLoop,
        process_client_states.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
    );
}

/// Reads the transport and sends the handshake. Runs once per live tick (not during replays).
fn pump_transport(world: &mut World) {
    let Some(mut transport) = world.remove_non_send::<ClientTransportBox>() else {
        return;
    };
    let mut client = world.resource_mut::<NetClient>();
    for event in transport.0.poll() {
        let ClientEvent::Packet(bytes) = event else {
            continue;
        };
        match wire::decode_server_message(&bytes) {
            Ok(ServerMessage::Welcome {
                nonce,
                client_id,
                tick,
            }) => {
                if client.phase == ClientPhase::Connecting && nonce == client.nonce {
                    let sent_at = client.hello_first_sent_at.unwrap_or(client.steps);
                    let round_trip = client.steps.saturating_sub(sent_at);
                    client.measured_round_trip = Some(round_trip);
                    let start = (tick as i64 + round_trip as i64 + client.margin_ticks).max(0);
                    client.id = client_id;
                    client.start_at(start as u64);
                    client.phase = ClientPhase::Running;
                }
            }
            Ok(ServerMessage::State {
                tick,
                chunk,
                chunks,
                bodies,
            }) => client.add_state_chunk(tick, chunk, chunks, bodies),
            Err(_) => client.undecodable_packets += 1,
        }
    }
    if client.phase == ClientPhase::Connecting {
        let due = match client.hello_first_sent_at {
            None => true,
            Some(first) => (client.steps - first).is_multiple_of(HELLO_RESEND_TICKS),
        };
        if due {
            let step = client.steps;
            client.hello_first_sent_at.get_or_insert(step);
            let hello = ClientMessage::Hello {
                nonce: client.nonce,
            };
            transport.0.send(wire::encode_client_message(&hello));
        }
    }
    client.steps += 1;
    world.insert_non_send(transport);
}

fn client_before_tick(world: &mut World) {
    if !world.resource::<NetClient>().replaying {
        pump_transport(world);
    }
    let lines = {
        let mut client = world.resource_mut::<NetClient>();
        if client.phase != ClientPhase::Running {
            return;
        }
        if client.replaying {
            client.replay_lines.clone()
        } else {
            std::mem::take(&mut client.pending_inputs)
        }
    };
    // Lines that client modules queued through the contract join the player's input.
    let mut lines = lines;
    if !world.resource::<NetClient>().replaying {
        lines.extend(std::mem::take(
            &mut world.resource_mut::<NetInputQueue>().lines,
        ));
    }
    let own_id = world.resource::<NetClient>().id;
    for line in &lines {
        // The client predicts its own commands as if it were the sender.
        world.insert_resource(NetCommandSender(Some(own_id)));
        let result = run_command_line(world, line);
        world.insert_resource(NetCommandSender(None));
        if let Err(error) = result {
            world
                .resource_mut::<NetClient>()
                .rejected_lines
                .push(format!("`{line}`: {error}"));
        }
    }
    world.resource_mut::<NetClient>().current_inputs = lines;
}

fn client_after_tick(world: &mut World) {
    if world.resource::<NetClient>().phase != ClientPhase::Running {
        return;
    }
    let tick = {
        let mut client = world.resource_mut::<NetClient>();
        client.tick += 1;
        client.tick
    };
    let snapshot = capture_physics(world).expect("a client has physics");
    let bodies = read_replicated_body_states(world);
    let mut client = world.resource_mut::<NetClient>();
    let inputs = std::mem::take(&mut client.current_inputs);
    let replaying = client.replaying;
    client.history.insert(
        tick,
        HistoryEntry {
            snapshot,
            bodies,
            inputs: inputs.clone(),
        },
    );
    while client.history.len() as u64 > MAX_HISTORY_TICKS {
        client.history.pop_first();
    }
    if !replaying {
        let message = wire::encode_client_message(&ClientMessage::Input {
            tick,
            lines: inputs,
        });
        if let Some(mut transport) = world.remove_non_send::<ClientTransportBox>() {
            transport.0.send(message);
            world.insert_non_send(transport);
        }
    }
}

/// Handles the server states received so far: matches them against the history and corrects on a
/// mismatch. Must run outside `FixedMain` (the plugin schedules it before the fixed loop; the
/// in-process harness calls it before each tick).
pub fn process_client_states(world: &mut World) {
    let (mut ready, waiting): (Vec<_>, Vec<_>) = {
        let mut client = world.resource_mut::<NetClient>();
        let present = client.tick;
        std::mem::take(&mut client.pending_states)
            .into_iter()
            .partition(|(tick, _)| *tick <= present)
    };
    world.resource_mut::<NetClient>().pending_states = waiting;
    ready.sort_by_key(|(tick, _)| *tick);
    for (tick, bodies) in ready {
        receive_state(world, tick, bodies);
    }
}

fn receive_state(world: &mut World, server_tick: u64, server_bodies: Vec<BodyState>) {
    let mut client = world.resource_mut::<NetClient>();
    client.stats.states_received += 1;
    let Some(entry) = client.history.get(&server_tick) else {
        client.stats.states_without_history += 1;
        return;
    };
    if state_lists_match(&entry.bodies, &server_bodies) {
        client.stats.states_matching += 1;
        // Ticks before the confirmed one can never need a rollback again.
        client.history = client.history.split_off(&server_tick);
        return;
    }
    correct(world, server_tick, &server_bodies);
}

/// Rewinds to `server_tick`, applies the server's states and replays the inputs since.
fn correct(world: &mut World, server_tick: u64, server_bodies: &[BodyState]) {
    let replay_inputs: Vec<(u64, Vec<String>)> = {
        let client = world.resource::<NetClient>();
        let present = client.tick;
        // Nothing to replay when the server's tick is the client's present (a range whose start is
        // past its end would panic).
        if server_tick < present {
            client
                .history
                .range(server_tick + 1..=present)
                .map(|(tick, entry)| (*tick, entry.inputs.clone()))
                .collect()
        } else {
            Vec::new()
        }
    };
    // Restore from the history entry (taken out and put back, so the world can be borrowed mutably).
    let entry = world
        .resource_mut::<NetClient>()
        .history
        .remove(&server_tick)
        .expect("checked above");
    let restore = restore_physics(world, &entry.snapshot);
    world
        .resource_mut::<NetClient>()
        .history
        .insert(server_tick, entry);
    if restore.is_err() {
        world.resource_mut::<NetClient>().stats.refused_rollbacks += 1;
        return;
    }
    world.resource_mut::<NetClient>().stats.corrections += 1;
    // Bodies the server has and this client lacks are created at the server's pose; bodies the
    // server lacks are removed.
    let report = apply_body_states(world, server_bodies);
    for name in &report.unknown_to_world {
        if let Some(state) = server_bodies.iter().find(|state| &state.name == name) {
            create_body_from_state(world, state);
        }
    }
    for name in &report.missing_from_states {
        despawn_body_named(world, name.trim_end_matches(PHYSICS_PROXY_NAME_SUFFIX));
    }
    // Replay: the history of every tick after `server_tick` is rebuilt.
    {
        let mut client = world.resource_mut::<NetClient>();
        client.history.retain(|tick, _| *tick <= server_tick);
        client.tick = server_tick;
        client.replaying = true;
    }
    for (_, lines) in replay_inputs {
        world.resource_mut::<NetClient>().replay_lines = lines;
        simulate_tick(world);
        world.resource_mut::<NetClient>().stats.replayed_ticks += 1;
    }
    let mut client = world.resource_mut::<NetClient>();
    client.replaying = false;
    client.replay_lines.clear();
    // The corrected state at `server_tick` is the server's.
    if let Some(entry) = client.history.get_mut(&server_tick) {
        entry.bodies = server_bodies.to_vec();
    }
}

/// Creates a body the client did not know, at the server's pose (cuboids only; the engine's
/// spawnable body today).
fn create_body_from_state(world: &mut World, state: &BodyState) {
    if let PhysicsShape::Cuboid { half_extents } = state.description.shape {
        let entity = spawn_physics_box(world, &state.name, state.translation, half_extents.x * 2.0);
        if let Some(mut transform) = world.get_mut::<Transform>(entity) {
            transform.rotation = state.rotation;
        }
    }
}

/// Despawns the physics box with this name, if there is one.
fn despawn_body_named(world: &mut World, name: &str) {
    let mut query = world.query_filtered::<(Entity, &Name), With<PhysicsBox>>();
    let found: Vec<Entity> = query
        .iter(world)
        .filter(|(_, entity_name)| entity_name.as_str() == name)
        .map(|(entity, _)| entity)
        .collect();
    for entity in found {
        world.despawn(entity);
    }
}
