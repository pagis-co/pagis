//! Full-daemon incoming-event tests (ADR-0006).
//!
//! These drive REST and the WebSocket firehose against a daemon whose
//! `gog` is a fake, and prove the behavior end to end: one collection
//! pass per Connection, one Run per matching rule, no replay across an
//! overlapping window, and a revoked grant that withdraws pending work
//! and blocks the rule.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use pagis_core::{AgentId, GrantId, WorkspaceId, now_ms};
use pagis_google::{GogCommand, GogRunner, ProcessFailure, ProcessOutput};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const CONNECTION_ID: &str = "connection-google";

/// How often the daemon polls the Connection in these tests. The
/// assertions on the number of collection passes count ticks of this.
const COLLECTOR_INTERVAL: Duration = Duration::from_millis(100);

/// A `gog` that answers Gmail searches from a scripted mailbox. The
/// mailbox can grow between passes, the way a real one does.
#[derive(Default)]
struct FakeGog {
    mailbox: Mutex<Vec<serde_json::Value>>,
    searches: Mutex<usize>,
}

impl FakeGog {
    fn deliver(&self, id: &str, from: &str, subject: &str, at: i64) {
        self.mailbox.lock().unwrap().push(serde_json::json!({
            "id": id,
            "threadId": format!("t-{id}"),
            "from": from,
            "subject": subject,
            "to": ["user@example.com"],
            "internalDate": at,
        }));
    }

    fn searches(&self) -> usize {
        *self.searches.lock().unwrap()
    }
}

#[async_trait]
impl GogRunner for FakeGog {
    async fn run(&self, command: &GogCommand) -> Result<ProcessOutput, ProcessFailure> {
        if command.args().iter().any(|arg| arg == "search") {
            *self.searches.lock().unwrap() += 1;
        }
        let messages = self.mailbox.lock().unwrap().clone();
        Ok(ProcessOutput {
            status: Some(0),
            stdout: serde_json::json!({"messages": messages})
                .to_string()
                .into_bytes(),
        })
    }
}

struct Harness {
    daemon: TestDaemon,
    brain: Arc<ScriptedBrain>,
    gog: Arc<FakeGog>,
}

async fn boot() -> Harness {
    let brain = Arc::new(ScriptedBrain::default());
    let gog = Arc::new(FakeGog::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        gog: Some(Arc::clone(&gog) as _),
        collector_interval: COLLECTOR_INTERVAL,
        ..TestDaemonOptions::default()
    })
    .await;
    connect_google(&daemon).await;
    grant(&daemon, &["gmail_read"]).await;
    Harness { daemon, brain, gog }
}

async fn workspace_id(daemon: &TestDaemon) -> WorkspaceId {
    use pagis_core::AgentStore;
    pagis_storage_sqlite::SqliteAgentStore::new(daemon.pool().clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap()
        .workspace_id
}

/// Seed a connected Google Connection. The connect flow that writes
/// this row for real has its own tests.
async fn connect_google(daemon: &TestDaemon) {
    daemon
        .plant_google_connection(&pagis_core::Connection {
            id: pagis_core::ConnectionId::from(CONNECTION_ID.to_string()),
            workspace_id: workspace_id(daemon).await,
            provider: "google".to_string(),
            alias: "work".to_string(),
            display_name: "Google".to_string(),
            status: pagis_core::Connection::CONNECTED.to_string(),
            authorized_capabilities: Vec::new(),
            config: serde_json::json!({"account": "alice@example.com", "client": "work"}),
            created_at: now_ms(),
        })
        .await;
}

async fn grant(daemon: &TestDaemon, capabilities: &[&str]) -> GrantId {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "connection_id": CONNECTION_ID,
            "capabilities": capabilities,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let body: serde_json::Value = response.json().await.unwrap();
    GrantId::from(body["id"].as_str().unwrap().to_string())
}

async fn get(daemon: &TestDaemon, path: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{}{}", daemon.base_url, path))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("GET")
        .error_for_status()
        .expect("success")
        .json()
        .await
        .expect("JSON")
}

async fn subscribe(daemon: &TestDaemon, name: &str, filter: serde_json::Value) -> String {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/event-subscriptions", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "connection_id": CONNECTION_ID,
            "event_kind": "mail.message_received",
            "name": name,
            "instruction": format!("Handle {name}"),
            "channel_id": daemon.dm_channel_id,
            "filter": filter,
        }))
        .send()
        .await
        .expect("create subscription");
    let status = response.status();
    let body: serde_json::Value = response.json().await.expect("JSON");
    assert_eq!(status, 201, "subscription create failed: {body}");
    body["id"].as_str().expect("subscription id").to_string()
}

