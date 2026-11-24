//! Networking core: server-authoritative simulation with client-side prediction, plus an
//! in-process harness that runs a server and N clients over simulated links (FRAMEWORK.md §12).
//!
//! **Model.** Server and clients run the same fixed-tick simulation. A *tick number* labels the n-th
//! simulated tick on every machine. A client runs `lead_ticks` ahead of the server, so an input it
//! makes for tick `n` reaches the server before the server simulates tick `n` (if the link allows).
//! The lead must be at least the one-way latency plus jitter plus one tick (a message sent at the end of
//! tick `n` arrives during tick `n + 1`); with less, the server runs inputs late and clients correct.
//! Inputs are command lines (everything is a command), applied just before the tick's update; the
//! server only runs commands it has been told to allow.
//!
//! **Prediction.** A client applies its own inputs at once and keeps a history (physics snapshot,
//! body states and inputs) per tick. When the server's state for tick `s` arrives, the client compares
//! it with its own state at `s`. On a mismatch it restores its snapshot of `s`, writes the server's
//! body states over it, creates bodies it did not know, and replays its inputs for ticks `s+1..` up to
//! its present. The server is always right; clients only guess ahead.
//!
//! **Limits.** Bodies created from server state start with zero velocity until the next correction;
//! a body the client despawned inside its own history window refuses the rollback (see
//! `physics_snapshot.rs`) and the client keeps its state until the next server state arrives.
//! Messages cross the links as bytes (`wire.rs`: a version byte plus postcard); the real transport
//! (WebTransport) comes later.

pub mod body_state;
pub mod link;
pub mod runner;
pub mod transport;
pub mod wire;

use std::collections::{BTreeMap, BTreeSet};

use bevy::app::FixedMain;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use retreat_engine_api::run_command_line;

use crate::physics::{PhysicsBox, spawn_physics_box};
use crate::physics_snapshot::{PhysicsSnapshot, capture_physics, restore_physics};
use body_state::{BodyState, apply_body_states, read_body_states};
use link::LinkConditions;
use retreat_engine_api::PhysicsShape;
use serde::{Deserialize, Serialize};

/// Largest distance (world units) between predicted and server positions that still counts as
/// "the same".
pub const POSITION_TOLERANCE: f32 = 1.0e-3;
/// Largest velocity difference that still counts as "the same".
pub const VELOCITY_TOLERANCE: f32 = 1.0e-2;
/// A client keeps this many ticks of history at most.
pub const MAX_HISTORY_TICKS: u64 = 512;

/// What a client sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Asks to join; the server answers with `Welcome` carrying the same `nonce`.
    Hello { nonce: u32 },
    /// Command lines to run at server tick `tick`. (Who sent it is the connection, never a field the
    /// client could forge.)
    Input { tick: u64, lines: Vec<String> },
}

/// One client's input as the server node takes it in (the runner fills `client_id` from the
/// connection).
#[derive(Clone, Debug, PartialEq)]
pub struct ClientInput {
    pub client_id: u32,
    pub tick: u64,
    pub lines: Vec<String>,
}

/// What the server sends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// The answer to `Hello`: the client's id and the server's last simulated tick.
    Welcome {
        nonce: u32,
        client_id: u32,
        tick: u64,
    },
    /// The state of every physics body after the server simulated `tick`.
    State { tick: u64, bodies: Vec<BodyState> },
}

/// Makes `app` step exactly one fixed tick per `update`, and runs the first update (which starts
/// Bevy's clock and simulates no tick). Call it after adding every plugin.
pub fn prepare_simulation_app(app: &mut App) {
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app.finish();
    app.cleanup();
    app.update();
}

/// Simulates exactly one fixed tick of `world` without running a frame: advances the fixed clock by one
/// timestep and runs the `FixedMain` schedule (the same schedules a frame runs for a tick). This is how
/// the net code steps a simulation, so the server, the clients and a client's replays all use one code
/// path, and so a correction can replay ticks from inside a system (a full `App::update` cannot nest).
///
/// Do not call it from inside `FixedMain` itself (the schedule is already running).
pub fn simulate_tick(world: &mut World) {
    let timestep = world.resource::<Time<Fixed>>().timestep();
    world.resource_mut::<Time<Fixed>>().advance_by(timestep);
    let fixed_time = world.resource::<Time<Fixed>>().as_generic();
    *world.resource_mut::<Time>() = fixed_time;
    world.run_schedule(FixedMain);
    let virtual_time = world.resource::<Time<Virtual>>().as_generic();
    *world.resource_mut::<Time>() = virtual_time;
}

