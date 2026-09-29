//! Synced arrivals enter the stored Wake-up and Run path.

use crate::common::{self, NOW, TEST_EVENT_KIND, World};

use pagis_core::{IngestBatch, NormalizedEvent, TriggerKind};
use pagis_trigger::ArrivalRun;

fn mail(id: &str, thread: &str) -> NormalizedEvent {
    NormalizedEvent {
        provider_event_id: id.to_string(),
        metadata: serde_json::json!({"thread_id": thread}),
        occurred_at: NOW,
        landing: None,
    }
}

fn batch(world: &World) -> IngestBatch {
    IngestBatch {
        workspace_id: world.workspace_id.clone(),
        connection_id: world.connection_id.clone(),
        agent_id: None,
        event_kind: TEST_EVENT_KIND.to_string(),
        cursor: Some("cursor-1".to_string()),
        events: vec![mail("m1", "thread-1"), mail("m2", "thread-2")],
        received_at: NOW,
        baseline: false,
    }
}

#[tokio::test]
async fn one_arrival_batch_creates_one_run_for_its_subject_pages() {
    let world = common::world().await;
    let arrival = ArrivalRun {
        agent_id: world.agent_id.clone(),
        subject_paths: vec![
            "private/subjects/gmail/thread-1.md".to_string(),
            "private/subjects/gmail/thread-2.md".to_string(),
        ],
        historical: false,
    };

    let first = world
        .trigger
        .ingest_arrivals(batch(&world), arrival.clone())
        .await
        .expect("ingest arrivals");
    let second = world
        .trigger
        .ingest_arrivals(batch(&world), arrival)
        .await
        .expect("repeat overlapping arrivals");

    assert_eq!(first.created.len(), 1);
    assert!(second.created.is_empty());
    let claims = world
        .trigger
        .claim_wakeups(
            &world.workspace_id,
            &world.agent_id,
            pagis_core::RunSlots {
                conversation: 10,
                arrival: 10,
            },
            NOW + 1,
        )
        .await
        .expect("claim arrival run");
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].run.trigger_kind, TriggerKind::Arrival);
    assert!(claims[0].run.channel_id.is_none());
    assert_eq!(
        claims[0].wakeup.rule.subject_paths(),
        &[
            "private/subjects/gmail/thread-1.md".to_string(),
            "private/subjects/gmail/thread-2.md".to_string(),
        ]
    );
}

#[tokio::test]
async fn a_backfill_batch_without_events_starts_one_historical_run() {
    let world = common::world().await;
    let paths = vec!["private/subjects/gmail/old-1.md".to_string()];

    let outcome = world
        .trigger
        .ingest_arrivals(
            IngestBatch {
                events: Vec::new(),
                cursor: None,
                ..batch(&world)
            },
            ArrivalRun {
                agent_id: world.agent_id.clone(),
                subject_paths: paths.clone(),
                historical: true,
            },
        )
        .await
        .expect("ingest a backfill batch");

    assert_eq!(outcome.created.len(), 1, "the pages alone start the Run");
    assert_eq!(outcome.created[0].source_count, 1);
    assert!(outcome.created[0].rule.historical());
    let claims = world
        .trigger
        .claim_wakeups(
            &world.workspace_id,
            &world.agent_id,
            pagis_core::RunSlots {
                conversation: 10,
                arrival: 10,
            },
            NOW + 1,
        )
        .await
        .expect("claim the backfill run");
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].run.trigger_kind, TriggerKind::Arrival);
    assert_eq!(claims[0].wakeup.rule.subject_paths(), paths.as_slice());
    assert!(
        claims[0].wakeup.rule.historical(),
        "the Wake-up keeps the historical flag"
    );
}
