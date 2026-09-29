//! The Backfill Reflection reflects historical Subject Pages in batches
//! (ADR-0011).

use std::sync::Arc;
use std::time::Duration;

use pagis_agent::{Brain, BrainError, TurnRequest, TurnStream};
use pagis_core::knowledge::{KnowledgeStore, SourceKey};
use pagis_core::{
    AgentId, AgentStore, Connection, ConnectionId, ConnectionStore, WorkspaceId, now_ms,
};
use pagis_storage_sqlite::{SqliteAgentStore, SqliteConnectionStore, SqliteKnowledgeStore};
use pagis_testkit::evaluation::FixtureClock;
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use serde_json::{Value, json};

/// A mailbox of two historical threads: the clinic thread is important,
/// and the shop thread is a promotion the default filter skips.
struct HistoricalMailbox {
    at: i64,
}

impl HistoricalMailbox {
    fn message(&self, id: &str) -> Value {
        let (thread, labels, subject) = match id {
            "mail-1" => ("clinic", vec!["INBOX", "IMPORTANT"], "Clinic visit"),
            _ => ("shop", vec!["INBOX", "CATEGORY_PROMOTIONS"], "Half price"),
        };
        json!({
            "id": id,
            "threadId": thread,
            "historyId": "42",
            "internalDate": self.at.to_string(),
            "labelIds": labels,
            "payload": {
                "mimeType": "text/plain",
                "headers": [
                    {"name": "From", "value": format!("{thread}@example.com")},
                    {"name": "Subject", "value": subject}
                ],
                "body": {"data": "VGhlIG9sZCBtYWls"}
            }
        })
    }
}

#[async_trait::async_trait]
impl pagis_google::GogRunner for HistoricalMailbox {
    async fn run(
        &self,
        command: &pagis_google::GogCommand,
    ) -> Result<pagis_google::ProcessOutput, pagis_google::ProcessFailure> {
        let args = command.args();
        let params = args
            .iter()
            .position(|value| value == "--params")
            .map(|index| serde_json::from_str::<Value>(&args[index + 1]).unwrap())
            .unwrap_or_default();
        let value = if args.iter().any(|value| value == "users.getProfile") {
            json!({"historyId": "42"})
        } else if args.iter().any(|value| value == "users.labels.list") {
            json!({"labels": [{"id": "INBOX"}, {"id": "IMPORTANT"}]})
        } else if args.iter().any(|value| value == "users.messages.list") {
            json!({"messages": [{"id": "mail-1"}, {"id": "mail-2"}]})
        } else if args.iter().any(|value| value == "users.messages.get") {
            self.message(params["id"].as_str().unwrap())
        } else if args.iter().any(|value| value == "users.threads.get") {
            let id = match params["id"].as_str().unwrap() {
                "clinic" => "mail-1",
                _ => "mail-2",
            };
            json!({"messages": [self.message(id)]})
        } else {
            // The history phase adds nothing, so the source is caught up.
            json!({"historyId": "42"})
        };
        Ok(pagis_google::ProcessOutput {
            status: Some(0),
            stdout: value.to_string().into_bytes(),
        })
    }
}

/// A brain that answers every reflection turn without a memory write.
struct QuietBrain {
    inner: ScriptedBrain,
}

#[async_trait::async_trait]
impl Brain for QuietBrain {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        self.inner.push(Script::reply(&["Read the old mail."]));
        self.inner.turn(request).await
    }
}

async fn get(daemon: &TestDaemon, path: &str) -> Value {
    reqwest::Client::new()
        .get(format!("{}{}", daemon.base_url, path))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn arrival_runs(daemon: &TestDaemon) -> usize {
    get(daemon, "/api/v1/runs").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|run| run["trigger_kind"] == "arrival")
        .count()
}

/// Waits until the daemon lists `count` arrival Runs. The backfill pass
/// records its counters and opens the Run in separate writes, so a
/// status that says a page reflected does not yet promise the Run is
/// listed.
async fn wait_for_arrival_runs(daemon: &TestDaemon, count: usize, what: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if arrival_runs(daemon).await == count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}: {count} arrival Run(s) are listed"));
}

