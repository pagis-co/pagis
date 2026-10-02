//! The Home Exit of each Person (ADR-0029): the exit sockets of the
//! Hosts, and one stream on them for each connection of a Computer.
//!
//! A Client App that is connected to a Server opens its exit socket, a
//! second WebSocket beside its Host socket, with the same Session. The
//! socket carries one byte stream, and yamux runs over it. The daemon
//! opens one stream for each connection and the Client App accepts it.
//! Each stream starts with the preamble of the daemon, the destination
//! as `host:port` and a line feed, and the Client App answers with one
//! status line before any other byte: `ok`, `refused <reason>` or
//! `failed <reason>`. After `ok` the stream carries the raw bytes of the
//! connection both ways. The Client App resolves the name, refuses the
//! addresses of the home network, and dials. [`preamble`] and [`Status`]
//! hold the two lines.
//!
//! [`HomeExits`] holds the open exit sockets by Host, each with the
//! Workspace of the Session that opened it. A socket carries the
//! connections of that Workspace's Computers alone, so no Computer leaves
//! through the machine of another Person, whatever the store names. The
//! life of the socket is the presence of the Home Exit, in memory, as
//! the life of the Host socket is the presence of a Host (ADR-0015).
//!
//! When the socket closes, every stream on it ends, so each connection
//! that the Home Exit carried closes, and the next connection of the
//! Person's Computers leaves from the server.

use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use pagis_core::{HostId, WorkspaceId, WorkspaceStore};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};

/// The longest preamble, with its line feed: a DNS name of 253 bytes and
/// a port, or an IPv6 address in brackets and a port.
pub const PREAMBLE_LIMIT: usize = 262;

/// The longest status line, with its line feed.
pub const STATUS_LIMIT: usize = 512;

/// How long the daemon waits for a stream and its status line. The Home
/// Exit resolves the name and connects before it answers, so the answer
/// can take a connect timeout of that machine.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// How many streams one exit socket holds at once. Chromium holds at
/// most 32 connections to one proxy, and a Person has a few Computers
/// awake.
const MAX_STREAMS: usize = 256;

/// How many requests for a stream wait for the driver of one socket.
const OPEN_QUEUE: usize = 64;

/// The first line of a stream: the destination, as the target of a
/// `CONNECT` names it, and a line feed.
pub fn preamble(host: &str, port: u16) -> String {
    format!("{}\n", authority(host, port))
}

/// `host:port`, with an IPv6 address in brackets.
pub fn authority(host: &str, port: u16) -> String {
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(_)) => format!("[{host}]:{port}"),
        _ => format!("{host}:{port}"),
    }
}

/// The destination of one preamble line, without its line feed. A host
/// in brackets is an IPv6 address, and the brackets go.
pub fn parse_preamble(line: &str) -> Option<(String, u16)> {
    let (host, port) = line.rsplit_once(':')?;
    let port = port.parse().ok()?;
    let host = match host.strip_prefix('[') {
        Some(bracketed) => {
            let address = bracketed.strip_suffix(']')?;
            address.parse::<std::net::Ipv6Addr>().ok()?;
            address
        }
        None if host.contains(':') => return None,
        None => host,
    };
    (!host.is_empty()).then(|| (host.to_string(), port))
}

/// The status line of the Home Exit for one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The connection is open, and the raw bytes follow.
    Ok,
    /// The address check of the Home Exit refused every address of the
    /// destination.
    Refused(String),
    /// The preamble was malformed, the name did not resolve, or the
    /// connection failed.
    Failed(String),
}

impl Status {
    /// The line, with its line feed. A reason loses its line breaks and
    /// is cut to fit the limit.
    pub fn line(&self) -> String {
        let (word, reason) = match self {
            Status::Ok => return "ok\n".to_string(),
            Status::Refused(reason) => ("refused", reason),
            Status::Failed(reason) => ("failed", reason),
        };
        let mut reason: String = reason
            .chars()
            .map(|char| if char.is_control() { ' ' } else { char })
            .collect();
        while word.len() + 2 + reason.len() > STATUS_LIMIT {
            reason.pop();
        }
        format!("{word} {reason}\n")
    }

