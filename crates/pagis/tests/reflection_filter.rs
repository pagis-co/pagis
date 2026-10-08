//! The Reflection Filter gates the live arrival Run (ADR-0011).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use pagis_core::knowledge::{KnowledgeStore, SourceKey};
use pagis_core::{AgentId, AgentStore, Connection, ConnectionId, WorkspaceId, now_ms};
use pagis_storage_sqlite::{SqliteAgentStore, SqliteKnowledgeStore};
use pagis_testkit::evaluation::FixtureClock;
use pagis_testkit::{ScriptedBrain, TestDaemon, TestDaemonOptions};
use serde_json::{Value, json};

/// One promotions thread. The default filter skips it.
struct PromotionsMailbox {
    phase: AtomicUsize,
    at: i64,
    /// The `From` header of the one message.
    from: &'static str,
}

/// The sender of the promotions thread.
const SHOP: &str = "Shop <offers@shop.test>";

impl PromotionsMailbox {
    fn message(&self) -> Value {
        json!({
            "id": "mail-1",
            "threadId": "shop",
            "historyId": "43",
            "internalDate": self.at.to_string(),
            "labelIds": ["INBOX", "CATEGORY_PROMOTIONS"],
            "payload": {
                "mimeType": "text/plain",
                "headers": [
                    {"name": "From", "value": self.from},
                    {"name": "Subject", "value": "Half price this week"}
                ],
                "body": {"data": "SGFsZiBwcmljZSB0aGlzIHdlZWs="}
            }
        })
    }
}

#[async_trait::async_trait]
impl pagis_google::GogRunner for PromotionsMailbox {
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
        let phase = self.phase.load(Ordering::SeqCst);
        let value = if args.iter().any(|value| value == "users.getProfile") {
            json!({"historyId": "42"})
        } else if args.iter().any(|value| value == "users.labels.list") {
            json!({"labels": [{"id": "INBOX"}, {"id": "Receipts"}]})
        } else if args.iter().any(|value| value == "users.messages.get") {
            self.message()
        } else if args.iter().any(|value| value == "users.threads.get") {
            json!({"messages": [self.message()]})
        } else if args.iter().any(|value| value == "users.history.list") {
            // The batch arrives once, from the baseline cursor alone.
            // Answering it on every poll would append the same arrival
            // again whenever the pass ran twice before the test read
            // the counters.
            match (phase, params["startHistoryId"].as_str()) {
                (1.., Some("42")) => json!({
                    "historyId": "43",
                    "history": [{"id": "43", "messagesAdded": [
                        {"message": {"id": "mail-1", "threadId": "shop"}}
                    ]}]
                }),
                // Nothing new leaves the cursor where the caller asked
                // from, as Gmail does.
                _ => json!({
                    "historyId": params["startHistoryId"].as_str().unwrap_or("42")
                }),
            }
        } else {
            json!({"messages": []})
        };
        Ok(pagis_google::ProcessOutput {
            status: Some(0),
            stdout: value.to_string().into_bytes(),
        })
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

struct Fixture {
    daemon: TestDaemon,
    connection: ConnectionId,
    workspace: WorkspaceId,
}

async fn start(now: i64, mailbox: Arc<PromotionsMailbox>) -> Fixture {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::new(ScriptedBrain::default()),
        gog: Some(mailbox as Arc<dyn pagis_google::GogRunner>),
        clock: Arc::new(FixtureClock::at(now)),
        collector_interval: Duration::from_millis(100),
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
        authorized_capabilities: vec!["gmail_read".into()],
        config: json!({"account": "owner@example.com", "client": "test"}),
        created_at: now_ms(),
    };
    daemon.plant_google_connection(&connection).await;
    let client = reqwest::Client::new();
    let grant = client
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
    Fixture {
        connection: connection.id,
        workspace: agent.workspace_id,
        daemon,
    }
}

