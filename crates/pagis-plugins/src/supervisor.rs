//! One supervised MCP client per declared server (ADR-0017).
//!
//! The rules here are the ones ADR-0017 fixed, and they are all about
//! time, failure and output: a server starts on the first call and not
//! before, it gets 30 s to answer the handshake, one call gets 120 s
//! unless the plugin declared a longer one, an idle server stops after
//! ten minutes, a crashed server starts again at the next call with a
//! backoff that grows to one minute, and three start failures in a row
//! stop the daemon from trying at all until the user asks. A stdout
//! message over 4 MiB ends the session of a stdio server, and three of
//! them in a row stop the daemon from trying in the same way. The
//! server's stderr goes to a bounded log behind [`StdioServers`]. A
//! `notifications/tools/list_changed` from a running server goes to
//! [`ToolListChanged`] when it arrives.
//!
//! Nothing here decides whether a call is allowed. The broker did that
//! before the call arrived.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{PluginId, WorkspaceId};
use pagis_rt::Instant;
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use tokio::sync::{Mutex, Semaphore};

use crate::http::{HttpClient, McpHttp, ReqwestMcpHttp};
use crate::session::{Overrun, Session, SessionError};
use crate::stdio::StdioServers;
use crate::substitute::{Endpoint, Spawn};

/// How long a server has to answer the handshake.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
/// The deadline of one call when the plugin declares none.
pub const DEFAULT_CALL_TIMEOUT: Duration = pagis_plugin::DEFAULT_CALL_TIMEOUT;
/// The longest deadline a plugin may declare.
pub const MAX_CALL_TIMEOUT: Duration = pagis_plugin::MAX_CALL_TIMEOUT;
/// How long a server may sit unused before it stops.
pub const IDLE_SHUTDOWN: Duration = Duration::from_secs(600);
/// The first wait after a start failure.
pub const BACKOFF_FLOOR: Duration = Duration::from_secs(1);
/// The longest wait between two start attempts.
pub const BACKOFF_CEILING: Duration = Duration::from_secs(60);
/// How many start failures in a row mark the Plugin `failed`.
pub const MAX_START_FAILURES: u32 = 3;
/// How many calls one server answers at once.
pub const MAX_IN_FLIGHT: usize = 8;
/// The largest message a stdio server may write on stdout: 4 MiB,
/// without the newline that ends it. It is far above the broker's cap
/// on a tool result, so no result the broker keeps passes it. A longer
/// line ends the session as soon as it passes the cap, and the pending
/// call answers `outcome_unknown`: the rest of the line cannot be read
/// as JSON-RPC, so the session cannot continue.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
/// How many output overruns in a row mark the Plugin `failed`. An
/// overrun is a stdout message over [`MAX_FRAME_BYTES`]. A call that
/// completes within the cap sets the count to zero, and a successful
/// start does not: a server that overruns its output can still start,
/// so a count that each start sets to zero never gets to the limit.
pub const MAX_OUTPUT_OVERRUNS: u32 = 3;

/// Why a call did not reach a server, or did not come back from one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServerError {
    /// The server did not start. The Plugin is not failed yet: the
    /// next call tries again after the backoff.
    #[error("{0}")]
    Start(String),
    /// The server failed to start [`MAX_START_FAILURES`] times in a
    /// row, or its stdout passed [`MAX_FRAME_BYTES`]
    /// [`MAX_OUTPUT_OVERRUNS`] times in a row. Only the user starts it
    /// again.
    #[error("the server {0:?} failed three times in a row")]
    Failed(String),
    /// A start failed a moment ago and the backoff has not passed.
    #[error("the server {server:?} is waiting {seconds} seconds before it starts again")]
    Waiting { server: String, seconds: u64 },
    /// Every one of the [`MAX_IN_FLIGHT`] slots is taken.
    #[error("the server {0:?} is answering as many calls as it can")]
    Busy(String),
    /// The call was sent and no answer came back. Nothing can say
    /// whether the server acted on it (ADR-0005).
    #[error("{0}")]
    Unknown(String),
    /// The server wrote a stdout message over [`MAX_FRAME_BYTES`], and
    /// its session ended. As with [`ServerError::Unknown`], nothing can
    /// say whether the server acted on the call. `failed` says that
    /// this overrun was the last of [`MAX_OUTPUT_OVERRUNS`] in a row,
    /// so the server now waits for the user's start.
    #[error(
        "the server {server:?} wrote a message larger than {max} bytes, so its session ended",
        max = MAX_FRAME_BYTES
    )]
    Overrun { server: String, failed: bool },
    /// The server answered, and the answer is not a tool result.
    #[error("the server {0:?} answered in a way Pagis does not support")]
    Unsupported(String),
}

/// How one server is reached.
pub enum Launch {
    /// A process in the tenant's Plugin Computer (ADR-0017).
    Stdio(Spawn),
    /// An outbound request of the daemon. An HTTP server is no process,
    /// so nothing of it runs anywhere for Pagis to place.
    Http(Endpoint),
}

