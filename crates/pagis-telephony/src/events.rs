//! The `call.ended` Incoming Event (ADR-0020, ADR-0021): what one
//! finished Call carries into the Trigger module, the trusted matcher
//! that reads a rule's typed filter against it, and the two seams the
//! Trigger module is reached through.
//!
//! A Call the Agent placed ends inside the Run that placed it, and
//! that Run reads the transcript in the tool result. A Call the Agent
//! answered has no such Run, so ADR-0021's "the Run that reads it
//! afterwards" is this event: the daemon raises it when the call
//! settles, the Standing Call Rule matches, and a Wake-up starts the
//! Run in the Agent's Thread with the user (ADR-0022).
//!
//! The metadata is the envelope of the call and never its words. The
//! Agent reads those with a live `call_transcript`, which wraps them
//! in the untrusted envelope with the tier. The tier stamps the
//! event and gates what the words are worth, never whether the Agent
//! wakes: only a `min_trust` filter stops a wake, as it does for mail
//! (ADR-0019).

use async_trait::async_trait;
use pagis_core::{
    AgentId, ArtifactId, CallDirection, CallId, CallOutcome, EventMatcher, IngestBatch,
    NormalizedEvent, PhoneNumber, TrustTier, UnixMillis,
};

pub use pagis_broker::{CALL_ENDED, CALL_MATCHER};

use crate::bridge::CallReport;

/// The name the rule carries on the Agent's number panel and in its
/// rule list.
pub const STANDING_CALL_RULE_NAME: &str = "Answered calls";

/// What the Wake-up tells the Agent to do. It names the tool, because
/// the event carries the envelope and never the words.
pub const STANDING_CALL_INSTRUCTION: &str = "You answered a call on your desk line. Read what was said with `call_transcript` and act \
     on your job.";

/// One Call, as it settled. The bridge has returned its report and the
/// transcript is stored; this is what the Trigger module is told about.
pub struct SettledCall<'a> {
    pub call_id: &'a CallId,
    pub direction: CallDirection,
    /// The line the call was on.
    pub number: &'a PhoneNumber,
    /// The other party.
    pub remote_e164: &'a str,
    pub report: &'a CallReport,
    pub transcript_artifact_id: Option<&'a ArtifactId>,
    pub ended_at: UnixMillis,
}

/// What one finished Call tells the Trigger module. The transcript is
/// never here: the Agent reads the words with `call_transcript`, and
/// the metadata carries the envelope alone.
pub fn call_ended_event(call: &SettledCall<'_>) -> NormalizedEvent {
    let report = call.report;
    let mut metadata = serde_json::Map::new();
    metadata.insert("call_id".into(), call.call_id.to_string().into());
    metadata.insert("direction".into(), direction_text(call.direction).into());
    metadata.insert("number".into(), call.number.e164.clone().into());
    metadata.insert("from".into(), call.remote_e164.to_string().into());
    metadata.insert("trust_tier".into(), report.tier.as_str().into());
    metadata.insert("outcome".into(), report.outcome.as_str().into());
    metadata.insert("ended_reason".into(), report.ended_reason.clone().into());
    metadata.insert(
        "duration_ms".into(),
        (report.duration.as_millis() as i64).into(),
    );
    if let Some(artifact_id) = call.transcript_artifact_id {
        metadata.insert(
            "transcript_artifact_id".into(),
            artifact_id.to_string().into(),
        );
    }
    if let Some(answered_at) = report.answered_at {
        metadata.insert("answered_at".into(), answered_at.into());
    }
    NormalizedEvent {
        // One Call settles once, so its own id is the identity of the
        // occurrence: a batch replayed after a restart wakes nobody
        // twice.
        provider_event_id: call.call_id.to_string(),
        metadata: serde_json::Value::Object(metadata),
        occurred_at: call.ended_at,
        // A Call the Agent answered has no Thread that started it, so
        // the Wake-up lands in the rule's own conversation, which is
        // the Agent's Thread with the user (ADR-0022).
        landing: None,
    }
}

fn direction_text(direction: CallDirection) -> &'static str {
    match direction {
        CallDirection::Inbound => "inbound",
        CallDirection::Outbound => "outbound",
    }
}

