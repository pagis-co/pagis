//! The one Workspace Mailbox record. Mail bodies stay at the mail host.
use crate::{ConnectionId, StoreError, UnixMillis, WorkspaceId};
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMailbox {
    pub workspace_id: WorkspaceId,
    pub connection_id: ConnectionId,
    pub address: String,
    pub created_at: UnixMillis,
}

#[async_trait]
pub trait WorkspaceMailboxRecordStore: Send + Sync {
    async fn get(&self, workspace_id: &WorkspaceId)
    -> Result<Option<WorkspaceMailbox>, StoreError>;
    /// Reserve both the Workspace and the address in one write.
    async fn create(&self, mailbox: &WorkspaceMailbox) -> Result<(), StoreError>;
    /// Undo a host create that failed. The address must match the reservation.
    async fn drop_reservation(
        &self,
        workspace_id: &WorkspaceId,
        address: &str,
    ) -> Result<bool, StoreError>;
    async fn count_for_connection(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<u32, StoreError>;
}
