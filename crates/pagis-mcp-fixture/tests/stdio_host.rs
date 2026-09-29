//! The MCP host over a real stdio server (ADR-0017): the freeze
//! at install, the environment a server starts with, the deadline of
//! one call, the restart after a crash, and the two ways a tool list
//! changes under an installed Plugin.

use crate::support;

use std::time::{Duration, Instant};

use pagis_core::{Plugin, PluginState, PluginStore, WorkspaceId};
use support::{Harness, api_key, stdio_plugin};

#[tokio::test]
async fn an_install_freezes_the_tool_list_of_every_server() {
    let harness = Harness::new();
    let directory = stdio_plugin();

    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    let frozen = harness.catalog(&plugin).await;
    let mut names: Vec<&str> = frozen.tools.iter().map(|tool| tool.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "blob",
            "crash",
            "drop_tool",
            "echo",
            "environment",
            "flood",
            "headers",
            "sleep",
            "stderr"
        ]
    );
    assert!(frozen.tools.iter().all(|tool| tool.server == "fixture"));
    assert_eq!(frozen.version, "v1");
    assert_eq!(frozen.installed_commit, plugin.installed_commit);
    assert!(!frozen.tools_changed);
}

#[tokio::test]
async fn the_frozen_manifest_names_the_tools_and_their_effect_classes() {
    let harness = Harness::new();
    let directory = stdio_plugin();

    harness.install(directory.path(), &api_key("the-key")).await;

    let manifests = harness.manifests.0.lock().expect("manifest lock");
    let manifest = manifests.last().expect("one manifest is installed");
    assert_eq!(manifest.namespace, "fixture");
    assert_eq!(manifest.source_version, "v1");
    let echo = manifest
        .tools
        .iter()
        .find(|tool| tool.definition.name == "fixture__echo")
        .expect("the echo tool");
    // The package declares `free` for echo, so it needs no card.
    assert_eq!(echo.effect, pagis_broker::EffectClass::Free);
    let crash = manifest
        .tools
        .iter()
        .find(|tool| tool.definition.name == "fixture__crash")
        .expect("the crash tool");
    // Every tool the plugin did not declare is `Host` (ADR-0017), and
    // its card offers the one rule that names it.
    assert_eq!(crash.effect, pagis_broker::EffectClass::Host);
    assert_eq!(
        crash
            .presentation
            .as_ref()
            .and_then(|presentation| presentation.allow_rule_builder.clone()),
        Some(pagis_broker::AllowRuleBuilder::PluginTool)
    );
    // The declared deadline stands, inside its bounds.
    let sleep = manifest
        .tools
        .iter()
        .find(|tool| tool.definition.name == "fixture__sleep")
        .expect("the sleep tool");
    assert_eq!(sleep.call_timeout, Some(Duration::from_secs(1)));
}

#[tokio::test]
async fn a_call_reaches_the_server_over_stdio() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    let result = harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "hello"}))
        .await;

    assert!(!result.is_error, "{result:?}");
    assert_eq!(result.content, "hello");
}

#[tokio::test]
async fn the_server_starts_with_a_minimal_environment_that_carries_the_secret() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    let result = harness
        .dispatch(&plugin, "environment", serde_json::json!({}))
        .await;

    let held: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&result.content).expect("the environment is JSON");
    // The secret reaches the server as an `env` value, and only there.
    assert_eq!(
        held.get("FIXTURE_API_KEY").map(String::as_str),
        Some("the-key")
    );
    assert_eq!(
        held.get("PLUGIN_ROOT").map(String::as_str),
        Some(harness.root(&plugin.id).display().to_string().as_str())
    );
    assert!(held.contains_key("PATH"), "PATH is inherited");
    // Nothing else of the daemon's environment is. This test process
    // runs under Cargo, which fills its own environment with `CARGO_`
    // variables; none of them may reach a plugin.
    let leaked: Vec<&String> = held
        .keys()
        .filter(|name| name.starts_with("CARGO") || name.starts_with("RUST"))
        .collect();
    assert!(leaked.is_empty(), "the environment holds {leaked:?}");
    // What is left is what the daemon put there, and what `/bin/sh`
    // adds for itself when it runs the command.
    let unexpected: Vec<&String> = held
        .keys()
        .filter(|name| {
            !matches!(
                name.as_str(),
                "PATH"
                    | "HOME"
                    | "TMPDIR"
                    | "PLUGIN_ROOT"
                    | "PLUGIN_DATA"
                    | "FIXTURE_API_KEY"
                    | "PAGIS_FIXTURE_DROP_FILE"
                    | "PWD"
                    | "SHLVL"
                    | "_"
                    | "__CF_USER_TEXT_ENCODING"
            )
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "the environment holds {unexpected:?}"
    );
}

