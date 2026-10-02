//! The Plugins destination (ADR-0022, ADR-0017): install from
//! an upload, read the install card, bind a field, grant an Agent, and
//! uninstall.
//!
//! An install freezes the tool list of every declared server
//! (ADR-0017), so a Plugin that reaches the `enabled` state needs a
//! server that answers. The fixture MCP server runs in this test
//! process on a loopback port and stands behind every such install.

use pagis_testkit::TestDaemon;

/// The fixture MCP server on a loopback port of its own.
async fn serve() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let port = listener.local_addr().expect("the address").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, pagis_mcp_fixture::http_router()).await;
    });
    port
}

/// The `plugin.json` of the weather plugin. `config` names the fields
/// the two packages below bind.
fn manifest(name: &str, config: serde_json::Value) -> String {
    serde_json::json!({
        "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
        "name": name,
        "description": "The weather plugin.",
        "extensions": {"pagis": {
            "config": config,
            "tools": {"forecast": {"effect": "free"}},
        }},
    })
    .to_string()
}

/// The plugin directory as one tar, as the desk uploads it. Its
/// server is the fixture, whose port arrives as the `endpoint`
/// Binding, so the install freezes a real tool list.
fn package(name: &str) -> Vec<u8> {
    let directory = tempfile::tempdir().expect("a scratch directory");
    std::fs::write(
        directory.path().join("plugin.json"),
        manifest(
            name,
            serde_json::json!({
                "api_key": {"type": "secret", "title": "API key", "required": true},
                "endpoint": {"type": "string", "title": "Endpoint", "required": true},
            }),
        ),
    )
    .expect("the manifest");
    std::fs::write(
        directory.path().join("mcp.json"),
        serde_json::json!({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
            "mcpServers": {
                "weather": {
                    "type": "streamable-http",
                    "url": "http://127.0.0.1:${config.endpoint}/mcp",
                    "headers": {"Authorization": "Bearer ${config.api_key}"},
                },
            },
        })
        .to_string(),
    )
    .expect("the mcp configuration");
    with_skill(directory)
}

/// The same plugin with one stdio server. Nothing starts it: a test
/// installs it without its required Binding, so the Plugin stays
/// `disabled` and the card is all there is to read.
fn stdio_package(name: &str) -> Vec<u8> {
    let directory = tempfile::tempdir().expect("a scratch directory");
    std::fs::write(
        directory.path().join("plugin.json"),
        manifest(
            name,
            serde_json::json!({
                "api_key": {"type": "secret", "title": "API key", "required": true},
            }),
        ),
    )
    .expect("the manifest");
    std::fs::write(
        directory.path().join("mcp.json"),
        serde_json::json!({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
            "mcpServers": {
                "weather": {
                    "type": "stdio",
                    "command": "weather-mcp",
                    "env": {"API_KEY": "${config.api_key}", "REGION": "eu"},
                },
            },
        })
        .to_string(),
    )
    .expect("the mcp configuration");
    with_skill(directory)
}

/// One Skill beside the manifest, and the directory as a tar.
fn with_skill(directory: tempfile::TempDir) -> Vec<u8> {
    std::fs::create_dir_all(directory.path().join("skills/forecast")).expect("the skill");
    std::fs::write(
        directory.path().join("skills/forecast/SKILL.md"),
        "# Forecast\n",
    )
    .expect("the skill");
    tar_of(directory.path())
}

fn tar_of(directory: &std::path::Path) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    builder
        .append_dir_all(".", directory)
        .expect("the upload tar");
    builder.into_inner().expect("the upload tar")
}

/// Upload the tar as an Artifact and answer with its id.
async fn upload(daemon: &TestDaemon, bytes: Vec<u8>) -> String {
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name("weather.tar".to_string())
        .mime_str("application/x-tar")
        .expect("the part");
    let body: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/api/v1/artifacts", daemon.base_url))
        .header("cookie", daemon.cookie())
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .expect("upload")
        .json()
        .await
        .expect("JSON");
    body["id"].as_str().expect("the artifact id").to_string()
}

/// The Bindings that make the weather plugin ready to serve.
fn bindings(port: u16) -> serde_json::Value {
    serde_json::json!([
        {"field": "api_key", "kind": "secret", "secret": "s3cret"},
        {"field": "endpoint", "kind": "value", "value": port.to_string()},
    ])
}

