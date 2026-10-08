//! The connections an Agent learns from: the Gmail syncs that
//! name the Agent as responsible.

use pagis_core::{AgentId, AgentStore, Connection, ConnectionId, ConnectionStore, now_ms};
use pagis_storage_sqlite::{SqliteAgentStore, SqliteConnectionStore};
use pagis_testkit::TestDaemon;

/// A Google connection that holds Gmail read access.
async fn google_connection(daemon: &TestDaemon, alias: &str, display_name: &str) -> Connection {
    let workspace_id = SqliteAgentStore::new(daemon.pool().clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap()
        .workspace_id;
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id,
        provider: "google".into(),
        alias: alias.into(),
        display_name: display_name.into(),
        status: Connection::CONNECTED.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: serde_json::json!({"account": format!("{alias}@example.com"), "client": "test"}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(daemon.pool().clone())
        .create(&connection)
        .await
        .unwrap();
    connection
}

/// Name one Agent responsible for the Gmail sync of one connection. A
/// paused sync needs no grant.
async fn name_responsible(daemon: &TestDaemon, connection: &Connection, agent_id: &str) {
    let mut filter = pagis_google::gmail_filter::default_filter();
    filter.rules.truncate(2);
    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/connections/{}/sync",
            daemon.base_url, connection.id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": agent_id, "enabled": false, "since": 0, "filter": filter,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn an_agent_lists_the_connections_whose_sync_names_it_responsible() {
    let daemon = TestDaemon::start().await;
    let client = reqwest::Client::new();
    let other: serde_json::Value = client
        .post(format!("{}/api/v1/agents", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"name": "Clown", "job": "helper"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let personal = google_connection(&daemon, "personal", "Personal").await;
    let work = google_connection(&daemon, "work", "Work").await;
    google_connection(&daemon, "unsynced", "Unsynced").await;
    name_responsible(&daemon, &personal, &daemon.agent_id).await;
    name_responsible(&daemon, &work, other["id"].as_str().unwrap()).await;

    let response = client
        .get(format!(
            "{}/api/v1/agents/{}/sync-connections",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body,
        serde_json::json!({"items": [{
            "connection_id": personal.id.to_string(),
            "display_name": "Personal",
            "provider": "google",
            "resource": "gmail",
            "enabled": false,
            "caught_up": false,
            "rule_count": 2,
        }]})
    );

    let missing = client
        .get(format!(
            "{}/api/v1/agents/no-such-agent/sync-connections",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}
