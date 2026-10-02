//! The Exit Proxy (ADR-0029): the HTTP proxy on loopback that every
//! connection of the browser and of the shells goes through. Chromium
//! starts with `--proxy-server` set to it, and the container environment
//! sets `HTTP_PROXY` and `HTTPS_PROXY` to it for every shell.
//!
//! A client sends `CONNECT host:port` for `https://`, `wss://` and
//! `ws://`, and the proxy answers with a TCP tunnel to that address. A
//! client sends a plain `http://` request in absolute form
//! (`GET http://host/path`), and the proxy forwards it to that host as an
//! HTTP/1.1 request in origin form. One client connection can carry
//! requests to several hosts. The forward adds no `Via` and no
//! `X-Forwarded-For`, because a site reads each one as the mark of a
//! proxy.
//!
//! Every connection that the proxy opens goes through [`dial`]: each
//! tunnel, and each connection of a forwarded request. The mode decides
//! where `dial` opens it. In `Direct` mode it opens a TCP connection from
//! the Computer, as a client with no proxy does.
//!
//! In `Direct` mode the proxy resolves each name itself, and it opens no
//! connection to this Computer's own loopback, to an unspecified address
//! or to a link-local address, as Squid's default `to_localhost` and
//! `to_linklocal` rules do. It checks an IPv4-mapped IPv6 address as its
//! IPv4 address, and it connects to the other addresses of the name in
//! order. A name with no other address gets 403. Chromium checks a page's
//! request to a local network against the address that it resolves, and
//! it resolves no name that it sends to a proxy, so without this rule a
//! page could name a host that resolves to 127.0.0.1 and reach the
//! services on the Computer's loopback. Private addresses pass: the
//! egress rules of the Docker host hold them.
//!
//! The daemon reads and sets the mode over the control endpoint
//! (`GET /exit`, `POST /exit`). A switch closes every connection that the
//! proxy holds, also when the mode stays the same, so each client opens a
//! new connection at once and every new connection takes the path of the
//! new mode.
//!
//! The proxy listens on loopback alone, so only the processes of this
//! Computer reach it.

use std::convert::Infallible;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::http::uri::{Authority, Scheme};
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// Where the proxy listens: the conventional port of an HTTP proxy, on
/// loopback. `browser.sh` and the container environment name the same
/// address.
pub const ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3128);

/// Where the proxy opens each connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Each connection leaves from the Computer, as a connection with no
    /// proxy does.
    Direct,
}

/// The answer of `GET /exit`: the mode, and the client connections that
/// the proxy holds now.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    pub mode: Mode,
    pub connections: usize,
}

/// The answer of `POST /exit`: the new mode, and how many connections the
/// switch closed.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Switched {
    pub mode: Mode,
    pub closed: usize,
}

/// The body of `POST /exit`, the wire shape with the daemon.
#[derive(Debug, Deserialize)]
pub struct SwitchBody {
    pub mode: Mode,
}

/// The mode and the client connections of the proxy, shared between the
/// proxy's runtime and the control endpoint.
pub struct ExitProxy {
    current: Mutex<Generation>,
    /// The client connections that the proxy holds now.
    connections: Arc<AtomicUsize>,
}

/// The mode, and the token that closes every connection that the proxy
/// took in while that mode was set.
struct Generation {
    mode: Mode,
    closing: CancellationToken,
}

