//! The exit listener of the daemon (ADR-0029): where the Exit Proxy of a
//! Computer in `Home` mode sends each connection, on a Server alone.
//!
//! The Exit Proxy sends one `CONNECT host:port` for each connection,
//! with `Proxy-Authorization: Bearer <token>`, where the token is the
//! Computer's control token. The listener reads one head of bounded size
//! within a short time, and the token check is the first thing it does
//! with it: a token of no awake Agent's Computer gets 407 and the
//! connection closes. The token names the Computer, and with it the
//! Agent and the Person, whose Workspace names the Home Exit.
//!
//! A destination that is an address and not a public unicast address
//! gets 403: loopback, private, link-local, carrier-grade NAT,
//! multicast, unspecified, broadcast, reserved and the documentation
//! ranges. Then:
//!
//! - When the Person's Home Exit is present, the listener opens one
//!   stream to it ([`HomeExits::open`]) and answers by its status line:
//!   200, 403 for a destination that the Home Exit refused, or 502. The
//!   name resolves at the Person's home, where the Client App checks the
//!   addresses.
//! - When it is absent, the listener resolves the name on the server,
//!   refuses every address that is not public unicast with 403, dials
//!   from the server, and answers 200 or 502.
//!
//! After a 200 the listener copies the bytes both ways until either side
//! closes.
//!
//! The listener binds every interface of the server, because the address
//! at which a Computer reaches the Docker host is the Docker daemon's
//! choice. The token check stands in for a narrower bind, and the
//! firewall of the server keeps the port closed to the network.

use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use crate::ComputerOwner;
use crate::home_exit::{ExitError, HomeExits, authority, parse_preamble};

/// The longest head that the listener reads.
const HEAD_LIMIT: usize = 8 * 1024;

/// How long a Computer has to send its head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the listener waits for one address of a destination to take
/// a connection from the server.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Which Agent's Computer holds a token. The exit listener knows a
/// Computer by the token that its Exit Proxy sends.
pub trait ComputerTokens: Send + Sync {
    /// The awake Agent's Computer that holds `token`, or `None`. The
    /// Plugin Computer is never one: it runs in `Direct` mode alone.
    fn computer_of(&self, token: &str) -> Option<ComputerOwner>;
}

/// Compare two secrets in a time that does not depend on how many bytes
/// agree, so a caller cannot find a token one byte at a time. The length
/// is not the secret: the daemon mints every token at one length.
pub(crate) fn same_secret(given: &str, expected: &str) -> bool {
    let (given, expected) = (given.as_bytes(), expected.as_bytes());
    given.len() == expected.len()
        && given
            .iter()
            .zip(expected)
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
}

/// Whether `ip` is a public unicast address, which a connection of a
/// Computer may reach from the server. An IPv4-mapped IPv6 address is
/// checked as its IPv4 address. IPv6 passes only in the global unicast
/// block 2000::/3, without the documentation blocks, the IETF protocol
/// assignments (2001::/23, Teredo among them) and 6to4 (2002::/16), which
/// carries an IPv4 address that can be a private one.
pub fn is_public_unicast(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            let [first, second, third, _] = ip.octets();
            let refused = first == 0
                || first == 10
                || first == 127
                || (first == 100 && (64..128).contains(&second))
                || (first == 169 && second == 254)
                || (first == 172 && (16..32).contains(&second))
                || (first == 192 && second == 168)
                || (first == 192 && second == 0 && third == 0)
                || (first == 192 && second == 0 && third == 2)
                || (first == 198 && (18..20).contains(&second))
                || (first == 198 && second == 51 && third == 100)
                || (first == 203 && second == 0 && third == 113)
                // Multicast, the reserved block and the broadcast address.
                || first >= 224;
            !refused
        }
        IpAddr::V6(ip) => {
            let [first, second, ..] = ip.segments();
            (first & 0xe000) == 0x2000
                && !(first == 0x2001 && second < 0x0200)
                && !(first == 0x2001 && second == 0x0db8)
                && first != 0x2002
                && !(first == 0x3fff && second < 0x1000)
        }
    }
}

