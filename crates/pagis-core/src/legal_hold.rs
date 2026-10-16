//! Legal Holds and the Org Audit Log.
use crate::{AuditEntryId, Author, StoreError, UnixMillis, WorkspaceId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LegalHold {
    pub started_at: UnixMillis,
    pub started_by: Author,
}
impl LegalHold {
    /// Serialize a hold change with content operations outside the database.
    /// One daemon owns an installation. Weak entries release unused gates.
    pub async fn lock(workspace: &WorkspaceId) -> futures::lock::OwnedMutexGuard<()> {
        use std::collections::BTreeMap;
        use std::sync::{Arc, Mutex, OnceLock, Weak};
        type Gates = BTreeMap<WorkspaceId, Weak<futures::lock::Mutex<()>>>;
        static GATES: OnceLock<Mutex<Gates>> = OnceLock::new();
        let gate = {
            let mut gates = GATES
                .get_or_init(Mutex::default)
                .lock()
                .expect("Legal Hold gates");
            gates.retain(|_, gate| gate.strong_count() > 0);
            let gate = gates
                .get(workspace)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(futures::lock::Mutex::new(())));
            gates.insert(workspace.clone(), Arc::downgrade(&gate));
            gate
        };
        gate.lock_owned().await
    }

    pub const MESSAGE: &str =
        "This Workspace is under legal hold. Nothing in it can be deleted until the hold ends.";
    pub fn refuse_delete(held: bool) -> Result<(), StoreError> {
        if held {
            Err(StoreError::LegalHold)
        } else {
            Ok(())
        }
    }
}

/// One append-only record owned by the Org Workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuditEntry {
    #[schema(value_type = String)]
    pub id: AuditEntryId,
    #[schema(value_type = String)]
    pub org_workspace_id: WorkspaceId,
    pub actor: Author,
    pub action: String,
    #[schema(value_type = String)]
    pub subject_workspace_id: WorkspaceId,
    pub reason: Option<String>,
    pub at: UnixMillis,
}
#[async_trait]
pub trait AuditLog: Send + Sync {
    async fn append(&self, entry: &AuditEntry) -> Result<(), StoreError>;
    /// Newest first, with id as the tie breaker.
    async fn list(
        &self,
        org_workspace_id: &WorkspaceId,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<AuditEntry>, StoreError>;
}

impl From<StoreError> for crate::MemoryError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::LegalHold => Self::LegalHold,
            other => Self::Storage(other.to_string()),
        }
    }
}