impl ExitProxy {
    /// A proxy in `Direct` mode, the mode of every Computer at start.
    pub fn new() -> Self {
        Self {
            current: Mutex::new(Generation {
                mode: Mode::Direct,
                closing: CancellationToken::new(),
            }),
            connections: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn status(&self) -> Status {
        let current = self.current.lock().expect("exit proxy lock");
        Status {
            mode: current.mode,
            connections: self.connections.load(Ordering::SeqCst),
        }
    }

    /// Set the mode and close every connection that the proxy holds.
    pub fn switch(&self, mode: Mode) -> Switched {
        let mut current = self.current.lock().expect("exit proxy lock");
        let closed = self.connections.load(Ordering::SeqCst);
        current.closing.cancel();
        *current = Generation {
            mode,
            closing: CancellationToken::new(),
        };
        Switched { mode, closed }
    }

    /// Take in one client connection: count it, and give it the mode and
    /// the token of now. `switch` takes the same lock, so each connection
    /// is counted and closed by a switch, or comes after it and takes the
    /// new mode.
    fn admit(&self) -> Admitted {
        let current = self.current.lock().expect("exit proxy lock");
        self.connections.fetch_add(1, Ordering::SeqCst);
        Admitted {
            mode: current.mode,
            closing: current.closing.clone(),
            held: Arc::new(Held(Arc::clone(&self.connections))),
        }
    }
}

/// One client connection while any task of it runs: the connection
/// itself, its tunnel, or the upstream connection of a forwarded request.
/// The last task to end takes it out of the count.
struct Held(Arc<AtomicUsize>);

impl Drop for Held {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What every task of one client connection carries.
#[derive(Clone)]
struct Admitted {
    mode: Mode,
    closing: CancellationToken,
    held: Arc<Held>,
}

/// Bind the proxy's port. The bind happens before the runtime starts, so
/// a port that is taken stops screend at once.
pub fn listen() -> io::Result<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind(ADDRESS)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Serve the proxy on a runtime of this thread's own, for the life of the
/// process. One thread is enough: the proxy copies bytes and waits.
pub fn run(proxy: Arc<ExitProxy>, listener: std::net::TcpListener) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the Exit Proxy runtime");
    runtime.block_on(async move {
        let listener = TcpListener::from_std(listener).expect("the Exit Proxy listener");
        eprintln!("[screend] Exit Proxy on {ADDRESS}");
        serve(proxy, listener).await;
    });
}

/// Take in each client connection and serve it on a task of its own.
pub async fn serve(proxy: Arc<ExitProxy>, listener: TcpListener) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(connection(stream, proxy.admit()));
            }
            Err(error) => {
                // A full table of open files refuses the accept and not
                // the listener, so the proxy waits and accepts again.
                eprintln!("[screend] the Exit Proxy accepted no connection: {error}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// The body of each answer: an upstream body, or a short one of the
/// proxy's own.
type Body = BoxBody<Bytes, hyper::Error>;

/// Serve one client connection until it ends or a switch closes it. The
/// client sees each failure on it: hyper answers a malformed request
/// with 400 and closes, and a client that goes away ends its own
/// connection.
async fn connection(stream: TcpStream, admitted: Admitted) {
    nodelay(&stream);
    let closing = admitted.closing.clone();
    let service = hyper::service::service_fn(move |request| answer(request, admitted.clone()));
    let served = hyper::server::conn::http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    tokio::select! {
        _ = closing.cancelled() => {}
        _ = served => {}
    }
}

async fn answer(
    request: Request<Incoming>,
    admitted: Admitted,
) -> Result<Response<Body>, Infallible> {
    Ok(if request.method() == Method::CONNECT {
        tunnel(request, admitted).await
    } else {
        forward(request, admitted).await
    })
}

/// Answer `CONNECT host:port`: open the connection, and once hyper hands
/// over the client's connection, copy the bytes both ways.
async fn tunnel(request: Request<Incoming>, admitted: Admitted) -> Response<Body> {
    let Some((host, port)) = request
        .uri()
        .authority()
        .and_then(|authority| host_and_port(authority, None))
    else {
        return refusal(StatusCode::BAD_REQUEST, "a CONNECT names a host and a port");
    };
    let mut upstream = match dial(admitted.mode, &host, port).await {
        Ok(upstream) => upstream,
        Err(error) => return dial_refusal(error, &host, port),
    };
    tokio::spawn(async move {
        let Admitted { closing, held, .. } = admitted;
        // A reset from either end is how a tunnel ends, so the copy has
        // no failure to report.
        let splice = async move {
            if let Ok(upgraded) = hyper::upgrade::on(request).await {
                let mut client = TokioIo::new(upgraded);
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            }
        };
        tokio::select! {
            _ = closing.cancelled() => {}
            _ = splice => {}
        }
        drop(held);
    });
    Response::new(empty())
}

/// Forward one absolute-form `http://` request to its host, in origin
/// form, on a connection of its own.
async fn forward(mut request: Request<Incoming>, admitted: Admitted) -> Response<Body> {
    let uri = request.uri().clone();
    let Some(authority) = uri
        .authority()
        .filter(|_| uri.scheme() == Some(&Scheme::HTTP))
    else {
        return refusal(
            StatusCode::BAD_REQUEST,
            "the Exit Proxy forwards an http:// request in absolute form, \
             and tunnels every other connection with CONNECT",
        );
    };
    let Some((host, port)) = host_and_port(authority, Some(80)) else {
        return refusal(StatusCode::BAD_REQUEST, "the request names no host");
    };
    // The Host of the address, not the one the client sent (RFC 9112,
    // section 3.2.2), and never the user information of the address.
    let host_header = match authority.port() {
        Some(port) => format!("{}:{port}", authority.host()),
        None => authority.host().to_string(),
    };
    let Ok(host_header) = HeaderValue::from_str(&host_header) else {
        return refusal(StatusCode::BAD_REQUEST, "the request names no valid host");
    };
    *request.uri_mut() = uri
        .path_and_query()
        .map_or_else(|| Uri::from_static("/"), |path| Uri::from(path.clone()));
    strip_hop_by_hop(request.headers_mut());
    request.headers_mut().insert(header::HOST, host_header);

    let upstream = match dial(admitted.mode, &host, port).await {
        Ok(upstream) => upstream,
        Err(error) => return dial_refusal(error, &host, port),
    };
    let (mut sender, upstream_connection) = match hyper::client::conn::http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .handshake(TokioIo::new(upstream))
        .await
    {
        Ok(handshake) => handshake,
        Err(error) => {
            return refusal(
                StatusCode::BAD_GATEWAY,
                format!("{host}:{port} did not answer: {error}"),
            );
        }
    };
    // The upstream connection ends when its response does, or when a
    // switch closes the client connection that it serves.
    let Admitted { closing, held, .. } = admitted;
    tokio::spawn(async move {
        tokio::select! {
            _ = closing.cancelled() => {}
            _ = upstream_connection => {}
        }
        drop(held);
    });
    match sender.send_request(request).await {
        Ok(response) => {
            let mut response = response.map(BodyExt::boxed);
            strip_hop_by_hop(response.headers_mut());
            response
        }
        Err(error) => refusal(
            StatusCode::BAD_GATEWAY,
            format!("{host}:{port} gave no answer: {error}"),
        ),
    }
}

/// The stream of one connection that the proxy opened.
type Upstream = TcpStream;

/// Why a dial opened no connection.
#[derive(Debug)]
enum DialError {
    /// Every address of the name is one that the proxy refuses (see
    /// [`is_refused`]).
    Refused,
    /// The name did not resolve, or no address took the connection.
    Failed(io::Error),
}

/// Open one connection to `host:port` on the path of `mode`. Every
/// connection of the proxy opens here, and the HTTP of the proxy stays
/// above it: a dial gives a byte stream to `host:port`, and the name
/// resolves where the connection leaves.
async fn dial(mode: Mode, host: &str, port: u16) -> Result<Upstream, DialError> {
    match mode {
        Mode::Direct => {
            let addresses = tokio::net::lookup_host((host, port))
                .await
                .map_err(DialError::Failed)?;
            let stream = connect_to_allowed(addresses).await?;
            nodelay(&stream);
            Ok(stream)
        }
    }
}

/// Connect to the first address that takes the connection, in order,
/// and skip each address that the proxy refuses. The proxy connects to
/// the address that it checked, so a name cannot pass the check with one
/// address and connect to another.
async fn connect_to_allowed(
    addresses: impl IntoIterator<Item = SocketAddr>,
) -> Result<TcpStream, DialError> {
    let mut refused = false;
    let mut failure = None;
    for address in addresses {
        if is_refused(address.ip()) {
            refused = true;
            continue;
        }
        match TcpStream::connect(address).await {
            Ok(stream) => return Ok(stream),
            Err(error) => failure = Some(error),
        }
    }
    Err(match failure {
        Some(error) => DialError::Failed(error),
        None if refused => DialError::Refused,
        None => DialError::Failed(io::Error::new(
            io::ErrorKind::NotFound,
            "the name has no address",
        )),
    })
}

/// Whether the proxy refuses to open a connection to `ip`: an address of
/// this Computer's own loopback, an unspecified address (0.0.0.0/8 and
/// `::`), or a link-local address (169.254.0.0/16 and fe80::/10). An
/// IPv4-mapped IPv6 address is checked as its IPv4 address. Private
/// addresses pass: the egress rules of the Docker host hold them.
fn is_refused(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => ip.is_loopback() || ip.octets()[0] == 0 || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unspecified() || ip.is_unicast_link_local(),
    }
}

/// The answer to a dial that opened no connection.
fn dial_refusal(error: DialError, host: &str, port: u16) -> Response<Body> {
    match error {
        DialError::Refused => refusal(
            StatusCode::FORBIDDEN,
            format!(
                "the Exit Proxy opens no connection to this Computer's own loopback \
                 or to a link-local address, and every address of {host} is one"
            ),
        ),
        DialError::Failed(error) => refusal(
            StatusCode::BAD_GATEWAY,
            format!("{host}:{port} did not answer: {error}"),
        ),
    }
}

/// Send small writes at once, such as the messages of a WebSocket. A
/// socket that refuses the option still carries the bytes.
fn nodelay(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
}

/// The host and the port of an address, with the brackets of an IPv6
/// host removed for the dial.
fn host_and_port(authority: &Authority, default_port: Option<u16>) -> Option<(String, u16)> {
    let port = authority.port_u16().or(default_port)?;
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    (!host.is_empty()).then(|| (host.to_string(), port))
}

/// Remove the headers of one hop, which a proxy does not pass on
/// (RFC 9110, section 7.6.1): each header that `Connection` names, and
/// the headers that HTTP/1.1 keeps to one hop.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in [
        header::CONNECTION,
        header::PROXY_AUTHENTICATE,
        header::PROXY_AUTHORIZATION,
        header::TE,
        header::TRAILER,
        header::TRANSFER_ENCODING,
        header::UPGRADE,
        HeaderName::from_static("keep-alive"),
        HeaderName::from_static("proxy-connection"),
    ] {
        headers.remove(name);
    }
}

