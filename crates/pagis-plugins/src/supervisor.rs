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
//! server's stderr goes to a log of a fixed size ([`crate::log`]). A
//! `notifications/tools/list_changed` from a running server goes to
//! [`ToolListChanged`] when it arrives.
//!
//! Nothing here decides whether a call is allowed. The broker did that
//! before the call arrived.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo,
    ClientRequest, ElicitRequestParams, ElicitResult, ElicitationCreateRequestMethod,
    Implementation, ServerResult, Tool,
};
use rmcp::service::{
    NotificationContext, PeerRequestOptions, RequestContext, RoleClient, RunningService,
    RxJsonRpcMessage, TxJsonRpcMessage,
};
use rmcp::transport::async_rw::{JsonRpcMessageCodec, JsonRpcMessageCodecError};
use rmcp::transport::sink_stream::SinkStreamTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, Transport};
use rmcp::{ClientHandler, ErrorData as McpError, ServiceError, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, FramedRead, FramedWrite};

use pagis_core::WorkspaceId;

use crate::log::PluginLog;
use crate::process::{ServerIo, ServerProcesses};
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
/// force (ADR-0017). The mark comes from the notification itself, so it
/// does not wait for a call to end, and a server that says so while no
/// call runs is marked all the same.
#[async_trait]
pub trait ToolListChanged: Send + Sync {
    async fn tool_list_changed(&self);
}

/// The Pagis side of one MCP connection. It consumes tools and
/// nothing else: sampling and elicitation are refused, and resources,
/// prompts and logging are ignored (ADR-0017).
#[derive(Clone)]
pub struct PluginClient {
    /// Told when the server says its tool list changed.
    changed: Arc<dyn ToolListChanged>,
    /// Set when the stdout of this session passed [`MAX_FRAME_BYTES`].
    overrun: Arc<Overrun>,
}

impl std::fmt::Debug for PluginClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginClient")
            .field("overrun", &self.overrun)
            .finish_non_exhaustive()
    }
}

/// Whether the stdout of one session passed [`MAX_FRAME_BYTES`]. The
/// stdout stream sets it when it ends the session. The supervisor
/// counts it once, however many calls the session lost.
#[derive(Debug, Default)]
struct Overrun {
    happened: AtomicBool,
    counted: AtomicBool,
}

impl Overrun {
    fn happened(&self) -> bool {
        self.happened.load(Ordering::SeqCst)
    }

    /// True for the first caller that sees the overrun, and false for
    /// every other caller.
    fn count(&self) -> bool {
        self.happened() && !self.counted.swap(true, Ordering::SeqCst)
    }
}

impl ClientHandler for PluginClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("pagis", env!("CARGO_PKG_VERSION")),
        )
    }

    async fn create_elicitation(
        &self,
        _request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, McpError> {
        // A plugin server may not interrupt the user. `ask_user`
        // belongs to the Agent, and a map from one onto the other is
        // not built (ADR-0017).
        Err(McpError::method_not_found::<ElicitationCreateRequestMethod>())
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.changed.tool_list_changed().await;
    }
}

/// One connected client of one plugin server.
pub type Client = RunningService<RoleClient, PluginClient>;

