//! Event Subscriptions and their public history (ADR-0006).

use async_trait::async_trait;
use pagis_core::{
    AgentId, ChannelId, CollectorTarget, ConnectionId, EventSource, EventSubscription,
    EventSubscriptionId, EventSubscriptionStore, IncomingEvent, IncomingEventId, MessageId,
    SourceBatch, StoreError, Wakeup, WakeupId, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;
use crate::schedule_store::{WAKEUP_COLUMNS, row_to_wakeup};
use crate::trigger_store::{
    BATCH_COLUMNS, EVENT_COLUMNS, row_to_batch, row_to_event, row_to_source, source_column,
};

const SUBSCRIPTION_COLUMNS: &str = "id, workspace_id, agent_id, connection_id, \
    coding_session_id, event_kind, \
    source_version, name, instruction, channel_id, root_message_id, filter, creator, state, \
    revision, approved_revision, watermark_at, blocked_reason, created_at, updated_at, \
    archived_at";

#[derive(Clone)]
pub struct SqliteEventSubscriptionStore {
    pool: SqlitePool,
}

impl SqliteEventSubscriptionStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn row_to_subscription(row: &sqlx::sqlite::SqliteRow) -> Result<EventSubscription, StoreError> {
    Ok(EventSubscription {
        id: EventSubscriptionId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        source: row_to_source(row)?,
        event_kind: row.get("event_kind"),
        source_version: row.get("source_version"),
        name: row.get("name"),
        instruction: row.get("instruction"),
        channel_id: ChannelId::from(row.get::<String, _>("channel_id")),
        root_message_id: row
            .get::<Option<String>, _>("root_message_id")
            .map(MessageId::from),
        filter: serde_json::from_str(&row.get::<String, _>("filter"))
            .map_err(|error| StoreError::Corrupt(format!("filter: {error}")))?,
        creator: row
            .get::<String, _>("creator")
            .parse()
            .map_err(StoreError::Corrupt)?,
        state: row
            .get::<String, _>("state")
            .parse()
            .map_err(StoreError::Corrupt)?,
        revision: revision(row, "revision")?,
        approved_revision: row
            .get::<Option<i64>, _>("approved_revision")
            .map(|value| {
                u32::try_from(value)
                    .map_err(|error| StoreError::Corrupt(format!("approved_revision: {error}")))
            })
            .transpose()?,
        watermark_at: row.get("watermark_at"),
        blocked_reason: row
            .get::<Option<String>, _>("blocked_reason")
            .map(|value| value.parse().map_err(StoreError::Corrupt))
            .transpose()?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        archived_at: row.get("archived_at"),
    })
}

fn revision(row: &sqlx::sqlite::SqliteRow, name: &str) -> Result<u32, StoreError> {
    u32::try_from(row.get::<i64, _>(name))
        .map_err(|error| StoreError::Corrupt(format!("{name}: {error}")))
}

/// Write the mutable fields of one rule. Answer how many rows changed.
async fn write_rule(
    executor: impl sqlx::SqliteExecutor<'_>,
    subscription: &EventSubscription,
) -> Result<u64, StoreError> {
    Ok(sqlx::query(
        "UPDATE event_subscriptions SET name = ?, instruction = ?, channel_id = ?, \
         root_message_id = ?, filter = ?, state = ?, revision = ?, approved_revision = ?, \
         watermark_at = ?, blocked_reason = ?, updated_at = ?, archived_at = ? \
         WHERE id = ? AND workspace_id = ?",
    )
    .bind(&subscription.name)
    .bind(&subscription.instruction)
    .bind(subscription.channel_id.as_str())
    .bind(subscription.root_message_id.as_ref().map(|id| id.as_str()))
    .bind(subscription.filter.to_string())
    .bind(subscription.state.as_str())
    .bind(i64::from(subscription.revision))
    .bind(subscription.approved_revision.map(i64::from))
    .bind(subscription.watermark_at)
    .bind(subscription.blocked_reason.map(|reason| reason.as_str()))
    .bind(subscription.updated_at)
    .bind(subscription.archived_at)
    .bind(subscription.id.as_str())
    .bind(subscription.workspace_id.as_str())
    .execute(executor)
    .await
    .map_err(db_err)?
    .rows_affected())
}

#[async_trait]
impl EventSubscriptionStore for SqliteEventSubscriptionStore {
    async fn create(&self, subscription: &EventSubscription) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO event_subscriptions (id, workspace_id, agent_id, connection_id, \
             coding_session_id, event_kind, source_version, name, instruction, channel_id, \
             root_message_id, filter, creator, state, revision, approved_revision, watermark_at, \
             blocked_reason, created_at, updated_at, archived_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(subscription.id.as_str())
        .bind(subscription.workspace_id.as_str())
        .bind(subscription.agent_id.as_str())
        .bind(
            subscription
                .source
                .connection_id()
                .map(ConnectionId::as_str),
        )
        .bind(
            subscription
                .source
                .coding_session_id()
                .map(pagis_core::CodingSessionId::as_str),
        )
        .bind(&subscription.event_kind)
        .bind(&subscription.source_version)
        .bind(&subscription.name)
        .bind(&subscription.instruction)
        .bind(subscription.channel_id.as_str())
        .bind(subscription.root_message_id.as_ref().map(|id| id.as_str()))
        .bind(subscription.filter.to_string())
        .bind(subscription.creator.as_str())
        .bind(subscription.state.as_str())
        .bind(i64::from(subscription.revision))
        .bind(subscription.approved_revision.map(i64::from))
        .bind(subscription.watermark_at)
        .bind(subscription.blocked_reason.map(|reason| reason.as_str()))
        .bind(subscription.created_at)
        .bind(subscription.updated_at)
        .bind(subscription.archived_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &EventSubscriptionId,
    ) -> Result<Option<EventSubscription>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM event_subscriptions \
             WHERE id = ? AND workspace_id = ?"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_subscription).transpose()
    }

    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        before: Option<&EventSubscriptionId>,
        limit: u32,
    ) -> Result<Vec<EventSubscription>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM event_subscriptions WHERE workspace_id = ? \
             AND connection_id IS NOT NULL AND (? IS NULL OR agent_id = ?) \
             AND (? IS NULL OR id < ?) ORDER BY id DESC LIMIT ?"
        ))
        .bind(workspace_id.as_str())
        .bind(agent_id.map(AgentId::as_str))
        .bind(agent_id.map(AgentId::as_str))
        .bind(before.map(EventSubscriptionId::as_str))
        .bind(before.map(EventSubscriptionId::as_str))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_subscription).collect()
    }

    async fn update(&self, subscription: &EventSubscription) -> Result<bool, StoreError> {
        Ok(write_rule(&self.pool, subscription).await? > 0)
    }

    async fn update_and_withdraw(
        &self,
        subscription: &EventSubscription,
    ) -> Result<Vec<Wakeup>, StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        // `begin_write` takes the write lock, so a collection pass that
        // rechecks the rule (see `ingest`) runs before or after this
        // whole write: its Wake-up is withdrawn here, or it sees the
        // new rule.
        write_rule(&mut *transaction, subscription).await?;
        let rows = sqlx::query(&format!(
            "UPDATE wakeups SET state = 'withdrawn' WHERE state = 'pending' \
             AND workspace_id = ? AND subscription_id = ? RETURNING {WAKEUP_COLUMNS}"
        ))
        .bind(subscription.workspace_id.as_str())
        .bind(subscription.id.as_str())
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;
        rows.iter().map(row_to_wakeup).collect()
    }

    async fn collector_targets(&self) -> Result<Vec<CollectorTarget>, StoreError> {
        let rows = sqlx::query(
            "SELECT DISTINCT workspace_id, connection_id, event_kind FROM event_subscriptions \
             WHERE connection_id IS NOT NULL AND state = 'active' \
             AND approved_revision = revision ORDER BY connection_id, event_kind",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|row| CollectorTarget {
                workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
                connection_id: ConnectionId::from(row.get::<String, _>("connection_id")),
                event_kind: row.get("event_kind"),
            })
            .collect())
    }

    async fn live_for_source(
        &self,
        workspace_id: &WorkspaceId,
        source: &EventSource,
        event_kind: &str,
    ) -> Result<Vec<EventSubscription>, StoreError> {
        let (column, source_id) = source_column(source);
        let rows = sqlx::query(&format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM event_subscriptions WHERE {column} = ? \
             AND workspace_id = ? AND event_kind = ? AND state = 'active' \
             AND approved_revision = revision ORDER BY id"
        ))
        .bind(source_id)
        .bind(workspace_id.as_str())
        .bind(event_kind)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_subscription).collect()
    }

    async fn list_for_source(
        &self,
        workspace_id: &WorkspaceId,
        source: &EventSource,
        states: &[&str],
    ) -> Result<Vec<EventSubscription>, StoreError> {
        if states.is_empty() {
            return Ok(Vec::new());
        }
        let (column, source_id) = source_column(source);
        let placeholders = vec!["?"; states.len()].join(", ");
        let sql = format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM event_subscriptions WHERE {column} = ? \
             AND workspace_id = ? AND state IN ({placeholders}) ORDER BY id"
        );
        let mut query = sqlx::query(&sql)
            .bind(source_id)
            .bind(workspace_id.as_str());
        for state in states {
            query = query.bind(*state);
        }
        let rows = query.fetch_all(&self.pool).await.map_err(db_err)?;
        rows.iter().map(row_to_subscription).collect()
    }

    async fn list_events(
        &self,
        workspace_id: &WorkspaceId,
        subscription_id: &EventSubscriptionId,
        before: Option<&IncomingEventId>,
        limit: u32,
    ) -> Result<Vec<IncomingEvent>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {} FROM incoming_events e JOIN wakeup_sources s \
             ON s.source_kind = 'incoming_event' AND s.source_id = e.id \
             JOIN wakeups w ON w.id = s.wakeup_id \
             WHERE w.subscription_id = ? AND e.workspace_id = ? \
             AND (? IS NULL OR e.id < ?) \
             ORDER BY e.id DESC LIMIT ?",
            EVENT_COLUMNS
                .split(", ")
                .map(|column| format!("e.{column}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .bind(subscription_id.as_str())
        .bind(workspace_id.as_str())
        .bind(before.map(IncomingEventId::as_str))
        .bind(before.map(IncomingEventId::as_str))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_event).collect()
    }

    async fn list_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        subscription_id: &EventSubscriptionId,
        before: Option<&WakeupId>,
        limit: u32,
    ) -> Result<Vec<Wakeup>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {WAKEUP_COLUMNS} FROM wakeups WHERE subscription_id = ? \
             AND workspace_id = ? AND (? IS NULL OR id < ?) ORDER BY id DESC LIMIT ?"
        ))
        .bind(subscription_id.as_str())
        .bind(workspace_id.as_str())
        .bind(before.map(WakeupId::as_str))
        .bind(before.map(WakeupId::as_str))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_wakeup).collect()
    }

    async fn collector_health(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
    ) -> Result<(Option<SourceBatch>, Option<SourceBatch>), StoreError> {
        let latest = self
            .newest_batch(workspace_id, connection_id, event_kind, None)
            .await?;
        let succeeded = self
            .newest_batch(workspace_id, connection_id, event_kind, Some("failed"))
            .await?;
        Ok((latest, succeeded))
    }
}

impl SqliteEventSubscriptionStore {
    /// The newest batch, optionally skipping one outcome.
    async fn newest_batch(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
        exclude: Option<&str>,
    ) -> Result<Option<SourceBatch>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {BATCH_COLUMNS} FROM source_batches WHERE connection_id = ? \
             AND workspace_id = ? AND event_kind = ? \
             AND (? IS NULL OR outcome != ?) ORDER BY id DESC LIMIT 1"
        ))
        .bind(connection_id.as_str())
        .bind(workspace_id.as_str())
        .bind(event_kind)
        .bind(exclude)
        .bind(exclude)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_batch).transpose()
    }
}
