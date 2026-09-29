//! One MCP server that behaves badly on demand (ADR-0017).
//!
//! The stored tests of the plugin MCP host need a real server on the
//! other side of a real transport: a subprocess over stdio, and a
//! streamable HTTP endpoint on the loopback interface. This crate is
//! that server. Every tool it offers exists to drive one rule of
//! ADR-0017: a call that never answers drives the per-call timeout, a
//! large answer drives the broker's output cap, an exit drives the
//! restart path, a dropped tool drives the frozen manifest, a stdout
//! line with no end drives the frame cap, and stderr without end drives
//! the size of the Plugin log.
//!
//! The tests of the host live beside it, in this crate's `tests`
//! directory, because Cargo builds a binary only for the tests of the
//! package that declares it.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::ErrorData as McpError;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, ServiceExt};

/// The environment variable that names the file whose presence drops
/// one tool from the list. A test writes the file and restarts the
/// server to reach the "the tools changed" path of ADR-0017.
pub const DROP_FILE: &str = "PAGIS_FIXTURE_DROP_FILE";

/// What the fixture answers `tools/list` with, and what each tool
/// does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixtureTool {
    /// Answer with the text of the call.
    Echo,
    /// Wait for the seconds of the call and then answer.
    Sleep,
    /// Answer with a string of the length the call names.
    Blob,
    /// Leave, without an answer.
    Crash,
    /// Answer with the whole environment of the process.
    Environment,
    /// Answer with the headers of the HTTP request, or with an empty
    /// object over stdio.
    Headers,
    /// Drop [`FixtureTool::Blob`] from the list, send
    /// `notifications/tools/list_changed`, and then wait for the seconds
    /// of the call before the answer. The notification goes out before
    /// the answer, so a host sees it while the call still runs.
    DropTool,
    /// Write one stdout line of the size the call names, with no
    /// newline after it, and never answer.
    Flood,
    /// Write lines to stderr until the number of bytes the call names
    /// is written, and then answer. Each line is the text of the call,
    /// or 1023 bytes of `e` when the call names no text.
    Stderr,
}

impl FixtureTool {
    const ALL: [FixtureTool; 9] = [
        FixtureTool::Echo,
        FixtureTool::Sleep,
        FixtureTool::Blob,
        FixtureTool::Crash,
        FixtureTool::Environment,
        FixtureTool::Headers,
        FixtureTool::DropTool,
        FixtureTool::Flood,
        FixtureTool::Stderr,
    ];

    fn name(self) -> &'static str {
        match self {
            FixtureTool::Echo => "echo",
            FixtureTool::Sleep => "sleep",
            FixtureTool::Blob => "blob",
            FixtureTool::Crash => "crash",
            FixtureTool::Environment => "environment",
            FixtureTool::Headers => "headers",
            FixtureTool::DropTool => "drop_tool",
            FixtureTool::Flood => "flood",
            FixtureTool::Stderr => "stderr",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        FixtureTool::ALL
            .into_iter()
            .find(|tool| tool.name() == name)
    }

    fn description(self) -> &'static str {
        match self {
            FixtureTool::Echo => "Answer with the text of the call.",
            FixtureTool::Sleep => "Wait for the given seconds, then answer.",
            FixtureTool::Blob => "Answer with a string of the given length.",
            FixtureTool::Crash => "Leave without an answer.",
            FixtureTool::Environment => "Answer with the environment of the server.",
            FixtureTool::Headers => "Answer with the headers of the request.",
            FixtureTool::DropTool => {
                "Drop the blob tool from the list, say so, then wait for the given seconds."
            }
            FixtureTool::Flood => "Write one stdout line of the given size with no end.",
            FixtureTool::Stderr => "Write the given text or number of bytes to stderr.",
        }
    }

    fn schema(self) -> serde_json::Value {
        match self {
            FixtureTool::Echo => serde_json::json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
            }),
            FixtureTool::Sleep => serde_json::json!({
                "type": "object",
                "properties": {"seconds": {"type": "number"}},
                "required": ["seconds"],
            }),
            FixtureTool::DropTool => serde_json::json!({
                "type": "object",
                "properties": {"seconds": {"type": "number"}},
            }),
            FixtureTool::Blob | FixtureTool::Flood => serde_json::json!({
                "type": "object",
                "properties": {"size": {"type": "integer"}},
                "required": ["size"],
            }),
            FixtureTool::Stderr => serde_json::json!({
                "type": "object",
                "properties": {"size": {"type": "integer"}, "text": {"type": "string"}},
            }),
            FixtureTool::Crash | FixtureTool::Environment | FixtureTool::Headers => {
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                })
            }
        }
    }

    fn definition(self) -> Tool {
        let schema = self.schema();
        let object = schema
            .as_object()
            .cloned()
            .expect("a fixture schema is an object");
        Tool::new(
            Cow::Borrowed(self.name()),
            Cow::Borrowed(self.description()),
            Arc::new(object),
        )
    }
}

