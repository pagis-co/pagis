//! The bridge that runs a real call (ADR-0020): it opens the
//! model session, places or takes the call, pumps it with a
//! [`CallSession`] and settles the Call record.
//!
//! An outbound call opens the model session **before** it dials: if the
//! session does not open, no call is placed. An inbound call is already
//! answered when the endpoint task hands the hub over, so it skips the
//! classify phase and starts in the conversation.
//!
//! The call is recorded from the moment the hub exists. Both
//! directions append raw to disk while the call runs, and the two legs
//! mux to one stereo WAV when the record settles. A call the daemon was
//! killed in the middle of settles the same way at the next start, from
//! [`RealtimeBridge::recover`].

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use object_store::{ObjectStore, ObjectStoreExt as _};
use pagis_core::{
    Artifact, ArtifactId, ArtifactKind, ArtifactOutcome, ArtifactStore, Call, CallOutcome,
    CallState, CallStore, TrustListStore, now_ms,
};
use tokio_util::sync::CancellationToken;

use crate::bridge::{BridgeError, CallBridge, CallLog, CallReport, PlacedCall};
use crate::call_session::{CallSession, CallSessionDeps, DAEMON_RESTART, MODEL_UNAVAILABLE};
use crate::call_tools::CallTools;
use crate::endpoint::{Endpoints, IncomingHub};
use crate::hub::MediaHub;
use crate::keypad::Keypad;
use crate::live::LiveCalls;
use crate::model::ModelSessions;
use crate::recording::{self, CallRecorder, RECORDING_FILENAME, RECORDING_MIME};
use crate::tiers::{self, LiveTiers, TierGate};

/// How long the mux waits for the recorder to write what it still
/// holds. The queue is five seconds deep at most.
const RECORDER_DRAIN: std::time::Duration = std::time::Duration::from_secs(5);
/// Why an inbound call ended when its tier could not be read from the
/// Trust List before the session opened.
const TIER_UNSETTLED: &str = "tier_unsettled";

pub struct RealtimeBridgeDeps {
    pub endpoints: Arc<Endpoints>,
    pub sessions: Arc<dyn ModelSessions>,
    pub tools: Arc<dyn CallTools>,
    pub calls: Arc<dyn CallStore>,
    /// The Artifact rows and the blob store the muxed recording lands
    /// in. The recording is an ordinary Artifact (ADR-0020).
    pub artifacts: Arc<dyn ArtifactStore>,
    pub blobs: Arc<dyn ObjectStore>,
    /// Where the raw G.711 legs are appended while a call runs. They
    /// outlive the process, so a killed daemon still has a recording.
    pub recordings: PathBuf,
    /// Where a live call is found by id, so a listener in the Thread
    /// hears it from the moment the hub exists.
    pub live: Arc<LiveCalls>,
    /// The Trust List the candidate tier of an inbound call comes
    /// from (ADR-0021).
    pub tiers: Arc<dyn TrustListStore>,
    /// The Keypad Code the challenge proves the tier against, and the
    /// failed-attempt count of each Workspace.
    pub keypad: Keypad,
    /// The gates of the live calls, so the user's drop reaches this
    /// call while it runs.
    pub live_tiers: Arc<LiveTiers>,
    /// Cancelled when the daemon stops. Every live call then ends with
    /// `daemon_restart`.
    pub cancel: CancellationToken,
}

pub struct RealtimeBridge {
    deps: RealtimeBridgeDeps,
}

impl RealtimeBridge {
    pub fn new(deps: RealtimeBridgeDeps) -> Self {
        Self { deps }
    }

    async fn open_session(
        &self,
        call: &PlacedCall,
        gate: Option<Arc<TierGate>>,
    ) -> Result<CallSession, BridgeError> {
        CallSession::open(CallSessionDeps {
            workspace_id: call.workspace_id.clone(),
            brief: call.brief.clone(),
            gate,
            run_id: call.run_id.clone(),
            capabilities: self.deps.endpoints.capabilities(),
            sessions: Arc::clone(&self.deps.sessions),
            tools: Arc::clone(&self.deps.tools),
            cancel: self.deps.cancel.clone(),
        })
        .await
    }

    async fn pump(
        &self,
        session: CallSession,
        hub: Arc<MediaHub>,
        call: PlacedCall,
        log: &CallLog,
        recorder: Option<tokio::task::JoinHandle<()>>,
    ) -> CallReport {
        self.deps
            .live
            .attach_hub(&call.workspace_id, &call.id, Arc::clone(&hub));
        let mut report = session.run(hub, log).await;
        self.deps.live.detach_hub(&call.workspace_id, &call.id);
        // The recorder's writer task drains its queue when the hub
        // ends, so the mux waits for it and the tail is not lost.
        if let Some(recorder) = recorder
            && let Err(error) = tokio::time::timeout(RECORDER_DRAIN, recorder).await
        {
            tracing::warn!(%error, "the recorder did not finish; the tail may be short");
        }
        report.recording_artifact_id = self
            .store_recording(
                &call.id,
                &call.workspace_id,
                Some(&call.brief.agent_id),
                Some(&call.run_id),
            )
            .await;
        self.settle(&call, &report).await;
        report
    }

