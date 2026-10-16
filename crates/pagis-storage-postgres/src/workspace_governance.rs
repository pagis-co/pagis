//! Shared Workspace Limits and append-only Agent Versions.
use crate::db_err;
use async_trait::async_trait;
use pagis_core::{
    AgentId, AgentVersion, AuditEntry, SharedWorkspaceLimits, StoreError, WorkspaceGovernanceStore,
    WorkspaceId,
};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PostgresWorkspaceGovernanceStore {
    pool: PgPool,
}
impl PostgresWorkspaceGovernanceStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
fn encode(value: &impl serde::Serialize) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Corrupt(e.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, StoreError> {
    serde_json::from_str(value).map_err(|e| StoreError::Corrupt(e.to_string()))
}
#[async_trait]
impl WorkspaceGovernanceStore for PostgresWorkspaceGovernanceStore {
    async fn limits(&self, workspace: &WorkspaceId) -> Result<SharedWorkspaceLimits, StoreError> {
        let row = sqlx::query("SELECT permitted_plugins, permitted_connections, max_mail_visibility FROM workspace_limits WHERE workspace_id=$1")
            .bind(workspace.as_str()).fetch_optional(&self.pool).await.map_err(db_err)?;
        let Some(row) = row else {
            return Ok(SharedWorkspaceLimits::default());
        };
        Ok(SharedWorkspaceLimits {
            permitted_plugins: row
                .get::<Option<String>, _>("permitted_plugins")
                .map(|s| decode(&s))
                .transpose()?,
            permitted_connections: row
                .get::<Option<String>, _>("permitted_connections")
                .map(|s| decode(&s))
                .transpose()?,
            max_mail_visibility: row
                .get::<Option<String>, _>("max_mail_visibility")
                .map(|s| decode(&s))
                .transpose()?
                .unwrap_or_default(),
        })
    }
    async fn set_limits(
        &self,
        limits: &SharedWorkspaceLimits,
        entry: &AuditEntry,
    ) -> Result<(), StoreError> {
        limits.validate()?;
        if entry.action != "workspace.limits_changed"
            || !matches!(entry.actor, pagis_core::Author::Person(_))
        {
            return Err(StoreError::Conflict(
                "Shared Workspace Limits need a Person author and the correct action.".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let shared: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id=$1 AND kind='shared')",
        )
        .bind(entry.subject_workspace_id.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        if !shared {
            return Err(StoreError::Conflict(
                "Shared Workspace Limits need a Shared Workspace.".into(),
            ));
        }
        sqlx::query("INSERT INTO workspace_limits (workspace_id, permitted_plugins, permitted_connections, max_mail_visibility) VALUES ($1,$2,$3,$4) ON CONFLICT(workspace_id) DO UPDATE SET permitted_plugins=excluded.permitted_plugins, permitted_connections=excluded.permitted_connections, max_mail_visibility=excluded.max_mail_visibility")
            .bind(entry.subject_workspace_id.as_str()).bind(limits.permitted_plugins.as_ref().map(encode).transpose()?).bind(limits.permitted_connections.as_ref().map(encode).transpose()?).bind(encode(&limits.max_mail_visibility)?)
            .execute(&mut *tx).await.map_err(db_err)?;
        crate::audit_log::append(&mut tx, entry).await?;
        tx.commit().await.map_err(db_err)
    }
    async fn record_version(
        &self,
        agent: &AgentId,
        configuration: &serde_json::Value,
        entry: &AuditEntry,
    ) -> Result<AgentVersion, StoreError> {
        if entry.action != "agent.changed" || !matches!(entry.actor, pagis_core::Author::Person(_))
        {
            return Err(StoreError::Conflict(
                "An Agent Version needs a Person author and the correct action.".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let exists =
            sqlx::query("SELECT id FROM agents WHERE workspace_id=$1 AND id=$2 FOR UPDATE")
                .bind(entry.subject_workspace_id.as_str())
                .bind(agent.as_str())
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;
        if exists.is_none() {
            return Err(StoreError::Conflict(
                "The Agent is missing from the Workspace.".into(),
            ));
        }
        let version: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version),0)+1 FROM agent_versions WHERE workspace_id=$1 AND agent_id=$2")
            .bind(entry.subject_workspace_id.as_str()).bind(agent.as_str()).fetch_one(&mut *tx).await.map_err(db_err)?;
        sqlx::query("INSERT INTO agent_versions (workspace_id, agent_id, version, author, configuration, at) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(entry.subject_workspace_id.as_str()).bind(agent.as_str()).bind(version).bind(encode(&entry.actor)?).bind(encode(configuration)?).bind(entry.at).execute(&mut *tx).await.map_err(db_err)?;
        crate::audit_log::append(&mut tx, entry).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(AgentVersion {
            workspace_id: entry.subject_workspace_id.clone(),
            agent_id: agent.clone(),
            version,
            author: entry.actor.clone(),
            configuration: configuration.clone(),
            at: entry.at,
        })
    }
    async fn versions(
        &self,
        workspace: &WorkspaceId,
        agent: &AgentId,
    ) -> Result<Vec<AgentVersion>, StoreError> {
        let rows = sqlx::query("SELECT * FROM agent_versions WHERE workspace_id=$1 AND agent_id=$2 ORDER BY version DESC")
            .bind(workspace.as_str()).bind(agent.as_str()).fetch_all(&self.pool).await.map_err(db_err)?;
        rows.into_iter()
            .map(|row| {
                Ok(AgentVersion {
                    workspace_id: workspace.clone(),
                    agent_id: agent.clone(),
                    version: row.get("version"),
                    author: decode(row.get("author"))?,
                    configuration: decode(row.get("configuration"))?,
                    at: row.get("at"),
                })
            })
            .collect()
    }
}
