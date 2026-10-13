//! The Plugin Supervisor over an HTTP router with no listener.

use pagis_rt::Instant;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use futures::StreamExt;
use http::{Method, Request, Response};
use pagis_core::{PluginId, PluginTool, WorkspaceId};
use pagis_plugins::{
    Endpoint, HttpBody, Launch, MAX_FRAME_BYTES, McpHttp, NoStdioServers, ServerError,
    ServerSupervisor, ToolListChanged,
};
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Default)]
struct Changed(AtomicUsize);
#[async_trait]
impl ToolListChanged for Changed {
    async fn tool_list_changed(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct MemoryHttp {
    router: axum::Router,
    requests: Mutex<Vec<Request<Vec<u8>>>>,
    expire: AtomicBool,
    stall_initialize: AtomicBool,
    answers: Mutex<VecDeque<(String, Vec<u8>)>>,
}

impl MemoryHttp {
    fn new() -> Self {
        Self {
            router: pagis_mcp_fixture::http_router(),
            requests: Mutex::default(),
            expire: AtomicBool::new(false),
            stall_initialize: AtomicBool::new(false),
            answers: Mutex::default(),
        }
    }

    fn messages(&self) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter_map(|request| serde_json::from_slice(request.body()).ok())
            .collect()
    }
}

#[async_trait]
impl McpHttp for MemoryHttp {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<HttpBody>, String> {
        assert_ne!(request.method(), Method::GET);
        if self.expire.swap(false, Ordering::SeqCst) {
            let mut delete = Request::builder()
                .method(Method::DELETE)
                .uri(request.uri())
                .body(Body::empty())
                .unwrap();
            *delete.headers_mut() = request.headers().clone();
            assert!(
                self.router
                    .clone()
                    .oneshot(delete)
                    .await
                    .unwrap()
                    .status()
                    .is_success()
            );
        }
        self.requests.lock().unwrap().push(request.clone());
        let message: Value = serde_json::from_slice(request.body()).unwrap_or(Value::Null);
        if message["method"] == "initialize" && self.stall_initialize.load(Ordering::SeqCst) {
            return std::future::pending().await;
        }
        if message.get("result").is_some() || message.get("error").is_some() {
            return Ok(Response::builder()
                .status(202)
                .body(futures::stream::empty().boxed())
                .unwrap());
        }
        if matches!(
            message["method"].as_str(),
            Some("tools/list" | "tools/call")
        ) {
            let answer = self.answers.lock().unwrap().pop_front();
            if let Some((content_type, data)) = answer {
                let data = String::from_utf8(data)
                    .unwrap()
                    .replace("CALL_ID", &message["id"].to_string());
                return Ok(Response::builder()
                    .header("content-type", content_type)
                    .body(futures::stream::iter([Ok(data.into())]).boxed())
                    .unwrap());
            }
        }
        let response = self
            .router
            .clone()
            .oneshot(request.map(Body::from))
            .await
            .unwrap();
        Ok(response.map(|body| {
            body.into_data_stream()
                .map(|item| item.map_err(|e| e.to_string()))
                .boxed()
        }))
    }
}

struct Harness {
    supervisor: ServerSupervisor,
    http: Arc<MemoryHttp>,
    changed: Arc<Changed>,
    _directory: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let workspace = WorkspaceId::generate();
        let http = Arc::new(MemoryHttp::new());
        let changed = Arc::new(Changed::default());
        let supervisor = ServerSupervisor::new(
            "fixture",
            workspace.clone(),
            PluginId::from("fixture".to_string()),
            Arc::new(NoStdioServers),
            changed.clone(),
        )
        .with_http(http.clone());
        Self {
            supervisor,
            http,
            changed,
            _directory: directory,
        }
    }

    async fn client(&self) -> Arc<dyn pagis_plugins::Session> {
        let launch = Launch::Http(Endpoint {
            url: "http://127.0.0.1/mcp".to_string(),
            headers: BTreeMap::from([(
                "authorization".to_string(),
                "Basic exact-value".to_string(),
            )]),
        });
        self.supervisor
            .client(&launch, "same", Instant::now())
            .await
            .unwrap()
            .0
    }
}

#[tokio::test]
async fn freeze_gets_the_tools_and_stop_deletes_the_session() {
    let harness = Harness::new();
    let client = harness.client().await;
    let tools = harness
        .supervisor
        .list_tools(client.as_ref())
        .await
        .unwrap();
    let frozen = freeze(&tools);
    assert!(frozen.iter().any(|tool| tool.name == "echo"));
    assert!(frozen.iter().all(|tool| tool.server == "fixture"));
    {
        let requests = harness.http.requests.lock().unwrap();
        assert_eq!(requests[0].headers()["authorization"], "Basic exact-value");
        assert!(!requests[0].headers().contains_key("mcp-session-id"));
        for request in &requests[1..] {
            assert!(request.headers().contains_key("mcp-session-id"));
            assert_eq!(request.headers()["mcp-protocol-version"], "2025-11-25");
        }
    }
    assert_eq!(harness.http.messages()[0]["method"], "initialize");
    assert_eq!(
        harness.http.messages()[1]["method"],
        "notifications/initialized"
    );
    harness.supervisor.stop().await;
    assert_eq!(
        harness
            .http
            .requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .method(),
        Method::DELETE
    );
    assert!(!harness.supervisor.is_running().await);
}

