//! An Incoming Event of a forgotten Source Item (ADR-0008, ADR-0006).
//!
//! A Forget blocks a new retrieval of its item. The Trigger module
//! reads the synced resource of each event kind from its declaration
//! and gives it to the store with the suppression keys, so an event
//! with the id of a forgotten item writes no row and wakes nothing.

use crate::common::{self, NOW, TEST_CAPABILITY, TEST_EVENT_KIND, TEST_SOURCE_RESOURCE, World};

use pagis_core::knowledge::{
    KnowledgeStore, SourceBatch, SourceChange, SourceKey, SourceOperation, SyncConfig,
};
use pagis_core::reflection_filter::{ReflectionFilter, Verdict};
use pagis_core::{ForgetStore, ForgetTarget, IngestBatch, NormalizedEvent};
use pagis_storage_sqlite::{SqliteForgetStore, SqliteKnowledgeStore};
use pagis_trigger::NewSubscription;

fn mail(id: &str) -> NormalizedEvent {
    NormalizedEvent {
        provider_event_id: id.to_string(),
        metadata: serde_json::json!({"from": "a@example.com", "subject": "Hello"}),
        occurred_at: NOW + 10,
        landing: None,
    }
}

/// The Sync of the world's Connection acquires one message, and the
/// Person forgets it. The suppression row that the Forget writes is
/// what blocks the message; the purge that follows keeps that row.
async fn forget_message(world: &World, id: &str) {
    let key = SourceKey {
        workspace_id: world.workspace_id.clone(),
        connection_id: world.connection_id.clone(),
        resource: TEST_SOURCE_RESOURCE.to_string(),
    };
    let knowledge = SqliteKnowledgeStore::new(world.pool.clone());
    let status = knowledge
        .configure(
            SyncConfig {
                workspace_id: key.workspace_id.clone(),
                connection_id: key.connection_id.clone(),
                resource: key.resource.clone(),
                agent_id: world.agent_id.clone(),
                required_capability: TEST_CAPABILITY.to_string(),
                enabled: true,
                since: 0,
                filter: ReflectionFilter {
                    rules: Vec::new(),
                    default: Verdict::Reflect,
                },
            },
            NOW,
        )
        .await
        .expect("configure the Sync");
    knowledge
        .acquire(
            &key,
            &SourceBatch {
                historical: false,
                arrivals: Vec::new(),
                expected_revision: status.cursor_revision,
                checkpoint: serde_json::json!({"page": 1}),
                caught_up: true,
                changes: vec![SourceChange {
                    id: id.to_string(),
                    version: "1".to_string(),
                    kind: "mail".to_string(),
                    parent: None,
                    source_at: Some(NOW),
                    operation: SourceOperation::Upsert,
                    metadata: serde_json::json!({}),
                }],
            },
            NOW,
            world.keys.as_ref(),
        )
        .await
        .expect("acquire the message");
    let forget = SqliteForgetStore::new(world.pool.clone());
    let target = ForgetTarget::Source {
        source: key,
        source_id: id.to_string(),
    };
    let preview = forget
        .preview(&world.workspace_id, &target)
        .await
        .expect("preview the Forget");
    forget
        .begin(
            &world.workspace_id,
            &target,
            &preview.revision,
            NOW,
            world.keys.as_ref(),
        )
        .await
        .expect("forget the message");
}

#[tokio::test]
async fn an_event_of_a_forgotten_message_is_dropped_and_wakes_nothing() {
    let world = common::world().await;
    world
        .trigger
        .create_subscription(NewSubscription {
            workspace_id: world.workspace_id.clone(),
            agent_id: world.agent_id.clone(),
            source: pagis_core::EventSource::connection(world.connection_id.clone()),
            event_kind: TEST_EVENT_KIND.to_string(),
            name: "inbox".to_string(),
            instruction: "Read the new mail.".to_string(),
            channel_id: world.channel_id.clone(),
            root_message_id: None,
            filter: serde_json::json!({}),
            creator: pagis_core::CreatorKind::User,
            now: NOW,
        })
        .await
        .expect("create subscription");
    forget_message(&world, "m-forgotten").await;

    let outcome = world
        .trigger
        .ingest(IngestBatch {
            workspace_id: world.workspace_id.clone(),
            source: pagis_core::EventSource::connection(world.connection_id.clone()),
            agent_id: None,
            event_kind: TEST_EVENT_KIND.to_string(),
            cursor: Some("cursor-1".to_string()),
            events: vec![mail("m-forgotten"), mail("m-kept")],
            received_at: NOW + 20,
            baseline: false,
        })
        .await
        .expect("ingest");

    assert_eq!(
        outcome
            .events
            .iter()
            .map(|event| event.provider_event_id.as_str())
            .collect::<Vec<_>>(),
        vec!["m-kept"],
        "the pass stores the event of the other message alone"
    );
    assert_eq!(outcome.created.len(), 1);
    assert_eq!(
        outcome.created[0].source_count, 1,
        "the Wake-up links the event of the forgotten message"
    );
    let stored: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM incoming_events WHERE provider_event_id = 'm-forgotten'",
    )
    .fetch_one(&world.pool)
    .await
    .expect("count the events of the forgotten message");
    assert_eq!(stored, 0, "the event of the forgotten message is stored");
}
