use std::sync::Arc;
use std::time::Duration;

use chrono::SecondsFormat;
use futures::{SinkExt, StreamExt};
use pagis_core::{MemoryStore, WorkspaceStore};
use pagis_testkit::evaluation::FixtureClock;
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

fn options(brain: &Arc<ScriptedBrain>) -> TestDaemonOptions {
    TestDaemonOptions {
        brain: Arc::clone(brain) as _,
        ..TestDaemonOptions::default()
    }
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

/// The whole-second UTC local time of the instant `at`.
fn local_time_of(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at)
        .unwrap()
        .to_rfc3339_opts(SecondsFormat::Secs, true)
        .trim_end_matches('Z')
        .to_string()
}

/// A one-shot Schedule in the DM channel, due at the instant `at`.
async fn create_one_shot(daemon: &TestDaemon, at: i64) -> String {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/schedules", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "name": "Morning plan",
            "instruction": "Prepare today's plan",
            "channel_id": daemon.dm_channel_id,
            "root_message_id": null,
            "local_time": local_time_of(at),
            "timezone": "UTC"
        }))
        .send()
        .await
        .expect("create schedule");
    if response.status() != 201 {
        panic!("schedule create failed: {}", response.text().await.unwrap());
    }
    response
        .json::<serde_json::Value>()
        .await
        .expect("schedule JSON")["id"]
        .as_str()
        .expect("schedule id")
        .to_string()
}

/// A daemon on `clock` that answers with `brain`.
fn on_clock(brain: &Arc<ScriptedBrain>, clock: &FixtureClock) -> TestDaemonOptions {
    TestDaemonOptions {
        clock: Arc::new(clock.clone()),
        ..options(brain)
    }
}

async fn seed_subject_page(daemon: &TestDaemon, path: &str, content: String) {
    let workspace = pagis_storage_sqlite::SqliteWorkspaceStore::new(daemon.pool().clone())
        .list()
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let agent_id = pagis_core::AgentId::from(daemon.agent_id.clone());
    let access = pagis_core::MemoryAccess::agent(agent_id.clone(), []);
    let memory = pagis_memory::GitMemoryStore::new(
        daemon.booted.home.join("memory"),
        Arc::new(pagis_storage_sqlite::SqliteForgetStore::new(
            daemon.pool().clone(),
        )),
        std::sync::Arc::new(pagis_storage_sqlite::SqliteMemoryPageIndex::new(
            daemon.pool().clone(),
        )),
    );
    let mut changeset = pagis_core::MemoryChangeset::default();
    changeset
        .writes
        .insert(pagis_core::ScopedPath::parse(path).unwrap(), content);
    memory
        .commit(
            &workspace.id,
            &agent_id,
            &access,
            &pagis_core::MemoryAuthor {
                name: "Fixture".into(),
                email: "fixture@pagis.local".into(),
            },
            &changeset,
            None,
            "Seed Subject Page",
            None,
        )
        .await
        .unwrap();
}

async fn read_subject_page(daemon: &TestDaemon, path: &str) -> String {
    let workspace = pagis_storage_sqlite::SqliteWorkspaceStore::new(daemon.pool().clone())
        .list()
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let agent_id = pagis_core::AgentId::from(daemon.agent_id.clone());
    let memory = pagis_memory::GitMemoryStore::new(
        daemon.booted.home.join("memory"),
        Arc::new(pagis_storage_sqlite::SqliteForgetStore::new(
            daemon.pool().clone(),
        )),
        std::sync::Arc::new(pagis_storage_sqlite::SqliteMemoryPageIndex::new(
            daemon.pool().clone(),
        )),
    );
    memory
        .read(
            &workspace.id,
            &pagis_core::MemoryAccess::agent(agent_id, []),
            &pagis_core::ScopedPath::parse(path).unwrap(),
        )
        .await
        .unwrap()
        .content
}