#[tokio::test]
async fn a_call_that_does_not_answer_in_time_is_outcome_unknown() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    let call = harness.call(
        &plugin,
        "fixture",
        "sleep",
        serde_json::json!({"seconds": 30}),
        Duration::from_millis(300),
    );
    let result = harness.host.dispatch(&call).await;

    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some("outcome_unknown"));
}

#[tokio::test]
async fn a_crashed_server_starts_again_at_the_next_call() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    let crashed = harness
        .dispatch(&plugin, "crash", serde_json::json!({}))
        .await;
    assert!(crashed.is_error);
    assert_eq!(crashed.code.as_deref(), Some("outcome_unknown"));

    let answered = harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "again"}))
        .await;
    assert!(!answered.is_error, "{answered:?}");
    assert_eq!(answered.content, "again");
}

#[tokio::test]
async fn a_start_whose_list_differs_marks_the_tools_changed() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;
    harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "one"}))
        .await;

    // The next start of the server drops one tool. The frozen list
    // does not move: the Plugin is marked, and the user accepts the
    // difference through the update flow (ADR-0017).
    std::fs::write(harness.data(&plugin.id).join("drop"), b"").expect("the drop file");
    harness.host.stop_plugin(&plugin.id).await;
    harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "two"}))
        .await;

    let frozen = harness.catalog(&plugin).await;
    assert!(frozen.tools_changed, "the changed list is recorded");
    assert_eq!(frozen.tools.len(), 9, "the frozen list does not move");
    assert!(
        harness
            .events()
            .iter()
            .any(|kind| kind == "plugin.tools_changed")
    );
}

/// The notification marks the Plugin when it arrives. No call has to end
/// after it: here the call that changed the list still runs, and no
/// other call follows.
#[tokio::test]
async fn a_tools_list_changed_notification_marks_the_tools_changed() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    // The server drops a tool, says so, and then holds the call open.
    let call = harness.dispatch(&plugin, "drop_tool", serde_json::json!({"seconds": 60}));
    tokio::select! {
        () = harness.bus.published("plugin.tools_changed") => {}
        answer = call => panic!("the call answered before the mark: {answer:?}"),
        () = tokio::time::sleep(Duration::from_secs(10)) => {
            panic!("the notification did not mark the Plugin")
        }
    }

    assert!(harness.catalog(&plugin).await.tools_changed);
}

#[tokio::test]
async fn a_tool_outside_the_frozen_list_is_refused_at_dispatch() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    let call = harness.call(
        &plugin,
        "fixture",
        "unknown",
        serde_json::json!({}),
        Duration::from_secs(5),
    );
    let result = harness.host.dispatch(&call).await;

    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some("tool_not_in_manifest"));
}

#[tokio::test]
async fn an_idle_server_stops_and_starts_again_on_the_next_call() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;
    harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "one"}))
        .await;

    let stopped = harness
        .host
        .reap_idle(Instant::now() + pagis_plugins::IDLE_SHUTDOWN)
        .await;

    assert_eq!(stopped, 1);
    let answered = harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "two"}))
        .await;
    assert_eq!(answered.content, "two");
}

#[tokio::test]
async fn the_server_output_reaches_the_plugin_log() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    // The script prints one line to stderr before it runs the server,
    // as a misbehaving server would.
    let script = format!(
        "#!/bin/sh\necho 'the fixture starts' >&2\nexec {} \"$@\"\n",
        support::fixture_binary()
    );
    std::fs::write(directory.path().join("server"), script).expect("the server script");
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;
    harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "one"}))
        .await;

    let mut log = String::new();
    for _ in 0..50 {
        log = harness.host.read_log(&plugin.id, 4096).await;
        if !log.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert!(log.contains("the fixture starts"), "the log holds {log:?}");
}

