//! Owner forget controls are authenticated and remain available without a model read grant.
use std::sync::Arc;

use pagis_core::knowledge::{KnowledgeStore, SourceKey, SyncConfig};
use pagis_core::{
    AgentId, AgentStore, Connection, ConnectionId, ConnectionStore, EventLog, ForgetKeys, Grant,
    GrantId, GrantStore, MemoryAccess, MemoryAuthor, MemoryChangeset, MemoryExposure,
    MemorySecretStore, MemoryStore, NewEvent, ScopedPath, SecretStore, TenantKeys, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteConnectionStore, SqliteEventLog, SqliteForgetStore, SqliteGrantStore,
    SqliteKnowledgeStore, SqliteMemoryPageIndex,
};
use pagis_testkit::{TestDaemon, TestDaemonOptions};

#[tokio::test]
async fn owner_can_preview_forget_and_explicitly_reopt_in_without_reconnecting() {
    let daemon = TestDaemon::start().await;
    let agent = SqliteAgentStore::new(daemon.pool().clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: agent.workspace_id,
        provider: "google".into(),
        alias: "personal".into(),
        display_name: "Personal".into(),
        status: Connection::CONNECTED.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: serde_json::json!({}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(daemon.pool().clone())
        .create(&connection)
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let base = format!("{}/api/v1/knowledge/forget", daemon.base_url);
    let target = serde_json::json!({"kind":"account", "connection_id":connection.id});
    assert_eq!(
        client
            .post(format!("{base}/preview"))
            .json(&serde_json::json!({"target":target}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let response = client
        .post(format!("{base}/preview"))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"target":target}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "owner preview must be available even when no Agent holds a read grant"
    );
    let preview: serde_json::Value = response.json().await.unwrap();
    assert_eq!(preview["source_items"], 0);
    assert_eq!(preview["raw_account_retrieval_blocked"], true);
    let response = client
        .post(&base)
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"target":target,"preview":preview}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let operation: serde_json::Value = response.json().await.unwrap();
    let id = operation["id"].as_str().unwrap();
    let operation_url = format!("{base}/{id}");
    let complete = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let current: serde_json::Value = client
                .get(&operation_url)
                .header("cookie", daemon.cookie())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert!(current["error"].is_null(), "purge failed: {current}");
            if current["phase"] == "complete" {
                break current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        complete["reopted_at"].is_null(),
        "completion must keep suppression active"
    );
    let response = client
        .post(format!("{operation_url}/reopt"))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let allowed: serde_json::Value = response.json().await.unwrap();
    assert!(allowed["reopted_at"].is_i64());
}

/// A Forget through the product keys its suppression row with the key
/// that the Tenant Data Key of the Workspace derives. The daemon holds
/// that key in `secrets.enc` and not in the database, so a new holder
/// over the same `secrets.enc`, as after a restart, computes the same
/// identity (ADR-0008).
#[tokio::test]
async fn a_forget_keys_its_suppression_row_with_the_tenant_data_key() {
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        secrets: Arc::clone(&secrets),
        ..Default::default()
    })
    .await;
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: daemon.workspace_id.clone(),
        provider: "google".into(),
        alias: "personal".into(),
        display_name: "Personal".into(),
        status: Connection::CONNECTED.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: serde_json::json!({}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(daemon.pool().clone())
        .create(&connection)
        .await
        .unwrap();
    // A Sync that is off holds the source item, so no collector reads
    // the Connection.
    SqliteKnowledgeStore::new(daemon.pool().clone())
        .configure(
            SyncConfig {
                workspace_id: daemon.workspace_id.clone(),
                connection_id: connection.id.clone(),
                resource: "gmail".into(),
                agent_id: AgentId::from(daemon.agent_id.clone()),
                required_capability: "gmail_read".into(),
                enabled: false,
                since: 0,
                filter: pagis_google::gmail_filter::default_filter(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO source_items(workspace_id,connection_id,resource,id,version,record,observed_at) \
         VALUES(?,?,'gmail','m-1','1','{}',1)",
    )
    .bind(daemon.workspace_id.as_str())
    .bind(connection.id.as_str())
    .execute(daemon.pool())
    .await
    .unwrap();
    let source = SourceKey {
        workspace_id: daemon.workspace_id.clone(),
        connection_id: connection.id.clone(),
        resource: "gmail".into(),
    };
    let target = serde_json::json!({"kind":"source", "source":source, "source_id":"m-1"});
    let client = reqwest::Client::new();
    let base = format!("{}/api/v1/knowledge/forget", daemon.base_url);
    let preview: serde_json::Value = client
        .post(format!("{base}/preview"))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"target":target}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview["source_items"], 1, "{preview}");
    let response = client
        .post(&base)
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"target":target,"preview":preview}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let operation: serde_json::Value = response.json().await.unwrap();
    let operation_url = format!("{base}/{}", operation["id"].as_str().unwrap());
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let current: serde_json::Value = client
                .get(&operation_url)
                .header("cookie", daemon.cookie())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert!(current["error"].is_null(), "purge failed: {current}");
            if current["phase"] == "complete" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();

    let identity: String = sqlx::query_scalar(
        "SELECT identity FROM forget_suppressions WHERE workspace_id=? AND connection_id=?",
    )
    .bind(daemon.workspace_id.as_str())
    .bind(connection.id.as_str())
    .fetch_one(daemon.pool())
    .await
    .unwrap();
    let restarted = TenantKeys::new(secrets);
    assert_eq!(
        identity,
        restarted
            .suppression_key(&daemon.workspace_id)
            .unwrap()
            .identity(&source, "m-1"),
        "the daemon keyed the Forget with a key that secrets.enc does not derive"
    );
}