async fn install(daemon: &TestDaemon, name: &str, port: u16) -> serde_json::Value {
    let artifact_id = upload(daemon, package(name)).await;
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/plugins", daemon.administration_base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "source": {"kind": "upload", "artifact_id": artifact_id},
            "bindings": bindings(port),
        }))
        .send()
        .await
        .expect("install");
    assert_eq!(response.status().as_u16(), 201);
    response.json().await.expect("JSON")
}

#[tokio::test]
async fn the_install_card_carries_every_env_value_and_the_effect_classes() {
    let daemon = TestDaemon::start().await;
    let artifact_id = upload(&daemon, stdio_package("weather")).await;

    // No Binding arrives, so the Plugin installs `disabled` and no
    // server starts. The card is what the user reads before binding.
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/plugins", daemon.administration_base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "source": {"kind": "upload", "artifact_id": artifact_id},
        }))
        .send()
        .await
        .expect("install");
    assert_eq!(response.status().as_u16(), 201);
    let installed: serde_json::Value = response.json().await.expect("JSON");

    assert_eq!(installed["name"], "weather");
    assert_eq!(installed["state"], "disabled");
    assert_eq!(installed["manifest_version"], "v1");
    assert_eq!(installed["source_kind"], "upload");

    let server = &installed["servers"][0];
    assert_eq!(server["transport"], "stdio");
    assert_eq!(server["command"], "weather-mcp");
    let env: Vec<(String, String)> = server["env"]
        .as_array()
        .expect("the env entries")
        .iter()
        .map(|entry| {
            (
                entry["name"].as_str().expect("a name").to_string(),
                entry["value"].as_str().expect("a value").to_string(),
            )
        })
        .collect();
    assert_eq!(
        env,
        vec![
            ("API_KEY".to_string(), "${config.api_key}".to_string()),
            ("REGION".to_string(), "eu".to_string()),
        ],
        "every env value is package data the user sees"
    );

    assert_eq!(installed["tools"][0]["tool"], "forecast");
    assert_eq!(installed["tools"][0]["effect"], "free");
    assert_eq!(installed["skills"][0]["name"], "forecast");
    assert_eq!(installed["fields"][0]["name"], "api_key");
    assert_eq!(installed["fields"][0]["kind"], "secret");
}

#[tokio::test]
async fn the_list_and_the_detail_read_the_installed_plugin() {
    let port = serve().await;
    let daemon = TestDaemon::start().await;
    let installed = install(&daemon, "weather", port).await;
    let plugin_id = installed["id"].as_str().expect("the id");

    assert_eq!(installed["state"], "enabled");
    assert_eq!(installed["bindings"][0]["field"], "api_key");
    assert_eq!(
        installed["bindings"][0]["value"],
        serde_json::Value::Null,
        "the secret never leaves the daemon"
    );

    let list: serde_json::Value = get(&daemon, "/api/v1/plugins").await;
    assert_eq!(list["items"].as_array().expect("the rows").len(), 1);
    assert_eq!(list["items"][0]["name"], "weather");

    let detail: serde_json::Value = get(&daemon, &format!("/api/v1/plugins/{plugin_id}")).await;
    assert_eq!(detail["installed_commit"], installed["installed_commit"]);
}

#[tokio::test]
async fn a_second_plugin_of_the_same_name_conflicts() {
    let port = serve().await;
    let daemon = TestDaemon::start().await;
    install(&daemon, "weather", port).await;
    let artifact_id = upload(&daemon, package("weather")).await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/plugins", daemon.administration_base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "source": {"kind": "upload", "artifact_id": artifact_id},
        }))
        .send()
        .await
        .expect("install");

    assert_eq!(response.status().as_u16(), 409);
}

#[tokio::test]
async fn a_package_that_is_not_valid_is_refused() {
    let daemon = TestDaemon::start().await;
    let mut builder = tar::Builder::new(Vec::new());
    let content = b"{\"name\": \"weather\"}";
    let mut header = tar::Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "plugin.json", content.as_slice())
        .expect("the entry");
    let artifact_id = upload(&daemon, builder.into_inner().expect("the tar")).await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/plugins", daemon.administration_base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "source": {"kind": "upload", "artifact_id": artifact_id},
        }))
        .send()
        .await
        .expect("install");

    assert_eq!(response.status().as_u16(), 422);
}

