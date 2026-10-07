//! The bridge seam (ADR-0020): what executes one call. The call
//! tool composes the brief, binds the session tools and writes the
//! audit trail; the bridge places the call, pumps the realtime session
//! and reports how it ended. Its fake lives
//! here beside the seam, as ADR-0020 says.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use pagis_broker::ToolDef;
use pagis_core::{
    AgentId, ArtifactId, CallId, CallOutcome, Classification, EventBus, KeypadFailures, NewEvent,
    RunId, StoreError, TranscriptLine, TrustTier, UnixMillis, WorkspaceId,
};

use crate::brief::CallBrief;
use crate::endpoint::IncomingHub;

/// One call handed to the bridge: the brief, and the tools the
/// realtime session binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedCall {
    pub id: CallId,
    pub workspace_id: WorkspaceId,
    pub run_id: RunId,
    pub brief: CallBrief,
    pub tools: Vec<ToolDef>,
}

/// How one call ended. The transcript is the whole conversation; the
/// recording is a pointer, because nothing in the loop reads audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallReport {
    pub outcome: CallOutcome,
    /// The Trust Tier the call ended at (ADR-0021). It starts at the
    /// tier the challenge settled and it rises only in place, so this
    /// is the tier the words in the transcript are worth.
    pub tier: TrustTier,
    /// Why the audio stopped, e.g. `hangup`, `media_timeout`,
    /// `duration_cap`, `model_unavailable`.
    pub ended_reason: String,
    pub classification: Option<Classification>,
    /// True when a voicemail message was left.
    pub message_left: bool,
    pub duration: Duration,
    /// When the far side started to ring, when it did.
    pub ringing_at: Option<UnixMillis>,
    /// When the carrier reported the call answered.
    pub answered_at: Option<UnixMillis>,
    /// The conversation, one line per turn, with the time it was said.
    pub transcript: Vec<TranscriptLine>,
    pub recording_artifact_id: Option<ArtifactId>,
}

/// The bridge did not carry the call to an end of its own. The reason
/// reaches the model and the audit log, and never a credential.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the call did not complete: {0}")]
pub struct BridgeError(pub String);

/// The audit trail of one call (ADR-0020): `call.placed`,
/// `call.answered` and `call.ended`, each with the E.164 number, the
/// Agent and the Run. The bridge reports the answer through it, so
/// the event lands when the carrier reports it and not after the
/// call. No event carries a credential.
#[derive(Clone)]
pub struct CallLog {
    bus: std::sync::Arc<dyn EventBus>,
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    run_id: RunId,
    call_id: CallId,
    own_e164: String,
    remote_e164: String,
}

impl CallLog {
    pub fn new(bus: std::sync::Arc<dyn EventBus>, call: &PlacedCall) -> Self {
        Self {
            bus,
            workspace_id: call.workspace_id.clone(),
            agent_id: call.brief.agent_id.clone(),
            run_id: call.run_id.clone(),
            call_id: call.id.clone(),
            own_e164: call.brief.own_e164.clone(),
            remote_e164: call.brief.remote_e164.clone(),
        }
    }

    pub async fn placed(&self, call: &PlacedCall) -> Result<(), StoreError> {
        self.publish(
            "call.placed",
            serde_json::json!({
                "direction": call.brief.direction.as_str(),
                "phone_number_id": call.brief.phone_number_id.as_str(),
                "tier": call.brief.tier.as_str(),
                "duration_cap_s": call.brief.duration_cap.as_secs(),
            }),
        )
        .await
    }

    /// The carrier reported the call answered.
    pub async fn answered(&self) -> Result<(), StoreError> {
        self.publish("call.answered", serde_json::json!({})).await
    }

    /// One line of the conversation, as the realtime session
    /// transcribed it. The live `call` block in the Thread reads these
    /// events, and the same lines settle on the Call record.
    pub async fn transcript(&self, line: &TranscriptLine) -> Result<(), StoreError> {
        self.publish(
            "call.transcript",
            serde_json::json!({
                "at": line.at,
                "speaker": line.speaker.as_str(),
                "text": line.text,
            }),
        )
        .await
    }

