//! The session sockets of the Hosts (ADR-0033), and one stream on them
//! for each Coding Session.
//!
//! A Client App that can start a Coding Harness opens its session
//! socket, a further WebSocket beside its Host socket, with the same
//! Session. The socket carries one byte stream, and yamux runs over it.
//! The daemon opens one stream for each Coding Session and the Client App
//! accepts it. The first line on a stream is the daemon's
//! [`OpenRequest`], and the Client App answers with one [`OpenAnswer`]
//! line before any other byte. After an answer that opened the session,
//! the stream carries the raw stdin and stdout of the harness process.
//! When the process exits, the Client App closes the stream and sends a
//! `session_exit` frame on its Host socket, which reaches
//! [`HostSessions::exited`].
//!
//! [`HostSessions`] holds the open session sockets by Host, each with the
//! Workspace of the Session that opened it. A socket carries the Coding
//! Sessions of that Workspace alone, so no session of one Person starts on
//! the machine of another, whatever the caller names. The presence of the
//! socket is memory of the running daemon, never a column, as the
//! presence of a Host is (ADR-0015).

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt};
use pagis_core::{CodingSessionId, HostId, WorkspaceId};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

/// The longest open request, with its line feed.
pub const REQUEST_LIMIT: usize = 64 * 1024;

/// The longest open answer, with its line feed.
pub const ANSWER_LIMIT: usize = 8 * 1024;

/// The longest stderr tail of a [`SessionExit`].
pub const STDERR_TAIL_LIMIT: usize = 4 * 1024;

/// How long the daemon waits for a stream and its answer: the longest
/// host command of ADR-0015, because the Client App can make a git
/// worktree before it starts the process.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

/// How many streams one session socket holds at once. An Agent holds at
/// most four open Coding Sessions, and a Person has a few Agents.
const MAX_STREAMS: usize = 64;

/// How many requests for a stream wait for the driver of one socket.
const OPEN_QUEUE: usize = 64;

/// The daemon's first line on a new stream: what to start, and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenRequest {
    pub session_id: CodingSessionId,
    /// The program to start, which the Client App finds on the Person's
    /// login-shell `PATH`.
    pub command: String,
    pub args: Vec<String>,
    /// The directory that the Agent named.
    pub cwd: String,
    /// The environment that the process gets beside the Person's own. It
    /// never holds a secret.
    pub env: BTreeMap<String, String>,
    /// The git worktree that the Client App makes first, if any.
    pub worktree: Option<WorktreeRequest>,
}

impl OpenRequest {
    /// The line, with its line feed.
    pub fn line(&self) -> String {
        let json = serde_json::to_string(self).expect("an open request is JSON");
        format!("{json}\n")
    }
}

/// A git worktree of `repo` on `branch`, made from `base`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeRequest {
    pub repo: String,
    pub branch: String,
    pub base: String,
}

/// Why the Client App did not start the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenFailure {
    /// The command is not on the Person's `PATH`.
    NotFound,
    /// The directory does not exist or is not a directory.
    BadDirectory,
    /// The git worktree was not made.
    WorktreeFailed,
    /// The process did not start.
    SpawnFailed,
}

/// The Client App's one line for a stream: `{"ok": true, "cwd": ...}` or
/// `{"ok": false, "error": <code>, "message": ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AnswerLine", into = "AnswerLine")]
pub enum OpenAnswer {
    /// The process runs in `cwd`: the worktree path when the session has
    /// a worktree, else the directory of the request.
    Opened {
        cwd: String,
    },
    Refused {
        error: OpenFailure,
        message: String,
    },
}

impl OpenAnswer {
    /// The line, with its line feed.
    pub fn line(&self) -> String {
        let json = serde_json::to_string(self).expect("an open answer is JSON");
        format!("{json}\n")
    }

    /// The answer of one line, without its line feed.
    pub fn parse(line: &str) -> Option<Self> {
        serde_json::from_str(line).ok()
    }
}

