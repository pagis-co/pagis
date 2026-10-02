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
//! the Computer, as a client with no proxy does. In `Home` mode it sends
//! the connection to the exit listener of the daemon, which carries it
//! through the Person's Home Exit, or from the server when that Host is
//! absent (see [`Daemon`]).
//!
//! In `Direct` mode the proxy resolves each name itself, and it opens no
//! connection to this Computer itself, by loopback or by any of its
//! addresses, to an unspecified address or to a link-local address. This
//! is Squid's default `to_localhost` and `to_linklocal` rules, with every
//! address of the Computer added to loopback. A UDP bind tells an address
//! of the Computer from any other, and a bind that fails for another
//! reason than EADDRNOTAVAIL refuses the address. The proxy checks an
//! IPv4-mapped IPv6 address as its IPv4 address, and it connects to the
//! other addresses of the name in order. A name with no other address
//! gets 403. Chromium checks a page's request to a local network against
//! the address that it resolves, and it resolves no name that it sends to
//! a proxy, so without this rule a page could name a host that resolves
//! to the Computer and reach the services that listen in it. The private
//! addresses of other machines pass: the egress rules of the Docker host
//! hold them.
//!
//! In `Home` mode a literal private address (RFC 1918, a unique local
//! IPv6 address or carrier-grade NAT) still leaves from the Computer, so
//! the egress rules of the Docker host and `PAGIS_COMPUTER_ALLOW` hold
//! it. The refusal of this Computer, loopback and link-local applies to
//! a literal first. Every other destination goes to the daemon, every
//! name too: the name resolves where the connection leaves, which is the
//! Person's machine.
//!
//! The daemon reads and sets the mode over the control endpoint
//! (`GET /exit`, `POST /exit`). A switch closes every connection that the
//! proxy holds, also when the mode stays the same, so each client opens a
//! new connection at once and every new connection takes the path of the
//! new mode. The Computer starts in the mode that `PAGIS_EXIT_MODE`
//! names, `direct` when it names none.
//!
//! Each mode writes Chromium's managed policy file [`POLICY`]: `Home`
//! sets `WebRtcIPHandling` to `disable_non_proxied_udp`, so WebRTC sends
//! no UDP past the proxy and shows no address of the server, and
//! `Direct` empties the file. The image makes the file and gives it to
//! the `screen` uid that runs screend. screend rewrites its contents
//! alone and never makes or removes a file in the policy directory, so
//! no other policy can enter it. Chromium watches the directory and
//! applies the change with no restart.
//!
//! The proxy listens on loopback alone, so only the processes of this
//! Computer reach it.

use std::convert::Infallible;
use std::io::{self, Write as _};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// Where the proxy listens: the conventional port of an HTTP proxy, on
/// loopback. `browser.sh` and the container environment name the same
/// address.
pub const ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3128);

/// Chromium's managed policy file of the Exit Proxy. The image makes it,
/// owned by `screen` with mode 644, and it holds `{}` until a mode writes
/// it.
pub const POLICY: &str = "/etc/chromium/policies/managed/pagis-exit.json";

/// The variable that names the exit listener of the daemon, as
/// `host:port`. The daemon sets it on the Computers of a Server alone.
pub const DAEMON_VARIABLE: &str = "PAGIS_EXIT_DAEMON";

/// The variable that names the mode the proxy starts in.
pub const MODE_VARIABLE: &str = "PAGIS_EXIT_MODE";

/// The longest head that the proxy reads from the daemon.
const DAEMON_HEAD_LIMIT: usize = 8 * 1024;

/// The longest reason that the proxy reads from the daemon.
const DAEMON_REASON_LIMIT: usize = 1024;

/// How long the proxy waits for the daemon to take its connection.
const DAEMON_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the proxy waits for the daemon's answer to a `CONNECT`. The
/// daemon waits for the Person's Home Exit to resolve the name and
/// connect, so the answer can take a connect timeout of that machine.
const DAEMON_ANSWER_TIMEOUT: Duration = Duration::from_secs(60);

/// Where the proxy opens each connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Each connection leaves from the Computer, as a connection with no
    /// proxy does.
    Direct,
    /// Each connection goes to the daemon, which carries it through the
    /// Person's Home Exit, or from the server when that Host is absent.
    /// A literal private address leaves from the Computer.
    Home,
}

impl Mode {
    /// The mode that `PAGIS_EXIT_MODE` names: `direct` when it names
    /// none.
    pub fn from_variable(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim) {
            None | Some("") | Some("direct") => Ok(Mode::Direct),
            Some("home") => Ok(Mode::Home),
            Some(other) => Err(format!(
                "{MODE_VARIABLE} is {other:?}; it is \"direct\" or \"home\""
            )),
        }
    }

    /// The contents of [`POLICY`] in this mode.
    fn policy(self) -> String {
        let policy = match self {
            Mode::Direct => serde_json::json!({}),
            Mode::Home => serde_json::json!({ "WebRtcIPHandling": "disable_non_proxied_udp" }),
        };
        format!("{policy}\n")
    }
}

