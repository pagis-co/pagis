//! Mail through the whole run loop. The other mail files test
//! one seam each; this one follows a message from the host to what the
//! Agent does about it, and a send from the model to the wire.
//!
//! Covered here: owner mail that reaches the Agent with the standing of
//! the user, the appointment chain (mail, note, one-shot
//! Schedule, a second Run that messages the user), the Outgoing Cap
//! refusing a send before any card, and a `dormant` mailbox refusing
//! one.
//!
//! The mail host and transport are fakes, so no socket opens. Every
//! wait polls to a deadline; nothing sleeps for effect.

use std::sync::Arc;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use pagis_core::{AgentMailboxId, AgentMailboxState, AgentMailboxStore, Clock, SystemClock};
use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost, raw_message};
use pagis_testkit::evaluation::FixtureClock;
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use serde_json::Value;

/// How long a wait may take before the test calls it a failure. The
/// gate runs several test binaries at once, so a poll needs room.
const DEADLINE: Duration = Duration::from_secs(20);

/// How often a wait looks again.
const POLL: Duration = Duration::from_millis(25);

struct Harness {
    daemon: TestDaemon,
    brain: Arc<ScriptedBrain>,
    transport: Arc<FakeMailTransport>,
}

async fn boot() -> Harness {
    boot_on(Arc::new(SystemClock)).await
}

/// The harness on this clock. A clock the test moves fires a Schedule
/// when the test says, and not after a real wait.
async fn boot_on(clock: Arc<dyn Clock>) -> Harness {
    let brain = Arc::new(ScriptedBrain::default());
    let transport = Arc::new(FakeMailTransport::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        mail_host: Arc::new(FakeMailboxHost::default()),
        mail_transport: Arc::clone(&transport) as _,
        collector_interval: Duration::from_millis(50),
        clock,
        ..TestDaemonOptions::default()
    })
    .await;
    Harness {
        daemon,
        brain,
        transport,
    }
}

