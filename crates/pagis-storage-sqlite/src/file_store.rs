use async_trait::async_trait;
use pagis_core::{
    AgentId, File, FileId, FileKind, FileOutcome, FileStore, RunId, StoreError, UnixMillis,
    WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

const COLUMNS: &str = "id, workspace_id, creator_agent_id, run_id, kind, filename, mime, \
     size_bytes, sha256, storage_key, created_at";

#[derive(Clone)]
pub struct SqliteFileStore {
    pool: SqlitePool,
}

impl SqliteFileStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn get_by_sha(
        &self,
        workspace_id: &WorkspaceId,
        sha256: &str,
    ) -> Result<Option<File>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM files WHERE workspace_id = ? AND sha256 = ?"
        ))
        .bind(workspace_id.as_str())
        .bind(sha256)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_file).transpose()
    }
}

fn row_to_file(row: &sqlx::sqlite::SqliteRow) -> Result<File, StoreError> {
    Ok(File {
        id: FileId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        creator_agent_id: row
            .get::<Option<String>, _>("creator_agent_id")
            .map(AgentId::from),
        run_id: row.get::<Option<String>, _>("run_id").map(RunId::from),
        kind: FileKind::parse(&row.get::<String, _>("kind")).ok_or_else(|| {
            StoreError::Corrupt(format!(
                "unknown file kind {:?}",
                row.get::<String, _>("kind")
            ))
        })?,
        filename: row.get("filename"),
        mime: row.get("mime"),
        size_bytes: row.get("size_bytes"),
        sha256: row.get("sha256"),
        storage_key: row.get("storage_key"),
        created_at: row.get("created_at"),
    })
}

#[async_trait]
impl FileStore for SqliteFileStore {
    async fn insert(&self, file: &File) -> Result<FileOutcome, StoreError> {
        let result = sqlx::query(
            "INSERT INTO files (id, workspace_id, creator_agent_id, run_id, kind, \
             filename, mime, size_bytes, sha256, storage_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (workspace_id, sha256) DO NOTHING",
        )
        .bind(file.id.as_str())
        .bind(file.workspace_id.as_str())
        .bind(
            file.creator_agent_id
                .as_ref()
                .map(|a| a.as_str().to_owned()),
        )
        .bind(file.run_id.as_ref().map(|r| r.as_str().to_owned()))
        .bind(file.kind.as_str())
        .bind(&file.filename)
        .bind(&file.mime)
        .bind(file.size_bytes)
        .bind(&file.sha256)
        .bind(&file.storage_key)
        .bind(file.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;

        if result.rows_affected() == 1 {
            return Ok(FileOutcome::Created(file.clone()));
        }
        let original = self
            .get_by_sha(&file.workspace_id, &file.sha256)
            .await?
            .ok_or_else(|| {
                StoreError::Corrupt("deduplicated file not found by sha256".to_string())
            })?;
        Ok(FileOutcome::Deduplicated(original))
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &FileId,
    ) -> Result<Option<File>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM files WHERE id = ? AND workspace_id = ?"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_file).transpose()
    }

    async fn expired_before(
        &self,
        workspace_id: &WorkspaceId,
        kind: FileKind,
        cutoff: UnixMillis,
        limit: u32,
    ) -> Result<Vec<File>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM files \
             WHERE workspace_id = ? AND kind = ? AND created_at < ? \
             ORDER BY created_at LIMIT ?"
        ))
        .bind(workspace_id.as_str())
        .bind(kind.as_str())
        .bind(cutoff)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_file).collect()
    }

    async fn delete(&self, workspace_id: &WorkspaceId, id: &FileId) -> Result<bool, StoreError> {
        let mut tx = crate::begin_write(&self.pool).await.map_err(db_err)?;
        crate::audit_log::refuse_delete(&mut tx, workspace_id).await?;
        let result = sqlx::query("DELETE FROM files WHERE id = ? AND workspace_id = ?")
            .bind(id.as_str())
            .bind(workspace_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }
}
