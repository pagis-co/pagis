//! Plugin calls reach an MCP fixture through the portable routers.

#[allow(dead_code)]
#[path = "conversation.rs"]
mod conversation;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use futures::StreamExt;
use pagis_agent::{Brain, BrainError, TurnRequest, TurnRole, TurnStream};
use pagis_core::*;
use pagis_plugin::{PackageFiles, fake::FixedPackageSource};
use pagis_plugins::{HttpBody, McpHttp, NoStdioServers};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{Embedder, install, response};
use conversation::Model;

struct MemoryHttp {
    router: axum::Router,
    calls: Arc<AtomicUsize>,
}

impl MemoryHttp {
    fn new() -> Self {
        let calls = Arc::new(AtomicUsize::default());
        Self {
            router: pagis_mcp_fixture::counting_http_router(calls.clone()),
            calls,
        }
    }
}

#[async_trait]
impl McpHttp for MemoryHttp {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<HttpBody>, String> {
        assert_eq!(request.uri(), "http://127.0.0.1/mcp");
        let response = self
            .router
            .clone()
            .oneshot(request.map(Body::from))
            .await
            .unwrap();
        Ok(response.map(|body| {
            body.into_data_stream()
                .map(|item| item.map_err(|error| error.to_string()))
                .boxed()
        }))
    }
}

fn package() -> PackageFiles {
    let mut files = PackageFiles::default();
    for (path, value) in [
        (
            "plugin.json",
            json!({
                "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
                "name": "fixture",
                "extensions": {"pagis": {"tools": {"echo": {"effect": "free"}}}}
            }),
        ),
        (
            "mcp.json",
            json!({
                "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
                "mcpServers": {"fixture": {"type": "streamable-http", "url": "http://127.0.0.1/mcp"}}
            }),
        ),
    ] {
        files
            .insert(path, value.to_string().into_bytes(), false)
            .unwrap();
    }
    files
}