/// A mark that no other value of the test holds, in the thread of the
/// forgotten item and in the title and sentence of its page.
const FORGOTTEN: &str = "forgotten-3f9a";

/// The Learning Feed of the daemon's Workspace.
async fn learning_feed(daemon: &TestDaemon) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{}/api/v1/memory/feed", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// One page that a commit writes: its path, its title and the sentence
/// of the commit.
struct Change<'a> {
    path: &'a str,
    title: &'a str,
    message: &'a str,
}

/// One memory commit of an Agent, with its Learning Feed entry as a Run
/// publishes it. The answer is the commit.
async fn commit_with_entry(
    daemon: &TestDaemon,
    memory: &pagis_memory::GitMemoryStore,
    access: &MemoryAccess,
    parent: Option<&str>,
    change: Change<'_>,
) -> String {
    let agent_id = access.agent_id().expect("an Agent writes the page");
    let mut changes = MemoryChangeset::default();
    changes.writes.insert(
        ScopedPath::parse(change.path).unwrap(),
        format!("---\ntitle: {}\n---\n\n{}\n", change.title, change.message),
    );
    let sha = memory
        .commit(
            &daemon.workspace_id,
            agent_id,
            access,
            &MemoryAuthor {
                name: "Pixie".into(),
                email: format!("{}@agents.pagis.local", agent_id.as_str()),
            },
            &changes,
            parent,
            change.message,
            None,
        )
        .await
        .unwrap();
    SqliteEventLog::new(daemon.pool().clone())
        .append(NewEvent {
            workspace_id: daemon.workspace_id.clone(),
            event_type: "memory.committed".into(),
            agent_id: Some(agent_id.clone()),
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({
                "sha": sha,
                "scopes": [change.path.split('/').next().unwrap()],
                "files": [change.path],
                "titles": [change.title],
                "message": change.message,
                "exposures": access.exposures(),
                "run_id": null,
                "source_scoped": !access.exposures().is_empty(),
            }),
        })
        .await
        .unwrap();
    sha
}

