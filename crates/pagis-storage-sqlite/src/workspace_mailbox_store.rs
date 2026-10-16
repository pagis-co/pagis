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
fn record(row: &sqlx::sqlite::SqliteRow) -> WorkspaceMailbox {
    WorkspaceMailbox {
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        connection_id: ConnectionId::from(row.get::<String, _>("connection_id")),
        address: row.get("address"),
        created_at: row.get("created_at"),
    }
}
#[async_trait]
impl WorkspaceMailboxRecordStore for SqliteWorkspaceMailboxStore {
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<WorkspaceMailbox>, StoreError> {
        let row = sqlx::query("SELECT workspace_id, connection_id, address, created_at FROM workspace_mailboxes WHERE workspace_id = ?")
            .bind(workspace_id.as_str()).fetch_optional(&self.pool).await.map_err(db_err)?;
        Ok(row.as_ref().map(record))
    }
    async fn create(&self, mailbox: &WorkspaceMailbox) -> Result<(), StoreError> {
        sqlx::query("INSERT INTO workspace_mailboxes (workspace_id, connection_id, address, created_at) VALUES (?, ?, ?, ?)")
            .bind(mailbox.workspace_id.as_str()).bind(mailbox.connection_id.as_str())
            .bind(&mailbox.address).bind(mailbox.created_at)
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
