//! Plugin routes over the stores and services of an Embedder.

#![cfg(not(feature = "native"))]

mod support {
    pub mod agents;
}

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, Response, StatusCode};
use futures::StreamExt;
use object_store::ObjectStoreExt as _;
use pagis_core::*;
use pagis_plugin::{PackageFiles, PackageSource, SourceError, fake::FixedPackageSource};
use pagis_plugins::{HttpBody, McpHttp, Session, Spawn, StdioServers, ToolListChanged};
use serde_json::{Value, json};
use support::agents::Embedder;
use tower::ServiceExt;

async fn response(
    e: &Embedder,
    admin: bool,
    cookie: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let router = if admin {
        &e.routers.administration
    } else {
        &e.routers.product
    };
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()))
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

fn package(description: &str) -> PackageFiles {
    let mut files = PackageFiles::default();
    files
        .insert(
            "plugin.json",
            json!({
                "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
                "name": "weather", "description": description,
                "extensions": {"pagis": {"config": {"city": {"type": "string", "title": "City"}}}}
            })
            .to_string()
            .into_bytes(),
            false,
        )
        .unwrap();
    files
}

struct RefusingSource;
#[async_trait]
impl PackageSource for RefusingSource {
    async fn fetch(&self, _: &str, _: Option<&str>) -> Result<PackageFiles, SourceError> {
        Err(SourceError::Refused("This source is refused.".into()))
    }
}

#[derive(Default)]
struct Logs(Mutex<Vec<(WorkspaceId, PluginId, usize)>>);
#[async_trait]
impl StdioServers for Logs {
    async fn connect(
        &self,
        _: &WorkspaceId,
        _: &PluginId,
        _: &str,
        _: &Spawn,
        _: Arc<dyn ToolListChanged>,
    ) -> Result<Arc<dyn Session>, String> {
        panic!("the package has no stdio server")
    }
    async fn tail(&self, workspace: &WorkspaceId, plugin: &PluginId, limit: usize) -> String {
        self.0
            .lock()
            .unwrap()
            .push((workspace.clone(), plugin.clone(), limit));
        format!("Log of {workspace}")
    }
    async fn remove(&self, _: &PluginId) {}
}

#[derive(Default)]
struct Http(Mutex<Vec<String>>);
#[async_trait]
impl McpHttp for Http {
    async fn send(&self, request: Request<Vec<u8>>) -> Result<Response<HttpBody>, String> {
        assert_eq!(request.uri(), "https://example.com/mcp");
        if request.method() == "DELETE" {
            return Ok(Response::new(futures::stream::empty().boxed()));
        }
        let message: Value = serde_json::from_slice(request.body()).unwrap();
        let method = message["method"].as_str().unwrap();
        self.0.lock().unwrap().push(method.into());
        let result = match method {
            "initialize" => {
                json!({"protocolVersion": "2025-11-25", "capabilities": {"tools": {}}, "serverInfo": {"name": "weather", "version": "1"}})
            }
            "notifications/initialized" => {
                return Ok(Response::builder()
                    .status(202)
                    .body(futures::stream::empty().boxed())
                    .unwrap());
            }
            "tools/list" => {
                json!({"tools": [{"name": "forecast", "inputSchema": {"type": "object"}}]})
            }
            _ => panic!("unexpected MCP request {method}"),
        };
        let answer = json!({"jsonrpc": "2.0", "id": message["id"], "result": result});
        Ok(Response::builder()
            .header("content-type", "application/json")
            .body(futures::stream::iter([Ok(answer.to_string().into())]).boxed())
            .unwrap())
    }
}