/// True when two body states are the same within the tolerances (name and description must match;
/// sleeping is ignored: it follows from the motion).
pub fn states_match(left: &BodyState, right: &BodyState) -> bool {
    left.name == right.name
        && left.description == right.description
        && left.translation.distance(right.translation) <= POSITION_TOLERANCE
        && left.rotation.angle_between(right.rotation) <= POSITION_TOLERANCE
        && left.linear_velocity.distance(right.linear_velocity) <= VELOCITY_TOLERANCE
        && left.angular_velocity.distance(right.angular_velocity) <= VELOCITY_TOLERANCE
}

/// True when two lists (sorted by name) describe the same bodies within the tolerances.
pub fn state_lists_match(left: &[BodyState], right: &[BodyState]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| states_match(left, right))
}

/// The authoritative simulation.
pub struct ServerNode {
    pub app: App,
    /// The last tick simulated (0 before the first).
    pub tick: u64,
    /// Command ids clients may run (empty: none).
    allowed_commands: BTreeSet<String>,
    /// Inputs waiting for their tick: tick -> (client id, lines), in arrival order.
    buffered: BTreeMap<u64, Vec<(u32, Vec<String>)>>,
    /// Ticks between state broadcasts (1 = every tick).
    state_interval: u64,
    /// Inputs that arrived after their tick and ran late.
    pub late_inputs: u64,
    /// Lines refused (command not allowed, or the command failed).
    pub rejected_lines: Vec<String>,
    /// The state broadcast most recently.
    pub last_broadcast: Option<(u64, Vec<BodyState>)>,
}

impl ServerNode {
    pub fn new(app: App, allowed_commands: &[&str], state_interval: u64) -> Self {
        Self {
            app,
            tick: 0,
            allowed_commands: allowed_commands.iter().map(|id| id.to_string()).collect(),
            buffered: BTreeMap::new(),
            state_interval: state_interval.max(1),
            late_inputs: 0,
            rejected_lines: Vec::new(),
            last_broadcast: None,
        }
    }

    /// Takes in messages, simulates the next tick and returns the state to broadcast (if due).
    pub fn step(&mut self, incoming: Vec<ClientInput>) -> Option<ServerMessage> {
        let next = self.tick + 1;
        for ClientInput {
            client_id,
            tick,
            lines,
        } in incoming
        {
            // An input for a tick that is already running or past runs now, late.
            let target = if tick < next {
                self.late_inputs += 1;
                next
            } else {
                tick
            };
            self.buffered
                .entry(target)
                .or_default()
                .push((client_id, lines));
        }
        let mut due = self.buffered.remove(&next).unwrap_or_default();
        due.sort_by_key(|(client_id, _)| *client_id);
        for (_, lines) in due {
            for line in lines {
                self.run_allowed_line(&line);
            }
        }
        simulate_tick(self.app.world_mut());
        self.tick = next;
        if self.tick.is_multiple_of(self.state_interval) {
            let bodies = read_body_states(self.app.world_mut());
            self.last_broadcast = Some((self.tick, bodies.clone()));
            return Some(ServerMessage::State {
                tick: self.tick,
                bodies,
            });
        }
        None
    }

    fn run_allowed_line(&mut self, line: &str) {
        let command_id = line.split_whitespace().next().unwrap_or("");
        if !self.allowed_commands.contains(command_id) {
            self.rejected_lines.push(format!(
                "`{line}`: command `{command_id}` is not allowed from clients"
            ));
            return;
        }
        if let Err(error) = run_command_line(self.app.world_mut(), line) {
            self.rejected_lines.push(format!("`{line}`: {error}"));
        }
    }
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

/// What a client did about server states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClientStats {
    /// Server states received.
    pub states_received: u64,
    /// States that matched the prediction.
    pub states_matching: u64,
    /// Rollbacks and replays done.
    pub corrections: u64,
    /// Corrections skipped because the client had no history for the state's tick.
    pub states_without_history: u64,
    /// Corrections that could not restore (a body was despawned inside the window).
    pub refused_rollbacks: u64,
    /// Ticks replayed in all corrections.
    pub replayed_ticks: u64,
}

