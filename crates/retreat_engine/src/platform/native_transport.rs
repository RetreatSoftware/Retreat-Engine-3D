//! Native WebTransport: a server and a client that implement the `net::transport` traits over
//! `wtransport` (QUIC datagrams). Native only; the browser has its own client in `wasm_transport.rs`.
//!
//! Each object owns a small Tokio runtime on background threads and talks to the game loop through a
//! channel, so `poll` and `send` never block (the traits' contract). Datagrams are unreliable and
//! unordered, which suits input and state messages; the largest message is limited by the connection's
//! datagram size (about 1200 bytes): an oversized message is dropped and counted in
//! `oversized_dropped`.
//!
//! **Testing without a real certificate.** The server makes a self-signed certificate valid for 14 days
//! and publishes its SHA-256 hash ([`WebTransportServer::certificate_hash_hex`]). A client that is given
//! the hash accepts exactly that certificate (the W3C `serverCertificateHashes` mechanism, which
//! browsers support too), so no certificate authority is needed. A production server would use a real
//! certificate and clients would pass no hash.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use wtransport::tls::{Sha256Digest, Sha256DigestFmt};
use wtransport::{ClientConfig, Connection, Endpoint, Identity, ServerConfig};

use bevy::prelude::App;

use crate::NetLaunch;
use crate::net::client::{ClientConfig as NetClientConfig, install_client};
use crate::net::server::{ServerConfig as NetServerConfig, install_server};
use crate::net::transport::{
    ClientEvent, ClientTransport, ConnectionId, ServerEvent, ServerTransport,
};
use retreat_engine_api::NetAllowedCommands;

/// Ticks between state broadcasts of a server started with `--server`.
const SERVER_STATE_INTERVAL_TICKS: u64 = 2;
/// Safety margin (ticks) a client started with `--connect` adds to the measured round trip.
const CLIENT_MARGIN_TICKS: i64 = 6;

/// Turns `--server` / `--connect` into the networking systems of `app`. Call it after the game's
/// plugins (the server's allow-list is the game's `NetAllowedCommands` resource). A server prints one
/// line, `retreat-net: listening on UDP port <port> certificate-hash <hex>`, so a script that starts
/// clients can read the port and the certificate hash from its output.
pub(crate) fn install_net(app: &mut App, net: &NetLaunch) -> Result<(), String> {
    match net {
        NetLaunch::Off => Ok(()),
        NetLaunch::Server { port } => {
            let transport = WebTransportServer::start(*port)?;
            println!(
                "retreat-net: listening on UDP port {} certificate-hash {}",
                transport.port,
                transport.certificate_hash_hex()
            );
            let allowed = app
                .world()
                .get_resource::<NetAllowedCommands>()
                .map(|allowed| allowed.0.iter().cloned().collect())
                .unwrap_or_default();
            install_server(
                app,
                NetServerConfig {
                    allowed_commands: allowed,
                    state_interval_ticks: SERVER_STATE_INTERVAL_TICKS,
                },
                transport,
            );
            Ok(())
        }
        NetLaunch::Client { url, cert_hash } => {
            let transport = WebTransportClient::connect(url, cert_hash.as_deref())?;
            install_client(
                app,
                NetClientConfig {
                    margin_ticks: CLIENT_MARGIN_TICKS,
                    nonce: std::process::id(),
                },
                transport,
            );
            Ok(())
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the network runtime: {error}"))
}

/// Parses a certificate hash written as 64 hex digits, with or without `:` separators.
pub fn parse_certificate_hash(text: &str) -> Result<Sha256Digest, String> {
    let digits: String = text.chars().filter(|c| *c != ':').collect();
    if digits.len() != 64 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "certificate hash `{text}` must be 64 hexadecimal digits (a SHA-256), optionally separated by `:`"
        ));
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16)
            .map_err(|error| error.to_string())?;
    }
    Ok(Sha256Digest::new(bytes))
}

type Connections = Arc<Mutex<HashMap<ConnectionId, Connection>>>;

/// A WebTransport server.
pub struct WebTransportServer {
    events: Receiver<ServerEvent>,
    connections: Connections,
    /// SHA-256 of the self-signed certificate, to give to clients (hex with `:`).
    certificate_hash: String,
    pub port: u16,
    /// Messages that were too large for a datagram and were dropped.
    pub oversized_dropped: Arc<AtomicU64>,
    // Keeps the runtime (and so the accept loop) alive.
    _runtime: tokio::runtime::Runtime,
}