#[tokio::test]
async fn a_grant_joins_one_agent_to_one_plugin() {
    let port = serve().await;
    let daemon = TestDaemon::start().await;
    let installed = install(&daemon, "weather", port).await;
    let plugin_id = installed["id"].as_str().expect("the id");

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/plugins/{plugin_id}/grants",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "agent_id": daemon.agent_id }))
        .send()
        .await
        .expect("grant");

    assert_eq!(response.status().as_u16(), 201);
    let grant: serde_json::Value = response.json().await.expect("JSON");
    assert_eq!(grant["resource_kind"], "plugin");
    assert_eq!(grant["resource_id"], plugin_id);

    let grants: serde_json::Value = get(&daemon, "/api/v1/grants").await;
    assert_eq!(grants["items"].as_array().expect("the grants").len(), 1);
}

#[tokio::test]
async fn an_uninstall_removes_the_plugin_and_its_grants() {
    let port = serve().await;
    let daemon = TestDaemon::start().await;
    let installed = install(&daemon, "weather", port).await;
    let plugin_id = installed["id"].as_str().expect("the id");
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/plugins/{plugin_id}/grants",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "agent_id": daemon.agent_id }))
        .send()
        .await
        .expect("grant");

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/plugins/{plugin_id}",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("uninstall");

    assert_eq!(response.status().as_u16(), 204);
    let list: serde_json::Value = get(&daemon, "/api/v1/plugins").await;
    assert!(list["items"].as_array().expect("the rows").is_empty());
    let grants: serde_json::Value = get(&daemon, "/api/v1/grants").await;
    assert!(
        grants["items"].as_array().expect("the grants").is_empty(),
        "the grants on the plugin are revoked"
    );
}

#[tokio::test]
async fn an_upload_updates_from_a_new_upload_and_names_what_changed() {
    let port = serve().await;
    let daemon = TestDaemon::start().await;
    let installed = install(&daemon, "weather", port).await;
    let plugin_id = installed["id"].as_str().expect("the id");
    // A second package with one more file, so the update has a diff.
    let next = with_readme(package("weather"));
    let artifact_id = upload(&daemon, next).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/plugins/{plugin_id}/update",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "artifact_id": artifact_id }))
        .send()
        .await
        .expect("update");

    assert_eq!(response.status().as_u16(), 200);
    let updated: serde_json::Value = response.json().await.expect("JSON");
    assert_eq!(updated["manifest_version"], "v2");
    assert_eq!(updated["changed"][0]["path"], "README.md");
    assert_eq!(updated["changed"][0]["status"], "added");
}

#[tokio::test]
async fn a_binding_reaches_a_field_the_plugin_declares() {
    let port = serve().await;
    let daemon = TestDaemon::start().await;
    let installed = install(&daemon, "weather", port).await;
    let plugin_id = installed["id"].as_str().expect("the id");

    let refused = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/plugins/{plugin_id}/bindings/region",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"kind": "value", "value": "eu"}))
        .send()
        .await
        .expect("bind");

    assert_eq!(refused.status().as_u16(), 422);

    let bound = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/plugins/{plugin_id}/bindings/api_key",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"kind": "secret", "secret": "rotated"}))
        .send()
        .await
        .expect("bind");

    assert_eq!(bound.status().as_u16(), 200);
    let body: serde_json::Value = bound.json().await.expect("JSON");
    assert_eq!(body["state"], "enabled");
}

/// The same package with one more file in it.
fn with_readme(tar: Vec<u8>) -> Vec<u8> {
    let directory = tempfile::tempdir().expect("a scratch directory");
    tar::Archive::new(tar.as_slice())
        .unpack(directory.path())
        .expect("unpack");
    std::fs::write(directory.path().join("README.md"), "the weather plugin\n").expect("the file");
    tar_of(directory.path())
}

async fn get(daemon: &TestDaemon, path: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{}{}", daemon.base_url, path))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("GET")
        .error_for_status()
        .expect("success")
        .json()
        .await
        .expect("JSON")
}

/// A bus that keeps nothing: this body asserts against the Computer
/// runtime, not the audit trail.
struct SilentBus;

#[async_trait::async_trait]
impl pagis_core::EventBus for SilentBus {
    async fn publish(
        &self,
        event: pagis_core::NewEvent,
    ) -> Result<pagis_core::Event, pagis_core::StoreError> {
        Ok(pagis_core::Event {
            id: pagis_core::EventId::generate(),
            seq: 0,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: pagis_core::now_ms(),
        })
    }

