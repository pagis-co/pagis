//! Shared Workspace membership. Workspace locks serialize Manager changes.
use crate::db_err;
use async_trait::async_trait;
use pagis_core::{
    Author, StoreError, UserId, WorkspaceId, WorkspaceMember, WorkspaceMemberStore, WorkspaceRole,
};
use sqlx::{Row, SqlitePool};

#[derive(Clone)]
pub struct SqliteWorkspaceMemberStore {
    pool: SqlitePool,
}
impl SqliteWorkspaceMemberStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}
fn read(row: &sqlx::sqlite::SqliteRow) -> Result<WorkspaceMember, StoreError> {
    Ok(WorkspaceMember {
        workspace_id: row.get::<String, _>("workspace_id").into(),
        person_id: row.get::<String, _>("person_id").into(),
        role: row
            .get::<String, _>("role")
            .parse()
            .map_err(StoreError::Corrupt)?,
        author: Author::from_columns(
            &row.get::<String, _>("author_kind"),
            row.get("author_person_id"),
            row.get("author_agent_id"),
        )
        .map_err(StoreError::Corrupt)?,
    })
}
#[async_trait]
impl WorkspaceMemberStore for SqliteWorkspaceMemberStore {
    async fn create_workspace(
        &self,
        workspace: &pagis_core::Workspace,
        manager: &WorkspaceMember,
    ) -> Result<(), StoreError> {
        if workspace.kind != pagis_core::WorkspaceKind::Shared
            || workspace.user_id.is_some()
            || workspace.id != manager.workspace_id
            || manager.role != WorkspaceRole::Manager
        {
            return Err(StoreError::Conflict(
                "a Shared Workspace needs its first Manager".into(),
            ));
        }
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        crate::workspace_store::insert_workspace(&mut *tx, workspace).await?;
        insert_member(&mut *tx, manager).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
    async fn list(&self, workspace: &WorkspaceId) -> Result<Vec<WorkspaceMember>, StoreError> {
        sqlx::query("SELECT * FROM workspace_members WHERE workspace_id = ?1 ORDER BY person_id")
            .bind(workspace.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?
            .iter()
            .map(read)
            .collect()
    }
    async fn get(
        &self,
        workspace: &WorkspaceId,
        person: &UserId,
    ) -> Result<Option<WorkspaceMember>, StoreError> {
        sqlx::query("SELECT * FROM workspace_members WHERE workspace_id = ?1 AND person_id = ?2")
            .bind(workspace.as_str())
            .bind(person.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
            .as_ref()
            .map(read)
            .transpose()
    }
    async fn add(&self, member: &WorkspaceMember) -> Result<(), StoreError> {
        insert_member(&self.pool, member).await
    }
    async fn change(&self, member: &WorkspaceMember) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        // Lock the Workspace row first, so two changes cannot each see
        // another Manager and remove the last one together.
        sqlx::query("UPDATE workspaces SET name = name WHERE id = ?1")
            .bind(member.workspace_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        let managers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workspace_members WHERE workspace_id = ?1 AND role = 'manager'",
        )
        .bind(member.workspace_id.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM workspace_members WHERE workspace_id = ?1 AND person_id = ?2",
        )
        .bind(member.workspace_id.as_str())
        .bind(member.person_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        if role.as_deref() == Some("manager")
            && managers == 1
            && member.role != WorkspaceRole::Manager
        {
            return Err(StoreError::Conflict(
                "the last Manager cannot be demoted".into(),
            ));
        }
        let result = sqlx::query("UPDATE workspace_members SET role = ?3, author_kind = ?4, author_person_id = ?5, author_agent_id = ?6 WHERE workspace_id = ?1 AND person_id = ?2")
            .bind(member.workspace_id.as_str()).bind(member.person_id.as_str()).bind(member.role.as_str()).bind(member.author.kind()).bind(member.author.person_id().map(UserId::as_str)).bind(member.author.agent_id().map(pagis_core::AgentId::as_str)).execute(&mut *tx).await.map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }
    async fn remove(&self, workspace: &WorkspaceId, person: &UserId) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        // Lock the Workspace row first, so two changes cannot each see
        // another Manager and remove the last one together.
        sqlx::query("UPDATE workspaces SET name = name WHERE id = ?1")
            .bind(workspace.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        let managers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workspace_members WHERE workspace_id = ?1 AND role = 'manager'",
        )
        .bind(workspace.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM workspace_members WHERE workspace_id = ?1 AND person_id = ?2",
        )
        .bind(workspace.as_str())
        .bind(person.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        if role.as_deref() == Some("manager") && managers == 1 {
            return Err(StoreError::Conflict(
                "the last Manager cannot be removed".into(),
            ));
        }
        let result =
            sqlx::query("DELETE FROM workspace_members WHERE workspace_id = ?1 AND person_id = ?2")
                .bind(workspace.as_str())
                .bind(person.as_str())
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }
}

async fn insert_member<'e, E>(executor: E, member: &WorkspaceMember) -> Result<(), StoreError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let result = sqlx::query("INSERT INTO workspace_members (workspace_id, person_id, role, author_kind, author_person_id, author_agent_id) SELECT ?1, ?2, ?3, ?4, ?5, ?6 WHERE EXISTS (SELECT 1 FROM workspaces WHERE id = ?1 AND kind = 'shared' AND user_id IS NULL AND id NOT IN (SELECT workspace_id FROM orgs)) AND EXISTS (SELECT 1 FROM users WHERE id = ?2 AND disabled_at IS NULL)")
            .bind(member.workspace_id.as_str()).bind(member.person_id.as_str()).bind(member.role.as_str())
            .bind(member.author.kind()).bind(member.author.person_id().map(UserId::as_str)).bind(member.author.agent_id().map(pagis_core::AgentId::as_str))
            .execute(executor).await.map_err(db_err)?;
    if result.rows_affected() == 0 {
        return Err(StoreError::Conflict(
            "membership needs a Shared Workspace and an active Person".into(),
        ));
    }
    Ok(())
}