async fn get_subject_page_api(daemon: &TestDaemon, relative_path: &str) -> String {
    reqwest::Client::new()
        .get(format!("{}/api/v1/memory/file", daemon.base_url))
        .header("cookie", daemon.cookie())
        .query(&[
            ("scope", format!("agent:{}", daemon.agent_id)),
            ("path", relative_path.to_string()),
        ])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_fixture_time_schedule_fires_when_the_injected_clock_advances() {
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["The fixture reminder fired."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        clock: Arc::new(clock.clone()),
        ..TestDaemonOptions::default()
    })
    .await;
    create_one_shot(&daemon, now + 60_000).await;

    clock.advance_to(now + 60_000);

    wait_for_schedule_run(&daemon, "completed").await;
}

#[tokio::test]
async fn a_fired_subject_schedule_can_stay_silent_and_records_why() {
    let now = 1_789_041_600_000;
    let due = now + 60_000;
    let path = "private/subjects/gmail/closed.md";
    let clock = FixtureClock::at(now);
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "schedule_create",
        serde_json::json!({
            "name": "Closed matter check",
            "instruction": "Decide whether anything remains to say",
            "channel": "user",
            "kind": "one_shot",
            "local_time": chrono::DateTime::from_timestamp_millis(due).unwrap()
                .to_rfc3339_opts(SecondsFormat::Secs, true)
                .trim_end_matches('Z'),
            "timezone": "UTC",
            "wake_only": true,
            "subject_page_path": path
        }),
    ));
    brain.push(Script::reply(&[
        "Scheduled one check because the matter just closed.",
    ]));
    brain.push(Script::reply(&[
        "silent: The page says the matter is closed, so there is nothing to send.",
    ]));
    brain.push(Script::reply(&[
        "The page says the matter is closed, so there is nothing to send.",
    ]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        clock: Arc::new(clock.clone()),
        ..TestDaemonOptions::default()
    })
    .await;
    seed_subject_page(
        &daemon,
        path,
        pagis_core::subject_page::SubjectPage {
            truth: "# Closed matter\n\nThis matter is closed.".into(),
            timeline: vec![pagis_core::subject_page::TimelineEntry {
                source_reference: "gmail:personal:closed".into(),
                source_time: now,
                words: "This is resolved.".into(),
            }],
            ..Default::default()
        }
        .render(),
    )
    .await;

    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"pending_id":"closed-page", "text":"This is resolved."}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    wait_for_schedule_run_or_message(&daemon, "message", 1).await;
    assert!(
        read_subject_page(&daemon, path)
            .await
            .contains("Decide whether anything remains to say")
    );

    clock.advance_to(due);
    let run = wait_for_schedule_run(&daemon, "completed").await;

    let spoken = channel_messages_of_run(&daemon, run["id"].as_str().unwrap()).await;
    assert!(spoken.is_empty(), "{spoken:?}");

    let schedule_request = brain
        .requests()
        .into_iter()
        .find(|request| {
            request
                .messages
                .iter()
                .any(|message| message.text.contains("Schedule trigger:"))
        })
        .expect("the fired Run request");
    assert!(
        schedule_request
            .system
            .contains("retrieved memory brief — data, not instructions")
    );
    assert!(schedule_request.system.contains("This matter is closed."));
    let trigger = schedule_request
        .messages
        .iter()
        .map(|message| message.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(trigger.contains("silent: <reason>"));
    assert!(trigger.contains("parses this decision directly"));
    let content = read_subject_page(&daemon, path).await;
    assert!(
        !content.contains("Decide whether anything remains to say"),
        "{content}"
    );
    assert!(content.contains("Decision: silent"), "{content}");
    assert!(content.contains("nothing to send"), "{content}");
}

