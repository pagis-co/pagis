use async_trait::async_trait;
use pagis_core::{
    AgentId, ChannelId, Event, EventId, EventLog, NewEvent, RunId, StoreError, WorkspaceId, now_ms,
};
use sqlx::{PgPool, Row};

use crate::db_err;

/// The events table. `seq` is the identity column: strictly increasing
/// for appends, never reused, and the catch-up cursor for the event
/// bus. No row is deleted: a Forget clears the record fields of an
/// entry in place (ADR-0008).
#[derive(Clone)]
pub struct PostgresEventLog {
    pool: PgPool,
}

impl PostgresEventLog {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_event(row: &sqlx::postgres::PgRow) -> Result<Event, StoreError> {
    let payload: String = row.get("payload");
    Ok(Event {
        id: EventId::from(row.get::<String, _>("id")),
        seq: row.get("seq"),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        event_type: row.get("event_type"),
        agent_id: row.get::<Option<String>, _>("agent_id").map(AgentId::from),
        run_id: row.get::<Option<String>, _>("run_id").map(RunId::from),
        channel_id: row
            .get::<Option<String>, _>("channel_id")
            .map(ChannelId::from),
        payload: serde_json::from_str(&payload)
            .map_err(|e| StoreError::Corrupt(format!("event payload: {e}")))?,
        created_at: row.get("created_at"),
    })
}

#[async_trait]
impl EventLog for PostgresEventLog {
    async fn append(&self, event: NewEvent) -> Result<Event, StoreError> {
        let id = EventId::generate();
        let created_at = now_ms();
        let payload = serde_json::to_string(&event.payload)
            .map_err(|e| StoreError::Corrupt(format!("event payload: {e}")))?;
        let row = sqlx::query(
            "INSERT INTO events (id, workspace_id, event_type, agent_id, run_id, channel_id, payload, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING seq",
        )
        .bind(id.as_str())
        .bind(event.workspace_id.as_str())
        .bind(&event.event_type)
        .bind(event.agent_id.as_ref().map(|a| a.as_str().to_owned()))
        .bind(event.run_id.as_ref().map(|r| r.as_str().to_owned()))
        .bind(event.channel_id.as_ref().map(|c| c.as_str().to_owned()))
        .bind(&payload)
        .bind(created_at)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(Event {
            id,
            seq: row.get("seq"),
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at,
        })
    }

    async fn list_after(
        &self,
        workspace_id: Option<&WorkspaceId>,
        after_seq: i64,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        // The filter goes to the database, so a Workspace's subscriber
        // never reads another tenant's rows.
        let sql = format!(
            "SELECT seq, id, workspace_id, event_type, agent_id, run_id, channel_id, payload, created_at \
             FROM events WHERE seq > $1{} ORDER BY seq LIMIT $2",
            if workspace_id.is_some() {
                " AND workspace_id = $3"
            } else {
                ""
            },
        );
        let mut query = sqlx::query(&sql).bind(after_seq).bind(i64::from(limit));
        if let Some(workspace_id) = workspace_id {
            query = query.bind(workspace_id.as_str().to_owned());
        }
        let rows = query.fetch_all(&self.pool).await.map_err(db_err)?;
        rows.iter().map(row_to_event).collect()
    }

    async fn latest_seq(&self) -> Result<i64, StoreError> {
        let row = sqlx::query("SELECT COALESCE(MAX(seq), 0) AS seq FROM events")
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(row.get("seq"))
    }

    async fn list_by_types(
        &self,
        workspace_id: &WorkspaceId,
        event_types: &[&str],
        before: Option<&EventId>,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        if event_types.is_empty() {
            return Ok(Vec::new());
        }
        // The event types lead the bind order, so the numbers of the
        // four binds behind them move with the length of the list.
        let placeholders = (0..event_types.len())
            .map(|index| format!("${}", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let workspace = event_types.len() + 1;
        let before_null = event_types.len() + 2;
        let before_id = event_types.len() + 3;
        let row_limit = event_types.len() + 4;
        let sql = format!(
            "SELECT seq, id, workspace_id, event_type, agent_id, run_id, channel_id, payload, created_at \
             FROM events WHERE event_type IN ({placeholders}) AND workspace_id = ${workspace} \
             AND (${before_null} IS NULL OR id < ${before_id}) \
             ORDER BY id DESC LIMIT ${row_limit}",
        );
        let mut query = sqlx::query(&sql);
        for event_type in event_types {
            query = query.bind(*event_type);
        }
        let before = before.map(|id| id.as_str().to_owned());
        let rows = query
            .bind(workspace_id.as_str())
            .bind(before.clone())
            .bind(before)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        rows.iter().map(row_to_event).collect()
    }

    async fn list_for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Vec<Event>, StoreError> {
        let rows = sqlx::query(
            "SELECT seq, id, workspace_id, event_type, agent_id, run_id, channel_id, payload, created_at \
             FROM events WHERE run_id = $1 AND workspace_id = $2 ORDER BY seq",
        )
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_event).collect()
    }
}