/// What the daemon does when a running server says that its tool list
/// changed. The host marks the Plugin, and the frozen list stays in
/// force (ADR-0017). The mark comes from the notification itself.
/// HTTP notifications arrive in POST answers; no GET stream opens.
#[async_trait]
pub trait ToolListChanged: Send + Sync {
    async fn tool_list_changed(&self);
}

struct Running {
    client: Arc<dyn Session>,
    /// What the bound values were at the start. A rotated Connection
    /// token changes it, and the next call restarts the server
    /// (ADR-0017).
    fingerprint: String,
    last_used: Instant,
}

#[derive(Default)]
struct State {
    running: Option<Running>,
    failures: u32,
    /// The earliest instant a start may be tried again.
    retry_at: Option<Instant>,
}

/// One declared server of one Plugin of one tenant, and the process
/// behind it. The process runs in that tenant's Plugin Computer;
/// this supervisor holds the client end of it.
pub struct ServerSupervisor {
    server: String,
    /// The tenant whose Plugin Computer runs the process.
    workspace_id: WorkspaceId,
    plugin_id: PluginId,
    stdio: Arc<dyn StdioServers>,
    state: Mutex<State>,
    http: Arc<dyn McpHttp>,
    /// The output overruns in a row. It is not in [`State`], so a call
    /// that counts one never waits for a start that holds the state.
    overruns: AtomicU32,
    /// Told when a running server says that its tool list changed.
    changed: Arc<dyn ToolListChanged>,
    permits: Arc<Semaphore>,
}

impl ServerSupervisor {
    pub fn new(
        server: impl Into<String>,
        workspace_id: WorkspaceId,
        plugin_id: PluginId,
        stdio: Arc<dyn StdioServers>,
        changed: Arc<dyn ToolListChanged>,
    ) -> Self {
        Self {
            server: server.into(),
            workspace_id,
            plugin_id,
            stdio,
            state: Mutex::new(State::default()),
            http: Arc::new(ReqwestMcpHttp::default()),
            overruns: AtomicU32::new(0),
            changed,
            permits: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
        }
    }

    /// Supply the HTTP transport of an Embedder.
    pub fn with_http(mut self, http: Arc<dyn McpHttp>) -> Self {
        self.http = http;
        self
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    /// The tools the running server offers now. The freeze calls it
    /// once at install, and every start compares its answer with the
    /// frozen list.
    pub async fn list_tools(&self, client: &dyn Session) -> Result<Vec<Tool>, ServerError> {
        client
            .list_tools()
            .await
            .map_err(|error| ServerError::Start(format!("tools/list failed: {error}")))
    }

    /// The running client, started if it is not running, restarted if
    /// it crashed or if a bound value rotated. The answer says whether
    /// this call started the server: the caller then compares the tool
    /// list with the frozen one (ADR-0017).
    pub async fn client(
        &self,
        launch: &Launch,
        fingerprint: &str,
        now: Instant,
    ) -> Result<(Arc<dyn Session>, bool), ServerError> {
        let mut state = self.state.lock().await;
        if let Some(running) = &mut state.running {
            let stale = running.fingerprint != fingerprint;
            // A server that left takes its transport with it: the
            // service is not cancelled, and only the closed transport
            // says that the process is gone.
            let gone = running.client.is_closed();
            if !gone && !stale {
                running.last_used = now;
                return Ok((Arc::clone(&running.client), false));
            }
            // A crashed server and a rotated token both end the same
            // way: the process goes, and a new one starts below.
            let running = state.running.take().expect("the running server is there");
            running.client.close().await;
        }
        if state.failures >= MAX_START_FAILURES
            || self.overruns.load(Ordering::SeqCst) >= MAX_OUTPUT_OVERRUNS
        {
            return Err(ServerError::Failed(self.server.clone()));
        }
        if let Some(retry_at) = state.retry_at
            && now < retry_at
        {
            return Err(ServerError::Waiting {
                server: self.server.clone(),
                seconds: (retry_at - now).as_secs() + 1,
            });
        }
        match self.start(launch).await {
            Ok(client) => {
                state.failures = 0;
                state.retry_at = None;
                state.running = Some(Running {
                    client: Arc::clone(&client),
                    fingerprint: fingerprint.to_string(),
                    last_used: now,
                });
                Ok((client, true))
            }
            Err(error) => {
                state.failures += 1;
                state.retry_at = Some(now + backoff(state.failures));
                if state.failures >= MAX_START_FAILURES {
                    return Err(ServerError::Failed(self.server.clone()));
                }
                Err(error)
            }
        }
    }

    /// One `tools/call`, inside the deadline the manifest fixed. A
    /// deadline that passes sends the cancellation the specification
    /// asks for and answers `outcome_unknown`: nothing here can say
    /// whether the server acted (ADR-0005). A session that ends at the
    /// frame cap answers [`ServerError::Overrun`], and a call that
    /// completes within the cap sets the overrun count to zero.
    pub async fn call(
        &self,
        client: &dyn Session,
        tool: &str,
        arguments: serde_json::Value,
        deadline: Duration,
    ) -> Result<CallToolResult, ServerError> {
        let _permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| ServerError::Busy(self.server.clone()))?;
        let mut params = CallToolRequestParams::new(tool.to_string());
        if let Some(arguments) = arguments.as_object() {
            params = params.with_arguments(arguments.clone());
        }
        let answer = client
            .call(params, deadline.min(MAX_CALL_TIMEOUT))
            .await
            .map_err(|error| self.lost(error))?;
        // The answer came within the cap, so the overruns before it
        // are not in a row with the next one.
        self.overruns.store(0, Ordering::SeqCst);
        Ok(answer)
    }

