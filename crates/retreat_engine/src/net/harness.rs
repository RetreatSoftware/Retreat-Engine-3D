//! An in-process harness: one server and N clients, each a full simulation `App`, connected by a
//! [`LoopbackNetwork`] with latency, jitter and loss. A `step` advances every machine by one tick with
//! `simulate_tick`, so tests and the sandbox's network checks are fast, reproducible and need no real
//! network. The server and clients are the real ones (`server.rs`, `client.rs`).

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::client::{ClientConfig, ClientPhase, NetClient, install_client, process_client_states};
use super::limits::ServerLimits;
use super::link::LinkConditions;
use super::server::{NetServer, ServerConfig, install_server};
use super::transport::LoopbackNetwork;
use super::{prepare_simulation_app, simulate_tick};

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

/// How a client's clock advances against the server's, like a real app's: a frame every
/// `every_ticks` server ticks that simulates the time passed (times `speed`), at most
/// `max_ticks_per_frame` ticks; the rest is lost, as Bevy loses it.
#[derive(Clone, Copy, Debug)]
pub struct ClientFrames {
    pub every_ticks: u64,
    pub max_ticks_per_frame: u64,
    /// 1 keeps the server's rate; 0.9 is a clock that runs slow.
    pub speed: f64,
}

impl Default for ClientFrames {
    fn default() -> Self {
        Self {
            every_ticks: 1,
            max_ticks_per_frame: 16,
            speed: 1.0,
        }
    }
}

#[derive(Default)]
struct ClientClock {
    frames: ClientFrames,
    owed_ticks: f64,
}

/// A server and clients in one process.
pub struct NetHarness {
    pub server_app: App,
    pub client_apps: Vec<App>,
    network: LoopbackNetwork,
    clocks: Vec<ClientClock>,
    /// Ticks stepped so far.
    pub global_tick: u64,
    /// Frame time `run_frames` has not yet turned into server ticks.
    owed_server_time: Duration,
}

impl NetHarness {
    /// `make_app` builds one simulation app (physics, the scene, the commands) with every plugin added
    /// but not finished; the harness installs the networking and prepares it. It is called once for the
    /// server and once per client.
    pub fn new(
        config: HarnessConfig,
        allowed_commands: &[&str],
        client_count: u32,
        make_app: impl Fn() -> App,
    ) -> Self {
        Self::with_limits(
            config,
            ServerLimits::default(),
            allowed_commands,
            client_count,
            make_app,
        )
    }

    /// Like [`NetHarness::new`] with the server's `limits` chosen by the caller.
    pub fn with_limits(
        config: HarnessConfig,
        limits: ServerLimits,
        allowed_commands: &[&str],
        client_count: u32,
        make_app: impl Fn() -> App,
    ) -> Self {
        let network = LoopbackNetwork::new(config.up, config.down, config.seed);
        let mut server_app = make_app();
        install_server(
            &mut server_app,
            ServerConfig {
                allowed_commands: allowed_commands.iter().map(|id| id.to_string()).collect(),
                state_interval_ticks: config.state_interval_ticks,
                limits,
            },
            network.server(),
        );
        prepare_simulation_app(&mut server_app);
        let client_apps = (0..client_count)
            .map(|id| {
                let mut app = make_app();
                install_client(
                    &mut app,
                    ClientConfig {
                        margin_ticks: config.margin_ticks,
                        nonce: 1000 + id,
                        token: None,
                    },
                    network.connect(),
                );
                prepare_simulation_app(&mut app);
                app
            })
            .collect();
        Self {
            server_app,
            client_apps,
            network,
            clocks: (0..client_count).map(|_| ClientClock::default()).collect(),
            global_tick: 0,
            owed_server_time: Duration::ZERO,
        }
    }