    /// The status of one line, without its line feed.
    pub fn parse(line: &str) -> Option<Self> {
        let (word, reason) = line.split_once(' ').unwrap_or((line, ""));
        match word {
            "ok" if reason.is_empty() => Some(Status::Ok),
            "refused" => Some(Status::Refused(reason.to_string())),
            "failed" => Some(Status::Failed(reason.to_string())),
            _ => None,
        }
    }
}

/// The yamux configuration of both ends of an exit socket. The receive
/// window of one connection grows with its round trip and its bandwidth,
/// and this bounds the sum of all windows to a window of 256 KiB for
/// each stream.
pub fn yamux_config() -> yamux::Config {
    let mut config = yamux::Config::default();
    config.set_max_num_streams(MAX_STREAMS);
    config.set_max_connection_receive_window(Some(MAX_STREAMS * yamux::DEFAULT_CREDIT as usize));
    config
}

/// Why the Home Exit carried no connection, when it is present.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExitError {
    /// The Home Exit refused every address of the destination.
    #[error("the Home Exit refused the destination: {0}")]
    Refused(String),
    /// No stream opened, or the Home Exit failed to connect, or gave no
    /// answer in time.
    #[error("the Home Exit did not connect: {0}")]
    Failed(String),
}

/// The bytes that one Person's Home Exit carried, in the memory of the
/// running daemon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExitBytes {
    /// From the Computers to the sites.
    pub sent: u64,
    /// From the sites to the Computers.
    pub received: u64,
}

#[derive(Debug, Default)]
struct Counter {
    sent: AtomicU64,
    received: AtomicU64,
}

/// A request for one stream, which the driver of the socket answers.
type Opening = oneshot::Sender<Result<yamux::Stream, yamux::ConnectionError>>;

/// One open exit socket.
struct Socket {
    /// The Workspace of the Session that opened the socket.
    workspace_id: WorkspaceId,
    /// Which registration this is, so a socket that a newer one of the
    /// same Host replaced does not take the newer one with it.
    epoch: u64,
    opens: mpsc::Sender<Opening>,
}

/// The open exit sockets of the Hosts, and the bytes that each Person's
/// Home Exit carried.
pub struct HomeExits {
    /// The Workspaces, which name each Person's Home Exit.
    workspaces: Arc<dyn WorkspaceStore>,
    sockets: Mutex<HashMap<HostId, Socket>>,
    next_epoch: AtomicU64,
    counters: Mutex<HashMap<WorkspaceId, Arc<Counter>>>,
}

impl HomeExits {
    pub fn new(workspaces: Arc<dyn WorkspaceStore>) -> Arc<Self> {
        Arc::new(Self {
            workspaces,
            sockets: Mutex::new(HashMap::new()),
            next_epoch: AtomicU64::new(0),
            counters: Mutex::new(HashMap::new()),
        })
    }