/// The fixture server. One instance serves one connection.
#[derive(Debug, Default)]
pub struct Fixture {
    /// Set by `drop_tool` for the life of this connection.
    dropped: Arc<std::sync::atomic::AtomicBool>,
    /// The tool calls that reached the server, over every connection
    /// of one router.
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl Fixture {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the blob tool is out of the list: the file the
    /// environment names is there, or `drop_tool` ran.
    fn drops_blob(&self) -> bool {
        if self.dropped.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }
        std::env::var(DROP_FILE)
            .ok()
            .is_some_and(|path| std::path::Path::new(&path).exists())
    }

    fn tools(&self) -> Vec<Tool> {
        FixtureTool::ALL
            .into_iter()
            .filter(|tool| !(self.drops_blob() && *tool == FixtureTool::Blob))
            .map(FixtureTool::definition)
            .collect()
    }
}

impl ServerHandler for Fixture {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let Some(tool) = FixtureTool::parse(&request.name) else {
            return Err(McpError::invalid_params("no such tool", None));
        };
        let arguments = request
            .arguments
            .map(serde_json::Value::Object)
            .unwrap_or_else(|| serde_json::json!({}));
        let text = match tool {
            FixtureTool::Echo => arguments["text"].as_str().unwrap_or_default().to_string(),
            FixtureTool::Sleep => {
                let seconds = arguments["seconds"].as_f64().unwrap_or_default();
                tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)).await;
                format!("slept {seconds}")
            }
            FixtureTool::Blob => {
                let size = arguments["size"].as_u64().unwrap_or_default() as usize;
                "b".repeat(size)
            }
            FixtureTool::Crash => {
                // The answer never arrives: the process leaves first.
                std::process::exit(9);
            }
            FixtureTool::Environment => {
                let held: BTreeMap<String, String> = std::env::vars().collect();
                serde_json::to_string(&held).unwrap_or_default()
            }
            FixtureTool::Headers => {
                let held: BTreeMap<String, String> = context
                    .extensions
                    .get::<http::request::Parts>()
                    .map(|parts| {
                        parts
                            .headers
                            .iter()
                            .map(|(name, value)| {
                                (
                                    name.as_str().to_string(),
                                    value.to_str().unwrap_or_default().to_string(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                serde_json::to_string(&held).unwrap_or_default()
            }
            FixtureTool::DropTool => {
                self.dropped
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = context.peer.notify_tool_list_changed().await;
                let seconds = arguments["seconds"].as_f64().unwrap_or_default();
                tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)).await;
                "the blob tool is dropped".to_string()
            }
            FixtureTool::Flood => {
                use tokio::io::AsyncWriteExt;

                let size = arguments["size"].as_u64().unwrap_or_default() as usize;
                let mut stdout = tokio::io::stdout();
                // No newline ends the line, and no answer follows it. A
                // host that ends the session closes the pipe, so the
                // write can fail.
                let _ = stdout.write_all(&vec![b'x'; size]).await;
                let _ = stdout.flush().await;
                std::future::pending::<String>().await
            }
            FixtureTool::Stderr => {
                use tokio::io::AsyncWriteExt;

                let size = arguments["size"].as_u64().unwrap_or(1) as usize;
                let text = arguments["text"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| "e".repeat(1023));
                let line = format!("{text}\n");
                let mut stderr = tokio::io::stderr();
                let mut written = 0;
                while written < size {
                    if stderr.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                    written += line.len();
                }
                let _ = stderr.flush().await;
                format!("wrote {written} bytes to stderr")
            }
        };
        Ok(CallToolResponse::Complete(CallToolResult::success(vec![
            ContentBlock::text(text),
        ])))
    }
}

/// Serve one connection over stdio, until the client closes it.
pub async fn serve_stdio() -> std::io::Result<()> {
    let service = Fixture::new()
        .serve(rmcp::transport::stdio())
        .await
        .map_err(std::io::Error::other)?;
    service.waiting().await.map_err(std::io::Error::other)?;
    Ok(())
}

/// The streamable HTTP endpoint, as an axum router at `/mcp`.
pub fn http_router() -> axum::Router {
    counting_http_router(Arc::default())
}

/// The streamable HTTP endpoint, which adds one to `calls` for each tool
/// call that reaches it. A test reads the count to prove that a call did
/// not reach the server.
pub fn counting_http_router(calls: Arc<std::sync::atomic::AtomicUsize>) -> axum::Router {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpService, session::local::LocalSessionManager,
    };

    let service = StreamableHttpService::new(
        move || {
            Ok(Fixture {
                calls: Arc::clone(&calls),
                ..Fixture::default()
            })
        },
        Arc::new(LocalSessionManager::default()),
        Default::default(),
    );
    axum::Router::new().route_service("/mcp", service)
}
