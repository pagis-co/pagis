//! The Model Request Capture (ADR-0031): a copy of one model request of
//! a Run and of the provider's answer, which the daemon keeps only while
//! an Administrator has the System Setting on.
//!
//! The capture is Person data: it holds the system prompt, the messages
//! and the tool results that the request sent. So only an Administrator
//! reads it, it expires after the retention of the setting, Forget
//! deletes it with the rest of the output of a Run, and a Backup leaves
//! it out.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use async_trait::async_trait;

use crate::id::{ModelRequestCaptureId, RunId, WorkspaceId};
use crate::store::StoreError;
use crate::time::UnixMillis;

/// The retention of a capture when the Administrator sets none.
pub const DEFAULT_CAPTURE_RETENTION_DAYS: u32 = 7;
/// The longest retention an Administrator can set.
pub const MAX_CAPTURE_RETENTION_DAYS: u32 = 30;

/// One model request of a Run and the provider's answer. `request` and
/// `answer` are JSON objects; the agent loop writes their shape.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequestCapture {
    pub id: ModelRequestCaptureId,
    pub workspace_id: WorkspaceId,
    pub run_id: RunId,
    /// `reply`, `compaction` or `reflection`, as on `model.requested`.
    pub phase: String,
    /// The number of the request inside its phase, from 0.
    pub phase_request: i64,
    pub request: serde_json::Value,
    pub answer: serde_json::Value,
    pub created_at: UnixMillis,
}

#[async_trait]
pub trait ModelRequestCaptureStore: Send + Sync {
    /// Keep one capture.
    async fn record(&self, capture: &ModelRequestCapture) -> Result<(), StoreError>;
    /// The captures of one Run, in the order the Run made the requests.
    async fn list_for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Vec<ModelRequestCapture>, StoreError>;
    /// Delete every capture made before `cutoff`, in every Workspace.
    /// The retention sweep of the installation calls it.
    async fn delete_before(&self, cutoff: UnixMillis) -> Result<u64, StoreError>;
    /// Delete every capture, in every Workspace. Turning the setting off
    /// calls it.
    async fn delete_all(&self) -> Result<u64, StoreError>;
}

/// The live value of the System Setting. The daemon sets it at boot from
/// `config.toml`, and the route of the setting changes it, so the agent
/// loop reads the change with no restart.
#[derive(Debug)]
pub struct CaptureSetting {
    enabled: AtomicBool,
    retention_days: AtomicU32,
}

impl CaptureSetting {
    pub fn new(enabled: bool, retention_days: u32) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            retention_days: AtomicU32::new(retention_days),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn retention_days(&self) -> u32 {
        self.retention_days.load(Ordering::Relaxed)
    }

    pub fn set(&self, enabled: bool, retention_days: u32) {
        self.retention_days.store(retention_days, Ordering::Relaxed);
        self.enabled.store(enabled, Ordering::Relaxed);
    }
}

impl Default for CaptureSetting {
    fn default() -> Self {
        Self::new(false, DEFAULT_CAPTURE_RETENTION_DAYS)
    }
}
