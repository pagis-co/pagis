//! Ops and settings REST tests.

use pagis_core::{
    AgentId, EventLog, Grant, GrantId, GrantStore, NewEvent, RunState, RunStore, WorkspaceStore,
    now_ms,
};
use pagis_storage_sqlite::{
    SqliteEventLog, SqliteGrantStore, SqliteRunStore, SqliteWorkspaceStore,
};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions, fixture};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn runs_list_filters_by_agent_and_state_newest_first() {
    let daemon = TestDaemon::start().await;
    let store = SqliteRunStore::new(daemon.pool().clone());
    let workspace = SqliteWorkspaceStore::new(daemon.pool().clone())
        .list()
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let agent_id = AgentId::from(daemon.agent_id.clone());

    let mut older = fixture::queued_run(
        &workspace.id,
        &agent_id,
        &daemon.dm_channel_id.clone().into(),
    );
    older.state = RunState::Completed;
    older.started_at = Some(1_000);
    older.ended_at = Some(1_250);
    older.created_at = 1_000;
    store.create(&older).await.unwrap();

    let mut newer = fixture::queued_run(
        &workspace.id,
        &agent_id,
        &daemon.dm_channel_id.clone().into(),
    );
    newer.state = RunState::Completed;
    newer.started_at = Some(2_000);
    newer.ended_at = Some(2_900);
    newer.created_at = 2_000;
    store.create(&newer).await.unwrap();

    let mut failed = fixture::queued_run(
        &workspace.id,
        &agent_id,
        &daemon.dm_channel_id.clone().into(),
    );
    failed.state = RunState::Failed;
    failed.created_at = 3_000;
    store.create(&failed).await.unwrap();

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/runs", daemon.base_url))
        .header("cookie", daemon.cookie())
        .query(&[
            ("agent_id", daemon.agent_id.as_str()),
            ("state", "completed"),
        ])
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], newer.id.as_str());
    assert_eq!(items[0]["duration_ms"], 900);
    assert_eq!(items[1]["id"], older.id.as_str());
    assert_eq!(items[1]["duration_ms"], 250);
}

#[tokio::test]
async fn runs_list_filters_by_a_set_of_states_and_rejects_an_unknown_one() {
    let daemon = TestDaemon::start().await;
    let store = SqliteRunStore::new(daemon.pool().clone());
    let workspace = SqliteWorkspaceStore::new(daemon.pool().clone())
        .list()
        .await
        .unwrap()
        .remove(0);
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let channel_id = daemon.dm_channel_id.clone().into();

    let mut queued = fixture::queued_run(&workspace.id, &agent_id, &channel_id);
    queued.created_at = 1_000;
    store.create(&queued).await.unwrap();

    let mut waiting = fixture::queued_run(&workspace.id, &agent_id, &channel_id);
    waiting.state = RunState::WaitingForUser;
    waiting.created_at = 2_000;
    store.create(&waiting).await.unwrap();

    let mut completed = fixture::queued_run(&workspace.id, &agent_id, &channel_id);
    completed.state = RunState::Completed;
    completed.created_at = 3_000;
    store.create(&completed).await.unwrap();

    let client = reqwest::Client::new();
    let response = client
        .get(format!("{}/api/v1/runs", daemon.base_url))
        .header("cookie", daemon.cookie())
        .query(&[("state", "queued,waiting_for_user")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let ids: Vec<&str> = items
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&queued.id.as_str()));
    assert!(ids.contains(&waiting.id.as_str()));

    let rejected = client
        .get(format!("{}/api/v1/runs", daemon.base_url))
        .header("cookie", daemon.cookie())
        .query(&[("state", "queued,not_a_state")])
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 422);
}

