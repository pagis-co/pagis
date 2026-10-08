//! The source of an Event Subscription and an Incoming Event: a
//! Connection or a Coding Session (ADR-0006, ADR-0033), on both
//! backends.
//!
//! Name every new body in `store_suite_event_sources!` below; the guard
//! test of the parent module fails while one is missing.

use pagis_core::{
    CodingSession, CodingSessionId, CodingSessionPlace, CreatorKind, EventMatcher, EventSource,
    EventSubscription, EventSubscriptionId, EventSubscriptionState, IngestBatch, IngestOutcome,
    NormalizedEvent, Workspace,
};

use super::Backend;
use crate::fixture;

const KIND: &str = "coding_session.turn_ended";

/// Every event matches every rule. The bodies here test the source, not
/// a filter.
struct Every;

impl EventMatcher for Every {
    fn matches(&self, _filter: &serde_json::Value, _metadata: &serde_json::Value) -> bool {
        true
    }
}

/// The rows a session event points at: one Agent, one Channel and two
/// Coding Sessions of that Agent, in the Agent's Computer.
struct World {
    workspace: Workspace,
    agent_id: pagis_core::AgentId,
    channel_id: pagis_core::ChannelId,
    sessions: [CodingSessionId; 2],
}

async fn world(backend: &Backend) -> World {
    let stores = backend.stores();
    let workspace = backend.seeded_workspace().await;
    let agent = fixture::agent(&workspace.id);
    stores.agents.create(&agent).await.expect("write the Agent");
    let channel = fixture::channel(&workspace.id);
    stores
        .channels
        .create(&channel)
        .await
        .expect("write the Channel");
    let run = fixture::queued_run(&workspace.id, &agent.id, &channel.id);
    stores.runs.create(&run).await.expect("write the Run");
    let mut sessions = Vec::new();
    for _ in 0..2 {
        let session = CodingSession {
            id: CodingSessionId::generate(),
            place: CodingSessionPlace::Computer,
            host_id: None,
            ..fixture::coding_session(
                &workspace.id,
                &agent.id,
                &run.id,
                &channel.id,
                &pagis_core::HostId::generate(),
            )
        };
        stores
            .coding_sessions
            .insert(&session)
            .await
            .expect("write the Coding Session");
        sessions.push(session.id);
    }
    World {
        agent_id: agent.id,
        channel_id: channel.id,
        sessions: sessions.try_into().expect("two sessions"),
        workspace,
    }
}

fn rule(world: &World, session: &CodingSessionId) -> EventSubscription {
    EventSubscription {
        id: EventSubscriptionId::generate(),
        workspace_id: world.workspace.id.clone(),
        agent_id: world.agent_id.clone(),
        source: EventSource::coding_session(session.clone()),
        event_kind: KIND.to_string(),
        source_version: "pagis-1".to_string(),
        name: "Session".to_string(),
        instruction: "Supervise the session.".to_string(),
        channel_id: world.channel_id.clone(),
        root_message_id: None,
        filter: serde_json::json!({}),
        creator: CreatorKind::Agent,
        state: EventSubscriptionState::Active,
        revision: 1,
        approved_revision: Some(1),
        watermark_at: Some(10),
        blocked_reason: None,
        created_at: 10,
        updated_at: 10,
        archived_at: None,
    }
}

/// One session event, as one batch with one event and no cursor.
async fn ingest(
    backend: &Backend,
    world: &World,
    session: &CodingSessionId,
    provider_event_id: &str,
    eligible: &[EventSubscription],
    at: i64,
) -> IngestOutcome {
    backend
        .stores()
        .triggers
        .ingest(
            IngestBatch {
                workspace_id: world.workspace.id.clone(),
                source: EventSource::coding_session(session.clone()),
                agent_id: None,
                event_kind: KIND.to_string(),
                cursor: None,
                events: vec![NormalizedEvent {
                    provider_event_id: provider_event_id.to_string(),
                    metadata: serde_json::json!({"state": "idle"}),
                    occurred_at: at,
                    landing: None,
                }],
                received_at: at,
                baseline: false,
            },
            eligible,
            &Every,
            None,
            None,
            backend.keys(),
        )
        .await
        .expect("ingest one session event")
}

