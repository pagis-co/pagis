//! Connection grants through the daemon REST surface (ADR-0005).

use pagis_core::{AgentId, AgentStore, WorkspaceId, now_ms};
use pagis_storage_sqlite::SqliteAgentStore;
use pagis_testkit::TestDaemon;

async fn workspace_id(daemon: &TestDaemon) -> WorkspaceId {
    SqliteAgentStore::new(daemon.pool().clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap()
        .workspace_id
}

async fn connect_google(daemon: &TestDaemon, id: &str) {
    sqlx::query(
        "INSERT INTO connections \
         (id, workspace_id, provider, alias, display_name, status, config, created_at) \
         VALUES (?, ?, 'google', 'work', 'Work Google', 'connected', ?, ?)",
    )
    .bind(id)
    .bind(workspace_id(daemon).await.as_str())
    .bind(r#"{"account":"alice@example.com","client":"work"}"#)
    .bind(now_ms())
    .execute(daemon.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn a_user_grants_an_agent_named_connection_capabilities() {
    let daemon = TestDaemon::start().await;
    connect_google(&daemon, "conn-work").await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "connection_id": "conn-work",
            "capabilities": ["gmail_read", "calendar_read"]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 201);
    let grant: serde_json::Value = response.json().await.unwrap();
    assert_eq!(grant["agent_id"], daemon.agent_id);
    assert_eq!(grant["agent_name"], "Pixie");
    assert_eq!(grant["resource_kind"], "connection");
    assert_eq!(grant["resource_id"], "conn-work");
    assert_eq!(
        grant["capabilities"],
        serde_json::json!(["gmail_read", "calendar_read"])
    );
    assert_eq!(grant["revision"], 1);

    let listed: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["items"], serde_json::json!([grant]));
}

#[tokio::test]
async fn a_capability_outside_the_installed_manifests_is_refused() {
    let daemon = TestDaemon::start().await;
    connect_google(&daemon, "conn-work").await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "connection_id": "conn-work",
            "capabilities": ["gmail_read", "google_everything"]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 422);
    let error: serde_json::Value = response.json().await.unwrap();
    assert_eq!(error["error"]["code"], "validation");
    assert_eq!(
        error["error"]["message"],
        "capability google_everything is not declared by the installed google manifest"
    );

    let listed: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["items"], serde_json::json!([]));
}

#[tokio::test]
async fn a_disconnected_account_cannot_receive_a_new_grant() {
    let daemon = TestDaemon::start().await;
    connect_google(&daemon, "conn-work").await;
    sqlx::query("UPDATE connections SET status = 'disconnected' WHERE id = 'conn-work'")
        .execute(daemon.pool())
        .await
        .unwrap();

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "connection_id": "conn-work",
            "capabilities": ["gmail_read"]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 422);
    let error: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        error["error"]["message"],
        "a Connection must be connected before it can be granted"
    );
}

#[tokio::test]
async fn replacing_connection_capabilities_increments_the_grant_revision() {
    let daemon = TestDaemon::start().await;
    connect_google(&daemon, "conn-work").await;
    let created: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "connection_id": "conn-work",
            "capabilities": ["gmail_read", "calendar_read"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/grants/{}/capabilities",
            daemon.base_url,
            created["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"capabilities": ["gmail_send"]}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let changed: serde_json::Value = response.json().await.unwrap();
    assert_eq!(changed["capabilities"], serde_json::json!(["gmail_send"]));
    assert_eq!(changed["revision"], 2);
}

#[tokio::test]
async fn a_connection_grant_requires_at_least_one_named_capability() {
    let daemon = TestDaemon::start().await;
    connect_google(&daemon, "conn-work").await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "connection_id": "conn-work",
            "capabilities": []
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 422);
    let error: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        error["error"]["message"],
        "a Connection grant requires at least one capability"
    );
}

#[tokio::test]
async fn connection_capabilities_cannot_be_replaced_with_allow_rules() {
    let daemon = TestDaemon::start().await;
    connect_google(&daemon, "conn-work").await;
    let created: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "connection_id": "conn-work",
            "capabilities": ["gmail_read"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/grants/{}/rules",
            daemon.base_url,
            created["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"allow": ["git status"]}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 422);
    let error: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        error["error"]["message"],
        "only host and credential grants have allow rules"
    );

    let listed: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        listed["items"][0]["capabilities"],
        serde_json::json!(["gmail_read"])
    );
    assert_eq!(listed["items"][0]["revision"], 1);
}
