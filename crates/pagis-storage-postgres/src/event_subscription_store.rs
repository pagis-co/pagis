//! Event Subscriptions and their public history (ADR-0006).

use async_trait::async_trait;
use pagis_core::{
    AgentId, ChannelId, CollectorTarget, ConnectionId, EventSource, EventSubscription,
    EventSubscriptionId, EventSubscriptionStore, IncomingEvent, IncomingEventId, MessageId,
    SourceBatch, StoreError, Wakeup, WakeupId, WorkspaceId,
};
use sqlx::{PgPool, Row};

use crate::db_err;
use crate::schedule_store::{WAKEUP_COLUMNS, row_to_wakeup};
use crate::trigger_store::{
    BATCH_COLUMNS, EVENT_COLUMNS, row_to_batch, row_to_event, row_to_source, source_column,
};

const SUBSCRIPTION_COLUMNS: &str = "created_by_kind, created_by_person_id, created_by_agent_id, id, workspace_id, agent_id, connection_id, \
    coding_session_id, event_kind, \
    source_version, name, instruction, channel_id, root_message_id, filter, state, \
    revision, approved_revision, watermark_at, blocked_reason, created_at, updated_at, \
    archived_at, failures, next_attempt_at";

#[derive(Clone)]
pub struct PostgresEventSubscriptionStore {
    pool: PgPool,
}

impl PostgresEventSubscriptionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_subscription(row: &sqlx::postgres::PgRow) -> Result<EventSubscription, StoreError> {
    Ok(EventSubscription {
        created_by: pagis_core::Author::from_columns(
            &row.get::<String, _>("created_by_kind"),
            row.get("created_by_person_id"),
            row.get("created_by_agent_id"),
        )
        .map_err(StoreError::Corrupt)?,
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
        failures: revision(row, "failures")?,
        next_attempt_at: row.get("next_attempt_at"),
        blocked_reason: row
            .get::<Option<String>, _>("blocked_reason")
            .map(|value| value.parse().map_err(StoreError::Corrupt))
            .transpose()?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        archived_at: row.get("archived_at"),
    })
}

fn revision(row: &sqlx::postgres::PgRow, name: &str) -> Result<u32, StoreError> {
    u32::try_from(row.get::<i64, _>(name))
        .map_err(|error| StoreError::Corrupt(format!("{name}: {error}")))
}

