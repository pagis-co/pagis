//! The retention sweep: one workflow for every Artifact class.
//!
//! ADR-0020 keeps every Artifact by default. The user sets a window per
//! class, and this sweep is the only code that deletes an Artifact for
//! age. A class with no window is never swept.

use std::sync::Arc;

use object_store::ObjectStore;
use object_store::ObjectStoreExt as _;
use pagis_core::{ArtifactStore, RetentionPolicyStore, UnixMillis, WorkspaceStore, now_ms};
use tokio_util::sync::CancellationToken;

const SWEEP_BATCH: u32 = 500;
const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Every store the sweep reads. One struct, because the sweep runs both
/// from the daemon and from a test.
pub struct RetentionDeps {
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub policies: Arc<dyn RetentionPolicyStore>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub blobs: Arc<dyn ObjectStore>,
}

/// Delete every expired Artifact in every Workspace. Returns the number
/// of rows deleted.
pub async fn sweep(deps: &RetentionDeps, now: UnixMillis) -> anyhow::Result<u64> {
    let mut deleted = 0;
    for workspace in deps.workspaces.list().await? {
        for policy in deps.policies.list(&workspace.id).await? {
            let Some(cutoff) = policy.cutoff(now) else {
                continue;
            };
            deleted += sweep_class(deps, &workspace.id, policy.kind, cutoff).await?;
        }
    }
    Ok(deleted)
}

/// One class in one Workspace: blob first, then the row, so an
/// interrupted sweep never leaves a row without its blob's delete being
/// retried on the next pass.
async fn sweep_class(
    deps: &RetentionDeps,
    workspace_id: &pagis_core::WorkspaceId,
    kind: pagis_core::ArtifactKind,
    cutoff: UnixMillis,
) -> anyhow::Result<u64> {
    let mut deleted = 0;
    loop {
        let batch = deps
            .artifacts
            .expired_before(workspace_id, kind, cutoff, SWEEP_BATCH)
            .await?;
        if batch.is_empty() {
            return Ok(deleted);
        }
        let mut removed_in_batch = 0;
        for artifact in batch {
            let path = object_store::path::Path::from(artifact.storage_key.clone());
            match deps.blobs.delete(&path).await {
                Ok(()) => {}
                // A missing blob is already what the sweep wants.
                Err(object_store::Error::NotFound { .. }) => {}
                Err(err) => {
                    tracing::error!(error = %err, artifact_id = %artifact.id, "blob delete failed");
                    continue;
                }
            }
            if deps
                .artifacts
                .delete(&artifact.workspace_id, &artifact.id)
                .await?
            {
                deleted += 1;
                removed_in_batch += 1;
            }
        }
        // Every row in the batch failed its blob delete, so another
        // pass reads the same rows for ever. Stop and try tomorrow.
        if removed_in_batch == 0 {
            return Ok(deleted);
        }
    }
}

/// The daily sweep loop. It ends when the daemon cancels the token.
pub fn spawn_sweeper(deps: RetentionDeps, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        while cancel.run_until_cancelled(tick.tick()).await.is_some() {
            match sweep(&deps, now_ms()).await {
                Ok(0) => {}
                Ok(deleted) => tracing::info!(deleted, "retention sweep removed artifacts"),
                Err(err) => tracing::error!(error = %err, "retention sweep failed"),
            }
        }
    });
}
