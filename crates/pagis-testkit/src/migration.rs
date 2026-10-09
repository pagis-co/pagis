//! The rows that a migration must keep, written before it and read
//! after it through the stores. One set serves both backends.

use std::path::Path;

use pagis_core::{
    CodingSessionEventKind, CodingSessionId, ConnectionId, EventSource, EventSubscriptionId,
    IncomingEventId, SessionApprovalMode, Stores, WakeupId, WakeupState, WorkspaceId,
};
use sqlx::migrate::Migrator;
use tempfile::TempDir;

/// The version of the migration that gives the source of an Event
/// Subscription, a Source Batch and an Incoming Event a Coding Session
/// beside a Connection.
pub const CODING_SESSION_EVENTS: i64 = 9;

/// A rule, a batch and an event of one Connection, and the pending
/// Wake-up that points at the rule and the event, in the schema before
/// [`CODING_SESSION_EVENTS`]. The statements run on both backends.
pub const CONNECTION_EVENT_ROWS: &str = "\
INSERT INTO workspaces (id, name, timezone, created_at) VALUES ('w', 'Home', 'UTC', 1);
INSERT INTO model_aliases (id, workspace_id, alias, candidates, created_at, updated_at)
    VALUES ('model', 'w', 'default', '[]', 1, 1);
INSERT INTO agents (id, workspace_id, name, job, personality, model_alias, status, created_at,
    updated_at) VALUES ('a', 'w', 'Sage', 'assistant', 'plain', 'default', 'active', 1, 1);
INSERT INTO channels (id, workspace_id, kind, title, created_at, updated_at)
    VALUES ('ch', 'w', 'dm', 'Sage', 1, 1);
INSERT INTO connections (id, workspace_id, provider, alias, display_name, status, auth_mode,
    created_at) VALUES ('cn', 'w', 'gmail', 'work', 'Work mail', 'connected', 'byo', 1);
INSERT INTO event_subscriptions (id, workspace_id, agent_id, connection_id, event_kind,
    source_version, name, instruction, channel_id, filter, creator, state, revision,
    approved_revision, watermark_at, created_at, updated_at)
    VALUES ('es', 'w', 'a', 'cn', 'mail.message_received', 'v1', 'Inbox', 'Read it', 'ch', '{}',
    'user', 'active', 1, 1, 1, 1, 1);
INSERT INTO source_batches (id, workspace_id, connection_id, event_kind, collected_at,
    collected_count, stored_count, wakeup_count, outcome)
    VALUES ('sb', 'w', 'cn', 'mail.message_received', 2, 1, 1, 1, 'collected');
INSERT INTO incoming_events (id, workspace_id, connection_id, event_kind, provider_event_id,
    metadata, occurred_at, received_at, batch_id)
    VALUES ('ie', 'w', 'cn', 'mail.message_received', 'm1', '{}', 2, 2, 'sb');
INSERT INTO wakeups (id, workspace_id, source_kind, subscription_id, rule_revision, rule_name,
    agent_id, channel_id, instruction, scheduled_at, state, created_at)
    VALUES ('wk', 'w', 'event_subscription', 'es', 1, 'Inbox', 'a', 'ch', 'Read it', 2,
    'pending', 2);
INSERT INTO wakeup_sources (workspace_id, wakeup_id, source_kind, source_id)
    VALUES ('w', 'wk', 'incoming_event', 'ie');
";

/// Runs created before titles, including a Call that references its Run.
pub const RUN_TITLE_ROWS: &str = "\
INSERT INTO messages (id, workspace_id, channel_id, author_kind, status, blocks, text_content, created_at)
    VALUES ('m', 'w', 'ch', 'user', 'complete', '[]', '  Book the Austin trip  \nIgnore this second line', 3);
INSERT INTO runs (id, workspace_id, agent_id, trigger_kind, trigger_ref, state, created_at)
    VALUES ('message', 'w', 'a', 'message', 'm', 'completed', 3),
           ('attachment', 'w', 'a', 'message', NULL, 'completed', 3),
           ('arrival', 'w', 'a', 'arrival', NULL, 'completed', 3),
           ('review', 'w', 'a', 'review', NULL, 'completed', 3),
           ('rule', 'w', 'a', 'event', 'wk', 'completed', 3),
           ('call', 'w', 'a', 'event', NULL, 'completed', 3);