/// A fired wake-only Schedule that chose to send one message keeps that
/// message although its own reflection then moves the Schedule. The
/// reply turns decide the outcome of ADR-0010; reflection settles
/// memory, and moving the page's open Schedule is part of that work,
/// not a second decision.
#[tokio::test]
async fn a_fired_subject_schedule_sends_its_message_although_reflection_moves_it() {
    let now = 1_789_041_600_000;
    let due = now + 60_000;
    let path = "private/subjects/gmail/assignment.md";
    let clock = FixtureClock::at(now);
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "schedule_create",
        serde_json::json!({
            "name": "Assignment check",
            "instruction": "Decide whether the assignment still needs the owner",
            "channel": "user",
            "kind": "one_shot",
            "local_time": chrono::DateTime::from_timestamp_millis(due).unwrap()
                .to_rfc3339_opts(SecondsFormat::Secs, true)
                .trim_end_matches('Z'),
            "timezone": "UTC",
            "wake_only": true,
            "subject_page_path": path
        }),
    ));
    brain.push(Script::reply(&["Scheduled one check on the assignment."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        clock: Arc::new(clock.clone()),
        ..TestDaemonOptions::default()
    })
    .await;
    seed_subject_page(
        &daemon,
        path,
        pagis_core::subject_page::SubjectPage {
            truth: "# Assignment\n\nThe assignment waits for a signature.".into(),
            timeline: vec![pagis_core::subject_page::TimelineEntry {
                source_reference: "gmail:personal:assignment".into(),
                source_time: now,
                words: "Please sign the assignment.".into(),
            }],
            ..Default::default()
        }
        .render(),
    )
    .await;

    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "pending_id": "assignment-page",
            "text": "Please sign the assignment."
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    wait_for_schedule_run_or_message(&daemon, "message", 1).await;

    // The fired Run says one thing to the user, and its reflection then
    // moves the page's open Schedule, as the reflection prompt asks.
    let schedules = get(&daemon, "/api/v1/schedules").await;
    let schedule_id = schedules["items"][0]["id"].as_str().unwrap().to_string();
    let next = due + 30 * 24 * 60 * 60 * 1_000;
    brain.push(Script::reply(&[
        "The assignment needs your signature today.",
    ]));
    brain.push(Script::tool_call(
        &[],
        "schedule_update",
        serde_json::json!({
            "schedule_id": schedule_id,
            "action": "edit",
            "name": "Assignment check",
            "instruction": "Decide whether the assignment still needs the owner",
            "channel": "user",
            "kind": "one_shot",
            "local_time": chrono::DateTime::from_timestamp_millis(next).unwrap()
                .to_rfc3339_opts(SecondsFormat::Secs, true)
                .trim_end_matches('Z'),
            "timezone": "UTC",
            "wake_only": true,
            "subject_page_path": path
        }),
    ));
    brain.push(Script::reply(&["Noted the signature deadline."]));

    clock.advance_to(due);
    let run = wait_for_schedule_run(&daemon, "completed").await;

    let spoken = channel_messages_of_run(&daemon, run["id"].as_str().unwrap()).await;
    assert!(
        spoken
            .iter()
            .any(|text| text.contains("needs your signature today")),
        "{spoken:?}"
    );
    let content = read_subject_page(&daemon, path).await;
    assert!(content.contains("Decision: sent one message"), "{content}");
}