/// A predicting client.
pub struct ClientNode {
    pub id: u32,
    pub app: App,
    /// The tick this client has simulated up to (it runs `lead_ticks` ahead of the server).
    pub tick: u64,
    history: BTreeMap<u64, HistoryEntry>,
    pub stats: ClientStats,
    /// Lines that failed on this client (a prediction that the rules refuse).
    pub rejected_lines: Vec<String>,
}

impl ClientNode {
    pub fn new(id: u32, app: App, start_tick: u64) -> Self {
        Self {
            id,
            app,
            tick: start_tick,
            history: BTreeMap::new(),
            stats: ClientStats::default(),
            rejected_lines: Vec::new(),
        }
    }

    /// Starts (or restarts) the client's clock at `tick`: it has simulated nothing yet, so its history is
    /// empty and the next tick it simulates is `tick + 1`.
    pub fn start_at(&mut self, tick: u64) {
        self.tick = tick;
        self.history.clear();
    }

    /// Body states this client had after `tick`, if still in its history.
    pub fn states_at(&self, tick: u64) -> Option<&[BodyState]> {
        self.history.get(&tick).map(|entry| entry.bodies.as_slice())
    }

    /// Predicts the next tick with `inputs`, then handles the server messages that arrived (a state
    /// for the tick just simulated needs that tick in the history). Returns the input message to
    /// send (always, even when empty, so the server sees the client is alive).
    pub fn step(&mut self, inputs: Vec<String>, incoming: Vec<ServerMessage>) -> ClientMessage {
        let next = self.tick + 1;
        self.simulate_tick(next, inputs.clone());
        for message in incoming {
            // Handshake messages are the runner's business; only states reach the node.
            if let ServerMessage::State { tick, bodies } = message {
                self.receive_state(tick, bodies);
            }
        }
        ClientMessage::Input {
            tick: next,
            lines: inputs,
        }
    }

    /// Runs `lines`, one update, and records the history for `tick`.
    fn simulate_tick(&mut self, tick: u64, lines: Vec<String>) {
        for line in &lines {
            if let Err(error) = run_command_line(self.app.world_mut(), line) {
                self.rejected_lines.push(format!("`{line}`: {error}"));
            }
        }
        simulate_tick(self.app.world_mut());
        self.tick = tick;
        let snapshot = capture_physics(self.app.world_mut()).expect("a client has physics");
        let bodies = read_body_states(self.app.world_mut());
        self.history.insert(
            tick,
            HistoryEntry {
                snapshot,
                bodies,
                inputs: lines,
            },
        );
        while self.history.len() as u64 > MAX_HISTORY_TICKS {
            self.history.pop_first();
        }
    }

    fn receive_state(&mut self, server_tick: u64, server_bodies: Vec<BodyState>) {
        self.stats.states_received += 1;
        let Some(entry) = self.history.get(&server_tick) else {
            self.stats.states_without_history += 1;
            return;
        };
        if state_lists_match(&entry.bodies, &server_bodies) {
            self.stats.states_matching += 1;
            // Ticks up to the confirmed one can never need a rollback again.
            self.history = self.history.split_off(&server_tick);
            return;
        }
        self.correct(server_tick, &server_bodies);
    }