/// Wait until the collector has run at least `passes` searches, which
/// is how a test knows the baseline is established.
async fn wait_for_searches(harness: &Harness, passes: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while harness.gog.searches() < passes {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the collector ran {passes} times"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_event_runs(
    daemon: &TestDaemon,
    state: &str,
    count: usize,
) -> Vec<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let page = get(daemon, "/api/v1/runs").await;
        let runs: Vec<serde_json::Value> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|run| run["trigger_kind"] == "event" && run["state"] == state)
            .cloned()
            .collect();
        if runs.len() >= count {
            return runs;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{count} event Runs reached {state}; saw {}",
            serde_json::to_string(&page["items"]).unwrap()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn three_rules_on_one_connection_produce_three_independent_runs() {
    let harness = boot().await;
    for _ in 0..3 {
        harness
            .brain
            .push_for("Pixie", Script::reply(&["Handled the mail."]));
    }
    let first = subscribe(
        &harness.daemon,
        "first",
        serde_json::json!({"mailbox": "work"}),
    )
    .await;
    subscribe(
        &harness.daemon,
        "second",
        serde_json::json!({"senders": ["ada@example.com"]}),
    )
    .await;
    subscribe(
        &harness.daemon,
        "third",
        serde_json::json!({"subject_contains": ["report"]}),
    )
    .await;
    wait_for_searches(&harness, 1).await;

    let before = harness.gog.searches();
    let watched_from = tokio::time::Instant::now();
    harness
        .gog
        .deliver("m1", "Ada <ada@example.com>", "Weekly report", now_ms());
    let runs = wait_for_event_runs(&harness.daemon, "completed", 3).await;

    assert_eq!(runs.len(), 3, "each rule keeps its own instruction");
    // One pass per tick carries all three rules. The wait above takes
    // as many ticks as the machine needs, so the bound counts the ticks
    // that went by; a collector that polled per rule would run three
    // passes a tick and break it on any machine.
    let passes = (harness.gog.searches() - before) as u128;
    let ticks = watched_from.elapsed().as_millis() / COLLECTOR_INTERVAL.as_millis() + 2;
    assert!(
        passes <= ticks,
        "three rules share one collection pass per tick, saw {passes} passes in {ticks} ticks"
    );

    let wakeups = get(
        &harness.daemon,
        &format!("/api/v1/event-subscriptions/{first}/wakeups"),
    )
    .await;
    assert_eq!(wakeups["items"].as_array().unwrap().len(), 1);
    assert_eq!(wakeups["items"][0]["source_kind"], "event_subscription");
    assert_eq!(wakeups["items"][0]["state"], "started");

    let events = get(
        &harness.daemon,
        &format!("/api/v1/event-subscriptions/{first}/events"),
    )
    .await;
    assert_eq!(events["items"].as_array().unwrap().len(), 1);
    assert_eq!(events["items"][0]["provider_event_id"], "m1");
    assert!(
        events["items"][0]["metadata"].get("body").is_none(),
        "no body is stored"
    );

    let detail = get(
        &harness.daemon,
        &format!("/api/v1/event-subscriptions/{first}"),
    )
    .await;
    assert_eq!(detail["state"], "active");
    assert_eq!(detail["creator"], "user");
    assert_eq!(detail["last_successful_collection"]["outcome"], "collected");
    assert_eq!(detail["last_matched_event"]["provider_event_id"], "m1");
}

#[tokio::test]
async fn the_briefing_names_the_mail_and_marks_it_untrusted() {
    let harness = boot().await;
    harness
        .brain
        .push_for("Pixie", Script::reply(&["Read it."]));
    subscribe(
        &harness.daemon,
        "inbox",
        serde_json::json!({"mailbox": "work"}),
    )
    .await;
    wait_for_searches(&harness, 1).await;
    harness
        .gog
        .deliver("m1", "Ada <ada@example.com>", "Weekly report", now_ms());
    wait_for_event_runs(&harness.daemon, "completed", 1).await;

    let request = harness
        .brain
        .requests()
        .into_iter()
        .find(|request| request.system.contains("Incoming event trigger:"))
        .expect("the event turn's request");
    assert!(request.system.contains("Handle inbox"));
    assert!(request.system.contains("ada@example.com"));
    assert!(request.system.contains("Weekly report"));
    assert!(request.system.contains("Connection: work"));

    // The mail metadata sits inside one envelope that names the event
    // kind and the Connection, and the envelope ends after the rows
    // (ADR-0005).
    let begin = request
        .system
        .find("[BEGIN UNTRUSTED source=event:mail.message_received@work]")
        .expect("the envelope around the mail metadata");
    let end = request
        .system
        .find("[END UNTRUSTED source=event:mail.message_received@work]")
        .expect("the end of the envelope");
    assert!(begin < end);
    let inside = &request.system[begin..end];
    assert!(inside.contains("ada@example.com"));
    assert!(inside.contains("Weekly report"));
}

#[tokio::test]
async fn a_large_batch_makes_one_run_and_the_overlap_replays_nothing() {
    let harness = boot().await;
    harness
        .brain
        .push_for("Pixie", Script::reply(&["Handled the batch."]));
    let subscription = subscribe(
        &harness.daemon,
        "inbox",
        serde_json::json!({"mailbox": "work"}),
    )
    .await;
    wait_for_searches(&harness, 1).await;

    let at = now_ms();
    for index in 0..8 {
        harness.gog.deliver(
            &format!("m{index}"),
            "ada@example.com",
            "Batch",
            at + i64::from(index),
        );
    }
    wait_for_event_runs(&harness.daemon, "completed", 1).await;
    // Let several more overlapping passes run over the same mailbox.
    let passes = harness.gog.searches();
    wait_for_searches(&harness, passes + 3).await;

    let runs = get(&harness.daemon, "/api/v1/runs").await;
    let event_runs = runs["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|run| run["trigger_kind"] == "event")
        .count();
    assert_eq!(event_runs, 1, "one Run for the batch, not one per message");

    let wakeups = get(
        &harness.daemon,
        &format!("/api/v1/event-subscriptions/{subscription}/wakeups"),
    )
    .await;
    assert_eq!(wakeups["items"].as_array().unwrap().len(), 1);
    assert_eq!(wakeups["items"][0]["source_count"], 8);

    let events = get(
        &harness.daemon,
        &format!("/api/v1/event-subscriptions/{subscription}/events"),
    )
    .await;
    assert_eq!(
        events["items"].as_array().unwrap().len(),
        8,
        "the overlapping window stores no duplicate"
    );
}

#[tokio::test]
async fn mail_from_before_the_first_activation_never_replays() {
    let harness = boot().await;
    harness
        .gog
        .deliver("old", "ada@example.com", "Old news", now_ms() - 600_000);
    subscribe(
        &harness.daemon,
        "inbox",
        serde_json::json!({"mailbox": "work"}),
    )
    .await;
    wait_for_searches(&harness, 3).await;

    let runs = get(&harness.daemon, "/api/v1/runs").await;
    assert!(
        runs["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|run| run["trigger_kind"] != "event"),
        "the baseline emits nothing"
    );
}

#[tokio::test]
async fn revoking_the_grant_withdraws_pending_work_and_blocks_the_rule() {
    let brain = Arc::new(ScriptedBrain::default());
    let gog = Arc::new(FakeGog::default());
    // No Run capacity, so the Wake-up stays pending and the test can
    // watch it be withdrawn rather than started.
    let mut options = TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        gog: Some(Arc::clone(&gog) as Arc<dyn GogRunner>),
        collector_interval: COLLECTOR_INTERVAL,
        ..TestDaemonOptions::default()
    };
    options.agents.max_concurrent_runs = 0;
    let daemon = TestDaemon::start_with(options).await;
    connect_google(&daemon).await;
    let grant_id = grant(&daemon, &["gmail_read"]).await;
    let subscription = subscribe(&daemon, "inbox", serde_json::json!({"mailbox": "work"})).await;
    let harness = Harness { daemon, brain, gog };
    wait_for_searches(&harness, 1).await;
    harness
        .gog
        .deliver("m1", "ada@example.com", "Pending", now_ms());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let wakeups = get(
            &harness.daemon,
            &format!("/api/v1/event-subscriptions/{subscription}/wakeups"),
        )
        .await;
        if wakeups["items"][0]["state"] == "pending" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "a pending Wake-up was created"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/grants/{grant_id}",
            harness.daemon.base_url
        ))
        .header("cookie", harness.daemon.cookie())
        .send()
        .await
        .expect("revoke")
        .error_for_status()
        .expect("revoked");
    harness
        .gog
        .deliver("m2", "ada@example.com", "After revocation", now_ms());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let detail = get(
            &harness.daemon,
            &format!("/api/v1/event-subscriptions/{subscription}"),
        )
        .await;
        if detail["state"] == "blocked" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the rule was blocked"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let wakeups = get(
        &harness.daemon,
        &format!("/api/v1/event-subscriptions/{subscription}/wakeups"),
    )
    .await;
    assert!(
        wakeups["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|wakeup| wakeup["state"] == "withdrawn"),
        "pending work is withdrawn"
    );
}