/// Reads a typed call filter against one call's metadata (ADR-0006):
/// different fields combine with AND, values inside one field combine
/// with OR.
#[derive(Debug, Default, Clone, Copy)]
pub struct CallEndedMatcher;

impl EventMatcher for CallEndedMatcher {
    fn matches(&self, filter: &serde_json::Value, metadata: &serde_json::Value) -> bool {
        if let Some(callers) = filter["callers"].as_array() {
            let from = metadata["from"].as_str().unwrap_or_default();
            if !callers
                .iter()
                .filter_map(serde_json::Value::as_str)
                .any(|caller| caller == from)
            {
                return false;
            }
        }
        if let Some(wanted) = filter["min_trust"].as_str().and_then(tier) {
            // The tiers are ordered by authority, so "at least this
            // much" is one comparison. An unreadable tier is unknown.
            let held = metadata["trust_tier"]
                .as_str()
                .and_then(tier)
                .unwrap_or(TrustTier::Unknown);
            if held < wanted {
                return false;
            }
        }
        if filter["answered_only"].as_bool() == Some(true)
            && metadata["outcome"].as_str() != Some(CallOutcome::Answered.as_str())
        {
            return false;
        }
        true
    }
}

fn tier(name: &str) -> Option<TrustTier> {
    match name {
        "owner" => Some(TrustTier::Owner),
        "trusted" => Some(TrustTier::Trusted),
        "unknown" => Some(TrustTier::Unknown),
        _ => None,
    }
}

/// Why a call batch did not reach the Trigger module. The call is
/// already over and its record is already written, so the daemon logs
/// this and does not fail the call.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CallIngestError(pub String);

/// The `ingest` seam over the Trigger module. The Trigger module lives
/// above this crate, and the call that ends lives here.
#[async_trait]
pub trait CallEvents: Send + Sync {
    async fn ingest(&self, batch: IngestBatch) -> Result<(), CallIngestError>;
}

/// Why a Standing Call Rule was not written. It carries the words the
/// number panel shows.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StandingRuleError(pub String);

/// The Standing Call Rule of one desk line. Assigning a number to an
/// Agent creates it, so the Agent wakes on every call it answers;
/// taking the line back archives it. It carries the provenance of the
/// user, so the Agent may narrow it and never delete it.
#[async_trait]
pub trait StandingCallRule: Send + Sync {
    async fn create(
        &self,
        number: &PhoneNumber,
        agent_id: &AgentId,
    ) -> Result<(), StandingRuleError>;
    async fn archive(
        &self,
        number: &PhoneNumber,
        agent_id: &AgentId,
    ) -> Result<(), StandingRuleError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(from: &str, tier: &str, outcome: &str) -> serde_json::Value {
        serde_json::json!({"from": from, "trust_tier": tier, "outcome": outcome})
    }

    #[test]
    fn an_empty_filter_matches_every_call() {
        let matcher = CallEndedMatcher;
        assert!(matcher.matches(
            &serde_json::json!({}),
            &metadata("+14155550123", "unknown", "answered")
        ));
    }

    #[test]
    fn min_trust_is_the_only_field_that_stops_a_wake_on_a_stranger() {
        let matcher = CallEndedMatcher;
        let stranger = metadata("+14155550123", "unknown", "answered");
        assert!(!matcher.matches(&serde_json::json!({"min_trust": "trusted"}), &stranger));
        assert!(matcher.matches(&serde_json::json!({"min_trust": "unknown"}), &stranger));
    }

    #[test]
    fn a_caller_list_holds_one_number_and_refuses_the_rest() {
        let matcher = CallEndedMatcher;
        let filter = serde_json::json!({"callers": ["+14155550123", "+14155550124"]});
        assert!(matcher.matches(&filter, &metadata("+14155550124", "owner", "answered")));
        assert!(!matcher.matches(&filter, &metadata("+16695550000", "owner", "answered")));
    }

    #[test]
    fn answered_only_drops_a_call_nobody_answered() {
        let matcher = CallEndedMatcher;
        let filter = serde_json::json!({"answered_only": true});
        assert!(matcher.matches(&filter, &metadata("+14155550123", "owner", "answered")));
        assert!(!matcher.matches(&filter, &metadata("+14155550123", "owner", "no_answer")));
    }
}
