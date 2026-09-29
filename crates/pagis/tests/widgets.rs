//! The Widget page route (ADR-0016). A Widget is a file inside
//! a Software Version, so the test seeds the Version the way a publish
//! does: the tree into the package's bare repository, and the record
//! into the store. The page route at the Product App origin then
//! serves the source as inert bytes under the immutable cache header a
//! Version earns. Only the sandbox proxy runs the page, under the
//! manifest's own Content-Security-Policy.
//!
//! The view's JSON-RPC needs a live Widget view, which only a Run that
//! called a widget tool can mint, so the parts of it that stand alone
//! are unit-tested inside `pagis-server` instead. What is reachable
//! from here is the 404 on an unknown view.
//!
//! The sandbox proxy is the one Widget route that reads no Session, so
//! its own test says what it serves and under which policy.

use pagis_core::{
    AgentStore, Run, RunId, RunState, RunStore, SoftwarePackage, SoftwarePackageId, SoftwareStore,
    SoftwareVersion, TriggerKind, WorkspaceStore, now_ms,
};
use pagis_software::{PublishAuthor, SoftwareGitStore};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteRunStore, SqliteSoftwareStore, SqliteWorkspaceStore,
};
use pagis_testkit::TestDaemon;

const PAGE: &str = "<!doctype html><title>Forecast</title><p>The forecast.</p>";

/// The stored manifest of the seeded Version: one widget tool and the
/// Widget it renders into, with one declared connect origin.
fn manifest(package: &str) -> serde_json::Value {
    serde_json::json!({
        "manifest": {
            "package": {
                "name": package,
                "description": "The forecast package.",
                "keywords": ["weather"],
                "setup": null,
            },
            "tool": [{
                "name": "forecast",
                "description": "The forecast of one city.",
                "entry": "bin/forecast.py",
                "schema": "schemas/forecast.json",
                "timeout_s": null,
                "widget": "forecast-card",
            }],
            "widget": [{
                "name": "forecast-card",
                "html": "widgets/forecast.html",
                "schema": "widgets/forecast.json",
                "csp": { "connect": ["https://api.example.com"], "resource": [] },
            }],
        },
        "schemas": {},
        "widget_schemas": { "forecast-card": { "type": "object" } },
    })
}

/// The package tree as a tar, with entries relative to the package
/// root, which is what `commit_version` takes.
fn tree(page: &str) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, body) in [
        ("widgets/forecast.html", page),
        ("widgets/forecast.json", "{\"type\":\"object\"}"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, body.as_bytes())
            .expect("the tar entry");
    }
    builder.into_inner().expect("the tar")
}

/// One published Version of `weather`, tree and record, as a publish
/// writes it.
async fn seed(daemon: &TestDaemon) {
    seed_page(daemon, PAGE).await;
}

/// Version `v1` of `weather`, whose Widget `forecast-card` is `page`.
pub(crate) async fn seed_page(daemon: &TestDaemon, page: &str) {
    seed_package(daemon, "weather", page).await;
}

/// Version `v1` of the package `name`, whose Widget `forecast-card` is
/// `page`.
pub(crate) async fn seed_package(daemon: &TestDaemon, name: &str, page: &str) {
    seed_version(daemon, name, manifest(name), tree(page)).await;
}