/// The exit listener: the token check, the address check, and the route
/// of each connection.
pub struct ExitListener {
    computers: Arc<dyn ComputerTokens>,
    home_exits: Arc<HomeExits>,
    /// Which addresses a connection may reach from the server: the
    /// public unicast addresses.
    reaches: fn(IpAddr) -> bool,
}

impl ExitListener {
    pub fn new(computers: Arc<dyn ComputerTokens>, home_exits: Arc<HomeExits>) -> Arc<Self> {
        Arc::new(Self {
            computers,
            home_exits,
            reaches: is_public_unicast,
        })
    }

    /// Take in each connection and carry it on a task of its own, until
    /// `stop`.
    pub async fn serve(self: Arc<Self>, listener: TcpListener, stop: CancellationToken) {
        loop {
            let accepted = tokio::select! {
                () = stop.cancelled() => return,
                accepted = listener.accept() => accepted,
            };
            match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(Arc::clone(&self).carry(stream));
                }
                Err(error) => {
                    // A full table of open files refuses the accept and
                    // not the listener, so the listener waits and accepts
                    // again.
                    tracing::warn!(%error, "the exit listener accepted no connection");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// Answer one `CONNECT`, and carry its bytes after a 200.
    async fn carry(self: Arc<Self>, mut client: TcpStream) {
        let _ = client.set_nodelay(true);
        let Ok(head) = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut client)).await else {
            return;
        };
        let head = head.unwrap_or_default();
        let mut headers = [httparse::EMPTY_HEADER; 16];
        let mut request = httparse::Request::new(&mut headers);
        let complete = matches!(request.parse(&head), Ok(httparse::Status::Complete(_)));
        let owner = complete
            .then(|| bearer(request.headers))
            .flatten()
            .and_then(|token| self.computers.computer_of(token));
        let Some(owner) = owner else {
            answer(
                &mut client,
                407,
                "the exit listener carries the connections of an Agent's Computer alone, \
                 and the request carries no token of one",
            )
            .await;
            return;
        };
        if request.method != Some("CONNECT") {
            answer(&mut client, 400, "the exit listener takes CONNECT alone").await;
            return;
        }
        let Some((host, port)) = request.path.and_then(parse_preamble) else {
            answer(&mut client, 400, "a CONNECT names a host and a port").await;
            return;
        };
        let target = authority(&host, port);
        if let Ok(ip) = host.parse::<IpAddr>()
            && !(self.reaches)(ip)
        {
            answer(
                &mut client,
                403,
                &format!("{target} is not a public address, and no exit carries it"),
            )
            .await;
            return;
        }
        match self.home_exits.open(&owner.workspace_id, &host, port).await {
            Some(Ok(mut upstream)) => splice(client, &mut upstream).await,
            Some(Err(ExitError::Refused(reason))) => {
                answer(
                    &mut client,
                    403,
                    &format!("the Home Exit refused {target}: {reason}"),
                )
                .await;
            }
            Some(Err(error)) => answer(&mut client, 502, &error.to_string()).await,
            None => match self.dial_from_server(&host, port).await {
                Ok(mut upstream) => splice(client, &mut upstream).await,
                Err(Dial::Refused) => {
                    answer(
                        &mut client,
                        403,
                        &format!(
                            "every address of {host} is one that is not public, \
                             and no exit carries it"
                        ),
                    )
                    .await;
                }
                Err(Dial::Failed(reason)) => {
                    answer(
                        &mut client,
                        502,
                        &format!("{target} did not answer the server: {reason}"),
                    )
                    .await;
                }
            },
        }
    }

