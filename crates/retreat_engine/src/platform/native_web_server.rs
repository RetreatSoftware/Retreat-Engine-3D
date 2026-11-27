//! The server's own web page: `--serve-web <dir>` serves the browser build over HTTPS to players on
//! the local network. Native only.
//!
//! **Why HTTPS with our own certificate.** A browser offers WebTransport only to a secure context, and
//! `http://<LAN address>` is not one. The page therefore uses the self-signed certificate the server
//! makes at start: the player sees a certificate warning once and chooses to proceed (nothing is
//! installed). The game connection needs no such step: it pins the certificate by its hash
//! (`serverCertificateHashes`), which the page reads from [`SERVER_OFFER_PATH`].
//!
//! The server only reads files below one directory and answers `GET` and `HEAD`. It trusts nothing a
//! client sends: connections, request size and time are limited, and every connection closes after one
//! response. It is written on `rustls` (already linked by the WebTransport library) and blocking
//! threads, so it adds no HTTP crate.
//!
//! **Compressed files (decided 2026-11-27, package plan P1).** A release web build
//! (`build_web.mjs --release`) writes `<file>.br` beside each file that compresses well. When the
//! browser accepts Brotli ([`accepts_brotli`]) and that copy exists, it is sent instead, with
//! `Content-Encoding: br` and the original file's type; the server compresses nothing itself.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

/// The file the page asks for to learn which game server to join. The web build writes it as `null`
/// (no server); this server answers with its own [`server_offer_json`] instead of the file.
pub(crate) const SERVER_OFFER_PATH: &str = "retreat-server.json";
/// A browser opens about six connections per page, so this serves about ten players loading at once.
const MAX_CONNECTIONS: usize = 64;
const MAX_REQUEST_BYTES: usize = 8 * 1024;
/// Time to finish the handshake and send the request headers.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest wait for the client to take more of a response.
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

/// The JSON the page reads from [`SERVER_OFFER_PATH`].
pub(crate) fn server_offer_json(web_transport_port: u16, certificate_hash_hex: &str) -> String {
    format!(
        "{{\"web_transport_port\":{web_transport_port},\"certificate_hash\":\"{certificate_hash_hex}\"}}"
    )
}

/// The address other machines on the local network most likely reach this one at: the address the
/// system would send from towards the internet. Nothing is sent (a UDP "connect" only picks a route).
/// With a VPN or several networks it can be the wrong one; `--lan-address` overrides it.
pub(crate) fn detect_lan_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    // A documentation address (RFC 5737): no real host.
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}

/// What the server prints so the person who started it knows what to open and what may block it.
pub(crate) fn startup_lines(
    lan: Option<IpAddr>,
    web_port: u16,
    web_transport_port: u16,
) -> Vec<String> {
    let mut lines = Vec::new();
    match lan {
        Some(address) => {
            lines.push(format!(
                "retreat-web: players on this network open https://{}/",
                host_and_port(address, web_port)
            ));
            if !matches!(address, IpAddr::V4(v4) if v4.is_private()) {
                lines.push(format!(
                    "retreat-web: {address} is not a usual home-network address; if players cannot open it, restart with --lan-address <this computer's address>"
                ));
            }
        }
        None => lines.push(
            "retreat-web: no local network address found; connect to a network or pass --lan-address <this computer's address>"
                .to_string(),
        ),
    }
    lines.push(format!(
        "retreat-web: on this computer: https://localhost:{web_port}/"
    ));
    lines.push(
        "retreat-web: the browser warns about the certificate (this server made it): choose Advanced, then Proceed"
            .to_string(),
    );
    lines.push(format!(
        "retreat-web: if Windows asks about the firewall, allow this app on Private networks; other devices need TCP port {web_port} and UDP port {web_transport_port}"
    ));
    lines
}

fn host_and_port(address: IpAddr, port: u16) -> String {
    std::net::SocketAddr::new(address, port).to_string()
}

/// A running HTTPS file server. Its threads end with the process.
pub(crate) struct WebFileServer {
    pub port: u16,
}

/// What every connection serves.
struct Site {
    root: PathBuf,
    offer: String,
}

impl Site {
    /// The file at a safe relative path, if it is a file below the root. Canonical paths also stop a
    /// link inside the directory from leading out of it.
    fn open(&self, path: &str) -> Option<std::fs::File> {
        self.root
            .join(path)
            .canonicalize()
            .ok()
            .filter(|file| file.starts_with(&self.root) && file.is_file())
            .and_then(|file| std::fs::File::open(file).ok())
    }
}