/// The exit listener of the daemon, which carries each connection of
/// `Home` mode, and the token of this Computer that it asks for. The
/// token is the one of the control endpoint, so the daemon knows the
/// Computer, its Agent and its Person by it.
#[derive(Clone)]
pub struct Daemon {
    /// `host:port`, such as `host.docker.internal:4403`.
    pub address: String,
    pub token: String,
}

impl Daemon {
    /// The daemon that `PAGIS_EXIT_DAEMON` names, with the token of the
    /// control endpoint. A Computer of a Local Installation has neither,
    /// and runs in `Direct` mode alone.
    pub fn from_variable(address: Option<&str>, token: Option<&str>) -> Option<Self> {
        let address = address
            .map(str::trim)
            .filter(|address| !address.is_empty())?;
        let token = token.filter(|token| !token.is_empty())?;
        Some(Self {
            address: address.to_string(),
            token: token.to_string(),
        })
    }
}

/// Why a switch did not happen. The mode and the connections stay as
/// they were.
#[derive(Debug)]
pub enum SwitchError {
    /// `Home` mode needs the daemon, and this Computer names none.
    NoDaemon,
    /// The policy file of the mode could not be written.
    Policy(io::Error),
}

impl std::fmt::Display for SwitchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwitchError::NoDaemon => write!(
                f,
                "this Computer names no exit listener of the daemon ({DAEMON_VARIABLE} is not \
                 set, or the control token is absent), so it runs in direct mode alone"
            ),
            SwitchError::Policy(error) => {
                write!(
                    f,
                    "the policy file {POLICY} of the mode was not written: {error}"
                )
            }
        }
    }
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
    /// The daemon of `Home` mode, or `None` where the Computer runs in
    /// `Direct` mode alone.
    daemon: Option<Arc<Daemon>>,
    /// Chromium's policy file that each mode writes.
    policy: PathBuf,
}

/// The mode, and the token that closes every connection that the proxy
/// took in while that mode was set.
struct Generation {
    mode: Mode,
    closing: CancellationToken,
}

impl ExitProxy {
    /// A proxy in `mode`, with the policy file of that mode written. It
    /// fails when the mode is `Home` and there is no daemon, and when the
    /// policy file cannot be written: screend then stops, because a
    /// Computer that leaves on another path than its Person chose is a
    /// fault to see and not to hide.
    pub fn start(daemon: Option<Daemon>, policy: PathBuf, mode: Mode) -> Result<Self, SwitchError> {
        let proxy = Self {
            current: Mutex::new(Generation {
                mode: Mode::Direct,
                closing: CancellationToken::new(),
            }),
            connections: Arc::new(AtomicUsize::new(0)),
            daemon: daemon.map(Arc::new),
            policy,
        };
        proxy.switch(mode)?;
        Ok(proxy)
    }

    pub fn status(&self) -> Status {
        let current = self.current.lock().expect("exit proxy lock");
        Status {
            mode: current.mode,
            connections: self.connections.load(Ordering::SeqCst),
        }
    }

    /// Set the mode, write its policy file, and close every connection
    /// that the proxy holds. A switch that fails changes nothing.
    pub fn switch(&self, mode: Mode) -> Result<Switched, SwitchError> {
        let mut current = self.current.lock().expect("exit proxy lock");
        if mode == Mode::Home && self.daemon.is_none() {
            return Err(SwitchError::NoDaemon);
        }
        write_policy(&self.policy, mode).map_err(SwitchError::Policy)?;
        let closed = self.connections.load(Ordering::SeqCst);
        current.closing.cancel();
        *current = Generation {
            mode,
            closing: CancellationToken::new(),
        };
        Ok(Switched { mode, closed })
    }

    /// Take in one client connection: count it, and give it the path and
    /// the token of now. `switch` takes the same lock, so each connection
    /// is counted and closed by a switch, or comes after it and takes the
    /// new mode.
    fn admit(&self) -> Admitted {
        let current = self.current.lock().expect("exit proxy lock");
        self.connections.fetch_add(1, Ordering::SeqCst);
        let route = match (current.mode, &self.daemon) {
            (Mode::Home, Some(daemon)) => Route::Home(Arc::clone(daemon)),
            _ => Route::Direct,
        };
        Admitted {
            route,
            closing: current.closing.clone(),
            held: Arc::new(Held(Arc::clone(&self.connections))),
        }
    }
}

/// Rewrite the contents of the policy file. It opens the file that is
/// there and never makes one, so screend adds no file to Chromium's
/// policy directory.
fn write_policy(path: &Path, mode: Mode) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)?;
    file.write_all(mode.policy().as_bytes())?;
    file.flush()
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

/// Where the connections of one client connection open: the route of
/// the mode that was set when the proxy took the connection in.
#[derive(Clone)]
enum Route {
    Direct,
    Home(Arc<Daemon>),
}