/// A `schedule_update` the Subject Page Cooldown refuses moved
/// nothing, so the fired Run still owes the message it then wrote.
#[tokio::test]
async fn a_refused_reschedule_does_not_silence_the_fired_run() {
    let now = 1_789_041_600_000;
    let due = now + 60_000;
    let path = "private/subjects/gmail/renewal.md";
    let clock = FixtureClock::at(now);
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "schedule_create",
        serde_json::json!({
            "name": "Renewal check",
            "instruction": "Decide whether the renewal still needs the owner",
            "channel": "user",
            "kind": "one_shot",
            "local_time": chrono::DateTime::from_timestamp_millis(due).unwrap()
                .to_rfc3339_opts(SecondsFormat::Secs, true)
                .trim_end_matches('Z'),
            "timezone": "UTC",
            "wake_only": true,
            "subject_page_path": path
        }),
    ));
    brain.push(Script::reply(&["Scheduled one check on the renewal."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        clock: Arc::new(clock.clone()),
        ..TestDaemonOptions::default()
    })
    .await;
    seed_subject_page(
        &daemon,
        path,
        pagis_core::subject_page::SubjectPage {
            truth: "# Renewal\n\nThe renewal needs a decision.".into(),
            timeline: vec![pagis_core::subject_page::TimelineEntry {
                source_reference: "gmail:personal:renewal".into(),
                source_time: now,
                words: "Your plan renews soon.".into(),
            }],
            ..Default::default()
        }
        .render(),
    )
    .await;

    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "pending_id": "renewal-page",
            "text": "Your plan renews soon."
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    wait_for_schedule_run_or_message(&daemon, "message", 1).await;

    // The fired Run asks for a check one day out. The 14-day Cooldown
    // refuses it, so the Run says its piece instead.
    let schedules = get(&daemon, "/api/v1/schedules").await;
    let schedule_id = schedules["items"][0]["id"].as_str().unwrap().to_string();
    let too_soon = due + 24 * 60 * 60 * 1_000;
    brain.push(Script::tool_call(
        &[],
        "schedule_update",
        serde_json::json!({
            "schedule_id": schedule_id,
            "action": "edit",
            "name": "Renewal check",
            "instruction": "Decide whether the renewal still needs the owner",
            "channel": "user",
            "kind": "one_shot",
            "local_time": chrono::DateTime::from_timestamp_millis(too_soon).unwrap()
                .to_rfc3339_opts(SecondsFormat::Secs, true)
                .trim_end_matches('Z'),
            "timezone": "UTC",
            "wake_only": true,
            "subject_page_path": path
        }),
    ));
    brain.push(Script::reply(&[
        "The renewal needs your decision this week.",
    ]));
    brain.push(Script::reply(&["Noted the renewal decision."]));

    clock.advance_to(due);
    let run = wait_for_schedule_run(&daemon, "completed").await;

    let spoken = channel_messages_of_run(&daemon, run["id"].as_str().unwrap()).await;
    assert!(
        spoken
            .iter()
            .any(|text| text.contains("needs your decision this week")),
        "{spoken:?}"
    );
    let content = read_subject_page(&daemon, path).await;
    assert!(content.contains("Decision: sent one message"), "{content}");
}

/// A fired Schedule reads the conversation of its channel, not an empty
/// Thread off its own progress row, so the Report for Home sees the DM
/// it writes into.
#[tokio::test]
async fn a_fired_schedule_reads_the_conversation_of_its_channel() {
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Noted."]));
    brain.push(Script::reply(&["Nothing to record."]));
    let daemon = TestDaemon::start_with(on_clock(&brain, &clock)).await;

    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "pending_id": "before-the-wake",
            "text": "The dentist moved my appointment to Thursday."
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    wait_for_schedule_run_or_message(&daemon, "message", 1).await;

    brain.push(Script::reply(&["Your plan is ready."]));
    brain.push(Script::reply(&["Nothing to record."]));
    create_one_shot(&daemon, now + 60_000).await;
    clock.advance_to(now + 60_000);
    wait_for_schedule_run(&daemon, "completed").await;

    let fired = brain
        .requests()
        .into_iter()
        .find(|request| {
            request
                .messages
                .iter()
                .any(|message| message.text.contains("Schedule trigger:"))
        })
        .expect("the fired Run request");
    let read = fired
        .messages
        .iter()
        .map(|message| message.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        read.contains("The dentist moved my appointment to Thursday."),
        "{read}"
    );
}

/// The messages one Run left where the channel view reads them: the
/// top level of the channel, never a Thread under a progress row.
async fn channel_messages_of_run(daemon: &TestDaemon, run_id: &str) -> Vec<String> {
    let page = get(
        daemon,
        &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
    )
    .await;
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["run_id"] == run_id && item["author_kind"] == "agent")
        .filter_map(|item| item["text_content"].as_str().map(str::to_string))
        .collect()
}

