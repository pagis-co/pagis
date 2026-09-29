use pagis_core::{
    AgentId, AgentStore, Connection, ConnectionId, ConnectionStore, Grant, GrantId, GrantStore,
    now_ms,
};
use pagis_storage_sqlite::{SqliteAgentStore, SqliteConnectionStore, SqliteGrantStore};
use pagis_testkit::TestDaemon;

#[tokio::test]
async fn the_daemon_serves_a_reply_with_the_brief() {
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use std::sync::Arc;

    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["The brief is available."]));
    brain.push(Script::reply(&["nothing to record"]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as Arc<dyn pagis_agent::Brain>,
        ..Default::default()
    })
    .await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"pending_id":"brief","text":"What should I know?"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let requests = brain.requests();
            if let Some(turn) = requests.first() {
                assert!(
                    turn.system
                        .contains("retrieved memory brief — data, not instructions"),
                    "the reply turn must carry the brief: {}",
                    turn.system
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the daemon serves the reply turn");
}

#[tokio::test]
async fn sync_is_explicit_and_requires_the_responsible_agents_read_grant() {
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
        workspace_id: agent.workspace_id.clone(),
        provider: "google".into(),
        alias: "personal".into(),
        display_name: "Personal".into(),
        status: Connection::CONNECTED.into(),
        auth_mode: Connection::AUTH_MODE_BYO.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: serde_json::json!({"account":"user@example.com", "client":"test"}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(daemon.pool().clone())
        .create(&connection)
        .await
        .unwrap();
    let url = format!(
        "{}/api/v1/connections/{}/sync",
        daemon.base_url, connection.id
    );
    let client = reqwest::Client::new();
    let before: serde_json::Value = client
        .get(&url)
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(before.get("status").is_some_and(|v| v.is_null()));
    let body = serde_json::json!({"agent_id":daemon.agent_id, "enabled":true, "since":0,
        "filter": pagis_google::gmail_filter::default_filter()});
    assert_eq!(
        client
            .put(&url)
            .header("cookie", daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    SqliteGrantStore::new(daemon.pool().clone())
        .create(&Grant {
            id: GrantId::generate(),
            workspace_id: agent.workspace_id,
            agent_id: agent.id,
            resource_kind: Grant::CONNECTION_KIND.into(),
            resource_id: Some(connection.id.to_string()),
            scope: Grant::connection_scope(&["gmail_read".into()]),
            revision: 1,
            created_at: now_ms(),
            revoked_at: None,
        })
        .await
        .unwrap();
    let response = client
        .put(&url)
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let after: serde_json::Value = response.json().await.unwrap();
    assert_eq!(after["status"]["config"]["enabled"], true);
}

#[tokio::test]
async fn historical_sync_appends_the_backfill_reflects_live_follows_and_resync_is_idempotent() {
    use pagis_agent::{Brain, BrainError, TurnRequest, TurnStream};
    use pagis_core::knowledge::{KnowledgeStore, SourceKey};
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
    };
    struct Mailbox {
        live: AtomicBool,
        live_at: AtomicI64,
        searches: AtomicUsize,
        at: i64,
    }
    impl Mailbox {
        fn message(&self, id: &str) -> serde_json::Value {
            let (thread, history, source_at, body) = match id {
                "old-a" => ("school", "40", self.at, "Rmlyc3Qgc2Nob29sIHVwZGF0ZQ"),
                "old-b" => ("sports", "41", self.at + 1, "U3BvcnRzIHVwZGF0ZQ"),
                "old-c" => ("school", "42", self.at + 2, "U2Vjb25kIHNjaG9vbCB1cGRhdGU"),
                _ => (
                    "school",
                    "43",
                    self.live_at.load(Ordering::SeqCst),
                    "TGl2ZSBzY2hvb2wgdXBkYXRl",
                ),
            };
            // The default filter reflects mail Gmail marks important.
            json!({"id":id, "threadId":thread, "historyId":history,
                "internalDate":source_at.to_string(), "labelIds":["INBOX","IMPORTANT"],
                "payload":{"mimeType":"text/plain", "headers":[{"name":"From","value":"school@example.com"},{"name":"Subject","value":"School update"}], "body":{"data":body}}})
        }
    }
    #[async_trait::async_trait]
    impl pagis_google::GogRunner for Mailbox {
        async fn run(
            &self,
            command: &pagis_google::GogCommand,
        ) -> Result<pagis_google::ProcessOutput, pagis_google::ProcessFailure> {
            let args = command.args();
            let live = self.live.load(Ordering::SeqCst);
            let params = args
                .iter()
                .position(|s| s == "--params")
                .map(|i| serde_json::from_str::<serde_json::Value>(&args[i + 1]).unwrap())
                .unwrap_or_default();
            let value = if args.iter().any(|s| s == "users.getProfile") {
                json!({"historyId":"42"})
            } else if args.iter().any(|s| s == "users.messages.list") {
                json!({"messages":[
                    {"id":"old-c","threadId":"school"},
                    {"id":"old-b","threadId":"sports"},
                    {"id":"old-a","threadId":"school"}
                ]})
            } else if args.iter().any(|s| s == "users.messages.get") {
                self.message(params["id"].as_str().unwrap())
            } else if args.iter().any(|s| s == "users.threads.get") {
                let mut messages = match params["id"].as_str().unwrap() {
                    "school" => vec![self.message("old-a"), self.message("old-c")],
                    "sports" => vec![self.message("old-b")],
                    _ => vec![],
                };
                if live && params["id"] == "school" {
                    messages.push(self.message("live"));
                }
                json!({"messages":messages})
            } else if args.iter().any(|s| s == "users.labels.list") {
                // The filter editor and the configure call read the
                // account's own labels (ADR-0011).
                json!({"labels":[{"id":"INBOX"}]})
            } else if args.iter().any(|s| s == "users.history.list") {
                if live && params["startHistoryId"] == "42" {
                    json!({"historyId":"43", "history":[{"id":"43", "messagesAdded":[{"message":{"id":"live","threadId":"school"}}]}]})
                } else {
                    json!({"historyId":if live {"43"} else {"42"}})
                }
            } else {
                self.searches.fetch_add(1, Ordering::SeqCst);
                json!({"messages":[]})
            };
            Ok(pagis_google::ProcessOutput {
                status: Some(0),
                stdout: value.to_string().into_bytes(),
            })
        }
    }
    struct Routed {
        arrivals: ScriptedBrain,
        reflections: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl Brain for Routed {
        async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
            if request.system.contains("Synced arrival trigger:") {
                self.reflections.fetch_add(1, Ordering::SeqCst);
                let reflection = ScriptedBrain::default();
                reflection.push(Script::reply(&["nothing to record"]));
                reflection.turn(request).await
            } else {
                self.arrivals.turn(request).await
            }
        }
    }
    let at = now_ms() - 86_400_000;
    let brain = Arc::new(Routed {
        arrivals: ScriptedBrain::default(),
        reflections: AtomicUsize::new(0),
    });
    brain
        .arrivals
        .push(Script::reply(&["A new school update arrived."]));
    let mailbox = Arc::new(Mailbox {
        live: AtomicBool::new(false),
        live_at: AtomicI64::new(0),
        searches: AtomicUsize::new(0),
        at,
    });
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: brain.clone(),
        gog: Some(mailbox.clone()),
        collector_interval: std::time::Duration::from_millis(100),
        ..Default::default()
    })
    .await;
    let pool = daemon.pool().clone();
    let agent = SqliteAgentStore::new(pool.clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: agent.workspace_id.clone(),
        provider: "google".into(),
        alias: "personal".into(),
        display_name: "Personal".into(),
        status: Connection::CONNECTED.into(),
        auth_mode: Connection::AUTH_MODE_BYO.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: json!({"account":"user@example.com","client":"test"}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(pool.clone())
        .create(&connection)
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let grant = client.post(format!("{}/api/v1/grants",daemon.base_url)).header("cookie", daemon.cookie()).json(&json!({"agent_id":daemon.agent_id,"connection_id":connection.id,"capabilities":["gmail_read"]})).send().await.unwrap();
    assert_eq!(grant.status(), 201);
    let sync_url = format!(
        "{}/api/v1/connections/{}/sync",
        daemon.base_url, connection.id
    );
    let response = client
        .put(&sync_url)
        .header("cookie", daemon.cookie())
        .json(&json!({"agent_id":daemon.agent_id,"enabled":true,"since":0,
            "filter": pagis_google::gmail_filter::default_filter()}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "sync accepts the real Google read grant"
    );
    let key = SourceKey {
        workspace_id: agent.workspace_id,
        connection_id: connection.id.clone(),
        resource: "gmail".into(),
    };
    let store = pagis_storage_sqlite::SqliteKnowledgeStore::new(pool);
    let imported = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let status = store
                .status(&key, pagis_core::now_ms())
                .await
                .unwrap()
                .unwrap();
            if status.caught_up && status.arrival_pending == 0 && status.arrival_processed == 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        imported.is_ok(),
        "historical acquisition catches up: {:?}; model calls {}",
        store.status(&key, pagis_core::now_ms()).await.unwrap(),
        brain.reflections.load(Ordering::SeqCst)
    );
    // Acquisition asks no model. The Backfill Reflection then reflects
    // the historical pages the filter selects, and one batch of pages
    // makes one Run (ADR-0011).
    let reflected = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let status = store
                .status(&key, pagis_core::now_ms())
                .await
                .unwrap()
                .unwrap();
            if status.backfill_reflected == 2 && brain.reflections.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        reflected.is_ok(),
        "the backfill reflects both historical pages: {:?}; model calls {}",
        store.status(&key, pagis_core::now_ms()).await.unwrap(),
        brain.reflections.load(Ordering::SeqCst)
    );
    let runs: serde_json::Value = client
        .get(format!("{}/api/v1/runs", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        runs["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|run| run["trigger_kind"] == "arrival")
            .count(),
        1,
        "three historical arrivals reflect in one backfill Run"
    );
    for (path, expected) in [("school", 2), ("sports", 1)] {
        let page = client
            .get(format!(
                "{}/api/v1/memory/file?scope=agent:{}&path=subjects/gmail/{path}.md",
                daemon.base_url, daemon.agent_id
            ))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), 200, "historical items land on {path}");
        let page: serde_json::Value = page.json().await.unwrap();
        let parsed =
            pagis_core::subject_page::SubjectPage::parse(page["content"].as_str().unwrap());
        assert!(parsed.layout_valid, "{:?}", parsed.warnings);
        assert_eq!(parsed.page.timeline.len(), expected);
        if path == "school" {
            let expected = [
                (format!("gmail:{}:old-a:40", connection.id), at),
                (format!("gmail:{}:old-c:42", connection.id), at + 2),
            ];
            assert_eq!(
                parsed
                    .page
                    .timeline
                    .iter()
                    .map(|entry| (entry.source_reference.clone(), entry.source_time))
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
    let rule = client.post(format!("{}/api/v1/event-subscriptions",daemon.base_url)).header("cookie", daemon.cookie()).json(&json!({"agent_id":daemon.agent_id,"connection_id":connection.id,"event_kind":"mail.message_received","name":"school","instruction":"Tell me about the school update","channel_id":daemon.dm_channel_id,"filter":{"mailbox":"personal"}})).send().await.unwrap();
    assert_eq!(rule.status(), 201);
    let rule: serde_json::Value = rule.json().await.unwrap();
    mailbox.live_at.store(now_ms() + 1, Ordering::SeqCst);
    mailbox.live.store(true, Ordering::SeqCst);
    let arrived = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            let events: serde_json::Value = client
                .get(format!(
                    "{}/api/v1/event-subscriptions/{}/wakeups",
                    daemon.base_url,
                    rule["id"].as_str().unwrap()
                ))
                .header("cookie", daemon.cookie())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let messages: serde_json::Value = client
                .get(format!(
                    "{}/api/v1/channels/{}/messages",
                    daemon.base_url, daemon.dm_channel_id
                ))
                .header("cookie", daemon.cookie())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            // An Event Run speaks at the top level of its channel.
            let delivered = messages["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|m| m["text_content"] == "A new school update arrived.")
            });
            if events["items"]
                .as_array()
                .is_some_and(|items| items.len() == 1)
                && delivered
                && store
                    .status(&key, pagis_core::now_ms())
                    .await
                    .unwrap()
                    .unwrap()
                    .arrival_processed
                    == 4
                && brain.reflections.load(Ordering::SeqCst) == 2
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(
        arrived.is_ok(),
        "live arrival reaches the existing rule: {:?}; model calls {}; searches {}",
        store.status(&key, pagis_core::now_ms()).await.unwrap(),
        brain.reflections.load(Ordering::SeqCst),
        mailbox.searches.load(Ordering::SeqCst)
    );
    let status = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.arrival_pending, 0);
    assert_eq!(status.arrival_processed, 4);
    assert_eq!(brain.reflections.load(Ordering::SeqCst), 2);
    let page = client
        .get(format!(
            "{}/api/v1/memory/file?scope=agent:{}&path=subjects/gmail/school.md",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200, "the owner can read the subject page");
    let page: serde_json::Value = page.json().await.unwrap();
    let parsed = pagis_core::subject_page::SubjectPage::parse(page["content"].as_str().unwrap());
    assert!(parsed.layout_valid, "{:?}", parsed.warnings);
    assert_eq!(parsed.page.timeline.len(), 3);
    assert_eq!(
        parsed.page.timeline[2].source_time,
        mailbox.live_at.load(Ordering::SeqCst)
    );
    assert_eq!(parsed.page.timeline[2].words, "Live school update");
    assert_eq!(
        parsed.page.timeline[2].source_reference,
        format!("gmail:{}:live:43", connection.id)
    );
    let response = client
        .put(&sync_url)
        .header("cookie", daemon.cookie())
        .json(&json!({"agent_id":daemon.agent_id,"enabled":true,"since":1,
            "filter": pagis_google::gmail_filter::default_filter()}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "the scope change starts a new scan");
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        loop {
            if store
                .status(&key, pagis_core::now_ms())
                .await
                .unwrap()
                .unwrap()
                .caught_up
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the scope-change scan catches up");
    let page = client
        .get(format!(
            "{}/api/v1/memory/file?scope=agent:{}&path=subjects/gmail/school.md",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let page: serde_json::Value = page.json().await.unwrap();
    let parsed = pagis_core::subject_page::SubjectPage::parse(page["content"].as_str().unwrap());
    assert_eq!(
        parsed.page.timeline.len(),
        3,
        "a scope-change scan does not duplicate source versions"
    );
    assert_eq!(brain.reflections.load(Ordering::SeqCst), 2);
    assert_eq!(
        mailbox.searches.load(Ordering::SeqCst),
        0,
        "opted-in account has one provider pipeline"
    );
}

/// A thread the provider cannot deliver fails alone. If the pass aborted
/// before it acknowledged anything, the same 100 rows would come back on
/// every pass and stop every other arrival for good.
#[tokio::test]
async fn an_arrival_the_source_rejects_is_skipped_and_the_others_arrive() {
    use pagis_core::knowledge::{KnowledgeStore, SourceKey};
    use pagis_testkit::TestDaemonOptions;
    use serde_json::json;
    use std::sync::Arc;

    /// Two threads: one ordinary, and one with more messages than the reader
    /// accepts. The oversized thread is refused on every read.
    struct Mailbox {
        at: i64,
    }
    impl Mailbox {
        fn message(&self, id: &str, thread: &str) -> serde_json::Value {
            json!({"id":id, "threadId":thread, "historyId":"40",
                "internalDate":self.at.to_string(), "labelIds":["INBOX"],
                "payload":{"mimeType":"text/plain","headers":[{"name":"From","value":"school@example.com"},{"name":"Subject","value":"Update"}],"body":{"data":"U2Nob29sIHVwZGF0ZQ"}}})
        }
    }
    #[async_trait::async_trait]
    impl pagis_google::GogRunner for Mailbox {
        async fn run(
            &self,
            command: &pagis_google::GogCommand,
        ) -> Result<pagis_google::ProcessOutput, pagis_google::ProcessFailure> {
            let args = command.args();
            let params = args
                .iter()
                .position(|s| s == "--params")
                .map(|i| serde_json::from_str::<serde_json::Value>(&args[i + 1]).unwrap())
                .unwrap_or_default();
            let value = if args.iter().any(|s| s == "users.getProfile") {
                json!({"historyId":"40"})
            } else if args.iter().any(|s| s == "users.messages.list") {
                json!({"messages":[
                    {"id":"small-1","threadId":"small"},
                    {"id":"huge-1","threadId":"huge"}
                ]})
            } else if args.iter().any(|s| s == "users.messages.get") {
                let id = params["id"].as_str().unwrap();
                let thread = if id.starts_with("huge") {
                    "huge"
                } else {
                    "small"
                };
                self.message(id, thread)
            } else if args.iter().any(|s| s == "users.threads.get") {
                match params["id"].as_str().unwrap() {
                    "huge" => json!({"messages": (0..101)
                        .map(|n| self.message(&format!("huge-{n}"), "huge"))
                        .collect::<Vec<_>>()}),
                    _ => json!({"messages":[self.message("small-1", "small")]}),
                }
            } else if args.iter().any(|s| s == "users.history.list") {
                json!({"historyId":"40"})
            } else {
                json!({"messages":[]})
            };
            Ok(pagis_google::ProcessOutput {
                status: Some(0),
                stdout: value.to_string().into_bytes(),
            })
        }
    }

    let daemon = TestDaemon::start_with(TestDaemonOptions {
        gog: Some(Arc::new(Mailbox {
            at: now_ms() - 86_400_000,
        })),
        collector_interval: std::time::Duration::from_millis(100),
        ..Default::default()
    })
    .await;
    let pool = daemon.pool().clone();
    let agent = SqliteAgentStore::new(pool.clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: agent.workspace_id.clone(),
        provider: "google".into(),
        alias: "personal".into(),
        display_name: "Personal".into(),
        status: Connection::CONNECTED.into(),
        auth_mode: Connection::AUTH_MODE_BYO.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: json!({"account":"user@example.com","client":"test"}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(pool.clone())
        .create(&connection)
        .await
        .unwrap();
    let client = reqwest::Client::new();
    assert_eq!(
        client.post(format!("{}/api/v1/grants", daemon.base_url)).header("cookie", daemon.cookie())
            .json(&json!({"agent_id":daemon.agent_id,"connection_id":connection.id,"capabilities":["gmail_read"]}))
            .send().await.unwrap().status(),
        201
    );
    assert_eq!(
        client
            .put(format!(
                "{}/api/v1/connections/{}/sync",
                daemon.base_url, connection.id
            ))
            .header("cookie", daemon.cookie())
            .json(&json!({"agent_id":daemon.agent_id,"enabled":true,"since":0,
            "filter": pagis_google::gmail_filter::default_filter()}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    let key = SourceKey {
        workspace_id: agent.workspace_id,
        connection_id: connection.id.clone(),
        resource: "gmail".into(),
    };
    let store = pagis_storage_sqlite::SqliteKnowledgeStore::new(pool);
    let settled = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let status = store
                .status(&key, pagis_core::now_ms())
                .await
                .unwrap()
                .unwrap();
            if status.arrival_pending == 0 && status.arrival_skipped == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await;
    let status = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert!(
        settled.is_ok(),
        "the rejected arrival is skipped: {status:?}"
    );
    assert_eq!(
        status.arrival_processed, 1,
        "the ordinary arrival reaches its subject page"
    );
    assert_eq!(
        status.arrival_error, None,
        "a skipped item does not leave the resource in error"
    );
    // The arrival the source refused fails alone, and the pass still
    // commits the arrival it read (ADR-0007).
    assert_eq!(
        crate::arrival_commit::arrival_commits(
            &daemon
                .booted
                .home
                .join("memory")
                .join(key.workspace_id.as_str())
        ),
        vec!["Append 1 source arrival to subject pages"],
    );

    let page = client
        .get(format!(
            "{}/api/v1/memory/file?scope=agent:{}&path=subjects/gmail/small.md",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200, "the ordinary thread has a subject page");
    let page: serde_json::Value = page.json().await.unwrap();
    let parsed = pagis_core::subject_page::SubjectPage::parse(page["content"].as_str().unwrap());
    assert_eq!(parsed.page.timeline.len(), 1);
}