    /// Record both directions from now on. A recorder that does not
    /// open loses the recording and not the call.
    async fn start_recording(
        &self,
        call: &PlacedCall,
        hub: &MediaHub,
    ) -> Option<tokio::task::JoinHandle<()>> {
        match CallRecorder::create(&self.deps.recordings, &call.id, hub.codec()).await {
            Ok(recorder) => Some(hub.record(Box::new(recorder))),
            Err(error) => {
                tracing::error!(%error, "the call is not recorded");
                None
            }
        }
    }

    /// Mux the two raw legs into one stereo WAV and store it as an
    /// Artifact. `None` when the call recorded nothing, or when the
    /// mux or the store failed: the Call record settles either way.
    async fn store_recording(
        &self,
        call_id: &pagis_core::CallId,
        workspace_id: &pagis_core::WorkspaceId,
        agent_id: Option<&pagis_core::AgentId>,
        run_id: Option<&pagis_core::RunId>,
    ) -> Option<ArtifactId> {
        use sha2::Digest as _;

        let wav = match recording::mux(&self.deps.recordings, call_id).await {
            Ok(Some(wav)) => wav,
            Ok(None) => return None,
            Err(error) => {
                tracing::error!(%error, "the recording did not mux");
                return None;
            }
        };
        let sha256 = format!("{:x}", sha2::Sha256::digest(&wav));
        let storage_key = format!("{workspace_id}/{sha256}");
        let artifact = Artifact {
            id: ArtifactId::generate(),
            workspace_id: workspace_id.clone(),
            kind: ArtifactKind::CallRecording,
            creator_agent_id: agent_id.cloned(),
            run_id: run_id.cloned(),
            filename: Some(RECORDING_FILENAME.to_string()),
            mime: RECORDING_MIME.to_string(),
            size_bytes: wav.len() as i64,
            sha256,
            storage_key: storage_key.clone(),
            created_at: now_ms(),
        };
        if let Err(error) = self
            .deps
            .blobs
            .put(&object_store::path::Path::from(storage_key), wav.into())
            .await
        {
            tracing::error!(%error, "the recording did not reach the blob store");
            return None;
        }
        let stored = match self.deps.artifacts.insert(&artifact).await {
            Ok(ArtifactOutcome::Created(artifact) | ArtifactOutcome::Deduplicated(artifact)) => {
                artifact.id
            }
            Err(error) => {
                tracing::error!(%error, "the recording Artifact was not written");
                return None;
            }
        };
        recording::discard(&self.deps.recordings, call_id).await;
        Some(stored)
    }

    /// Settle the calls a stopped daemon left behind (ADR-0020). A call
    /// does not survive a restart, so each unsettled record ends with
    /// `daemon_restart` and its recording muxes from the files on disk.
    /// It answers how many records it settled.
    pub async fn recover(&self) -> Result<usize, pagis_core::StoreError> {
        let unsettled = self.deps.calls.list_unsettled().await?;
        let mut settled = 0;
        for call in unsettled {
            let recording_artifact_id = self
                .store_recording(
                    &call.id,
                    &call.workspace_id,
                    Some(&call.agent_id),
                    Some(&call.run_id),
                )
                .await;
            let outcome = if call.answered_at.is_some() {
                CallOutcome::Answered
            } else {
                CallOutcome::Failed
            };
            let recovered = Call {
                state: CallState::Ended,
                outcome: Some(outcome),
                ended_reason: Some(DAEMON_RESTART.to_string()),
                ended_at: Some(now_ms()),
                recording_artifact_id,
                ..call
            };
            if let Err(error) = self.deps.calls.update(&recovered).await {
                tracing::error!(%error, "an interrupted Call record did not settle");
                continue;
            }
            settled += 1;
        }
        Ok(settled)
    }

    /// Write the Call record as the call starts. A record that cannot
    /// be written does not stop the call: the audio matters more than
    /// the row, and the failure is logged.
    async fn record(&self, call: &PlacedCall, state: CallState, answered_at: Option<i64>) {
        let record = Call {
            id: call.id.clone(),
            workspace_id: call.workspace_id.clone(),
            agent_id: call.brief.agent_id.clone(),
            run_id: call.run_id.clone(),
            phone_number_id: call.brief.phone_number_id.clone(),
            direction: call.brief.direction,
            remote_e164: call.brief.remote_e164.clone(),
            agent_name: call.brief.agent_name.clone(),
            own_e164: call.brief.own_e164.clone(),
            purpose: call.brief.purpose.clone(),
            tools: call
                .brief
                .tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect(),
            tier: call.brief.tier,
            state,
            outcome: None,
            ended_reason: None,
            classification: None,
            message_left: false,
            transcript: Vec::new(),
            recording_artifact_id: None,
            created_at: now_ms(),
            ringing_at: None,
            answered_at,
            ended_at: None,
        };
        if let Err(error) = self.deps.calls.insert(&record).await {
            tracing::error!(%error, "the Call record was not written");
        }
    }