/// Waits until `read` holds, then answers the status it read.
async fn wait_for_status(
    knowledge: &SqliteKnowledgeStore,
    key: &SourceKey,
    what: &str,
    read: impl Fn(&pagis_core::knowledge::SyncStatus) -> bool,
) -> pagis_core::knowledge::SyncStatus {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = knowledge
                .status(key, now_ms())
                .await
                .unwrap()
                .expect("the resource is configured");
            if read(&status) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"))
}

async fn configure(daemon: &TestDaemon, connection: &ConnectionId, filter: Value) {
    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/connections/{}/sync",
            daemon.base_url, connection
        ))
        .header("cookie", daemon.cookie())
        .json(&json!({
            "agent_id": daemon.agent_id,
            "enabled": true,
            "since": 0,
            "filter": filter,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

/// Connects one Gmail account with the default filter and answers the
/// key of its sync resource and the knowledge store.
async fn connect(daemon: &TestDaemon) -> (SourceKey, SqliteKnowledgeStore) {
    let pool = daemon.pool().clone();
    let agent = SqliteAgentStore::new(pool.clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let workspace: WorkspaceId = agent.workspace_id.clone();
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace.clone(),
        provider: "google".into(),
        alias: "personal".into(),
        display_name: "Personal".into(),
        status: Connection::CONNECTED.into(),
        auth_mode: Connection::AUTH_MODE_BYO.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: json!({"account": "owner@example.com", "client": "test"}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(pool.clone())
        .create(&connection)
        .await
        .unwrap();
    let grant = reqwest::Client::new()
        .post(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&json!({
            "agent_id": daemon.agent_id,
            "connection_id": connection.id,
            "capabilities": ["gmail_read"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(grant.status(), 201);
    configure(
        daemon,
        &connection.id,
        serde_json::to_value(pagis_google::gmail_filter::default_filter()).unwrap(),
    )
    .await;

    let key = SourceKey {
        workspace_id: workspace.clone(),
        connection_id: connection.id.clone(),
        resource: "gmail".into(),
    };
    let knowledge = SqliteKnowledgeStore::new(pool.clone());
    (key, knowledge)
}

#[tokio::test]
async fn a_caught_up_resource_reflects_its_selected_historical_pages() {
    let now = 1_789_041_600_000;
    let brain = Arc::new(QuietBrain {
        inner: ScriptedBrain::default(),
    });
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as Arc<dyn Brain>,
        gog: Some(Arc::new(HistoricalMailbox { at: now }) as Arc<dyn pagis_google::GogRunner>),
        clock: Arc::new(FixtureClock::at(now)),
        collector_interval: Duration::from_millis(100),
        ..Default::default()
    })
    .await;
    let (key, knowledge) = connect(&daemon).await;
    let status = wait_for_status(
        &knowledge,
        &key,
        "the backfill reflects the important thread",
        |status| status.backfill_reflected > 0,
    )
    .await;
    assert!(status.caught_up);
    assert_eq!(status.backfill_reflected, 1, "one page reaches a Run");
    assert_eq!(status.backfill_pending, 0, "no selected page waits");
    assert_eq!(status.reflected_pages, 1);
    assert_eq!(status.skipped_pages, 1, "the promotion starts no Run");
    assert!(!status.backfill_capped);
    wait_for_arrival_runs(&daemon, 1, "the selected page reaches a Run").await;

    // The briefing says that the pages are historical, and it names the
    // selected page alone.
    let request = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(request) = brain
                .inner
                .requests()
                .into_iter()
                .find(|request| request.system.contains("Synced arrival trigger:"))
            {
                return request;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the backfill Run briefs the agent");
    assert!(
        request
            .system
            .contains("These Subject Pages are historical. Record the facts."),
        "{}",
        request.system
    );
    assert!(
        request
            .system
            .contains("Set a wake-up only when the act-by moment is still in the future."),
        "{}",
        request.system
    );
    assert!(
        request.system.contains("private/subjects/gmail/clinic.md"),
        "{}",
        request.system
    );
    assert!(
        !request.system.contains("private/subjects/gmail/shop.md"),
        "a skipped page is not in the briefing"
    );

    // A version acquired without metadata gives the filter nothing to
    // read. The collect pass completes the metadata from the provider
    // before the backfill decides again, so the promotion still skips
    // and the important thread reflects once more (ADR-0011).
    let pool = daemon.pool().clone();
    sqlx::query("UPDATE source_versions SET record = json_set(record, '$.metadata', json('null'))")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM page_reflections")
        .execute(&pool)
        .await
        .unwrap();
    let status = wait_for_status(
        &knowledge,
        &key,
        "the pass completes the metadata and the backfill decides again",
        |status| status.incomplete_versions == 0 && status.backfill_reflected == 1,
    )
    .await;
    assert_eq!(
        status.skipped_pages, 1,
        "the completed promotion skips again"
    );
    assert_eq!(status.backfill_pending, 0);
    wait_for_arrival_runs(&daemon, 2, "the completed metadata reflects again").await;

    // A new filter decides about every page again, so the page the old
    // filter skipped now reflects.
    configure(
        &daemon,
        &key.connection_id,
        json!({"rules": [], "default": "reflect"}),
    )
    .await;
    let status = wait_for_status(
        &knowledge,
        &key,
        "the new filter revision reflects both pages",
        |status| status.backfill_reflected == 2,
    )
    .await;
    assert_eq!(status.filter_revision, 2);
    assert_eq!(status.skipped_pages, 0, "the new revision skips nothing");
    wait_for_arrival_runs(&daemon, 3, "one batch starts one Run").await;
}

/// The brain fails the first backfill reflection turn and answers every
/// later one.
struct FlakyBrain {
    inner: ScriptedBrain,
    failed: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl Brain for FlakyBrain {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        let backfill = request.system.contains("Synced arrival trigger:");
        if backfill && !self.failed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            self.inner.push(Script::fail_after(
                &[],
                "no stream event for 90s (idle timeout)",
            ));
        } else {
            self.inner.push(Script::reply(&["Read the old mail."]));
        }
        self.inner.turn(request).await
    }
}

/// A backfill Run whose reflection turn fails ends failed, and its pages
/// wait for the next Run (ADR-0011).
#[tokio::test]
async fn a_failed_backfill_run_reflects_its_pages_again() {
    let now = 1_789_041_600_000;
    let brain = Arc::new(FlakyBrain {
        inner: ScriptedBrain::default(),
        failed: std::sync::atomic::AtomicBool::new(false),
    });
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as Arc<dyn Brain>,
        gog: Some(Arc::new(HistoricalMailbox { at: now }) as Arc<dyn pagis_google::GogRunner>),
        clock: Arc::new(FixtureClock::at(now)),
        collector_interval: Duration::from_millis(100),
        ..Default::default()
    })
    .await;
    let (key, knowledge) = connect(&daemon).await;

    let runs = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let runs = get(&daemon, "/api/v1/runs").await["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|run| run["trigger_kind"] == "arrival")
                .cloned()
                .collect::<Vec<_>>();
            if runs.len() == 2 && runs.iter().any(|run| run["state"] == "completed") {
                return runs;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the failed batch starts a second Run");
    let failed = runs
        .iter()
        .find(|run| run["state"] == "failed")
        .expect("the first Run failed");
    assert_eq!(failed["failure_kind"], "model_failed");
    assert!(
        failed["error"]
            .as_str()
            .unwrap_or_default()
            .contains("idle timeout"),
        "{failed}"
    );
    let status = wait_for_status(&knowledge, &key, "the page reflected once", |status| {
        status.backfill_reflected == 1
    })
    .await;
    assert_eq!(status.backfill_pending, 0);
    assert_eq!(status.backfill_failed, 0);
    let requests = brain.inner.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.system.contains("private/subjects/gmail/clinic.md"))
            .count(),
        2,
        "the same page reaches both Runs"
    );
}
