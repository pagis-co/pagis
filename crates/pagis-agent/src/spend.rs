//! What a Run spends, and the cap that stops one.
//!
//! Two things live here, and they are the two halves of the same fact.
//!
//! The router computes the cost of every model call from the serving
//! model's own price table. [`record`] keeps it, named by the Workspace
//! that spent it and the Run that asked. The Administrator supplies the
//! provider keys, and the Usage Records show what each Workspace spends.
//!
//! [`cap_stop`] reads the same records back. An Administrator may
//! give a Workspace a monthly Spend Cap. A Run checks it before each model
//! call, in its first Turn and in each later Turn. It stops before the
//! call that would go over the cap, and the conversation says why.
//! Replies from earlier Turns stay in the conversation. The Harness
//! Model Endpoint makes the same check before each model request of a
//! Coding Session.
//!
//! A model with no known price (neither the Provider Model List nor
//! `models.json` prices it) costs an unknown amount. Its record keeps
//! the cost as unknown, never as zero. A cap cannot count an unknown
//! cost, so a capped Workspace's Run makes no call on a route that names
//! such a model: the cap holds, and the note names the model.

use std::sync::Arc;

use async_trait::async_trait;

use pagis_core::{
    Clock, FailureKind, Run, UsageId, UsagePeriod, UsageRecord, UsageStore, WorkspaceId,
    WorkspaceStore,
};

use crate::ModelCatalog;
use crate::brain::TurnEnd;
use crate::system::AgentDeps;
use crate::{GateStop, ModelCallGate};

/// Why the Spend Cap stops a model call before it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapStop {
    /// The month's spend reached the cap.
    Reached,
    /// The route names a model with no known price, whose cost the cap
    /// cannot count.
    Unpriced(String),
}

impl CapStop {
    /// What the conversation reads when a Run stops. It names the cap
    /// and what to do about it, and it names no other person's spend.
    pub fn note(&self) -> String {
        format!(
            "This sprite stopped before its next model call: {}",
            self.cause()
        )
    }

    /// What the Harness Model Endpoint answers a Coding Session. Only
    /// the Agent reads it, through the harness.
    pub fn refusal(&self) -> String {
        format!(
            "This model request stopped before it reached the provider: {}",
            self.cause()
        )
    }

    fn cause(&self) -> String {
        match self {
            CapStop::Reached => {
                "this Workspace's monthly spend cap is reached for model calls. An \
                 administrator of this installation raises the cap, or it resets at the start \
                 of next month."
                    .to_string()
            }
            CapStop::Unpriced(candidate) => format!(
                "the model {candidate} has no known price, so this Workspace's monthly spend cap cannot \
                 count what it costs. An administrator of this installation can remove the \
                 cap, or you can choose a model with a known price in Settings under Models."
            ),
        }
    }
}

/// Keep what one model call spent. A turn that reports no usage records
/// nothing, because a zero row would read as a call that cost nothing.
///
/// A write that fails is logged and never fails the Run: the answer the
/// person asked for matters more than the accounting row, and the row is
/// derived from a number the audit event already carries.
pub async fn record(deps: &AgentDeps, run: &Run, end: &TurnEnd) {
    let Some(usage) = end.usage else {
        return;
    };
    let record = UsageRecord {
        id: UsageId::generate(),
        workspace_id: run.workspace_id.clone(),
        run_id: Some(run.id.clone()),
        provider: end.provider.clone(),
        model: end.model.clone(),
        input_tokens: i64::try_from(usage.input_tokens).unwrap_or(i64::MAX),
        output_tokens: i64::try_from(usage.output_tokens).unwrap_or(i64::MAX),
        cache_read_tokens: i64::try_from(usage.cache_read_input_tokens).unwrap_or(i64::MAX),
        cache_write_tokens: i64::try_from(usage.cache_write_input_tokens).unwrap_or(i64::MAX),
        // A model no layer prices costs an unknown amount, which the
        // record keeps as unknown: the tokens are still the truth, and a
        // zero or made-up price would be worse than none.
        cost_usd: end.estimated_cost_usd,
        created_at: deps.clock.now_ms(),
    };
    if let Err(error) = deps.usage.record(&record).await {
        tracing::error!(%error, run_id = %run.id, "the usage record was not kept");
    }
}

