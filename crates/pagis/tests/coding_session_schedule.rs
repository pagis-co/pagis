//! A Schedule starts a Coding Session that runs to its report with no
//! Person present (ADR-0033).
//!
//! The Person does the setup first, through the routes of the Access tab
//! and of Schedules: the host Grant with the widest mode `agent`, the
//! allowance of Unattended Modes, the allow rule of Claude Code sessions
//! in one directory, and a one-shot Schedule. Then the Person does
//! nothing. The Schedule's Run starts Claude Code in `bypassPermissions`
//! with no card, the harness acts without asking, and the end of its turn
//! wakes the Agent in the session's Thread. That Run reads the session and
//! closes it. The end of the session wakes the Agent at the top level of
//! the Schedule's Channel, where the Schedule asked, and that Run posts
//! the report there.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_agent::{Brain, BrainError, TurnRequest, TurnStream};
use pagis_broker::fake::FakeClientApp;
use pagis_coding::fake::{FakeHarness, Script, Turn, acp};
use pagis_core::{
    AgentId, AuthorKind, ChannelId, CodingSession, CodingSessionEventKind, CodingSessionState,
    EventSource, HostId, MessageId, RequestState, Run, RunOrigin, RunState, TriggerKind,
    WakeupRule, harness,
};
use pagis_testkit::evaluation::FixtureClock;
use pagis_testkit::{
    HostAnswer, HostClient, ScriptedBrain, SessionClient, TestDaemon, TestDaemonOptions,
};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);
const NOW: i64 = 1_789_041_600_000;
const DUE: i64 = NOW + 60_000;
const DIRECTORY: &str = "/work/app";
const HARNESS_MESSAGE: &str = "The nightly tests pass.";
const REPORT: &str = "Claude Code ran the nightly tests on Air, and they pass.";
const CLOSED: &str = "The tests pass, so I closed the session.";

/// The brain of the test. The Schedule's Run takes the scripts of
/// `schedule`. The Wake-up of the end of a turn waits until the test
/// opens the gate, and then takes the scripts of `wakeup`: the test
/// knows the session id only after the Schedule's Run. The Wake-up of
/// the end of the session takes the scripts of `report`. Each other
/// Wake-up of a Coding Session gets no script.
struct ScheduleThenWakeup {
    schedule: Arc<ScriptedBrain>,
    wakeup: Arc<ScriptedBrain>,
    report: Arc<ScriptedBrain>,
    gate: tokio::sync::watch::Receiver<bool>,
}

#[async_trait]
impl Brain for ScheduleThenWakeup {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        if request
            .system
            .contains("Event kind: coding_session.turn_ended")
        {
            let mut gate = self.gate.clone();
            let _ = gate.wait_for(|open| *open).await;
            return self.wakeup.turn(request).await;
        }
        if request.system.contains("Event kind: coding_session.ended") {
            return self.report.turn(request).await;
        }
        if request.system.contains("Event kind: coding_session.") {
            return Err(BrainError::new(
                "no script for this Wake-up of a Coding Session",
            ));
        }
        self.schedule.turn(request).await
    }
}