async fn wait_for_schedule_run(daemon: &TestDaemon, state: &str) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let page = get(daemon, "/api/v1/runs").await;
        if let Some(run) = page["items"].as_array().and_then(|items| {
            items.iter().find(|run| {
                run["trigger_kind"] == "schedule" && run["state"].as_str() == Some(state)
            })
        }) {
            return run.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "schedule Run reached {state}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_rest_created_one_shot_fires_once_and_links_public_history() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Your plan is ready."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("WS connect");
    socket
        .send(Message::text(
            serde_json::json!({ "type": "auth" }).to_string(),
        ))
        .await
        .expect("WS auth");
    let ready = socket.next().await.unwrap().unwrap().into_text().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ready).unwrap()["type"],
        "ready"
    );
    // The socket waits on the clock of the daemon for the end of its
    // Session, so this Schedule comes due on the wall clock.
    let schedule_id = create_one_shot(&daemon, pagis_core::now_ms() + 3_000).await;
    let run = wait_for_schedule_run(&daemon, "completed").await;

    let occurrences = get(
        &daemon,
        &format!("/api/v1/schedules/{schedule_id}/occurrences"),
    )
    .await;
    let wakeups = get(&daemon, &format!("/api/v1/schedules/{schedule_id}/wakeups")).await;
    assert_eq!(occurrences["items"].as_array().unwrap().len(), 1);
    assert_eq!(wakeups["items"].as_array().unwrap().len(), 1);
    assert_eq!(wakeups["items"][0]["state"], "started");
    assert_eq!(wakeups["items"][0]["run_id"], run["id"]);
    assert_eq!(run["trigger_ref"], wakeups["items"][0]["id"]);

    let timeline = get(
        &daemon,
        &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
    )
    .await;
    let items = timeline["items"].as_array().unwrap();
    // A fired Schedule speaks at the top level of its channel, where
    // the channel view reads, and its progress row sits beside the
    // message rather than over it.
    assert_eq!(items.len(), 3);
    assert!(run["root_message_id"].is_null());
    assert_eq!(items[0]["author_kind"], "agent");
    assert_eq!(items[0]["text_content"], "Your plan is ready.");
    assert_eq!(items[0]["run_id"], run["id"]);
    assert!(items[0]["parent_message_id"].is_null());
    assert_eq!(items[1]["blocks"][0]["type"], "progress");
    assert_eq!(items[1]["author_kind"], "system");
    assert_eq!(items[1]["run_id"], run["id"]);

    let request = brain.requests().into_iter().next().expect("agent request");
    let trigger = request
        .messages
        .iter()
        .map(|message| message.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(trigger.contains("Prepare today's plan"));
    assert!(trigger.contains("Scheduled for"));
    assert!(trigger.contains("silent: <reason>"));

    let mut event_types = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = tokio::time::timeout_at(deadline, socket.next())
            .await
            .expect("terminal Run event")
            .expect("socket open")
            .expect("WS frame");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        let event_type = frame["type"].as_str().unwrap_or_default().to_string();
        event_types.insert(event_type.clone());
        if event_type == "run.state_changed" && frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    for event_type in [
        "schedule.created",
        "schedule.occurrence_recorded",
        "wakeup.created",
        "wakeup.started",
        "run.created",
        "message.completed",
        "run.state_changed",
    ] {
        assert!(event_types.contains(event_type), "missing {event_type}");
    }
}