pub async fn a_rule_and_an_event_of_a_coding_session_read_back_with_their_source(
    backend: &Backend,
) {
    let world = world(backend).await;
    let session = &world.sessions[0];
    let subscriptions = &backend.stores().subscriptions;
    let rule = rule(&world, session);
    subscriptions.create(&rule).await.unwrap();

    assert_eq!(
        subscriptions
            .get(&world.workspace.id, &rule.id)
            .await
            .unwrap()
            .as_ref(),
        Some(&rule)
    );
    let source = EventSource::coding_session(session.clone());
    assert_eq!(
        subscriptions
            .live_for_source(&world.workspace.id, &source, KIND)
            .await
            .unwrap(),
        vec![rule.clone()]
    );
    assert_eq!(
        subscriptions
            .list_for_source(&world.workspace.id, &source, &["active"])
            .await
            .unwrap(),
        vec![rule.clone()]
    );
    // The Person and the Agent do not manage the rule, and no collector
    // polls a Coding Session.
    assert!(
        subscriptions
            .list(&world.workspace.id, None, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(subscriptions.collector_targets().await.unwrap().is_empty());

    let outcome = ingest(
        backend,
        &world,
        session,
        "turn-1",
        std::slice::from_ref(&rule),
        20,
    )
    .await;
    assert_eq!(outcome.batch.source, source);
    assert_eq!(outcome.events.len(), 1);
    assert_eq!(outcome.events[0].source, source);
    assert_eq!(outcome.created.len(), 1);

    let stored = subscriptions
        .list_events(&world.workspace.id, &rule.id, None, 10)
        .await
        .unwrap();
    assert_eq!(stored, outcome.events);
    let context = backend
        .stores()
        .triggers
        .event_context(&world.workspace.id, &outcome.created[0].id)
        .await
        .unwrap()
        .expect("the Wake-up has its events");
    assert_eq!(context.source, source);
    assert_eq!(context.connection_alias, None);
    assert_eq!(context.events, outcome.events);
}

pub async fn one_event_id_stored_twice_for_one_session_makes_one_event(backend: &Backend) {
    let world = world(backend).await;
    let session = &world.sessions[0];

    let first = ingest(backend, &world, session, "turn-1", &[], 20).await;
    let second = ingest(backend, &world, session, "turn-1", &[], 30).await;

    assert_eq!(first.events.len(), 1);
    assert!(second.events.is_empty(), "the id is already stored");
    assert_eq!(
        backend
            .count(
                "SELECT COUNT(*) FROM incoming_events WHERE coding_session_id = ?",
                &[session.as_str().into()],
            )
            .await
            .unwrap(),
        1
    );
}

pub async fn one_event_id_of_two_sessions_makes_two_events(backend: &Backend) {
    let world = world(backend).await;

    let first = ingest(backend, &world, &world.sessions[0], "turn-1", &[], 20).await;
    let second = ingest(backend, &world, &world.sessions[1], "turn-1", &[], 30).await;

    assert_eq!(first.events.len(), 1);
    assert_eq!(second.events.len(), 1, "each session has its own ids");
    assert_eq!(
        backend
            .count(
                "SELECT COUNT(*) FROM incoming_events WHERE provider_event_id = ?",
                &["turn-1".into()],
            )
            .await
            .unwrap(),
        2
    );
}

#[macro_export]
macro_rules! store_suite_event_sources {
    ($emit:path) => {
        $emit!(
            event_sources,
            a_rule_and_an_event_of_a_coding_session_read_back_with_their_source,
            one_event_id_stored_twice_for_one_session_makes_one_event,
            one_event_id_of_two_sessions_makes_two_events,
        );
    };
}
