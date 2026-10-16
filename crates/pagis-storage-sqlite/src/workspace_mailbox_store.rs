//! The Workspace Mailbox record and its Address Ledger reservation.
use crate::{db_err, unique_violation};
use async_trait::async_trait;
use pagis_core::{
    ConnectionId, StoreError, WorkspaceId, WorkspaceMailbox, WorkspaceMailboxRecordStore,
};
use sqlx::{Row, SqlitePool};

#[derive(Clone)]
pub struct SqliteWorkspaceMailboxStore {
    pool: SqlitePool,
}
impl SqliteWorkspaceMailboxStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}
fn record(row: &sqlx::sqlite::SqliteRow) -> Result<WorkspaceMailbox, StoreError> {
    Ok(WorkspaceMailbox {
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        connection_id: ConnectionId::from(row.get::<String, _>("connection_id")),
        address: row.get("address"),
        visibility: row.get::<String, _>("visibility").parse()?,
        spam_mode: row.get::<String, _>("spam_mode").parse()?,
        outgoing_cap: row.get::<i64, _>("outgoing_cap") as u32,
        created_at: row.get("created_at"),
    })
}
#[async_trait]
impl WorkspaceMailboxRecordStore for SqliteWorkspaceMailboxStore {
    async fn set_visibility(
        &self,
        workspace: &WorkspaceId,
        visibility: pagis_core::MailVisibility,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE workspace_mailboxes SET visibility = ? WHERE workspace_id = ?")
            .bind(visibility.as_str())
            .bind(workspace.as_str())
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
    async fn escalate(
        &self,
        workspace: &WorkspaceId,
        message: &str,
        agent: &pagis_core::AgentId,
    ) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO workspace_mail_escalations (workspace_id, message_id, agent_id) VALUES (?, ?, ?) ON CONFLICT (workspace_id, message_id) DO NOTHING")
            .bind(workspace.as_str()).bind(message).bind(agent.as_str()).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn is_escalated(
        &self,
        workspace: &WorkspaceId,
        message: &str,
    ) -> Result<bool, StoreError> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspace_mail_escalations WHERE workspace_id = ? AND message_id = ?)")
            .bind(workspace.as_str()).bind(message).fetch_one(&self.pool).await.map_err(db_err)
    }
    async fn set_spam_mode(
        &self,
        workspace: &WorkspaceId,
        mode: pagis_core::MailScoringMode,
    ) -> Result<(), StoreError> {
        let result =
            sqlx::query("UPDATE workspace_mailboxes SET spam_mode = ? WHERE workspace_id = ?")
                .bind(mode.as_str())
                .bind(workspace.as_str())
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::Conflict("mail record does not exist".into()));
        }
        Ok(())
    }
    async fn reserve_send(&self, workspace: &WorkspaceId, day: i64) -> Result<bool, StoreError> {
        let result = sqlx::query("UPDATE workspace_mailboxes SET sends_today = CASE WHEN send_day = ? THEN sends_today + 1 ELSE 1 END, send_day = ? WHERE workspace_id = ? AND (send_day <> ? OR sends_today < outgoing_cap)")
            .bind(day).bind(day).bind(workspace.as_str()).bind(day).execute(&self.pool).await.map_err(db_err)?;
        Ok(result.rows_affected() == 1)
    }
    async fn release_send(&self, workspace: &WorkspaceId, day: i64) -> Result<(), StoreError> {
        sqlx::query("UPDATE workspace_mailboxes SET sends_today = sends_today - 1 WHERE workspace_id = ? AND send_day = ? AND sends_today > 0")
            .bind(workspace.as_str()).bind(day).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<WorkspaceMailbox>, StoreError> {
        let row = sqlx::query("SELECT workspace_id, connection_id, address, outgoing_cap, spam_mode, visibility, created_at FROM workspace_mailboxes WHERE workspace_id = ?")
            .bind(workspace_id.as_str()).fetch_optional(&self.pool).await.map_err(db_err)?;
        row.as_ref().map(record).transpose()
    }
    async fn create(&self, mailbox: &WorkspaceMailbox) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO workspace_mailboxes (workspace_id, connection_id, address, created_at, outgoing_cap, spam_mode, visibility) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(mailbox.workspace_id.as_str()).bind(mailbox.connection_id.as_str())
            .bind(&mailbox.address).bind(mailbox.created_at).bind(i64::from(mailbox.outgoing_cap)).bind(mailbox.spam_mode.as_str()).bind(mailbox.visibility.as_str())
            .execute(&self.pool).await.map_err(|error| if unique_violation(&error) {
                StoreError::Conflict("that Workspace or address already has a mailbox".into())
            } else { db_err(error) })?;
        Ok(())
    }
    async fn drop_reservation(
        &self,
        workspace_id: &WorkspaceId,
        address: &str,
    ) -> Result<bool, StoreError> {
        let result =
            sqlx::query("DELETE FROM workspace_mailboxes WHERE workspace_id = ? AND address = ?")
                .bind(workspace_id.as_str())
                .bind(address)
                .execute(&self.pool)
                .await
                .map_err(db_err)?;
        Ok(result.rows_affected() > 0)
    }
    async fn count_for_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<u32, StoreError> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS count FROM workspace_mailboxes WHERE workspace_id = ? AND connection_id = ?",
        )
        .bind(workspace_id.as_str())
        .bind(connection_id.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.get::<i64, _>("count") as u32)
    }
}