    async fn subscribe(
        &self,
        _scope: pagis_core::EventScope,
        _after_seq: Option<i64>,
    ) -> pagis_core::EventStream {
        unreachable!("this body never subscribes")
    }
}

/// A Plugin's server process starts in the Plugin Computer of the tenant
/// that calls it, not of the tenant that installed it (ADR-0017).
///
/// The Plugin is the Org's install, so the mount set is the Org's
/// checkout; the container the mounts go into belongs to the caller. The
/// MCP host reaches Docker through this seam.
#[tokio::test]
async fn a_plugin_server_starts_in_the_calling_tenants_plugin_computer() {
    let runtime = std::sync::Arc::new(pagis_computer::fake::FakeComputerRuntime::with_image());
    let org_workspace = pagis_core::WorkspaceId::generate();
    let tenant_b = pagis_core::WorkspaceId::generate();
    let screens = tempfile::tempdir().expect("screens dir");
    let managers = pagis_computer::ComputerManagers::new(pagis_computer::ComputerManagersDeps {
        runtime: std::sync::Arc::clone(&runtime) as _,
        skills: std::sync::Arc::new(pagis_core::NoSkills) as _,
        workspaces: std::sync::Arc::new(pagis_computer::fake::FakeWorkspaces::with_timezone(
            &org_workspace,
            "UTC",
        )) as _,
        agents: std::sync::Arc::new(pagis_computer::fake::FakeAgents::open()) as _,
        bus: std::sync::Arc::new(SilentBus) as _,
        screens_dir: screens.path().to_path_buf(),
        idle_stop: std::time::Duration::from_secs(600),
        relay: pagis_computer::fake::loopback_relay(),
        caps: pagis_computer::AwakeCaps::default(),
        cancel: tokio_util::sync::CancellationToken::new(),
        exit: None,
    });
    let processes = pagis::plugin_tools::ComputerServerProcesses::new(
        std::sync::Arc::clone(&managers),
        std::sync::Arc::new(pagis_testkit::MemoryPluginStore::new(std::sync::Arc::new(
            pagis_testkit::MemoryGrantStore::default(),
        ))) as _,
        std::sync::Arc::new(pagis_plugin::PluginGitStore::new(
            screens.path().join("plugins"),
        )),
        org_workspace.clone(),
    );

    let started = pagis_plugins::ServerProcesses::start(
        &processes,
        &tenant_b,
        &pagis_plugins::Spawn {
            program: "weather-mcp".to_string(),
            args: Vec::new(),
            cwd: std::path::PathBuf::from("/data/agent"),
            env: Default::default(),
        },
    )
    .await
    .expect("the server starts");
    drop(started);

    // The Plugin Computer the runtime was asked for is B's, and its Agent
    // id is the Plugin Computer's, not a sprite's.
    let owners = runtime.started_owners();
    assert_eq!(owners.len(), 1, "{owners:?}");
    assert_eq!(owners[0].workspace_id, tenant_b);
    assert_eq!(owners[0].agent_id, pagis_computer::plugin_agent());
    assert_ne!(owners[0].workspace_id, org_workspace);
}

/// A restore gives every file of the state directory the owner's bits
/// alone, and the Plugin Computer reads a checkout as a uid that does not
/// own it. The next start makes the checkout readable to every uid again.
#[tokio::test]
async fn a_start_makes_a_checkout_that_only_the_owner_reads_readable() {
    use std::os::unix::fs::PermissionsExt;

    use crate::boot::{mode, tree};

    let port = serve().await;
    let daemon = TestDaemon::start().await;
    let installed = install(&daemon, "weather", port).await;
    let plugin_id = installed["id"].as_str().expect("the id").to_string();
    let home = daemon.stop().await;
    let plugins = home.path().join("plugins");
    let checkout = tree(&plugins)
        .into_iter()
        .find(|path| path.is_dir() && path.ends_with(&plugin_id))
        .expect("the checkout of the Plugin");
    for path in tree(&plugins) {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode(&path) & 0o700))
            .expect("the owner's bits alone");
    }

    let _restarted = TestDaemon::start_on(home, pagis_testkit::TestDaemonOptions::default()).await;

    for path in tree(&checkout) {
        let expected = if path.is_dir() { 0o755 } else { 0o644 };
        assert_eq!(mode(&path), expected, "{}", path.display());
    }
    assert!(checkout.join("skills/forecast/SKILL.md").is_file());
}