/// After a Forget, the Learning Feed still lists the change of the page
/// that stays, as it was. It lists no change of the purged Subject
/// Page, and no entry names its path, its title or the sentence of its
/// commit (ADR-0008).
#[tokio::test]
async fn the_learning_feed_lists_the_pages_that_stay_after_a_forget() {
    let daemon = TestDaemon::start().await;
    let pool = daemon.pool().clone();
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: daemon.workspace_id.clone(),
        provider: "google".into(),
        alias: "personal".into(),
        display_name: "Personal".into(),
        status: Connection::CONNECTED.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: serde_json::json!({}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(pool.clone())
        .create(&connection)
        .await
        .unwrap();
    // A Sync that is off holds the source item, so no collector reads
    // the Connection.
    SqliteKnowledgeStore::new(pool.clone())
        .configure(
            SyncConfig {
                workspace_id: daemon.workspace_id.clone(),
                connection_id: connection.id.clone(),
                resource: "gmail".into(),
                agent_id: agent_id.clone(),
                required_capability: "gmail_read".into(),
                enabled: false,
                since: 0,
                filter: pagis_google::gmail_filter::default_filter(),
            },
            now_ms(),
        )
        .await
        .unwrap();
    // The message of the thread whose Subject Page the Forget purges.
    sqlx::query(
        "INSERT INTO source_items(workspace_id,connection_id,resource,id,version,record,observed_at) \
         VALUES(?,?,'gmail','m-1','1',?,1)",
    )
    .bind(daemon.workspace_id.as_str())
    .bind(connection.id.as_str())
    .bind(serde_json::json!({"parent": format!("thread-{FORGOTTEN}")}).to_string())
    .execute(&pool)
    .await
    .unwrap();
    let grant = Grant {
        id: GrantId::generate(),
        workspace_id: daemon.workspace_id.clone(),
        agent_id: agent_id.clone(),
        resource_kind: Grant::CONNECTION_KIND.into(),
        resource_id: Some(connection.id.to_string()),
        scope: Grant::connection_scope(&["gmail_read".into()]),
        revision: 1,
        created_at: now_ms(),
        revoked_at: None,
    };
    SqliteGrantStore::new(pool.clone())
        .create(&grant)
        .await
        .unwrap();
    let memory = pagis_memory::GitMemoryStore::new(
        daemon.booted.home.join("memory"),
        Arc::new(SqliteForgetStore::new(pool.clone())),
        Arc::new(SqliteMemoryPageIndex::new(pool.clone())),
    );
    // A conversation wrote a note, and then a Reflection of the message
    // wrote the Subject Page of its thread.
    let kept = commit_with_entry(
        &daemon,
        &memory,
        &MemoryAccess::agent(agent_id.clone(), []),
        None,
        Change {
            path: "shared/kept.md",
            title: "Kept note",
            message: "Noted that the lanternfish note stays.",
        },
    )
    .await;
    let reader = MemoryAccess::agent(
        agent_id.clone(),
        [MemoryExposure {
            grant_id: grant.id.clone(),
            revision: grant.revision,
        }],
    );
    commit_with_entry(
        &daemon,
        &memory,
        &reader,
        Some(&kept),
        Change {
            path: &format!("private/subjects/gmail/thread-{FORGOTTEN}.md"),
            title: &format!("The {FORGOTTEN} invoice"),
            message: &format!("Learned that the {FORGOTTEN} invoice is due on Friday."),
        },
    )
    .await;
    let before = learning_feed(&daemon).await;
    assert_eq!(before["items"].as_array().unwrap().len(), 2, "{before}");

    let source = SourceKey {
        workspace_id: daemon.workspace_id.clone(),
        connection_id: connection.id.clone(),
        resource: "gmail".into(),
    };
    let target = serde_json::json!({"kind":"source", "source":source, "source_id":"m-1"});
    let client = reqwest::Client::new();
    let base = format!("{}/api/v1/knowledge/forget", daemon.base_url);
    let preview: serde_json::Value = client
        .post(format!("{base}/preview"))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"target":target}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let operation: serde_json::Value = client
        .post(&base)
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"target":target,"preview":preview}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let operation_url = format!("{base}/{}", operation["id"].as_str().unwrap());
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let current: serde_json::Value = client
                .get(&operation_url)
                .header("cookie", daemon.cookie())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert!(current["error"].is_null(), "purge failed: {current}");
            if current["phase"] == "complete" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();

    let after = learning_feed(&daemon).await;
    assert!(
        !after.to_string().contains(FORGOTTEN),
        "the Learning Feed names the purged page: {after}"
    );
    let items = after["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{after}");
    assert_eq!(items[0]["sha"], kept.as_str());
    assert_eq!(items[0]["files"], serde_json::json!(["shared/kept.md"]));
    assert_eq!(items[0]["titles"], serde_json::json!(["Kept note"]));
    assert_eq!(
        items[0]["message"],
        "Noted that the lanternfish note stays."
    );
}
