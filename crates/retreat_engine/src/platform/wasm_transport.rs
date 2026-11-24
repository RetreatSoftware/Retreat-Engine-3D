//! Browser WebTransport: a client that implements `net::transport::ClientTransport` through the browser's
//! own `WebTransport` API (datagrams). A server cannot run in a browser.
//!
//! The JavaScript half lives in `web/index.html` as `window.__retreatNet` (`connect`, `send`, `poll`);
//! this file is the Rust half. Events cross the boundary as byte arrays whose first byte is the kind:
//! `0` connected, `1` datagram (the rest is the payload), `2` disconnected (the rest is the reason as
//! UTF-8). `poll` is non-blocking and `send` never waits, like every transport.
//!
//! A self-signed certificate is accepted by its SHA-256 hash (`serverCertificateHashes`), exactly like
//! the native client.

use bevy::prelude::App;
use js_sys::{Array, Uint8Array};
use wasm_bindgen::prelude::*;

use crate::NetLaunch;
use crate::net::client::{ClientConfig as NetClientConfig, install_client};
use crate::net::transport::{ClientEvent, ClientTransport};

/// Safety margin (ticks) a browser client adds to the measured round trip.
const CLIENT_MARGIN_TICKS: i64 = 6;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__retreatNet"], js_name = connect)]
    fn retreat_net_connect(url: &str, certificate_hash_hex: &str);
    #[wasm_bindgen(js_namespace = ["window", "__retreatNet"], js_name = send)]
    fn retreat_net_send(bytes: &[u8]);
    #[wasm_bindgen(js_namespace = ["window", "__retreatNet"], js_name = poll)]
    fn retreat_net_poll() -> Array;
}

/// The browser's WebTransport client.
pub struct BrowserTransportClient;

impl BrowserTransportClient {
    /// Starts connecting to `url`; `certificate_hash_hex` (64 hex digits, or empty for none) pins a
    /// self-signed certificate.
    pub fn connect(url: &str, certificate_hash_hex: &str) -> Self {
        retreat_net_connect(url, certificate_hash_hex);
        Self
    }
}

impl ClientTransport for BrowserTransportClient {
    fn poll(&mut self) -> Vec<ClientEvent> {
        retreat_net_poll()
            .iter()
            .filter_map(|value| {
                let bytes = Uint8Array::new(&value).to_vec();
                let (kind, rest) = bytes.split_first()?;
                match kind {
                    0 => Some(ClientEvent::Connected),
                    1 => Some(ClientEvent::Packet(rest.to_vec())),
                    2 => Some(ClientEvent::Disconnected(
                        String::from_utf8_lossy(rest).to_string(),
                    )),
                    _ => None,
                }
            })
            .collect()
    }

    fn send(&mut self, bytes: Vec<u8>) {
        retreat_net_send(&bytes);
    }
}

/// Turns `?connect=<url>` into the client networking systems of `app`. A server cannot run in a
/// browser, so `NetLaunch::Server` is an error.
pub(crate) fn install_net(app: &mut App, net: &NetLaunch) -> Result<(), String> {
    match net {
        NetLaunch::Off => Ok(()),
        NetLaunch::Server { .. } => Err(
            "a server cannot run in a browser; start it natively with `--server <port>`"
                .to_string(),
        ),
        NetLaunch::Client { url, cert_hash } => {
            let transport =
                BrowserTransportClient::connect(url, cert_hash.as_deref().unwrap_or(""));
            install_client(
                app,
                NetClientConfig {
                    margin_ticks: CLIENT_MARGIN_TICKS,
                    nonce: js_sys::Math::random().to_bits() as u32,
                },
                transport,
            );
            Ok(())
        }
    }
}