#[tokio::test]
async fn pausing_a_rule_stops_the_collector_and_publishes_the_change() {
    let harness = boot().await;
    let (mut socket, _) = connect_async(harness.daemon.ws_request(&harness.daemon.ws_url()))
        .await
        .expect("WS connect");
    socket
        .send(Message::text(
            serde_json::json!({ "type": "auth" }).to_string(),
        ))
        .await
        .expect("WS auth");
    // The daemon attaches the socket to the event bus and then sends
    // `ready`. A rule created before that frame publishes its
    // `event_subscription.created` event to nobody.
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("a ready frame")
            .expect("socket open")
            .expect("WS frame");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        if frame["type"] == "ready" {
            break;
        }
    }
    let subscription = subscribe(
        &harness.daemon,
        "inbox",
        serde_json::json!({"mailbox": "work"}),
    )
    .await;
    wait_for_searches(&harness, 1).await;

    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/event-subscriptions/{subscription}",
            harness.daemon.base_url
        ))
        .header("cookie", harness.daemon.cookie())
        .json(&serde_json::json!({"action": "pause"}))
        .send()
        .await
        .expect("pause")
        .error_for_status()
        .expect("paused");

    let passes = harness.gog.searches();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        harness.gog.searches(),
        passes,
        "a paused rule leaves no collector running"
    );

    let mut seen = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !seen.contains("event_subscription.state_changed") {
        let frame = tokio::time::timeout_at(deadline, socket.next())
            .await
            .expect("a state change event")
            .expect("socket open")
            .expect("WS frame");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        if let Some(event_type) = frame["type"].as_str() {
            seen.insert(event_type.to_string());
        }
    }
    assert!(seen.contains("event_subscription.created"));
}
