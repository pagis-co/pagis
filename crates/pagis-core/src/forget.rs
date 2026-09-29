//! Owner-authorized suppression is distinct from access grants and Sync state.
use crate::{
    ConnectionId, MemoryExposure, RunId, SealError, StoreError, WorkspaceId, knowledge::SourceKey,
};
use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// The key of the suppression identities of one Workspace (ADR-0008).
///
/// It derives from the Tenant Data Key of the Workspace, so the database
/// never holds it. A copy of the database alone therefore cannot test a
/// known source id against the suppression rows. It gives identities
/// and nothing else, and no accessor gives its bytes.
pub struct SuppressionKey([u8; 32]);

impl SuppressionKey {
    pub(crate) fn derived(key: [u8; 32]) -> Self {
        Self(key)
    }

    /// The suppression identity of one source item of this Workspace.
    pub fn identity(&self, source: &SourceKey, source_id: &str) -> String {
        suppression_identity(&self.0, source, source_id)
    }
}

/// Where a store gets the suppression key of a Workspace. The daemon
/// passes its one shared [`crate::TenantKeys`]. A store asks for a key
/// only when a Forget writes suppression rows, or when the Connection of
/// a source id has one, so a Workspace that never forgot anything makes
/// no key.
pub trait ForgetKeys: Send + Sync {
    fn suppression_key(&self, workspace_id: &WorkspaceId) -> Result<SuppressionKey, SealError>;
}

/// The suppression identity of one source item under `key`: the
/// HMAC-SHA256 of the source and its id, as hex. A Forget stores it in
/// place of the id, and a new retrieval of the same id computes it
/// again to find the suppression (ADR-0008). [`SuppressionKey::identity`]
/// is the only caller in the daemon.
pub fn suppression_identity(key: &[u8], source: &SourceKey, source_id: &str) -> String {
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes a key of any length");
    let message = serde_json::to_string(&(source, source_id)).expect("a source key is text");
    mac.update(message.as_bytes());
    crate::seal::hex_encode(&mac.finalize().into_bytes())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForgetTarget {
    Source {
        source: SourceKey,
        source_id: String,
    },
    Account {
        #[schema(value_type = String)]
        connection_id: ConnectionId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ForgetPhase {
    Blocked,
    StructuredPurged,
    MemoryPurged,
    Complete,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ForgetPreview {
    pub revision: String,
    pub source_items: u64,
    pub memory_revision: Option<String>,
    pub memory_paths: u64,
    pub memory_revisions: u64,
    pub raw_account_retrieval_blocked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ForgetOperation {
    pub id: String,
    #[schema(value_type = String)]
    pub connection_id: ConnectionId,
    pub phase: ForgetPhase,
    pub created_at: i64,
    pub error: Option<String>,
    pub reopted_at: Option<i64>,
}

#[async_trait]
pub trait ForgetStore: Send + Sync {
    async fn preview(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
    ) -> Result<ForgetPreview, StoreError>;
    async fn preview_affects(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
        exposures: &[MemoryExposure],
    ) -> Result<bool, StoreError>;
    /// Block the target: write a suppression row for each source item
    /// that it removes. `keys` gives the suppression key of the
    /// Workspace, which the database never holds.
    async fn begin(
        &self,
        workspace: &WorkspaceId,
        target: &ForgetTarget,
        expected_revision: &str,
        now: i64,
        keys: &dyn ForgetKeys,
    ) -> Result<ForgetOperation, StoreError>;
    async fn is_suppressed(
        &self,
        source: &SourceKey,
        source_id: &str,
        keys: &dyn ForgetKeys,
    ) -> Result<bool, StoreError>;
    /// Whether a Forget permits content with these exposures. It reads
    /// the suppression rows and no key.
    async fn permits_provenance(
        &self,
        workspace: &WorkspaceId,
        exposures: &[MemoryExposure],
    ) -> Result<bool, StoreError>;
    /// A run that a forgotten reminder queued must not start work.
    /// The suppression outlives the reminder record the purge deletes.
    async fn run_allowed(&self, workspace: &WorkspaceId, run: &RunId) -> Result<bool, StoreError>;
    async fn raw_connection_allowed(
        &self,
        workspace: &WorkspaceId,
        connection: &ConnectionId,
    ) -> Result<bool, StoreError>;
    async fn operation(
        &self,
        workspace: &WorkspaceId,
        id: &str,
    ) -> Result<Option<ForgetOperation>, StoreError>;
    async fn operations(&self, workspace: &WorkspaceId)
    -> Result<Vec<ForgetOperation>, StoreError>;
    async fn unfinished(&self, workspace: &WorkspaceId)
    -> Result<Vec<ForgetOperation>, StoreError>;
    async fn purge_structured(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError>;
    /// Record the end of the memory purge, after the Page Index holds
    /// the rebuilt history. The same step clears each Learning Feed
    /// entry of a change that the purge took out of the history, and
    /// keeps in each Brief cursor only the pages that memory holds.
    async fn memory_purged(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError>;
    /// Erase from the database files the rows that the memory purge and
    /// [`Self::memory_purged`] removed, then complete the operation.
    async fn complete(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError>;
    async fn record_failure(
        &self,
        workspace: &WorkspaceId,
        id: &str,
        error: &str,
    ) -> Result<(), StoreError>;
    async fn retry(&self, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError>;
    /// Allow a new retrieval of what a completed Forget blocked: delete
    /// its suppression rows. The purge left no row of a forgotten item,
    /// so the call restores nothing and computes no identity.
    async fn reopt_in(&self, workspace: &WorkspaceId, id: &str, now: i64)
    -> Result<(), StoreError>;
}
