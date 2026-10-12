//! The host's check before each model call of a Run.

use async_trait::async_trait;
use pagis_core::{FailureKind, Run};

/// Why the host stops a Run before a model call.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{note}")]
pub struct GateStop {
    /// The note that the conversation shows.
    pub note: String,
    pub failure_kind: FailureKind,
}

/// A host checks the Run and its route before each model call.
#[async_trait]
pub trait ModelCallGate: Send + Sync {
    async fn before_call(&self, run: &Run, model_candidates: &[String]) -> Result<(), GateStop>;
}
