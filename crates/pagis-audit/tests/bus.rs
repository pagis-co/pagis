//! Trait-seam tests for the event bus: append-first publication,
//! live delivery, catch-up from the table, and lag recovery.

use std::sync::Arc;

use futures::StreamExt;
use pagis_audit::AuditEventBus;
use pagis_core::{EventBus, EventScope, NewEvent, Workspace, WorkspaceId, WorkspaceStore};
use pagis_storage_sqlite::{SqliteEventLog, SqliteWorkspaceStore};
use sqlx::SqlitePool;

async fn seeded_workspace(pool: &SqlitePool) -> WorkspaceId {
    let ws = pagis_testkit::fixture::seeded_workspace(pool).await;
    SqliteWorkspaceStore::new(pool.clone())
        .create(&ws)
        .await
        .unwrap();
    ws.id
}

fn event(workspace_id: &WorkspaceId, event_type: &str) -> NewEvent {
    NewEvent {
        workspace_id: workspace_id.clone(),
        event_type: event_type.to_string(),
        agent_id: None,
        run_id: None,
        channel_id: None,
        payload: serde_json::json!({}),
    }
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn publish_reaches_live_subscriber(pool: SqlitePool) {
    let ws = seeded_workspace(&pool).await;
    let bus = AuditEventBus::new(Arc::new(SqliteEventLog::new(pool)));

    let mut sub = bus.subscribe(EventScope::Installation, None).await;
    let published = bus.publish(event(&ws, "workspace.created")).await.unwrap();

    let received = sub.next().await.unwrap();
    assert_eq!(received, published);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn subscriber_from_cursor_catches_up_from_table(pool: SqlitePool) {
    let ws = seeded_workspace(&pool).await;
    let bus = AuditEventBus::new(Arc::new(SqliteEventLog::new(pool)));

    let first = bus.publish(event(&ws, "a")).await.unwrap();
    let second = bus.publish(event(&ws, "b")).await.unwrap();

    let mut sub = bus.subscribe(EventScope::Installation, Some(0)).await;
    assert_eq!(sub.next().await.unwrap(), first);
    assert_eq!(sub.next().await.unwrap(), second);

    let mut sub_after_first = bus
        .subscribe(EventScope::Installation, Some(first.seq))
        .await;
    assert_eq!(sub_after_first.next().await.unwrap(), second);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn lagged_subscriber_catches_up_without_gaps(pool: SqlitePool) {
    let ws = seeded_workspace(&pool).await;
    // Broadcast capacity 2: publishing 10 events before the subscriber
    // polls guarantees the in-memory channel lags.
    let bus = AuditEventBus::with_capacity(Arc::new(SqliteEventLog::new(pool)), 2);

    let mut sub = bus.subscribe(EventScope::Installation, None).await;
    let mut published = Vec::new();
    for i in 0..10 {
        published.push(bus.publish(event(&ws, &format!("e{i}"))).await.unwrap());
    }

    for expected in &published {
        assert_eq!(&sub.next().await.unwrap(), expected);
    }
}

/// A Workspace subscription carries that Workspace's events and no
/// other's, live and from the table.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_workspace_subscription_skips_every_other_workspace(pool: SqlitePool) {
    let first = seeded_workspace(&pool).await;
    let second = WorkspaceId::generate();
    let stores = SqliteWorkspaceStore::new(pool.clone());
    let template = stores.get(&first).await.unwrap().expect("the seeded one");
    stores
        .create(&Workspace {
            id: second.clone(),
            ..template
        })
        .await
        .expect("write the second Workspace");
    let bus = AuditEventBus::new(Arc::new(SqliteEventLog::new(pool)));

    // One event of each before the subscription, one of each after: the
    // catch-up read and the live wakeup both filter.
    let before_mine = bus.publish(event(&first, "before.mine")).await.unwrap();
    bus.publish(event(&second, "before.theirs")).await.unwrap();

    let mut sub = bus
        .subscribe(EventScope::workspace(&first), Some(before_mine.seq - 1))
        .await;

    bus.publish(event(&second, "after.theirs")).await.unwrap();
    let after_mine = bus.publish(event(&first, "after.mine")).await.unwrap();

    assert_eq!(sub.next().await.unwrap(), before_mine);
    assert_eq!(sub.next().await.unwrap(), after_mine);
}
