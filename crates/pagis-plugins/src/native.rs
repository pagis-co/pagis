//! The daemon's stdio sessions, process streams and bounded logs.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

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
use rmcp::transport::Transport;
use rmcp::transport::async_rw::{JsonRpcMessageCodec, JsonRpcMessageCodecError};
use rmcp::transport::sink_stream::SinkStreamTransport;
use rmcp::{ClientHandler, ErrorData as McpError, ServiceError, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, FramedRead, FramedWrite};

use crate::session::Overrun;
use crate::{
    MAX_FRAME_BYTES, PluginLogs, ServerIo, ServerProcesses, Session, SessionError, Spawn,
    StdioServers, ToolListChanged,
};
use pagis_core::{PluginId, WorkspaceId};

/// The daemon's stdio client. All processes and logs stay behind this seam.
pub struct NativeStdioServers {
    processes: Arc<dyn ServerProcesses>,
    logs: Arc<PluginLogs>,
}

impl NativeStdioServers {
    pub fn new(processes: Arc<dyn ServerProcesses>, logs: Arc<PluginLogs>) -> Self {
        Self { processes, logs }
    }
}

#[async_trait]
impl StdioServers for NativeStdioServers {
    async fn connect(
        &self,
        workspace: &WorkspaceId,
        plugin: &PluginId,
        server: &str,
        spawn: &Spawn,
        changed: Arc<dyn ToolListChanged>,
    ) -> Result<Arc<dyn Session>, String> {
        let overrun = Arc::new(Overrun::default());
        let handler = PluginClient {
            changed,
            overrun: overrun.clone(),
        };
        let ServerIo {
            stdin,
            stdout,
            stderr,
        } = self.processes.start(workspace, spawn).await?;
        self.logs
            .log(workspace, plugin)
            .record_stderr(server, stderr);
        let client = handler
            .serve(stdio_transport(server, stdout, stdin, overrun))
            .await
            .map_err(|error| error.to_string())?;
        Ok(Arc::new(StdioSession(client)))
    }
    async fn tail(&self, workspace: &WorkspaceId, plugin: &PluginId, limit: usize) -> String {
        self.logs.tail(workspace, plugin, limit).await
    }
    async fn remove(&self, plugin: &PluginId) {
        self.logs.remove(plugin).await;
    }
}

#[derive(Debug)]
struct StdioSession(RunningService<RoleClient, PluginClient>);

#[async_trait]
impl Session for StdioSession {
    async fn list_tools(&self) -> Result<Vec<Tool>, String> {
        self.0
            .list_all_tools()
            .await
            .map_err(|error| error.to_string())
    }
    async fn call(
        &self,
        params: CallToolRequestParams,
        deadline: Duration,
    ) -> Result<CallToolResult, SessionError> {
        let sent = self
            .0
            .send_cancellable_request(
                ClientRequest::CallToolRequest(CallToolRequest::new(params)),
                PeerRequestOptions::with_timeout(deadline),
            )
            .await;
        let answer = match sent {
            Ok(handle) => handle.await_response().await,
            Err(error) => Err(error),
        };
        match answer {
            Ok(ServerResult::CallToolResult(result)) => Ok(result),
            Ok(_) => Err(SessionError::Unsupported),
            Err(error) => {
                let message = match error {
                    ServiceError::Timeout { timeout } => {
                        format!("the call did not answer in {} seconds", timeout.as_secs())
                    }
                    error => error.to_string(),
                };
                Err(self.0.service().overrun.error(message))
            }
        }
    }
    fn is_closed(&self) -> bool {
        self.0.is_closed() || self.0.is_transport_closed()
    }
    async fn close(&self) {
        self.0.cancellation_token().cancel();
    }
}

/// The Pagis side of one MCP connection. It consumes tools and
/// nothing else: sampling and elicitation are refused, and resources,
/// prompts and logging are ignored (ADR-0017).
#[derive(Clone)]
struct PluginClient {
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