/// The wire shape of [`OpenAnswer`]: one object with a boolean `ok`.
#[derive(Serialize, Deserialize)]
struct AnswerLine {
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<OpenFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl TryFrom<AnswerLine> for OpenAnswer {
    type Error = &'static str;

    fn try_from(line: AnswerLine) -> Result<Self, Self::Error> {
        match line {
            AnswerLine {
                ok: true,
                cwd: Some(cwd),
                ..
            } => Ok(OpenAnswer::Opened { cwd }),
            AnswerLine {
                ok: false,
                error: Some(error),
                message,
                ..
            } => Ok(OpenAnswer::Refused {
                error,
                message: message.unwrap_or_default(),
            }),
            AnswerLine { ok: true, .. } => Err("an open answer with `ok` names its `cwd`"),
            AnswerLine { ok: false, .. } => Err("a refusal names its `error`"),
        }
    }
}

impl From<OpenAnswer> for AnswerLine {
    fn from(answer: OpenAnswer) -> Self {
        match answer {
            OpenAnswer::Opened { cwd } => AnswerLine {
                ok: true,
                cwd: Some(cwd),
                error: None,
                message: None,
            },
            OpenAnswer::Refused { error, message } => AnswerLine {
                ok: false,
                cwd: None,
                error: Some(error),
                message: Some(message),
            },
        }
    }
}

/// How the process of one Coding Session ended, as the Client App
/// reported it on its Host socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionExit {
    /// `None` when a signal ended the process.
    pub exit_code: Option<i64>,
    /// The last bytes of its stderr, at most [`STDERR_TAIL_LIMIT`].
    pub stderr_tail: String,
}

impl SessionExit {
    /// The exit, with the last [`STDERR_TAIL_LIMIT`] bytes of
    /// `stderr_tail` at most, cut at a character boundary.
    pub fn new(exit_code: Option<i64>, stderr_tail: String) -> Self {
        let mut start = stderr_tail.len().saturating_sub(STDERR_TAIL_LIMIT);
        while !stderr_tail.is_char_boundary(start) {
            start += 1;
        }
        Self {
            exit_code,
            stderr_tail: stderr_tail[start..].to_string(),
        }
    }
}

/// Why no Coding Session opened on a Host.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionOpenError {
    /// No session socket of that Host is open for that Workspace.
    #[error("the Host has no session socket open")]
    NotConnected,
    /// The Client App answered with a refusal.
    #[error("the Client App refused the session ({code:?}): {message}")]
    Refused { code: OpenFailure, message: String },
    /// No stream opened, the stream broke, no answer came in time, or a
    /// line came that is no answer.
    #[error("the session did not open: {0}")]
    Failed(String),
}

/// One Coding Session that opened on a Host.
#[derive(Debug)]
pub struct OpenedSession {
    /// After the answer, the raw stdin (written) and stdout (read) of the
    /// process. Its end (a FIN) closes stdin; dropping it resets the
    /// stream.
    pub stream: yamux::Stream,
    /// The directory that the process runs in.
    pub cwd: String,
    /// How the process ended. An error when the session socket ended
    /// first: the place is lost.
    pub exit: oneshot::Receiver<SessionExit>,
}

/// The yamux configuration of both ends of a session socket. It bounds
/// the sum of all receive windows to the default window of each stream.
pub fn yamux_config() -> yamux::Config {
    let mut config = yamux::Config::default();
    config.set_max_num_streams(MAX_STREAMS);
    config.set_max_connection_receive_window(Some(MAX_STREAMS * yamux::DEFAULT_CREDIT as usize));
    config
}

/// A request for one stream, which the driver of the socket answers.
type Opening = oneshot::Sender<Result<yamux::Stream, yamux::ConnectionError>>;

