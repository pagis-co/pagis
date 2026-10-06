//! The Model Request Captures of the Runs (ADR-0030).

use async_trait::async_trait;
use pagis_core::{
    ModelRequestCapture, ModelRequestCaptureId, ModelRequestCaptureStore, RunId, StoreError,
    UnixMillis, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

fn row_to_capture(row: &sqlx::sqlite::SqliteRow) -> Result<ModelRequestCapture, StoreError> {
    let json = |column: &str| {
        serde_json::from_str(row.get::<&str, _>(column))
            .map_err(|e| StoreError::Corrupt(format!("model request capture {column}: {e}")))
    };
    Ok(ModelRequestCapture {
        id: ModelRequestCaptureId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        run_id: RunId::from(row.get::<String, _>("run_id")),
        phase: row.get("phase"),
        phase_request: row.get("phase_request"),
        request: json("request")?,
        answer: json("answer")?,
        created_at: row.get("created_at"),
    })
}

#[derive(Clone)]
pub struct SqliteModelRequestCaptureStore {
    pool: SqlitePool,
}

impl SqliteModelRequestCaptureStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ModelRequestCaptureStore for SqliteModelRequestCaptureStore {
    async fn record(&self, capture: &ModelRequestCapture) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO model_request_captures (id, workspace_id, run_id, phase, \
             phase_request, request, answer, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(capture.id.as_str())
        .bind(capture.workspace_id.as_str())
        .bind(capture.run_id.as_str())
        .bind(&capture.phase)
        .bind(capture.phase_request)
        .bind(capture.request.to_string())
        .bind(capture.answer.to_string())
        .bind(capture.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn list_for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Vec<ModelRequestCapture>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, workspace_id, run_id, phase, phase_request, request, answer, created_at \
             FROM model_request_captures WHERE workspace_id = ? AND run_id = ? \
             ORDER BY created_at, phase_request, id",
        )
        .bind(workspace_id.as_str())
        .bind(run_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_capture).collect()
    }

    async fn delete_before(&self, cutoff: UnixMillis) -> Result<u64, StoreError> {
        let done = sqlx::query("DELETE FROM model_request_captures WHERE created_at < ?")
            .bind(cutoff)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(done.rows_affected())
    }

    async fn delete_all(&self) -> Result<u64, StoreError> {
        let done = sqlx::query("DELETE FROM model_request_captures")
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(done.rows_affected())
    }
}
