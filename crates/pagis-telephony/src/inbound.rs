//! The runner of the calls the lines answer. The line routes a call by
//! its dialed number, answers it and hands the hub over; this is what
//! runs it. A call nobody placed has no Run and no Thread, so the
//! runner opens a Run for the Agent that holds the number, in its
//! Thread with the user (ADR-0022), writes the Call Brief from the
//! standing brief, marks the number live, hands the call to the bridge
//! and settles the transcript, the way the call tool does for an
//! outbound call.

use std::sync::Arc;

use async_trait::async_trait;
use object_store::ObjectStore;
use pagis_core::{
    AgentId, AgentStore, ArtifactId, ArtifactStore, CallDirection, CallId, EventBus, IngestBatch,
    MessageStore, PhoneNumber, PhoneNumberId, PhoneNumberStore, Run, RunId, RunStore, TrustTier,
    WorkspaceId, now_ms,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::bridge::{CallBridge, CallLog, PlacedCall};
use crate::brief::CallBrief;
use crate::endpoint::IncomingHub;
use crate::events::{CALL_ENDED, CallEvents, SettledCall, call_ended_event};
use crate::live::LiveCalls;
use crate::phone_tool::{RunTools, mint_strip, store_transcript};
use crate::session::session_tools;
use crate::transport::CallTransport;

/// The Run of a call the Agent answered (ADR-0020). The daemon opens
/// it in the Agent's Thread with the user and binds its Capability
/// Snapshot, so the call's free tools come from the same place an
/// outbound call's do.
#[async_trait]
pub trait InboundRuns: Send + Sync {
    /// Open a running Run for the Agent, referenced to the Call.
    async fn open(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        call_id: &CallId,
    ) -> Result<Run, String>;
    /// The Run ended: the call settled, or it failed for this reason.
    async fn close(&self, workspace_id: &WorkspaceId, run_id: &RunId, error: Option<String>);
}

pub struct InboundCallsDeps {
    pub numbers: Arc<dyn PhoneNumberStore>,
    pub agents: Arc<dyn AgentStore>,
    pub runs: Arc<dyn InboundRuns>,
    pub run_tools: Arc<dyn RunTools>,
    pub run_records: Arc<dyn RunStore>,
    pub messages: Arc<dyn MessageStore>,
    pub transport: Arc<dyn CallTransport>,
    pub bridge: Arc<dyn CallBridge>,
    pub live: Arc<LiveCalls>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub blobs: Arc<dyn ObjectStore>,
    pub bus: Arc<dyn EventBus>,
    /// Where the `call.ended` event goes when the call settles. It is
    /// the Run that reads the call afterwards (ADR-0021).
    pub events: Arc<dyn CallEvents>,
}

pub struct InboundCalls {
    deps: InboundCallsDeps,
}

/// Why an answered call did not run. The hub is hung up either way.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InboundError(pub String);

impl InboundCalls {
    pub fn new(deps: InboundCallsDeps) -> Self {
        Self { deps }
    }

    /// Drain the answered calls until the daemon stops. Each call runs
    /// in its own task, so a second line takes its call while the
    /// first is on one.
    pub fn spawn(
        self: Arc<Self>,
        mut answered: mpsc::Receiver<IncomingHub>,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let incoming = tokio::select! {
                    incoming = answered.recv() => incoming,
                    () = cancel.cancelled() => None,
                };
                let Some(incoming) = incoming else { return };
                let runner = Arc::clone(&self);
                tokio::spawn(async move {
                    if let Err(error) = runner.answer(incoming).await {
                        tracing::error!(%error, "an answered call did not run");
                    }
                });
            }
        })
    }

    /// Run one answered call to its end, and answer the Call id. A
    /// call that cannot run is hung up, so the line is free again.
    pub async fn answer(&self, incoming: IncomingHub) -> Result<CallId, InboundError> {
        let hub = Arc::clone(&incoming.hub);
        match self.run(incoming).await {
            Ok(call_id) => Ok(call_id),
            Err(error) => {
                hub.hangup().await;
                Err(error)
            }
        }
    }

    async fn run(&self, incoming: IncomingHub) -> Result<CallId, InboundError> {
        let number = self
            .held_number(&incoming.workspace_id, &incoming.phone_number_id)
            .await?;
        let agent_id = number
            .agent_id
            .clone()
            .ok_or_else(|| InboundError(format!("{} is held by no Agent", number.e164)))?;
        let agent = self
            .deps
            .agents
            .get(&number.workspace_id, &agent_id)
            .await
            .map_err(|error| InboundError(error.to_string()))?
            .ok_or_else(|| InboundError("the Agent is gone".to_string()))?;
        let call_id = CallId::generate();
        let run = self
            .deps
            .runs
            .open(&number.workspace_id, &agent.id, &call_id)
            .await
            .map_err(InboundError)?;
        let placed = match self.brief(&incoming, &agent, &number, &run, &call_id).await {
            Ok(placed) => placed,
            Err(error) => {
                self.deps
                    .runs
                    .close(&run.workspace_id, &run.id, Some(error.0.clone()))
                    .await;
                return Err(error);
            }
        };
        let _live = match self.deps.live.begin(
            &placed.workspace_id,
            &number.id,
            &placed.id,
            placed.tools.clone(),
        ) {
            Ok(guard) => guard,
            Err(active) => {
                let error =
                    InboundError(format!("this line is on call {} already", active.call_id));
                self.deps
                    .runs
                    .close(&run.workspace_id, &run.id, Some(error.0.clone()))
                    .await;
                return Err(error);
            }
        };
        let log = CallLog::new(Arc::clone(&self.deps.bus), &placed);
        if let Err(error) = log.placed(&placed).await {
            tracing::error!(%error, "the call.placed event did not reach the audit log");
        }
        mint_strip(
            self.deps.messages.as_ref(),
            self.deps.run_records.as_ref(),
            self.deps.bus.as_ref(),
            &number.workspace_id,
            &agent.id,
            &run.id,
            &call_id,
        )
        .await;
        // The bridge consumes the answered call, so the caller's
        // number is read before it goes.
        let from_e164 = incoming.from_e164.clone();
        let report = match self.deps.bridge.answer(incoming, placed, &log).await {
            Ok(report) => report,
            Err(error) => {
                if let Err(error) = log.failed(&error).await {
                    tracing::error!(%error, "the call.ended event did not reach the audit log");
                }
                self.deps
                    .runs
                    .close(&run.workspace_id, &run.id, Some(error.0.clone()))
                    .await;
                return Err(InboundError(error.0));
            }
        };
        let transcript_id = store_transcript(
            self.deps.artifacts.as_ref(),
            self.deps.blobs.as_ref(),
            &run.workspace_id,
            &agent.id,
            &run.id,
            &report,
        )
        .await;
        let transcript_id = match transcript_id {
            Ok(id) => Some(id),
            Err(error) => {
                tracing::error!(%error, "the transcript of the call was not stored");
                None
            }
        };
        if let Err(error) = log.ended(&report, transcript_id.as_ref()).await {
            tracing::error!(%error, "the call.ended event did not reach the audit log");
        }
        self.deps.runs.close(&run.workspace_id, &run.id, None).await;
        // The call is over and its record is written. Now the Agent
        // reads it: the Standing Call Rule matches this event and its
        // Wake-up starts the Run that takes the message somewhere
        // (ADR-0020). A call the Agent cannot be told about is still a
        // call that happened, so a refused batch is logged and never
        // fails the call.
        self.report(
            &number,
            &agent.id,
            &call_id,
            &from_e164,
            &report,
            transcript_id.as_ref(),
        )
        .await;
        Ok(call_id)
    }

    /// Hand one finished call to the Trigger module.
    async fn report(
        &self,
        number: &PhoneNumber,
        agent_id: &AgentId,
        call_id: &CallId,
        from_e164: &str,
        report: &crate::bridge::CallReport,
        transcript_id: Option<&ArtifactId>,
    ) {
        let at = now_ms();
        let batch = IngestBatch {
            workspace_id: number.workspace_id.clone(),
            connection_id: number.connection_id.clone(),
            // The desk line is one Agent's own identity, so only that
            // Agent's rules read the call (ADR-0019).
            agent_id: Some(agent_id.clone()),
            event_kind: CALL_ENDED.to_string(),
            cursor: None,
            events: vec![call_ended_event(&SettledCall {
                call_id,
                direction: CallDirection::Inbound,
                number,
                remote_e164: from_e164,
                report,
                transcript_artifact_id: transcript_id,
                ended_at: at,
            })],
            received_at: at,
            baseline: false,
        };
        if let Err(error) = self.deps.events.ingest(batch).await {
            tracing::error!(%error, "the finished call did not reach the Trigger module");
        }
    }

    /// The Call Brief of the answered call (ADR-0020): the standing
    /// brief on the Agent, read now, and the Run's free tools. The
    /// tier is Unknown until the bridge settles it on the answered
    /// call (ADR-0021), so the session tools carry no brief tool yet.
    async fn brief(
        &self,
        incoming: &IncomingHub,
        agent: &pagis_core::Agent,
        number: &PhoneNumber,
        run: &Run,
        call_id: &CallId,
    ) -> Result<PlacedCall, InboundError> {
        let free_tools = self
            .deps
            .run_tools
            .free_tools(&run.workspace_id, &run.id)
            .await
            .map_err(InboundError)?;
        let brief = CallBrief::inbound(
            agent,
            number,
            &incoming.from_e164,
            TrustTier::Unknown,
            free_tools,
        );
        let tools = session_tools(&brief, self.deps.transport.capabilities(), None);
        Ok(PlacedCall {
            id: call_id.clone(),
            workspace_id: run.workspace_id.clone(),
            run_id: run.id.clone(),
            brief,
            tools,
        })
    }

    /// The number the call was dialed to, read again now. The line
    /// answers only a number an Agent holds, so a number that no Agent
    /// holds now was unassigned while the call rang.
    ///
    /// The carrier names an E.164 and no tenant. The line found the
    /// stored record of the dialed number, and that record names the
    /// Workspace; every read after this one uses that Workspace.
    async fn held_number(
        &self,
        workspace_id: &WorkspaceId,
        id: &PhoneNumberId,
    ) -> Result<PhoneNumber, InboundError> {
        self.deps
            .numbers
            .get(workspace_id, id)
            .await
            .map_err(|error| InboundError(error.to_string()))?
            .ok_or_else(|| InboundError(format!("{id} is not a number of this Workspace")))
    }
}
