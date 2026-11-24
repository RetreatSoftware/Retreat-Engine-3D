//! Runners connect a [`ServerNode`] or a [`ClientNode`] to a transport: they decode and encode
//! messages, do the handshake and call `step` once per tick.
//!
//! **Handshake.** A client sends `Hello { nonce }` (again every [`HELLO_RESEND_STEPS`] steps until
//! answered); the server replies `Welcome { nonce, client_id, tick }` with its last simulated tick. The
//! client measures the round trip in its own steps and starts simulating at
//! `welcome.tick + round_trip + margin_ticks`: where the server will be when the client's inputs
//! arrive, plus a safety margin for jitter. After that, every step sends one `Input` message.

use std::collections::BTreeSet;
use std::ops::{Deref, DerefMut};

use super::transport::{ClientEvent, ClientTransport, ConnectionId, ServerEvent, ServerTransport};
use super::wire;
use super::{ClientInput, ClientMessage, ClientNode, ServerMessage, ServerNode};

/// A client repeats its `Hello` this often while it waits for a `Welcome`.
pub const HELLO_RESEND_STEPS: u64 = 16;

/// A server node on a transport.
pub struct ServerRunner<T: ServerTransport> {
    pub node: ServerNode,
    transport: T,
    /// Connections that said hello.
    known: BTreeSet<ConnectionId>,
    /// Packets that could not be decoded.
    pub undecodable_packets: u64,
}

impl<T: ServerTransport> ServerRunner<T> {
    pub fn new(node: ServerNode, transport: T) -> Self {
        Self {
            node,
            transport,
            known: BTreeSet::new(),
            undecodable_packets: 0,
        }
    }

    /// One tick: read the transport, answer hellos, simulate, broadcast state.
    pub fn step(&mut self) {
        let mut inputs = Vec::new();
        for event in self.transport.poll() {
            match event {
                ServerEvent::Connected(_) => {}
                ServerEvent::Disconnected(id) => {
                    self.known.remove(&id);
                }
                ServerEvent::Packet { from, bytes } => match wire::decode_client_message(&bytes) {
                    Ok(ClientMessage::Hello { nonce }) => {
                        self.known.insert(from);
                        let welcome = ServerMessage::Welcome {
                            nonce,
                            client_id: from,
                            tick: self.node.tick,
                        };
                        self.transport
                            .send(from, wire::encode_server_message(&welcome));
                    }
                    Ok(ClientMessage::Input { tick, lines }) => {
                        // Input from a connection that never said hello is ignored.
                        if self.known.contains(&from) {
                            inputs.push(ClientInput {
                                client_id: from,
                                tick,
                                lines,
                            });
                        }
                    }
                    Err(_) => self.undecodable_packets += 1,
                },
            }
        }
        if let Some(message) = self.node.step(inputs) {
            let bytes = wire::encode_server_message(&message);
            for id in &self.known {
                self.transport.send(*id, bytes.clone());
            }
        }
    }
}

impl<T: ServerTransport> Deref for ServerRunner<T> {
    type Target = ServerNode;
    fn deref(&self) -> &ServerNode {
        &self.node
    }
}

impl<T: ServerTransport> DerefMut for ServerRunner<T> {
    fn deref_mut(&mut self) -> &mut ServerNode {
        &mut self.node
    }
}

/// Where a client is in joining.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientPhase {
    /// Waiting for a `Welcome`.
    Connecting,
    /// Simulating and sending input.
    Running,
}

/// A client node on a transport.
pub struct ClientRunner<T: ClientTransport> {
    pub node: ClientNode,
    transport: T,
    pub phase: ClientPhase,
    /// Steps taken so far (the client's own clock before it has a tick).
    steps: u64,
    nonce: u32,
    /// The step at which the first `Hello` went out (the round trip is measured from it).
    hello_first_sent_at: Option<u64>,
    /// Safety margin (ticks) added to the round trip; may be negative to provoke late inputs in tests.
    margin_ticks: i64,
    /// The round trip measured at the handshake, in steps.
    pub measured_round_trip: Option<u64>,
    pub undecodable_packets: u64,
}

impl<T: ClientTransport> ClientRunner<T> {
    pub fn new(node: ClientNode, transport: T, nonce: u32, margin_ticks: i64) -> Self {
        Self {
            node,
            transport,
            phase: ClientPhase::Connecting,
            steps: 0,
            nonce,
            hello_first_sent_at: None,
            margin_ticks,
            measured_round_trip: None,
            undecodable_packets: 0,
        }
    }

    /// One tick. `inputs` are the command lines the player makes now (ignored until running).
    pub fn step(&mut self, inputs: Vec<String>) {
        let mut states = Vec::new();
        for event in self.transport.poll() {
            let ClientEvent::Packet(bytes) = event else {
                continue;
            };
            match wire::decode_server_message(&bytes) {
                Ok(ServerMessage::Welcome {
                    nonce,
                    client_id,
                    tick,
                }) => {
                    if self.phase == ClientPhase::Connecting && nonce == self.nonce {
                        let sent_at = self.hello_first_sent_at.unwrap_or(self.steps);
                        let round_trip = self.steps.saturating_sub(sent_at);
                        self.measured_round_trip = Some(round_trip);
                        let start = (tick as i64 + round_trip as i64 + self.margin_ticks).max(0);
                        self.node.id = client_id;
                        self.node.start_at(start as u64);
                        self.phase = ClientPhase::Running;
                    }
                }
                Ok(state @ ServerMessage::State { .. }) => states.push(state),
                Err(_) => self.undecodable_packets += 1,
            }
        }
        match self.phase {
            ClientPhase::Connecting => {
                let due = match self.hello_first_sent_at {
                    None => true,
                    Some(first) => (self.steps - first).is_multiple_of(HELLO_RESEND_STEPS),
                };
                if due {
                    self.hello_first_sent_at.get_or_insert(self.steps);
                    let hello = ClientMessage::Hello { nonce: self.nonce };
                    self.transport.send(wire::encode_client_message(&hello));
                }
            }
            ClientPhase::Running => {
                let message = self.node.step(inputs, states);
                self.transport.send(wire::encode_client_message(&message));
            }
        }
        self.steps += 1;
    }
}

impl<T: ClientTransport> Deref for ClientRunner<T> {
    type Target = ClientNode;
    fn deref(&self) -> &ClientNode {
        &self.node
    }
}

impl<T: ClientTransport> DerefMut for ClientRunner<T> {
    fn deref_mut(&mut self) -> &mut ClientNode {
        &mut self.node
    }
}