    /// Resolve the name on the server, and connect from the server to the
    /// first address that it may reach and that takes the connection. The
    /// listener connects to the address that it checked, so a name cannot
    /// pass the check with one address and connect to another.
    async fn dial_from_server(&self, host: &str, port: u16) -> Result<TcpStream, Dial> {
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| Dial::Failed(error.to_string()))?;
        let mut refused = false;
        let mut failure = None;
        for address in addresses {
            if !(self.reaches)(address.ip()) {
                refused = true;
                continue;
            }
            match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address)).await {
                Ok(Ok(stream)) => {
                    let _ = stream.set_nodelay(true);
                    return Ok(stream);
                }
                Ok(Err(error)) => failure = Some(error.to_string()),
                Err(_) => failure = Some(format!("{address} took no connection in time")),
            }
        }
        Err(match failure {
            Some(failure) => Dial::Failed(failure),
            None if refused => Dial::Refused,
            None => Dial::Failed("the name has no address".to_string()),
        })
    }
}

/// Why the server opened no connection.
enum Dial {
    /// Every address of the name is one that the server does not reach.
    Refused,
    Failed(String),
}

/// The token of a `Proxy-Authorization: Bearer` header. The scheme name
/// is case-insensitive (RFC 9110).
fn bearer<'a>(headers: &[httparse::Header<'a>]) -> Option<&'a str> {
    let value = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("proxy-authorization"))?
        .value;
    let (scheme, token) = std::str::from_utf8(value).ok()?.trim().split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|token| !token.is_empty())
}

/// Read one head byte by byte, so the bytes after it stay in the stream.
/// A head past the limit is cut there, and fails to parse.
async fn read_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") && head.len() < HEAD_LIMIT {
        head.push(stream.read_u8().await?);
    }
    Ok(head)
}

