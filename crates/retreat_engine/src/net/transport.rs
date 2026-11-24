//! The transport interface: how encoded messages travel between a server and its clients.
//!
//! A transport moves *datagrams* (unreliable, unordered byte messages: a lost state or input is simply
//! replaced by the next one, which is what prediction wants) and reports connections coming and going.
//! Both traits are non-blocking: `poll` returns what has arrived since the last call and never waits,
//! so one thread can drive a transport from the game loop (also in the single-threaded browser).
//!
//! Implementations: [`LoopbackNetwork`] (in one process, over [`SimulatedLink`]s, for tests and the
//! harness) lives here; the WebTransport implementation lives in the platform layer.

use std::cell::RefCell;
use std::rc::Rc;

use super::link::{LinkConditions, SimulatedLink};

/// Identifies one client connection on the server.
pub type ConnectionId = u32;

/// What a server transport reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Connected(ConnectionId),
    Disconnected(ConnectionId),
    Packet { from: ConnectionId, bytes: Vec<u8> },
}

/// What a client transport reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    Connected,
    /// The connection ended or could not be made; the text says why.
    Disconnected(String),
    Packet(Vec<u8>),
}

/// The server side of a transport.
pub trait ServerTransport {
    /// Everything that happened since the last call.
    fn poll(&mut self) -> Vec<ServerEvent>;
    /// Sends a datagram to one client (dropped if the connection is gone).
    fn send(&mut self, to: ConnectionId, bytes: Vec<u8>);
}

/// The client side of a transport.
pub trait ClientTransport {
    /// Everything that happened since the last call.
    fn poll(&mut self) -> Vec<ClientEvent>;
    /// Sends a datagram to the server (dropped while not connected).
    fn send(&mut self, bytes: Vec<u8>);
}

struct Connection {
    up: SimulatedLink<Vec<u8>>,
    down: SimulatedLink<Vec<u8>>,
    announced: bool,
    client_announced: bool,
    /// The connection was ended with [`LoopbackNetwork::disconnect`]; the server is told once.
    closed: bool,
    closed_announced: bool,
}

struct Shared {
    now: u64,
    connections: Vec<Connection>,
    up_conditions: LinkConditions,
    down_conditions: LinkConditions,
    seed: u64,
    bytes_up: u64,
    bytes_down: u64,
}

/// An in-process network: any number of clients connect to one server over simulated links. The owner
/// calls [`LoopbackNetwork::set_tick`] once per tick; latency is counted in those ticks.
#[derive(Clone)]
pub struct LoopbackNetwork {
    shared: Rc<RefCell<Shared>>,
}

impl LoopbackNetwork {
    pub fn new(up: LinkConditions, down: LinkConditions, seed: u64) -> Self {
        Self {
            shared: Rc::new(RefCell::new(Shared {
                now: 0,
                connections: Vec::new(),
                up_conditions: up,
                down_conditions: down,
                seed,
                bytes_up: 0,
                bytes_down: 0,
            })),
        }
    }

    /// Tells the network what tick it is (messages sent now are due `latency` ticks later).
    pub fn set_tick(&self, tick: u64) {
        self.shared.borrow_mut().now = tick;
    }

    /// The server's end.
    pub fn server(&self) -> LoopbackServer {
        LoopbackServer {
            shared: self.shared.clone(),
        }
    }

    /// Connects a new client; its connection id is its index in connection order.
    pub fn connect(&self) -> LoopbackClient {
        let mut shared = self.shared.borrow_mut();
        let id = shared.connections.len() as ConnectionId;
        let seed = shared.seed.wrapping_add(u64::from(id) * 2);
        let (up, down) = (shared.up_conditions, shared.down_conditions);
        shared.connections.push(Connection {
            up: SimulatedLink::new(up, seed),
            down: SimulatedLink::new(down, seed.wrapping_add(1)),
            announced: false,
            client_announced: false,
            closed: false,
            closed_announced: false,
        });
        LoopbackClient {
            id,
            shared: self.shared.clone(),
        }
    }

    /// Ends connection `id`: the server gets one `Disconnected` and nothing more is delivered either way.
    pub fn disconnect(&self, id: ConnectionId) {
        if let Some(connection) = self.shared.borrow_mut().connections.get_mut(id as usize) {
            connection.closed = true;
        }
    }

    /// Changes the conditions of every connection for messages sent from now on.
    pub fn set_conditions(&self, up: LinkConditions, down: LinkConditions) {
        let mut shared = self.shared.borrow_mut();
        shared.up_conditions = up;
        shared.down_conditions = down;
        for connection in &mut shared.connections {
            connection.up.set_conditions(up);
            connection.down.set_conditions(down);
        }
    }