async fn configure(fixture: &Fixture, filter: &Value) -> reqwest::StatusCode {
    reqwest::Client::new()
        .put(format!(
            "{}/api/v1/connections/{}/sync",
            fixture.daemon.base_url, fixture.connection
        ))
        .header("cookie", fixture.daemon.cookie())
        .json(&json!({
            "agent_id": fixture.daemon.agent_id,
            "enabled": true,
            "since": 0,
            "filter": filter,
        }))
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_skipped_page_keeps_its_timeline_and_starts_no_run() {
    let now = 1_789_041_600_000;
    let mailbox = Arc::new(PromotionsMailbox {
        phase: AtomicUsize::new(0),
        at: now,
        from: SHOP,
    });
    let fixture = start(now, Arc::clone(&mailbox)).await;
    let catalogue = get(
        &fixture.daemon,
        &format!("/api/v1/connections/{}/sync/catalogue", fixture.connection),
    )
    .await;
    let signals = catalogue["catalogue"]["signals"].as_array().unwrap();
    assert!(signals.iter().any(|signal| signal["id"] == "sender_trust"));
    let labels = signals
        .iter()
        .find(|signal| signal["id"] == "labels")
        .unwrap();
    assert!(
        labels["kind"]["options"]
            .as_array()
            .unwrap()
            .contains(&json!("Receipts")),
        "the catalogue offers the account's own labels"
    );
    assert_eq!(configure(&fixture, &catalogue["default_filter"]).await, 200);

    let key = SourceKey {
        workspace_id: fixture.workspace.clone(),
        connection_id: fixture.connection.clone(),
        resource: "gmail".into(),
    };
    let knowledge = SqliteKnowledgeStore::new(fixture.daemon.pool().clone());
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if knowledge
                .status(&key, pagis_core::now_ms())
                .await
                .unwrap()
                .unwrap()
                .caught_up
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the source reaches its baseline");

    mailbox.phase.store(1, Ordering::SeqCst);
    let status = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let status = knowledge
                .status(&key, pagis_core::now_ms())
                .await
                .unwrap()
                .unwrap();
            // The skip and the arrival acknowledgement are two writes.
            if status.skipped_pages == 1 && status.arrival_processed == 1 {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the filter skips the promotions page, and the arrival still appends");
    assert_eq!(status.reflected_pages, 0);
    assert_eq!(arrival_runs(&fixture.daemon).await, 0);

    // The page itself is complete: the filter gates reflection, never
    // the Timeline.
    let file = get(
        &fixture.daemon,
        &format!(
            "/api/v1/memory/file?scope=agent:{}&path=subjects/gmail/shop.md",
            fixture.daemon.agent_id
        ),
    )
    .await;
    let page = pagis_core::subject_page::SubjectPage::parse(file["content"].as_str().unwrap());
    assert_eq!(page.page.timeline.len(), 1);
}

async fn preview(fixture: &Fixture, filter: &Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/connections/{}/sync/filter/preview",
            fixture.daemon.base_url, fixture.connection
        ))
        .header("cookie", fixture.daemon.cookie())
        .json(filter)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn a_preview_counts_the_stored_pages_under_a_filter_and_under_one_rule() {
    let now = 1_789_041_600_000;
    let mailbox = Arc::new(PromotionsMailbox {
        phase: AtomicUsize::new(0),
        at: now,
        from: SHOP,
    });
    let fixture = start(now, Arc::clone(&mailbox)).await;
    let catalogue = get(
        &fixture.daemon,
        &format!("/api/v1/connections/{}/sync/catalogue", fixture.connection),
    )
    .await;
    let default_filter = &catalogue["default_filter"];

    // No stored page yet: every count is zero, one per rule.
    let empty: Value = preview(&fixture, default_filter)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["rules"].as_array().unwrap().len(), 4);

    assert_eq!(configure(&fixture, default_filter).await, 200);
    let key = SourceKey {
        workspace_id: fixture.workspace.clone(),
        connection_id: fixture.connection.clone(),
        resource: "gmail".into(),
    };
    let knowledge = SqliteKnowledgeStore::new(fixture.daemon.pool().clone());
    mailbox.phase.store(1, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let status = knowledge
                .status(&key, pagis_core::now_ms())
                .await
                .unwrap()
                .unwrap();
            if status.skipped_pages == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the filter skips the promotions page");

    let whole: Value = preview(&fixture, default_filter)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        whole,
        // The promotions page matches no reflect rule, so the last
        // line of the filter skips it and no rule counts it.
        json!({"total": 1, "reflect": 0, "skip": 1, "rules": [0, 0, 0, 0]})
    );

    // One rule alone gives the rule editor its line.
    let one_rule: Value = preview(
        &fixture,
        &json!({
            "rules": [{"verdict": "reflect", "conditions": [
                {"signal": "sender_domain", "operator": "is",
                 "value": {"kind": "text", "value": "shop.test"}}
            ]}],
            "default": "skip"
        }),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(
        one_rule,
        json!({"total": 1, "reflect": 1, "skip": 0, "rules": [1]})
    );

    let refused = preview(
        &fixture,
        &json!({"rules": [{"verdict": "skip", "conditions": [
            {"signal": "sender_mood", "operator": "is",
             "value": {"kind": "text", "value": "cold"}}
        ]}], "default": "reflect"}),
    )
    .await;
    assert_eq!(refused.status(), 422);
}

#[tokio::test]
async fn a_filter_the_catalogue_refuses_names_the_rule_and_the_condition() {
    let now = 1_789_041_600_000;
    let fixture = start(
        now,
        Arc::new(PromotionsMailbox {
            phase: AtomicUsize::new(0),
            at: now,
            from: SHOP,
        }),
    )
    .await;
    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/connections/{}/sync",
            fixture.daemon.base_url, fixture.connection
        ))
        .header("cookie", fixture.daemon.cookie())
        .json(&json!({
            "agent_id": fixture.daemon.agent_id,
            "enabled": true,
            "since": 0,
            "filter": {
                "rules": [
                    {"verdict": "reflect", "conditions": [
                        {"signal": "labels", "operator": "has",
                         "value": {"kind": "choice", "value": "IMPORTANT"}}
                    ]},
                    {"verdict": "skip", "conditions": [
                        {"signal": "sender_mood", "operator": "is",
                         "value": {"kind": "text", "value": "cold"}}
                    ]}
                ],
                "default": "reflect"
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    let body: Value = response.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("Rule 2, condition 1") && message.contains("sender_mood"),
        "{message}"
    );
}

/// A `From` header that names the Person's own Connection account proves
/// nothing. A Connection page stores no verified authentication result,
/// so the `sender_trust` signal reads its sender as Unknown, both when
/// the arrival is gated and in the preview (ADR-0019).
#[tokio::test]
async fn a_connection_page_with_no_stored_verified_result_is_unknown_to_sender_trust() {
    let now = 1_789_041_600_000;
    let mailbox = Arc::new(PromotionsMailbox {
        phase: AtomicUsize::new(0),
        at: now,
        // The account of the Connection that `start` makes.
        from: "Me <owner@example.com>",
    });
    let fixture = start(now, Arc::clone(&mailbox)).await;
    let trusted_senders = json!({
        "rules": [{"verdict": "reflect", "conditions": [
            {"signal": "sender_trust", "operator": "in",
             "value": {"kind": "choices", "values": ["owner", "trusted"]}}
        ]}],
        "default": "skip"
    });
    assert_eq!(configure(&fixture, &trusted_senders).await, 200);

    let key = SourceKey {
        workspace_id: fixture.workspace.clone(),
        connection_id: fixture.connection.clone(),
        resource: "gmail".into(),
    };
    let knowledge = SqliteKnowledgeStore::new(fixture.daemon.pool().clone());
    mailbox.phase.store(1, Ordering::SeqCst);
    let status = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let status = knowledge
                .status(&key, pagis_core::now_ms())
                .await
                .unwrap()
                .unwrap();
            if status.skipped_pages + status.reflected_pages == 1 {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the filter decides the page");
    assert_eq!(
        (status.reflected_pages, status.skipped_pages),
        (0, 1),
        "the arrival gate read the sender as Unknown"
    );

    let counted: Value = preview(&fixture, &trusted_senders)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        counted,
        json!({"total": 1, "reflect": 0, "skip": 1, "rules": [0]})
    );
}
