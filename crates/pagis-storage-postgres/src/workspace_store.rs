use async_trait::async_trait;
use pagis_core::{
    AgentId, ScheduleId, StoreError, UnixMillis, UserId, Workspace, WorkspaceId, WorkspaceStore,
};
use sqlx::{PgPool, Row};

use crate::db_err;

#[derive(Clone)]
pub struct PostgresWorkspaceStore {
    pool: PgPool,
}

impl PostgresWorkspaceStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_workspace(row: &sqlx::postgres::PgRow) -> Workspace {
    Workspace {
        id: WorkspaceId::from(row.get::<String, _>("id")),
        user_id: UserId::from(row.get::<String, _>("user_id")),
        name: row.get("name"),
        timezone: row.get("timezone"),
        created_at: row.get("created_at"),
        onboarded_at: row.get("onboarded_at"),
        chief_of_staff_agent_id: row
            .get::<Option<String>, _>("chief_of_staff_agent_id")
            .map(AgentId::from),
        report_schedule_id: row
            .get::<Option<String>, _>("report_schedule_id")
            .map(ScheduleId::from),
    }
}

/// The columns [`row_to_workspace`] reads, in one place.
const COLUMNS: &str = "id, user_id, name, timezone, created_at, onboarded_at, \
     chief_of_staff_agent_id, report_schedule_id";

#[async_trait]
impl WorkspaceStore for PostgresWorkspaceStore {
    async fn create(&self, workspace: &Workspace) -> Result<(), StoreError> {
        sqlx::query(&format!(
            "INSERT INTO workspaces ({COLUMNS}) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
        ))
        .bind(workspace.id.as_str())
        .bind(workspace.user_id.as_str())
        .bind(&workspace.name)
        .bind(&workspace.timezone)
        .bind(workspace.created_at)
        .bind(workspace.onboarded_at)
        .bind(
            workspace
                .chief_of_staff_agent_id
                .as_ref()
                .map(AgentId::as_str),
        )
        .bind(
            workspace
                .report_schedule_id
                .as_ref()
                .map(ScheduleId::as_str),
        )
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn get(&self, id: &WorkspaceId) -> Result<Option<Workspace>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM workspaces WHERE id = $1 AND user_id IS NOT NULL"
        ))
        .bind(id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.as_ref().map(row_to_workspace))
    }

    async fn for_user(&self, user_id: &UserId) -> Result<Option<Workspace>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM workspaces WHERE user_id = $1 ORDER BY id LIMIT 1"
        ))
        .bind(user_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.as_ref().map(row_to_workspace))
    }

    async fn list(&self) -> Result<Vec<Workspace>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {COLUMNS} FROM workspaces WHERE user_id IS NOT NULL ORDER BY id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.iter().map(row_to_workspace).collect())
    }

    async fn set_onboarded(&self, id: &WorkspaceId, at: UnixMillis) -> Result<(), StoreError> {
        sqlx::query(
            "UPDATE workspaces SET onboarded_at = $1 WHERE id = $2 AND onboarded_at IS NULL",
        )
        .bind(at)
        .bind(id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn set_timezone(&self, id: &WorkspaceId, timezone: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE workspaces SET timezone = $1 WHERE id = $2")
            .bind(timezone)
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn set_chief_of_staff(
        &self,
        id: &WorkspaceId,
        agent_id: Option<&AgentId>,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE workspaces SET chief_of_staff_agent_id = $1 WHERE id = $2")
            .bind(agent_id.map(AgentId::as_str))
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn set_report_schedule(
        &self,
        id: &WorkspaceId,
        schedule_id: Option<&ScheduleId>,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE workspaces SET report_schedule_id = $1 WHERE id = $2")
            .bind(schedule_id.map(ScheduleId::as_str))
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}
