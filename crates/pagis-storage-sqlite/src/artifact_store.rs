use async_trait::async_trait;
use pagis_core::{
    AgentId, Artifact, ArtifactId, ArtifactKind, ArtifactOutcome, ArtifactStore, RunId, StoreError,
    UnixMillis, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

const COLUMNS: &str = "id, workspace_id, creator_agent_id, run_id, kind, filename, mime, \
     size_bytes, sha256, storage_key, created_at";

#[derive(Clone)]
pub struct SqliteArtifactStore {
    pool: SqlitePool,
}

impl SqliteArtifactStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn get_by_sha(
        &self,
        workspace_id: &WorkspaceId,
        sha256: &str,
    ) -> Result<Option<Artifact>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM artifacts WHERE workspace_id = ? AND sha256 = ?"
        ))
        .bind(workspace_id.as_str())
        .bind(sha256)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_artifact).transpose()
    }
}

fn row_to_artifact(row: &sqlx::sqlite::SqliteRow) -> Result<Artifact, StoreError> {
    Ok(Artifact {
        id: ArtifactId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        creator_agent_id: row
            .get::<Option<String>, _>("creator_agent_id")
            .map(AgentId::from),
        run_id: row.get::<Option<String>, _>("run_id").map(RunId::from),
        kind: ArtifactKind::parse(&row.get::<String, _>("kind")).ok_or_else(|| {
            StoreError::Corrupt(format!(
                "unknown artifact kind {:?}",
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
impl ArtifactStore for SqliteArtifactStore {
    async fn insert(&self, artifact: &Artifact) -> Result<ArtifactOutcome, StoreError> {
        let result = sqlx::query(
            "INSERT INTO artifacts (id, workspace_id, creator_agent_id, run_id, kind, \
             filename, mime, size_bytes, sha256, storage_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (workspace_id, sha256) DO NOTHING",
        )
        .bind(artifact.id.as_str())
        .bind(artifact.workspace_id.as_str())
        .bind(
            artifact
                .creator_agent_id
                .as_ref()
                .map(|a| a.as_str().to_owned()),
        )
        .bind(artifact.run_id.as_ref().map(|r| r.as_str().to_owned()))
        .bind(artifact.kind.as_str())
        .bind(&artifact.filename)
        .bind(&artifact.mime)
        .bind(artifact.size_bytes)
        .bind(&artifact.sha256)
        .bind(&artifact.storage_key)
        .bind(artifact.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;

        if result.rows_affected() == 1 {
            return Ok(ArtifactOutcome::Created(artifact.clone()));
        }
        let original = self
            .get_by_sha(&artifact.workspace_id, &artifact.sha256)
            .await?
            .ok_or_else(|| {
                StoreError::Corrupt("deduplicated artifact not found by sha256".to_string())
            })?;
        Ok(ArtifactOutcome::Deduplicated(original))
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ArtifactId,
    ) -> Result<Option<Artifact>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM artifacts WHERE id = ? AND workspace_id = ?"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_artifact).transpose()
    }

    async fn expired_before(
        &self,
        workspace_id: &WorkspaceId,
        kind: ArtifactKind,
        cutoff: UnixMillis,
        limit: u32,
    ) -> Result<Vec<Artifact>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM artifacts \
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
        rows.iter().map(row_to_artifact).collect()
    }

    async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &ArtifactId,
    ) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM artifacts WHERE id = ? AND workspace_id = ?")
            .bind(id.as_str())
            .bind(workspace_id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }
}
