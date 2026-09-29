use async_trait::async_trait;
use pagis_core::{ModelAlias, ModelAliasId, ModelAliasStore, StoreError, WorkspaceId};
use sqlx::{PgPool, Row};

use crate::db_err;

#[derive(Clone)]
pub struct PostgresModelAliasStore {
    pool: PgPool,
}

impl PostgresModelAliasStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_alias(row: &sqlx::postgres::PgRow) -> Result<ModelAlias, StoreError> {
    let candidates: String = row.get("candidates");
    Ok(ModelAlias {
        id: ModelAliasId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        alias: row.get("alias"),
        candidates: serde_json::from_str(&candidates)
            .map_err(|error| StoreError::Corrupt(format!("model alias candidates: {error}")))?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

const COLUMNS: &str = "id, workspace_id, alias, candidates, created_at, updated_at";

#[async_trait]
impl ModelAliasStore for PostgresModelAliasStore {
    async fn create(&self, model_alias: &ModelAlias) -> Result<(), StoreError> {
        let candidates = serde_json::to_string(&model_alias.candidates)
            .map_err(|error| StoreError::Corrupt(error.to_string()))?;
        sqlx::query(
            "INSERT INTO model_aliases (id, workspace_id, alias, candidates, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(model_alias.id.as_str())
        .bind(model_alias.workspace_id.as_str())
        .bind(&model_alias.alias)
        .bind(candidates)
        .bind(model_alias.created_at)
        .bind(model_alias.updated_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get_by_alias(
        &self,
        workspace_id: &WorkspaceId,
        alias: &str,
    ) -> Result<Option<ModelAlias>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM model_aliases WHERE workspace_id = $1 AND alias = $2"
        ))
        .bind(workspace_id.as_str())
        .bind(alias)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_alias).transpose()
    }

    async fn list(&self, workspace_id: &WorkspaceId) -> Result<Vec<ModelAlias>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM model_aliases WHERE workspace_id = $1 ORDER BY alias"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_alias).collect()
    }

    async fn update_candidates(
        &self,
        workspace_id: &WorkspaceId,
        alias: &str,
        candidates: &[String],
        updated_at: i64,
    ) -> Result<bool, StoreError> {
        let candidates = serde_json::to_string(candidates)
            .map_err(|error| StoreError::Corrupt(error.to_string()))?;
        let result = sqlx::query(
            "UPDATE model_aliases SET candidates = $1, updated_at = $2 \
             WHERE workspace_id = $3 AND alias = $4",
        )
        .bind(candidates)
        .bind(updated_at)
        .bind(workspace_id.as_str())
        .bind(alias)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }

    async fn delete(&self, workspace_id: &WorkspaceId, alias: &str) -> Result<bool, StoreError> {
        let result =
            sqlx::query("DELETE FROM model_aliases WHERE workspace_id = $1 AND alias = $2")
                .bind(workspace_id.as_str())
                .bind(alias)
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }
}
