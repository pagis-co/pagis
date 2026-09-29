use async_trait::async_trait;
use pagis_core::{
    CapabilitySnapshotRecord, CapabilitySnapshotStore, RunId, StoreError, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

#[derive(Clone)]
pub struct SqliteCapabilitySnapshotStore {
    pool: SqlitePool,
}

impl SqliteCapabilitySnapshotStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// One stored snapshot by its content address. The table holds no
    /// workspace: the id is the SHA-256 of the content, so the same
    /// capability set is one row for every Workspace and there is no
    /// tenant to name. Only the write's own read-back calls it.
    async fn stored(&self, id: &str) -> Result<Option<CapabilitySnapshotRecord>, StoreError> {
        let row = sqlx::query(
            "SELECT id, hash, content, created_at FROM capability_snapshots WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_snapshot).transpose()
    }
}

fn row_to_snapshot(row: &sqlx::sqlite::SqliteRow) -> Result<CapabilitySnapshotRecord, StoreError> {
    let content: String = row.get("content");
    Ok(CapabilitySnapshotRecord {
        id: row.get("id"),
        hash: row.get("hash"),
        content: serde_json::from_str(&content)
            .map_err(|error| StoreError::Corrupt(format!("capability snapshot: {error}")))?,
        created_at: row.get("created_at"),
    })
}

#[async_trait]
impl CapabilitySnapshotStore for SqliteCapabilitySnapshotStore {
    async fn put(&self, snapshot: &CapabilitySnapshotRecord) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO capability_snapshots (id, hash, content, created_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(&snapshot.id)
        .bind(&snapshot.hash)
        .bind(snapshot.content.to_string())
        .bind(snapshot.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        let stored = self
            .stored(&snapshot.id)
            .await?
            .ok_or_else(|| StoreError::Corrupt(format!("missing snapshot {}", snapshot.id)))?;
        if stored.hash != snapshot.hash || stored.content != snapshot.content {
            return Err(StoreError::Corrupt(format!(
                "snapshot {} conflicts with stored content",
                snapshot.id
            )));
        }
        Ok(())
    }

    async fn bind_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        snapshot_id: &str,
    ) -> Result<(), StoreError> {
        // `run_capability_snapshots` carries no workspace of its own, so
        // the binding is written and read through the Run that owns it.
        sqlx::query(
            "INSERT INTO run_capability_snapshots (run_id, snapshot_id) \
             SELECT id, ? FROM runs WHERE id = ? AND workspace_id = ? \
             ON CONFLICT(run_id) DO UPDATE SET snapshot_id = excluded.snapshot_id \
             WHERE snapshot_id = excluded.snapshot_id",
        )
        .bind(snapshot_id)
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        let bound: String = sqlx::query_scalar(
            "SELECT b.snapshot_id FROM run_capability_snapshots b \
             JOIN runs r ON r.id = b.run_id \
             WHERE b.run_id = ? AND r.workspace_id = ?",
        )
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        if bound != snapshot_id {
            return Err(StoreError::Corrupt(format!(
                "run {run_id} already has capability snapshot {bound}"
            )));
        }
        Ok(())
    }

    async fn for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Option<CapabilitySnapshotRecord>, StoreError> {
        let row = sqlx::query(
            "SELECT s.id, s.hash, s.content, s.created_at FROM capability_snapshots s \
             JOIN run_capability_snapshots b ON b.snapshot_id = s.id \
             JOIN runs r ON r.id = b.run_id \
             WHERE b.run_id = ? AND r.workspace_id = ?",
        )
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_snapshot).transpose()
    }
}