/// One published Version of any package: the tree into the package's
/// bare repository, and the record into the store.
async fn seed_version(daemon: &TestDaemon, name: &str, manifest: serde_json::Value, tar: Vec<u8>) {
    let pool = daemon.pool().clone();
    let workspace = SqliteWorkspaceStore::new(pool.clone())
        .list()
        .await
        .expect("workspaces")
        .remove(0);
    let author = SqliteAgentStore::new(pool.clone())
        .list_by_workspace(&workspace.id)
        .await
        .expect("agents")
        .remove(0);
    let run = Run {
        id: RunId::generate(),
        workspace_id: workspace.id.clone(),
        agent_id: author.id.clone(),
        channel_id: None,
        root_message_id: None,
        trigger_kind: TriggerKind::Message,
        trigger_ref: None,
        hop_count: 0,
        origin: None,
        state: RunState::Completed,
        failure_kind: None,
        error: None,
        started_at: None,
        ended_at: None,
        created_at: now_ms(),
    };
    SqliteRunStore::new(pool.clone())
        .create(&run)
        .await
        .expect("the run");

    let git = SoftwareGitStore::new(daemon.booted.home.join("software"));
    let commit_id = git
        .commit_version(
            &workspace.id,
            pagis_software::NewVersion {
                package: name,
                version: "v1",
                previous: None,
                notes: "The first Version.",
                author: &PublishAuthor::of_agent(&author.name, &author.id),
                tar,
            },
        )
        .await
        .expect("the version tree");

    let now = now_ms();
    let package = SoftwarePackage {
        id: SoftwarePackageId::generate(),
        workspace_id: workspace.id.clone(),
        name: name.to_string(),
        author_agent_id: author.id.clone(),
        description: "The forecast package.".to_string(),
        keywords: vec!["weather".to_string()],
        latest_version: "v1".to_string(),
        origin_package_id: None,
        origin_version: None,
        created_at: now,
        updated_at: now,
    };
    let packages = SqliteSoftwareStore::new(pool.clone());
    packages
        .create_package(&package)
        .await
        .expect("the package");
    packages
        .add_version(
            &package,
            &SoftwareVersion {
                package_id: package.id.clone(),
                version: "v1".to_string(),
                notes: "The first Version.".to_string(),
                commit_id,
                manifest,
                published_at: now,
                run_id: run.id.clone(),
            },
        )
        .await
        .expect("the version");
}

/// The scaffold package the `pagis:widgets` Skill ships, read
/// from the tree the Computer image copies to `/opt/pagis/skills/`.
fn scaffold_root() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    )
    .join("../../computer/skills/widgets/scaffold")
}

/// The scaffold as a publish leaves it: the Version record from the
/// publish validation, and the tree as a tar with entries relative to
/// the package root.
fn scaffold_version() -> (serde_json::Value, Vec<u8>) {
    let root = scaffold_root();
    let version = pagis_software::validate(&root).expect("the scaffold is sound");
    let mut builder = tar::Builder::new(Vec::new());
    builder
        .append_dir_all("", &root)
        .expect("the scaffold tree");
    (
        serde_json::to_value(&version).expect("the version record"),
        builder.into_inner().expect("the tar"),
    )
}

async fn get(daemon: &TestDaemon, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{}{}", daemon.base_url, path))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("GET")
}

/// A direct visit to the page route never runs the page at the
/// Product App origin: the route answers the source as bytes to save,
/// which the Product App reads as text and hands to the sandbox proxy.
#[tokio::test]
async fn the_widget_page_route_serves_the_source_as_inert_bytes() {
    let daemon = TestDaemon::start().await;
    seed(&daemon).await;

    let response = get(&daemon, "/api/v1/widgets/weather/v1/forecast-card").await;
    assert_eq!(response.status(), 200);
    let headers = response.headers().clone();

    assert_eq!(headers["content-type"], "application/octet-stream");
    assert_eq!(headers["content-disposition"], "attachment");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(
        headers["content-security-policy"], "sandbox; default-src 'none'",
        "a browser runs no script in the response, even when it shows it"
    );
    assert_eq!(
        headers["cache-control"], "public, max-age=31536000, immutable",
        "a Version never changes"
    );

    assert_eq!(response.text().await.expect("the body"), PAGE);
}

#[tokio::test]
async fn an_unknown_widget_is_not_found() {
    let daemon = TestDaemon::start().await;
    seed(&daemon).await;

    for path in [
        // No such `[[widget]]` entry in the Version.
        "/api/v1/widgets/weather/v1/missing-card",
        // No such Version.
        "/api/v1/widgets/weather/v9/forecast-card",
        // No such package.
        "/api/v1/widgets/banking/v1/forecast-card",
    ] {
        assert_eq!(get(&daemon, path).await.status(), 404, "{path}");
    }
}