/// An answer of the proxy's own, with the reason as its body.
fn refusal(status: StatusCode, reason: impl Into<String>) -> Response<Body> {
    let body = Full::new(Bytes::from(reason.into()))
        .map_err(|never| match never {})
        .boxed();
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response
}

fn empty() -> Body {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty};
    use hyper::{Request, Response};
    use hyper_util::rt::TokioIo;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    /// How long a test waits for one answer.
    const WAIT: Duration = Duration::from_secs(5);

    /// An address of this machine off loopback, where the targets of
    /// these tests listen, because the proxy opens no connection to
    /// loopback. A UDP connect sends nothing: it takes the route to an
    /// address of TEST-NET-1, and with it the address of this machine on
    /// that route.
    fn allowed_ip() -> std::net::IpAddr {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").expect("a UDP socket");
        socket
            .connect("192.0.2.1:9")
            .expect("these tests need a route off loopback");
        let ip = socket.local_addr().expect("an address").ip();
        assert!(!ip.is_loopback(), "the route off loopback leaves from {ip}");
        ip
    }

    /// A proxy on a free loopback port.
    async fn proxy() -> (Arc<ExitProxy>, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
        let address = listener.local_addr().expect("the proxy address");
        let proxy = Arc::new(ExitProxy::new());
        tokio::spawn(serve(Arc::clone(&proxy), listener));
        (proxy, address)
    }

    /// A TCP server that greets each connection, then sends back what it
    /// reads.
    async fn echo_target() -> SocketAddr {
        let listener = TcpListener::bind((allowed_ip(), 0))
            .await
            .expect("bind a port");
        let address = listener.local_addr().expect("the target address");
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.expect("accept");
                tokio::spawn(async move {
                    stream.write_all(b"hello from the target\n").await.ok();
                    let (mut reader, mut writer) = stream.split();
                    tokio::io::copy(&mut reader, &mut writer).await.ok();
                });
            }
        });
        address
    }

    /// An HTTP server that answers each request with what it got: its
    /// own name, the method, the request target, the Host header and the
    /// header names.
    async fn http_target(name: &'static str) -> SocketAddr {
        let listener = TcpListener::bind((allowed_ip(), 0))
            .await
            .expect("bind a port");
        let address = listener.local_addr().expect("the target address");
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.expect("accept");
                let service = hyper::service::service_fn(move |request: Request<_>| async move {
                    let headers: Vec<String> = request
                        .headers()
                        .keys()
                        .map(|name| name.as_str().to_string())
                        .collect();
                    let seen = serde_json::json!({
                        "name": name,
                        "method": request.method().as_str(),
                        "target": request.uri().to_string(),
                        "host": request.headers()[hyper::header::HOST].to_str().unwrap_or(""),
                        "headers": headers,
                    });
                    Ok::<_, std::convert::Infallible>(Response::new(http_body_util::Full::new(
                        Bytes::from(seen.to_string()),
                    )))
                });
                tokio::spawn(
                    hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service),
                );
            }
        });
        address
    }

    /// Read one response head, byte by byte, so the bytes after it stay
    /// in the stream.
    async fn read_head(stream: &mut TcpStream) -> String {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let byte = tokio::time::timeout(WAIT, stream.read_u8())
                .await
                .expect("the head arrives in time")
                .expect("the head is readable");
            head.push(byte);
        }
        String::from_utf8(head).expect("the head is text")
    }

    /// Open a tunnel to `target` through the proxy and read the answer
    /// to the CONNECT.
    async fn tunnel_to(proxy: SocketAddr, target: &str) -> (TcpStream, String) {
        let mut stream = TcpStream::connect(proxy).await.expect("reach the proxy");
        stream
            .write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .await
            .expect("send the CONNECT");
        let head = read_head(&mut stream).await;
        (stream, head)
    }

    /// The answer of the proxy to one raw request on a new connection.
    async fn answer_to(proxy: SocketAddr, raw: &str) -> String {
        let mut stream = TcpStream::connect(proxy).await.expect("reach the proxy");
        stream.write_all(raw.as_bytes()).await.expect("send");
        read_head(&mut stream).await
    }

    /// Read exactly `expected.len()` bytes and compare them.
    async fn expect_bytes(stream: &mut TcpStream, expected: &[u8]) {
        let mut got = vec![0; expected.len()];
        tokio::time::timeout(WAIT, stream.read_exact(&mut got))
            .await
            .expect("the bytes arrive in time")
            .expect("the bytes are readable");
        assert_eq!(
            String::from_utf8_lossy(&got),
            String::from_utf8_lossy(expected)
        );
    }

    /// Wait until the proxy holds `count` connections.
    async fn wait_for_connections(proxy: &ExitProxy, count: usize) {
        let deadline = tokio::time::Instant::now() + WAIT;
        while proxy.status().connections != count {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the proxy holds {} connections, not {count}",
                proxy.status().connections
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Whether the peer closed `stream`: a read gives the end of the
    /// stream or an error.
    async fn closed(stream: &mut TcpStream) -> bool {
        let mut byte = [0; 1];
        matches!(
            tokio::time::timeout(WAIT, stream.read(&mut byte)).await,
            Ok(Ok(0) | Err(_))
        )
    }

    #[tokio::test]
    async fn a_connect_tunnel_carries_bytes_both_ways() {
        let (_proxy, address) = proxy().await;
        let target = echo_target().await;

        let (mut stream, head) = tunnel_to(address, &target.to_string()).await;

        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        expect_bytes(&mut stream, b"hello from the target\n").await;
        stream
            .write_all(b"ping\n")
            .await
            .expect("send through the tunnel");
        expect_bytes(&mut stream, b"ping\n").await;
    }

    /// Chromium and curl send a plain `http://` request to the proxy in
    /// absolute form, and Chromium sends requests to several hosts on one
    /// proxy connection. Each one reaches its own host in origin form,
    /// with the Host of its address and without the headers of the hop
    /// to the proxy.
    #[tokio::test]
    async fn an_absolute_form_request_reaches_its_host_also_two_hosts_on_one_connection() {
        let (proxy, address) = proxy().await;
        let first = http_target("first").await;
        let second = http_target("second").await;
        let stream = TcpStream::connect(address).await.expect("reach the proxy");
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .expect("an HTTP connection to the proxy");
        tokio::spawn(connection);

        let mut seen = Vec::new();
        for uri in [
            format!("http://{first}/one"),
            format!("http://{second}/two?x=1"),
        ] {
            let request = Request::get(uri)
                .header("Proxy-Connection", "keep-alive")
                .header("Proxy-Authorization", "Basic c2VjcmV0")
                .header("Connection", "keep-alive, X-Hop")
                .header("X-Hop", "one hop")
                .header("X-End", "end to end")
                .body(Empty::<Bytes>::new())
                .expect("a request");
            let response = tokio::time::timeout(WAIT, sender.send_request(request))
                .await
                .expect("an answer in time")
                .expect("an answer");
            assert_eq!(response.status(), 200);
            let body = response
                .into_body()
                .collect()
                .await
                .expect("a body")
                .to_bytes();
            seen.push(serde_json::from_slice::<serde_json::Value>(&body).expect("JSON"));
        }

        assert_eq!(seen[0]["name"], "first");
        assert_eq!(seen[0]["method"], "GET");
        assert_eq!(seen[0]["target"], "/one");
        assert_eq!(seen[0]["host"], first.to_string());
        assert_eq!(seen[1]["name"], "second");
        assert_eq!(seen[1]["target"], "/two?x=1");
        assert_eq!(seen[1]["host"], second.to_string());
        for answer in &seen {
            let headers = answer["headers"].as_array().expect("header names");
            assert!(headers.iter().any(|name| name == "x-end"), "{answer}");
            for hop in [
                "proxy-connection",
                "proxy-authorization",
                "connection",
                "x-hop",
            ] {
                assert!(
                    !headers.iter().any(|name| name == hop),
                    "the host got the hop header {hop}: {answer}"
                );
            }
        }
        // Both requests went over the one connection to the proxy.
        assert_eq!(proxy.status().connections, 1);
    }

    #[tokio::test]
    async fn a_malformed_request_is_refused() {
        let (_proxy, address) = proxy().await;

        for raw in [
            "NOT HTTP AT ALL\r\n\r\n",
            // A request to the proxy itself, in origin form.
            "GET /index.html HTTP/1.1\r\nHost: 127.0.0.1:3128\r\n\r\n",
            // A CONNECT with no port.
            "CONNECT example.com HTTP/1.1\r\nHost: example.com\r\n\r\n",
            // An absolute form that is not http://: a client tunnels
            // https:// with CONNECT.
            "GET https://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n",
        ] {
            let head = answer_to(address, raw).await;
            assert!(head.starts_with("HTTP/1.1 400"), "{raw:?} got {head}");
        }
    }

    /// A destination that does not answer is the gateway's failure, and
    /// the client hears it as one.
    #[tokio::test]
    async fn a_destination_that_refuses_is_a_bad_gateway() {
        let (_proxy, address) = proxy().await;
        let closed_port = {
            let listener = TcpListener::bind((allowed_ip(), 0))
                .await
                .expect("bind a port");
            listener.local_addr().expect("an address")
        };

        let (_stream, head) = tunnel_to(address, &closed_port.to_string()).await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        let head = answer_to(
            address,
            &format!("GET http://{closed_port}/ HTTP/1.1\r\nHost: {closed_port}\r\n\r\n"),
        )
        .await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    }

    /// A listener on every IPv4 address and on the IPv6 loopback, where
    /// it can bind, of one port. It counts the connections it takes.
    async fn counting_listener() -> (u16, Arc<AtomicUsize>) {
        let taken = Arc::new(AtomicUsize::new(0));
        let ipv4 = TcpListener::bind("0.0.0.0:0").await.expect("bind a port");
        let port = ipv4.local_addr().expect("an address").port();
        let mut listeners = vec![ipv4];
        if let Ok(ipv6) = TcpListener::bind(("::1", port)).await {
            listeners.push(ipv6);
        }
        for listener in listeners {
            let taken = Arc::clone(&taken);
            tokio::spawn(async move {
                while listener.accept().await.is_ok() {
                    taken.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
        (port, taken)
    }

    /// A page can name a host that resolves to this Computer's loopback,
    /// and Chromium resolves no name it sends to a proxy, so the proxy
    /// refuses each address of the loopback, the unspecified addresses
    /// and the link-local addresses itself, with 403, and dials none.
    #[tokio::test]
    async fn a_destination_on_loopback_or_link_local_is_forbidden_and_never_dialled() {
        let (_proxy, address) = proxy().await;
        let (port, taken) = counting_listener().await;

        for host in [
            "localhost",
            "127.0.0.1",
            "[::1]",
            "[::ffff:127.0.0.1]",
            "0.0.0.0",
            "169.254.169.254",
        ] {
            let target = format!("{host}:{port}");
            let (_stream, head) = tunnel_to(address, &target).await;
            assert!(
                head.starts_with("HTTP/1.1 403"),
                "CONNECT {target} got {head}"
            );
            let head = answer_to(
                address,
                &format!("GET http://{target}/ HTTP/1.1\r\nHost: {target}\r\n\r\n"),
            )
            .await;
            assert!(head.starts_with("HTTP/1.1 403"), "GET {target} got {head}");
        }

        assert_eq!(taken.load(Ordering::SeqCst), 0, "the proxy dialled");
        let mut stream = TcpStream::connect(address).await.expect("reach the proxy");
        stream
            .write_all(
                b"CONNECT 127.0.0.1:7900 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            )
            .await
            .expect("send the CONNECT");
        let mut answer = String::new();
        tokio::time::timeout(WAIT, stream.read_to_string(&mut answer))
            .await
            .expect("the answer arrives in time")
            .expect("the answer is readable");
        assert!(
            answer.contains("no connection to this Computer's own loopback"),
            "{answer}"
        );
    }

    /// A name that resolves to a refused address and to an allowed one
    /// reaches the allowed one, and the refused one is never dialled.
    #[tokio::test]
    async fn a_name_with_a_refused_and_an_allowed_address_reaches_the_allowed_one() {
        let (port, taken) = counting_listener().await;
        let allowed = SocketAddr::new(allowed_ip(), port);
        let loopback = SocketAddr::from(([127, 0, 0, 1], port));

        let stream = connect_to_allowed(vec![loopback, allowed])
            .await
            .expect("a connection");

        assert_eq!(stream.peer_addr().expect("a peer"), allowed);
        drop(stream);
        let deadline = tokio::time::Instant::now() + WAIT;
        while taken.load(Ordering::SeqCst) == 0 {
            assert!(tokio::time::Instant::now() < deadline, "nothing was taken");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(taken.load(Ordering::SeqCst), 1);
    }

    /// A name with no address is a failure of the gateway, not a refusal.
    #[tokio::test]
    async fn a_name_with_no_address_is_a_failure_and_not_a_refusal() {
        let error = connect_to_allowed(Vec::new())
            .await
            .expect_err("no connection");

        assert!(matches!(error, DialError::Failed(_)), "{error:?}");
    }

    /// The addresses that the proxy refuses, by the rule of production.
    #[test]
    fn the_proxy_refuses_loopback_unspecified_and_link_local_alone() {
        for refused in [
            "127.0.0.1",
            "127.1.2.3",
            "0.0.0.0",
            "169.254.169.254",
            "::1",
            "::",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "fe80::1",
        ] {
            let ip: std::net::IpAddr = refused.parse().expect("an address");
            assert!(is_refused(ip), "{refused} is not refused");
        }
        for allowed in [
            "93.184.215.14",
            "10.0.0.1",
            "172.18.0.5",
            "192.168.1.40",
            "100.64.0.1",
            "2606:2800:21f:cb07:6820:80da:af6b:8b2c",
            "fd00::1",
            "::ffff:10.0.0.1",
        ] {
            let ip: std::net::IpAddr = allowed.parse().expect("an address");
            assert!(!is_refused(ip), "{allowed} is refused");
        }
    }

    /// A switch closes every connection that the proxy holds, also when
    /// the mode stays the same, and answers how many it closed. The
    /// proxy goes on serving new connections.
    #[tokio::test]
    async fn a_switch_closes_every_connection_and_counts_them() {
        let (proxy, address) = proxy().await;
        let target = echo_target().await;
        let (mut first, _) = tunnel_to(address, &target.to_string()).await;
        let (mut second, _) = tunnel_to(address, &target.to_string()).await;
        expect_bytes(&mut first, b"hello from the target\n").await;
        expect_bytes(&mut second, b"hello from the target\n").await;
        // A connection that sent nothing yet.
        let mut idle = TcpStream::connect(address).await.expect("reach the proxy");
        wait_for_connections(&proxy, 3).await;

        let switched = proxy.switch(Mode::Direct);

        assert_eq!(
            switched,
            Switched {
                mode: Mode::Direct,
                closed: 3
            }
        );
        assert!(closed(&mut first).await, "the first tunnel stays open");
        assert!(closed(&mut second).await, "the second tunnel stays open");
        assert!(closed(&mut idle).await, "the idle connection stays open");
        wait_for_connections(&proxy, 0).await;

        let (mut after, head) = tunnel_to(address, &target.to_string()).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        expect_bytes(&mut after, b"hello from the target\n").await;
        assert_eq!(
            proxy.status(),
            Status {
                mode: Mode::Direct,
                connections: 1
            }
        );
    }

    #[test]
    fn the_proxy_listens_on_loopback_alone() {
        let listener = listen().expect("bind the proxy port");
        let address = listener.local_addr().expect("an address");

        assert!(address.ip().is_loopback(), "{address}");
        assert_eq!(address.port(), 3128);
    }

    /// The wire shapes of `GET /exit` and `POST /exit`, which the daemon
    /// reads.
    #[test]
    fn the_status_and_the_switch_speak_the_wire_shape() {
        let proxy = ExitProxy::new();

        assert_eq!(
            serde_json::to_value(proxy.status()).expect("JSON"),
            serde_json::json!({ "mode": "direct", "connections": 0 })
        );
        assert_eq!(
            serde_json::to_value(proxy.switch(Mode::Direct)).expect("JSON"),
            serde_json::json!({ "mode": "direct", "closed": 0 })
        );
        let body: SwitchBody =
            serde_json::from_str(r#"{"mode": "direct"}"#).expect("a switch body");
        assert_eq!(body.mode, Mode::Direct);
        assert!(serde_json::from_str::<SwitchBody>(r#"{"mode": "home"}"#).is_err());
    }
}