impl WebTransportServer {
    /// Starts listening on `port` (0 picks a free port; see `self.port`).
    pub fn start(port: u16) -> Result<Self, String> {
        let runtime = runtime()?;
        let identity = Identity::self_signed(["localhost", "127.0.0.1", "::1"])
            .map_err(|error| format!("cannot make a self-signed certificate: {error}"))?;
        let certificate_hash = identity.certificate_chain().as_slice()[0]
            .hash()
            .fmt(Sha256DigestFmt::DottedHex)
            .replace(':', "");
        let config = ServerConfig::builder()
            .with_bind_default(port)
            .with_identity(identity)
            .build();
        let endpoint = {
            let _guard = runtime.enter();
            Endpoint::server(config)
                .map_err(|error| format!("cannot listen on UDP port {port}: {error}"))?
        };
        let bound_port = endpoint
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let (sender, events) = channel();
        let connections: Connections = Arc::default();
        runtime.spawn(accept_loop(endpoint, sender, connections.clone()));
        Ok(Self {
            events,
            connections,
            certificate_hash,
            port: bound_port,
            oversized_dropped: Arc::new(AtomicU64::new(0)),
            _runtime: runtime,
        })
    }

    /// The certificate hash as 64 hex digits, for clients (`--cert-hash`).
    pub fn certificate_hash_hex(&self) -> &str {
        &self.certificate_hash
    }
}

async fn accept_loop(
    endpoint: Endpoint<wtransport::endpoint::endpoint_side::Server>,
    events: Sender<ServerEvent>,
    connections: Connections,
) {
    let mut next_id: ConnectionId = 0;
    loop {
        let incoming = endpoint.accept().await;
        let events = events.clone();
        let connections = connections.clone();
        let id = next_id;
        next_id += 1;
        tokio::spawn(async move {
            let Ok(request) = incoming.await else { return };
            let Ok(connection) = request.accept().await else {
                return;
            };
            connections.lock().unwrap().insert(id, connection.clone());
            let _ = events.send(ServerEvent::Connected(id));
            while let Ok(datagram) = connection.receive_datagram().await {
                let _ = events.send(ServerEvent::Packet {
                    from: id,
                    bytes: datagram.payload().to_vec(),
                });
            }
            connections.lock().unwrap().remove(&id);
            let _ = events.send(ServerEvent::Disconnected(id));
        });
    }
}

impl ServerTransport for WebTransportServer {
    fn poll(&mut self) -> Vec<ServerEvent> {
        self.events.try_iter().collect()
    }

    fn send(&mut self, to: ConnectionId, bytes: Vec<u8>) {
        let connection = self.connections.lock().unwrap().get(&to).cloned();
        let Some(connection) = connection else {
            return;
        };
        if connection
            .max_datagram_size()
            .is_some_and(|max| bytes.len() > max)
        {
            self.oversized_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let _ = connection.send_datagram(bytes);
    }
}

/// A WebTransport client.
pub struct WebTransportClient {
    events: Receiver<ClientEvent>,
    connection: Arc<Mutex<Option<Connection>>>,
    pub oversized_dropped: Arc<AtomicU64>,
    _runtime: tokio::runtime::Runtime,
}

impl WebTransportClient {
    /// Connects to `url` (`https://host:port`). With `certificate_hash` the client accepts exactly that
    /// self-signed certificate; with `None` it uses the system's certificate authorities.
    pub fn connect(url: &str, certificate_hash: Option<&str>) -> Result<Self, String> {
        let runtime = runtime()?;
        let builder = ClientConfig::builder().with_bind_default();
        let config = match certificate_hash {
            Some(hash) => builder
                .with_server_certificate_hashes([parse_certificate_hash(hash)?])
                .build(),
            None => builder.with_native_certs().build(),
        };
        let endpoint = {
            let _guard = runtime.enter();
            Endpoint::client(config)
                .map_err(|error| format!("cannot open a client socket: {error}"))?
        };
        let (sender, events) = channel();
        let connection: Arc<Mutex<Option<Connection>>> = Arc::default();
        let url = url.to_string();
        let shared = connection.clone();
        runtime.spawn(async move {
            let connected = match endpoint.connect(&url).await {
                Ok(connected) => connected,
                Err(error) => {
                    let _ = sender.send(ClientEvent::Disconnected(format!(
                        "cannot connect to {url}: {error}"
                    )));
                    return;
                }
            };
            *shared.lock().unwrap() = Some(connected.clone());
            let _ = sender.send(ClientEvent::Connected);
            loop {
                match connected.receive_datagram().await {
                    Ok(datagram) => {
                        let _ = sender.send(ClientEvent::Packet(datagram.payload().to_vec()));
                    }
                    Err(error) => {
                        let _ = sender.send(ClientEvent::Disconnected(error.to_string()));
                        break;
                    }
                }
            }
            *shared.lock().unwrap() = None;
        });
        Ok(Self {
            events,
            connection,
            oversized_dropped: Arc::new(AtomicU64::new(0)),
            _runtime: runtime,
        })
    }
}

impl ClientTransport for WebTransportClient {
    fn poll(&mut self) -> Vec<ClientEvent> {
        self.events.try_iter().collect()
    }