/// Wait until one host's log of the Plugin holds the text.
async fn log_holding(host: &pagis_plugins::PluginHost, plugin: &Plugin, text: &str) -> String {
    let mut log = String::new();
    for _ in 0..100 {
        log = host.read_log(&plugin.id, 4096).await;
        if log.contains(text) {
            return log;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the log never held {text:?}; it holds {log:?}");
}

/// Each Workspace runs the Org's Plugin in its own Plugin Computer, and
/// each one reads the stderr of its own servers only (ADR-0023). A
/// server can write Person data or a bound value to stderr.
#[tokio::test]
async fn each_workspace_reads_the_plugin_log_of_its_own_servers() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;
    let other = harness.host_for(WorkspaceId::generate());
    let never_ran = harness.host_for(WorkspaceId::generate());

    let a = harness
        .dispatch(
            &plugin,
            "stderr",
            serde_json::json!({"text": "the stderr of A"}),
        )
        .await;
    assert!(!a.is_error, "{a:?}");
    let call = harness.call(
        &plugin,
        "fixture",
        "stderr",
        serde_json::json!({"text": "the stderr of B"}),
        Duration::from_secs(30),
    );
    let b = other.dispatch(&call).await;
    assert!(!b.is_error, "{b:?}");

    let a_log = log_holding(&harness.host, &plugin, "the stderr of A").await;
    let b_log = log_holding(&other, &plugin, "the stderr of B").await;

    assert!(!a_log.contains("the stderr of B"), "A reads {a_log:?}");
    assert!(!b_log.contains("the stderr of A"), "B reads {b_log:?}");
    // A Workspace whose servers never ran has no log of the Plugin.
    assert_eq!(never_ran.read_log(&plugin.id, 4096).await, "");
}

#[tokio::test]
async fn an_uninstall_stops_the_servers_and_forgets_the_frozen_list() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;
    harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "one"}))
        .await;

    harness
        .plugins
        .uninstall(&harness.workspace_id, &plugin.id)
        .await
        .expect("uninstalled");

    assert!(
        harness
            .host
            .catalog(&plugin)
            .await
            .expect("the catalog reads")
            .is_none()
    );
}

#[tokio::test]
async fn an_install_whose_server_does_not_start_installs_nothing() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    std::fs::write(directory.path().join("server"), "#!/bin/sh\nexit 1\n")
        .expect("the server script");

    let refused = harness
        .plugins
        .install(
            &harness.workspace_id,
            &pagis_plugin::SourceInput::Upload {
                tar: {
                    let mut builder = tar::Builder::new(Vec::new());
                    builder
                        .append_dir_all(".", directory.path())
                        .expect("the tar");
                    builder.into_inner().expect("the tar")
                },
            },
            &api_key("the-key"),
        )
        .await;

    assert!(refused.is_err(), "the install is refused");
    assert!(
        harness
            .plugins
            .list(&harness.workspace_id)
            .await
            .expect("the list reads")
            .is_empty(),
        "nothing is installed"
    );
}

#[tokio::test]
async fn three_start_failures_mark_the_plugin_failed_and_a_manual_start_clears_it() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;
    // From here the server leaves as soon as it starts.
    std::fs::write(harness.data(&plugin.id).join("fail"), b"").expect("the fail marker");

    for attempt in 0..3 {
        let refused = harness
            .dispatch(&plugin, "echo", serde_json::json!({"text": "one"}))
            .await;
        assert!(refused.is_error, "{refused:?}");
        // Every attempt waits longer than the one before it, so the
        // next dispatch reaches a start rather than the backoff.
        if attempt < 2 {
            tokio::time::sleep(pagis_plugins::backoff(attempt + 1) + Duration::from_millis(200))
                .await;
        }
    }

    assert_eq!(harness.reread(&plugin).await.state, PluginState::Failed);
    assert!(harness.events().iter().any(|kind| kind == "plugin.failed"));

    std::fs::remove_file(harness.data(&plugin.id).join("fail")).expect("the fail marker goes");
    harness
        .host
        .start(&plugin.id)
        .await
        .expect("the manual start works");

    assert_eq!(harness.reread(&plugin).await.state, PluginState::Enabled);
    let answered = harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "again"}))
        .await;
    assert_eq!(answered.content, "again");
}

#[tokio::test]
async fn a_plugin_that_is_not_enabled_answers_no_call() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;
    harness
        .store
        .set_state(&plugin.workspace_id, &plugin.id, PluginState::Failed, 0)
        .await
        .expect("the state is written");

    let result = harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "one"}))
        .await;

    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some("permission_revoked"));
}