    /// Serve the exit socket of `host_id`, which a Session of
    /// `workspace_id` opened, until it ends. The Host is present as a Home
    /// Exit while this runs, and absent from the moment it returns or is
    /// dropped. A second socket of the same Host replaces the first for
    /// new streams.
    pub async fn serve<T>(&self, workspace_id: WorkspaceId, host_id: HostId, socket: T)
    where
        T: futures::AsyncRead + futures::AsyncWrite + Unpin + Send + 'static,
    {
        let (opens, mut requests) = mpsc::channel::<Opening>(OPEN_QUEUE);
        let epoch = self.next_epoch.fetch_add(1, Ordering::SeqCst);
        self.sockets.lock().expect("the exit sockets").insert(
            host_id.clone(),
            Socket {
                workspace_id,
                epoch,
                opens,
            },
        );
        let _registered = Registered {
            exits: self,
            host_id,
            epoch,
        };
        let mut connection = yamux::Connection::new(socket, yamux_config(), yamux::Mode::Client);
        let mut waiting: Option<Opening> = None;
        let mut requests_open = true;
        let ended = futures::future::poll_fn(|cx: &mut Context<'_>| {
            loop {
                if waiting.is_none() && requests_open {
                    match requests.poll_recv(cx) {
                        Poll::Ready(Some(opening)) => waiting = Some(opening),
                        Poll::Ready(None) => requests_open = false,
                        Poll::Pending => {}
                    }
                }
                if let Some(opening) = waiting.take() {
                    match connection.poll_new_outbound(cx) {
                        Poll::Ready(opened) => {
                            let _ = opening.send(opened);
                            continue;
                        }
                        Poll::Pending => waiting = Some(opening),
                    }
                }
                // This drives every stream of the socket, both ways.
                return match connection.poll_next_inbound(cx) {
                    // The Client App opens no stream; dropping one resets
                    // it.
                    Poll::Ready(Some(Ok(_inbound))) => continue,
                    Poll::Ready(Some(Err(error))) => Poll::Ready(Err(error)),
                    Poll::Ready(None) => Poll::Ready(Ok(())),
                    Poll::Pending => Poll::Pending,
                };
            }
        })
        .await;
        if let Err(error) = ended {
            tracing::debug!(%error, "an exit socket ended");
        }
    }

    /// Whether the exit socket of `host_id` is open.
    pub fn is_open(&self, host_id: &HostId) -> bool {
        self.sockets
            .lock()
            .expect("the exit sockets")
            .contains_key(host_id)
    }

    /// Open one connection to `host:port` through the Home Exit of the
    /// Person of `workspace_id`. `None` when that Person has chosen no
    /// Home Exit, or when it is absent: the connection then leaves from
    /// the server. Only an exit socket that a Session of `workspace_id`
    /// opened carries it, so the Host of another Person carries nothing
    /// for this Workspace, even where the store names it.
    pub async fn open(
        &self,
        workspace_id: &WorkspaceId,
        host: &str,
        port: u16,
    ) -> Option<Result<ExitStream, ExitError>> {
        let host_id = match self.workspaces.get(workspace_id).await {
            Ok(workspace) => workspace?.home_exit_host_id?,
            Err(error) => {
                tracing::warn!(
                    %error,
                    %workspace_id,
                    "the Home Exit was not read; the connection leaves from the server"
                );
                return None;
            }
        };
        let opens = {
            let sockets = self.sockets.lock().expect("the exit sockets");
            let socket = sockets.get(&host_id)?;
            if socket.workspace_id != *workspace_id {
                tracing::warn!(
                    %workspace_id,
                    %host_id,
                    "the Home Exit of the Workspace is a Host of another Person; \
                     the connection leaves from the server"
                );
                return None;
            }
            socket.opens.clone()
        };
        let (opening, opened) = oneshot::channel();
        // A socket that closed since the read is an absent Home Exit.
        opens.send(opening).await.ok()?;
        let answer = async {
            let stream = match opened.await {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => {
                    return Some(Err(ExitError::Failed(format!(
                        "the exit socket opened no stream: {error}"
                    ))));
                }
                Err(_) => return None,
            };
            let mut stream = stream.compat();
            Some(
                handshake(&mut stream, host, port)
                    .await
                    .map(|()| ExitStream {
                        inner: stream,
                        counter: self.counter(workspace_id),
                    }),
            )
        };
        match tokio::time::timeout(ANSWER_TIMEOUT, answer).await {
            Ok(answer) => answer,
            Err(_) => Some(Err(ExitError::Failed(format!(
                "no answer for {} in {} seconds",
                authority(host, port),
                ANSWER_TIMEOUT.as_secs()
            )))),
        }
    }

    /// The bytes that the Home Exit of the Person of `workspace_id`
    /// carried since the daemon started.
    pub fn bytes(&self, workspace_id: &WorkspaceId) -> ExitBytes {
        self.counters
            .lock()
            .expect("the exit counters")
            .get(workspace_id)
            .map(|counter| ExitBytes {
                sent: counter.sent.load(Ordering::SeqCst),
                received: counter.received.load(Ordering::SeqCst),
            })
            .unwrap_or_default()
    }

    fn counter(&self, workspace_id: &WorkspaceId) -> Arc<Counter> {
        Arc::clone(
            self.counters
                .lock()
                .expect("the exit counters")
                .entry(workspace_id.clone())
                .or_default(),
        )
    }
}

/// One registration of an exit socket. Dropping it makes the Host absent
/// as a Home Exit, whatever ended the socket, unless a newer socket of the
/// same Host replaced it.
struct Registered<'a> {
    exits: &'a HomeExits,
    host_id: HostId,
    epoch: u64,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        let mut sockets = self.exits.sockets.lock().expect("the exit sockets");
        if sockets
            .get(&self.host_id)
            .is_some_and(|socket| socket.epoch == self.epoch)
        {
            sockets.remove(&self.host_id);
        }
    }
}