/// One open session socket.
struct Socket {
    /// The Workspace of the Session that opened the socket.
    workspace_id: WorkspaceId,
    /// Which registration this is, so a socket that a newer one of the
    /// same Host replaced does not take the newer one with it.
    epoch: u64,
    opens: mpsc::Sender<Opening>,
}

/// One opened session whose exit has not arrived.
struct Waiting {
    /// The registration of the socket that carries the session.
    epoch: u64,
    exit: oneshot::Sender<SessionExit>,
}

/// A session, by the Workspace and the Host that it runs for and on.
type SessionKey = (WorkspaceId, HostId, CodingSessionId);

#[derive(Default)]
struct State {
    sockets: HashMap<HostId, Socket>,
    waiting: HashMap<SessionKey, Waiting>,
}

/// The open session sockets of the Hosts, and the sessions on them that
/// wait for their exit.
#[derive(Default)]
pub struct HostSessions {
    state: Mutex<State>,
    next_epoch: AtomicU64,
}

impl HostSessions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serve the session socket of `host_id`, which a Session of
    /// `workspace_id` opened, until it ends. A second socket of the same
    /// Host replaces the first for new streams. When the socket ends,
    /// every stream on it ends, and the `exit` of each of its sessions
    /// ends with an error.
    pub async fn serve<T>(&self, workspace_id: WorkspaceId, host_id: HostId, socket: T)
    where
        T: futures::AsyncRead + futures::AsyncWrite + Unpin + Send + 'static,
    {
        let (opens, mut requests) = mpsc::channel::<Opening>(OPEN_QUEUE);
        let epoch = self.next_epoch.fetch_add(1, Ordering::SeqCst);
        self.lock().sockets.insert(
            host_id.clone(),
            Socket {
                workspace_id,
                epoch,
                opens,
            },
        );
        let _registered = Registered {
            sessions: self,
            host_id,
            epoch,
        };
        let mut connection = yamux::Connection::new(socket, yamux_config(), yamux::Mode::Client);
        let mut waiting: Option<Opening> = None;
        let mut requests_open = true;
        // The end of the socket, cleanly or with an error, is the end of
        // its sessions either way.
        let _ = futures::future::poll_fn(|cx: &mut Context<'_>| {
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
    }

    /// Whether the session socket of `host_id` is open.
    pub fn is_open(&self, host_id: &HostId) -> bool {
        self.lock().sockets.contains_key(host_id)
    }

    /// Open one Coding Session on `host_id` for the Person of
    /// `workspace_id`: open one stream, write the request line and read
    /// the answer line. Only a session socket that a Session of
    /// `workspace_id` opened carries it.
    pub async fn open(
        &self,
        workspace_id: &WorkspaceId,
        host_id: &HostId,
        request: &OpenRequest,
    ) -> Result<OpenedSession, SessionOpenError> {
        let line = request.line();
        if line.len() > REQUEST_LIMIT {
            return Err(SessionOpenError::Failed(format!(
                "the open request is {} bytes, over the limit of {REQUEST_LIMIT}",
                line.len()
            )));
        }
        let (opens, exit) = {
            let mut state = self.lock();
            let socket = state
                .sockets
                .get(host_id)
                .filter(|socket| socket.workspace_id == *workspace_id)
                .ok_or(SessionOpenError::NotConnected)?;
            let (opens, epoch) = (socket.opens.clone(), socket.epoch);
            state.waiting.retain(|_, waiting| !waiting.exit.is_closed());
            let key = (
                workspace_id.clone(),
                host_id.clone(),
                request.session_id.clone(),
            );
            if state.waiting.contains_key(&key) {
                return Err(SessionOpenError::Failed(format!(
                    "the session {} is open on this Host already",
                    request.session_id
                )));
            }
            // The exit waits before the request leaves, so an exit that
            // comes before the answer is read still reaches it.
            let (exit, exited) = oneshot::channel();
            state.waiting.insert(key, Waiting { epoch, exit });
            (opens, exited)
        };
        let answer = async {
            let closed = || SessionOpenError::Failed("the session socket closed".to_string());
            let (opening, opened) = oneshot::channel();
            opens.send(opening).await.map_err(|_| closed())?;
            let mut stream = opened.await.map_err(|_| closed())?.map_err(|error| {
                SessionOpenError::Failed(format!("the session socket opened no stream: {error}"))
            })?;
            let broke =
                |error: io::Error| SessionOpenError::Failed(format!("the stream broke: {error}"));
            stream.write_all(line.as_bytes()).await.map_err(broke)?;
            stream.flush().await.map_err(broke)?;
            let answer = read_line(&mut stream, ANSWER_LIMIT).await.map_err(broke)?;
            match OpenAnswer::parse(&answer) {
                Some(OpenAnswer::Opened { cwd }) => Ok((stream, cwd)),
                Some(OpenAnswer::Refused { error, message }) => Err(SessionOpenError::Refused {
                    code: error,
                    message,
                }),
                None => Err(SessionOpenError::Failed(format!(
                    "the Client App answered {answer:?}, which is no open answer"
                ))),
            }
        };
        let (stream, cwd) = tokio::time::timeout(ANSWER_TIMEOUT, answer)
            .await
            .map_err(|_| {
                SessionOpenError::Failed(format!(
                    "no answer in {} seconds",
                    ANSWER_TIMEOUT.as_secs()
                ))
            })??;
        Ok(OpenedSession { stream, cwd, exit })
    }

    /// Hand the exit of one session to its `exit`. An exit of another
    /// Workspace, of another Host, or of an unknown session reaches
    /// nothing.
    pub fn exited(
        &self,
        workspace_id: &WorkspaceId,
        host_id: &HostId,
        session_id: &CodingSessionId,
        exit: SessionExit,
    ) {
        let key = (workspace_id.clone(), host_id.clone(), session_id.clone());
        if let Some(waiting) = self.lock().waiting.remove(&key) {
            // The owner of the session may have dropped it.
            let _ = waiting.exit.send(exit);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("the session sockets")
    }
}

/// One registration of a session socket. Dropping it makes the socket
/// absent, unless a newer socket of the same Host replaced it, and ends
/// the `exit` of each session that it carried.
struct Registered<'a> {
    sessions: &'a HostSessions,
    host_id: HostId,
    epoch: u64,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        let mut state = self.sessions.lock();
        if state
            .sockets
            .get(&self.host_id)
            .is_some_and(|socket| socket.epoch == self.epoch)
        {
            state.sockets.remove(&self.host_id);
        }
        state.waiting.retain(|(_, host_id, _), waiting| {
            !(*host_id == self.host_id && waiting.epoch == self.epoch)
        });
    }
}