#[tokio::test]
async fn the_page_needs_a_session() {
    let daemon = TestDaemon::start().await;
    seed(&daemon).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/widgets/weather/v1/forecast-card",
            daemon.base_url
        ))
        .send()
        .await
        .expect("GET");
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn the_sandbox_proxy_serves_under_the_widgets_policy_without_a_session() {
    let daemon = TestDaemon::start().await;
    seed(&daemon).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/widgets/{}/weather/v1/forecast-card/sandbox",
            daemon.base_url, daemon.workspace_id
        ))
        .send()
        .await
        .expect("GET");
    assert_eq!(response.status(), 200, "a sandboxed frame sends no cookie");

    let headers = response.headers().clone();
    assert!(
        headers["content-type"]
            .to_str()
            .expect("the type is text")
            .starts_with("text/html"),
        "{:?}",
        headers["content-type"]
    );
    let policy = headers["content-security-policy"]
        .to_str()
        .expect("the policy is text")
        .to_string();
    assert!(
        policy.contains("connect-src https://api.example.com;"),
        "the proxy carries the Widget's own policy, which the `srcdoc` \
         page inherits: {policy}"
    );

    let body = response.text().await.expect("the body");
    assert!(
        body.contains("ui/notifications/sandbox-proxy-ready"),
        "the proxy announces itself to the host"
    );
    assert!(
        !body.contains(PAGE),
        "the proxy carries no widget page of its own"
    );
}

#[tokio::test]
async fn an_unknown_widget_still_gets_the_closed_sandbox_policy() {
    let daemon = TestDaemon::start().await;
    seed(&daemon).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/widgets/{}/weather/v1/missing-card/sandbox",
            daemon.base_url, daemon.workspace_id
        ))
        .send()
        .await
        .expect("GET");
    // The route needs no Session, so it must not say what is installed.
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .expect("the policy is text")
            .contains("connect-src 'none';"),
    );
}

#[tokio::test]
async fn the_view_of_an_unknown_tool_call_is_not_found() {
    let daemon = TestDaemon::start().await;

    let response = get(&daemon, "/api/v1/widgets/call_nothing/view").await;
    assert_eq!(response.status(), 404);
    let body: serde_json::Value = response.json().await.expect("JSON");
    assert_eq!(body["error"]["code"], "not_found");
}

#[tokio::test]
async fn the_view_needs_a_session() {
    let daemon = TestDaemon::start().await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/widgets/call_1/view", daemon.base_url))
        .send()
        .await
        .expect("GET");
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn the_chart_scaffold_serves_its_page_through_the_daemon() {
    let daemon = TestDaemon::start().await;
    let (manifest, tar) = scaffold_version();
    seed_version(&daemon, "chart", manifest, tar).await;

    let response = get(&daemon, "/api/v1/widgets/chart/v1/chart").await;

    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    let page = response.text().await.expect("the body");
    assert_eq!(
        page,
        std::fs::read_to_string(scaffold_root().join("widgets/chart.html")).expect("the page"),
        "the route serves the file the Version holds"
    );
    assert!(
        page.contains("ui/notifications/initialized"),
        "the page speaks the MCP Apps protocol"
    );

    let proxy = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/widgets/{}/chart/v1/chart/sandbox",
            daemon.base_url, daemon.workspace_id
        ))
        .send()
        .await
        .expect("GET");
    let policy = proxy.headers()["content-security-policy"]
        .to_str()
        .expect("the policy is text")
        .to_string();
    assert!(
        policy.contains("connect-src 'none';"),
        "the scaffold declares no origin, so its page reaches no network: {policy}"
    );
}

#[tokio::test]
async fn the_rpc_of_an_unknown_view_is_not_found() {
    let daemon = TestDaemon::start().await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/widgets/call_nothing/rpc",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .send()
        .await
        .expect("POST");
    assert_eq!(response.status(), 404);
    let body: serde_json::Value = response.json().await.expect("JSON");
    assert_eq!(body["error"]["code"], "not_found");
}