/// Write the preamble and read the status line.
async fn handshake(
    stream: &mut Compat<yamux::Stream>,
    host: &str,
    port: u16,
) -> Result<(), ExitError> {
    let failed = |error: io::Error| ExitError::Failed(format!("the stream broke: {error}"));
    stream
        .write_all(preamble(host, port).as_bytes())
        .await
        .map_err(failed)?;
    stream.flush().await.map_err(failed)?;
    let line = read_line(stream, STATUS_LIMIT).await.map_err(failed)?;
    match Status::parse(&line) {
        Some(Status::Ok) => Ok(()),
        Some(Status::Refused(reason)) => Err(ExitError::Refused(reason)),
        Some(Status::Failed(reason)) => Err(ExitError::Failed(reason)),
        None => Err(ExitError::Failed(format!(
            "the Home Exit answered {line:?}, which is no status"
        ))),
    }
}

/// Read one line of at most `limit` bytes with its line feed, byte by
/// byte, so the bytes after it stay in the stream. The line comes
/// without its line feed.
pub async fn read_line<S: AsyncRead + Unpin>(stream: &mut S, limit: usize) -> io::Result<String> {
    let mut line = Vec::new();
    loop {
        let byte = stream.read_u8().await?;
        if byte == b'\n' {
            return String::from_utf8(line).map_err(|_| io::Error::other("the line is not text"));
        }
        if line.len() + 1 == limit {
            return Err(io::Error::other("the line is too long"));
        }
        line.push(byte);
    }
}

/// One connection that the Home Exit carries: a stream of the exit socket
/// past its status line. It counts the bytes for its Person.
pub struct ExitStream {
    inner: Compat<yamux::Stream>,
    counter: Arc<Counter>,
}

impl AsyncRead for ExitStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let read = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = read {
            let count = (buf.filled().len() - before) as u64;
            self.counter.received.fetch_add(count, Ordering::SeqCst);
        }
        read
    }
}

