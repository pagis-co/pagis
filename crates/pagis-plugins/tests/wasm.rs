//! The same HTTP Plugin tests run on the host and under Node.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use http::{Method, Request, Response};
use pagis_broker::{AuthorizedCall, CapabilityManifest, ToolExecutor, ToolRoute};
use pagis_core::{
    AgentId, Grant, GrantId, MemorySecretStore, Plugin, PluginId, PluginSource, PluginState, RunId,
    WorkspaceId, fake::FakeDatabase,
};
use pagis_plugin::{PackageFiles, PluginStates, check_access, fake::MemoryPluginStates, validate};
use pagis_plugins::{
    HttpBody, Manifests, McpHttp, NoConnectionTokens, NoStdioServers, PluginHosts, PluginHostsDeps,
    PluginToolRuntime,
};
use serde_json::{Value, json};

#[derive(Default)]
struct ScriptedHttp {
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl McpHttp for ScriptedHttp {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<HttpBody>, String> {
        assert_eq!(request.uri(), "https://example.com/mcp");
        assert_eq!(
            request.headers()["authorization"],
            "Bearer installed-secret"
        );
        if request.method() == Method::DELETE {
            return Ok(Response::new(futures::stream::empty().boxed()));
        }
        let message: Value = serde_json::from_slice(request.body()).unwrap();
        self.requests.lock().unwrap().push(message.clone());
        let result = match message["method"].as_str().unwrap() {
            "initialize" => {
                json!({"protocolVersion": "2025-11-25", "capabilities": {"tools": {}}, "serverInfo": {"name": "script", "version": "1"}})
            }
            "notifications/initialized" | "notifications/cancelled" => {
                return Ok(Response::builder()
                    .status(202)
                    .body(futures::stream::empty().boxed())
                    .unwrap());
            }
            "tools/list" => {
                json!({"tools": [{"name": "echo", "description": "Echo text", "inputSchema": {"type": "object", "properties": {"mode": {"type": "string"}}}}]})
            }
            "tools/call" => {
                if message["params"]["arguments"]["mode"] == "deadline" {
                    return Ok(Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(futures::stream::pending().boxed())
                        .unwrap());
                }
                json!({"content": [{"type": "text", "text": "scripted answer"}]})
            }
            other => panic!("unexpected method {other}"),
        };
        let answer = json!({"jsonrpc": "2.0", "id": message["id"], "result": result});
        let sse = message["params"]["arguments"]["mode"] == "sse";
        let bytes = if sse {
            format!("event: message\ndata: {answer}\n\n")
        } else {
            answer.to_string()
        };
        Ok(Response::builder()
            .header(
                "content-type",
                if sse {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .header("mcp-session-id", "script-session")
            .body(futures::stream::iter([Ok(bytes.into())]).boxed())
            .unwrap())
    }
}

#[derive(Default)]
struct HeldManifests(Mutex<Vec<CapabilityManifest>>);
impl Manifests for HeldManifests {
    fn install(&self, _: &WorkspaceId, manifest: CapabilityManifest) -> Result<(), String> {
        self.0.lock().unwrap().push(manifest);
        Ok(())
    }
}

async fn harness(
    stdio: bool,
) -> (
    Arc<PluginHosts>,
    Plugin,
    Arc<ScriptedHttp>,
    Arc<HeldManifests>,
    pagis_core::Stores,
) {
    let db = FakeDatabase::default();
    let stores = db.stores();
    let states = Arc::new(MemoryPluginStates::default());
    let workspace = WorkspaceId::generate();
    let mut files = PackageFiles::default();
    files.insert("plugin.json", json!({"$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json", "name": "weather"}).to_string().into_bytes(), false).unwrap();
    let server = if stdio {
        json!({"type": "stdio", "command": "weather"})
    } else {
        json!({"type": "streamable-http", "url": "https://example.com/mcp", "headers": {"authorization": "Bearer installed-secret"}})
    };
    files.insert("mcp.json", json!({"$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json", "mcpServers": {"weather": server}}).to_string().into_bytes(), false).unwrap();
    let mut plugin = Plugin {
        id: PluginId::generate(),
        workspace_id: workspace.clone(),
        name: "weather".into(),
        source: PluginSource::Upload,
        installed_commit: String::new(),
        manifest_version: "v1".into(),
        state: PluginState::Enabled,
        created_at: 0,
        updated_at: 0,
    };
    plugin.installed_commit = states
        .write(&workspace, plugin.id.as_str(), &files, "install")
        .await
        .unwrap();
    stores.plugins.create(&plugin, &[]).await.unwrap();
    let http = Arc::new(ScriptedHttp::default());
    let manifests = Arc::new(HeldManifests::default());
    let hosts = PluginHosts::new(PluginHostsDeps {
        org_workspace_id: workspace,
        workspaces: stores.workspaces.clone(),
        plugins: stores.plugins.clone(),
        catalogs: stores.plugin_tools.clone(),
        connections: stores.connections.clone(),
        grants: stores.grants.clone(),
        secrets: Arc::new(MemorySecretStore::default()),
        tokens: Arc::new(NoConnectionTokens),
        states,
        manifests: manifests.clone(),
        bus: Arc::new(db.bus()),
        stdio: Arc::new(NoStdioServers),
        http: http.clone(),
    });
    let package = validate(&files).unwrap();
    let frozen = hosts.org().freeze(&plugin, &package, &[]).await;
    if stdio {
        assert_eq!(
            frozen.unwrap_err().to_string(),
            "the server \"weather\" did not start: stdio Plugin servers are not available"
        );
    } else {
        let frozen = frozen.unwrap();
        assert_eq!(frozen.tools.len(), 1);
        assert_eq!(frozen.tools[0].name, "echo");
        assert_eq!(hosts.org().catalog(&plugin).await.unwrap().unwrap(), frozen);
    }
    (hosts, plugin, http, manifests, stores)
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn an_http_plugin_freezes_calls_json_and_sse_and_obeys_the_deadline() {
    let (hosts, plugin, http, manifests, stores) = harness(false).await;
    assert!(!manifests.0.lock().unwrap().is_empty());
    assert!(check_access(&plugin, &[], &[], &[]).is_err());
    let agent = AgentId::generate();
    let grant = Grant {
        id: GrantId::generate(),
        workspace_id: plugin.workspace_id.clone(),
        agent_id: agent.clone(),
        resource_kind: Grant::PLUGIN_KIND.into(),
        resource_id: Some(plugin.id.to_string()),
        scope: json!({}),
        revision: 1,
        created_at: 0,
        revoked_at: None,
    };
    stores.grants.create(&grant).await.unwrap();
    let runtime = PluginToolRuntime::new(hosts.clone());
    let call = |mode: &str| AuthorizedCall {
        workspace_id: plugin.workspace_id.clone(),
        agent_id: agent.clone(),
        run_id: RunId::generate(),
        tool_name: "weather__echo".into(),
        source_version: "v1".into(),
        route: ToolRoute::Plugin {
            plugin: plugin.id.to_string(),
            server: "weather".into(),
            tool: "echo".into(),
            on_computer: false,
        },
        arguments: json!({"mode": mode}),
        selected_connection: None,
        grant_id: Some(grant.id.clone()),
        grant_revision: Some(1),
        call_timeout: Some(Duration::from_millis(5)),
        tool_call_id: None,
        host: None,
        approved_by_rule: false,
    };
    for mode in ["json", "sse"] {
        let answer = runtime.execute(call(mode)).await;
        assert!(!answer.is_error, "{answer:?}");
        assert_eq!(answer.content, "scripted answer");
    }
    let answer = runtime.execute(call("deadline")).await;
    assert_eq!(answer.code.as_deref(), Some("outcome_unknown"));
    assert!(
        http.requests
            .lock()
            .unwrap()
            .iter()
            .any(|message| message["method"] == "notifications/cancelled")
    );
    stores
        .grants
        .revoke(&plugin.workspace_id, &grant.id, 1)
        .await
        .unwrap();
    let before = http.requests.lock().unwrap().len();
    assert_eq!(
        runtime.execute(call("json")).await.code.as_deref(),
        Some("permission_revoked")
    );
    assert_eq!(http.requests.lock().unwrap().len(), before);
    hosts
        .reap_idle(pagis_rt::Instant::now() + pagis_plugins::IDLE_SHUTDOWN)
        .await;
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_stdio_plugin_cannot_freeze_without_stdio_servers() {
    let (hosts, plugin, _, _, _) = harness(true).await;
    assert!(hosts.org().catalog(&plugin).await.unwrap().is_none());
    assert_eq!(hosts.org().read_log(&plugin.id, 100).await, "");
}

struct NoListChange;
#[async_trait]
impl pagis_plugins::ToolListChanged for NoListChange {
    async fn tool_list_changed(&self) {}
}

#[derive(Debug)]
struct UnsupportedAnswer;
#[async_trait]
impl pagis_plugins::Session for UnsupportedAnswer {
    async fn list_tools(&self) -> Result<Vec<rmcp::model::Tool>, String> {
        Ok(Vec::new())
    }
    async fn call(
        &self,
        params: rmcp::model::CallToolRequestParams,
        _: Duration,
    ) -> Result<rmcp::model::CallToolResult, pagis_plugins::SessionError> {
        if params.name == "overrun" {
            Err(pagis_plugins::SessionError::Overrun { first: true })
        } else {
            Err(pagis_plugins::SessionError::Unsupported)
        }
    }
    fn is_closed(&self) -> bool {
        false
    }
    async fn close(&self) {}
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn an_unsupported_answer_within_the_cap_breaks_the_overrun_sequence() {
    let supervisor = pagis_plugins::ServerSupervisor::new(
        "weather",
        WorkspaceId::generate(),
        PluginId::generate(),
        Arc::new(NoStdioServers),
        Arc::new(NoListChange),
    );
    for _ in 0..2 {
        assert!(matches!(
            supervisor
                .call(
                    &UnsupportedAnswer,
                    "overrun",
                    json!({}),
                    Duration::from_secs(1)
                )
                .await,
            Err(pagis_plugins::ServerError::Overrun { failed: false, .. })
        ));
    }
    assert!(matches!(
        supervisor
            .call(
                &UnsupportedAnswer,
                "unsupported",
                json!({}),
                Duration::from_secs(1)
            )
            .await,
        Err(pagis_plugins::ServerError::Unsupported(_))
    ));
    assert!(matches!(
        supervisor
            .call(
                &UnsupportedAnswer,
                "overrun",
                json!({}),
                Duration::from_secs(1)
            )
            .await,
        Err(pagis_plugins::ServerError::Overrun { failed: false, .. })
    ));
}