    /// What one response of the realtime session cost. The event is
    /// the `turn.completed` every Run reports its usage with, so a
    /// call meters the way a thinking turn does.
    pub async fn usage(
        &self,
        turn: u32,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<(), StoreError> {
        self.publish(
            "turn.completed",
            serde_json::json!({
                "turn": turn,
                "stop_reason": "call_response",
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
            }),
        )
        .await
    }

    pub async fn ended(
        &self,
        report: &CallReport,
        transcript_artifact_id: Option<&ArtifactId>,
    ) -> Result<(), StoreError> {
        self.publish(
            "call.ended",
            serde_json::json!({
                "outcome": report.outcome,
                "ended_reason": report.ended_reason,
                "classification": report.classification,
                "message_left": report.message_left,
                "duration_s": report.duration.as_secs(),
                "transcript_artifact_id": transcript_artifact_id.map(ArtifactId::as_str),
                "recording_artifact_id": report.recording_artifact_id.as_ref().map(ArtifactId::as_str),
            }),
        )
        .await
    }

    /// The bridge failed: the call still ends, with the failure as its
    /// reason, because every call ends with a reason (ADR-0020).
    pub async fn failed(&self, error: &BridgeError) -> Result<(), StoreError> {
        self.publish(
            "call.ended",
            serde_json::json!({
                "outcome": CallOutcome::Failed,
                "ended_reason": error.0,
            }),
        )
        .await
    }

    /// A tier moved (ADR-0021): the old tier, the new tier and the
    /// cause. It never carries the code, and a tier that moved with no
    /// audit row is a tier nobody can review afterwards.
    pub async fn tier_changed(&self, change: crate::tiers::TierChange) -> Result<(), StoreError> {
        self.publish(
            "call.tier_changed",
            serde_json::json!({
                // The common part of every call event already uses
                // `from` and `to` for the two numbers.
                "from_tier": change.from.as_str(),
                "to_tier": change.to.as_str(),
                "cause": change.cause.as_str(),
            }),
        )
        .await
    }

    /// A caller entered a wrong Keypad Code (ADR-0021): the tier caller
    /// ID proposed, the new failed-attempt count of the Workspace, and
    /// the end of the latest delay. It never carries the digits.
    pub async fn keypad_failed(
        &self,
        candidate: TrustTier,
        failures: &KeypadFailures,
    ) -> Result<(), StoreError> {
        self.publish(
            "call.keypad_failed",
            serde_json::json!({
                "candidate_tier": candidate.as_str(),
                "failed_attempts": failures.failed_attempts,
                "suspended_until": failures.suspended_until,
            }),
        )
        .await
    }

    /// A correct Keypad Code cleared the failed-attempt count of the
    /// Workspace and ended its delay (ADR-0021).
    pub async fn keypad_cleared(&self) -> Result<(), StoreError> {
        self.publish("keypad.cleared", serde_json::json!({})).await
    }

    async fn publish(
        &self,
        event_type: &str,
        mut payload: serde_json::Value,
    ) -> Result<(), StoreError> {
        let common = serde_json::json!({
            "call_id": self.call_id.as_str(),
            "from": self.own_e164,
            "to": self.remote_e164,
        });
        if let (Some(payload), Some(common)) = (payload.as_object_mut(), common.as_object()) {
            payload.extend(common.clone());
        }
        self.bus
            .publish(NewEvent {
                workspace_id: self.workspace_id.clone(),
                event_type: event_type.to_string(),
                agent_id: Some(self.agent_id.clone()),
                run_id: Some(self.run_id.clone()),
                channel_id: None,
                payload,
            })
            .await?;
        Ok(())
    }
}

/// Signaling, media and the realtime session for one call.
#[async_trait]
pub trait CallBridge: Send + Sync {
    /// Place the call and stay on it until it ends. The bridge calls
    /// `log.answered()` when the carrier reports the answer.
    async fn place(&self, call: PlacedCall, log: &CallLog) -> Result<CallReport, BridgeError>;