#[tokio::test]
async fn run_transcript_keeps_unknown_events_and_sums_turn_usage() {
    let daemon = TestDaemon::start().await;
    let workspaces = SqliteWorkspaceStore::new(daemon.pool().clone());
    let workspace = workspaces.list().await.unwrap().remove(0);
    let run_store = SqliteRunStore::new(daemon.pool().clone());
    let mut run = fixture::queued_run(
        &workspace.id,
        &daemon.agent_id.clone().into(),
        &daemon.dm_channel_id.clone().into(),
    );
    run.state = RunState::Completed;
    run.started_at = Some(1_000);
    run.ended_at = Some(1_600);
    run_store.create(&run).await.unwrap();

    let events = SqliteEventLog::new(daemon.pool().clone());
    for (event_type, payload) in [
        (
            "turn.started",
            serde_json::json!({ "turn": 0, "model_alias": "default" }),
        ),
        (
            "tool.completed",
            serde_json::json!({
                "turn": 0,
                "name": "computer",
                "duration_ms": 125,
                "artifact_ids": ["shot-1"]
            }),
        ),
        (
            "turn.completed",
            serde_json::json!({ "turn": 0, "input_tokens": 40, "output_tokens": 12 }),
        ),
        (
            "plugin.future_event",
            serde_json::json!({ "detail": "keep this" }),
        ),
    ] {
        events
            .append(NewEvent {
                workspace_id: workspace.id.clone(),
                event_type: event_type.to_string(),
                agent_id: Some(run.agent_id.clone()),
                run_id: Some(run.id.clone()),
                channel_id: run.channel_id.clone(),
                payload,
            })
            .await
            .unwrap();
    }

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/runs/{}/events", daemon.base_url, run.id))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["run"]["duration_ms"], 600);
    assert_eq!(body["usage"]["input_tokens"], 40);
    assert_eq!(body["usage"]["output_tokens"], 12);
    assert_eq!(body["events"][0]["event_type"], "turn.started");
    assert_eq!(body["events"][1]["payload"]["duration_ms"], 125);
    assert_eq!(body["events"][1]["payload"]["artifact_ids"][0], "shot-1");
    assert_eq!(body["events"][3]["event_type"], "plugin.future_event");
    assert_eq!(body["events"][3]["payload"]["detail"], "keep this");
}

#[tokio::test]
async fn an_alias_edit_applies_to_the_next_run() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Done"]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;

    let updated = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/settings/model-aliases/default",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "candidates": ["openai/gpt-5.6-luna"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(updated.status(), 200);

    let sent = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": "alias-next-run", "text": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), 201);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !brain.requests().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the next run reaches the brain");

    let request = brain.requests().remove(0);
    assert_eq!(request.model_alias, "default");
    assert_eq!(request.model_candidates, ["openai/gpt-5.6-luna"]);
}