INSERT INTO calls (id, workspace_id, agent_id, run_id, phone_number_id, direction, remote_e164,
    tier, state, outcome, ended_reason, transcript, created_at)
    VALUES ('call', 'w', 'a', 'call', 'phone', 'inbound', '+14155550189', 'unknown', 'ended',
    'no_answer', 'no_answer', '[]', 3);
";

/// Every backend backfills the same titles without changing Run ids.
pub async fn assert_run_titles(stores: &Stores) {
    for (id, title) in [
        ("message", "Book the Austin trip"),
        ("attachment", "A message with an attachment"),
        ("arrival", "Bring a source into memory"),
        ("review", "Review what was learned"),
        ("rule", "Inbox"),
        ("call", "Call from +14155550189"),
    ] {
        let run = stores
            .runs
            .get(
                &WorkspaceId::from("w".to_string()),
                &pagis_core::RunId::from(id.to_string()),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run.title, title, "{id}");
    }
    let call = stores
        .calls
        .get(
            &WorkspaceId::from("w".to_string()),
            &pagis_core::CallId::from("call".to_string()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.run_id.as_str(), "call");
}

/// The version of the migration that leaves `person` and `agent` as the
/// Session Approval Modes of a Coding Session.
pub const PERSON_AND_AGENT_MODES: i64 = 12;

/// The hash of the model token of the session of [`AUTO_SESSION_ROWS`].
pub const AUTO_SESSION_TOKEN_HASH: &str = "4f1c";

/// A Coding Session in the `auto` mode, with one row of its transcript,
/// its model token hash and a Session Rule, in the schema before
/// [`PERSON_AND_AGENT_MODES`]. The statements run on both backends.
pub const AUTO_SESSION_ROWS: &str = "\
INSERT INTO workspaces (id, name, timezone, created_at) VALUES ('w', 'Home', 'UTC', 1);
INSERT INTO model_aliases (id, workspace_id, alias, candidates, created_at, updated_at)
    VALUES ('model', 'w', 'default', '[]', 1, 1);
INSERT INTO agents (id, workspace_id, name, job, personality, model_alias, status, created_at,
    updated_at) VALUES ('a', 'w', 'Sage', 'assistant', 'plain', 'default', 'active', 1, 1);
INSERT INTO channels (id, workspace_id, kind, title, created_at, updated_at)
    VALUES ('ch', 'w', 'dm', 'Sage', 1, 1);
INSERT INTO runs (id, workspace_id, agent_id, trigger_kind, state, created_at)
    VALUES ('r', 'w', 'a', 'message', 'completed', 1);
INSERT INTO coding_sessions (id, workspace_id, agent_id, harness_id, harness_version, place,
    directory, approval_mode, title, state, channel_id, root_message_id, message_id, run_id,
    created_at, updated_at, model_token_hash)
    VALUES ('cs', 'w', 'a', 'pi', '0.1.0', 'computer', '/home/agent/app', 'auto', 'Fix it',
    'idle', 'ch', 'm1', 'm2', 'r', 1, 1, '4f1c');
INSERT INTO coding_session_events (workspace_id, coding_session_id, seq, at, kind, payload)
    VALUES ('w', 'cs', 1, 1, 'prompt', '{\"text\":\"Fix it.\"}');
INSERT INTO event_subscriptions (id, workspace_id, agent_id, coding_session_id, event_kind,
    source_version, name, instruction, channel_id, filter, creator, state, revision,
    approved_revision, created_at, updated_at)
    VALUES ('es', 'w', 'a', 'cs', 'coding_session.turn_ended', 'v1', 'Turn ended', 'Read it',
    'ch', '{}', 'agent', 'active', 1, 1, 1, 1);
";

/// The version of the migration that gives a Coding Session its Harness
/// Mode and adds the `mode` rows of the transcript.
pub const HARNESS_MODES: i64 = 14;

/// A Coding Session with one row of each transcript kind, in the schema
/// before [`HARNESS_MODES`]. The statements run on both backends.
pub const MODELESS_SESSION_ROWS: &str = "\
INSERT INTO workspaces (id, name, timezone, created_at) VALUES ('w', 'Home', 'UTC', 1);
INSERT INTO model_aliases (id, workspace_id, alias, candidates, created_at, updated_at)
    VALUES ('model', 'w', 'default', '[]', 1, 1);
INSERT INTO agents (id, workspace_id, name, job, personality, model_alias, status, created_at,
    updated_at) VALUES ('a', 'w', 'Sage', 'assistant', 'plain', 'default', 'active', 1, 1);
INSERT INTO channels (id, workspace_id, kind, title, created_at, updated_at)
    VALUES ('ch', 'w', 'dm', 'Sage', 1, 1);
INSERT INTO runs (id, workspace_id, agent_id, trigger_kind, state, title, created_at)
    VALUES ('r', 'w', 'a', 'message', 'completed', 'Fix it', 1);
INSERT INTO coding_sessions (id, workspace_id, agent_id, harness_id, harness_version, place,
    directory, approval_mode, title, state, channel_id, root_message_id, message_id, run_id,
    created_at, updated_at)
    VALUES ('cs', 'w', 'a', 'claude', '0.1.0', 'computer', '/home/agent/app', 'person',
    'Fix it', 'idle', 'ch', 'm1', 'm2', 'r', 1, 1);
INSERT INTO coding_session_events (workspace_id, coding_session_id, seq, at, kind, payload)
    VALUES ('w', 'cs', 1, 1, 'prompt', '{\"text\":\"Fix it.\"}'),
    ('w', 'cs', 2, 2, 'turn_end', '{\"stop_reason\":\"end_turn\"}');
INSERT INTO event_subscriptions (id, workspace_id, agent_id, coding_session_id, event_kind,
    source_version, name, instruction, channel_id, filter, creator, state, revision,
    approved_revision, created_at, updated_at)
    VALUES ('es', 'w', 'a', 'cs', 'coding_session.turn_ended', 'v1', 'Turn ended', 'Read it',
    'ch', '{}', 'agent', 'active', 1, 1, 1, 1);
";

/// The migrations of `directory` before `version`, from a directory of
/// their own, so a test can write rows into the schema that a later
/// migration changes. The caller holds the directory while it runs the
/// migrator. Each migration keeps its checksum, so the full migrator
/// runs on the same database afterwards.
pub async fn migrator_before(directory: &Path, version: i64) -> (Migrator, TempDir) {
    let all = Migrator::new(directory).await.expect("read the migrations");
    let earlier = tempfile::tempdir().expect("a directory for the earlier migrations");
    for migration in all.iter().filter(|migration| migration.version < version) {
        let name = format!(
            "{:04}_{}.sql",
            migration.version,
            migration.description.replace(' ', "_")
        );
        std::fs::write(earlier.path().join(name), migration.sql.as_bytes())
            .expect("copy a migration");
    }
    let before = Migrator::new(earlier.path())
        .await
        .expect("read the earlier migrations");
    (before, earlier)
}

/// The rows of [`CONNECTION_EVENT_ROWS`] read back through the stores,
/// each with its Connection as its source.
pub async fn assert_connection_event_rows(stores: &Stores) {
    let workspace_id = WorkspaceId::from("w".to_string());
    let connection_id = ConnectionId::from("cn".to_string());
    let source = EventSource::connection(connection_id.clone());

    let rule = stores
        .subscriptions
        .get(&workspace_id, &EventSubscriptionId::from("es".to_string()))
        .await
        .expect("read the rule")
        .expect("the rule is kept");
    assert_eq!(rule.source, source);
    assert_eq!(rule.name, "Inbox");
    assert_eq!(
        stores
            .subscriptions
            .list(&workspace_id, None, None, 10)
            .await
            .expect("list the rules")
            .len(),
        1
    );

    let events = stores
        .subscriptions
        .list_events(&workspace_id, &rule.id, None, 10)
        .await
        .expect("read the events of the rule");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, IncomingEventId::from("ie".to_string()));
    assert_eq!(events[0].source, source);
    assert_eq!(events[0].provider_event_id, "m1");

    let (last, _) = stores
        .subscriptions
        .collector_health(&workspace_id, &connection_id, "mail.message_received")
        .await
        .expect("read the batches");
    let batch = last.expect("the batch is kept");
    assert_eq!(batch.source, source);
    assert_eq!(batch.stored_count, 1);

    let wakeup = stores
        .triggers
        .get_wakeup(&workspace_id, &WakeupId::from("wk".to_string()))
        .await
        .expect("read the Wake-up")
        .expect("the Wake-up is kept");
    assert_eq!(wakeup.state, WakeupState::Pending);
    let context = stores
        .triggers
        .event_context(&workspace_id, &wakeup.id)
        .await
        .expect("read the events of the Wake-up")
        .expect("the Wake-up still points at its rule");
    assert_eq!(context.connection_alias.as_deref(), Some("work"));
    assert_eq!(context.events, events);
}

