//! What the model may call while it is on the phone (ADR-0020).
//!
//! A tool the model calls as a realtime event goes through the broker,
//! exactly as a tool the agent loop calls: `prepare_run` reads the
//! Run's Capability Snapshot, `invoke` validates and runs it. A live
//! call is a free-effect zone, so a tool that mints an approval card is
//! refused with `resolve`, and the Remote Party is never left waiting
//! for a user who may be asleep.
//!
//! The session tools `hang_up` and `send_digits` never reach this seam:
//! they act on the call itself, so the bridge sends them to the media
//! hub.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{Broker, InvokeOutcome, ToolCall, ToolResult};
use pagis_core::WorkspaceId;
use pagis_core::{AgentId, RequestState, RunId};

/// One tool call from inside a live call.
#[async_trait]
pub trait CallTools: Send + Sync {
    async fn invoke(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        call: ToolCall,
    ) -> ToolResult;
}

/// The broker, as the bridge reaches it.
pub struct BrokerCallTools {
    broker: Arc<Broker>,
}

impl BrokerCallTools {
    pub fn new(broker: Arc<Broker>) -> Self {
        Self { broker }
    }
}

#[async_trait]
impl CallTools for BrokerCallTools {
    async fn invoke(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        run_id: &RunId,
        call: ToolCall,
    ) -> ToolResult {
        let snapshot = match self
            .broker
            .prepare_run(workspace_id, agent_id, run_id)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return ToolResult::error("temporarily_unavailable", error.to_string());
            }
        };
        let outcome = match self
            .broker
            .invoke(workspace_id, run_id, &snapshot.id, call)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                return ToolResult::error("temporarily_unavailable", error.to_string());
            }
        };
        match outcome {
            InvokeOutcome::Completed(result) | InvokeOutcome::Rejected(result) => result,
            InvokeOutcome::Waiting(pending) => {
                // A card cannot be answered while a Remote Party waits.
                // The Run stays alive after the call, so the Agent does
                // this work then.
                let _ = self
                    .broker
                    .resolve(&pending.request.id, RequestState::Denied)
                    .await;
                ToolResult::error(
                    "permission_denied",
                    "this action needs the user's approval, and a call cannot wait for one. Do \
                     it after the call ends.",
                )
            }
        }
    }
}
