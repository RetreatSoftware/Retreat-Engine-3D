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
//! **Join.** The client sends `Hello { nonce, token }` every [`HELLO_RESEND_TICKS`] ticks until the server's
//! `Welcome` arrives, measures the round trip in ticks and starts simulating at
//! `welcome.tick + round trip + margin_ticks`: where the server will be when the client's inputs arrive,
//! plus a margin for jitter (negative margins make inputs late on purpose, in tests).

use std::collections::BTreeMap;
use std::time::Duration;

use bevy::app::{FixedFirst, FixedLast, RunFixedMainLoop, RunFixedMainLoopSystems};
use bevy::prelude::*;
use retreat_engine_api::{
    NetCommandSender, NetInputQueue, NetLocalClient, NetReplaying, NetStateRegistry, NetViewTick,
    PhysicsBody, Replicated, RetiredEntity, run_command_line,
};

use super::body_state::{BodyState, apply_body_states, read_replicated_body_states};
use super::lag_compensation::{CLIENT_HISTORY_TICKS, PoseHistory, ViewClock, advance_view};
use super::module_state::{
    ModuleState, apply_module_states, module_state_lists_match, read_module_states,
};
use super::state_delta::{ReplicatedState, StateAssembler};
use super::tick_pacing::{ClientPacing, TimingReport};
use super::transport::{ClientEvent, ClientTransport};
use super::wire;
use super::{ClientMessage, MAX_HISTORY_TICKS, ServerMessage, simulate_tick, state_lists_match};
use crate::physics::{spawn_body_from_description, sync_body_readouts};
use crate::physics_snapshot::{PhysicsSnapshot, capture_physics, restore_physics};
use crate::render_smoothing::{absorb_correction, poses_before_correction};
use crate::retired_bodies::revive_retired_entities;

/// A partly received state is kept for this many ticks while newer ones arrive.
const PARTIAL_STATE_TICKS: u64 = 64;

/// The longest frame a client simulates in full.
pub const CLIENT_MAX_FRAME_TIME: Duration = Duration::from_secs(1);

/// A client repeats its `Hello` this often while it waits for a `Welcome`.
pub const HELLO_RESEND_TICKS: u64 = 16;

/// How the client behaves.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// Ticks added to the measured round trip when choosing how far ahead of the server to run.
    pub margin_ticks: i64,
    /// Identifies this `Hello` (the server echoes it).
    pub nonce: u32,
    /// Sent in the `Hello` for the server's identity check (`None` when the platform gave none).
    pub token: Option<String>,
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
    /// Bytes of every datagram received from the server.
    pub bytes_received: u64,
    /// Bytes of every datagram sent to the server.
    pub bytes_sent: u64,
    /// Extra ticks run because inputs were arriving late.
    pub catch_up_ticks: u64,
    /// Times the tick number jumped because the client was hopelessly behind.
    pub resyncs: u64,
    /// Module state values from the server that this client cannot use (type not registered,
    /// another version or damaged): the server and client builds differ.
    pub unusable_module_states: u64,
}

/// One tick of a client's history.
struct HistoryEntry {
    /// Physics state after the client simulated this tick.
    snapshot: PhysicsSnapshot,
    /// Replicated bodies and module states after this tick.
    state: ReplicatedState,
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
    token: Option<String>,
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
    pending_states: Vec<(u64, ReplicatedState)>,
    /// Whole body states received since the last tick, for the pose history.
    received: Vec<(u64, Vec<BodyState>)>,
    /// State chunks of ticks that are not complete yet: tick -> chunks received so far (by index).
    partial_states: BTreeMap<u64, Vec<Option<ReplicatedState>>>,
    /// Rebuilds whole states from the deltas the server sends.
    assembler: StateAssembler,
    /// Complete states discarded because they were not usable (a delta before any full state, or
    /// not newer than the last one used).
    pub states_discarded: u64,
    /// Whether each partial tick is a full state (the flag is the same on every chunk of a tick).
    partial_full: BTreeMap<u64, bool>,
    /// The transport reported `Connected`. Hellos (and the round-trip clock) start only then.
    connected: bool,
    /// Keeps the lead over the server right (see `tick_pacing.rs`).
    pub pacing: ClientPacing,
}

