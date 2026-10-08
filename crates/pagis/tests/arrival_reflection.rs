//! A synced source starts one reflection-only Run for each arrival batch.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use pagis_agent::{Brain, BrainError, TurnRequest, TurnStream};
use pagis_core::knowledge::{KnowledgeStore, SourceKey};
use pagis_core::{AgentId, AgentStore, Connection, ConnectionId, RunStore, ScheduleStore, now_ms};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteKnowledgeStore, SqliteRunStore, SqliteScheduleStore,
};
use pagis_testkit::evaluation::FixtureClock;
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use serde_json::json;

struct ArrivalMailbox {
    phase: AtomicUsize,
    at: i64,
    /// The Gmail labels every message of the thread carries.
    labels: Vec<&'static str>,
}

impl ArrivalMailbox {
    /// Mail the default filter reflects: Gmail marked it important.
    fn important(at: i64) -> Self {
        Self {
            phase: AtomicUsize::new(0),
            at,
            labels: vec!["INBOX", "IMPORTANT"],
        }
    }

    fn message(&self, id: &str) -> serde_json::Value {
        let body = match id {
            "mail-1" => "UmVwbHkgYnkgV2VkbmVzZGF5",
            "mail-2" => "QnJpbmcgaW5zdXJhbmNlIGNhcmQ=",
            _ => "VGhlIGNsaW5pYyBjaGFuZ2VkIHRoZSByb29tLg==",
        };
        json!({
            "id": id,
            "threadId": "clinic",
            "historyId": match id { "mail-1" => "43", "mail-2" => "44", _ => "45" },
            "internalDate": self.at.to_string(),
            "labelIds": self.labels,
            "payload": {
                "mimeType": "text/plain",
                "headers": [
                    {"name": "From", "value": "clinic@example.com"},
                    {"name": "Subject", "value": "Clinic visit"}
                ],
                "body": {"data": body}
            }
        })
    }
}