#[tokio::test]
async fn model_aliases_support_crud_and_protect_aliases_in_use() {
    let daemon = TestDaemon::start().await;
    let client = reqwest::Client::new();

    let created = client
        .post(format!("{}/api/v1/settings/model-aliases", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "alias": "fast",
            "candidates": ["openai/gpt-5.4-mini"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);

    let listed: serde_json::Value = client
        .get(format!("{}/api/v1/settings/model-aliases", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // The seeded aliases (`default`, and the voice pair `speak` and
    // `transcribe`, ADR-0020), telephone call aliases, plus the new one,
    // by name.
    let aliases: Vec<&str> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["alias"].as_str().unwrap())
        .collect();
    assert_eq!(
        aliases,
        vec![
            "default",
            "fast",
            "gpt-live-reasoning",
            "phone",
            "phone-classifier",
            "speak",
            "transcribe"
        ]
    );

    let deleted = client
        .delete(format!(
            "{}/api/v1/settings/model-aliases/fast",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);

    let in_use = client
        .delete(format!(
            "{}/api/v1/settings/model-aliases/default",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(in_use.status(), 409);
}

#[tokio::test]
async fn deleting_connections_and_credentials_revokes_their_grants() {
    let daemon = TestDaemon::start().await;
    let workspace = SqliteWorkspaceStore::new(daemon.pool().clone())
        .list()
        .await
        .unwrap()
        .remove(0);
    sqlx::query(
        "INSERT INTO connections \
         (id, workspace_id, provider, alias, display_name, status, auth_mode, config, \
         created_at) \
         VALUES ('conn-1', ?, 'google', 'work', 'Work Google', 'connected', 'byo', '{}', ?)",
    )
    .bind(workspace.id.as_str())
    .bind(now_ms())
    .execute(daemon.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO credentials (id, workspace_id, domain, username, login_url, secret, \
         recipe, provenance, created_at) \
         VALUES ('cred-1', ?, 'example.com', 'alice', 'https://example.com/login', \
         x'00', '', 'user_supplied', ?)",
    )
    .bind(workspace.id.as_str())
    .bind(now_ms())
    .execute(daemon.pool())
    .await
    .unwrap();
    let grants = SqliteGrantStore::new(daemon.pool().clone());
    for (kind, id) in [("connection", "conn-1"), ("credential", "cred-1")] {
        grants
            .create(&Grant {
                id: GrantId::generate(),
                workspace_id: workspace.id.clone(),
                agent_id: daemon.agent_id.clone().into(),
                resource_kind: kind.to_string(),
                resource_id: Some(id.to_string()),
                scope: serde_json::json!({}),
                revision: 1,
                created_at: now_ms(),
                revoked_at: None,
            })
            .await
            .unwrap();
    }

    let client = reqwest::Client::new();
    for path in ["connections/conn-1", "credentials/cred-1"] {
        let response = client
            .delete(format!("{}/api/v1/settings/{path}", daemon.base_url))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 204, "{path}");
    }

    assert!(grants.list_live(&workspace.id).await.unwrap().is_empty());
    for resource in ["connections", "credentials"] {
        let listed: serde_json::Value = client
            .get(format!("{}/api/v1/settings/{resource}", daemon.base_url))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(listed["items"].as_array().unwrap().is_empty());
    }
}

/// Artifact retention: every class starts at "keep for ever",
/// the user sets a window per class, and an unknown class is a 404.
#[tokio::test]
async fn retention_policies_read_back_and_accept_a_window_per_class() {
    let daemon = TestDaemon::start().await;
    let client = reqwest::Client::new();

    let listed: serde_json::Value = client
        .get(format!("{}/api/v1/settings/retention", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = listed["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["screenshot", "call_recording", "call_transcript", "file"]
    );
    assert!(
        items.iter().all(|item| item["retain_days"].is_null()),
        "every class is kept for ever by default"
    );

    let set = client
        .put(format!(
            "{}/api/v1/settings/retention/screenshot",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "retain_days": 30 }))
        .send()
        .await
        .unwrap();
    assert_eq!(set.status(), 200);

    let listed: serde_json::Value = client
        .get(format!("{}/api/v1/settings/retention", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["items"][0]["retain_days"], 30);
    assert!(listed["items"][1]["retain_days"].is_null());

    let cleared = client
        .put(format!(
            "{}/api/v1/settings/retention/screenshot",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "retain_days": serde_json::Value::Null }))
        .send()
        .await
        .unwrap();
    assert_eq!(cleared.status(), 200);

    let zero = client
        .put(format!(
            "{}/api/v1/settings/retention/file",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "retain_days": 0 }))
        .send()
        .await
        .unwrap();
    assert_eq!(zero.status(), 422);

    let unknown = client
        .put(format!(
            "{}/api/v1/settings/retention/receipts",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "retain_days": 30 }))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);
}

#[tokio::test]
async fn a_tool_dispatch_failure_has_a_kind_in_the_runs_api() {
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use std::sync::Arc;
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "missing_tool",
        serde_json::json!({}),
    ));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain,
        ..TestDaemonOptions::default()
    })
    .await;
    // Fail the tool audit write at the database boundary. The dispatcher must
    // stop the run when it cannot record a tool result.
    sqlx::query("CREATE TRIGGER fail_tool_audit BEFORE INSERT ON events WHEN NEW.event_type = 'tool.completed' BEGIN SELECT RAISE(FAIL, 'tool audit unavailable'); END")
        .execute(daemon.pool()).await.unwrap();
    let client = reqwest::Client::new();
    // The trigger bumps SQLite's schema version, so every statement the
    // daemon holds prepared is re-prepared on its next use, once per
    // pooled connection. A request that lands on a connection in the
    // middle of that reload fails on a table that plainly exists, so the
    // test reads the channel until every connection has come back.
    // Nothing in the product changes a schema under itself: the
    // migrations run at boot, before the daemon serves.
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut settled = 0;
        while settled < 20 {
            let read = client
                .get(format!(
                    "{}/api/v1/channels/{}/messages",
                    daemon.base_url, daemon.dm_channel_id
                ))
                .header("cookie", daemon.cookie())
                .send()
                .await
                .unwrap();
            settled = match read.status() == 200 {
                true => settled + 1,
                false => 0,
            };
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the daemon serves again after the schema change");
    let sent = client
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"pending_id": "tool-failure", "text": "Try the tool"}))
        .send()
        .await
        .unwrap();
    let status = sent.status();
    let body = sent.text().await.unwrap();
    assert_eq!(status, 201, "{body}");
    let run = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let page: serde_json::Value = client
                .get(format!("{}/api/v1/runs?state=failed", daemon.base_url))
                .header("cookie", daemon.cookie())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if let Some(run) = page["items"].as_array().unwrap().first() {
                break run.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tool failure settled");
    assert_eq!(run["failure_kind"], "tool_failed");
    assert!(
        run["error"]
            .as_str()
            .unwrap()
            .contains("tool audit unavailable")
    );
    let failed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let transcript: serde_json::Value = client
                .get(format!(
                    "{}/api/v1/runs/{}/events",
                    daemon.base_url,
                    run["id"].as_str().unwrap()
                ))
                .header("cookie", daemon.cookie())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            if let Some(failed) = transcript["events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| {
                    event["event_type"] == "run.state_changed" && event["payload"]["to"] == "failed"
                })
            {
                break failed.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tool failure event settled");
    assert_eq!(failed["payload"]["failure_kind"], "tool_failed");
}