impl Harness {
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}{path}", self.daemon.base_url))
            .header("cookie", self.daemon.cookie());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        (
            status,
            response.json().await.unwrap_or(serde_json::json!({})),
        )
    }

    async fn get(&self, path: &str) -> serde_json::Value {
        self.request(reqwest::Method::GET, path, None).await.1
    }

    async fn post(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        self.request(reqwest::Method::POST, path, Some(body)).await
    }

    /// Give the daemon's own Agent a mailbox on a Migadu Connection and
    /// wait for the login proof to settle. One Agent keeps the shared
    /// script queue in order, which is what the reflection turns need.
    async fn hold_mailbox(&self, outgoing_cap: u32) -> serde_json::Value {
        let connection_id = self
            .daemon
            .connect_installation(
                "migadu",
                serde_json::json!({
                    "account": "owner@example.com",
                    "api_key": "migadu-key",
                    "domain": "example.com",
                }),
            )
            .await;
        let (status, body) = self
            .post(
                &format!("/api/v1/agents/{}/mailbox", self.daemon.agent_id),
                serde_json::json!({
                    "connection_id": connection_id,
                    "local_part": "ada",
                    "outgoing_cap": outgoing_cap,
                }),
            )
            .await;
        assert_eq!(status, 201, "{body}");
        let mailbox = self
            .wait_for("the login proof settles", || async {
                let page = self
                    .get(&format!("/api/v1/agents/{}/mailbox", self.daemon.agent_id))
                    .await;
                (page["mailbox"]["state"] != "provisioning").then(|| page["mailbox"].clone())
            })
            .await;
        assert_eq!(mailbox["state"], "active", "{mailbox}");
        mailbox
    }

    /// Poll `look` until it answers, or fail with `what` in the words
    /// of the test. Nothing here sleeps for effect.
    async fn wait_for<T, F, Fut>(&self, what: &str, look: F) -> T
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Option<T>>,
    {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        loop {
            if let Some(found) = look().await {
                return found;
            }
            assert!(tokio::time::Instant::now() < deadline, "{what}");
            tokio::time::sleep(POLL).await;
        }
    }

    /// The Runs of one trigger kind that reached one state.
    async fn wait_for_runs(&self, trigger_kind: &str, state: &str, count: usize) -> Vec<Value> {
        self.wait_for(
            &format!("{count} {trigger_kind} Runs reached {state}"),
            || async {
                let page = self.get("/api/v1/runs").await;
                let runs: Vec<Value> = page["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|run| run["trigger_kind"] == trigger_kind && run["state"] == state)
                    .cloned()
                    .collect();
                (runs.len() >= count).then_some(runs)
            },
        )
        .await
    }

    /// The one Request the Run parked on.
    async fn wait_for_request(&self) -> Value {
        self.wait_for("the Run parks on a Request", || async {
            let page = self.get("/api/v1/requests?state=pending").await;
            page["items"]
                .as_array()
                .and_then(|items| items.first())
                .cloned()
        })
        .await
    }

    async fn decide(&self, request_id: &str, scope: &str) {
        let (status, body) = self
            .post(
                &format!("/api/v1/requests/{request_id}/decision"),
                serde_json::json!({"decision": "approved", "scope": scope}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
    }

    async fn tell_the_agent(&self, pending_id: &str, text: &str) {
        let (status, body) = self
            .post(
                &format!("/api/v1/channels/{}/messages", self.daemon.dm_channel_id),
                serde_json::json!({"pending_id": pending_id, "text": text}),
            )
            .await;
        assert_eq!(status, 201, "{body}");
    }

    /// The system prompt of the turn the Incoming Event drove.
    async fn event_turn_prompt(&self) -> String {
        self.wait_for("the event turn reaches the model", || async {
            self.brain
                .requests()
                .into_iter()
                .find(|request| request.system.contains("Incoming event trigger:"))
                .map(|request| request.system)
        })
        .await
    }

    /// Wait until a tool result carrying `code` reached the model.
    async fn wait_for_tool_error(&self, code: &str) -> String {
        self.wait_for(&format!("a tool result carries {code}"), || async {
            self.brain
                .requests()
                .into_iter()
                .flat_map(|request| request.messages)
                .find(|message| message.text.contains(code))
                .map(|message| message.text)
        })
        .await
    }

    fn script_send(&self, to: &str, subject: &str) {
        self.brain.push(Script::tool_call(
            &[],
            "mail__send",
            serde_json::json!({
                "mailbox": "own",
                "to": [to],
                "subject": subject,
                "body": "the invoice is paid",
            }),
        ));
        self.brain.push(Script::reply(&["That is done."]));
    }
}

/// The Schedules the Agent created. The seed gives the Workspace its
/// own Report Schedule, which is the user's, not the Agent's.
async fn agent_schedules(harness: &Harness) -> Vec<Value> {
    harness.get("/api/v1/schedules").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|schedule| schedule["creator"] == "agent")
        .cloned()
        .collect()
}

/// `YYYY-MM-DDTHH:MM:SS`, the shape one-shot Schedules take.
fn local_time_at(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .expect("a time in range")
        .to_rfc3339_opts(SecondsFormat::Secs, true)
        .trim_end_matches('Z')
        .to_string()
}

/// Mail from an address the user lists as their own, with the Mailbox
/// Provider's aligned DMARC pass, is owner mail, and the Run is told its
/// words stand where the user's own do (ADR-0019).
#[tokio::test]
async fn owner_mail_wakes_the_agent_with_the_standing_of_the_user() {
    let harness = boot().await;
    harness.brain.push(Script::reply(&["I will pay it today."]));
    harness.hold_mailbox(5).await;
    let (status, entry) = harness
        .post(
            "/api/v1/settings/trust-list",
            serde_json::json!({"value": "owner@example.com", "tier": "owner", "label": "Me"}),
        )
        .await;
    assert_eq!(status, 201, "{entry}");

    // The Mailbox Provider's own result shows a DMARC pass for the
    // domain in the `From` header, so the listed tier holds.
    harness.transport.deliver_raw(&raw_message(
        "owner@example.com",
        "Pay the invoice",
        &["aspmx1.migadu.com; dkim=pass header.d=example.com; \
           dmarc=pass (policy=reject) header.from=example.com"],
    ));

    harness.wait_for_runs("event", "completed", 1).await;
    let prompt = harness.event_turn_prompt().await;
    assert!(prompt.contains("trust_tier: owner"), "{prompt}");
    assert!(prompt.contains("Owner mail is from the user."), "{prompt}");
    assert!(
        prompt.contains("standing of a message the user writes to you"),
        "{prompt}"
    );
    // One Wake-up carries one tier, so no other tier line is read.
    assert!(
        !prompt.contains("Unknown mail is from a sender"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("Trusted mail is from a sender"),
        "{prompt}"
    );
}

/// The appointment chain: an unknown mail arrives, the Run notes the fact
/// with its source and asks for a one-shot Schedule, and the Schedule
/// fires a second Run that messages the user.
#[tokio::test]
async fn an_appointment_mail_becomes_a_note_a_schedule_and_a_second_run() {
    let now = Utc::now().timestamp_millis();
    let due = now + 60_000;
    let clock = FixtureClock::at(now);
    let harness = boot_on(Arc::new(clock.clone())).await;
    // The Run the mail wakes: it asks for the reminder and records the
    // source-backed fact before it answers.
    harness.brain.push(Script::tool_call(
        &[],
        "schedule_create",
        serde_json::json!({
            "kind": "one_shot",
            "name": "Clinic appointment",
            "instruction": "Remind the user about the clinic appointment",
            "local_time": local_time_at(due),
            "timezone": "UTC",
        }),
    ));
    harness.brain.push(Script::tool_call(
        &[],
        "memory_write",
        serde_json::json!({
            "path": "shared/MEMORY.md",
            "content": "- Clinic appointment on Tuesday at 10:00 \
                        (from mail by care@clinic.test, subject \"Your appointment\")\n",
            "expected_memory_revision": "missing",
        }),
    ));
    harness
        .brain
        .push(Script::reply(&["I noted it and set a reminder."]));
    // The Run the Schedule fires, and its own Reflection.
    harness
        .brain
        .push(Script::reply(&["Your clinic appointment is on Tuesday."]));
    harness.brain.push(Script::reply(&["nothing to record"]));

    harness.hold_mailbox(5).await;
    harness.transport.deliver(
        "Clinic <care@clinic.test>",
        "Your appointment",
        "Tuesday at 10:00.",
    );

    // The Schedule the Agent asks for waits for the user.
    let request = harness.wait_for_request().await;
    assert_eq!(request["payload"]["tool_name"], "schedule_create");
    let body = request["payload"]["body"].as_str().unwrap_or_default();
    assert!(
        body.contains("Remind the user about the clinic appointment"),
        "{request}"
    );
    // The seed gives the Workspace its Report Schedule, so the
    // question is whether the Agent's Schedule exists, not whether any
    // Schedule does.
    assert!(
        !agent_schedules(&harness).await.iter().any(|schedule| {
            schedule["instruction"]
                .as_str()
                .unwrap_or_default()
                .contains("clinic appointment")
        }),
        "the parked call creates no Schedule"
    );
    harness
        .decide(request["id"].as_str().unwrap(), "once")
        .await;

    harness.wait_for_runs("event", "completed", 1).await;
    let schedules = agent_schedules(&harness).await;
    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0]["creator"], "agent");
    assert_eq!(schedules[0]["state"], "active");

    // The note carries the sender and the subject as its source, so a
    // later reader knows an unknown sender supplied the fact.
    let fact = harness
        .wait_for("the Reflection writes the fact", || async {
            let file = harness
                .get("/api/v1/memory/file?scope=shared&path=MEMORY.md")
                .await;
            let content = file["content"].as_str().unwrap_or_default().to_string();
            content.contains("Clinic appointment").then_some(content)
        })
        .await;
    assert!(fact.contains("care@clinic.test"), "{fact}");
    assert!(fact.contains("Your appointment"), "{fact}");

    // The Schedule fires the second Run when its time comes. The Run
    // speaks at the top level of the same channel the mail woke the
    // Agent in, where the channel view reads it.
    clock.advance_to(due);
    let runs = harness.wait_for_runs("schedule", "completed", 1).await;
    let run_id = runs[0]["id"].as_str().unwrap().to_string();
    let channel_id = harness.daemon.dm_channel_id.clone();
    let timeline = harness
        .get(&format!("/api/v1/channels/{channel_id}/messages"))
        .await;
    let reminder = timeline["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["run_id"] == run_id.as_str()
                && item["text_content"] == "Your clinic appointment is on Tuesday."
        })
        .unwrap_or_else(|| panic!("the reminder reaches the user: {timeline}"))
        .clone();
    assert!(
        reminder["parent_message_id"].is_null(),
        "the reminder is a top-level message: {reminder}"
    );
}

