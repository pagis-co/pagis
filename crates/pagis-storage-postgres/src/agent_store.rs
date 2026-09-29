use async_trait::async_trait;
use pagis_core::{Agent, AgentId, AgentStore, StoreError, WorkspaceId};
use sqlx::{PgPool, Row};

use crate::db_err;

#[derive(Clone)]
pub struct PostgresAgentStore {
    pool: PgPool,
}

impl PostgresAgentStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

const COLUMNS: &str = "id, workspace_id, name, job, description, personality, model_alias, \
     voice, standing_brief, avatar, status, created_at, updated_at";

fn row_to_agent(row: &sqlx::postgres::PgRow) -> Result<Agent, StoreError> {
    let status: String = row.get("status");
    Ok(Agent {
        id: AgentId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        name: row.get("name"),
        job: row.get("job"),
        description: row.get("description"),
        personality: row.get("personality"),
        model_alias: row.get("model_alias"),
        avatar: serde_json::from_str(&row.get::<String, _>("avatar"))
            .map_err(|error| StoreError::Corrupt(error.to_string()))?,
        voice: row.get("voice"),
        standing_brief: row.get("standing_brief"),
        status: status.parse().map_err(StoreError::Corrupt)?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

#[async_trait]
impl AgentStore for PostgresAgentStore {
    async fn create(&self, agent: &Agent) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO agents (id, workspace_id, name, job, description, personality, \
             model_alias, voice, standing_brief, avatar, status, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(agent.id.as_str())
        .bind(agent.workspace_id.as_str())
        .bind(&agent.name)
        .bind(&agent.job)
        .bind(&agent.description)
        .bind(&agent.personality)
        .bind(&agent.model_alias)
        .bind(&agent.voice)
        .bind(&agent.standing_brief)
        .bind(
            serde_json::to_string(&agent.avatar)
                .map_err(|error| StoreError::Corrupt(error.to_string()))?,
        )
        .bind(agent.status.as_str())
        .bind(agent.created_at)
        .bind(agent.updated_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
    ) -> Result<Option<Agent>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM agents WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_agent).transpose()
    }

    async fn update(&self, agent: &Agent) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE agents SET name = $1, job = $2, description = $3, personality = $4, \
             voice = $5, standing_brief = $6, status = $7, updated_at = $8 \
             WHERE id = $9 AND workspace_id = $10",
        )
        .bind(&agent.name)
        .bind(&agent.job)
        .bind(&agent.description)
        .bind(&agent.personality)
        .bind(&agent.voice)
        .bind(&agent.standing_brief)
        .bind(agent.status.as_str())
        .bind(agent.updated_at)
        .bind(agent.id.as_str())
        .bind(agent.workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn update_avatar(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
        avatar: &pagis_core::AvatarAppearance,
        updated_at: i64,
    ) -> Result<bool, StoreError> {
        let encoded = serde_json::to_string(avatar)
            .map_err(|error| StoreError::Corrupt(error.to_string()))?;
        let result = sqlx::query(
            "UPDATE agents SET avatar = $1, updated_at = $2 WHERE id = $3 AND workspace_id = $4",
        )
        .bind(encoded)
        .bind(updated_at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }

    async fn list_by_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<Agent>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM agents WHERE workspace_id = $1 ORDER BY id"
        ))
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_agent).collect()
    }
}