/// A Plugin's server process runs inside the tenant's Plugin Computer,
/// so the daemon starts none of its own (ADR-0017).
///
/// Two things prove it. The daemon asks the container seam for every
/// start, and the command it asks for names `/opt/pagis-plugins/<id>`,
/// a path that exists inside the Plugin Computer and nowhere on the
/// daemon host: a host process could not run it. And `pagis-plugins`
/// does not build the `process` feature of tokio, so the crate cannot
/// spawn a process at all — the compiler holds that half.
#[tokio::test]
async fn every_server_starts_in_the_container_and_none_on_the_daemon_host() {
    let harness = Harness::new();
    let directory = stdio_plugin();
    let plugin = harness.install(directory.path(), &api_key("the-key")).await;

    harness
        .dispatch(&plugin, "echo", serde_json::json!({"text": "hello"}))
        .await;

    let starts = harness.computer.starts.lock().expect("the stand-in lock");
    assert!(
        !starts.is_empty(),
        "the daemon started no server through the container"
    );
    for start in starts.iter() {
        let container_root = format!("{}/{}", pagis_plugins::CONTAINER_PLUGIN_ROOT, plugin.id);
        assert_eq!(
            start.env.get("PLUGIN_ROOT").map(String::as_str),
            Some(container_root.as_str()),
            "the server reads the daemon's own directory"
        );
        assert_eq!(
            start.env.get("PLUGIN_DATA").map(String::as_str),
            Some(format!("{}/{}", pagis_plugins::CONTAINER_PLUGIN_DATA, plugin.id).as_str())
        );
        assert!(
            start
                .program
                .starts_with(pagis_plugins::CONTAINER_PLUGIN_ROOT),
            "the command is a host path: {}",
            start.program
        );
        // The tenant's token travels in that exec's environment and
        // nowhere else.
        assert_eq!(
            start.env.get("FIXTURE_API_KEY").map(String::as_str),
            Some("the-key")
        );
        // The base environment is the container's, not the daemon's.
        assert_eq!(
            start.env.get("HOME").map(String::as_str),
            Some("/data/agent")
        );
    }
}

/// The token of a Connection, as a daemon that holds carrier keys hands
/// it out.
struct KeyOf;

#[async_trait::async_trait]
impl pagis_plugins::ConnectionTokens for KeyOf {
    async fn token(&self, connection: &pagis_core::Connection) -> Option<String> {
        Some(format!("key-of-{}", connection.alias))
    }
}

/// A `connection` Binding names an Installation Connection of the Org's
/// Workspace. A host that runs the Plugin for another tenant hands the
/// server the token of that Connection, because the Connection is the
/// Org's and not a person's (ADR-0017).
#[tokio::test]
async fn a_tenant_runs_the_plugin_with_the_token_of_the_orgs_connection() {
    use pagis_core::{ConnectionStore, GrantStore};

    let harness = Harness::for_a_tenant(std::sync::Arc::new(KeyOf));
    let carrier = pagis_core::Connection {
        id: pagis_core::ConnectionId::generate(),
        workspace_id: harness.workspace_id.clone(),
        provider: "telnyx".to_string(),
        alias: "telephony".to_string(),
        display_name: "Telnyx".to_string(),
        status: pagis_core::Connection::CONNECTED.to_string(),
        auth_mode: pagis_core::Connection::AUTH_MODE_BYO.to_string(),
        authorized_capabilities: Vec::new(),
        config: serde_json::json!({}),
        created_at: pagis_core::now_ms(),
    };
    harness.connections.create(&carrier).await.unwrap();
    harness
        .grants
        .create(&pagis_core::Grant {
            id: pagis_core::GrantId::generate(),
            workspace_id: harness.workspace_id.clone(),
            agent_id: harness.agent_id.clone(),
            resource_kind: pagis_core::Grant::CONNECTION_KIND.to_string(),
            resource_id: Some(carrier.id.to_string()),
            scope: pagis_core::Grant::connection_scope(&[]),
            revision: 1,
            created_at: pagis_core::now_ms(),
            revoked_at: None,
        })
        .await
        .unwrap();
    let directory = stdio_plugin();
    let manifest = directory.path().join("plugin.json");
    let package = std::fs::read_to_string(&manifest).unwrap().replace(
        r#""type": "secret""#,
        r#""type": "connection", "provider": "telnyx""#,
    );
    std::fs::write(&manifest, package).unwrap();

    let plugin = harness
        .install(
            directory.path(),
            &[pagis_plugin::BindingInput {
                field: "api_key".to_string(),
                value: pagis_plugin::BindingValueInput::Connection {
                    connection_id: carrier.id.clone(),
                },
            }],
        )
        .await;
    let result = harness
        .dispatch(&plugin, "environment", serde_json::json!({}))
        .await;

    let held: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&result.content).expect("the environment is JSON");
    assert_eq!(
        held.get("FIXTURE_API_KEY").map(String::as_str),
        Some("key-of-telephony")
    );
}