/// What every task of one client connection carries.
#[derive(Clone)]
struct Admitted {
    route: Route,
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
    let mut upstream = match dial(&admitted.route, &host, port).await {
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

    let upstream = match dial(&admitted.route, &host, port).await {
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
    /// The exit of `Home` mode refused the destination, for the reason
    /// that it gave: the daemon, or the Person's Home Exit.
    Forbidden(String),
    /// The name did not resolve, or no address took the connection.
    Failed(io::Error),
}

/// Open one connection to `host:port` on `route`. Every connection of
/// the proxy opens here, and the HTTP of the proxy stays above it: a dial
/// gives a byte stream to `host:port`, and the name resolves where the
/// connection leaves.
async fn dial(route: &Route, host: &str, port: u16) -> Result<Upstream, DialError> {
    match route {
        Route::Direct => dial_from_here(host, port).await,
        Route::Home(daemon) => match host.parse::<IpAddr>().map(|ip| ip.to_canonical()) {
            Ok(ip) if is_refused(ip) => Err(DialError::Refused),
            Ok(ip) if is_private(ip) => dial_from_here(host, port).await,
            _ => through_daemon(daemon, host, port).await,
        },
    }
}

/// Resolve the name here, and connect from the Computer.
async fn dial_from_here(host: &str, port: u16) -> Result<Upstream, DialError> {
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(DialError::Failed)?;
    let stream = connect_to_allowed(addresses).await?;
    nodelay(&stream);
    Ok(stream)
}

/// Ask the daemon for a tunnel to `host:port`, as a client asks this
/// proxy: one `CONNECT` with the Computer's token. A 200 gives the
/// stream to the tunnel or the forward that asked for it, so the HTTP of
/// the proxy stays in the proxy. A 403 is the daemon's refusal or the
/// Home Exit's, and every other answer is a failure of the gateway.
async fn through_daemon(daemon: &Daemon, host: &str, port: u16) -> Result<Upstream, DialError> {
    let failed = |reason: String| DialError::Failed(io::Error::other(reason));
    let mut stream = match tokio::time::timeout(
        DAEMON_CONNECT_TIMEOUT,
        TcpStream::connect(daemon.address.as_str()),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            return Err(failed(format!(
                "the daemon at {} did not answer: {error}",
                daemon.address
            )));
        }
        Err(_) => {
            return Err(failed(format!(
                "the daemon at {} took no connection in time",
                daemon.address
            )));
        }
    };
    nodelay(&stream);
    let target = authority(host, port);
    let request = format!(
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Bearer {}\r\n\r\n",
        daemon.token
    );
    let answer = async {
        stream.write_all(request.as_bytes()).await?;
        let head = read_daemon_head(&mut stream).await?;
        let reason = match head.status {
            200 => String::new(),
            _ => read_reason(&mut stream, head.content_length).await,
        };
        Ok::<_, io::Error>((head.status, reason))
    };
    let (status, reason) = tokio::time::timeout(DAEMON_ANSWER_TIMEOUT, answer)
        .await
        .map_err(|_| failed(format!("the daemon gave no answer for {target} in time")))?
        .map_err(|error| failed(format!("the daemon gave no answer for {target}: {error}")))?;
    match status {
        200 => Ok(stream),
        403 => Err(DialError::Forbidden(reason)),
        status => Err(failed(format!(
            "the daemon answered {status} for {target}: {reason}"
        ))),
    }
}

/// `host:port`, with an IPv6 host in brackets, as a `CONNECT` names it.
fn authority(host: &str, port: u16) -> String {
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => format!("[{host}]:{port}"),
        _ => format!("{host}:{port}"),
    }
}

/// The status and the body length of one answer of the daemon.
struct DaemonHead {
    status: u16,
    content_length: usize,
}

/// Read the head of the daemon's answer byte by byte, so the bytes of the
/// tunnel after it stay in the stream.
async fn read_daemon_head(stream: &mut TcpStream) -> io::Result<DaemonHead> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() == DAEMON_HEAD_LIMIT {
            return Err(io::Error::other("the head is too long"));
        }
        head.push(stream.read_u8().await?);
    }
    let head = String::from_utf8_lossy(&head);
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| io::Error::other("the answer has no status"))?;
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0);
    Ok(DaemonHead {
        status,
        content_length,
    })
}

/// The reason in the body of a refusal, cut to a length the proxy keeps.
async fn read_reason(stream: &mut TcpStream, length: usize) -> String {
    let mut body = vec![0; length.min(DAEMON_REASON_LIMIT)];
    match stream.read_exact(&mut body).await {
        Ok(_) => String::from_utf8_lossy(&body).into_owned(),
        Err(_) => String::new(),
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

/// Whether `ip` is a private address that `Home` mode opens from the
/// Computer: RFC 1918 (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16),
/// carrier-grade NAT (100.64.0.0/10) or a unique local IPv6 address
/// (fc00::/7). An IPv4-mapped IPv6 address is checked as its IPv4
/// address. Such a destination is on the server's network, never at the
/// Person's home, and the egress rules of the Docker host hold it.
fn is_private(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            ip.is_private() || (first == 100 && (64..128).contains(&second))
        }
        IpAddr::V6(ip) => ip.is_unique_local(),
    }
}

/// Whether the proxy refuses to open a connection to `ip`: this Computer
/// itself, by loopback or by any of its addresses, an unspecified address
/// (0.0.0.0/8 and `::`), or a link-local address (169.254.0.0/16 and
/// fe80::/10). An IPv4-mapped IPv6 address is checked as its IPv4
/// address. The private addresses of other machines pass: the egress
/// rules of the Docker host hold them.
fn is_refused(ip: IpAddr) -> bool {
    let ip = ip.to_canonical();
    let special = match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.octets()[0] == 0 || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unspecified() || ip.is_unicast_link_local(),
    };
    special || is_this_computer(ip)
}