#[tokio::test]
async fn restart_before_due_still_fires_the_one_shot() {
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let first_brain = Arc::new(ScriptedBrain::default());
    let mut first_options = on_clock(&first_brain, &clock);
    first_options.agents.max_concurrent_runs = 0;
    let daemon = TestDaemon::start_with(first_options).await;
    let schedule_id = create_one_shot(&daemon, now + 60_000).await;

    let second_brain = Arc::new(ScriptedBrain::default());
    second_brain.push(Script::reply(&["After restart"]));
    let daemon = daemon.restart(on_clock(&second_brain, &clock)).await;
    clock.advance_to(now + 60_000);
    wait_for_schedule_run(&daemon, "completed").await;

    let occurrences = get(
        &daemon,
        &format!("/api/v1/schedules/{schedule_id}/occurrences"),
    )
    .await;
    assert_eq!(occurrences["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_pending_wakeup_survives_restart_and_then_runs() {
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let first_brain = Arc::new(ScriptedBrain::default());
    let mut first_options = on_clock(&first_brain, &clock);
    first_options.agents.max_concurrent_runs = 0;
    let daemon = TestDaemon::start_with(first_options).await;
    let schedule_id = create_one_shot(&daemon, now + 60_000).await;
    clock.advance_to(now + 60_000);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    loop {
        let wakeups = get(&daemon, &format!("/api/v1/schedules/{schedule_id}/wakeups")).await;
        if wakeups["items"][0]["state"] == "pending" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "pending Wake-up created"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let second_brain = Arc::new(ScriptedBrain::default());
    second_brain.push(Script::reply(&["Resumed"]));
    let daemon = daemon.restart(on_clock(&second_brain, &clock)).await;
    let run = wait_for_schedule_run(&daemon, "completed").await;
    let wakeups = get(&daemon, &format!("/api/v1/schedules/{schedule_id}/wakeups")).await;
    assert_eq!(wakeups["items"].as_array().unwrap().len(), 1);
    assert_eq!(wakeups["items"][0]["run_id"], run["id"]);
}

#[tokio::test]
async fn restart_after_run_start_fails_it_without_replay() {
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let first_brain = Arc::new(ScriptedBrain::default());
    first_brain.push(Script::hang(&["Working"]));
    let daemon = TestDaemon::start_with(on_clock(&first_brain, &clock)).await;
    let schedule_id = create_one_shot(&daemon, now + 60_000).await;
    clock.advance_to(now + 60_000);
    let started = wait_for_schedule_run(&daemon, "running").await;

    let second_brain = Arc::new(ScriptedBrain::default());
    let daemon = daemon.restart(on_clock(&second_brain, &clock)).await;
    let failed = wait_for_schedule_run(&daemon, "failed").await;
    assert_eq!(failed["id"], started["id"]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let runs = get(&daemon, "/api/v1/runs").await;
    assert_eq!(
        runs["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|run| run["trigger_kind"] == "schedule")
            .count(),
        1
    );
    let wakeups = get(&daemon, &format!("/api/v1/schedules/{schedule_id}/wakeups")).await;
    assert_eq!(wakeups["items"].as_array().unwrap().len(), 1);
    assert_eq!(wakeups["items"][0]["state"], "started");
}

#[tokio::test]
async fn a_one_shot_in_an_existing_thread_reads_its_current_context() {
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let brain = Arc::new(ScriptedBrain::default());
    brain.push_for("Pixie", Script::reply(&["Acknowledged"]));
    brain.push_for("Pixie", Script::reply(&["Used the context"]));
    let daemon = TestDaemon::start_with(on_clock(&brain, &clock)).await;
    let root = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "pending_id": "schedule-context-root",
            "text": "Context to remember",
            "parent_message_id": null,
            "artifact_ids": []
        }))
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    wait_for_schedule_run_or_message(&daemon, "message", 1).await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/schedules", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "name": "Context follow-up",
            "instruction": "Use the current Thread context",
            "channel_id": daemon.dm_channel_id,
            "root_message_id": root["id"],
            "local_time": local_time_of(now + 60_000),
            "timezone": "UTC"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    clock.advance_to(now + 60_000);
    wait_for_schedule_run(&daemon, "completed").await;

    let schedule_request = brain
        .requests()
        .into_iter()
        .find(|request| {
            request
                .messages
                .iter()
                .any(|message| message.text.contains("Schedule trigger:"))
        })
        .expect("Schedule turn request");
    assert!(
        schedule_request
            .messages
            .iter()
            .any(|message| message.text.contains("Context to remember"))
    );
}

async fn wait_for_schedule_run_or_message(daemon: &TestDaemon, kind: &str, count: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let page = get(daemon, "/api/v1/runs").await;
        let complete = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|run| run["trigger_kind"] == kind && run["state"] == "completed")
            .count();
        if complete >= count {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{kind} Run completed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn an_agent_created_schedule_starts_only_after_approval() {
    let brain = Arc::new(ScriptedBrain::default());
    let anchor_at = pagis_core::now_ms() + 3_600_000;
    brain.push(Script::tool_call(
        &[],
        "schedule_create",
        serde_json::json!({
            "name": "Hourly review",
            "instruction": "Review the latest work",
            "channel": "user",
            "kind": "interval",
            "interval_minutes": 60,
            "anchor_at": anchor_at
        }),
    ));
    brain.push(Script::reply(&["The Schedule is active."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;

    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "pending_id": "schedule-agent-create",
            "text": "Set up an hourly review"
        }))
        .send()
        .await
        .expect("send message")
        .error_for_status()
        .expect("message accepted");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let request = loop {
        let page = get(&daemon, "/api/v1/requests?state=pending").await;
        if let Some(request) = page["items"].as_array().and_then(|items| items.first()) {
            break request.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Schedule approval Request was created"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(request["kind"], "tool_action");
    let body = request["payload"]["body"].as_str().expect("Request body");
    assert!(body.contains("Destination: user"));
    assert!(body.contains("Instruction: Review the latest work"));
    assert!(body.contains("Cadence: every 60 minute(s)"));
    assert!(body.contains("Maximum Runs per day: 24"));
    // The seed gives the Workspace its Report Schedule, so the
    // question is whether this Agent's Schedule exists, not whether any
    // Schedule does.
    assert!(
        !get(&daemon, "/api/v1/schedules").await["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|schedule| schedule["instruction"] == "Review the latest work"),
        "approval creates the Schedule, not the parked tool call"
    );

    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/requests/{}/decision",
            daemon.base_url,
            request["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"decision": "approved"}))
        .send()
        .await
        .expect("approve Request")
        .error_for_status()
        .expect("Request approved");

    wait_for_schedule_run_or_message(&daemon, "message", 1).await;
    let schedules = get(&daemon, "/api/v1/schedules").await;
    let schedule = &schedules["items"][0];
    assert_eq!(schedule["state"], "active");
    assert_eq!(schedule["creator"], "agent");
    assert_eq!(schedule["revision"], 1);
    assert_eq!(schedule["approved_revision"], 1);
    assert_eq!(schedule["creating_run_id"], request["run_id"]);
    assert_eq!(schedule["next_due_at"], anchor_at);
}

#[tokio::test]
async fn a_foreground_run_creates_a_wake_only_schedule_without_an_approval_card() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "schedule_create",
        serde_json::json!({
            "name": "School follow-up",
            "instruction": "Check whether the school date changed",
            "channel": "user",
            "kind": "one_shot",
            "local_time": local_time_of(pagis_core::now_ms() + 3_600_000),
            "timezone": "UTC",
            "wake_only": true,
            "subject_page_path": "private/subjects/gmail/school.md"
        }),
    ));
    brain.push(Script::reply(&[
        "Scheduled a check because the date can change.",
    ]));
    let daemon = TestDaemon::start_with(options(&brain)).await;
    seed_subject_page(
        &daemon,
        "private/subjects/gmail/school.md",
        pagis_core::subject_page::SubjectPage::default().render(),
    )
    .await;

    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "pending_id": "wake-only-reflection",
            "text": "School starts next month"
        }))
        .send()
        .await
        .expect("send message")
        .error_for_status()
        .expect("message accepted");

    wait_for_schedule_run_or_message(&daemon, "message", 1).await;
    let requests = get(&daemon, "/api/v1/requests?state=pending").await;
    assert!(requests["items"].as_array().unwrap().is_empty());
    let schedules = get(&daemon, "/api/v1/schedules").await;
    assert_eq!(
        schedules["items"][0]["subject_page_path"],
        "private/subjects/gmail/school.md"
    );
    assert!(schedules["items"][0]["approved_revision"].is_null());
    let feed = get(&daemon, "/api/v1/memory/feed").await;
    let item = feed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "schedule")
        .expect("the learning feed lists the Schedule");
    assert_eq!(item["action"]["kind"], "cancel_schedule");
    assert_eq!(item["action"]["schedule_id"], schedules["items"][0]["id"]);
    let schedule_id = schedules["items"][0]["id"].as_str().unwrap();
    assert!(
        get_subject_page_api(&daemon, "subjects/gmail/school.md")
            .await
            .contains(schedule_id)
    );
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/schedules/{schedule_id}/archive",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(
        !get_subject_page_api(&daemon, "subjects/gmail/school.md")
            .await
            .contains(schedule_id)
    );
}