async fn setup(
    brain: Arc<dyn Brain>,
    model: Arc<Model>,
) -> (Embedder, Arc<MemoryHttp>, Agent, String) {
    let source = Arc::new(FixedPackageSource::new(package()));
    let http = Arc::new(MemoryHttp::new());
    let e = Embedder::with_services(
        brain,
        model.clone(),
        model,
        source.clone(),
        Arc::new(NoStdioServers),
        http.clone(),
        Arc::new(pagis_server::NoOrgMenu),
    )
    .await;
    e.stores
        .model_aliases
        .create(&ModelAlias {
            id: ModelAliasId::generate(),
            workspace_id: e.workspace.clone(),
            alias: DEFAULT_MODEL_ALIAS.into(),
            candidates: vec!["anthropic/claude-sonnet-4-6".into()],
            created_at: 0,
            updated_at: 0,
        })
        .await
        .unwrap();
    let installed = install(
        &e,
        json!({"kind": "git", "url": "https://example.com/fixture.git", "ref": "stable"}),
    )
    .await;
    assert_eq!(
        source.requests(),
        [(
            "https://example.com/fixture.git".into(),
            Some("stable".into())
        )]
    );
    assert!(
        installed["frozen_tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "fixture__echo")
    );
    assert_eq!(http.calls.load(Ordering::SeqCst), 0);
    let agent = e.agent(&e.workspace, "Ada").await;
    (e, http, agent, installed["id"].as_str().unwrap().into())
}

async fn grant(e: &Embedder, agent: &Agent, plugin: &str) -> Value {
    let (status, grant) = response(
        e,
        false,
        &e.cookie,
        "POST",
        &format!("/api/v1/plugins/{plugin}/grants"),
        json!({"agent_id": agent.id}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    assert_eq!(grant["resource_kind"], Grant::PLUGIN_KIND);
    assert_eq!(grant["resource_id"], plugin);
    grant
}

async fn run(e: &Embedder, agent: &Agent) -> Run {
    let (status, channel) = response(
        e,
        false,
        &e.cookie,
        "POST",
        "/api/v1/channels",
        json!({"title": "Echo", "agent_ids": [agent.id]}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let (status, message) = response(
        e,
        false,
        &e.cookie,
        "POST",
        &format!(
            "/api/v1/channels/{}/messages",
            channel["id"].as_str().unwrap()
        ),
        json!({"pending_id": "echo", "text": "@Ada Echo the text."}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{message}");
    pagis_rt::timeout(Duration::from_secs(5), async {
        loop {
            let runs = e
                .stores
                .runs
                .list(&e.workspace, None, None, &[], None, 10)
                .await
                .unwrap();
            if let Some(run) = runs.into_iter().find(|run| run.state.is_terminal()) {
                assert_eq!(run.state, RunState::Completed, "{run:?}");
                assert_eq!(run.agent_id, agent.id);
                assert_eq!(run.trigger_ref.as_deref(), message["id"].as_str());
                return run;
            }
            pagis_rt::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the Run completes without an Approval")
}

async fn completed(e: &Embedder, run: &Run) -> Value {
    let (status, events) = response(
        e,
        false,
        &e.cookie,
        "GET",
        &format!("/api/v1/runs/{}/events", run.id),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events}");
    events["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| {
            event["event_type"] == "tool.completed" && event["payload"]["name"] == "fixture__echo"
        })
        .expect("the Broker records the Plugin call")["payload"]
        .clone()
}

async fn no_approval(e: &Embedder) {
    let (status, requests) =
        response(e, false, &e.cookie, "GET", "/api/v1/requests", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{requests}");
    assert_eq!(requests["items"], json!([]));
}

#[tokio::test]
async fn installs_grants_and_calls_the_http_plugin_without_a_port() {
    let model = Arc::new(Model::default());
    model.tool(
        "fixture__echo",
        json!({"text": "The fixture answers in memory."}),
    );
    model.reply("The call completes.");
    let (e, http, agent, plugin) = setup(model.clone(), model.clone()).await;
    let grant = grant(&e, &agent, &plugin).await;
    let run = run(&e, &agent).await;
    assert_eq!(http.calls.load(Ordering::SeqCst), 1);
    let event = completed(&e, &run).await;
    assert_eq!(event["authorization"], "free");
    assert_eq!(event["selected_connection"], plugin);
    assert_eq!(event["grant_revision"], grant["revision"]);
    assert_eq!(event["outcome"], "completed");
    assert_eq!(event["ok"], true);
    assert_eq!(event["request_id"], Value::Null);
    let snapshot = e
        .stores
        .capability_snapshots
        .for_run(&e.workspace, &run.id)
        .await
        .unwrap()
        .unwrap();
    let echo = snapshot.content["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["manifest"]["definition"]["name"] == "fixture__echo")
        .unwrap();
    assert_eq!(echo["grants"][0]["id"], grant["id"]);
    assert_eq!(echo["manifest"]["effect"], "free");
    no_approval(&e).await;
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "fixture__echo")
    );
    let answer = requests[1]
        .messages
        .iter()
        .find(|message| message.role == TurnRole::Tool)
        .unwrap();
    assert_eq!(answer.tool_call_id.as_deref(), Some("call_1"));
    assert!(
        answer.text.contains("The fixture answers in memory."),
        "{answer:?}"
    );
}

#[tokio::test]
async fn an_agent_without_a_plugin_grant_does_not_reach_the_http_server() {
    let model = Arc::new(Model::default());
    model.tool("fixture__echo", json!({"text": "This call is refused."}));
    model.reply("The call is refused.");
    let (e, http, agent, plugin) = setup(model.clone(), model.clone()).await;
    let other = e.agent(&e.workspace, "Grace").await;
    grant(&e, &other, &plugin).await;
    let run = run(&e, &agent).await;
    assert_eq!(http.calls.load(Ordering::SeqCst), 0);
    let event = completed(&e, &run).await;
    assert_eq!(event["authorization"], "rejected");
    assert_eq!(event["outcome"], "rejected");
    assert_eq!(event["ok"], false);
    assert_eq!(event["error_code"], "invalid_request");
    no_approval(&e).await;
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0]
            .tools
            .iter()
            .all(|tool| tool.name != "fixture__echo")
    );
    let answer = requests[1]
        .messages
        .iter()
        .find(|message| message.role == TurnRole::Tool)
        .unwrap();
    assert_eq!(answer.tool_call_id.as_deref(), Some("call_1"));
    assert!(
        answer.text.contains("unknown tool: fixture__echo"),
        "{answer:?}"
    );
}

struct RevokingModel {
    model: Arc<Model>,
    grant: Mutex<Option<(Arc<dyn GrantStore>, Grant)>>,
}

#[async_trait]
impl Brain for RevokingModel {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        let grant = self.grant.lock().unwrap().take();
        if let Some((store, grant)) = grant {
            assert!(
                request
                    .tools
                    .iter()
                    .any(|tool| tool.name == "fixture__echo")
            );
            assert!(
                store
                    .revoke(&grant.workspace_id, &grant.id, now_ms())
                    .await
                    .unwrap()
            );
        }
        self.model.turn(request).await
    }
}

#[tokio::test]
async fn a_revoked_plugin_grant_blocks_a_call_from_an_existing_snapshot() {
    let model = Arc::new(Model::default());
    model.tool("fixture__echo", json!({"text": "This call is refused."}));
    model.reply("The call is refused.");
    let brain = Arc::new(RevokingModel {
        model: model.clone(),
        grant: Mutex::default(),
    });
    let (e, http, agent, plugin) = setup(brain.clone(), model.clone()).await;
    let grant = grant(&e, &agent, &plugin).await;
    let held = e
        .stores
        .grants
        .get(
            &e.workspace,
            &GrantId::from(grant["id"].as_str().unwrap().to_string()),
        )
        .await
        .unwrap()
        .unwrap();
    *brain.grant.lock().unwrap() = Some((e.stores.grants.clone(), held));
    let run = run(&e, &agent).await;
    assert_eq!(http.calls.load(Ordering::SeqCst), 0);
    let event = completed(&e, &run).await;
    assert_eq!(event["authorization"], "revoked");
    assert_eq!(event["outcome"], "rejected");
    assert_eq!(event["ok"], false);
    assert_eq!(event["error_code"], "permission_revoked");
    no_approval(&e).await;
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let answer = requests[1]
        .messages
        .iter()
        .find(|message| message.role == TurnRole::Tool)
        .unwrap();
    assert!(answer.text.contains("permission_revoked"), "{answer:?}");
}
