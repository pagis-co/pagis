use async_trait::async_trait;
use pagis_core::{AgentId, Grant, GrantId, GrantStore, StoreError, UnixMillis, WorkspaceId};
use sqlx::{PgPool, Row};

use crate::db_err;

const COLUMNS: &str = "id, workspace_id, agent_id, resource_kind, resource_id, scope, revision, \
                       created_at, revoked_at";

#[derive(Clone)]
pub struct PostgresGrantStore {
    pool: PgPool,
}

impl PostgresGrantStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_grant(row: &sqlx::postgres::PgRow) -> Result<Grant, StoreError> {
    let scope: String = row.get("scope");
    Ok(Grant {
        id: GrantId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        resource_kind: row.get("resource_kind"),
        resource_id: row.get("resource_id"),
        scope: serde_json::from_str(&scope)
            .map_err(|e| StoreError::Corrupt(format!("grant scope: {e}")))?,
        revision: row.get("revision"),
        created_at: row.get("created_at"),
        revoked_at: row.get("revoked_at"),
    })
}

#[async_trait]
impl GrantStore for PostgresGrantStore {
    async fn create(&self, grant: &Grant) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO grants (id, workspace_id, agent_id, resource_kind, resource_id, scope, \
             revision, created_at, revoked_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(grant.id.as_str())
        .bind(grant.workspace_id.as_str())
        .bind(grant.agent_id.as_str())
        .bind(&grant.resource_kind)
        .bind(grant.resource_id.as_deref())
        .bind(grant.scope.to_string())
        .bind(grant.revision)
        .bind(grant.created_at)
        .bind(grant.revoked_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
    ) -> Result<Option<Grant>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM grants WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_grant).transpose()
    }

    async fn list_live(&self, workspace_id: &WorkspaceId) -> Result<Vec<Grant>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM grants \
             WHERE workspace_id = $1 AND revoked_at IS NULL ORDER BY id"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_grant).collect()
    }

    async fn live_grant(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        resource_kind: &str,
    ) -> Result<Option<Grant>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM grants \
             WHERE agent_id = $1 AND workspace_id = $2 AND resource_kind = $3 \
             AND revoked_at IS NULL"
        ))
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .bind(resource_kind)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_grant).transpose()
    }

    async fn list_live_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Vec<Grant>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM grants \
             WHERE agent_id = $1 AND workspace_id = $2 AND revoked_at IS NULL ORDER BY id"
        ))
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_grant).collect()
    }

    async fn live_for_resource(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        resource_kind: &str,
        resource_id: &str,
    ) -> Result<Option<Grant>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM grants \
             WHERE agent_id = $1 AND workspace_id = $2 AND resource_kind = $3 \
             AND resource_id = $4 AND revoked_at IS NULL"
        ))
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .bind(resource_kind)
        .bind(resource_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_grant).transpose()
    }

    async fn set_scope(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
        scope: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE grants SET scope = $1, revision = revision + 1 \
                 WHERE id = $2 AND workspace_id = $3 AND revoked_at IS NULL",
        )
        .bind(scope.to_string())
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() == 1)
    }

    async fn revoke(
        &self,
        workspace_id: &WorkspaceId,
        id: &GrantId,
        revoked_at: UnixMillis,
    ) -> Result<bool, StoreError> {
        let updated = sqlx::query(
            "UPDATE grants SET revoked_at = $1 \
             WHERE id = $2 AND workspace_id = $3 AND revoked_at IS NULL",
        )
        .bind(revoked_at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(updated.rows_affected() == 1)
    }
}