struct Running {
    client: Arc<Client>,
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
    /// Where a server process runs. It is never the host.
    processes: Arc<dyn ServerProcesses>,
    /// Where the server's stderr goes. The desk reads it through the
    /// API; nothing of it reaches the model (ADR-0005).
    log: PluginLog,
    state: Mutex<State>,
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
        processes: Arc<dyn ServerProcesses>,
        log: PluginLog,
        changed: Arc<dyn ToolListChanged>,
    ) -> Self {
        Self {
            server: server.into(),
            workspace_id,
            processes,
            log,
            state: Mutex::new(State::default()),
            overruns: AtomicU32::new(0),
            changed,
            permits: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
        }
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    /// The tools the running server offers now. The freeze calls it
    /// once at install, and every start compares its answer with the
    /// frozen list.
    pub async fn list_tools(&self, client: &Client) -> Result<Vec<Tool>, ServerError> {
        client
            .list_all_tools()
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
    ) -> Result<(Arc<Client>, bool), ServerError> {
        let mut state = self.state.lock().await;
        if let Some(running) = &mut state.running {
            let stale = running.fingerprint != fingerprint;
            // A server that left takes its transport with it: the
            // service is not cancelled, and only the closed transport
            // says that the process is gone.
            let gone = running.client.is_closed() || running.client.is_transport_closed();
            if !gone && !stale {
                running.last_used = now;
                return Ok((Arc::clone(&running.client), false));
            }
            // A crashed server and a rotated token both end the same
            // way: the process goes, and a new one starts below.
            let running = state.running.take().expect("the running server is there");
            running.client.cancellation_token().cancel();
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
                let client = Arc::new(client);
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
        client: &Client,
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
        let sent = client
            .send_cancellable_request(
                ClientRequest::CallToolRequest(CallToolRequest::new(params)),
                PeerRequestOptions::with_timeout(deadline.min(MAX_CALL_TIMEOUT)),
            )
            .await;
        let answer = match sent {
            Ok(handle) => handle.await_response().await,
            Err(error) => Err(error),
        };
        let answer = match answer {
            Ok(answer) => answer,
            Err(error) => return Err(self.lost(client, error)),
        };
        // The answer came within the cap, so the overruns before it
        // are not in a row with the next one.
        self.overruns.store(0, Ordering::SeqCst);
        match answer {
            ServerResult::CallToolResult(result) => Ok(result),
            _ => Err(ServerError::Unsupported(self.server.clone())),
        }
    }

    /// Why a sent call has no answer. A session that ended at the frame
    /// cap counts one output overrun toward the failed state, however
    /// many calls it lost (ADR-0017).
    fn lost(&self, client: &Client, error: ServiceError) -> ServerError {
        let overrun = &client.service().overrun;
        if overrun.happened() {
            let failed = overrun.count()
                && self.overruns.fetch_add(1, Ordering::SeqCst) + 1 >= MAX_OUTPUT_OVERRUNS;
            return ServerError::Overrun {
                server: self.server.clone(),
                failed,
            };
        }
        match error {
            ServiceError::Timeout { timeout } => ServerError::Unknown(format!(
                "the call did not answer in {} seconds",
                timeout.as_secs()
            )),
            error => ServerError::Unknown(error.to_string()),
        }
    }

    /// Stop the server if it is running. The transport closes, which
    /// closes the child's stdin, waits for it and kills it if it
    /// overruns.
    pub async fn stop(&self) {
        let mut state = self.state.lock().await;
        if let Some(running) = state.running.take() {
            running.client.cancellation_token().cancel();
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
            running.client.cancellation_token().cancel();
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
            .is_some_and(|running| {
                !running.client.is_closed() && !running.client.is_transport_closed()
            })
    }

    async fn start(&self, launch: &Launch) -> Result<Client, ServerError> {
        let overrun = Arc::new(Overrun::default());
        let handler = PluginClient {
            changed: Arc::clone(&self.changed),
            overrun: Arc::clone(&overrun),
        };
        let started = match launch {
            Launch::Stdio(spawn) => {
                let transport = self.start_in_container(spawn, overrun).await?;
                tokio::time::timeout(STARTUP_TIMEOUT, handler.serve(transport)).await
            }
            Launch::Http(endpoint) => {
                let transport = http_transport(endpoint)?;
                tokio::time::timeout(STARTUP_TIMEOUT, handler.serve(transport)).await
            }
        };
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

    /// Start the server inside the tenant's Plugin Computer and
    /// speak the stdio protocol over the exec's streams. The
    /// environment the package asked for travels in that exec and
    /// nowhere else, and the server's stderr goes to the Plugin's log.
    /// A stdout message over [`MAX_FRAME_BYTES`] sets `overrun` and
    /// ends the session.
    ///
    /// Nothing here kills the process: the Engine API cannot stop a
    /// running exec. An MCP server ends when its stdin closes, which is
    /// what the transport does when the client is cancelled, and the
    /// container stop kills whatever is left.
    async fn start_in_container(
        &self,
        spawn: &Spawn,
        overrun: Arc<Overrun>,
    ) -> Result<impl Transport<RoleClient> + 'static, ServerError> {
        let ServerIo {
            stdin,
            stdout,
            stderr,
        } = self
            .processes
            .start(&self.workspace_id, spawn)
            .await
            .map_err(|error| {
                ServerError::Start(format!(
                    "the server {:?} did not start: {error}",
                    self.server
                ))
            })?;
        self.log.record_stderr(&self.server, stderr);
        Ok(stdio_transport(&self.server, stdout, stdin, overrun))
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

/// The stdio transport of one server, with a cap of
/// [`MAX_FRAME_BYTES`] on each stdout message. rmcp's own stdio
/// transport reads a line with no limit, so one line with no newline
/// could fill the daemon's memory. A line over the cap ends the stream
/// when it passes the cap, which ends the session, and sets `overrun`.
fn stdio_transport(
    server: &str,
    stdout: Pin<Box<dyn AsyncRead + Send + Unpin>>,
    stdin: Pin<Box<dyn AsyncWrite + Send>>,
    overrun: Arc<Overrun>,
) -> impl Transport<RoleClient> + 'static {
    let server = server.to_string();
    let codec = StdoutCodec(JsonRpcMessageCodec::new_with_max_length(MAX_FRAME_BYTES));
    let messages = FramedRead::new(stdout, codec)
        .take_while(move |frame| {
            if let Err(error) = frame {
                if matches!(error, JsonRpcMessageCodecError::MaxLineLengthExceeded) {
                    overrun.happened.store(true, Ordering::SeqCst);
                }
                tracing::warn!(%error, server, "the plugin server's stdout ends its session");
            }
            std::future::ready(frame.is_ok())
        })
        .filter_map(|frame| std::future::ready(frame.ok()));
    let requests = FramedWrite::new(
        stdin,
        JsonRpcMessageCodec::<TxJsonRpcMessage<RoleClient>>::default(),
    );
    SinkStreamTransport::new(requests, Box::pin(messages))
}

/// rmcp's JSON-RPC line codec with the frame cap, as the stdout of one
/// server needs it. rmcp's stdio transport skips a line that is not
/// JSON-RPC, and so does this codec: a server that prints a banner on
/// stdout keeps its session. Only a line over the cap and a read error
/// end the stream.
struct StdoutCodec(JsonRpcMessageCodec<RxJsonRpcMessage<RoleClient>>);

type StdoutFrame = Result<Option<RxJsonRpcMessage<RoleClient>>, JsonRpcMessageCodecError>;

impl Decoder for StdoutCodec {
    type Item = RxJsonRpcMessage<RoleClient>;
    type Error = JsonRpcMessageCodecError;

    fn decode(&mut self, buffer: &mut BytesMut) -> StdoutFrame {
        skip_unreadable(buffer, |buffer| self.0.decode(buffer))
    }

    fn decode_eof(&mut self, buffer: &mut BytesMut) -> StdoutFrame {
        skip_unreadable(buffer, |buffer| self.0.decode_eof(buffer))
    }
}

/// Decode until a message, the need for more bytes, or an error that
/// ends the stream. A line that is not JSON-RPC is skipped. The codec
/// also answers `None` after it skips a notification that is not MCP,
/// so a buffer that got shorter and still holds bytes is decoded again.
/// Each skip makes the buffer shorter, so the loop ends.
fn skip_unreadable(
    buffer: &mut BytesMut,
    mut decode: impl FnMut(&mut BytesMut) -> StdoutFrame,
) -> StdoutFrame {
    loop {
        let held = buffer.len();
        match decode(buffer) {
            Err(JsonRpcMessageCodecError::Serde(error)) => {
                tracing::debug!(%error, "the plugin server wrote a line that is not JSON-RPC");
            }
            Ok(None) if buffer.len() < held && !buffer.is_empty() => {}
            decoded => return decoded,
        }
    }
}

/// The streamable HTTP transport of one server. An `Authorization`
/// header becomes the transport's own auth header, which is where
/// `rmcp` puts a bearer token; every other header travels as it is.
fn http_transport(
    endpoint: &Endpoint,
) -> Result<impl Transport<RoleClient> + 'static, ServerError> {
    install_crypto_provider();
    let mut config = StreamableHttpClientTransportConfig::with_uri(endpoint.url.clone())
        .reinit_on_expired_session(true);
    let mut custom = std::collections::HashMap::new();
    for (name, value) in &endpoint.headers {
        if name.eq_ignore_ascii_case("authorization") {
            config = config.auth_header(value.trim_start_matches("Bearer ").to_string());
            continue;
        }
        let name = http::HeaderName::try_from(name.as_str())
            .map_err(|_| ServerError::Start(format!("the header name {name:?} is not one")))?;
        let value = http::HeaderValue::try_from(value.as_str()).map_err(|_| {
            ServerError::Start(format!(
                "the header {name} carries a value a request cannot hold"
            ))
        })?;
        custom.insert(name, value);
    }
    config = config.custom_headers(custom);
    Ok(StreamableHttpClientTransport::from_config(config))
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

/// Install the workspace's `ring` provider as the process-level rustls
/// provider, once. `rmcp`'s HTTP client is built with no provider of
/// its own, so that the workspace does not carry two crypto backends
/// (its second reqwest would otherwise pull in `aws-lc-rs` beside the
/// mail crates' `ring`, and rustls then refuses to pick one).
fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Another crate may have installed a provider first; that is
        // fine, there is one and it is `ring`.
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    });
}