/// One refusal with its reason, and the end of the connection.
async fn answer(stream: &mut TcpStream, status: u16, reason: &str) {
    let phrase = match status {
        400 => "Bad Request",
        403 => "Forbidden",
        407 => "Proxy Authentication Required",
        _ => "Bad Gateway",
    };
    let challenge = match status {
        407 => "Proxy-Authenticate: Bearer\r\n",
        _ => "",
    };
    let head = format!(
        "HTTP/1.1 {status} {phrase}\r\n{challenge}Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{reason}",
        reason.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Answer 200, then copy the bytes both ways until either side closes. A
/// reset from either end is how a tunnel ends, so the copy has no
/// failure to report.
async fn splice<U: AsyncRead + AsyncWrite + Unpin>(mut client: TcpStream, upstream: &mut U) {
    if client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, upstream).await;
}

/// The interface the listener binds on a Server: every interface (see
/// the module documentation).
pub const BIND_ADDRESS: Ipv4Addr = Ipv4Addr::UNSPECIFIED;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use pagis_core::{AgentId, HostId, WorkspaceId, WorkspaceStore};

    use super::*;
    use crate::fake::{FakeExitAnswer, FakeHomeExit, FakeWorkspaces, exit_socket_pair};

    /// How long a test waits for one answer.
    const WAIT: Duration = Duration::from_secs(5);

    /// More than the window of one yamux stream, so the bytes of a
    /// tunnel wait for window updates on the way.
    const BULK: usize = 600 * 1024;

    /// The awake Computers of the test, by token.
    struct Tokens(HashMap<String, ComputerOwner>);

    impl ComputerTokens for Tokens {
        fn computer_of(&self, token: &str) -> Option<ComputerOwner> {
            self.0
                .iter()
                .find(|(held, _)| same_secret(token, held))
                .map(|(_, owner)| owner.clone())
        }
    }

    /// Two People, each with one awake Agent's Computer, and the exit
    /// sockets of their Hosts.
    struct World {
        workspaces: Arc<FakeWorkspaces>,
        home_exits: Arc<HomeExits>,
        mine: Person,
        theirs: Person,
    }

    struct Person {
        workspace_id: WorkspaceId,
        token: String,
        host_id: HostId,
    }

    impl World {
        fn new() -> Self {
            let mine = Person {
                workspace_id: WorkspaceId::generate(),
                token: "my-computer-token".to_string(),
                host_id: HostId::generate(),
            };
            let theirs = Person {
                workspace_id: WorkspaceId::generate(),
                token: "their-computer-token".to_string(),
                host_id: HostId::generate(),
            };
            let workspaces = Arc::new(FakeWorkspaces::with_timezone(&mine.workspace_id, "UTC"));
            futures::executor::block_on(
                workspaces.create(&FakeWorkspaces::workspace(&theirs.workspace_id, "UTC")),
            )
            .expect("the second Workspace");
            Self {
                home_exits: HomeExits::new(Arc::clone(&workspaces) as _),
                workspaces,
                mine,
                theirs,
            }
        }

        fn tokens(&self) -> Arc<Tokens> {
            Arc::new(Tokens(HashMap::from([
                (
                    self.mine.token.clone(),
                    ComputerOwner::new(self.mine.workspace_id.clone(), AgentId::generate()),
                ),
                (
                    self.theirs.token.clone(),
                    ComputerOwner::new(self.theirs.workspace_id.clone(), AgentId::generate()),
                ),
            ])))
        }

        /// The exit listener of production on a free loopback port.
        async fn listener(&self) -> SocketAddr {
            self.listener_reaching(is_public_unicast).await
        }

        /// The exit listener, where the server reaches the addresses that
        /// `reaches` passes: a test reaches its targets on loopback.
        async fn listener_reaching(&self, reaches: fn(IpAddr) -> bool) -> SocketAddr {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
            let address = listener.local_addr().expect("an address");
            let exit = Arc::new(ExitListener {
                computers: self.tokens(),
                home_exits: Arc::clone(&self.home_exits),
                reaches,
            });
            tokio::spawn(exit.serve(listener, CancellationToken::new()));
            address
        }

        /// Name the Host of `person` as their Home Exit.
        async fn choose(&self, workspace_id: &WorkspaceId, host_id: &HostId) {
            assert!(
                self.workspaces
                    .set_home_exit(workspace_id, Some(host_id))
                    .await
                    .expect("the Home Exit is written")
            );
        }

        /// Open the exit socket of `person`'s Host, with `exit` at the
        /// Client App's end. Aborting the answer closes the socket from the
        /// Client App's side, as a Client App that goes away does.
        async fn open_exit(
            &self,
            person: &Person,
            exit: Arc<FakeHomeExit>,
        ) -> tokio::task::JoinHandle<()> {
            let (daemon_end, client_app_end) = exit_socket_pair();
            let home_exits = Arc::clone(&self.home_exits);
            let (workspace_id, host_id) = (person.workspace_id.clone(), person.host_id.clone());
            tokio::spawn(async move { home_exits.serve(workspace_id, host_id, daemon_end).await });
            let client_app = tokio::spawn(exit.serve(client_app_end));
            let deadline = tokio::time::Instant::now() + WAIT;
            while !self.home_exits.is_open(&person.host_id) {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the exit socket did not open"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            client_app
        }
    }

    /// Whether the address is on this machine's loopback, which a test
    /// lets the server reach.
    fn loopback_too(ip: IpAddr) -> bool {
        ip.to_canonical().is_loopback() || is_public_unicast(ip)
    }

    /// A TCP target on loopback that greets each connection, then sends
    /// back what it reads. It counts the connections that it took.
    async fn echo_target() -> (SocketAddr, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
        let address = listener.local_addr().expect("an address");
        let taken = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&taken);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    stream.write_all(b"hello from the target\n").await.ok();
                    let (mut reader, mut writer) = stream.split();
                    tokio::io::copy(&mut reader, &mut writer).await.ok();
                });
            }
        });
        (address, taken)
    }

    /// Send one raw request to the listener and read the head of its
    /// answer, byte by byte, so the bytes of a tunnel stay in the stream.
    async fn ask(listener: SocketAddr, raw: &str) -> (TcpStream, String) {
        let mut stream = TcpStream::connect(listener)
            .await
            .expect("reach the listener");
        stream.write_all(raw.as_bytes()).await.expect("send");
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            match tokio::time::timeout(WAIT, stream.read_u8()).await {
                Ok(Ok(byte)) => head.push(byte),
                Ok(Err(_)) => break,
                Err(_) => panic!("no answer in time: {}", String::from_utf8_lossy(&head)),
            }
        }
        (stream, String::from_utf8_lossy(&head).into_owned())
    }

    /// One `CONNECT` with a token, as the Exit Proxy sends it.
    async fn connect(listener: SocketAddr, token: &str, target: &str) -> (TcpStream, String) {
        ask(
            listener,
            &format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\
                 Proxy-Authorization: Bearer {token}\r\n\r\n"
            ),
        )
        .await
    }

    /// The rest of an answer after its head: the reason of a refusal.
    async fn rest(stream: &mut TcpStream) -> String {
        let mut rest = String::new();
        let _ = tokio::time::timeout(WAIT, stream.read_to_string(&mut rest)).await;
        rest
    }

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

    /// Send more than a window through a tunnel and read it back, at the
    /// same time, so neither direction waits for the other.
    async fn echo_bulk(stream: &mut TcpStream) {
        let sent: Vec<u8> = (0..BULK).map(|index| (index % 251) as u8).collect();
        let (mut reader, mut writer) = stream.split();
        let mut back = vec![0; BULK];
        let (written, read) = tokio::join!(writer.write_all(&sent), async {
            tokio::time::timeout(Duration::from_secs(20), reader.read_exact(&mut back)).await
        });
        written.expect("the bytes go out");
        read.expect("the bytes come back in time")
            .expect("the bytes are readable");
        assert!(back == sent, "the bytes came back changed");
    }

    /// Whether the peer closed `stream`.
    async fn closed(stream: &mut TcpStream) -> bool {
        let mut byte = [0; 1];
        matches!(
            tokio::time::timeout(WAIT, stream.read(&mut byte)).await,
            Ok(Ok(0) | Err(_))
        )
    }

    /// Only an awake Agent's Computer reaches the listener: a request
    /// with no token, with a token of no Computer, with another scheme,
    /// or with no head that parses gets 407, and nothing is dialled.
    #[tokio::test]
    async fn a_request_without_the_token_of_a_computer_gets_407() {
        let world = World::new();
        let (target, taken) = echo_target().await;
        world
            .choose(&world.mine.workspace_id, &world.mine.host_id)
            .await;
        let exit = FakeHomeExit::to(target);
        let _client_app = world.open_exit(&world.mine, Arc::clone(&exit)).await;
        let listener = world.listener_reaching(loopback_too).await;

        for raw in [
            "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com\r\n\r\n".to_string(),
            "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Bearer not-a-token\r\n\r\n"
                .to_string(),
            format!(
                "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Basic {}\r\n\r\n",
                world.mine.token
            ),
            format!(
                "CONNECT example.com:443 HTTP/1.1\r\nProxy-Authorization: Bearer {}x\r\n\r\n",
                world.mine.token
            ),
            "NOT HTTP AT ALL\r\n\r\n".to_string(),
            format!("CONNECT localhost:{} HTTP/1.1\r\n\r\n", target.port()),
        ] {
            let (mut stream, head) = ask(listener, &raw).await;
            assert!(head.starts_with("HTTP/1.1 407"), "{raw:?} got {head}");
            assert!(head.contains("Proxy-Authenticate: Bearer"), "{head}");
            assert!(rest(&mut stream).await.contains("carries no token"));
        }
        assert!(exit.destinations().is_empty(), "the Home Exit was asked");
        assert_eq!(taken.load(Ordering::SeqCst), 0, "the target was dialled");
    }

    /// The token comes first, then the request: a Computer that sends
    /// another method or no target gets 400.
    #[tokio::test]
    async fn a_request_that_is_no_connect_gets_400() {
        let world = World::new();
        let listener = world.listener().await;
        let token = &world.mine.token;

        for raw in [
            format!(
                "GET http://example.com/ HTTP/1.1\r\nProxy-Authorization: Bearer {token}\r\n\r\n"
            ),
            format!("CONNECT example.com HTTP/1.1\r\nProxy-Authorization: Bearer {token}\r\n\r\n"),
            format!(
                "CONNECT 2001:db8::1:443 HTTP/1.1\r\nProxy-Authorization: Bearer {token}\r\n\r\n"
            ),
        ] {
            let (_stream, head) = ask(listener, &raw).await;
            assert!(head.starts_with("HTTP/1.1 400"), "{raw:?} got {head}");
        }
    }

    /// A destination that is an address and not a public unicast address
    /// gets 403 before any exit carries it, also while the Home Exit is
    /// present: the address check of the Client App sees a name's
    /// addresses, and an address that comes as a literal never reaches it.
    #[tokio::test]
    async fn a_literal_destination_that_is_not_public_gets_403() {
        let world = World::new();
        let (target, taken) = echo_target().await;
        world
            .choose(&world.mine.workspace_id, &world.mine.host_id)
            .await;
        let exit = FakeHomeExit::to(target);
        let _client_app = world.open_exit(&world.mine, Arc::clone(&exit)).await;
        let listener = world.listener().await;

        for host in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "224.0.0.251",
            "0.0.0.0",
            "255.255.255.255",
            "240.0.0.1",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "[::1]",
            "[::]",
            "[fd00::1]",
            "[fe80::1]",
            "[ff02::1]",
            "[2001:db8::1]",
            "[::ffff:10.0.0.1]",
            "[::ffff:127.0.0.1]",
        ] {
            let destination = format!("{host}:443");
            let (mut stream, head) = connect(listener, &world.mine.token, &destination).await;
            assert!(head.starts_with("HTTP/1.1 403"), "{destination} got {head}");
            assert!(rest(&mut stream).await.contains("not a public address"));
        }
        assert!(exit.destinations().is_empty(), "the Home Exit was asked");
        assert_eq!(taken.load(Ordering::SeqCst), 0);
    }

    /// The public unicast addresses, by the rule of production.
    #[test]
    fn the_public_unicast_addresses() {
        for public in [
            "93.184.215.14",
            "1.1.1.1",
            "172.32.0.1",
            "100.128.0.1",
            "198.20.0.1",
            "223.255.255.254",
            "2606:2800:21f:cb07:6820:80da:af6b:8b2c",
            "2a00:1450:4001:82a::200e",
            "::ffff:93.184.215.14",
        ] {
            let ip: IpAddr = public.parse().expect("an address");
            assert!(is_public_unicast(ip), "{public} is refused");
        }
        for refused in [
            "0.1.2.3",
            "10.255.255.254",
            "100.127.255.254",
            "127.0.0.1",
            "169.254.0.1",
            "172.31.255.254",
            "192.0.0.8",
            "192.0.2.10",
            "192.168.0.1",
            "198.18.0.1",
            "198.19.255.254",
            "198.51.100.10",
            "203.0.113.10",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:192.168.0.1",
            "64:ff9b::a00:1",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::fb",
            "2001::1",
            "2001:db8::1",
            "2002:a00:1::1",
            "3fff::1",
        ] {
            let ip: IpAddr = refused.parse().expect("an address");
            assert!(!is_public_unicast(ip), "{refused} passes");
        }
    }

    /// The Person's Home Exit carries the connection: the stream names
    /// the destination, and the bytes go both ways, more than a window of
    /// them. The bytes count for the Person.
    #[tokio::test]
    async fn a_present_home_exit_carries_the_connection() {
        let world = World::new();
        let (target, _taken) = echo_target().await;
        world
            .choose(&world.mine.workspace_id, &world.mine.host_id)
            .await;
        let exit = FakeHomeExit::to(target);
        let _client_app = world.open_exit(&world.mine, Arc::clone(&exit)).await;
        let listener = world.listener().await;

        let (mut stream, head) = connect(listener, &world.mine.token, "shop.example.com:443").await;

        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        expect_bytes(&mut stream, b"hello from the target\n").await;
        echo_bulk(&mut stream).await;
        assert_eq!(exit.destinations(), ["shop.example.com:443"]);
        let bytes = world.home_exits.bytes(&world.mine.workspace_id);
        assert_eq!(bytes.sent, BULK as u64);
        assert_eq!(
            bytes.received,
            (BULK + b"hello from the target\n".len()) as u64
        );
        assert_eq!(
            world.home_exits.bytes(&world.theirs.workspace_id),
            Default::default()
        );
    }

    /// The status line of the Home Exit is the answer: a refusal of its
    /// address check is 403 with its reason, and a failure is 502.
    #[tokio::test]
    async fn the_status_of_the_home_exit_is_the_answer() {
        let world = World::new();
        world
            .choose(&world.mine.workspace_id, &world.mine.host_id)
            .await;
        let exit = FakeHomeExit::new(|host, _| match host {
            "router.home.example" => {
                FakeExitAnswer::Refuse("every address of the name is on the home network".into())
            }
            _ => FakeExitAnswer::Fail("the name does not resolve".into()),
        });
        let _client_app = world.open_exit(&world.mine, exit).await;
        let listener = world.listener().await;

        let (mut refused, head) =
            connect(listener, &world.mine.token, "router.home.example:80").await;
        assert!(head.starts_with("HTTP/1.1 403"), "{head}");
        assert!(rest(&mut refused).await.contains("on the home network"));

        let (mut failed, head) = connect(listener, &world.mine.token, "nowhere.example:443").await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        assert!(rest(&mut failed).await.contains("does not resolve"));
    }

    /// A Person with no Home Exit, and a Person whose Home Exit is
    /// absent, leave from the server: the server resolves the name and
    /// dials.
    #[tokio::test]
    async fn an_absent_home_exit_leaves_from_the_server() {
        let world = World::new();
        let (target, taken) = echo_target().await;
        let listener = world.listener_reaching(loopback_too).await;
        let destination = format!("localhost:{}", target.port());

        for chosen in [false, true] {
            if chosen {
                // The Home Exit is chosen, and its socket is not open.
                world
                    .choose(&world.mine.workspace_id, &world.mine.host_id)
                    .await;
            }
            let (mut stream, head) = connect(listener, &world.mine.token, &destination).await;
            assert!(head.starts_with("HTTP/1.1 200"), "{head}");
            expect_bytes(&mut stream, b"hello from the target\n").await;
            echo_bulk(&mut stream).await;
        }
        assert_eq!(taken.load(Ordering::SeqCst), 2);
        assert_eq!(
            world.home_exits.bytes(&world.mine.workspace_id),
            Default::default(),
            "the server path counted as the Home Exit"
        );
    }

    /// From the server, a name whose every address is not public gets
    /// 403, and none of them is dialled.
    #[tokio::test]
    async fn the_server_refuses_a_name_whose_every_address_is_not_public() {
        let world = World::new();
        let (target, taken) = echo_target().await;
        let listener = world.listener().await;

        let (mut stream, head) = connect(
            listener,
            &world.mine.token,
            &format!("localhost:{}", target.port()),
        )
        .await;

        assert!(head.starts_with("HTTP/1.1 403"), "{head}");
        assert!(
            rest(&mut stream)
                .await
                .contains("every address of localhost")
        );
        assert_eq!(
            taken.load(Ordering::SeqCst),
            0,
            "a refused address was dialled"
        );
    }

    /// When the Home Exit goes away, every connection that it carried
    /// closes, and the next connection leaves from the server.
    #[tokio::test]
    async fn when_the_home_exit_goes_its_connections_close_and_new_ones_leave_from_the_server() {
        let world = World::new();
        let (home_target, home_taken) = echo_target().await;
        let (server_target, server_taken) = echo_target().await;
        world
            .choose(&world.mine.workspace_id, &world.mine.host_id)
            .await;
        let exit = FakeHomeExit::to(home_target);
        let client_app = world.open_exit(&world.mine, Arc::clone(&exit)).await;
        let listener = world.listener_reaching(loopback_too).await;
        let destination = format!("localhost:{}", server_target.port());

        let (mut first, head) = connect(listener, &world.mine.token, &destination).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        expect_bytes(&mut first, b"hello from the target\n").await;
        let (mut second, _) = connect(listener, &world.mine.token, &destination).await;
        expect_bytes(&mut second, b"hello from the target\n").await;
        assert_eq!(home_taken.load(Ordering::SeqCst), 2);
        assert_eq!(server_taken.load(Ordering::SeqCst), 0);

        client_app.abort();

        assert!(closed(&mut first).await, "a carried connection stays open");
        assert!(closed(&mut second).await, "a carried connection stays open");
        let deadline = tokio::time::Instant::now() + WAIT;
        while world.home_exits.is_open(&world.mine.host_id) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the Home Exit stays present"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let (mut after, head) = connect(listener, &world.mine.token, &destination).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        expect_bytes(&mut after, b"hello from the target\n").await;
        assert_eq!(server_taken.load(Ordering::SeqCst), 1);
        assert_eq!(exit.destinations().len(), 2);
    }

    /// Only a Host of the Computer's own Person carries its connections.
    /// The other Person's Host has an open exit socket and is their Home
    /// Exit, and it carries nothing for this Computer: not while this
    /// Person has no Home Exit, and not when the store names the other
    /// Person's Host as this Person's Home Exit. It still carries its own
    /// Person's connections.
    #[tokio::test]
    async fn the_host_of_another_person_carries_nothing_for_this_computer() {
        let world = World::new();
        let (home_target, home_taken) = echo_target().await;
        let (server_target, server_taken) = echo_target().await;
        world
            .choose(&world.theirs.workspace_id, &world.theirs.host_id)
            .await;
        let their_exit = FakeHomeExit::to(home_target);
        let _their_client_app = world
            .open_exit(&world.theirs, Arc::clone(&their_exit))
            .await;
        let listener = world.listener_reaching(loopback_too).await;
        let destination = format!("localhost:{}", server_target.port());

        for forged in [false, true] {
            if forged {
                // A store fault that names the other Person's Host. The
                // stores refuse it (the store suite proves it); the exit
                // socket of that Host belongs to another Workspace.
                world
                    .choose(&world.mine.workspace_id, &world.theirs.host_id)
                    .await;
            }
            let (mut stream, head) = connect(listener, &world.mine.token, &destination).await;
            assert!(head.starts_with("HTTP/1.1 200"), "{head}");
            expect_bytes(&mut stream, b"hello from the target\n").await;
        }
        assert!(
            their_exit.destinations().is_empty(),
            "another Person's Host carried this Computer's connection"
        );
        assert_eq!(home_taken.load(Ordering::SeqCst), 0);
        assert_eq!(server_taken.load(Ordering::SeqCst), 2);

        let (mut theirs, head) = connect(listener, &world.theirs.token, "example.org:443").await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        expect_bytes(&mut theirs, b"hello from the target\n").await;
        assert_eq!(their_exit.destinations(), ["example.org:443"]);
    }
}