impl AsyncWrite for ExitStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let written = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(count)) = written {
            self.counter.sent.fetch_add(count as u64, Ordering::SeqCst);
        }
        written
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    /// The end of one direction: the stream sends its FIN, and the Home
    /// Exit ends the write side of its connection.
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use pagis_core::{HostId, WorkspaceId, WorkspaceStore};

    use super::*;
    use crate::fake::{FakeHomeExit, FakeWorkspaces, exit_socket_pair};

    const WAIT: Duration = Duration::from_secs(5);

    /// The preamble names the destination as a `CONNECT` names it, and
    /// the Home Exit reads back the same host and port.
    #[test]
    fn a_preamble_names_the_destination_as_a_connect_does() {
        for (host, port, line) in [
            ("example.com", 443, "example.com:443\n"),
            ("93.184.215.14", 80, "93.184.215.14:80\n"),
            ("2001:db8::1", 8443, "[2001:db8::1]:8443\n"),
        ] {
            assert_eq!(preamble(host, port), line);
            assert_eq!(
                parse_preamble(line.trim_end()),
                Some((host.to_string(), port))
            );
        }
        for malformed in [
            "example.com",
            "example.com:",
            "example.com:https",
            "example.com:70000",
            ":443",
            "2001:db8::1:443",
            "[example.com]:443",
            "[2001:db8::1:443",
        ] {
            assert_eq!(parse_preamble(malformed), None, "{malformed}");
        }
    }

    /// The status line is one of three words, with a reason after a
    /// refusal and a failure. A reason holds no line break, and the line
    /// fits its limit.
    #[test]
    fn a_status_line_is_ok_refused_or_failed() {
        for status in [
            Status::Ok,
            Status::Refused("every address is on the home network".to_string()),
            Status::Failed("connection refused".to_string()),
        ] {
            let line = status.line();
            assert!(line.ends_with('\n'), "{line:?}");
            assert_eq!(Status::parse(line.trim_end()), Some(status));
        }
        assert_eq!(Status::Ok.line(), "ok\n");
        assert_eq!(
            Status::Failed("two\nlines".to_string()).line(),
            "failed two lines\n"
        );
        let long = Status::Refused("x".repeat(2 * STATUS_LIMIT)).line();
        assert_eq!(long.len(), STATUS_LIMIT);
        for other in ["", "OK", "ok then", "yes", "refusedx"] {
            assert_eq!(Status::parse(other), None, "{other:?}");
        }
    }

    /// The Host is present as a Home Exit while its exit socket lives. A
    /// second socket of the same Host replaces the first, and the end of
    /// the first does not take the second with it.
    #[tokio::test]
    async fn a_second_socket_replaces_the_first_and_outlives_its_end() {
        let workspace_id = WorkspaceId::generate();
        let host_id = HostId::generate();
        let workspaces = Arc::new(FakeWorkspaces::with_timezone(&workspace_id, "UTC"));
        workspaces
            .set_home_exit(&workspace_id, Some(&host_id))
            .await
            .expect("the Home Exit is written");
        let exits = HomeExits::new(workspaces);
        let target = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a port");
        let target_address = target.local_addr().expect("an address");

        let (first_daemon, first_client_app) = exit_socket_pair();
        let first = tokio::spawn({
            let exits = Arc::clone(&exits);
            let (workspace_id, host_id) = (workspace_id.clone(), host_id.clone());
            async move { exits.serve(workspace_id, host_id, first_daemon).await }
        });
        let first_exit = FakeHomeExit::to(target_address);
        tokio::spawn(Arc::clone(&first_exit).serve(first_client_app));
        wait_until(|| exits.is_open(&host_id)).await;

        let (second_daemon, second_client_app) = exit_socket_pair();
        tokio::spawn({
            let exits = Arc::clone(&exits);
            let (workspace_id, host_id) = (workspace_id.clone(), host_id.clone());
            async move { exits.serve(workspace_id, host_id, second_daemon).await }
        });
        let second_exit = FakeHomeExit::to(target_address);
        tokio::spawn(Arc::clone(&second_exit).serve(second_client_app));
        // New streams go to the second socket once it is registered.
        let deadline = tokio::time::Instant::now() + WAIT;
        while second_exit.destinations().is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the second socket took no stream"
            );
            let _ = exits.open(&workspace_id, "probe.example", 443).await;
        }

        first.abort();
        let _ = first.await;

        assert!(
            exits.is_open(&host_id),
            "the end of the first socket took the second"
        );
        let opened = exits.open(&workspace_id, "example.com", 443).await;
        assert!(
            matches!(opened, Some(Ok(_))),
            "{:?}",
            opened.map(|opened| opened.err())
        );
        assert_eq!(
            second_exit.destinations().last().map(String::as_str),
            Some("example.com:443")
        );
        assert!(
            !first_exit
                .destinations()
                .contains(&"example.com:443".to_string())
        );
    }

    /// An answer that is no status line is a failure of the Home Exit.
    #[tokio::test]
    async fn an_answer_that_is_no_status_line_is_a_failure() {
        let workspace_id = WorkspaceId::generate();
        let host_id = HostId::generate();
        let workspaces = Arc::new(FakeWorkspaces::with_timezone(&workspace_id, "UTC"));
        workspaces
            .set_home_exit(&workspace_id, Some(&host_id))
            .await
            .expect("the Home Exit is written");
        let exits = HomeExits::new(workspaces);
        let (daemon_end, client_app_end) = exit_socket_pair();
        tokio::spawn({
            let exits = Arc::clone(&exits);
            let (workspace_id, host_id) = (workspace_id.clone(), host_id.clone());
            async move { exits.serve(workspace_id, host_id, daemon_end).await }
        });
        // A Client App end that answers every stream with a line that is
        // no status.
        tokio::spawn(async move {
            let mut connection =
                yamux::Connection::new(client_app_end, yamux_config(), yamux::Mode::Server);
            while let Some(Ok(stream)) =
                futures::future::poll_fn(|cx| connection.poll_next_inbound(cx)).await
            {
                tokio::spawn(async move {
                    let mut stream = stream.compat();
                    let _ = read_line(&mut stream, PREAMBLE_LIMIT).await;
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\n").await;
                });
            }
        });
        wait_until(|| exits.is_open(&host_id)).await;

        let opened = exits.open(&workspace_id, "example.com", 443).await;

        match opened {
            Some(Err(ExitError::Failed(reason))) => {
                assert!(reason.contains("no status"), "{reason}")
            }
            other => panic!("{:?}", other.map(|opened| opened.err())),
        }
    }

    async fn wait_until(ready: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT;
        while !ready() {
            assert!(tokio::time::Instant::now() < deadline, "not in time");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