async fn call(daemon: &TestDaemon, method: reqwest::Method, path: &str, body: Value) -> Value {
    let response = reqwest::Client::new()
        .request(method.clone(), format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .expect("the daemon answers");
    let status = response.status();
    let text = response.text().await.expect("a body");
    assert!(status.is_success(), "{method} {path}: {status} {text}");
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// The whole-second UTC local time of the instant `at`.
fn local_time_of(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

/// Waits until `read` answers `Some`.
async fn wait_for<T, F, Fut>(what: &str, read: F) -> T
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(value) = read().await {
            return value;
        }
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn runs(daemon: &TestDaemon) -> Vec<Run> {
    daemon
        .stores()
        .runs
        .list(&daemon.workspace_id, None, None, &[], None, 100)
        .await
        .expect("read the Runs")
}

/// The one Run of `trigger_kind` in the Thread `root`, or at the top
/// level for `None`, once it is in `state`.
async fn run_of(
    daemon: &TestDaemon,
    trigger_kind: TriggerKind,
    root: Option<&MessageId>,
    state: RunState,
) -> Run {
    wait_for(&format!("no {trigger_kind:?} Run is {state:?}"), || async {
        let runs: Vec<Run> = runs(daemon)
            .await
            .into_iter()
            .filter(|run| run.trigger_kind == trigger_kind && run.root_message_id.as_ref() == root)
            .collect();
        assert!(runs.len() <= 1, "{runs:?}");
        runs.into_iter().find(|run| run.state == state)
    })
    .await
}

/// Asserts that nothing waits for the Person and nothing went to them: no
/// Request in any state, no approval card, and no item that entered the
/// Needs-You Queue, which is what a Notification sends.
async fn assert_nobody_was_asked(daemon: &TestDaemon) {
    for state in [
        RequestState::Pending,
        RequestState::Approved,
        RequestState::Denied,
        RequestState::Expired,
        RequestState::Superseded,
    ] {
        let requests = daemon
            .stores()
            .requests
            .list_by_state(&daemon.workspace_id, state, None)
            .await
            .expect("read the Requests");
        assert!(requests.is_empty(), "{state:?}: {requests:?}");
    }
    let events = daemon
        .stores()
        .events
        .list_by_types(
            &daemon.workspace_id,
            &["request.created", "needs_you.added"],
            None,
            100,
        )
        .await
        .expect("read the event log");
    assert!(events.is_empty(), "{events:?}");
}

/// The text inside each untrusted envelope of `session` that a request
/// of `brain` holds, in its system prompt or in its messages.
fn envelopes(brain: &ScriptedBrain, session: &CodingSession) -> Vec<String> {
    let begin = format!("[BEGIN UNTRUSTED source=coding_session:{}", session.id);
    let end = format!("[END UNTRUSTED source=coding_session:{}", session.id);
    let mut found = Vec::new();
    for request in brain.requests() {
        let texts = std::iter::once(request.system.clone())
            .chain(request.messages.iter().map(|m| m.text.clone()));
        for text in texts {
            let mut rest = text.as_str();
            while let Some(start) = rest.find(&begin) {
                let after = &rest[start..];
                let stop = after.find(&end).expect("the envelope ends");
                found.push(after[..stop].to_string());
                rest = &after[stop..];
            }
        }
    }
    found
}

#[tokio::test]
async fn a_schedule_starts_a_coding_session_that_runs_to_its_report_with_no_person_present() {
    let clock = FixtureClock::at(NOW);
    let schedule_brain = Arc::new(ScriptedBrain::default());
    let wakeup_brain = Arc::new(ScriptedBrain::default());
    let report_brain = Arc::new(ScriptedBrain::default());
    let (gate, opened) = tokio::sync::watch::channel(false);
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::new(ScheduleThenWakeup {
            schedule: Arc::clone(&schedule_brain),
            wakeup: Arc::clone(&wakeup_brain),
            report: Arc::clone(&report_brain),
            gate: opened,
        }),
        clock: Arc::new(clock.clone()),
        ..TestDaemonOptions::default()
    })
    .await;
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let channel_id = ChannelId::from(daemon.dm_channel_id.clone());

    // The fake Host and its session socket. The harness offers the modes
    // of Claude Code, `default` first, and in its turn it runs a command
    // with no permission and writes one message.
    let modes: Vec<(&str, &str)> = harness::entry("claude")
        .unwrap()
        .modes
        .iter()
        .map(|mode| (mode.id, mode.name))
        .collect();
    let script = Script::default().modes("default", &modes).turn(Turn::new(
        vec![
            acp::SessionUpdate::ToolCall(
                acp::ToolCall::new("call-1", "Run the nightly tests")
                    .kind(acp::ToolKind::Execute)
                    .status(acp::ToolCallStatus::Completed),
            ),
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new(HARNESS_MESSAGE),
            ))),
        ],
        acp::StopReason::EndTurn,
    ));
    let harnesses: Arc<Mutex<Vec<FakeHarness>>> = Arc::default();
    let client_app = FakeClientApp::running(DIRECTORY, {
        let harnesses = Arc::clone(&harnesses);
        move |stream| {
            let (read, write) = futures::io::AsyncReadExt::split(stream);
            harnesses
                .lock()
                .unwrap()
                .push(FakeHarness::serve(script.clone(), write, read));
        }
    });
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let host_id = HostId::from(host.host_id().to_string());
    let _sessions = SessionClient::connect(&daemon, daemon.cookie(), host.host_id(), client_app)
        .await
        .expect("the session socket opens");
    wait_for("the session socket is not open", || async {
        daemon.host_sessions.is_open(&host_id).then_some(())
    })
    .await;

    // What the Person does first, through the routes that the Person uses.
    let hosts = format!("/api/v1/agents/{}/hosts/{host_id}", daemon.agent_id);
    let grant = call(
        &daemon,
        reqwest::Method::PUT,
        &format!("{hosts}/session-approval-mode"),
        json!({"mode": "agent"}),
    )
    .await;
    call(
        &daemon,
        reqwest::Method::PUT,
        &format!("{hosts}/unattended-modes"),
        json!({"allowed": true}),
    )
    .await;
    call(
        &daemon,
        reqwest::Method::PUT,
        &format!("/api/v1/grants/{}/rules", grant["id"].as_str().unwrap()),
        json!({"allow": [], "sessions": [{"harness": "claude", "directory": DIRECTORY}]}),
    )
    .await;
    schedule_brain.push(pagis_testkit::Script::tool_call(
        &[],
        "coding_session_start",
        json!({
            "harness": "claude",
            "directory": DIRECTORY,
            "mode": "agent",
            "harness_mode": "bypassPermissions",
            "title": "Run the nightly tests",
            "prompt": "Run the tests and tell me what fails.",
        }),
    ));
    schedule_brain.push(pagis_testkit::Script::reply(&[
        "Claude Code runs the tests.",
    ]));
    call(
        &daemon,
        reqwest::Method::POST,
        "/api/v1/schedules",
        json!({
            "agent_id": daemon.agent_id,
            "name": "Nightly tests",
            "instruction": "Start Claude Code to run the nightly tests of the app.",
            "channel_id": daemon.dm_channel_id,
            "root_message_id": null,
            "local_time": local_time_of(DUE),
            "timezone": "UTC"
        }),
    )
    .await;

    clock.advance_to(DUE);

    let scheduled = run_of(&daemon, TriggerKind::Schedule, None, RunState::Completed).await;
    assert_eq!(scheduled.root_message_id, None, "{scheduled:?}");
    let session = wait_for("the Schedule's Run started no session", || async {
        daemon
            .stores()
            .coding_sessions
            .list(&daemon.workspace_id, Some(&agent_id), None, None, 10)
            .await
            .expect("read the Coding Sessions")
            .into_iter()
            .next()
    })
    .await;
    assert_eq!(session.run_id, scheduled.id);
    wakeup_brain.push(pagis_testkit::Script::tool_call(
        &[],
        "coding_session_read",
        json!({"session": session.id.as_str()}),
    ));
    wakeup_brain.push(pagis_testkit::Script::tool_call(
        &[],
        "coding_session_close",
        json!({"session": session.id.as_str()}),
    ));
    wakeup_brain.push(pagis_testkit::Script::reply(&[CLOSED]));
    report_brain.push(pagis_testkit::Script::tool_call(
        &[],
        "coding_session_read",
        json!({"session": session.id.as_str()}),
    ));
    report_brain.push(pagis_testkit::Script::reply(&[REPORT]));
    gate.send_replace(true);

    // The session's block is a top-level message of the Schedule's
    // Channel, and the root of the session's Thread.
    assert_eq!(session.channel_id, channel_id);
    assert_eq!(scheduled.channel_id.as_ref(), Some(&channel_id));
    assert_eq!(session.message_id, session.root_message_id);
    let block = daemon
        .stores()
        .messages
        .get(&daemon.workspace_id, &session.message_id)
        .await
        .expect("read the block")
        .expect("the block of the session");
    assert_eq!(block.channel_id, channel_id);
    assert_eq!(block.parent_message_id, None);

    // The Session Rule wakes the Agent in the session's Thread.
    let woken = run_of(
        &daemon,
        TriggerKind::Event,
        Some(&session.root_message_id),
        RunState::Completed,
    )
    .await;
    let thread = RunOrigin {
        agent_id: agent_id.clone(),
        channel_id: channel_id.clone(),
        root_message_id: Some(session.root_message_id.clone()),
    };
    assert_eq!(woken.origin.as_ref(), Some(&thread), "{woken:?}");
    assert_eq!(
        woken.root_message_id.as_ref(),
        Some(&session.root_message_id)
    );
    let rule = daemon
        .stores()
        .subscriptions
        .list_for_source(
            &daemon.workspace_id,
            &EventSource::coding_session(session.id.clone()),
            // The close of the session archives the rule.
            &["archived"],
        )
        .await
        .expect("read the Session Rule")
        .into_iter()
        .find(|rule| rule.event_kind == "coding_session.turn_ended")
        .expect("the Session Rule of the end of a turn");
    let wakeups = daemon
        .stores()
        .subscriptions
        .list_wakeups(&daemon.workspace_id, &rule.id, None, 10)
        .await
        .expect("read the Wake-ups");
    assert_eq!(wakeups.len(), 1, "{wakeups:?}");
    assert_eq!(wakeups[0].run_id.as_ref(), Some(&woken.id));
    assert_eq!(woken.trigger_ref.as_deref(), Some(wakeups[0].id.as_str()));
    assert!(matches!(
        wakeups[0].rule,
        WakeupRule::EventSubscription { .. }
    ));

    // The woken Run read the message of the harness inside the envelope
    // of the session.
    let read = envelopes(&wakeup_brain, &session);
    assert!(
        read.iter()
            .any(|envelope| envelope.contains(HARNESS_MESSAGE)),
        "{read:#?}"
    );

    // The supervision stays in the session's Thread.
    let replies = daemon
        .stores()
        .messages
        .list_thread(&daemon.workspace_id, &session.root_message_id)
        .await
        .expect("read the Thread");
    let closed = replies
        .iter()
        .find(|message| {
            message.run_id.as_ref() == Some(&woken.id) && message.author_kind == AuthorKind::Agent
        })
        .unwrap_or_else(|| panic!("no note in the Thread: {replies:#?}"));
    assert_eq!(closed.text_content, CLOSED);

    // The end of the session wakes the Agent at the top level of the
    // Schedule's Channel, and the report is an Agent message there.
    let reporting = run_of(&daemon, TriggerKind::Event, None, RunState::Completed).await;
    assert_eq!(
        reporting.origin,
        Some(RunOrigin {
            agent_id: agent_id.clone(),
            channel_id: channel_id.clone(),
            root_message_id: None,
        })
    );
    let report = daemon
        .stores()
        .messages
        .list_top_level(&daemon.workspace_id, &channel_id, None, 50)
        .await
        .expect("read the top level")
        .into_iter()
        .map(|entry| entry.message)
        .find(|message| message.run_id.as_ref() == Some(&reporting.id))
        .expect("the report at the top level");
    assert_eq!(report.author_kind, AuthorKind::Agent);
    assert_eq!(report.text_content, REPORT);
    assert_eq!(report.parent_message_id, None);
    assert!(
        envelopes(&report_brain, &session)
            .iter()
            .any(|envelope| envelope.contains(HARNESS_MESSAGE)),
        "the report reads the last message of the harness"
    );

    // The session acted in `bypassPermissions` and asked nobody.
    let record = daemon
        .stores()
        .coding_sessions
        .get(&daemon.workspace_id, &session.id)
        .await
        .expect("read the session")
        .expect("the session");
    assert_eq!(record.state, CodingSessionState::Closed);
    assert_eq!(record.harness_mode.as_deref(), Some("bypassPermissions"));
    let rows = daemon
        .stores()
        .coding_sessions
        .list_events(&daemon.workspace_id, &session.id, None, 1_000)
        .await
        .expect("read the transcript");
    let mode = rows
        .iter()
        .find(|row| row.kind == CodingSessionEventKind::Mode)
        .unwrap_or_else(|| panic!("no mode row: {rows:#?}"));
    assert_eq!(mode.payload["mode"], "bypassPermissions");
    let tool_call = rows
        .iter()
        .find(|row| row.kind == CodingSessionEventKind::ToolCall)
        .unwrap_or_else(|| panic!("no tool_call row: {rows:#?}"));
    assert_eq!(tool_call.payload["kind"], "execute", "{tool_call:?}");
    assert!(
        !rows
            .iter()
            .any(|row| row.kind == CodingSessionEventKind::Permission),
        "{rows:#?}"
    );
    let harness = harnesses.lock().unwrap()[0].clone();
    let methods: Vec<_> = harness
        .received()
        .into_iter()
        .map(|received| received.method)
        .filter(|method| method == "session/set_mode" || method == "session/prompt")
        .collect();
    assert_eq!(methods, ["session/set_mode", "session/prompt"]);
    assert_eq!(
        harness.params("session/set_mode")[0]["modeId"],
        "bypassPermissions"
    );
    assert!(harness.answers().is_empty(), "the harness asked nobody");

    assert_nobody_was_asked(&daemon).await;
}
