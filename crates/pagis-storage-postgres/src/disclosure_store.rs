//! Profiles and Disclosure Rules, scoped to their owning Workspace.
use crate::db_err;
use async_trait::async_trait;
use pagis_core::{DisclosureRule, DisclosureStore, StoreError, WorkspaceId};
use sqlx::{PgPool, Row};

pub struct PostgresDisclosureStore {
    pool: PgPool,
}
impl PostgresDisclosureStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
fn error(e: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt(e.to_string())
}
#[async_trait]
impl DisclosureStore for PostgresDisclosureStore {
    async fn profile(&self, workspace: &WorkspaceId) -> Result<String, StoreError> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT profile FROM disclosure_profiles WHERE workspace_id = $1",
        )
        .bind(workspace.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        .unwrap_or_default())
    }
    async fn set_profile(&self, workspace: &WorkspaceId, profile: &str) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO disclosure_profiles (workspace_id, profile) VALUES ($1, $2) ON CONFLICT (workspace_id) DO UPDATE SET profile = excluded.profile").bind(workspace.as_str()).bind(profile).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn list(&self, workspace: &WorkspaceId) -> Result<Vec<DisclosureRule>, StoreError> {
        sqlx::query("SELECT asker_workspace_id, category FROM disclosure_rules WHERE workspace_id = $1 ORDER BY asker_workspace_id, category").bind(workspace.as_str()).fetch_all(&self.pool).await.map_err(db_err)?.into_iter().map(|row| {
            let asker: String = row.try_get("asker_workspace_id").map_err(db_err)?;
            let category: String = row.try_get("category").map_err(db_err)?;
            Ok(DisclosureRule { asker_workspace_id: (!asker.is_empty()).then(|| WorkspaceId::from(asker)), category: serde_json::from_value(serde_json::Value::String(category)).map_err(error)? })
        }).collect()
    }
    async fn add(&self, workspace: &WorkspaceId, rule: &DisclosureRule) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO disclosure_rules (workspace_id, asker_workspace_id, category) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING").bind(workspace.as_str()).bind(rule.asker_workspace_id.as_ref().map_or("", |id| id.as_str())).bind(rule.category.as_str()).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn remove(
        &self,
        workspace: &WorkspaceId,
        rule: &DisclosureRule,
    ) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM disclosure_rules WHERE workspace_id = $1 AND asker_workspace_id = $2 AND category = $3").bind(workspace.as_str()).bind(rule.asker_workspace_id.as_ref().map_or("", |id| id.as_str())).bind(rule.category.as_str()).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
}