/// The session of [`AUTO_SESSION_ROWS`] read back through the stores: in
/// the `person` mode, with its transcript, its model token and its
/// Session Rule.
pub async fn assert_auto_session_rows(stores: &Stores) {
    let workspace_id = WorkspaceId::from("w".to_string());
    let session_id = CodingSessionId::from("cs".to_string());

    let session = stores
        .coding_sessions
        .get(&workspace_id, &session_id)
        .await
        .expect("read the session")
        .expect("the session is kept");
    assert_eq!(session.approval_mode, SessionApprovalMode::Person);
    assert_eq!(session.title, "Fix it");

    let events = stores
        .coding_sessions
        .list_events(&workspace_id, &session_id, None, 10)
        .await
        .expect("read the transcript");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, CodingSessionEventKind::Prompt);
    assert_eq!(events[0].payload, serde_json::json!({"text": "Fix it."}));

    let owner = stores
        .coding_sessions
        .model_token_owner(AUTO_SESSION_TOKEN_HASH)
        .await
        .expect("read the token")
        .expect("the token hash is kept");
    assert_eq!(owner.session_id, session_id);

    let rule = stores
        .subscriptions
        .get(&workspace_id, &EventSubscriptionId::from("es".to_string()))
        .await
        .expect("read the Session Rule")
        .expect("the Session Rule is kept");
    assert_eq!(rule.source, EventSource::coding_session(session_id));
}