    /// Run a call the endpoint task answered. The line is
    /// already open, so the bridge settles the tier, opens the model
    /// session and stays on the call until it ends. The Agent, the
    /// Run and the standing brief are resolved by the caller, the way
    /// the call tool resolves them for an outbound call.
    async fn answer(
        &self,
        incoming: IncomingHub,
        call: PlacedCall,
        log: &CallLog,
    ) -> Result<CallReport, BridgeError>;
}

/// A bridge that answers from a script. It keeps every call it was
/// handed, so a test reads the brief and the tools the daemon wrote.
pub struct FakeCallBridge {
    script: Mutex<Result<CallReport, BridgeError>>,
    placed: Mutex<Vec<PlacedCall>>,
    answered: Mutex<Vec<PlacedCall>>,
}

impl FakeCallBridge {
    /// A bridge whose every call ends with this report.
    pub fn reporting(report: CallReport) -> Self {
        Self {
            script: Mutex::new(Ok(report)),
            placed: Mutex::new(Vec::new()),
            answered: Mutex::new(Vec::new()),
        }
    }

    /// A bridge whose every call fails with this reason.
    pub fn failing(reason: &str) -> Self {
        Self {
            script: Mutex::new(Err(BridgeError(reason.to_string()))),
            placed: Mutex::new(Vec::new()),
            answered: Mutex::new(Vec::new()),
        }
    }

    /// A call that a person answered and that ended with a hang-up.
    pub fn answered_report(transcript: &str) -> CallReport {
        CallReport {
            outcome: CallOutcome::Answered,
            tier: TrustTier::Unknown,
            ended_reason: "hangup".to_string(),
            classification: Some(Classification::Human),
            message_left: false,
            duration: Duration::from_secs(42),
            ringing_at: None,
            answered_at: Some(pagis_core::now_ms()),
            transcript: vec![TranscriptLine::new(
                pagis_core::now_ms(),
                pagis_core::Speaker::Caller,
                transcript,
            )],
            recording_artifact_id: None,
        }
    }

    pub fn placed(&self) -> Vec<PlacedCall> {
        self.placed.lock().unwrap().clone()
    }

    /// Every inbound call the bridge was handed, in order.
    pub fn answered(&self) -> Vec<PlacedCall> {
        self.answered.lock().unwrap().clone()
    }
}

#[async_trait]
impl CallBridge for FakeCallBridge {
    async fn place(&self, call: PlacedCall, log: &CallLog) -> Result<CallReport, BridgeError> {
        self.placed.lock().unwrap().push(call);
        let report = self.script.lock().unwrap().clone()?;
        if report.outcome == CallOutcome::Answered || report.outcome == CallOutcome::Voicemail {
            log.answered()
                .await
                .map_err(|error| BridgeError(error.to_string()))?;
        }
        Ok(report)
    }

    /// The fake takes the call and hangs up at once, so the line is
    /// free again and the Remote Party hears the end. It returns once
    /// the hub has ended, so the next call to the number finds the line
    /// free.
    async fn answer(
        &self,
        incoming: IncomingHub,
        call: PlacedCall,
        log: &CallLog,
    ) -> Result<CallReport, BridgeError> {
        self.answered.lock().unwrap().push(call);
        incoming.hub.hangup().await;
        // A hub whose task is gone never reports the end, and the
        // line is free then as well.
        let _ = incoming
            .hub
            .watch_ended()
            .wait_for(|ended| ended.is_some())
            .await;
        let report = self.script.lock().unwrap().clone()?;
        log.answered()
            .await
            .map_err(|error| BridgeError(error.to_string()))?;
        Ok(report)
    }
}
