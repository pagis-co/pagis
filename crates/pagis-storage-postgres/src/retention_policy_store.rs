use async_trait::async_trait;
use pagis_core::{
    ArtifactKind, RetentionPolicy, RetentionPolicyStore, StoreError, UnixMillis, WorkspaceId,
};
use sqlx::{PgPool, Row};

use crate::db_err;

#[derive(Clone)]
pub struct PostgresRetentionPolicyStore {
    pool: PgPool,
}

impl PostgresRetentionPolicyStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RetentionPolicyStore for PostgresRetentionPolicyStore {
    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<RetentionPolicy>, StoreError> {
        let rows = sqlx::query(
            "SELECT kind, retain_days FROM artifact_retention_policies WHERE workspace_id = $1",
        )
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let mut policies = Vec::with_capacity(ArtifactKind::ALL.len());
        for kind in ArtifactKind::ALL {
            let retain_days = rows
                .iter()
                .find(|row| row.get::<String, _>("kind") == kind.as_str())
                .map(|row| row.get::<i64, _>("retain_days"));
            policies.push(RetentionPolicy { kind, retain_days });
        }
        Ok(policies)
    }

    async fn set(
        &self,
        workspace_id: &WorkspaceId,
        kind: ArtifactKind,
        retain_days: Option<i64>,
        now: UnixMillis,
    ) -> Result<(), StoreError> {
        // "Keep for ever" is the absence of a row, so the reset path is
        // a delete and not a stored sentinel.
        let Some(days) = retain_days else {
            sqlx::query(
                "DELETE FROM artifact_retention_policies WHERE workspace_id = $1 AND kind = $2",
            )
            .bind(workspace_id.as_str())
            .bind(kind.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO artifact_retention_policies (workspace_id, kind, retain_days, updated_at) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (workspace_id, kind) \
             DO UPDATE SET retain_days = excluded.retain_days, updated_at = excluded.updated_at",
        )
        .bind(workspace_id.as_str())
        .bind(kind.as_str())
        .bind(days)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }
}
