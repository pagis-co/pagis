//! What a Run spends, and the cap that stops one.
//!
//! Two things live here, and they are the two halves of the same fact.
//!
//! The router computes the cost of every model call from the serving
//! model's own price table. [`record`] keeps it, named by the Workspace
//! that spent it and the Run that asked. Nothing kept it before, so an
//! installation whose Administrator supplies the keys could not say who
//! spent the budget.
//!
//! [`cap_stop`] reads the same records back. An Administrator may
//! give a Person a monthly Spend Cap; a Run that would start over it
//! stops before it asks a model anything, and the conversation says so.
//! A silent stop would be the worst answer: the person would see a
//! sprite that does nothing and no reason for it.
//!
//! A model with no known price (neither the Provider Model List nor
//! `models.json` prices it) costs an unknown amount. Its record keeps
//! the cost as unknown, never as zero. A cap cannot count an unknown
//! cost, so a capped Person's Run does not start on a route that names
//! such a model: the cap holds, and the note names the model.

use pagis_core::{Run, UsageId, UsagePeriod, UsageRecord};

use crate::brain::TurnEnd;
use crate::system::AgentDeps;

/// What the conversation reads when a Run stops at the cap. It names the
/// cap and what to do about it, and it names no other person's spend.
pub const CAP_NOTE: &str = "This sprite stopped before it started work: \
     you are at your monthly spend cap for model calls. An administrator \
     of this installation raises the cap, or it resets at the start of \
     next month.";

/// Why the Spend Cap stops a Run before it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapStop {
    /// The month's spend reached the cap.
    Reached,
    /// The route names a model with no known price, whose cost the cap
    /// cannot count.
    Unpriced(String),
}

impl CapStop {
    /// What the conversation reads.
    pub fn note(&self) -> String {
        match self {
            CapStop::Reached => CAP_NOTE.to_string(),
            CapStop::Unpriced(candidate) => format!(
                "This sprite stopped before it started work: the model {candidate} \
                 has no known price, so your monthly spend cap cannot count what it \
                 costs. An administrator of this installation can remove the cap, or \
                 you can choose a model with a known price in Settings under Models."
            ),
        }
    }

    /// The short reason the Run's failure event carries.
    pub fn reason(&self) -> &'static str {
        match self {
            CapStop::Reached => "monthly spend cap reached",
            CapStop::Unpriced(_) => "model without a price under a spend cap",
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
        run_id: run.id.clone(),
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

/// Whether the Spend Cap stops this Run before it starts, and why.
///
/// A Person with no cap is never stopped, and neither is one the daemon
/// cannot resolve: a missing record must not stop somebody's work, and
/// the daemon says so in the log instead.
pub async fn cap_stop(deps: &AgentDeps, run: &Run, candidates: &[String]) -> Option<CapStop> {
    let Ok(Some(workspace)) = deps.workspaces.get(&run.workspace_id).await else {
        return None;
    };
    let person = match deps.users.get(&workspace.user_id).await {
        Ok(Some(person)) => person,
        Ok(None) => return None,
        Err(error) => {
            tracing::error!(%error, run_id = %run.id, "the spend cap could not be read");
            return None;
        }
    };
    let cap = person.monthly_spend_cap_usd?;
    if let Some(unpriced) = candidates.iter().find(|candidate| {
        candidate
            .split_once('/')
            .is_some_and(|(provider, model)| deps.models.metadata(provider, model).prices.is_none())
    }) {
        return Some(CapStop::Unpriced(unpriced.clone()));
    }
    let month = UsagePeriod::calendar_month(deps.clock.now_ms(), &workspace.timezone);
    let spent = match deps
        .usage
        .total_for_workspace(&run.workspace_id, month)
        .await
    {
        Ok(total) => total.cost_usd,
        Err(error) => {
            tracing::error!(%error, run_id = %run.id, "the month's spend could not be read");
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
        assert!(CAP_NOTE.contains("spend cap"));
        assert!(CAP_NOTE.contains("administrator"));
        assert!(CAP_NOTE.contains("next month"));
    }

    /// The note for an unpriced model names the model and why the cap
    /// stops it.
    #[test]
    fn the_unpriced_note_names_the_model() {
        let note = CapStop::Unpriced("openai/gpt-unlisted".into()).note();
        assert!(note.contains("openai/gpt-unlisted"));
        assert!(note.contains("no known price"));
        assert!(note.contains("spend cap"));
        assert_eq!(CapStop::Reached.note(), CAP_NOTE);
    }
}
