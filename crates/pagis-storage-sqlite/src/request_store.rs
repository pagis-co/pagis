use async_trait::async_trait;
use pagis_core::{
    AgentId, DecideOutcome, Request, RequestId, RequestState, RequestStore, RunId, StoreError,
    UnixMillis, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

const COLUMNS: &str = "id, workspace_id, agent_id, run_id, kind, payload, state, \
                       submitted_values, decided_at, created_at";

#[derive(Clone)]
pub struct SqliteRequestStore {
    pool: SqlitePool,
}

impl SqliteRequestStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn row_to_request(row: &sqlx::sqlite::SqliteRow) -> Result<Request, StoreError> {
    let payload: String = row.get("payload");
    let values: Option<String> = row.get("submitted_values");
    let state: String = row.get("state");
    Ok(Request {
        id: RequestId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        run_id: row.get::<Option<String>, _>("run_id").map(RunId::from),
        kind: row.get("kind"),
        payload: serde_json::from_str(&payload)
            .map_err(|e| StoreError::Corrupt(format!("request payload: {e}")))?,
        state: state.parse::<RequestState>().map_err(StoreError::Corrupt)?,
        values: values
            .map(|values| serde_json::from_str(&values))
            .transpose()
            .map_err(|e| StoreError::Corrupt(format!("request values: {e}")))?,
        decided_at: row.get("decided_at"),
        created_at: row.get("created_at"),
    })
}

#[async_trait]
impl RequestStore for SqliteRequestStore {
    async fn create(&self, request: &Request) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO requests (id, workspace_id, agent_id, run_id, kind, payload, state, \
             submitted_values, decided_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(request.id.as_str())
        .bind(request.workspace_id.as_str())
        .bind(request.agent_id.as_str())
        .bind(request.run_id.as_ref().map(|r| r.as_str().to_owned()))
        .bind(&request.kind)
        .bind(request.payload.to_string())
        .bind(request.state.as_str())
        .bind(request.values.as_ref().map(ToString::to_string))
        .bind(request.decided_at)
        .bind(request.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &RequestId,
    ) -> Result<Option<Request>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM requests WHERE id = ? AND workspace_id = ?"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_request).transpose()
    }

    async fn list_by_state(
        &self,
        workspace_id: &WorkspaceId,
        state: RequestState,
        kind: Option<&str>,
    ) -> Result<Vec<Request>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM requests \
             WHERE workspace_id = ? AND state = ? AND (? IS NULL OR kind = ?) \
             ORDER BY created_at DESC, id DESC"
        ))
        .bind(workspace_id.as_str())
        .bind(state.as_str())
        .bind(kind)
        .bind(kind)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_request).collect()
    }

    async fn decide(
        &self,
        workspace_id: &WorkspaceId,
        id: &RequestId,
        state: RequestState,
        values: Option<serde_json::Value>,
        decided_at: UnixMillis,
    ) -> Result<DecideOutcome, StoreError> {
        let updated = sqlx::query(
            "UPDATE requests SET state = ?, submitted_values = ?, decided_at = ? \
             WHERE id = ? AND workspace_id = ? AND state = 'pending'",
        )
        .bind(state.as_str())
        .bind(values.as_ref().map(ToString::to_string))
        .bind(decided_at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        let current = self
            .get(workspace_id, id)
            .await?
            .ok_or_else(|| StoreError::Corrupt(format!("request {id} vanished")))?;
        if updated.rows_affected() == 1 {
            Ok(DecideOutcome::Decided(current))
        } else {
            Ok(DecideOutcome::NotPending(current))
        }
    }

    async fn expire_pending_for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        decided_at: UnixMillis,
    ) -> Result<u64, StoreError> {
        let updated = sqlx::query(
            "UPDATE requests SET state = 'expired', decided_at = ? \
             WHERE run_id = ? AND workspace_id = ? AND state = 'pending'",
        )
        .bind(decided_at)
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected())
    }
}