impl NetClient {
    /// Queues a command line the player makes; it runs at the start of the next tick and is sent to
    /// the server. Lines queued before the client has joined are dropped when it joins.
    pub fn queue_input(&mut self, line: impl Into<String>) {
        self.pending_inputs.push(line.into());
    }

    /// Body states this client had after `tick`, if still in its history.
    pub fn states_at(&self, tick: u64) -> Option<&[BodyState]> {
        self.history
            .get(&tick)
            .map(|entry| entry.state.bodies.as_slice())
    }

    /// Module states this client had after `tick`, if still in its history.
    pub fn module_states_at(&self, tick: u64) -> Option<&[ModuleState]> {
        self.history
            .get(&tick)
            .map(|entry| entry.state.states.as_slice())
    }

    /// Collects one chunk of a state; when every chunk of the tick has arrived, the whole state
    /// (chunks in order, so still sorted by name) becomes ready for `process_client_states`.
    fn add_state_chunk(
        &mut self,
        tick: u64,
        full: bool,
        chunk: u16,
        chunks: u16,
        piece: ReplicatedState,
    ) {
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
        slots[chunk] = Some(piece);
        if slots.iter().all(Option::is_some) {
            let mut complete = ReplicatedState::default();
            for piece in self
                .partial_states
                .remove(&tick)
                .into_iter()
                .flatten()
                .flatten()
            {
                complete.bodies.extend(piece.bodies);
                complete.states.extend(piece.states);
            }
            let full = self.partial_full.remove(&tick).unwrap_or(full);
            match self.assembler.assemble(tick, full, complete) {
                Some(whole) => {
                    self.received.push((tick, whole.bodies.clone()));
                    self.pending_states.push((tick, whole));
                }
                None => self.states_discarded += 1,
            }
        } else {
            self.partial_full.insert(tick, full);
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
        self.partial_full
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
    app.init_resource::<NetStateRegistry>();
    app.init_resource::<NetLocalClient>();
    app.init_resource::<NetReplaying>();
    app.init_resource::<NetViewTick>();
    app.init_resource::<PoseHistory>();
    app.init_resource::<ViewClock>();
    app.world_mut().resource_mut::<NetInputQueue>().accepting = true;
    // A slow frame must not lose simulated time (Bevy caps a frame at 250 ms by default): a
    // client that ticks slower than the server falls behind it.
    if let Some(mut clock) = app.world_mut().get_resource_mut::<Time<Virtual>>() {
        clock.set_max_delta(CLIENT_MAX_FRAME_TIME);
    }
    app.insert_resource(NetClient {
        id: 0,
        phase: ClientPhase::Connecting,
        tick: 0,
        history: BTreeMap::new(),
        stats: ClientStats::default(),
        rejected_lines: Vec::new(),
        nonce: config.nonce,
        token: config.token,
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
        received: Vec::new(),
        partial_states: BTreeMap::new(),
        assembler: StateAssembler::default(),
        states_discarded: 0,
        partial_full: BTreeMap::new(),
        connected: false,
        pacing: ClientPacing::default(),
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
    let mut joined_as = None;
    let mut client = world.resource_mut::<NetClient>();
    for event in transport.0.poll() {
        let bytes = match event {
            ClientEvent::Packet(bytes) => bytes,
            ClientEvent::Connected => {
                client.connected = true;
                continue;
            }
            ClientEvent::Disconnected(_) => continue,
        };
        client.stats.bytes_received += bytes.len() as u64;
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
                    joined_as = Some(client_id);
                }
            }
            Ok(ServerMessage::State {
                tick,
                full,
                chunk,
                chunks,
                bodies,
                states,
            }) => client.add_state_chunk(
                tick,
                full,
                chunk,
                chunks,
                ReplicatedState { bodies, states },
            ),
            Ok(ServerMessage::Timing { slack, oldest_tick }) => {
                if client.phase == ClientPhase::Running {
                    client.pacing.on_report(TimingReport { slack, oldest_tick });
                }
            }
            Err(_) => client.undecodable_packets += 1,
        }
    }
    // The round trip is measured from the first Hello, so Hellos wait for the connection: a browser
    // needs seconds to open a WebTransport session, and counting them as round trip made the client
    // start hundreds of ticks ahead of the server, far outside the window the server accepts.
    if client.phase == ClientPhase::Connecting && client.connected {
        let due = match client.hello_first_sent_at {
            None => true,
            Some(first) => (client.steps - first).is_multiple_of(HELLO_RESEND_TICKS),
        };
        if due {
            let step = client.steps;
            client.hello_first_sent_at.get_or_insert(step);
            let hello = ClientMessage::Hello {
                nonce: client.nonce,
                token: client.token.clone(),
            };
            let hello_bytes = wire::encode_client_message(&hello);
            client.stats.bytes_sent += hello_bytes.len() as u64;
            transport.0.send(hello_bytes);
        }
    }
    client.steps += 1;
    let received = std::mem::take(&mut client.received);
    let mut history = world.resource_mut::<PoseHistory>();
    for (tick, bodies) in received {
        history.record(tick, &bodies, CLIENT_HISTORY_TICKS);
    }
    if let Some(client_id) = joined_as {
        world.insert_resource(NetLocalClient(Some(client_id)));
    }
    world.insert_non_send(transport);
}

fn client_before_tick(world: &mut World) {
    if !world.resource::<NetClient>().replaying {
        pump_transport(world);
    }
    let lines = {
        let mut client = world.resource_mut::<NetClient>();
        if client.phase != ClientPhase::Running {
            // Input made before joining is dropped (see `queue_input`), including what modules
            // queued: it would otherwise pile up and go out as one oversized `Input` on joining.
            world.resource_mut::<NetInputQueue>().lines.clear();
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
    if !world.resource::<NetClient>().replaying {
        advance_view(world);
    }
    let tick = {
        let mut client = world.resource_mut::<NetClient>();
        client.tick += 1;
        client.tick
    };
    let snapshot = capture_physics(world).expect("a client has physics");
    let state = read_replicated_state(world);
    let mut client = world.resource_mut::<NetClient>();
    let inputs = std::mem::take(&mut client.current_inputs);
    let replaying = client.replaying;
    client.history.insert(
        tick,
        HistoryEntry {
            snapshot,
            state,
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
            world.resource_mut::<NetClient>().stats.bytes_sent += message.len() as u64;
            transport.0.send(message);
            world.insert_non_send(transport);
        }
    }
}

/// What this machine has of the replicated state now.
fn read_replicated_state(world: &mut World) -> ReplicatedState {
    ReplicatedState {
        bodies: read_replicated_body_states(world),
        states: read_module_states(world),
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
    for (tick, state) in ready {
        receive_state(world, tick, state);
    }
    keep_pace(world);
}

/// Does what the server's timing reports asked for: runs the missing ticks (live ones: they send
/// inputs), jumps a hopeless gap, or slows the clock.
fn keep_pace(world: &mut World) {
    let pacing = world.resource::<NetClient>().pacing;
    if let Some(mut clock) = world.get_resource_mut::<Time<Virtual>>()
        && clock.relative_speed() != pacing.time_scale
    {
        clock.set_relative_speed(pacing.time_scale);
    }
    if pacing.jump == 0 && pacing.catch_up == 0 {
        return;
    }
    if pacing.jump > 0 {
        let mut client = world.resource_mut::<NetClient>();
        let tick = client.tick + pacing.jump;
        client.start_at(tick);
        client.stats.resyncs += 1;
    }
    for _ in 0..pacing.catch_up {
        simulate_tick(world);
        world.resource_mut::<NetClient>().stats.catch_up_ticks += 1;
    }
    let mut client = world.resource_mut::<NetClient>();
    let tick = client.tick;
    client.pacing.adjusted(tick);
}

fn receive_state(world: &mut World, server_tick: u64, mut server: ReplicatedState) {
    let registry = world.resource::<NetStateRegistry>().clone();
    // Values this build cannot read are left out, or they would force a correction every time.
    let received = server.states.len();
    server.states.retain(|state| state.is_usable(&registry));
    let unusable = (received - server.states.len()) as u64;
    if unusable > 0 {
        warn_once!(
            "net client: the server sends module state this build cannot use (type not registered \
             with `register_net_state`, another `NetState::VERSION` or damaged data); update the \
             older side"
        );
    }
    let mut client = world.resource_mut::<NetClient>();
    client.stats.states_received += 1;
    client.stats.unusable_module_states += unusable;
    let Some(entry) = client.history.get(&server_tick) else {
        client.stats.states_without_history += 1;
        return;
    };
    if state_lists_match(&entry.state.bodies, &server.bodies)
        && module_state_lists_match(&registry, &entry.state.states, &server.states)
    {
        client.stats.states_matching += 1;
        // Ticks before the confirmed one can never need a rollback again.
        client.history = client.history.split_off(&server_tick);
        return;
    }
    correct(world, server_tick, &server);
}

/// Rewinds to `server_tick`, applies the server's states and replays the inputs since.
fn correct(world: &mut World, server_tick: u64, server: &ReplicatedState) {
    let server_bodies = &server.bodies;
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
    let drawn_before = poses_before_correction(world);
    // An entity the server still lists is alive, even if this client had removed it.
    let listed = server.states.iter().map(|state| state.entity.as_str());
    revive_retired_entities(world, &listed.collect());
    // Bodies the server has and this client lacks are created at the server's pose; bodies the
    // server lacks are removed.
    let report = apply_body_states(world, server_bodies);
    for name in &report.unknown_to_world {
        if let Some(state) = server_bodies.iter().find(|state| &state.name == name) {
            create_body_from_state(world, state);
        }
    }
    for name in &report.missing_from_states {
        despawn_body_named(world, name);
    }
    // After the bodies, so a body created above gets its module state.
    apply_module_states(world, &server.states);
    // The replay's first tick must read the corrected poses and velocities.
    sync_body_readouts(world);
    // Replay: the history of every tick after `server_tick` is rebuilt.
    {
        let mut client = world.resource_mut::<NetClient>();
        client.history.retain(|tick, _| *tick <= server_tick);
        client.tick = server_tick;
        client.replaying = true;
    }
    world.insert_resource(NetReplaying(true));
    // Module systems keep queueing lines in `NetInputQueue` while ticks replay, but a replay runs
    // the recorded inputs instead. Set aside the lines queued for the next real tick and put them
    // back afterwards; without this the replay's lines pile up and go out as one oversized `Input`
    // that the server refuses (found by `net.stats` on the players scenario).
    let queued_for_next_tick = std::mem::take(&mut world.resource_mut::<NetInputQueue>().lines);
    for (_, lines) in replay_inputs {
        world.resource_mut::<NetClient>().replay_lines = lines;
        simulate_tick(world);
        world.resource_mut::<NetClient>().stats.replayed_ticks += 1;
        world.resource_mut::<NetInputQueue>().lines.clear();
    }
    world.resource_mut::<NetInputQueue>().lines = queued_for_next_tick;
    world.insert_resource(NetReplaying(false));
    absorb_correction(world, &drawn_before);
    let mut client = world.resource_mut::<NetClient>();
    client.replaying = false;
    client.replay_lines.clear();
    // The corrected state at `server_tick` is the server's.
    if let Some(entry) = client.history.get_mut(&server_tick) {
        entry.state = server.clone();
    }
}

/// Creates a body the client did not know, at the server's pose (any shape): the description and the
/// list of replicated kinds come from the state, so it behaves like the server's body.
fn create_body_from_state(world: &mut World, state: &BodyState) {
    spawn_body_from_description(
        world,
        &state.name,
        &state.description,
        Replicated::of(&state.synced),
        Transform {
            translation: state.translation,
            rotation: state.rotation,
            scale: Vec3::ONE,
        },
    );
}

/// Despawns the body with this name that the server no longer has (not a retired one: it is not
/// part of the shared state).
fn despawn_body_named(world: &mut World, name: &str) {
    let mut query =
        world.query_filtered::<(Entity, &Name), (With<PhysicsBody>, Without<RetiredEntity>)>();
    let found: Vec<Entity> = query
        .iter(world)
        .filter(|(_, entity_name)| entity_name.as_str() == name)
        .map(|(entity, _)| entity)
        .collect();
    for entity in found {
        world.despawn(entity);
    }
}