/// Whether `ip` is an address of this Computer. A socket binds only to an
/// address of this machine, and the kernel answers every other address
/// with EADDRNOTAVAIL. The probe socket closes at once. A bind that fails
/// for another reason gives no answer, so the address counts as this
/// Computer's, and the rule fails closed.
fn is_this_computer(ip: IpAddr) -> bool {
    match std::net::UdpSocket::bind((ip, 0)) {
        Ok(_probe) => true,
        Err(error) => error.kind() != io::ErrorKind::AddrNotAvailable,
    }
}

/// The answer to a dial that opened no connection.
fn dial_refusal(error: DialError, host: &str, port: u16) -> Response<Body> {
    match error {
        DialError::Refused => refusal(
            StatusCode::FORBIDDEN,
            format!(
                "the Exit Proxy opens no connection to this Computer itself \
                 (loopback or one of its addresses) or to a link-local address, \
                 and every address of {host} is one"
            ),
        ),
        DialError::Forbidden(reason) => refusal(
            StatusCode::FORBIDDEN,
            format!("the exit of this Computer refused {host}:{port}: {reason}"),
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

    /// The address of this machine on its route off loopback. A UDP
    /// connect sends nothing: it takes the route to an address of
    /// TEST-NET-1, and with it the address of this machine on that route.
    fn own_ip() -> IpAddr {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").expect("a UDP socket");
        socket
            .connect("192.0.2.1:9")
            .expect("these tests need a route off loopback");
        let ip = socket.local_addr().expect("an address").ip();
        assert!(!ip.is_loopback(), "the route off loopback leaves from {ip}");
        ip
    }

    /// The ports of the targets that `serve_the_proxy_test_targets`
    /// serves, and one port of that machine where nothing listens.
    const ECHO_PORT: u16 = 7101;
    const FIRST_PORT: u16 = 7102;
    const SECOND_PORT: u16 = 7103;
    const CLOSED_PORT: u16 = 7104;

    /// The machine of the targets of the tests that need a destination
    /// that answers. The proxy opens no connection to this machine, so
    /// those targets run on another one: a second container or network
    /// namespace that runs `serve_the_proxy_test_targets`, by the name or
    /// the address in `PAGIS_EXIT_TEST_TARGETS`.
    async fn targets() -> IpAddr {
        let name = std::env::var("PAGIS_EXIT_TEST_TARGETS")
            .expect("PAGIS_EXIT_TEST_TARGETS names the machine of serve_the_proxy_test_targets");
        let address = tokio::net::lookup_host((name.as_str(), 0))
            .await
            .expect("the machine of the targets resolves")
            .next()
            .expect("the machine of the targets has an address");
        assert!(
            !is_refused(address.ip()),
            "{name} is this machine, and the proxy refuses it"
        );
        address.ip()
    }

    /// A policy file as the image makes it: `{}`, in a file of this test
    /// alone, because the tests run in parallel threads.
    fn policy_file() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "screend-exit-policy-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::write(&path, "{}").expect("write the test policy file");
        path
    }

    /// The token that the Home tests give the proxy.
    const TOKEN: &str = "test-token";

    /// A proxy on a free loopback port, in `Direct` mode with no daemon.
    async fn proxy() -> (Arc<ExitProxy>, SocketAddr) {
        proxy_of(ExitProxy::start(None, policy_file(), Mode::Direct).expect("the proxy starts"))
            .await
    }

    /// A proxy on a free loopback port, in `Home` mode with the daemon at
    /// `daemon`.
    async fn home_proxy(daemon: &str) -> (Arc<ExitProxy>, SocketAddr) {
        let daemon = Daemon {
            address: daemon.to_string(),
            token: TOKEN.to_string(),
        };
        proxy_of(
            ExitProxy::start(Some(daemon), policy_file(), Mode::Home).expect("the proxy starts"),
        )
        .await
    }

    async fn proxy_of(proxy: ExitProxy) -> (Arc<ExitProxy>, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
        let address = listener.local_addr().expect("the proxy address");
        let proxy = Arc::new(proxy);
        tokio::spawn(serve(Arc::clone(&proxy), listener));
        (proxy, address)
    }

    /// The heads of the `CONNECT` requests that a fake daemon read.
    type Heads = Arc<Mutex<Vec<String>>>;

    /// A fake exit listener of the daemon on loopback. It reads the head
    /// of one `CONNECT` on each connection and keeps it, then answers
    /// with `answer`. After a 200 it is the destination: it greets, then
    /// sends back what it reads.
    async fn fake_daemon(answer: &'static str) -> (String, Heads) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
        let address = listener.local_addr().expect("an address").to_string();
        let heads: Heads = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&heads);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let kept = Arc::clone(&kept);
                tokio::spawn(async move {
                    let head = read_head(&mut stream).await;
                    kept.lock().expect("the heads").push(head);
                    stream.write_all(answer.as_bytes()).await.ok();
                    if answer.starts_with("HTTP/1.1 200") {
                        stream.write_all(b"hello from the daemon\n").await.ok();
                        let (mut reader, mut writer) = stream.split();
                        tokio::io::copy(&mut reader, &mut writer).await.ok();
                    }
                });
            }
        });
        (address, heads)
    }

    /// A fake daemon that, after a 200, is an HTTP server that answers
    /// each request with its target and its Host header.
    async fn fake_http_daemon() -> (String, Heads) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
        let address = listener.local_addr().expect("an address").to_string();
        let heads: Heads = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&heads);
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let kept = Arc::clone(&kept);
                tokio::spawn(async move {
                    let head = read_head(&mut stream).await;
                    kept.lock().expect("the heads").push(head);
                    stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.ok();
                    let service = hyper::service::service_fn(|request: Request<_>| async move {
                        let seen = format!(
                            "{} {}",
                            request.uri(),
                            request.headers()[hyper::header::HOST]
                                .to_str()
                                .unwrap_or("")
                        );
                        Ok::<_, std::convert::Infallible>(Response::new(http_body_util::Full::new(
                            Bytes::from(seen),
                        )))
                    });
                    hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                        .ok();
                });
            }
        });
        (address, heads)
    }

    /// A TCP server on `port` that greets each connection, then sends
    /// back what it reads.
    async fn echo_target(port: u16) {
        let listener = TcpListener::bind(("0.0.0.0", port))
            .await
            .expect("bind the echo port");
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
    }

    /// An HTTP server on `port` that answers each request with what it
    /// got: its own name, the method, the request target, the Host header
    /// and the header names.
    async fn http_target(name: &'static str, port: u16) {
        let listener = TcpListener::bind(("0.0.0.0", port))
            .await
            .expect("bind the HTTP port");
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
    }

    /// The targets of the tests that need a destination that answers.
    /// Run it on another machine than those tests, such as a second
    /// container on one Docker network with them, and name that machine
    /// in `PAGIS_EXIT_TEST_TARGETS` for them:
    ///
    /// ```text
    /// docker network create screend-test
    /// docker run -d --name screend-targets --network screend-test <builder> \
    ///     cargo test --locked serve_the_proxy_test_targets -- --ignored
    /// docker run --rm --network screend-test -e PAGIS_EXIT_TEST_TARGETS=screend-targets \
    ///     <builder> cargo test --locked -- --include-ignored \
    ///     --skip serve_the_proxy_test_targets
    /// ```
    ///
    /// It serves until it is stopped.
    #[tokio::test]
    #[ignore = "the targets of the proxy tests, for a second machine; it serves until it is stopped"]
    async fn serve_the_proxy_test_targets() {
        echo_target(ECHO_PORT).await;
        http_target("first", FIRST_PORT).await;
        http_target("second", SECOND_PORT).await;
        eprintln!("the proxy test targets serve");
        std::future::pending::<()>().await;
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
    #[ignore = "needs serve_the_proxy_test_targets on a second machine, named in PAGIS_EXIT_TEST_TARGETS"]
    async fn a_connect_tunnel_carries_bytes_both_ways() {
        let (_proxy, address) = proxy().await;
        let target = SocketAddr::new(targets().await, ECHO_PORT);

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
    #[ignore = "needs serve_the_proxy_test_targets on a second machine, named in PAGIS_EXIT_TEST_TARGETS"]
    async fn an_absolute_form_request_reaches_its_host_also_two_hosts_on_one_connection() {
        let (proxy, address) = proxy().await;
        let targets = targets().await;
        let first = SocketAddr::new(targets, FIRST_PORT);
        let second = SocketAddr::new(targets, SECOND_PORT);
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
    #[ignore = "needs serve_the_proxy_test_targets on a second machine, named in PAGIS_EXIT_TEST_TARGETS"]
    async fn a_destination_that_refuses_is_a_bad_gateway() {
        let (_proxy, address) = proxy().await;
        let closed_port = SocketAddr::new(targets().await, CLOSED_PORT);

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

    /// A page can name a host that resolves to this Computer, and
    /// Chromium resolves no name it sends to a proxy, so the proxy itself
    /// refuses this Computer, by loopback or by any of its addresses, the
    /// unspecified addresses and the link-local addresses, with 403, and
    /// dials none of them.
    #[tokio::test]
    async fn a_destination_on_this_computer_or_link_local_is_forbidden_and_never_dialled() {
        let (_proxy, address) = proxy().await;
        let (port, taken) = counting_listener().await;
        let own = own_ip().to_string();

        for host in [
            "localhost",
            "127.0.0.1",
            "[::1]",
            "[::ffff:127.0.0.1]",
            "0.0.0.0",
            "169.254.169.254",
            own.as_str(),
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
            answer.contains(
                "the Exit Proxy opens no connection to this Computer itself \
                 (loopback or one of its addresses) or to a link-local address"
            ),
            "{answer}"
        );
    }

    /// A name that resolves to a refused address and to an allowed one
    /// reaches the allowed one, and the refused one is never dialled.
    #[tokio::test]
    #[ignore = "needs serve_the_proxy_test_targets on a second machine, named in PAGIS_EXIT_TEST_TARGETS"]
    async fn a_name_with_a_refused_and_an_allowed_address_reaches_the_allowed_one() {
        let (port, taken) = counting_listener().await;
        let refused = SocketAddr::new(own_ip(), port);
        let allowed = SocketAddr::new(targets().await, ECHO_PORT);

        let mut stream = connect_to_allowed(vec![refused, allowed])
            .await
            .expect("a connection");

        assert_eq!(stream.peer_addr().expect("a peer"), allowed);
        expect_bytes(&mut stream, b"hello from the target\n").await;
        assert_eq!(
            taken.load(Ordering::SeqCst),
            0,
            "the refused address was dialled"
        );
    }

    /// A name with no address is a failure of the gateway, not a refusal.
    #[tokio::test]
    async fn a_name_with_no_address_is_a_failure_and_not_a_refusal() {
        let error = connect_to_allowed(Vec::new())
            .await
            .expect_err("no connection");

        assert!(matches!(error, DialError::Failed(_)), "{error:?}");
    }

    /// The addresses that the proxy refuses, by the rule of production:
    /// this machine, by loopback or by its own address, the unspecified
    /// addresses and the link-local addresses. The addresses of other
    /// machines pass, private ones too. None of them is an address of a
    /// machine that runs these tests.
    #[test]
    fn the_proxy_refuses_this_computer_unspecified_and_link_local_alone() {
        let own = own_ip();
        let own_mapped = match own {
            IpAddr::V4(ip) => IpAddr::V6(ip.to_ipv6_mapped()),
            IpAddr::V6(ip) => IpAddr::V6(ip),
        };
        let refused = [
            "127.0.0.1",
            "127.1.2.3",
            "0.0.0.0",
            "0.1.2.3",
            "169.254.169.254",
            "::1",
            "::",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "fe80::1",
        ]
        .map(|ip| ip.parse::<IpAddr>().expect("an address"));
        for ip in refused.into_iter().chain([own, own_mapped]) {
            assert!(is_refused(ip), "{ip} is not refused");
        }
        for allowed in [
            "93.184.215.14",
            "10.255.255.254",
            "192.168.255.254",
            "100.127.255.254",
            "2606:2800:21f:cb07:6820:80da:af6b:8b2c",
            "fd00:ffff::1",
            "::ffff:10.255.255.254",
        ] {
            let ip: IpAddr = allowed.parse().expect("an address");
            assert!(!is_refused(ip), "{allowed} is refused");
        }
    }

    /// A switch closes every connection that the proxy holds, also when
    /// the mode stays the same, and answers how many it closed. The
    /// proxy goes on serving new connections.
    #[tokio::test]
    #[ignore = "needs serve_the_proxy_test_targets on a second machine, named in PAGIS_EXIT_TEST_TARGETS"]
    async fn a_switch_closes_every_connection_and_counts_them() {
        let (proxy, address) = proxy().await;
        let target = SocketAddr::new(targets().await, ECHO_PORT);
        let (mut first, _) = tunnel_to(address, &target.to_string()).await;
        let (mut second, _) = tunnel_to(address, &target.to_string()).await;
        expect_bytes(&mut first, b"hello from the target\n").await;
        expect_bytes(&mut second, b"hello from the target\n").await;
        // A connection that sent nothing yet.
        let mut idle = TcpStream::connect(address).await.expect("reach the proxy");
        wait_for_connections(&proxy, 3).await;

        let switched = proxy.switch(Mode::Direct).expect("the switch");

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
        let daemon = Daemon {
            address: "host.docker.internal:4403".to_string(),
            token: TOKEN.to_string(),
        };
        let proxy =
            ExitProxy::start(Some(daemon), policy_file(), Mode::Direct).expect("the proxy starts");

        assert_eq!(
            serde_json::to_value(proxy.status()).expect("JSON"),
            serde_json::json!({ "mode": "direct", "connections": 0 })
        );
        assert_eq!(
            serde_json::to_value(proxy.switch(Mode::Home).expect("the switch")).expect("JSON"),
            serde_json::json!({ "mode": "home", "closed": 0 })
        );
        assert_eq!(
            serde_json::to_value(proxy.status()).expect("JSON"),
            serde_json::json!({ "mode": "home", "connections": 0 })
        );
        for (body, mode) in [
            (r#"{"mode": "direct"}"#, Mode::Direct),
            (r#"{"mode": "home"}"#, Mode::Home),
        ] {
            let parsed: SwitchBody = serde_json::from_str(body).expect("a switch body");
            assert_eq!(parsed.mode, mode);
        }
        assert!(serde_json::from_str::<SwitchBody>(r#"{"mode": "elsewhere"}"#).is_err());
    }

    /// The daemon names the first mode in the container environment,
    /// and the exit listener beside it. A Computer of a Local Installation
    /// names neither.
    #[test]
    fn the_first_mode_and_the_daemon_come_from_the_environment() {
        for (value, mode) in [
            (None, Mode::Direct),
            (Some(""), Mode::Direct),
            (Some("direct"), Mode::Direct),
            (Some("home"), Mode::Home),
        ] {
            assert_eq!(Mode::from_variable(value), Ok(mode), "{value:?}");
        }
        let error = Mode::from_variable(Some("elsewhere")).expect_err("an unknown mode");
        assert!(error.contains(MODE_VARIABLE), "{error}");

        let daemon = Daemon::from_variable(Some("host.docker.internal:4403"), Some(TOKEN))
            .expect("a daemon");
        assert_eq!(daemon.address, "host.docker.internal:4403");
        assert_eq!(daemon.token, TOKEN);
        for (address, token) in [
            (None, Some(TOKEN)),
            (Some(" "), Some(TOKEN)),
            (Some("host.docker.internal:4403"), None),
            (Some("host.docker.internal:4403"), Some("")),
        ] {
            assert!(
                Daemon::from_variable(address, token).is_none(),
                "{address:?} {token:?}"
            );
        }
    }

    /// Each mode writes its policy file: `Home` turns off WebRTC's UDP
    /// past the proxy, and `Direct` empties the file.
    #[test]
    fn each_mode_writes_its_policy_file() {
        let policy = policy_file();
        let daemon = Daemon {
            address: "host.docker.internal:4403".to_string(),
            token: TOKEN.to_string(),
        };
        let home = serde_json::json!({ "WebRtcIPHandling": "disable_non_proxied_udp" });
        let read = || -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(&policy).expect("the policy file"))
                .expect("the policy file is JSON")
        };

        let proxy = ExitProxy::start(Some(daemon.clone()), policy.clone(), Mode::Home)
            .expect("the proxy starts");
        assert_eq!(read(), home);
        proxy.switch(Mode::Direct).expect("the switch");
        assert_eq!(read(), serde_json::json!({}));
        proxy.switch(Mode::Home).expect("the switch");
        assert_eq!(read(), home);

        let direct =
            ExitProxy::start(Some(daemon), policy.clone(), Mode::Direct).expect("the proxy starts");
        assert_eq!(read(), serde_json::json!({}));
        assert_eq!(direct.status().mode, Mode::Direct);
    }

    /// A switch that cannot happen changes nothing: `Home` with no daemon,
    /// and a policy file that is not there. screend never makes the file,
    /// so it adds no file to Chromium's policy directory.
    #[test]
    fn a_switch_that_cannot_happen_changes_nothing() {
        let policy = policy_file();
        let proxy = ExitProxy::start(None, policy.clone(), Mode::Direct).expect("the proxy starts");

        assert!(matches!(
            proxy.switch(Mode::Home),
            Err(SwitchError::NoDaemon)
        ));
        assert_eq!(proxy.status().mode, Mode::Direct);
        assert_eq!(
            std::fs::read_to_string(&policy).expect("the policy file"),
            "{}\n"
        );

        let daemon = Daemon {
            address: "host.docker.internal:4403".to_string(),
            token: TOKEN.to_string(),
        };
        let proxy = ExitProxy::start(Some(daemon.clone()), policy.clone(), Mode::Direct)
            .expect("the proxy starts");
        std::fs::remove_file(&policy).expect("remove the policy file");
        assert!(matches!(
            proxy.switch(Mode::Home),
            Err(SwitchError::Policy(_))
        ));
        assert_eq!(proxy.status().mode, Mode::Direct);
        assert!(!policy.exists(), "screend made the policy file");
        assert!(matches!(
            ExitProxy::start(Some(daemon), policy.clone(), Mode::Home),
            Err(SwitchError::Policy(_))
        ));
        assert!(!policy.exists(), "screend made the policy file");
    }

    /// In `Home` mode every name goes to the daemon, and so does every
    /// literal public address, IPv6 in brackets. The `CONNECT` names the
    /// destination and carries the Computer's token, and on a 200 the
    /// tunnel carries the bytes.
    #[tokio::test]
    async fn home_mode_sends_every_name_and_public_address_to_the_daemon() {
        let (daemon, heads) = fake_daemon("HTTP/1.1 200 OK\r\n\r\n").await;
        let (_proxy, address) = home_proxy(&daemon).await;
        let targets = [
            "example.com:443",
            "93.184.215.14:443",
            "[2606:2800:21f:cb07:6820:80da:af6b:8b2c]:443",
        ];

        for target in targets {
            let (mut stream, head) = tunnel_to(address, target).await;
            assert!(head.starts_with("HTTP/1.1 200"), "{target}: {head}");
            expect_bytes(&mut stream, b"hello from the daemon\n").await;
            stream.write_all(b"ping\n").await.expect("send");
            expect_bytes(&mut stream, b"ping\n").await;
        }

        let heads = heads.lock().expect("the heads").clone();
        assert_eq!(heads.len(), targets.len(), "{heads:?}");
        for (head, target) in heads.iter().zip(targets) {
            assert!(
                head.starts_with(&format!("CONNECT {target} HTTP/1.1\r\n")),
                "{head}"
            );
            assert!(
                head.contains(&format!("\r\nProxy-Authorization: Bearer {TOKEN}\r\n")),
                "{head}"
            );
        }
    }

    /// A plain `http://` request in `Home` mode reaches its host through
    /// the daemon too: the forward opens its connection with the same
    /// dial.
    #[tokio::test]
    async fn home_mode_forwards_an_absolute_form_request_through_the_daemon() {
        let (daemon, heads) = fake_http_daemon().await;
        let (_proxy, address) = home_proxy(&daemon).await;
        let stream = TcpStream::connect(address).await.expect("reach the proxy");
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .expect("an HTTP connection to the proxy");
        tokio::spawn(connection);

        let response = tokio::time::timeout(
            WAIT,
            sender.send_request(
                Request::get("http://shop.example/cart?x=1")
                    .body(Empty::<Bytes>::new())
                    .expect("a request"),
            ),
        )
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
        assert_eq!(body, "/cart?x=1 shop.example");
        let heads = heads.lock().expect("the heads").clone();
        assert!(
            heads[0].starts_with("CONNECT shop.example:80 HTTP/1.1\r\n"),
            "{heads:?}"
        );
    }

    /// The refusal of this Computer, loopback and link-local holds in
    /// `Home` mode too, before anything reaches the daemon.
    #[tokio::test]
    async fn home_mode_refuses_this_computer_and_link_local_before_the_daemon() {
        let (daemon, heads) = fake_daemon("HTTP/1.1 200 OK\r\n\r\n").await;
        let (_proxy, address) = home_proxy(&daemon).await;
        let own = own_ip().to_string();

        for host in [
            "127.0.0.1",
            "[::1]",
            "[::ffff:127.0.0.1]",
            "0.0.0.0",
            "169.254.169.254",
            own.as_str(),
        ] {
            let target = format!("{host}:443");
            let (_stream, head) = tunnel_to(address, &target).await;
            assert!(
                head.starts_with("HTTP/1.1 403"),
                "CONNECT {target} got {head}"
            );
        }
        assert!(
            heads.lock().expect("the heads").is_empty(),
            "the daemon was asked"
        );
    }

    /// A 403 of the daemon is the client's 403, with the daemon's reason.
    /// Every other answer is a failure of the gateway: a 407 means the
    /// daemon knows no Computer by this token. A daemon that does not
    /// answer is a failure too.
    #[tokio::test]
    async fn the_answer_of_the_daemon_is_the_answer_of_the_proxy() {
        let reason = "the Home Exit refused every address";
        let refusing: &'static str = Box::leak(
            format!(
                "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\n\r\n{reason}",
                reason.len()
            )
            .into_boxed_str(),
        );
        for (answer, status) in [
            (refusing, "403"),
            (
                "HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n",
                "502",
            ),
            (
                "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n",
                "502",
            ),
        ] {
            let (daemon, _heads) = fake_daemon(answer).await;
            let (_proxy, address) = home_proxy(&daemon).await;
            let mut stream = TcpStream::connect(address).await.expect("reach the proxy");
            stream
                .write_all(
                    b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("send the CONNECT");
            let mut whole = String::new();
            tokio::time::timeout(WAIT, stream.read_to_string(&mut whole))
                .await
                .expect("the answer arrives in time")
                .expect("the answer is readable");
            assert!(
                whole.starts_with(&format!("HTTP/1.1 {status}")),
                "{answer:?} gave {whole}"
            );
            if status == "403" {
                assert!(whole.contains(reason), "{whole}");
            }
        }

        let closed = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
        let nobody = closed.local_addr().expect("an address").to_string();
        drop(closed);
        let (_proxy, address) = home_proxy(&nobody).await;
        let (_stream, head) = tunnel_to(address, "example.com:443").await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    }

    /// In `Home` mode a literal private address leaves from the Computer,
    /// as in `Direct` mode, so the egress rules hold it: such an address
    /// is on the server's network and never at the Person's home.
    #[tokio::test]
    #[ignore = "needs serve_the_proxy_test_targets on a second machine, named in PAGIS_EXIT_TEST_TARGETS"]
    async fn home_mode_opens_a_literal_private_address_from_the_computer() {
        let (daemon, heads) = fake_daemon("HTTP/1.1 200 OK\r\n\r\n").await;
        let (_proxy, address) = home_proxy(&daemon).await;
        let targets = targets().await;
        assert!(
            is_private(targets),
            "the targets are at {targets}, which is not private"
        );
        let target = SocketAddr::new(targets, ECHO_PORT);

        let (mut stream, head) = tunnel_to(address, &target.to_string()).await;

        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        expect_bytes(&mut stream, b"hello from the target\n").await;
        assert!(
            heads.lock().expect("the heads").is_empty(),
            "the daemon was asked"
        );
    }

    /// The private addresses of `Home` mode: RFC 1918, carrier-grade NAT
    /// and unique local IPv6, by the rule of production.
    #[test]
    fn the_private_addresses_of_home_mode() {
        for private in [
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.1",
            "100.64.0.1",
            "100.127.255.254",
            "fd00::1",
            "fc00::1",
            "::ffff:10.1.2.3",
        ] {
            let ip: IpAddr = private.parse().expect("an address");
            assert!(is_private(ip), "{private} is not private");
        }
        for other in [
            "93.184.215.14",
            "172.32.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "2606:2800:21f:cb07:6820:80da:af6b:8b2c",
            "fe80::1",
        ] {
            let ip: IpAddr = other.parse().expect("an address");
            assert!(!is_private(ip), "{other} is private");
        }
    }
}
