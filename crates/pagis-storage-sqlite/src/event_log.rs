use async_trait::async_trait;
use pagis_core::{
    AgentId, ChannelId, Event, EventId, EventLog, NewEvent, RunId, StoreError, WorkspaceId, now_ms,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

/// The events table. `seq` is the SQLite rowid: strictly increasing for
/// appends, never reused while rows are retained, and the catch-up
/// cursor for the event bus. No row is deleted: a Forget clears the
/// record fields of an entry in place (ADR-0008).
#[derive(Clone)]
pub struct SqliteEventLog {
    pool: SqlitePool,
}

impl SqliteEventLog {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn row_to_event(row: &sqlx::sqlite::SqliteRow) -> Result<Event, StoreError> {
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
impl EventLog for SqliteEventLog {
    async fn append(&self, event: NewEvent) -> Result<Event, StoreError> {
        let id = EventId::generate();
        let created_at = now_ms();
        let payload = serde_json::to_string(&event.payload)
            .map_err(|e| StoreError::Corrupt(format!("event payload: {e}")))?;
        let row = sqlx::query(
            "INSERT INTO events (id, workspace_id, event_type, agent_id, run_id, channel_id, payload, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING rowid AS seq",
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
            "SELECT rowid AS seq, id, workspace_id, event_type, agent_id, run_id, channel_id, payload, created_at \
             FROM events WHERE rowid > ?{} ORDER BY rowid LIMIT ?",
            if workspace_id.is_some() {
                " AND workspace_id = ?"
            } else {
                ""
            },
        );
        let mut query = sqlx::query(&sql).bind(after_seq);
        if let Some(workspace_id) = workspace_id {
            query = query.bind(workspace_id.as_str().to_owned());
        }
        let rows = query
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        rows.iter().map(row_to_event).collect()
    }

    async fn latest_seq(&self) -> Result<i64, StoreError> {
        let row = sqlx::query("SELECT COALESCE(MAX(rowid), 0) AS seq FROM events")
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
        let placeholders = vec!["?"; event_types.len()].join(", ");
        let sql = format!(
            "SELECT rowid AS seq, id, workspace_id, event_type, agent_id, run_id, channel_id, payload, created_at \
             FROM events WHERE event_type IN ({placeholders}) AND workspace_id = ? \
             AND (? IS NULL OR id < ?) \
             ORDER BY id DESC LIMIT ?",
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
            "SELECT rowid AS seq, id, workspace_id, event_type, agent_id, run_id, channel_id, payload, created_at \
             FROM events WHERE run_id = ? AND workspace_id = ? ORDER BY rowid",
        )
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_event).collect()
    }
}