    fn send(&mut self, bytes: Vec<u8>) {
        let connection = self.connection.lock().unwrap().clone();
        let Some(connection) = connection else {
            return;
        };
        if connection
            .max_datagram_size()
            .is_some_and(|max| bytes.len() > max)
        {
            self.oversized_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let _ = connection.send_datagram(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Polls `check` until it returns `Some`, or panics after five seconds.
    fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(found) = check() {
                return found;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn certificate_hashes_parse_with_or_without_colons_and_reject_garbage() {
        let plain = "ab".repeat(32);
        let colons = vec!["ab"; 32].join(":");
        assert_eq!(
            parse_certificate_hash(&plain),
            parse_certificate_hash(&colons)
        );
        assert!(parse_certificate_hash(&plain).is_ok());
        assert!(
            parse_certificate_hash("12345")
                .unwrap_err()
                .contains("64 hexadecimal digits")
        );
        assert!(parse_certificate_hash(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn a_client_with_the_certificate_hash_exchanges_datagrams_with_the_server() {
        let mut server = WebTransportServer::start(0).expect("server");
        let url = format!("https://127.0.0.1:{}", server.port);
        let mut client =
            WebTransportClient::connect(&url, Some(server.certificate_hash_hex())).expect("client");

        let connection_id = wait_for("the server to see the client", || {
            server.poll().into_iter().find_map(|event| match event {
                ServerEvent::Connected(id) => Some(id),
                _ => None,
            })
        });
        wait_for("the client to connect", || {
            client
                .poll()
                .into_iter()
                .any(|event| event == ClientEvent::Connected)
                .then_some(())
        });

        client.send(b"hello server".to_vec());
        let received = wait_for("a datagram at the server", || {
            server.poll().into_iter().find_map(|event| match event {
                ServerEvent::Packet { from, bytes } => Some((from, bytes)),
                _ => None,
            })
        });
        assert_eq!(received, (connection_id, b"hello server".to_vec()));

        server.send(connection_id, b"hello client".to_vec());
        let reply = wait_for("a datagram at the client", || {
            client.poll().into_iter().find_map(|event| match event {
                ClientEvent::Packet(bytes) => Some(bytes),
                _ => None,
            })
        });
        assert_eq!(reply, b"hello client".to_vec());
    }

    #[test]
    fn a_client_with_the_wrong_certificate_hash_is_refused() {
        let server = WebTransportServer::start(0).expect("server");
        let url = format!("https://127.0.0.1:{}", server.port);
        let wrong = "00".repeat(32);
        let mut client = WebTransportClient::connect(&url, Some(&wrong)).expect("client object");
        let reason = wait_for("the refusal", || {
            client.poll().into_iter().find_map(|event| match event {
                ClientEvent::Disconnected(reason) => Some(reason),
                _ => None,
            })
        });
        assert!(reason.contains("cannot connect"), "{reason}");
    }

    #[test]
    fn a_datagram_larger_than_the_connection_allows_is_dropped_and_counted() {
        let mut server = WebTransportServer::start(0).expect("server");
        let url = format!("https://127.0.0.1:{}", server.port);
        let mut client =
            WebTransportClient::connect(&url, Some(server.certificate_hash_hex())).expect("client");
        wait_for("the client to connect", || {
            client
                .poll()
                .into_iter()
                .any(|event| event == ClientEvent::Connected)
                .then_some(())
        });
        client.send(vec![0u8; 100_000]);
        assert_eq!(client.oversized_dropped.load(Ordering::Relaxed), 1);
        // The server is untouched.
        assert!(
            server
                .poll()
                .iter()
                .all(|event| !matches!(event, ServerEvent::Packet { .. }))
        );
    }
}