    /// Why a sent call has no answer. A session that ended at the frame
    /// cap counts one output overrun toward the failed state, however
    /// many calls it lost (ADR-0017).
    fn lost(&self, error: SessionError) -> ServerError {
        match error {
            SessionError::Overrun { first } => {
                let failed = first
                    && self.overruns.fetch_add(1, Ordering::SeqCst) + 1 >= MAX_OUTPUT_OVERRUNS;
                ServerError::Overrun {
                    server: self.server.clone(),
                    failed,
                }
            }
            SessionError::Lost(error) => ServerError::Unknown(error),
            SessionError::Unsupported => {
                self.overruns.store(0, Ordering::SeqCst);
                ServerError::Unsupported(self.server.clone())
            }
        }
    }

    /// Close the running session. A stdio session closes the process
    /// input. An HTTP session sends DELETE when it has a session id.
    pub async fn stop(&self) {
        let mut state = self.state.lock().await;
        if let Some(running) = state.running.take() {
            running.client.close().await;
        }
        state.failures = 0;
        state.retry_at = None;
        self.overruns.store(0, Ordering::SeqCst);
    }

    /// Stop the server when it has been unused for [`IDLE_SHUTDOWN`].
    /// The answer says whether it stopped.
    pub async fn stop_if_idle(&self, now: Instant) -> bool {
        let mut state = self.state.lock().await;
        let idle = state
            .running
            .as_ref()
            .is_some_and(|running| now.duration_since(running.last_used) >= IDLE_SHUTDOWN);
        if idle && let Some(running) = state.running.take() {
            running.client.close().await;
            return true;
        }
        false
    }

    /// Forget the start failures and the output overruns, so the next
    /// call starts the server. Only the user reaches this, through the
    /// Start of the desk.
    pub async fn clear_failures(&self) {
        let mut state = self.state.lock().await;
        state.failures = 0;
        state.retry_at = None;
        self.overruns.store(0, Ordering::SeqCst);
    }

    pub async fn is_running(&self) -> bool {
        self.state
            .lock()
            .await
            .running
            .as_ref()
            .is_some_and(|running| !running.client.is_closed())
    }

    async fn start(&self, launch: &Launch) -> Result<Arc<dyn Session>, ServerError> {
        let connect = async {
            match launch {
                Launch::Stdio(spawn) => {
                    self.stdio
                        .connect(
                            &self.workspace_id,
                            &self.plugin_id,
                            &self.server,
                            spawn,
                            Arc::clone(&self.changed),
                        )
                        .await
                }
                Launch::Http(endpoint) => HttpClient::start(
                    Arc::clone(&self.http),
                    endpoint,
                    Arc::new(Overrun::default()),
                    Arc::clone(&self.changed),
                )
                .await
                .map(|client| Arc::new(client) as Arc<dyn Session>),
            }
        };
        let started = pagis_rt::timeout(STARTUP_TIMEOUT, connect).await;
        match started {
            Ok(Ok(client)) => Ok(client),
            Ok(Err(error)) => Err(ServerError::Start(format!(
                "the server {:?} did not start: {error}",
                self.server
            ))),
            Err(_) => Err(ServerError::Start(format!(
                "the server {:?} did not answer in {} seconds",
                self.server,
                STARTUP_TIMEOUT.as_secs()
            ))),
        }
    }
}

/// The wait before the next start attempt: one second, then two, four
/// and so on to one minute (ADR-0017).
pub fn backoff(failures: u32) -> Duration {
    let seconds = BACKOFF_FLOOR
        .as_secs()
        .saturating_mul(1u64 << failures.saturating_sub(1).min(6));
    Duration::from_secs(seconds).min(BACKOFF_CEILING)
}

/// A digest of the bound values one server starts with. It carries no
/// value: it is only long enough to say that something changed.
pub fn fingerprint(values: &BTreeMap<String, String>) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (name, value) in values {
        name.hash(&mut hasher);
        value.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}
