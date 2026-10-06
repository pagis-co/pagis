//! Model Request Capture (ADR-0031): the System Setting, the captures a
//! Run keeps while it is on, and who reads them.

use std::sync::Arc;
use std::time::Duration;

use pagis_core::{
    AgentId, ChannelId, ModelRequestCapture, ModelRequestCaptureId, RunId, WorkspaceId, now_ms,
};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn setting(daemon: &TestDaemon, cookie: &str, body: serde_json::Value) -> reqwest::Response {
    client()
        .put(format!(
            "{}/api/v1/settings/system/model-request-capture",
            daemon.administration_base_url
        ))
        .header("cookie", cookie)
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn settings(daemon: &TestDaemon) -> serde_json::Value {
    client()
        .get(format!(
            "{}/api/v1/settings/system",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn captures(daemon: &TestDaemon, cookie: &str, run_id: &str) -> reqwest::Response {
    client()
        .get(format!(
            "{}/api/v1/runs/{run_id}/model-requests",
            daemon.base_url
        ))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
}

async fn user(daemon: &TestDaemon, cookie: &str) -> serde_json::Value {
    client()
        .get(format!("{}/api/v1/user", daemon.base_url))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// A Run of `agent_id` in `workspace_id`, written straight to the store.
async fn stored_run(daemon: &TestDaemon, workspace_id: &WorkspaceId, agent_id: &AgentId) -> RunId {
    let mut channel = pagis_testkit::fixture::channel(workspace_id);
    channel.id = ChannelId::generate();
    daemon.stores().channels.create(&channel).await.unwrap();
    let run = pagis_testkit::fixture::queued_run(workspace_id, agent_id, &channel.id);
    daemon.stores().runs.create(&run).await.unwrap();
    run.id
}

async fn stored_capture(daemon: &TestDaemon, workspace_id: &WorkspaceId, run_id: &RunId) {
    daemon
        .stores()
        .model_request_captures
        .record(&ModelRequestCapture {
            id: ModelRequestCaptureId::generate(),
            workspace_id: workspace_id.clone(),
            run_id: run_id.clone(),
            phase: "reply".into(),
            phase_request: 0,
            request: serde_json::json!({"system": "Follow the user."}),
            answer: serde_json::json!({"outcome": "completed"}),
            created_at: now_ms(),
        })
        .await
        .unwrap();
}

async fn wait_for_completed_run(daemon: &TestDaemon) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let page: serde_json::Value = client()
            .get(format!("{}/api/v1/runs", daemon.base_url))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if let Some(run) = page["items"]
            .as_array()
            .and_then(|items| items.iter().find(|run| run["state"] == "completed"))
        {
            return run["id"].as_str().unwrap().to_string();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no run completed: {page}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The setting is off on a new installation. An Administrator turns it
/// on with a retention, the daemon writes `config.toml`, and the next Run
/// keeps its request with no restart.
#[tokio::test]
async fn an_administrator_turns_the_capture_on_and_the_next_run_keeps_its_request() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Hello!"]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let read = settings(&daemon).await;
    assert_eq!(
        read["model_request_capture"],
        serde_json::json!({"enabled": false, "retention_days": 7})
    );
    assert!(user(&daemon, daemon.cookie()).await["model_request_capture_days"].is_null());

    let saved = setting(
        &daemon,
        daemon.cookie(),
        serde_json::json!({"enabled": true, "retention_days": 3}),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let saved: serde_json::Value = saved.json().await.unwrap();
    assert_eq!(saved["restart_required"], false);
    assert_eq!(
        saved["settings"]["model_request_capture"],
        serde_json::json!({"enabled": true, "retention_days": 3})
    );
    let file = pagis::Config::read_file(&daemon.booted.home.join("config.toml")).unwrap();
    assert!(file.model_request_capture.enabled);
    assert_eq!(file.model_request_capture.retention_days, 3);
    // Each Person learns that Pagis keeps their model requests.
    assert_eq!(
        user(&daemon, daemon.cookie()).await["model_request_capture_days"],
        3
    );

    client()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"pending_id": "p1", "text": "the vault code is 7713"}))
        .send()
        .await
        .unwrap();
    let run_id = wait_for_completed_run(&daemon).await;

    let read = captures(&daemon, daemon.cookie(), &run_id).await;
    assert_eq!(read.status(), StatusCode::OK);
    let read: serde_json::Value = read.json().await.unwrap();
    let items = read["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["phase"], "reply");
    assert_eq!(items[0]["phase_request"], 0);
    assert!(
        items[0]["request"]
            .to_string()
            .contains("the vault code is 7713")
    );
    assert_eq!(items[0]["answer"]["outcome"], "completed");
}

#[tokio::test]
async fn a_retention_out_of_range_is_refused() {
    let daemon = TestDaemon::start().await;

    for days in [0, 31] {
        let response = setting(
            &daemon,
            daemon.cookie(),
            serde_json::json!({"enabled": true, "retention_days": days}),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{days}"
        );
    }
    assert_eq!(
        settings(&daemon).await["model_request_capture"]["enabled"],
        false
    );
}

/// Turning the setting off deletes every capture of the installation.
#[tokio::test]
async fn turning_the_capture_off_deletes_every_capture() {
    let daemon = TestDaemon::start().await;
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let run_id = stored_run(&daemon, &daemon.workspace_id, &agent_id).await;
    stored_capture(&daemon, &daemon.workspace_id, &run_id).await;
    stored_capture(&daemon, &daemon.workspace_id, &run_id).await;

    let saved = setting(
        &daemon,
        daemon.cookie(),
        serde_json::json!({"enabled": false, "retention_days": 7}),
    )
    .await;

    assert_eq!(saved.status(), StatusCode::OK);
    let left = daemon
        .stores()
        .model_request_captures
        .list_for_run(&daemon.workspace_id, &run_id)
        .await
        .unwrap();
    assert!(left.is_empty());
}

/// A capture holds Person data, so only an Administrator reads it. A
/// Member gets `404` for a capture of their own Run, as for a Run that
/// does not exist, and cannot change the setting.
#[tokio::test]
async fn a_member_reads_no_capture_and_changes_no_setting() {
    let daemon = TestDaemon::start().await;
    let member =
        crate::remote_access::member(&daemon, "grace@example.com", "a long password").await;
    let cookie = daemon.cookie_for(&member.id).await;
    let workspace = daemon
        .stores()
        .workspaces
        .for_user(&member.id)
        .await
        .unwrap()
        .unwrap();
    let agent = pagis_testkit::fixture::agent(&workspace.id);
    daemon.stores().agents.create(&agent).await.unwrap();
    let run_id = stored_run(&daemon, &workspace.id, &agent.id).await;
    stored_capture(&daemon, &workspace.id, &run_id).await;

    let read = captures(&daemon, &cookie, run_id.as_str()).await;
    assert_eq!(read.status(), StatusCode::NOT_FOUND);

    let written = setting(
        &daemon,
        &cookie,
        serde_json::json!({"enabled": true, "retention_days": 7}),
    )
    .await;
    assert_eq!(written.status(), StatusCode::FORBIDDEN);
}