/// The reads of the Spend Cap.
#[derive(Clone, Copy)]
pub struct CapReads<'a> {
    pub workspaces: &'a dyn WorkspaceStore,
    pub usage: &'a dyn UsageStore,
    pub models: &'a ModelCatalog,
    pub clock: &'a dyn Clock,
}

/// The daemon's Spend Cap check before each model call of a Run.
pub struct SpendCapGate {
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub usage: Arc<dyn UsageStore>,
    pub models: Arc<ModelCatalog>,
    pub clock: Arc<dyn Clock>,
}

#[async_trait]
impl ModelCallGate for SpendCapGate {
    async fn before_call(&self, run: &Run, bound: &crate::ModelCallBound) -> Result<(), GateStop> {
        let reads = CapReads {
            workspaces: self.workspaces.as_ref(),
            usage: self.usage.as_ref(),
            models: self.models.as_ref(),
            clock: self.clock.as_ref(),
        };
        match cap_stop(reads, &run.workspace_id, &bound.model_candidates).await {
            Some(stop) => Err(GateStop {
                note: stop.note(),
                failure_kind: FailureKind::SpendCapReached,
            }),
            None => Ok(()),
        }
    }
}

/// Whether the Spend Cap of `workspace_id` stops a
/// model call on the `provider/model` candidates, and why.
///
/// A Workspace with no cap is never stopped, and neither is one the daemon
/// cannot resolve: a missing record must not stop somebody's work, and
/// the daemon says so in the log instead.
pub async fn cap_stop(
    reads: CapReads<'_>,
    workspace_id: &WorkspaceId,
    candidates: &[String],
) -> Option<CapStop> {
    let Ok(Some(workspace)) = reads.workspaces.get(workspace_id).await else {
        return None;
    };
    let cap = workspace.monthly_spend_cap_usd?;
    if let Some(unpriced) = candidates.iter().find(|candidate| {
        candidate.split_once('/').is_some_and(|(provider, model)| {
            reads.models.metadata(provider, model).prices.is_none()
        })
    }) {
        return Some(CapStop::Unpriced(unpriced.clone()));
    }
    let month = UsagePeriod::calendar_month(reads.clock.now_ms(), &workspace.timezone);
    let spent = match reads.usage.total_for_workspace(workspace_id, month).await {
        Ok(total) => total.cost_usd,
        Err(error) => {
            tracing::error!(%error, %workspace_id, "the month's spend could not be read");
            return None;
        }
    };
    (spent >= cap).then_some(CapStop::Reached)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The note names the cap and what changes it, so a person reads a
    /// reason rather than a silence.
    #[test]
    fn the_note_names_the_cap_and_the_way_out() {
        let note = CapStop::Reached.note();
        assert!(note.contains("sprite"));
        assert!(note.contains("spend cap"));
        assert!(note.contains("administrator"));
        assert!(note.contains("next month"));
    }

    /// The note for an unpriced model names the model and why the cap
    /// stops it.
    #[test]
    fn the_unpriced_note_names_the_model() {
        let note = CapStop::Unpriced("openai/gpt-unlisted".into()).note();
        assert!(note.contains("openai/gpt-unlisted"));
        assert!(note.contains("no known price"));
        assert!(note.contains("spend cap"));
    }

    /// The refusal of the Harness Model Endpoint gives the reason of the
    /// note, and no sprite: only the Agent reads it.
    #[test]
    fn the_refusal_gives_the_reason_of_the_note_without_a_sprite() {
        for stop in [
            CapStop::Reached,
            CapStop::Unpriced("anthropic/claude-unlisted".into()),
        ] {
            let refusal = stop.refusal();
            assert!(!refusal.contains("sprite"), "{refusal}");
            assert!(refusal.contains("spend cap"), "{refusal}");
            assert_eq!(
                refusal.split_once(": ").map(|(_, cause)| cause),
                stop.note().split_once(": ").map(|(_, cause)| cause)
            );
        }
    }
}