    /// Bytes handed to the network so far (before loss): client to server, server to clients.
    pub fn bytes(&self) -> (u64, u64) {
        let shared = self.shared.borrow();
        (shared.bytes_up, shared.bytes_down)
    }
}

/// The server end of a [`LoopbackNetwork`].
pub struct LoopbackServer {
    shared: Rc<RefCell<Shared>>,
}

impl ServerTransport for LoopbackServer {
    fn poll(&mut self) -> Vec<ServerEvent> {
        let mut shared = self.shared.borrow_mut();
        let now = shared.now;
        let mut events = Vec::new();
        for (index, connection) in shared.connections.iter_mut().enumerate() {
            let id = index as ConnectionId;
            if !connection.announced {
                connection.announced = true;
                events.push(ServerEvent::Connected(id));
            }
            if connection.closed {
                if !connection.closed_announced {
                    connection.closed_announced = true;
                    events.push(ServerEvent::Disconnected(id));
                }
                continue;
            }
            for bytes in connection.up.deliver(now) {
                events.push(ServerEvent::Packet { from: id, bytes });
            }
        }
        events
    }

    fn send(&mut self, to: ConnectionId, bytes: Vec<u8>) {
        let mut shared = self.shared.borrow_mut();
        let now = shared.now;
        shared.bytes_down += bytes.len() as u64;
        if let Some(connection) = shared.connections.get_mut(to as usize)
            && !connection.closed
        {
            connection.down.send(now, bytes);
        }
    }
}

/// One client end of a [`LoopbackNetwork`].
pub struct LoopbackClient {
    id: ConnectionId,
    shared: Rc<RefCell<Shared>>,
}

impl ClientTransport for LoopbackClient {
    fn poll(&mut self) -> Vec<ClientEvent> {
        let mut shared = self.shared.borrow_mut();
        let now = shared.now;
        let connection = &mut shared.connections[self.id as usize];
        let mut events = Vec::new();
        if !connection.client_announced {
            connection.client_announced = true;
            events.push(ClientEvent::Connected);
        }
        for bytes in connection.down.deliver(now) {
            events.push(ClientEvent::Packet(bytes));
        }
        events
    }

    fn send(&mut self, bytes: Vec<u8>) {
        let mut shared = self.shared.borrow_mut();
        let now = shared.now;
        shared.bytes_up += bytes.len() as u64;
        let connection = &mut shared.connections[self.id as usize];
        if !connection.closed {
            connection.up.send(now, bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(latency_ticks: u32) -> LoopbackNetwork {
        let conditions = LinkConditions {
            latency_ticks,
            jitter_ticks: 0,
            loss: 0.0,
        };
        LoopbackNetwork::new(conditions, conditions, 1)
    }

    #[test]
    fn a_new_connection_is_reported_once_on_both_ends() {
        let net = network(0);
        let mut server = net.server();
        let mut client = net.connect();
        assert_eq!(server.poll(), vec![ServerEvent::Connected(0)]);
        assert!(server.poll().is_empty());
        assert_eq!(client.poll(), vec![ClientEvent::Connected]);
        assert!(client.poll().is_empty());
    }

    #[test]
    fn packets_travel_both_ways_after_their_latency() {
        let net = network(2);
        let mut server = net.server();
        let mut client = net.connect();
        server.poll();
        client.poll();
        net.set_tick(10);
        client.send(vec![1, 2]);
        net.set_tick(11);
        assert!(server.poll().is_empty());
        net.set_tick(12);
        assert_eq!(
            server.poll(),
            vec![ServerEvent::Packet {
                from: 0,
                bytes: vec![1, 2]
            }]
        );
        server.send(0, vec![9]);
        net.set_tick(14);
        assert_eq!(client.poll(), vec![ClientEvent::Packet(vec![9])]);
        assert_eq!(net.bytes(), (2, 1));
    }

    #[test]
    fn each_client_has_its_own_connection_id() {
        let net = network(0);
        let mut server = net.server();
        let mut first = net.connect();
        let mut second = net.connect();
        server.poll();
        second.send(vec![7]);
        first.send(vec![8]);
        let events = server.poll();
        assert!(events.contains(&ServerEvent::Packet {
            from: 1,
            bytes: vec![7]
        }));
        assert!(events.contains(&ServerEvent::Packet {
            from: 0,
            bytes: vec![8]
        }));
    }

    #[test]
    fn sending_to_an_unknown_connection_is_ignored() {
        let net = network(0);
        let mut server = net.server();
        server.send(5, vec![1]);
    }
}