    /// Rewinds to `server_tick`, applies the server's states and replays the inputs since.
    fn correct(&mut self, server_tick: u64, server_bodies: &[BodyState]) {
        let present = self.tick;
        // Nothing to replay when the server's tick is the client's present (a range whose start is
        // past its end would panic).
        let replay_inputs: Vec<(u64, Vec<String>)> = if server_tick < present {
            self.history
                .range(server_tick + 1..=present)
                .map(|(tick, entry)| (*tick, entry.inputs.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let snapshot = &self.history[&server_tick].snapshot;
        if restore_physics(self.app.world_mut(), snapshot).is_err() {
            self.stats.refused_rollbacks += 1;
            return;
        }
        self.stats.corrections += 1;
        // Bodies the server has and this client lacks are created at the server's pose; bodies the
        // server lacks are removed.
        let report = apply_body_states(self.app.world_mut(), server_bodies);
        for name in &report.unknown_to_world {
            if let Some(state) = server_bodies.iter().find(|state| &state.name == name) {
                create_body_from_state(self.app.world_mut(), state);
            }
        }
        for name in &report.missing_from_states {
            let name = name.trim_end_matches(crate::physics::PHYSICS_PROXY_NAME_SUFFIX);
            despawn_body_named(self.app.world_mut(), name);
        }
        // Replay: the history of every tick after `server_tick` is rebuilt.
        self.history.retain(|tick, _| *tick <= server_tick);
        self.tick = server_tick;
        for (tick, lines) in replay_inputs {
            self.simulate_tick(tick, lines);
            self.stats.replayed_ticks += 1;
        }
        // The corrected state at `server_tick` is the server's.
        if let Some(entry) = self.history.get_mut(&server_tick) {
            entry.bodies = server_bodies.to_vec();
        }
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

/// How the harness is set up.
#[derive(Clone, Copy, Debug)]
pub struct HarnessConfig {
    /// Safety margin, in ticks, that clients add to the measured round trip when choosing how far
    /// ahead of the server to run. It must cover jitter plus one tick; a negative value makes inputs
    /// arrive late on purpose.
    pub margin_ticks: i64,
    /// Ticks between server state broadcasts.
    pub state_interval_ticks: u64,
    /// Link conditions client to server and server to client (each client gets its own links).
    pub up: LinkConditions,
    pub down: LinkConditions,
    /// Seeds the links' random loss and jitter.
    pub seed: u64,
}

/// A server and clients in one process, connected by a [`LoopbackNetwork`](transport::LoopbackNetwork).
pub struct NetHarness {
    pub server: runner::ServerRunner<transport::LoopbackServer>,
    pub clients: Vec<runner::ClientRunner<transport::LoopbackClient>>,
    network: transport::LoopbackNetwork,
    /// Ticks stepped so far.
    pub global_tick: u64,
}

impl NetHarness {
    /// `make_app` builds one simulation app (physics, the scene, the commands); it is called once for
    /// the server and once per client. Every app must come out of [`prepare_simulation_app`].
    pub fn new(
        config: HarnessConfig,
        allowed_commands: &[&str],
        client_count: u32,
        make_app: impl Fn() -> App,
    ) -> Self {
        let network = transport::LoopbackNetwork::new(config.up, config.down, config.seed);
        let node = ServerNode::new(make_app(), allowed_commands, config.state_interval_ticks);
        let server = runner::ServerRunner::new(node, network.server());
        let clients = (0..client_count)
            .map(|id| {
                let node = ClientNode::new(id, make_app(), 0);
                runner::ClientRunner::new(node, network.connect(), 1000 + id, config.margin_ticks)
            })
            .collect();
        Self {
            server,
            clients,
            network,
            global_tick: 0,
        }
    }

    /// Advances everything by one tick. `inputs` lists `(client index, command lines)` that players
    /// make this tick.
    pub fn step(&mut self, inputs: &[(usize, Vec<String>)]) {
        self.global_tick += 1;
        self.network.set_tick(self.global_tick);
        self.server.step();
        for (index, client) in self.clients.iter_mut().enumerate() {
            let lines: Vec<String> = inputs
                .iter()
                .filter(|(who, _)| *who == index)
                .flat_map(|(_, lines)| lines.clone())
                .collect();
            client.step(lines);
        }
    }

    /// Steps `ticks` times with no player input.
    pub fn run_idle(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.step(&[]);
        }
    }

    /// Changes the link conditions of every client from now on.
    pub fn set_links(&mut self, up: LinkConditions, down: LinkConditions) {
        self.network.set_conditions(up, down);
    }

    /// Bytes sent so far (before loss): client to server, server to clients.
    pub fn bytes(&self) -> (u64, u64) {
        self.network.bytes()
    }

    /// Packets that arrived but could not be decoded, on any machine (never happens unless the wire
    /// format is broken).
    pub fn undecodable_packets(&self) -> u64 {
        self.server.undecodable_packets
            + self
                .clients
                .iter()
                .map(|client| client.undecodable_packets)
                .sum::<u64>()
    }
}
