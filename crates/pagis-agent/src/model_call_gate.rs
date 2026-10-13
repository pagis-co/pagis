//! The Embedder's check and accounting of each model call of a Run.

use async_trait::async_trait;
use pagis_core::{FailureKind, Run};

use crate::brain::{BrainError, TurnEnd};

/// The route and token bound of one model request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCallBound {
    /// The ordered `provider/model` candidates of the request.
    pub model_candidates: Vec<String>,
    /// The input token bound that `model.requested` records.
    pub input_tokens: u64,
    /// The output token limit that the request sends.
    pub output_limit: u32,
}

/// Why the Embedder stops a Run before a model call.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{note}")]
pub struct GateStop {
    /// The note that the conversation shows.
    pub note: String,
    pub failure_kind: FailureKind,
}

/// An Embedder checks the Run and its request bound before each model call.
#[async_trait]
pub trait ModelCallGate: Send + Sync {
    async fn before_call(&self, run: &Run, bound: &ModelCallBound) -> Result<(), GateStop>;

    /// Receive one outcome after each permitted call. `TurnEnd` carries
    /// the provider usage and the router's cost estimate in USD. Missing
    /// usage or prices stay unknown. A cancellation returns a failure.
    /// A call stopped by `before_call` has no outcome.
    async fn after_call(&self, _run: &Run, _outcome: Result<&TurnEnd, &BrainError>) {}
}