/// The Outgoing Cap answers before the card, so the user is never asked
/// to approve a send that cannot happen.
#[tokio::test]
async fn the_outgoing_cap_refuses_a_send_before_any_card() {
    let harness = boot().await;
    harness.hold_mailbox(1).await;

    // The first send spends the whole allowance of the day.
    harness.script_send("bob@other.test", "Invoice 42");
    harness.tell_the_agent("p-1", "answer Bob").await;
    let request = harness.wait_for_request().await;
    assert_eq!(request["payload"]["tool_name"], "mail__send");
    harness
        .decide(request["id"].as_str().unwrap(), "once")
        .await;
    // The first Run completes before the second message arrives. A
    // message for a thread whose Run still runs folds into that Run, and
    // then no Run asks for the second send.
    harness.wait_for_runs("message", "completed", 1).await;
    assert_eq!(
        harness.transport.sent().len(),
        1,
        "the first message leaves"
    );

    // The second goes to a domain no rule covers, so it would need a
    // card. The cap refuses it first.
    harness.script_send("dave@third.test", "Invoice 43");
    harness.tell_the_agent("p-2", "answer Dave as well").await;

    let refusal = harness.wait_for_tool_error("outgoing_cap_reached").await;
    assert!(refusal.contains("ada@example.com"), "{refusal}");
    assert!(refusal.contains("Try again tomorrow"), "{refusal}");
    assert_eq!(harness.transport.sent().len(), 1, "the cap holds the wire");
    assert!(
        harness.get("/api/v1/requests?state=pending").await["items"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a refused send asks the user nothing"
    );
    let mailbox = harness
        .get(&format!(
            "/api/v1/agents/{}/mailbox",
            harness.daemon.agent_id
        ))
        .await;
    assert_eq!(mailbox["mailbox"]["sends_today"], 1);
}

/// A `dormant` mailbox sends nothing. The Agent that holds it still
/// runs, so the refusal reaches the model as a tool error and not as a
/// missing tool.
#[tokio::test]
async fn a_dormant_mailbox_refuses_a_send() {
    let harness = boot().await;
    let mailbox = harness.hold_mailbox(5).await;
    pagis_storage_sqlite::SqliteAgentMailboxStore::new(harness.daemon.pool().clone())
        .set_state(
            &harness.daemon.workspace_id,
            &AgentMailboxId::from(mailbox["id"].as_str().unwrap().to_string()),
            AgentMailboxState::Dormant,
            None,
        )
        .await
        .expect("the mailbox sleeps");

    harness.script_send("bob@other.test", "Invoice 42");
    harness.tell_the_agent("p-1", "answer Bob").await;

    let refusal = harness.wait_for_tool_error("mailbox_unavailable").await;
    assert!(refusal.contains("ada@example.com"), "{refusal}");
    assert!(
        refusal.contains("you are archived"),
        "the refusal says why: {refusal}"
    );
    assert!(harness.transport.sent().is_empty());
    assert!(
        harness.get("/api/v1/requests?state=pending").await["items"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a mailbox that cannot send asks the user nothing"
    );
}