/// The session of [`MODELESS_SESSION_ROWS`] read back through the stores:
/// with no Harness Mode and no offered modes, with its transcript and its
/// Session Rule. A `mode` row then goes after its rows.
pub async fn assert_modeless_session_rows(stores: &Stores) {
    let workspace_id = WorkspaceId::from("w".to_string());
    let session_id = CodingSessionId::from("cs".to_string());

    let session = stores
        .coding_sessions
        .get(&workspace_id, &session_id)
        .await
        .expect("read the session")
        .expect("the session is kept");
    assert_eq!(session.harness_mode, None);
    assert_eq!(session.harness_modes, []);
    assert_eq!(session.title, "Fix it");

    let events = stores
        .coding_sessions
        .list_events(&workspace_id, &session_id, None, 10)
        .await
        .expect("read the transcript");
    let kinds: Vec<_> = events.iter().map(|event| event.kind).collect();
    assert_eq!(
        kinds,
        [
            CodingSessionEventKind::Prompt,
            CodingSessionEventKind::TurnEnd
        ]
    );

    let mode = pagis_core::NewCodingSessionEvent {
        at: 3,
        kind: CodingSessionEventKind::Mode,
        payload: serde_json::json!({"mode": "plan", "name": "Plan", "by": "harness"}),
    };
    let written = stores
        .coding_sessions
        .append_event(&workspace_id, &session_id, mode)
        .await
        .expect("the check takes a mode row");
    assert_eq!(written.last().map(|row| row.seq), Some(3));

    let rule = stores
        .subscriptions
        .get(&workspace_id, &EventSubscriptionId::from("es".to_string()))
        .await
        .expect("read the Session Rule")
        .expect("the Session Rule is kept");
    assert_eq!(rule.source, EventSource::coding_session(session_id));
}