#[tokio::test]
async fn a_call_gets_the_sse_answer() {
    let harness = Harness::new();
    let client = harness.client().await;
    let result = harness
        .supervisor
        .call(
            client.as_ref(),
            "echo",
            json!({"text":"in memory"}),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(result.content[0].as_text().unwrap().text, "in memory");
}

#[tokio::test]
async fn a_deadline_sends_cancellation_and_answers_unknown() {
    let harness = Harness::new();
    let client = harness.client().await;
    let result = harness
        .supervisor
        .call(
            client.as_ref(),
            "sleep",
            json!({"seconds":60}),
            Duration::from_millis(20),
        )
        .await;
    assert!(matches!(result, Err(ServerError::Unknown(_))), "{result:?}");
    let messages = harness.http.messages();
    let call = messages
        .iter()
        .find(|m| m["method"] == "tools/call")
        .unwrap();
    let cancel = messages
        .iter()
        .find(|m| m["method"] == "notifications/cancelled")
        .unwrap();
    assert_eq!(cancel["params"]["requestId"], call["id"]);
}

#[tokio::test]
async fn an_expired_session_initializes_again_before_the_call() {
    let harness = Harness::new();
    let client = harness.client().await;
    harness.http.expire.store(true, Ordering::SeqCst);
    let result = harness
        .supervisor
        .call(
            client.as_ref(),
            "echo",
            json!({"text":"new session"}),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(result.content[0].as_text().unwrap().text, "new session");
    assert_eq!(
        harness
            .http
            .messages()
            .iter()
            .filter(|m| m["method"] == "initialize")
            .count(),
        2
    );
}

#[tokio::test]
async fn a_call_answer_marks_the_tool_list_changed() {
    let harness = Harness::new();
    let client = harness.client().await;
    let frozen = freeze(
        &harness
            .supervisor
            .list_tools(client.as_ref())
            .await
            .unwrap(),
    );
    harness
        .supervisor
        .call(
            client.as_ref(),
            "drop_tool",
            json!({}),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(harness.changed.0.load(Ordering::SeqCst), 1);
    assert!(frozen.iter().any(|tool| tool.name == "blob"));
    assert!(
        !harness
            .supervisor
            .list_tools(client.as_ref())
            .await
            .unwrap()
            .iter()
            .any(|tool| tool.name == "blob")
    );
}

#[tokio::test]
async fn an_answer_over_the_cap_ends_the_session() {
    let harness = Harness::new();
    let client = harness.client().await;
    let result = harness
        .supervisor
        .call(
            client.as_ref(),
            "blob",
            json!({"size":MAX_FRAME_BYTES + 1}),
            Duration::from_secs(2),
        )
        .await;
    assert!(
        matches!(result, Err(ServerError::Overrun { failed: false, .. })),
        "{result:?}"
    );
    assert!(!harness.supervisor.is_running().await);
}

fn queue(harness: &Harness, content_type: &str, data: String) {
    harness
        .http
        .answers
        .lock()
        .unwrap()
        .push_back((content_type.to_string(), data.into_bytes()));
}

#[tokio::test]
async fn json_answers_and_tool_list_pages_are_read() {
    let harness = Harness::new();
    let client = harness.client().await;
    queue(&harness, "application/json; charset=utf-8", r#"{"jsonrpc":"2.0","id":CALL_ID,"result":{"tools":[{"name":"first","inputSchema":{"type":"object"}}],"nextCursor":"next"}}"#.to_string());
    queue(&harness, "application/json", r#"{"jsonrpc":"2.0","id":CALL_ID,"result":{"tools":[{"name":"second","inputSchema":{"type":"object"}}]}}"#.to_string());
    let tools = harness
        .supervisor
        .list_tools(client.as_ref())
        .await
        .unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(
        harness.http.messages().last().unwrap()["params"]["cursor"],
        "next"
    );
    queue(
        &harness,
        "application/json",
        r#"{"jsonrpc":"2.0","id":CALL_ID,"result":{"content":[{"type":"text","text":"JSON"}]}}"#
            .to_string(),
    );
    let result = harness
        .supervisor
        .call(client.as_ref(), "echo", json!({}), Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(result.content[0].as_text().unwrap().text, "JSON");
}

#[tokio::test]
async fn ping_is_answered_and_every_other_server_request_is_refused() {
    let harness = Harness::new();
    let client = harness.client().await;
    let mut events = String::new();
    for (id, method) in [
        ("ping-id", "ping"),
        ("sampling-id", "sampling/createMessage"),
        ("roots-id", "roots/list"),
        ("elicitation-id", "elicitation/create"),
        ("custom-id", "anything/new"),
    ] {
        events += &format!(
            "data: {{\"jsonrpc\":\"2.0\",\"id\":\"{id}\",\"method\":\"{method}\",\"params\":{{}}}}\n\n"
        );
    }
    events += "data: {\"jsonrpc\":\"2.0\",\"id\":CALL_ID,\"result\":{\"content\":[]}}\n\n";
    queue(&harness, "text/event-stream", events);
    harness
        .supervisor
        .call(client.as_ref(), "echo", json!({}), Duration::from_secs(1))
        .await
        .unwrap();
    let messages = harness.http.messages();
    assert_eq!(
        messages.iter().find(|m| m["id"] == "ping-id").unwrap()["result"],
        json!({})
    );
    for id in ["sampling-id", "roots-id", "elicitation-id", "custom-id"] {
        assert_eq!(
            messages.iter().find(|m| m["id"] == id).unwrap()["error"]["code"],
            -32601
        );
    }
}

#[tokio::test]
async fn a_json_body_over_the_cap_ends_the_session() {
    let harness = Harness::new();
    let client = harness.client().await;
    queue(
        &harness,
        "application/json",
        "x".repeat(MAX_FRAME_BYTES + 1),
    );
    let result = harness
        .supervisor
        .call(client.as_ref(), "echo", json!({}), Duration::from_secs(1))
        .await;
    assert!(
        matches!(result, Err(ServerError::Overrun { .. })),
        "{result:?}"
    );
}

#[tokio::test]
async fn an_unterminated_sse_event_over_the_cap_ends_the_session() {
    let harness = Harness::new();
    let client = harness.client().await;
    queue(
        &harness,
        "text/event-stream",
        format!("data: {}", "x".repeat(MAX_FRAME_BYTES + 1)),
    );
    let result = harness
        .supervisor
        .call(client.as_ref(), "echo", json!({}), Duration::from_secs(1))
        .await;
    assert!(
        matches!(result, Err(ServerError::Overrun { .. })),
        "{result:?}"
    );
}

fn freeze(tools: &[rmcp::model::Tool]) -> Vec<PluginTool> {
    let tools = tools
        .iter()
        .map(|tool| PluginTool {
            server: String::new(),
            name: tool.name.to_string(),
            description: tool.description.as_deref().unwrap_or_default().to_string(),
            schema: Value::Object((*tool.input_schema).clone()),
        })
        .collect();
    pagis_plugins::freeze_tools("fixture", &[("fixture".to_string(), tools)]).unwrap()
}

#[tokio::test]
async fn an_output_overrun_ends_every_pending_call_of_the_session() {
    let harness = Harness::new();
    let client = harness.client().await;
    let waiting = harness.supervisor.call(
        client.as_ref(),
        "sleep",
        json!({"seconds":60}),
        Duration::from_secs(10),
    );
    let overrun = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        harness
            .supervisor
            .call(
                client.as_ref(),
                "blob",
                json!({"size":MAX_FRAME_BYTES + 1}),
                Duration::from_secs(2),
            )
            .await
    };
    let answers = tokio::time::timeout(Duration::from_millis(500), async {
        futures::join!(waiting, overrun)
    })
    .await
    .expect("the frame cap ends all calls without waiting for their deadlines");
    assert!(
        matches!(answers.0, Err(ServerError::Overrun { failed: false, .. })),
        "{:?}",
        answers.0
    );
    assert!(
        matches!(answers.1, Err(ServerError::Overrun { failed: false, .. })),
        "{:?}",
        answers.1
    );
}

#[tokio::test]
async fn a_pending_handshake_does_not_delay_another_calls_deadline() {
    let harness = Harness::new();
    let client = harness.client().await;
    harness.http.expire.store(true, Ordering::SeqCst);
    harness.http.stall_initialize.store(true, Ordering::SeqCst);
    let restarting = harness.supervisor.call(
        client.as_ref(),
        "echo",
        json!({"text":"waiting"}),
        Duration::from_secs(60),
    );
    let limited = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let result = harness
            .supervisor
            .call(
                client.as_ref(),
                "echo",
                json!({"text":"deadline"}),
                Duration::from_millis(20),
            )
            .await;
        assert!(matches!(result, Err(ServerError::Unknown(_))), "{result:?}");
        let messages = harness.http.messages();
        let cancel = messages
            .iter()
            .find(|m| m["method"] == "notifications/cancelled")
            .expect("the deadline sends cancellation even during a handshake");
        let call = messages
            .iter()
            .find(|m| m["params"]["arguments"]["text"] == "deadline")
            .expect("the call request");
        assert_eq!(cancel["params"]["requestId"], call["id"]);
    };
    futures::pin_mut!(restarting, limited);
    let finished = tokio::time::timeout(
        Duration::from_millis(500),
        futures::future::select(restarting, limited),
    )
    .await
    .expect("the other call's handshake cannot extend the deadline");
    assert!(matches!(finished, futures::future::Either::Right(_)));
}