#[async_trait::async_trait]
impl pagis_google::GogRunner for ArrivalMailbox {
    async fn run(
        &self,
        command: &pagis_google::GogCommand,
    ) -> Result<pagis_google::ProcessOutput, pagis_google::ProcessFailure> {
        let args = command.args();
        let params = args
            .iter()
            .position(|value| value == "--params")
            .map(|index| serde_json::from_str::<serde_json::Value>(&args[index + 1]).unwrap())
            .unwrap_or_default();
        let phase = self.phase.load(Ordering::SeqCst);
        let value = if args.iter().any(|value| value == "users.getProfile") {
            json!({"historyId": "42"})
        } else if args.iter().any(|value| value == "users.labels.list") {
            json!({"labels": [{"id": "INBOX"}, {"id": "IMPORTANT"}]})
        } else if args.iter().any(|value| value == "users.messages.list") {
            json!({"messages": []})
        } else if args.iter().any(|value| value == "users.messages.get") {
            self.message(params["id"].as_str().unwrap())
        } else if args.iter().any(|value| value == "users.threads.get") {
            let mut messages = vec![self.message("mail-1"), self.message("mail-2")];
            if phase >= 2 {
                messages.push(self.message("mail-3"));
            }
            json!({"messages": messages})
        } else if args.iter().any(|value| value == "users.history.list") {
            match (phase, params["startHistoryId"].as_str()) {
                (1.., Some("42")) => json!({
                    "historyId": "44",
                    "history": [{"id": "44", "messagesAdded": [
                        {"message": {"id": "mail-1", "threadId": "clinic"}},
                        {"message": {"id": "mail-2", "threadId": "clinic"}}
                    ]}]
                }),
                (2.., Some("44")) => json!({
                    "historyId": "45",
                    "history": [{"id": "45", "messagesAdded": [
                        {"message": {"id": "mail-3", "threadId": "clinic"}}
                    ]}]
                }),
                // Nothing new leaves the cursor where the caller
                // asked from, as Gmail does. Naming a later id here
                // would carry the cursor past a batch a later phase
                // still has to publish, and the poll that read it
                // decided the test rather than the phase.
                _ => json!({
                    "historyId": params["startHistoryId"]
                        .as_str()
                        .unwrap_or(if phase == 0 { "42" } else { "45" })
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

struct ArrivalBrain {
    inner: ScriptedBrain,
    calls: AtomicUsize,
    connection_id: Mutex<Option<String>>,
    schedule_at: i64,
}

#[async_trait::async_trait]
impl Brain for ArrivalBrain {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let script = match call {
            0 => {
                let revision = request
                    .system
                    .split("Pass memory revision `")
                    .nth(1)
                    .and_then(|rest| rest.split('`').next())
                    .expect("memory revision in the prompt");
                let connection = self
                    .connection_id
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("connection id");
                let page = pagis_core::subject_page::SubjectPage {
                    truth: "# Clinic visit\n\nThe owner must reply by Wednesday.".into(),
                    facts: vec![pagis_core::subject_page::Fact {
                        claim: "The owner must reply by Wednesday.".into(),
                        kind: "deadline".into(),
                        source_reference: format!("gmail:{connection}:mail-1:43"),
                        status: pagis_core::subject_page::FactStatus::Active,
                    }],
                    ..Default::default()
                }
                .render();
                Script::tool_call(
                    &[],
                    "memory_write",
                    json!({
                        "path": "private/subjects/gmail/clinic.md",
                        "content": page,
                        "expected_memory_revision": revision
                    }),
                )
            }
            1 => Script::tool_call(
                &[],
                "schedule_create",
                json!({
                    "name": "Clinic reply deadline",
                    "instruction": "Check that the clinic reply was sent",
                    "channel": "user",
                    "kind": "one_shot",
                    "local_time": chrono::DateTime::from_timestamp_millis(self.schedule_at)
                        .unwrap()
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                        .trim_end_matches('Z'),
                    "timezone": "UTC",
                    "wake_only": true,
                    "subject_page_path": "private/subjects/gmail/clinic.md"
                }),
            ),
            2 => Script::reply(&["Recorded the clinic deadline and its wake-up."]),
            _ => Script::hang(&[]),
        };
        self.inner.push(script);
        self.inner.turn(request).await
    }
}

async fn get(daemon: &TestDaemon, path: &str) -> serde_json::Value {
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

async fn message_count(daemon: &TestDaemon) -> usize {
    let channels = get(daemon, "/api/v1/channels").await;
    let mut count = 0;
    for channel in channels["items"].as_array().unwrap() {
        let messages = get(
            daemon,
            &format!(
                "/api/v1/channels/{}/messages",
                channel["id"].as_str().unwrap()
            ),
        )
        .await;
        count += messages["items"].as_array().unwrap().len();
    }
    count
}

/// Waits for `count` arrival Runs, one of which holds any of `states`.
/// A Run in flight is named by every state it can be caught in, because
/// a reply that finished carries the Run on to its reflection and a
/// reader that names one of the two reads how fast the host is rather
/// than what the daemon did.
async fn wait_for_arrival_run(
    daemon: &TestDaemon,
    states: &[&str],
    count: usize,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let runs = get(daemon, "/api/v1/runs").await;
        let arrivals: Vec<&serde_json::Value> = runs["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|run| run["trigger_kind"] == "arrival")
            .collect();
        if arrivals.len() == count
            && let Some(run) = arrivals.iter().find(|run| {
                run["state"]
                    .as_str()
                    .is_some_and(|state| states.contains(&state))
            })
        {
            return (*run).clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{count} arrival Run(s) reach {states:?}: {runs}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_synced_arrival_has_a_model_message_but_no_channel_message_and_cancel_keeps_the_timeline()
{
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let mailbox = Arc::new(ArrivalMailbox::important(now));
    let brain = Arc::new(ArrivalBrain {
        inner: ScriptedBrain::default(),
        calls: AtomicUsize::new(0),
        connection_id: Mutex::new(None),
        schedule_at: now + 86_400_000,
    });
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as Arc<dyn Brain>,
        gog: Some(Arc::clone(&mailbox) as Arc<dyn pagis_google::GogRunner>),
        clock: Arc::new(clock),
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
    *brain.connection_id.lock().unwrap() = Some(connection.id.to_string());
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
    let sync = client
        .put(format!(
            "{}/api/v1/connections/{}/sync",
            daemon.base_url, connection.id
        ))
        .header("cookie", daemon.cookie())
        .json(&json!({
            "agent_id": daemon.agent_id, "enabled": true, "since": 0,
            "filter": pagis_google::gmail_filter::default_filter(),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(sync.status(), 200);
    let source = SourceKey {
        workspace_id: agent.workspace_id.clone(),
        connection_id: connection.id,
        resource: "gmail".into(),
    };
    let knowledge = SqliteKnowledgeStore::new(pool.clone());
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if knowledge
                .status(&source, pagis_core::now_ms())
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
    let messages_before = message_count(&daemon).await;

    mailbox.phase.store(1, Ordering::SeqCst);
    let completed = wait_for_arrival_run(&daemon, &["completed"], 1).await;
    assert!(completed["channel_id"].is_null());
    assert_eq!(
        SqliteRunStore::new(pool.clone())
            .list(&agent.workspace_id, Some(&agent.id), None, &[], None, 20)
            .await
            .unwrap()
            .iter()
            .filter(|run| run.trigger_kind == pagis_core::TriggerKind::Arrival)
            .count(),
        1,
        "two arrivals in one provider batch make one reflection Run"
    );
    let requests = brain.inner.requests();
    assert_eq!(requests.len(), 3, "the Run has only reflection turns");
    assert!(
        requests.iter().all(|request| !request.messages.is_empty()),
        "each arrival reflection request must have a message"
    );
    assert!(requests[0].system.contains("Synced arrival trigger:"));
    assert!(requests[0].system.contains("Reply by Wednesday"));
    assert!(
        requests[0]
            .system
            .contains("you have no Computer in this run")
    );
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|message| message.text.contains("The run is over"))
    );
    assert!(requests.iter().all(|request| !request.computer));
    assert!(requests.iter().all(|request| {
        request
            .tools
            .iter()
            .all(|tool| tool.name.starts_with("memory_") || tool.name.starts_with("schedule_"))
    }));
    let messages_after = message_count(&daemon).await;
    assert_eq!(messages_after, messages_before);
    let file = get(
        &daemon,
        &format!(
            "/api/v1/memory/file?scope=agent:{}&path=subjects/gmail/clinic.md",
            daemon.agent_id
        ),
    )
    .await;
    let page = pagis_core::subject_page::SubjectPage::parse(file["content"].as_str().unwrap());
    assert_eq!(page.page.timeline.len(), 2);
    assert_eq!(page.page.facts.len(), 1);
    assert_eq!(page.page.schedules.len(), 1);
    // The seed gives the Workspace its Report Schedule, which
    // carries no Subject Page; the Reflection's own Schedule does.
    let schedules: Vec<_> = SqliteScheduleStore::new(pool.clone())
        .list(&agent.workspace_id, None, 10)
        .await
        .unwrap()
        .into_iter()
        .filter(|schedule| schedule.subject_page_path.is_some())
        .collect();
    assert_eq!(schedules.len(), 1);
    assert_eq!(
        schedules[0].subject_page_path.as_deref(),
        Some("private/subjects/gmail/clinic.md")
    );
    assert!(
        schedules[0].approved_revision.is_none(),
        "a Subject Page Schedule is wake-only"
    );
    let feed = get(&daemon, "/api/v1/memory/feed").await;
    assert!(
        feed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| { item["kind"] == "committed" && item["run_id"] == completed["id"] })
    );

    mailbox.phase.store(2, Ordering::SeqCst);
    let running = wait_for_arrival_run(&daemon, &["running", "reflecting"], 2).await;
    client
        .post(format!(
            "{}/api/v1/runs/{}/cancel",
            daemon.base_url,
            running["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    wait_for_arrival_run(&daemon, &["canceled"], 2).await;
    let file = get(
        &daemon,
        &format!(
            "/api/v1/memory/file?scope=agent:{}&path=subjects/gmail/clinic.md",
            daemon.agent_id
        ),
    )
    .await;
    let page = pagis_core::subject_page::SubjectPage::parse(file["content"].as_str().unwrap());
    assert_eq!(page.page.timeline.len(), 3);
}