#[tokio::test]
async fn recurring_schedule_rest_reads_and_controls_round_trip() {
    let brain = Arc::new(ScriptedBrain::default());
    let mut daemon_options = options(&brain);
    daemon_options.agents.max_concurrent_runs = 0;
    let daemon = TestDaemon::start_with(daemon_options).await;
    let anchor_at = pagis_core::now_ms() + 3_600_000;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/api/v1/schedules", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "name": "Review",
            "instruction": "Review the work",
            "channel_id": daemon.dm_channel_id,
            "kind": "interval",
            "interval_minutes": 60,
            "anchor_at": anchor_at,
            "timezone": "UTC"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["kind"], "interval");
    assert_eq!(created["next_due_at"], anchor_at);
    assert_eq!(created["creator"], "user");

    let paused = client
        .post(format!("{}/api/v1/schedules/{id}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"action": "pause"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(paused["state"], "paused");
    assert!(paused["next_due_at"].is_null());

    let resumed = client
        .post(format!("{}/api/v1/schedules/{id}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"action": "resume"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(resumed["state"], "active");
    assert_eq!(resumed["next_due_at"], anchor_at);

    let skipped = client
        .post(format!("{}/api/v1/schedules/{id}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "action": "skip_next",
            "expected_due_at": anchor_at
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(skipped["next_due_at"], anchor_at + 3_600_000);

    let replacement_anchor = anchor_at + 7_200_000;
    let edited = client
        .post(format!("{}/api/v1/schedules/{id}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "action": "edit",
            "expected_revision": 1,
            "name": "Review twice daily",
            "kind": "interval",
            "interval_minutes": 720,
            "anchor_at": replacement_anchor
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(edited["revision"], 2);
    assert_eq!(edited["approved_revision"], 2);
    assert_eq!(edited["next_due_at"], replacement_anchor);

    let revisions = get(&daemon, &format!("/api/v1/schedules/{id}/revisions")).await;
    assert_eq!(revisions["items"].as_array().unwrap().len(), 2);
    assert_eq!(revisions["items"][0]["name"], "Review");
    assert_eq!(revisions["items"][1]["name"], "Review twice daily");
}

/// A skip-next that names a due instant the daemon has moved past lost
/// the race with due processing. The answer is 409 with the sentence
/// the Automations surface shows, never a bare internal error.
#[tokio::test]
async fn a_lost_skip_next_race_answers_conflict_in_words() {
    let brain = Arc::new(ScriptedBrain::default());
    let mut daemon_options = options(&brain);
    daemon_options.agents.max_concurrent_runs = 0;
    let daemon = TestDaemon::start_with(daemon_options).await;
    let anchor_at = pagis_core::now_ms() + 3_600_000;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/api/v1/schedules", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "name": "Review",
            "instruction": "Review the work",
            "channel_id": daemon.dm_channel_id,
            "kind": "interval",
            "interval_minutes": 60,
            "anchor_at": anchor_at,
            "timezone": "UTC"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let response = client
        .post(format!("{}/api/v1/schedules/{id}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "action": "skip_next",
            "expected_due_at": anchor_at - 1
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(body["error"]["code"], "conflict");
    assert_eq!(
        body["error"]["message"],
        "the Schedule became due before skip-next completed"
    );
}

/// The daemon's background loops stop on the cancellation token.
/// The scheduler is the one that shows it: a Schedule that
/// becomes due after the cancel does not fire.
#[tokio::test]
async fn the_scheduler_stops_after_the_daemon_is_cancelled() {
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Your plan is ready."]));
    let daemon = TestDaemon::start_with(on_clock(&brain, &clock)).await;

    daemon.cancel();
    create_one_shot(&daemon, now + 60_000).await;
    clock.advance_to(now + 60_000);

    // A live scheduler wakes on the advance of its clock and starts the
    // Run at once, as the fixture-time test shows. A stopped one starts
    // nothing in the same time.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let runs = get(&daemon, "/api/v1/runs").await;
    let items = runs["items"].as_array().expect("runs page").clone();
    assert!(
        items.iter().all(|run| run["trigger_kind"] != "schedule"),
        "the stopped scheduler made a Run: {items:?}"
    );
}
