use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream;
use pagis_agent::{
    Brain, BrainError, ToolInvocation, TurnDelta, TurnEnd, TurnRequest, TurnStream, Usage,
};
use pagis_core::{
    AgentId, AgentStore, ChannelId, Connection, ConnectionId, ConnectionStore, ForgetStore,
    ForgetTarget, Grant, GrantId, GrantStore, MemoryAccess, MemoryAuthor, MemoryChangeset,
    MemoryStore, MessageStore, PendingEvidence, PendingEvidenceStore, PendingUrgency, RunState,
    RunStore, ScopedPath, TriggerKind,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteConnectionStore, SqliteForgetStore, SqliteGrantStore,
    SqliteMemoryPageIndex, SqliteMessageStore, SqlitePendingEvidenceStore, SqliteRunStore,
};
use pagis_testkit::evaluation::FixtureClock;
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};

#[derive(Default)]
struct PausedReviewBrain {
    turns: AtomicUsize,
    entered_second_turn: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[derive(Default)]
struct BlockedScheduleBrain {
    requests: Mutex<Vec<TurnRequest>>,
    schedule_entered: tokio::sync::Notify,
    release_schedule: tokio::sync::Notify,
    schedule_completed: AtomicBool,
}

impl BlockedScheduleBrain {
    fn requests(&self) -> Vec<TurnRequest> {
        self.requests.lock().unwrap().clone()
    }
}

fn finished(text: &str) -> TurnStream {
    stream::iter([
        Ok(TurnDelta::Text(text.to_string())),
        Ok(TurnDelta::Finish(TurnEnd {
            stop_reason: "stop".into(),
            provider: Some("scripted".into()),
            model: Some("test".into()),
            usage: Some(Usage {
                input_tokens: 1,
                output_tokens: 1,
                ..Default::default()
            }),
            estimated_cost_usd: None,
        })),
    ])
    .boxed()
}

#[async_trait]
impl Brain for PausedReviewBrain {
    async fn turn(&self, _request: TurnRequest) -> Result<TurnStream, BrainError> {
        if self.turns.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(stream::iter([
                Ok(TurnDelta::ToolCall(ToolInvocation {
                    id: "call_1".into(),
                    name: "memory_write".into(),
                    arguments: serde_json::json!({
                        "path": "private/subjects/finch.md",
                        "content": "Project Finch starts Monday.\n",
                        "expected_memory_revision": "missing"
                    })
                    .to_string(),
                })),
                Ok(TurnDelta::Finish(TurnEnd {
                    stop_reason: "tool_calls".into(),
                    provider: Some("scripted".into()),
                    model: Some("test".into()),
                    usage: None,
                    estimated_cost_usd: None,
                })),
            ])
            .boxed());
        }
        self.entered_second_turn.notify_one();
        self.release.notified().await;
        Ok(finished("Reviewed the evidence."))
    }
}

#[async_trait]
impl Brain for BlockedScheduleBrain {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        let is_blocked_schedule = request.system.contains("Hold this Schedule open")
            || request
                .messages
                .iter()
                .any(|message| message.text.contains("Hold this Schedule open"));
        self.requests.lock().unwrap().push(request);
        if is_blocked_schedule {
            self.schedule_entered.notify_one();
            self.release_schedule.notified().await;
            self.schedule_completed.store(true, Ordering::SeqCst);
        }
        Ok(finished("nothing to record"))
    }
}