impl WebFileServer {
    /// Serves `directory` on TCP `port` (0 picks a free one) with the given certificate (DER) and
    /// PKCS#8 key. `offer` answers [`SERVER_OFFER_PATH`].
    pub(crate) fn start(
        directory: &Path,
        port: u16,
        certificate_der: Vec<u8>,
        key_der: Vec<u8>,
        offer: String,
    ) -> Result<Self, String> {
        let root = directory.canonicalize().map_err(|error| {
            format!(
                "--serve-web: cannot open the directory `{}`: {error}; build the web page first",
                directory.display()
            )
        })?;
        if !root.join("index.html").is_file() {
            return Err(format!(
                "--serve-web: `{}` has no index.html; build the web page first (scripts/web/build_web.mjs) and pass its output directory",
                directory.display()
            ));
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .and_then(|builder| {
                builder.with_no_client_auth().with_single_cert(
                    vec![CertificateDer::from(certificate_der)],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
                )
            })
            .map_err(|error| format!("--serve-web: cannot set up HTTPS: {error}"))?;
        let listener = TcpListener::bind(("0.0.0.0", port)).map_err(|error| {
            format!(
                "--serve-web: cannot listen on TCP port {port}: {error}; pass another --web-port"
            )
        })?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let site = Arc::new(Site { root, offer });
        std::thread::Builder::new()
            .name("retreat-web".to_string())
            .spawn(move || accept_loop(listener, Arc::new(tls), site))
            .map_err(|error| format!("--serve-web: cannot start the web server: {error}"))?;
        Ok(Self { port })
    }
}

fn accept_loop(listener: TcpListener, tls: Arc<ServerConfig>, site: Arc<Site>) {
    let open = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            continue;
        }
        open.fetch_add(1, Ordering::SeqCst);
        let (tls, site, open_here) = (tls.clone(), site.clone(), open.clone());
        let spawned = std::thread::Builder::new()
            .name("retreat-web-connection".to_string())
            .spawn(move || {
                // A failure is the client's business (it refused the certificate, closed the
                // connection or sent nonsense): there is nothing to do about it here.
                let _ = serve_connection(stream, tls, &site);
                open_here.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            open.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn serve_connection(stream: TcpStream, tls: Arc<ServerConfig>, site: &Site) -> std::io::Result<()> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let connection = ServerConnection::new(tls).map_err(std::io::Error::other)?;
    let mut stream = StreamOwned::new(connection, stream);
    let head = read_request_head(&mut stream)?;
    respond(&mut stream, site, parse_request(&head))?;
    stream.conn.send_close_notify();
    stream.flush()
}

/// Reads up to the blank line that ends the headers.
fn read_request_head(stream: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut head = Vec::new();
    let mut buffer = [0u8; 1024];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        if head.len() > MAX_REQUEST_BYTES || Instant::now() > deadline {
            return Err(std::io::Error::other("request too large or too slow"));
        }
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(std::io::Error::other("closed before the request ended"));
        }
        head.extend_from_slice(&buffer[..read]);
    }
    Ok(head)
}

#[derive(Debug, PartialEq, Eq)]
struct Request {
    /// `HEAD`: headers only.
    head_only: bool,
    /// The requested path relative to the site, `/`-separated, safe to join to the root.
    path: String,
    /// The browser takes a Brotli-compressed answer (its `Accept-Encoding` header).
    brotli: bool,
}

/// The suffix of a file's compressed copy (written by `scripts/web/build_web.mjs --release`).
const BROTLI_SUFFIX: &str = ".br";

/// An HTTP status line's code and text.
type Status = (u16, &'static str);

const BAD_REQUEST: Status = (400, "Bad Request");
const NOT_FOUND: Status = (404, "Not Found");
const METHOD_NOT_ALLOWED: Status = (405, "Method Not Allowed");

fn parse_request(head: &[u8]) -> Result<Request, Status> {
    let line = head.split(|byte| *byte == b'\r').next().unwrap_or_default();
    let line = std::str::from_utf8(line).map_err(|_| BAD_REQUEST)?;
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(BAD_REQUEST);
    };
    if !version.starts_with("HTTP/1.") {
        return Err(BAD_REQUEST);
    }
    let head_only = match method {
        "GET" => false,
        "HEAD" => true,
        _ => return Err(METHOD_NOT_ALLOWED),
    };
    let path = safe_relative_path(target).ok_or(NOT_FOUND)?;
    let brotli = header_value(head, "accept-encoding").is_some_and(|value| accepts_brotli(&value));
    Ok(Request {
        head_only,
        path,
        brotli,
    })
}