/// Write the mutable fields of one rule. Answer how many rows changed.
async fn write_rule(
    executor: impl sqlx::PgExecutor<'_>,
    subscription: &EventSubscription,
) -> Result<u64, StoreError> {
    Ok(sqlx::query(
        "UPDATE event_subscriptions SET name = $1, instruction = $2, channel_id = $3, \
         root_message_id = $4, filter = $5, state = $6, revision = $7, \
         approved_revision = $8, watermark_at = $9, blocked_reason = $10, updated_at = $11, \
         archived_at = $12 WHERE id = $13 AND workspace_id = $14",
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
impl EventSubscriptionStore for PostgresEventSubscriptionStore {
    async fn create(&self, subscription: &EventSubscription) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO event_subscriptions (created_by_kind, created_by_person_id, created_by_agent_id, id, workspace_id, agent_id, connection_id, \
             coding_session_id, event_kind, source_version, name, instruction, channel_id, \
             root_message_id, filter, state, revision, approved_revision, watermark_at, \
             blocked_reason, created_at, updated_at, archived_at, failures, next_attempt_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, \
             $20, $21, $22, $23, $24, $25)",
        )
        .bind(subscription.created_by.kind())
        .bind(subscription.created_by.person_id().map(|id| id.as_str()))
        .bind(subscription.created_by.agent_id().map(|id| id.as_str()))
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
        .bind(subscription.state.as_str())
        .bind(i64::from(subscription.revision))
        .bind(subscription.approved_revision.map(i64::from))
        .bind(subscription.watermark_at)
        .bind(subscription.blocked_reason.map(|reason| reason.as_str()))
        .bind(subscription.created_at)
        .bind(subscription.updated_at)
        .bind(subscription.archived_at)
        .bind(i64::from(subscription.failures))
        .bind(subscription.next_attempt_at)
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
             WHERE id = $1 AND workspace_id = $2"
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
            "SELECT {SUBSCRIPTION_COLUMNS} FROM event_subscriptions WHERE workspace_id = $1 \
             AND connection_id IS NOT NULL AND ($2 IS NULL OR agent_id = $3) AND ($4 IS NULL OR id < $5) \
             ORDER BY id DESC LIMIT $6"
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
        // The rule is written first. Its row lock makes a collection
        // pass that rechecks the rule (see `ingest`) wait for this
        // commit and then see the new rule. A pass that locked the rule
        // first commits before this write, and its Wake-up is withdrawn
        // here.
        write_rule(&mut *transaction, subscription).await?;
        let rows = sqlx::query(&format!(
            "UPDATE wakeups SET state = 'withdrawn' WHERE state = 'pending' \
             AND workspace_id = $1 AND subscription_id = $2 RETURNING {WAKEUP_COLUMNS}"
        ))
        .bind(subscription.workspace_id.as_str())
        .bind(subscription.id.as_str())
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;
        rows.iter().map(row_to_wakeup).collect()
    }

    async fn collector_targets(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<CollectorTarget>, StoreError> {
        let rows = sqlx::query(
            "SELECT workspace_id, connection_id, event_kind, MAX(failures) AS failures, \
             MAX(next_attempt_at) AS next_attempt_at FROM event_subscriptions \
             WHERE workspace_id = $1 AND connection_id IS NOT NULL AND state = 'active' \
             AND approved_revision = revision GROUP BY workspace_id, connection_id, event_kind \
             ORDER BY connection_id, event_kind",
        )
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.into_iter()
            .map(|row| {
                Ok(CollectorTarget {
                    workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
                    connection_id: ConnectionId::from(row.get::<String, _>("connection_id")),
                    event_kind: row.get("event_kind"),
                    failures: revision(&row, "failures")?,
                    next_attempt_at: row.get("next_attempt_at"),
                })
            })
            .collect()
    }

    async fn set_collector_backoff(
        &self,
        target: &CollectorTarget,
        failures: u32,
        next_attempt_at: i64,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE event_subscriptions SET failures = $1, next_attempt_at = $2 \
            WHERE workspace_id = $3 AND connection_id = $4 AND event_kind = $5",
        )
        .bind(i64::from(failures))
        .bind(next_attempt_at)
        .bind(target.workspace_id.as_str())
        .bind(target.connection_id.as_str())
        .bind(&target.event_kind)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn live_for_source(
        &self,
        workspace_id: &WorkspaceId,
        source: &EventSource,
        event_kind: &str,
    ) -> Result<Vec<EventSubscription>, StoreError> {
        let (column, source_id) = source_column(source);
        let rows = sqlx::query(&format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM event_subscriptions WHERE {column} = $1 \
             AND workspace_id = $2 AND event_kind = $3 AND state = 'active' \
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

    async fn system_for_source(
        &self,
        workspace_id: &WorkspaceId,
        agent: &AgentId,
        source: &EventSource,
        event_kind: &str,
        name: &str,
    ) -> Result<Option<EventSubscription>, StoreError> {
        let (column, source_id) = source_column(source);
        let row = sqlx::query(&format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM event_subscriptions WHERE {column} = $1 AND workspace_id = $2 AND event_kind = $3 AND agent_id = $4 AND name = $5 AND created_by_kind = 'system' AND state = 'active' AND approved_revision = revision ORDER BY id LIMIT 1"
        ))
        .bind(source_id).bind(workspace_id.as_str()).bind(event_kind).bind(agent.as_str())
        .bind(name)
        .fetch_optional(&self.pool).await.map_err(db_err)?;
        row.as_ref().map(row_to_subscription).transpose()
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
        // The state list is the last of the binds, so it starts behind
        // the source and the Workspace.
        let placeholders = (0..states.len())
            .map(|index| format!("${}", index + 3))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {SUBSCRIPTION_COLUMNS} FROM event_subscriptions WHERE {column} = $1 \
             AND workspace_id = $2 AND state IN ({placeholders}) ORDER BY id"
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
             WHERE w.subscription_id = $1 AND e.workspace_id = $2 \
             AND ($3 IS NULL OR e.id < $4) \
             ORDER BY e.id DESC LIMIT $5",
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
            "SELECT {WAKEUP_COLUMNS} FROM wakeups WHERE subscription_id = $1 \
             AND workspace_id = $2 AND ($3 IS NULL OR id < $4) ORDER BY id DESC LIMIT $5"
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

impl PostgresEventSubscriptionStore {
    /// The newest batch, optionally skipping one outcome.
    async fn newest_batch(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
        exclude: Option<&str>,
    ) -> Result<Option<SourceBatch>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {BATCH_COLUMNS} FROM source_batches WHERE connection_id = $1 \
             AND workspace_id = $2 AND event_kind = $3 \
             AND ($4 IS NULL OR outcome != $5) ORDER BY id DESC LIMIT 1"
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