    /// Settle the record of a call that failed before it ran: the
    /// tier of the answered call was not read, or the model session did
    /// not open.
    async fn fail(&self, call: &PlacedCall, reason: &str) {
        let existing = match self.deps.calls.get(&call.workspace_id, &call.id).await {
            Ok(Some(existing)) => existing,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(%error, "the Call record was not read back");
                return;
            }
        };
        let failed = Call {
            state: CallState::Ended,
            outcome: Some(CallOutcome::Failed),
            ended_reason: Some(reason.to_string()),
            ended_at: Some(now_ms()),
            ..existing
        };
        if let Err(error) = self.deps.calls.update(&failed).await {
            tracing::error!(%error, "the failed Call record did not settle");
        }
    }

    /// Settle the record: every Call ends with a reason (ADR-0020).
    async fn settle(&self, call: &PlacedCall, report: &CallReport) {
        let existing = match self.deps.calls.get(&call.workspace_id, &call.id).await {
            Ok(Some(existing)) => existing,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(%error, "the Call record was not read back");
                return;
            }
        };
        let settled = Call {
            state: CallState::Ended,
            outcome: Some(report.outcome),
            ended_reason: Some(report.ended_reason.clone()),
            classification: report.classification,
            message_left: report.message_left,
            transcript: report.transcript.clone(),
            recording_artifact_id: report.recording_artifact_id.clone(),
            ringing_at: report.ringing_at.or(existing.ringing_at),
            answered_at: report.answered_at.or(existing.answered_at),
            ended_at: Some(now_ms()),
            ..existing
        };
        if let Err(error) = self.deps.calls.update(&settled).await {
            tracing::error!(%error, "the Call record did not settle");
        }
    }
}

#[async_trait]
impl CallBridge for RealtimeBridge {
    async fn place(&self, call: PlacedCall, log: &CallLog) -> Result<CallReport, BridgeError> {
        // The model session opens first. A call nobody can talk on is
        // worse than a call that was never placed.
        let session = self.open_session(&call, None).await?;
        self.record(&call, CallState::Dialing, None).await;
        let hub = self
            .deps
            .endpoints
            .place_call(
                &call.workspace_id,
                &call.brief.phone_number_id,
                &call.brief.remote_e164,
            )
            .await
            .map_err(|error| BridgeError(error.as_str().to_string()))?;
        let recorder = self.start_recording(&call, &hub).await;
        Ok(self.pump(session, hub, call, log, recorder).await)
    }

    /// Run a call the endpoint task answered. The Agent, the Run and
    /// the standing brief are resolved by the caller, exactly as the
    /// call tool resolves them for an outbound call.
    async fn answer(
        &self,
        incoming: IncomingHub,
        mut call: PlacedCall,
        log: &CallLog,
    ) -> Result<CallReport, BridgeError> {
        // The recording starts before anything else: the Remote Party
        // speaks as soon as the line is answered, so nothing of the
        // call is lost while the tier settles and the socket comes up.
        // The keypad never reaches it, because a telephone event is
        // not audio and the hub records no telephone event (ADR-0021).
        // The record exists from the answer: the call strip reads it
        // the moment it is minted, and a call that fails before its
        // session opens still settles with a reason (ADR-0020).
        self.record(&call, CallState::Live, Some(now_ms())).await;
        let recorder = self.start_recording(&call, &incoming.hub).await;
        // The tier settles before the model session exists
        // (ADR-0021): the challenge runs on the answered call, and the
        // brief the session opens with carries the tier it proved.
        let gate = match tiers::settle_inbound(
            self.deps.tiers.as_ref(),
            &self.deps.keypad,
            &incoming.hub,
            log,
            &call.workspace_id,
            &call.brief.agent_id,
            &incoming.from_e164,
        )
        .await
        {
            Ok(gate) => gate,
            Err(error) => {
                self.fail(&call, TIER_UNSETTLED).await;
                return Err(BridgeError(error.to_string()));
            }
        };
        call.brief.tier = gate.tier();
        self.deps.live_tiers.bind(
            &call.workspace_id,
            &call.id,
            Arc::clone(&gate),
            Arc::new(log.clone()),
        );
        let session = match self.open_session(&call, Some(Arc::clone(&gate))).await {
            Ok(session) => session,
            Err(error) => {
                self.deps.live_tiers.release(&call.workspace_id, &call.id);
                self.fail(&call, MODEL_UNAVAILABLE).await;
                return Err(error);
            }
        };
        if let Err(error) = log.answered().await {
            tracing::error!(%error, "the answered event did not reach the audit log");
        }
        let report = self
            .pump(session, incoming.hub, call.clone(), log, recorder)
            .await;
        self.deps.live_tiers.release(&call.workspace_id, &call.id);
        Ok(report)
    }
}