/// The value of the first header called `name` (any case), if the request head has one.
fn header_value(head: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    text.split("\r\n").skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

/// Whether an `Accept-Encoding` value allows `br`: listed (or `*`) and not refused with `q=0`.
fn accepts_brotli(accept_encoding: &str) -> bool {
    accept_encoding.split(',').any(|item| {
        let mut parts = item.split(';');
        let coding = parts.next().unwrap_or_default().trim();
        let refused = parts.any(|parameter| {
            parameter
                .trim()
                .strip_prefix("q=")
                .and_then(|quality| quality.trim().parse::<f32>().ok())
                .is_some_and(|quality| quality <= 0.0)
        });
        (coding.eq_ignore_ascii_case("br") || coding == "*") && !refused
    })
}

/// The file a request target names, or `None` when it could name anything outside the site.
fn safe_relative_path(target: &str) -> Option<String> {
    let path = target.split(['?', '#']).next()?.strip_prefix('/')?;
    let path = percent_decode(path)?;
    if path.is_empty() {
        return Some("index.html".to_string());
    }
    let safe = path.split('/').all(|part| {
        !part.is_empty()
            && !part.ends_with(['.', ' '])
            && !part.contains(|c: char| c == '\\' || c == ':' || c.is_control())
    });
    safe.then_some(path)
}

fn percent_decode(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.bytes();
    while let Some(byte) = rest.next() {
        if byte != b'%' {
            bytes.push(byte);
            continue;
        }
        let high = char::from(rest.next()?).to_digit(16)?;
        let low = char::from(rest.next()?).to_digit(16)?;
        bytes.push((high * 16 + low) as u8);
    }
    String::from_utf8(bytes).ok()
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "wesl" | "cmds" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn write_head(
    stream: &mut impl Write,
    status: Status,
    kind: &str,
    length: u64,
    brotli: bool,
) -> std::io::Result<()> {
    let (code, text) = status;
    let encoding = if brotli {
        "Content-Encoding: br\r\n"
    } else {
        ""
    };
    write!(
        stream,
        "HTTP/1.1 {code} {text}\r\nContent-Type: {kind}\r\n{encoding}Content-Length: {length}\r\nCache-Control: no-store\r\nVary: Accept-Encoding\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n"
    )
}

fn respond(
    stream: &mut impl Write,
    site: &Site,
    request: Result<Request, Status>,
) -> std::io::Result<()> {
    let plain = "text/plain; charset=utf-8";
    let refuse = |stream: &mut dyn Write, status: Status| {
        let mut stream = stream;
        write_head(&mut stream, status, plain, status.1.len() as u64, false)?;
        stream.write_all(status.1.as_bytes())
    };
    let request = match request {
        Ok(request) => request,
        Err(status) => return refuse(stream, status),
    };
    if request.path == SERVER_OFFER_PATH {
        write_head(
            stream,
            (200, "OK"),
            content_type(&request.path),
            site.offer.len() as u64,
            false,
        )?;
        if !request.head_only {
            stream.write_all(site.offer.as_bytes())?;
        }
        return Ok(());
    }
    // The compressed copy when the browser takes it; the type is the original file's.
    let compressed = request
        .brotli
        .then(|| site.open(&format!("{}{BROTLI_SUFFIX}", request.path)))
        .flatten();
    let brotli = compressed.is_some();
    let Some(mut file) = compressed.or_else(|| site.open(&request.path)) else {
        return refuse(stream, NOT_FOUND);
    };
    let length = file.metadata()?.len();
    write_head(
        stream,
        (200, "OK"),
        content_type(&request.path),
        length,
        brotli,
    )?;
    if !request.head_only {
        std::io::copy(&mut file, stream)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_name_files_below_the_site_only() {
        let path = |target: &str| safe_relative_path(target);
        assert_eq!(path("/"), Some("index.html".into()));
        assert_eq!(path("/?connect=x"), Some("index.html".into()));
        assert_eq!(path("/assets/a%20b.png?v=1"), Some("assets/a b.png".into()));
        for bad in [
            "index.html",
            "/../secret",
            "/a/..",
            "/%2e%2e/secret",
            "/a%2F..%2Fb",
            "/a\\..\\b",
            "/%5c%5chost",
            "/C:/Windows",
            "//host/share",
            "/a//b",
            "/a/",
            "/a%00",
            "/a%zz",
            "/file.",
        ] {
            assert_eq!(path(bad), None, "{bad}");
        }
    }

    #[test]
    fn only_get_and_head_are_answered() {
        let get = parse_request(b"GET /x.js HTTP/1.1\r\nHost: a\r\n\r\n").expect("valid");
        assert_eq!((get.head_only, get.path.as_str()), (false, "x.js"));
        assert!(parse_request(b"HEAD / HTTP/1.1\r\n\r\n").unwrap().head_only);
        assert_eq!(
            parse_request(b"POST / HTTP/1.1\r\n\r\n"),
            Err(METHOD_NOT_ALLOWED)
        );
        assert_eq!(parse_request(b"nonsense\r\n\r\n"), Err(BAD_REQUEST));
        assert_eq!(parse_request(b"GET /../a HTTP/1.1\r\n\r\n"), Err(NOT_FOUND));
    }

    #[test]
    fn brotli_is_sent_only_to_browsers_that_accept_it() {
        assert!(accepts_brotli("gzip, deflate, br, zstd"));
        assert!(accepts_brotli("BR;q=0.5"));
        assert!(accepts_brotli("*"));
        assert!(!accepts_brotli("gzip, deflate"));
        assert!(!accepts_brotli("br;q=0, gzip"));
        assert!(!accepts_brotli("br; q=0.0"));
        assert!(!accepts_brotli("brotli"));
        assert!(!accepts_brotli(""));
        let request = |head: &[u8]| parse_request(head).expect("valid").brotli;
        assert!(request(
            b"GET /a.wasm HTTP/1.1\r\nHost: x\r\nAccept-Encoding: gzip, br\r\n\r\n"
        ));
        assert!(request(b"GET / HTTP/1.1\r\naccept-encoding:br\r\n\r\n"));
        assert!(!request(b"GET / HTTP/1.1\r\nHost: br\r\n\r\n"));
    }

    #[test]
    fn the_startup_lines_name_the_address_the_warning_and_both_ports() {
        let text = startup_lines(Some("192.168.1.20".parse().unwrap()), 8443, 5000).join("\n");
        assert!(text.contains("https://192.168.1.20:8443/"), "{text}");
        assert!(text.contains("https://localhost:8443/"), "{text}");
        assert!(text.contains("TCP port 8443 and UDP port 5000"), "{text}");
        assert!(!text.contains("--lan-address"), "{text}");
        let odd = startup_lines(Some("100.64.0.1".parse().unwrap()), 8443, 5000).join("\n");
        assert!(odd.contains("--lan-address"), "{odd}");
        let none = startup_lines(None, 8443, 5000).join("\n");
        assert!(none.contains("no local network address"), "{none}");
    }

    /// A real HTTPS exchange: a client that pins nothing but trusts this certificate.
    #[test]
    fn serves_files_and_the_offer_over_https() {
        let directory = std::env::temp_dir().join(format!("retreat-web-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("index.html"), "<p>page</p>").unwrap();
        std::fs::write(directory.join(SERVER_OFFER_PATH), "null").unwrap();
        let identity = wtransport::Identity::self_signed(["localhost"]).unwrap();
        let certificate = identity.certificate_chain().as_slice()[0].der().to_vec();
        let server = WebFileServer::start(
            &directory,
            0,
            certificate.clone(),
            identity.private_key().secret_der().to_vec(),
            server_offer_json(5000, "abcd"),
        )
        .expect("server");

        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(certificate)).unwrap();
        let client = Arc::new(
            rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth(),
        );
        let request = |path: &str, headers: &str| {
            let connection =
                rustls::ClientConnection::new(client.clone(), "localhost".try_into().unwrap())
                    .unwrap();
            let socket = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
            let mut stream = StreamOwned::new(connection, socket);
            write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: localhost\r\n{headers}\r\n"
            )
            .unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).unwrap();
            answer
        };
        let get = |path: &str| request(path, "");
        let page = get("/");
        assert!(page.starts_with("HTTP/1.1 200 OK"), "{page}");
        assert!(page.ends_with("\r\n\r\n<p>page</p>"), "{page}");
        assert!(!page.contains("Content-Encoding"), "{page}");

        // A compressed copy goes only to a browser that takes it, typed as the original.
        std::fs::write(directory.join("game.js"), "plain").unwrap();
        std::fs::write(directory.join("game.js.br"), "squeezed").unwrap();
        let brotli = request("/game.js", "Accept-Encoding: gzip, br\r\n");
        assert!(brotli.contains("Content-Encoding: br\r\n"), "{brotli}");
        assert!(brotli.contains("Content-Type: text/javascript"), "{brotli}");
        assert!(brotli.ends_with("\r\n\r\nsqueezed"), "{brotli}");
        let plain = request("/game.js", "Accept-Encoding: gzip\r\n");
        assert!(plain.ends_with("\r\n\r\nplain"), "{plain}");
        assert!(!plain.contains("Content-Encoding"), "{plain}");
        // A file without a compressed copy is sent as it is.
        let page = request("/", "Accept-Encoding: br\r\n");
        assert!(page.ends_with("\r\n\r\n<p>page</p>"), "{page}");
        assert!(!page.contains("Content-Encoding"), "{page}");
        let offer = get("/retreat-server.json");
        assert!(
            offer.ends_with("{\"web_transport_port\":5000,\"certificate_hash\":\"abcd\"}"),
            "{offer}"
        );
        assert!(get("/missing.js").starts_with("HTTP/1.1 404"));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_directory_without_a_page_is_refused_with_the_fix() {
        let error = WebFileServer::start(
            Path::new("no/such/directory"),
            0,
            Vec::new(),
            Vec::new(),
            String::new(),
        )
        .err()
        .expect("refused");
        assert!(error.contains("build the web page first"), "{error}");
    }
}