async fn embedder(source: Arc<dyn PackageSource>) -> Embedder {
    Embedder::with_plugins(source, Arc::new(Logs::default()), Arc::new(Http::default())).await
}
async fn fixed() -> (Embedder, Arc<FixedPackageSource>) {
    let source = Arc::new(FixedPackageSource::new(package("Weather")));
    (embedder(source.clone()).await, source)
}
async fn install(e: &Embedder, source: Value) -> Value {
    let (status, body) = response(
        e,
        true,
        &e.cookie,
        "POST",
        "/api/v1/plugins",
        json!({"source": source, "bindings": []}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}
async fn from_source(e: &Embedder) -> Value {
    install(
        e,
        json!({"kind": "git", "url": "https://example.com/weather.git", "ref": "stable"}),
    )
    .await
}
async fn upload(e: &Embedder, workspace: &WorkspaceId, files: &PackageFiles) -> ArtifactId {
    let mut archive = tar::Builder::new(Vec::new());
    for (path, file) in files.iter() {
        let mut header = tar::Header::new_gnu();
        header.set_size(file.bytes.len() as u64);
        header.set_mode(if file.executable { 0o755 } else { 0o644 });
        header.set_cksum();
        archive
            .append_data(&mut header, path, file.bytes.as_slice())
            .unwrap();
    }
    let tar = archive.into_inner().unwrap();
    let id = ArtifactId::generate();
    let key = format!("{workspace}/{id}");
    e.routers
        .state
        .blobs
        .put(
            &object_store::path::Path::from(key.clone()),
            tar.clone().into(),
        )
        .await
        .unwrap();
    e.stores
        .artifacts
        .insert(&Artifact {
            id: id.clone(),
            workspace_id: workspace.clone(),
            kind: ArtifactKind::File,
            creator_agent_id: None,
            run_id: None,
            filename: Some("weather.tar".into()),
            mime: "application/x-tar".into(),
            size_bytes: tar.len() as i64,
            sha256: "test".into(),
            storage_key: key,
            created_at: now_ms(),
        })
        .await
        .unwrap();
    id
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn installs_from_the_embedders_source() {
    let (e, source) = fixed().await;
    let body = from_source(&e).await;
    assert_eq!(body["name"], "weather");
    assert_eq!(body["source_kind"], "git");
    assert_eq!(body["source_ref"], "stable");
    assert_eq!(
        source.requests(),
        [(
            "https://example.com/weather.git".into(),
            Some("stable".into())
        )]
    );
    let id = PluginId::from(body["id"].as_str().unwrap().to_string());
    assert!(
        e.stores
            .plugins
            .get(&e.routers.state.org_workspace_id, &id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        e.stores
            .plugins
            .get(&e.workspace, &id)
            .await
            .unwrap()
            .is_none()
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn installs_an_uploaded_package_without_a_source() {
    let e = embedder(Arc::new(RefusingSource)).await;
    let artifact = upload(&e, &e.workspace, &package("Upload")).await;
    let body = install(&e, json!({"kind": "upload", "artifact_id": artifact})).await;
    assert_eq!(body["source_kind"], "upload");
    assert_eq!(body["state"], "enabled");
    let path = format!("/api/v1/plugins/{}", body["id"].as_str().unwrap());
    let (status, detail) = response(&e, false, &e.cookie, "GET", &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail, body);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn refuses_a_source_with_422() {
    let e = embedder(Arc::new(RefusingSource)).await;
    let (status, body) = response(
        &e,
        true,
        &e.cookie,
        "POST",
        "/api/v1/plugins",
        json!({"source": {"kind": "git", "url": "https://example.com/weather.git"}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["message"], "This source is refused.");
    assert!(
        e.stores
            .plugins
            .list(&e.routers.state.org_workspace_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn grants_only_to_an_agent_in_the_persons_workspace_and_uninstalls() {
    let (e, _) = fixed().await;
    let body = from_source(&e).await;
    let plugin = body["id"].as_str().unwrap();
    let own = e.agent(&e.workspace, "Ada").await;
    let other = e.agent(&e.other_workspace, "Grace").await;
    let path = format!("/api/v1/plugins/{plugin}/grants");
    let (status, _) = response(
        &e,
        false,
        &e.cookie,
        "POST",
        &path,
        json!({"agent_id": other.id}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for (cookie, agent) in [(&e.cookie, &own), (&e.other_cookie, &other)] {
        let (status, grant) = response(
            &e,
            false,
            cookie,
            "POST",
            &path,
            json!({"agent_id": agent.id}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{grant}");
        assert_eq!(grant["resource_kind"], "plugin");
        assert_eq!(grant["resource_id"], plugin);
        let held = e
            .stores
            .grants
            .get(
                &agent.workspace_id,
                &GrantId::from(grant["id"].as_str().unwrap().to_string()),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held.agent_id, agent.id);
    }
    let (status, _) = response(
        &e,
        true,
        &e.cookie,
        "DELETE",
        &format!("/api/v1/plugins/{plugin}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    for agent in [&own, &other] {
        assert!(
            e.stores
                .grants
                .list_live_for_agent(&agent.workspace_id, &agent.id)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert!(e.plugin_states.is_empty());
    let (status, _) = response(
        &e,
        false,
        &e.cookie,
        "GET",
        &format!("/api/v1/plugins/{plugin}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn lists_the_orgs_plugins_for_each_person() {
    let (e, _) = fixed().await;
    let body = from_source(&e).await;
    let mut private = e
        .stores
        .plugins
        .get(
            &e.routers.state.org_workspace_id,
            &PluginId::from(body["id"].as_str().unwrap().to_string()),
        )
        .await
        .unwrap()
        .unwrap();
    private.id = PluginId::generate();
    private.workspace_id = e.workspace.clone();
    private.name = "private".into();
    e.stores.plugins.create(&private, &[]).await.unwrap();
    for (admin, cookie) in [
        (false, &e.cookie),
        (false, &e.other_cookie),
        (true, &e.cookie),
    ] {
        let (status, list) =
            response(&e, admin, cookie, "GET", "/api/v1/plugins", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        assert_eq!(list["items"][0]["id"], body["id"]);
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn reads_only_the_persons_log_and_requires_an_installed_plugin() {
    let logs = Arc::new(Logs::default());
    let e = Embedder::with_plugins(
        Arc::new(FixedPackageSource::new(package("Weather"))),
        logs.clone(),
        Arc::new(Http::default()),
    )
    .await;
    let body = from_source(&e).await;
    let plugin = PluginId::from(body["id"].as_str().unwrap().to_string());
    let path = format!("/api/v1/plugins/{plugin}/log");
    for (cookie, workspace) in [
        (&e.cookie, &e.workspace),
        (&e.other_cookie, &e.other_workspace),
    ] {
        let (status, body) = response(&e, false, cookie, "GET", &path, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["text"], format!("Log of {workspace}"));
    }
    assert_eq!(
        *logs.0.lock().unwrap(),
        [
            (e.workspace.clone(), plugin.clone(), 64 * 1024),
            (e.other_workspace.clone(), plugin, 64 * 1024)
        ]
    );
    let (status, _) = response(
        &e,
        false,
        &e.cookie,
        "GET",
        "/api/v1/plugins/missing/log",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(logs.0.lock().unwrap().len(), 2);
    let (status, _) = response(&e, true, &e.cookie, "GET", &path, Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn updates_from_a_source_and_binds_a_field() {
    let (e, source) = fixed().await;
    let installed = from_source(&e).await;
    let plugin = installed["id"].as_str().unwrap();
    source.set(package("Updated weather"));
    let (status, updated) = response(
        &e,
        true,
        &e.cookie,
        "POST",
        &format!("/api/v1/plugins/{plugin}/update"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_ne!(updated["installed_commit"], installed["installed_commit"]);
    assert_eq!(
        updated["changed"],
        json!([{"path": "plugin.json", "status": "modified"}])
    );
    let (status, bound) = response(
        &e,
        true,
        &e.cookie,
        "PUT",
        &format!("/api/v1/plugins/{plugin}/bindings/city"),
        json!({"kind": "value", "value": "Paris"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bound}");
    assert_eq!(bound["bindings"][0]["value"], "Paris");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn freezes_and_starts_http_through_the_embedders_transport() {
    let mut files = package("HTTP weather");
    files.insert("mcp.json", json!({"$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json", "mcpServers": {"weather": {"type": "streamable-http", "url": "https://example.com/mcp"}}}).to_string().into_bytes(), false).unwrap();
    let http = Arc::new(Http::default());
    let e = Embedder::with_plugins(
        Arc::new(FixedPackageSource::new(files)),
        Arc::new(pagis_plugins::NoStdioServers),
        http.clone(),
    )
    .await;
    let body = from_source(&e).await;
    assert_eq!(body["frozen_tools"][0]["name"], "weather__forecast");
    let (status, started) = response(
        &e,
        true,
        &e.cookie,
        "POST",
        &format!("/api/v1/plugins/{}/start", body["id"].as_str().unwrap()),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    assert_eq!(started["servers"][0]["running"], true);
    assert!(
        http.0
            .lock()
            .unwrap()
            .iter()
            .any(|method| method == "tools/list")
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn refuses_another_workspaces_upload() {
    let (e, _) = fixed().await;
    let artifact = upload(&e, &e.other_workspace, &package("Private")).await;
    let (status, _) = response(
        &e,
        true,
        &e.cookie,
        "POST",
        "/api/v1/plugins",
        json!({"source": {"kind": "upload", "artifact_id": artifact}}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