async fn send(daemon: &TestDaemon, pending_id: &str, text: &str) -> serde_json::Value {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "pending_id": pending_id,
            "text": text,
            "parent_message_id": null,
            "artifact_ids": []
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn idle_time_without_pending_evidence_starts_no_model_work() {
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        clock: Arc::new(clock.clone()),
        ..TestDaemonOptions::default()
    })
    .await;

    clock.advance_to(now + 31 * 60_000);
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(brain.requests().is_empty());
    assert!(
        SqliteRunStore::new(daemon.pool().clone())
            .list_unfinished()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn due_reviews_and_a_waiting_schedule_are_all_admitted() {
    let now = 1_789_041_600_000;
    let due = now + 5 * 60_000;
    let clock = FixtureClock::at(now);
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Source recorded."]));
    brain.push(Script::reply(&["nothing to record"]));
    brain.push(Script::reply(&["The reminder fired."]));
    brain.push(Script::reply(&["nothing to record"]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        clock: Arc::new(clock.clone()),
        ..TestDaemonOptions::default()
    })
    .await;
    let source = send(&daemon, "fair-source", "Keep this source for both reviews.").await;
    wait_until(|| !brain.requests().is_empty()).await;
    let message_id = pagis_core::MessageId::from(source["id"].as_str().unwrap().to_string());
    let agent = SqliteAgentStore::new(daemon.pool().clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let store = SqlitePendingEvidenceStore::new(daemon.pool().clone());
    for subject in ["one", "two"] {
        store
            .record(PendingEvidence {
                workspace_id: agent.workspace_id.clone(),
                agent_id: agent.id.clone(),
                channel_id: ChannelId::from(daemon.dm_channel_id.clone()),
                root_message_id: None,
                subject: subject.into(),
                after_exclusive: None,
                through_inclusive: message_id.clone(),
                source_message_ids: vec![message_id.clone()],
                exposures: Vec::new(),
                reason: format!("Review {subject}."),
                urgency: PendingUrgency::Normal,
                created_at: now,
            })
            .await
            .unwrap()
            .unwrap();
    }
    let local_time = chrono::DateTime::from_timestamp_millis(due)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        .trim_end_matches('Z')
        .to_string();
    reqwest::Client::new()
        .post(format!("{}/api/v1/schedules", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "name": "Fair reminder",
            "instruction": "Run between the two reviews",
            "channel_id": daemon.dm_channel_id,
            "root_message_id": null,
            "local_time": local_time,
            "timezone": "UTC"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    clock.advance_to(due);
    wait_until(|| brain.requests().len() >= 4).await;
    let requests = brain.requests();
    let background = &requests[1..4];
    let review_count = background
        .iter()
        .filter(|request| request.system.contains("pending evidence reason"))
        .count();
    let schedule_ran = background.iter().any(|request| {
        request.system.contains("Run between the two reviews")
            || request
                .messages
                .iter()
                .any(|message| message.text.contains("Run between the two reviews"))
    });
    assert_eq!(review_count, 2, "both pending reviews must run");
    assert!(schedule_ran, "the waiting Schedule must run");
}

#[tokio::test]
async fn a_blocked_schedule_does_not_hold_urgent_or_overdue_reviews() {
    let now = 1_789_041_600_000;
    let due = now + 5 * 60_000;
    let clock = FixtureClock::at(now);
    let brain = Arc::new(BlockedScheduleBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        clock: Arc::new(clock.clone()),
        ..TestDaemonOptions::default()
    })
    .await;
    let source = send(
        &daemon,
        "blocked-schedule-source",
        "Keep this review source.",
    )
    .await;
    wait_until(|| !brain.requests().is_empty()).await;
    let message_id = pagis_core::MessageId::from(source["id"].as_str().unwrap().to_string());
    let agent = SqliteAgentStore::new(daemon.pool().clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let local_time = chrono::DateTime::from_timestamp_millis(due)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        .trim_end_matches('Z')
        .to_string();
    reqwest::Client::new()
        .post(format!("{}/api/v1/schedules", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "agent_id": daemon.agent_id,
            "name": "Blocked reminder",
            "instruction": "Hold this Schedule open",
            "channel_id": daemon.dm_channel_id,
            "root_message_id": null,
            "local_time": local_time,
            "timezone": "UTC"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    clock.advance_to(due);
    tokio::time::timeout(Duration::from_secs(5), brain.schedule_entered.notified())
        .await
        .expect("Schedule reached the model");
    let store = SqlitePendingEvidenceStore::new(daemon.pool().clone());
    for (subject, urgency, created_at) in [
        ("urgent", PendingUrgency::Urgent, due),
        ("overdue", PendingUrgency::Normal, due - 31 * 60_000),
    ] {
        store
            .record(PendingEvidence {
                workspace_id: agent.workspace_id.clone(),
                agent_id: agent.id.clone(),
                channel_id: ChannelId::from(daemon.dm_channel_id.clone()),
                root_message_id: None,
                subject: subject.into(),
                after_exclusive: None,
                through_inclusive: message_id.clone(),
                source_message_ids: vec![message_id.clone()],
                exposures: Vec::new(),
                reason: format!("Review {subject} evidence."),
                urgency,
                created_at,
            })
            .await
            .unwrap()
            .unwrap();
    }
    clock.advance_to(due + 1);

    let reviews_started = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let review_count = brain
                .requests()
                .iter()
                .filter(|request| request.system.contains("pending evidence reason"))
                .count();
            if review_count == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let request_kinds = brain
        .requests()
        .iter()
        .map(|request| {
            if request.system.contains("pending evidence reason") {
                "review"
            } else if request.system.contains("Hold this Schedule open")
                || request
                    .messages
                    .iter()
                    .any(|message| message.text.contains("Hold this Schedule open"))
            {
                "schedule"
            } else {
                "reply"
            }
        })
        .collect::<Vec<_>>();
    assert!(
        reviews_started.is_ok(),
        "urgent and overdue Reviews did not both start: {request_kinds:?}"
    );
    assert!(
        !brain.schedule_completed.load(Ordering::SeqCst),
        "both Reviews must start while the Schedule remains blocked"
    );
    brain.release_schedule.notify_one();
    wait_until(|| brain.schedule_completed.load(Ordering::SeqCst)).await;
}

#[tokio::test]
async fn restart_settles_a_committed_review_without_replaying_the_model() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        pagis_broker::MEMORY_REVIEW,
        serde_json::json!({
            "subject": "trip",
            "reason": "The reservation evidence needs reconciliation.",
            "urgency": "normal"
        }),
    ));
    brain.push(Script::reply(&["I will review it later."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    send(&daemon, "recovery-source", "The reservation date changed.").await;
    wait_until(|| brain.requests().len() >= 2).await;

    let agent_id = AgentId::from(daemon.agent_id.clone());
    let pending = SqlitePendingEvidenceStore::new(daemon.pool().clone());
    let claim = pending
        .claim(&daemon.workspace_id, &agent_id, 1, i64::MAX / 2)
        .await
        .unwrap()
        .remove(0);
    let agent = SqliteAgentStore::new(daemon.pool().clone())
        .get(&daemon.workspace_id, &agent_id)
        .await
        .unwrap()
        .unwrap();
    let memory = pagis_memory::GitMemoryStore::new(
        daemon.booted.home.join("memory"),
        Arc::new(SqliteForgetStore::new(daemon.pool().clone())),
        Arc::new(SqliteMemoryPageIndex::new(daemon.pool().clone())),
    );
    let access = MemoryAccess::agent(agent_id.clone(), claim.evidence.exposures.clone());
    let expected = memory
        .load_indexes(&agent.workspace_id, &access)
        .await
        .unwrap()
        .revision;
    let mut changes = MemoryChangeset::default();
    changes.writes.insert(
        ScopedPath::parse("private/subjects/trip.md").unwrap(),
        "---\ntitle: Trip\nkind: Subject\n---\n# Trip\n".into(),
    );
    let after = claim
        .evidence
        .after_exclusive
        .as_ref()
        .map_or("none", |id| id.as_str());
    let message = format!(
        "Reviewed trip evidence\n\nPagis-Memory-Phase: reflection\nPending-Review-Id: {}\nPending-Review-Revision: {}\nSource-After-Exclusive: {}\nSource-Through-Inclusive: {}\nAdvances-Review-Cursor: pending",
        claim.evidence.id, claim.lease_revision, after, claim.evidence.through_inclusive
    );
    let sha = memory
        .commit(
            &agent.workspace_id,
            &agent_id,
            &access,
            &MemoryAuthor {
                name: agent.name,
                email: format!("{}@agents.pagis.local", agent_id.as_str()),
            },
            &changes,
            expected.as_deref(),
            &message,
            Some(&claim.run.id),
        )
        .await
        .unwrap();

    let replay = Arc::new(ScriptedBrain::default());
    let daemon = daemon
        .restart(TestDaemonOptions {
            brain: Arc::clone(&replay) as _,
            ..TestDaemonOptions::default()
        })
        .await;
    let settled = SqlitePendingEvidenceStore::new(daemon.pool().clone())
        .for_run(&daemon.workspace_id, &claim.run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, pagis_core::PendingEvidenceState::Completed);
    assert_eq!(settled.memory_revision.as_deref(), Some(sha.as_str()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(replay.requests().is_empty(), "the review must not replay");
}

#[tokio::test]
async fn restart_retries_a_leased_review_that_has_no_commit() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        pagis_broker::MEMORY_REVIEW,
        serde_json::json!({
            "subject": "trip",
            "reason": "The reservation evidence needs reconciliation.",
            "urgency": "normal"
        }),
    ));
    brain.push(Script::reply(&["I will review it later."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    send(&daemon, "retry-source", "The reservation date changed.").await;
    wait_until(|| brain.requests().len() >= 2).await;
    let pending = SqlitePendingEvidenceStore::new(daemon.pool().clone());
    let claim = pending
        .claim(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
            1,
            i64::MAX / 2,
        )
        .await
        .unwrap()
        .remove(0);

    let replay = Arc::new(ScriptedBrain::default());
    let daemon = daemon
        .restart(TestDaemonOptions {
            brain: Arc::clone(&replay) as _,
            ..TestDaemonOptions::default()
        })
        .await;
    let retained = SqlitePendingEvidenceStore::new(daemon.pool().clone())
        .for_run(&daemon.workspace_id, &claim.run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.state, pagis_core::PendingEvidenceState::Pending);
    assert_eq!(retained.attempt_count, 1);
    assert_eq!(retained.error.as_deref(), Some("daemon restarted"));
    assert!(replay.requests().is_empty());
}

#[tokio::test]
async fn failed_review_retry_is_authenticated_and_requeues_the_same_evidence() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Source recorded."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let source = send(&daemon, "retry-api-source", "Keep this source.").await;
    wait_until(|| brain.requests().len() == 1).await;
    daemon.cancel();
    let agent = SqliteAgentStore::new(daemon.pool().clone())
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let pending = SqlitePendingEvidenceStore::new(daemon.pool().clone());
    let record = pending
        .record(PendingEvidence {
            workspace_id: agent.workspace_id,
            agent_id: agent.id.clone(),
            channel_id: ChannelId::from(daemon.dm_channel_id.clone()),
            root_message_id: None,
            subject: "retry".into(),
            after_exclusive: None,
            through_inclusive: pagis_core::MessageId::from(
                source["id"].as_str().unwrap().to_string(),
            ),
            source_message_ids: vec![pagis_core::MessageId::from(
                source["id"].as_str().unwrap().to_string(),
            )],
            exposures: Vec::new(),
            reason: "Manual review needed.".into(),
            urgency: PendingUrgency::Urgent,
            created_at: 1,
        })
        .await
        .unwrap()
        .unwrap();
    let mut now = 1;
    let mut last_run = None;
    for attempt in 1..=3 {
        let claim = pending
            .claim(&daemon.workspace_id, &agent.id, 1, now)
            .await
            .unwrap()
            .remove(0);
        last_run = Some(claim.run.id.clone());
        pending
            .fail(
                &daemon.workspace_id,
                &claim.run.id,
                claim.lease_revision,
                "failed",
                now,
            )
            .await
            .unwrap();
        now += if attempt == 1 { 60_000 } else { 5 * 60_000 };
    }
    let run_id = last_run.unwrap();
    sqlx::query("UPDATE runs SET state='failed',error='failed',ended_at=? WHERE id=?")
        .bind(now)
        .bind(run_id.as_str())
        .execute(daemon.pool())
        .await
        .unwrap();
    let url = format!("{}/api/v1/runs/{run_id}/retry", daemon.base_url);
    assert_eq!(
        reqwest::Client::new()
            .post(&url)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        reqwest::Client::new()
            .post(&url)
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    let retried: (String, i64) =
        sqlx::query_as("SELECT state,attempt_count FROM pending_evidence WHERE id=?")
            .bind(record.id.as_str())
            .fetch_one(daemon.pool())
            .await
            .unwrap();
    assert_eq!(retried, ("pending".into(), 0));
}

#[tokio::test]
async fn forget_during_an_in_flight_review_prevents_its_staged_write_from_committing() {
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let brain = Arc::new(PausedReviewBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        clock: Arc::new(clock.clone()),
        ..TestDaemonOptions::default()
    })
    .await;
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
        alias: "work".into(),
        display_name: "Work".into(),
        status: Connection::CONNECTED.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: serde_json::json!({}),
        created_at: now,
    };
    SqliteConnectionStore::new(daemon.pool().clone())
        .create(&connection)
        .await
        .unwrap();
    let grant = Grant {
        id: GrantId::generate(),
        workspace_id: agent.workspace_id.clone(),
        agent_id: agent.id.clone(),
        resource_kind: Grant::CONNECTION_KIND.into(),
        resource_id: Some(connection.id.to_string()),
        scope: Grant::connection_scope(&["gmail_read".into()]),
        revision: 1,
        created_at: now,
        revoked_at: None,
    };
    SqliteGrantStore::new(daemon.pool().clone())
        .create(&grant)
        .await
        .unwrap();
    let source = pagis_testkit::fixture::agent_message(
        &agent.workspace_id,
        &ChannelId::from(daemon.dm_channel_id.clone()),
        &agent.id,
        "Project Finch starts Monday.",
    );
    let exposure = pagis_core::MemoryExposure {
        grant_id: grant.id,
        revision: 1,
    };
    SqliteMessageStore::new(daemon.pool().clone())
        .insert_stamped(&source, std::slice::from_ref(&exposure))
        .await
        .unwrap();
    let pending = SqlitePendingEvidenceStore::new(daemon.pool().clone());
    let record = pending
        .record(PendingEvidence {
            workspace_id: agent.workspace_id.clone(),
            agent_id: agent.id.clone(),
            channel_id: source.channel_id.clone(),
            root_message_id: None,
            subject: "finch".into(),
            after_exclusive: None,
            through_inclusive: source.id.clone(),
            source_message_ids: vec![source.id],
            exposures: vec![exposure],
            reason: "Reconcile the source date.".into(),
            urgency: PendingUrgency::Urgent,
            created_at: now + 1,
        })
        .await
        .unwrap()
        .unwrap();
    clock.advance_to(now + 1);
    tokio::time::timeout(Duration::from_secs(5), brain.entered_second_turn.notified())
        .await
        .expect("review paused after staging its write");

    let forget = SqliteForgetStore::new(daemon.pool().clone());
    let target = ForgetTarget::Account {
        connection_id: connection.id,
    };
    let preview = forget.preview(&agent.workspace_id, &target).await.unwrap();
    forget
        .begin(
            &agent.workspace_id,
            &target,
            &preview.revision,
            now + 2,
            &pagis_core::TenantKeys::new(Arc::new(pagis_core::MemorySecretStore::default())),
        )
        .await
        .unwrap();
    brain.release.notify_one();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let state: String = sqlx::query_scalar("SELECT state FROM runs WHERE trigger_ref=?")
            .bind(record.id.as_str())
            .fetch_one(daemon.pool())
            .await
            .unwrap();
        if state == "failed" {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "review failed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let invalidated: (String, String, i64, i64) = sqlx::query_as(
        "SELECT state,reason,(SELECT COUNT(*) FROM pending_evidence_messages WHERE pending_id=?),(SELECT COUNT(*) FROM pending_evidence_exposures WHERE pending_id=?) FROM pending_evidence WHERE id=?",
    )
    .bind(record.id.as_str())
    .bind(record.id.as_str())
    .bind(record.id.as_str())
    .fetch_one(daemon.pool())
    .await
    .unwrap();
    assert_eq!(
        invalidated,
        ("invalidated".into(), "Forgotten source.".into(), 0, 0)
    );
    let repo = git2::Repository::open(
        daemon
            .booted
            .home
            .join("memory")
            .join(agent.workspace_id.as_str()),
    );
    assert!(
        repo.is_err(),
        "forgotten review must not create a memory commit"
    );
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition before timeout");
}

#[tokio::test]
async fn an_urgent_review_starts_while_its_foreground_conversation_is_busy() {
    let now = 1_789_041_600_000;
    let clock = FixtureClock::at(now);
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        pagis_broker::MEMORY_REVIEW,
        serde_json::json!({
            "subject": "trip",
            "reason": "The reservation dates need a later comparison.",
            "urgency": "normal"
        }),
    ));
    brain.push(Script::reply(&["I will review it later."]));
    brain.push(Script::hang(&["I am still working."]));
    brain.push(Script::tool_call(
        &[],
        "schedule_create",
        serde_json::json!({
            "name": "Reservation deadline",
            "instruction": "Check the reservation deadline",
            "channel": "user",
            "kind": "one_shot",
            "local_time": chrono::DateTime::from_timestamp_millis(now + 3_600_000)
                .unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                .trim_end_matches('Z'),
            "timezone": "UTC",
            "wake_only": true,
            "subject_page_path": "private/subjects/trip.md"
        }),
    ));
    brain.push(Script::reply(&["Recorded the deadline wake-up."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        clock: Arc::new(clock.clone()),
        ..TestDaemonOptions::default()
    })
    .await;

    send(
        &daemon,
        "review-source",
        "Compare the two reservation dates.",
    )
    .await;
    wait_until(|| brain.requests().len() >= 2).await;
    send(&daemon, "busy-turn", "Work on this while the review waits.").await;
    wait_until(|| brain.requests().len() >= 3).await;

    sqlx::query(
        "UPDATE pending_evidence SET urgency='urgent',eligible_at=?,maximum_due_at=? WHERE state='pending'",
    )
    .bind(now + 1)
    .bind(now + 60_001)
    .execute(daemon.pool())
    .await
    .unwrap();
    clock.advance_to(now + 1);
    wait_until(|| brain.requests().len() >= 5).await;

    let runs = SqliteRunStore::new(daemon.pool().clone());
    let unfinished = runs.list_unfinished().await.unwrap();
    assert!(
        unfinished.iter().any(|run| {
            run.trigger_kind == TriggerKind::Message && run.state == RunState::Running
        })
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let state = sqlx::query_scalar::<_, String>(
            "SELECT state FROM runs WHERE trigger_kind='review' ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_optional(daemon.pool())
        .await
        .unwrap();
        if state.as_deref() == Some("completed") {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "review completed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let schedule: (String, String) = sqlx::query_as(
        "SELECT s.creator,p.path FROM schedules s JOIN schedule_subject_pages p ON p.schedule_id=s.id WHERE s.name='Reservation deadline'",
    )
    .fetch_one(daemon.pool())
    .await
    .unwrap();
    assert_eq!(
        schedule,
        ("agent".into(), "private/subjects/trip.md".into())
    );
    let review_request = &brain.requests()[3];
    assert!(
        review_request
            .tools
            .iter()
            .any(|tool| tool.name == pagis_broker::SCHEDULE_CREATE)
    );
    assert!(
        !review_request
            .tools
            .iter()
            .any(|tool| tool.name == pagis_broker::MEMORY_REVIEW)
    );
}