/// Read one line of at most `limit` bytes with its line feed, byte by
/// byte, so the bytes after it stay in the stream. The line comes
/// without its line feed.
pub async fn read_line<S: futures::AsyncRead + Unpin>(
    stream: &mut S,
    limit: usize,
) -> io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0; 1];
    loop {
        stream.read_exact(&mut byte).await?;
        if byte[0] == b'\n' {
            return String::from_utf8(line).map_err(|_| io::Error::other("the line is not text"));
        }
        if line.len() + 1 == limit {
            return Err(io::Error::other("the line is too long"));
        }
        line.push(byte[0]);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

    use super::*;
    use crate::fake::{FakeAnswer, FakeClientApp};

    const WAIT: Duration = Duration::from_secs(5);

    fn request(session_id: &CodingSessionId) -> OpenRequest {
        OpenRequest {
            session_id: session_id.clone(),
            command: "npx".to_string(),
            args: vec!["--yes".to_string(), "pkg@1.0.0".to_string()],
            cwd: "/Users/bo/code/app".to_string(),
            env: BTreeMap::from([("NO_COLOR".to_string(), "1".to_string())]),
            worktree: Some(WorktreeRequest {
                repo: "/Users/bo/code/app".to_string(),
                branch: "pagis/fix-login".to_string(),
                base: "main".to_string(),
            }),
        }
    }

    fn socket_pair() -> (
        Compat<tokio::io::DuplexStream>,
        Compat<tokio::io::DuplexStream>,
    ) {
        let (daemon, client_app) = tokio::io::duplex(64 * 1024);
        (daemon.compat(), client_app.compat())
    }

    /// Serve one session socket of `host_id` for `workspace_id`, with
    /// `client_app` at its other end, until the task ends.
    async fn open_socket(
        sessions: &Arc<HostSessions>,
        workspace_id: &WorkspaceId,
        host_id: &HostId,
        client_app: Arc<FakeClientApp>,
    ) -> tokio::task::JoinHandle<()> {
        let (daemon_end, client_app_end) = socket_pair();
        let served = tokio::spawn({
            let sessions = Arc::clone(sessions);
            let (workspace_id, host_id) = (workspace_id.clone(), host_id.clone());
            async move { sessions.serve(workspace_id, host_id, daemon_end).await }
        });
        tokio::spawn(client_app.serve(client_app_end));
        wait_until(|| sessions.is_open(host_id)).await;
        served
    }

    async fn wait_until(ready: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT;
        while !ready() {
            assert!(tokio::time::Instant::now() < deadline, "not in time");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The request is one JSON line with every field. Each answer form
    /// goes out as one line and reads back the same, and a line that is
    /// no answer reads as none.
    #[test]
    fn the_request_and_each_answer_are_one_json_line() {
        let session_id = CodingSessionId::generate();
        let line = request(&session_id).line();
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).expect("JSON"),
            serde_json::json!({
                "session_id": session_id.as_str(),
                "command": "npx",
                "args": ["--yes", "pkg@1.0.0"],
                "cwd": "/Users/bo/code/app",
                "env": {"NO_COLOR": "1"},
                "worktree": {
                    "repo": "/Users/bo/code/app",
                    "branch": "pagis/fix-login",
                    "base": "main",
                },
            })
        );
        assert_eq!(
            serde_json::from_str::<OpenRequest>(&line).expect("a request"),
            request(&session_id)
        );

        let opened = OpenAnswer::Opened {
            cwd: "/Users/bo/.pagis-worktrees/app/pagis/fix-login".to_string(),
        };
        assert_eq!(
            opened.line(),
            "{\"ok\":true,\"cwd\":\"/Users/bo/.pagis-worktrees/app/pagis/fix-login\"}\n"
        );
        for (answer, line) in [
            (
                OpenAnswer::Refused {
                    error: OpenFailure::NotFound,
                    message: "npx is not on PATH".to_string(),
                },
                "{\"ok\":false,\"error\":\"not_found\",\"message\":\"npx is not on PATH\"}\n",
            ),
            (
                OpenAnswer::Refused {
                    error: OpenFailure::BadDirectory,
                    message: String::new(),
                },
                "{\"ok\":false,\"error\":\"bad_directory\",\"message\":\"\"}\n",
            ),
            (
                OpenAnswer::Refused {
                    error: OpenFailure::WorktreeFailed,
                    message: "x".to_string(),
                },
                "{\"ok\":false,\"error\":\"worktree_failed\",\"message\":\"x\"}\n",
            ),
            (
                OpenAnswer::Refused {
                    error: OpenFailure::SpawnFailed,
                    message: "x".to_string(),
                },
                "{\"ok\":false,\"error\":\"spawn_failed\",\"message\":\"x\"}\n",
            ),
        ] {
            assert_eq!(answer.line(), line);
        }
        for answer in [
            opened,
            OpenAnswer::Refused {
                error: OpenFailure::SpawnFailed,
                message: "exec format error".to_string(),
            },
        ] {
            assert_eq!(OpenAnswer::parse(answer.line().trim_end()), Some(answer));
        }
        for other in [
            "",
            "ok",
            "{\"ok\":true}",
            "{\"ok\":false}",
            "{\"ok\":false,\"error\":\"gone\"}",
            "{\"ok\":\"yes\",\"cwd\":\"/\"}",
        ] {
            assert_eq!(OpenAnswer::parse(other), None, "{other}");
        }
    }

    /// The exit keeps the last 4 KiB of stderr, cut at a character
    /// boundary.
    #[test]
    fn an_exit_keeps_the_tail_of_stderr() {
        let short = SessionExit::new(Some(1), "boom\n".to_string());
        assert_eq!(short.stderr_tail, "boom\n");
        assert_eq!(short.exit_code, Some(1));

        let long = format!("{}{}", "é".repeat(STDERR_TAIL_LIMIT), "the end");
        let cut = SessionExit::new(None, long.clone());
        assert!(cut.stderr_tail.len() <= STDERR_TAIL_LIMIT);
        assert!(cut.stderr_tail.len() >= STDERR_TAIL_LIMIT - 1);
        assert!(long.ends_with(&cut.stderr_tail));
    }

    /// An open request goes out on its own stream, and after `ok` the
    /// stream carries bytes both ways.
    #[tokio::test]
    async fn an_open_session_carries_bytes_both_ways() {
        let (workspace_id, host_id) = (WorkspaceId::generate(), HostId::generate());
        let sessions = Arc::new(HostSessions::new());
        let client_app = FakeClientApp::opening("/work");
        let _socket =
            open_socket(&sessions, &workspace_id, &host_id, Arc::clone(&client_app)).await;
        let session_id = CodingSessionId::generate();

        let mut opened = sessions
            .open(&workspace_id, &host_id, &request(&session_id))
            .await
            .expect("the session opens");

        assert_eq!(opened.cwd, "/work");
        assert_eq!(client_app.requests(), [request(&session_id)]);
        opened
            .stream
            .write_all(b"{\"jsonrpc\":\"2.0\"}\n")
            .await
            .expect("write");
        let mut echoed = [0; 18];
        tokio::time::timeout(WAIT, opened.stream.read_exact(&mut echoed))
            .await
            .expect("in time")
            .expect("read");
        assert_eq!(&echoed, b"{\"jsonrpc\":\"2.0\"}\n");
    }

    /// A request over 64 KiB never leaves the daemon.
    #[tokio::test]
    async fn a_request_over_the_limit_is_refused_before_it_is_sent() {
        let (workspace_id, host_id) = (WorkspaceId::generate(), HostId::generate());
        let sessions = Arc::new(HostSessions::new());
        let client_app = FakeClientApp::opening("/work");
        let _socket =
            open_socket(&sessions, &workspace_id, &host_id, Arc::clone(&client_app)).await;
        let mut big = request(&CodingSessionId::generate());
        big.env.insert("BIG".to_string(), "x".repeat(REQUEST_LIMIT));

        let opened = sessions.open(&workspace_id, &host_id, &big).await;

        match opened {
            Err(SessionOpenError::Failed(reason)) => assert!(reason.contains("limit"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(client_app.streams(), 0, "a stream opened");
    }

    /// An answer over 8 KiB and an answer that is not JSON are failures.
    #[tokio::test]
    async fn an_answer_too_long_or_not_json_is_a_failure() {
        for (answer, says) in [
            (
                format!("{{\"ok\":true,\"cwd\":\"{}\"}}\n", "x".repeat(ANSWER_LIMIT)),
                "too long",
            ),
            ("HTTP/1.1 200 OK\n".to_string(), "no open answer"),
        ] {
            let (workspace_id, host_id) = (WorkspaceId::generate(), HostId::generate());
            let sessions = Arc::new(HostSessions::new());
            let client_app = FakeClientApp::answering(FakeAnswer::Raw(answer.into_bytes()));
            let _socket = open_socket(&sessions, &workspace_id, &host_id, client_app).await;

            let opened = sessions
                .open(
                    &workspace_id,
                    &host_id,
                    &request(&CodingSessionId::generate()),
                )
                .await;

            match opened {
                Err(SessionOpenError::Failed(reason)) => {
                    assert!(reason.contains(says), "{reason}")
                }
                other => panic!("{other:?}"),
            }
        }
    }

    /// No answer in 120 seconds is a failure.
    #[tokio::test(start_paused = true)]
    async fn no_answer_in_time_is_a_failure() {
        let (workspace_id, host_id) = (WorkspaceId::generate(), HostId::generate());
        let sessions = Arc::new(HostSessions::new());
        let client_app = FakeClientApp::answering(FakeAnswer::Silent);
        let _socket = open_socket(&sessions, &workspace_id, &host_id, client_app).await;

        let opened = sessions
            .open(
                &workspace_id,
                &host_id,
                &request(&CodingSessionId::generate()),
            )
            .await;

        match opened {
            Err(SessionOpenError::Failed(reason)) => {
                assert!(reason.contains("120 seconds"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }

    /// A refusal of the Client App is `Refused` with its code and
    /// message.
    #[tokio::test]
    async fn a_refusal_is_refused_with_its_code_and_message() {
        let (workspace_id, host_id) = (WorkspaceId::generate(), HostId::generate());
        let sessions = Arc::new(HostSessions::new());
        let client_app = FakeClientApp::answering(FakeAnswer::Refuse {
            code: OpenFailure::BadDirectory,
            message: "/nowhere does not exist".to_string(),
        });
        let _socket = open_socket(&sessions, &workspace_id, &host_id, client_app).await;

        let opened = sessions
            .open(
                &workspace_id,
                &host_id,
                &request(&CodingSessionId::generate()),
            )
            .await;

        assert_eq!(
            opened.err(),
            Some(SessionOpenError::Refused {
                code: OpenFailure::BadDirectory,
                message: "/nowhere does not exist".to_string(),
            })
        );
    }

    /// A Host with no socket, and a socket that a Session of another
    /// Workspace opened, carry nothing: `NotConnected`.
    #[tokio::test]
    async fn no_socket_of_the_workspace_is_not_connected() {
        let (workspace_id, host_id) = (WorkspaceId::generate(), HostId::generate());
        let sessions = Arc::new(HostSessions::new());
        let session = request(&CodingSessionId::generate());
        assert_eq!(
            sessions.open(&workspace_id, &host_id, &session).await.err(),
            Some(SessionOpenError::NotConnected)
        );

        let client_app = FakeClientApp::opening("/work");
        let _theirs = open_socket(
            &sessions,
            &WorkspaceId::generate(),
            &host_id,
            Arc::clone(&client_app),
        )
        .await;

        assert_eq!(
            sessions.open(&workspace_id, &host_id, &session).await.err(),
            Some(SessionOpenError::NotConnected)
        );
        assert_eq!(client_app.streams(), 0);
    }

    /// A second socket of one Host replaces the first for new streams,
    /// and the end of the first does not take the second.
    #[tokio::test]
    async fn a_second_socket_replaces_the_first_and_outlives_its_end() {
        let (workspace_id, host_id) = (WorkspaceId::generate(), HostId::generate());
        let sessions = Arc::new(HostSessions::new());
        let first_app = FakeClientApp::opening("/first");
        let first = open_socket(&sessions, &workspace_id, &host_id, Arc::clone(&first_app)).await;
        let second_app = FakeClientApp::opening("/second");
        let _second =
            open_socket(&sessions, &workspace_id, &host_id, Arc::clone(&second_app)).await;
        // New streams go to the second socket once it is registered.
        let deadline = tokio::time::Instant::now() + WAIT;
        while second_app.streams() == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the second socket took no stream"
            );
            let _ = sessions
                .open(
                    &workspace_id,
                    &host_id,
                    &request(&CodingSessionId::generate()),
                )
                .await;
        }
        let first_streams = first_app.streams();

        first.abort();
        let _ = first.await;

        assert!(
            sessions.is_open(&host_id),
            "the end of the first took the second"
        );
        let opened = sessions
            .open(
                &workspace_id,
                &host_id,
                &request(&CodingSessionId::generate()),
            )
            .await
            .expect("the session opens");
        assert_eq!(opened.cwd, "/second");
        assert_eq!(first_app.streams(), first_streams);
    }

    /// An exit reaches the `exit` of its own session only: an exit of
    /// another Workspace or of another Host reaches nothing.
    #[tokio::test]
    async fn an_exit_reaches_its_own_session_only() {
        let (workspace_id, host_id) = (WorkspaceId::generate(), HostId::generate());
        let sessions = Arc::new(HostSessions::new());
        let _socket = open_socket(
            &sessions,
            &workspace_id,
            &host_id,
            FakeClientApp::opening("/work"),
        )
        .await;
        let session_id = CodingSessionId::generate();
        let mut opened = sessions
            .open(&workspace_id, &host_id, &request(&session_id))
            .await
            .expect("the session opens");
        let exit = SessionExit::new(Some(0), "done".to_string());

        sessions.exited(
            &WorkspaceId::generate(),
            &host_id,
            &session_id,
            exit.clone(),
        );
        sessions.exited(
            &workspace_id,
            &HostId::generate(),
            &session_id,
            exit.clone(),
        );
        sessions.exited(
            &workspace_id,
            &host_id,
            &CodingSessionId::generate(),
            exit.clone(),
        );
        assert_eq!(
            opened.exit.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );

        sessions.exited(&workspace_id, &host_id, &session_id, exit.clone());
        assert_eq!(opened.exit.await, Ok(exit));
    }

    /// A second open of a session id whose first session still waits is a
    /// failure, and an id whose first session was dropped opens again.
    #[tokio::test]
    async fn a_session_id_opens_once_while_it_waits() {
        let (workspace_id, host_id) = (WorkspaceId::generate(), HostId::generate());
        let sessions = Arc::new(HostSessions::new());
        let _socket = open_socket(
            &sessions,
            &workspace_id,
            &host_id,
            FakeClientApp::opening("/work"),
        )
        .await;
        let session = request(&CodingSessionId::generate());
        let first = sessions
            .open(&workspace_id, &host_id, &session)
            .await
            .expect("the session opens");

        assert!(matches!(
            sessions.open(&workspace_id, &host_id, &session).await,
            Err(SessionOpenError::Failed(_))
        ));

        drop(first);
        sessions
            .open(&workspace_id, &host_id, &session)
            .await
            .expect("the session opens again");
    }

    /// The end of the socket ends the `exit` of each of its sessions with
    /// an error: the place is lost.
    #[tokio::test]
    async fn the_end_of_the_socket_ends_each_exit_with_an_error() {
        let (workspace_id, host_id) = (WorkspaceId::generate(), HostId::generate());
        let sessions = Arc::new(HostSessions::new());
        let socket = open_socket(
            &sessions,
            &workspace_id,
            &host_id,
            FakeClientApp::opening("/work"),
        )
        .await;
        let mut opened = Vec::new();
        for _ in 0..2 {
            opened.push(
                sessions
                    .open(
                        &workspace_id,
                        &host_id,
                        &request(&CodingSessionId::generate()),
                    )
                    .await
                    .expect("the session opens"),
            );
        }

        socket.abort();
        let _ = socket.await;

        assert!(!sessions.is_open(&host_id));
        for session in opened {
            let ended = tokio::time::timeout(WAIT, session.exit).await;
            assert!(ended.expect("in time").is_err());
        }
    }
}