    /// Advances everything by one tick. `inputs` lists `(client index, command lines)` that players
    /// make this tick.
    pub fn step(&mut self, inputs: &[(usize, Vec<String>)]) {
        self.global_tick += 1;
        self.network.set_tick(self.global_tick);
        simulate_tick(self.server_app.world_mut());
        for (index, app) in self.client_apps.iter_mut().enumerate() {
            for (who, lines) in inputs {
                if *who == index {
                    let mut client = app.world_mut().resource_mut::<NetClient>();
                    for line in lines {
                        client.queue_input(line.clone());
                    }
                }
            }
            let clock = &mut self.clocks[index];
            let paced = f64::from(app.world().resource::<NetClient>().pacing.time_scale);
            clock.owed_ticks += clock.frames.speed * paced;
            if !self.global_tick.is_multiple_of(clock.frames.every_ticks) {
                continue;
            }
            // A little slack, so adding 0.9 ten times makes nine whole ticks.
            let whole = (clock.owed_ticks + 1.0e-9).floor();
            clock.owed_ticks -= whole;
            let ticks = (whole as u64).min(clock.frames.max_ticks_per_frame);
            process_client_states(app.world_mut());
            for _ in 0..ticks {
                simulate_tick(app.world_mut());
            }
        }
    }

    /// Runs the clients the way real apps run: `frames` rendered frames of `frame` real time each
    /// (`App::update`, so Bevy's fixed loop, per-frame systems and drawing between ticks run),
    /// while the server ticks at its own rate. `after_frame` sees the harness after every frame.
    /// Do not mix with [`NetHarness::step`] afterwards (it leaves the clients on manual time).
    pub fn run_frames(
        &mut self,
        frame: Duration,
        frames: u64,
        mut after_frame: impl FnMut(&mut Self, u64),
    ) {
        for app in &mut self.client_apps {
            app.insert_resource(TimeUpdateStrategy::ManualDuration(frame));
        }
        let tick = self.server_app.world().resource::<Time<Fixed>>().timestep();
        for index in 0..frames {
            self.owed_server_time += frame;
            while self.owed_server_time >= tick {
                self.owed_server_time -= tick;
                self.global_tick += 1;
                self.network.set_tick(self.global_tick);
                simulate_tick(self.server_app.world_mut());
            }
            for app in &mut self.client_apps {
                app.update();
            }
            after_frame(self, index);
        }
    }

    /// Changes how fast client `index` runs from now on.
    pub fn set_client_frames(&mut self, index: usize, frames: ClientFrames) {
        self.clocks[index].frames = frames;
    }

    /// Steps `ticks` times with no player input.
    pub fn run_idle(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.step(&[]);
        }
    }

    /// Ends client `index`'s connection (the server hears one disconnect). The client's own app keeps
    /// running but nothing it sends arrives and nothing reaches it.
    pub fn disconnect_client(&mut self, index: usize) {
        self.network.disconnect(index as u32);
    }

    /// The in-process network, to connect extra raw connections (tests of misbehaving clients).
    pub fn network(&self) -> &LoopbackNetwork {
        &self.network
    }

    /// Changes the link conditions of every client from now on.
    pub fn set_links(&mut self, up: LinkConditions, down: LinkConditions) {
        self.network.set_conditions(up, down);
    }

    /// Bytes sent so far (before loss): client to server, server to clients.
    pub fn bytes(&self) -> (u64, u64) {
        self.network.bytes()
    }

    /// The server's networking state.
    pub fn server(&self) -> &NetServer {
        self.server_app.world().resource::<NetServer>()
    }

    /// Client `index`'s networking state.
    pub fn client(&self, index: usize) -> &NetClient {
        self.client_apps[index].world().resource::<NetClient>()
    }

    /// True when every client has joined.
    pub fn all_joined(&self) -> bool {
        (0..self.client_apps.len()).all(|index| self.client(index).phase == ClientPhase::Running)
    }

    /// Packets that arrived but could not be decoded, on any machine (never happens unless the wire
    /// format is broken).
    pub fn undecodable_packets(&self) -> u64 {
        self.server().undecodable_packets
            + (0..self.client_apps.len())
                .map(|index| self.client(index).undecodable_packets)
                .sum::<u64>()
    }
}
