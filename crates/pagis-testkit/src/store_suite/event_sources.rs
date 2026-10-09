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

/// A rule has at most one active Run (ADR-0006): while the Run of its
/// first Wake-up is active, the next Wake-up of the rule waits, and the
/// events that come in that time join it.
pub async fn a_rule_whose_run_is_active_claims_no_second_wakeup(backend: &Backend) {
    let world = world(backend).await;
    let session = &world.sessions[0];
    let rule = rule(&world, session);
    backend.stores().subscriptions.create(&rule).await.unwrap();
    let eligible = std::slice::from_ref(&rule);
    let triggers = &backend.stores().triggers;
    let slots = pagis_core::RunSlots {
        conversation: 3,
        arrival: 0,
    };
    ingest(backend, &world, session, "turn-1", eligible, 20).await;
    let active = triggers
        .claim_wakeups(&world.workspace.id, &world.agent_id, slots, 21)
        .await
        .unwrap();
    assert_eq!(active.len(), 1);

    ingest(backend, &world, session, "turn-2", eligible, 30).await;
    let waiting = triggers
        .claim_wakeups(&world.workspace.id, &world.agent_id, slots, 31)
        .await
        .unwrap();
    assert!(waiting.is_empty(), "the Run of the rule is active");
    let joined = ingest(backend, &world, session, "turn-3", eligible, 40).await;
    assert_eq!(joined.combined.len(), 1);
    assert_eq!(joined.combined[0].source_count, 2);
}

pub async fn blocking_a_rule_withdraws_its_pending_work_in_the_same_write(backend: &Backend) {
    let world = world(backend).await;
    let session = &world.sessions[0];
    let subscriptions = &backend.stores().subscriptions;
    let rule = rule(&world, session);
    subscriptions.create(&rule).await.unwrap();
    let created = ingest(
        backend,
        &world,
        session,
        "turn-1",
        std::slice::from_ref(&rule),
        20,
    )
    .await
    .created;

    let blocked = EventSubscription {
        state: EventSubscriptionState::Blocked,
        blocked_reason: Some(pagis_core::BlockReason::GrantRevoked),
        updated_at: 30,
        ..rule.clone()
    };
    let withdrawn = subscriptions.update_and_withdraw(&blocked).await.unwrap();

    assert_eq!(
        withdrawn
            .iter()
            .map(|wakeup| &wakeup.id)
            .collect::<Vec<_>>(),
        created.iter().map(|wakeup| &wakeup.id).collect::<Vec<_>>()
    );
    assert!(
        withdrawn
            .iter()
            .all(|wakeup| wakeup.state == pagis_core::WakeupState::Withdrawn)
    );
    assert_eq!(
        subscriptions
            .get(&world.workspace.id, &rule.id)
            .await
            .unwrap(),
        Some(blocked)
    );
    let stored = subscriptions
        .list_wakeups(&world.workspace.id, &rule.id, None, 10)
        .await
        .unwrap();
    assert_eq!(stored, withdrawn);
}

pub async fn a_collection_pass_that_overlaps_a_block_of_the_rule_adds_no_wakeup(backend: &Backend) {
    let world = world(backend).await;
    let session = &world.sessions[0];
    let rule = rule(&world, session);
    backend.stores().subscriptions.create(&rule).await.unwrap();

    // A write blocks the rule and has not committed yet. The collection
    // pass started from a snapshot in which the rule is active.
    let block = backend
        .rows()
        .hold(
            "UPDATE event_subscriptions SET state = 'blocked' WHERE id = ?",
            &[rule.id.as_str().into()],
        )
        .await
        .unwrap();
    let (outcome, ()) = tokio::join!(
        ingest(
            backend,
            &world,
            session,
            "turn-1",
            std::slice::from_ref(&rule),
            20
        ),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            block.commit().await.unwrap();
        }
    );

    assert!(
        outcome.created.is_empty(),
        "a pass adds no Wake-up to a rule that a concurrent write blocks"
    );
    let wakeups = backend
        .stores()
        .subscriptions
        .list_wakeups(&world.workspace.id, &rule.id, None, 10)
        .await
        .unwrap();
    assert!(wakeups.is_empty(), "the blocked rule holds no pending work");
}

#[macro_export]
macro_rules! store_suite_event_sources {
    ($emit:path) => {
        $emit!(
            event_sources,
            a_rule_and_an_event_of_a_coding_session_read_back_with_their_source,
            one_event_id_stored_twice_for_one_session_makes_one_event,
            one_event_id_of_two_sessions_makes_two_events,
            a_rule_whose_run_is_active_claims_no_second_wakeup,
            blocking_a_rule_withdraws_its_pending_work_in_the_same_write,
            a_collection_pass_that_overlaps_a_block_of_the_rule_adds_no_wakeup,
        );
    };
}
