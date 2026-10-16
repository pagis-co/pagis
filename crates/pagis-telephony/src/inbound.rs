//! Inbound Calls run under the Chief of Staff and can move to another Agent of the Workspace.

use std::sync::Arc;

use async_trait::async_trait;
use object_store::ObjectStore;
use pagis_core::{
    AgentId, AgentStore, CallDirection, CallId, EventBus, FileId, FileStore, IngestBatch,
    MessageStore, PhoneNumber, PhoneNumberId, PhoneNumberStore, Run, RunId, RunStore, TrustTier,
    WorkspaceId, now_ms,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::IncomingHub;
use crate::bridge::{CallBridge, CallLog, PlacedCall};
use crate::brief::CallBrief;
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
        caller: &str,
    ) -> Result<Run, String>;
    /// The Run ended: the call settled, or it failed for this reason.
    async fn close(&self, workspace_id: &WorkspaceId, run_id: &RunId, error: Option<String>);
}

pub struct InboundCallsDeps {
    pub numbers: Arc<dyn PhoneNumberStore>,
    pub calls: Arc<dyn pagis_core::CallStore>,
    pub agents: Arc<dyn AgentStore>,
    pub workspaces: Arc<dyn pagis_core::WorkspaceStore>,
    pub users: Arc<dyn pagis_core::UserStore>,
    pub runs: Arc<dyn InboundRuns>,
    pub run_tools: Arc<dyn RunTools>,
    pub run_records: Arc<dyn RunStore>,
    pub messages: Arc<dyn MessageStore>,
    pub transport: Arc<dyn CallTransport>,
    pub bridge: Arc<dyn CallBridge>,
    pub live: Arc<LiveCalls>,
    pub files: Arc<dyn FileStore>,
    pub blobs: Arc<dyn ObjectStore>,
    pub bus: Arc<dyn EventBus>,
    /// Where the `call.ended` event goes when the call settles. It is
    /// the Run that reads the call afterwards (ADR-0021).
    pub events: Arc<dyn CallEvents>,
    pub standing_rule: Arc<dyn crate::events::StandingCallRule>,
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
    ) -> crate::task::JoinHandle<()> {
        crate::task::spawn(async move {
            loop {
                let incoming = tokio::select! {
                    incoming = answered.recv() => incoming,
                    () = cancel.cancelled() => None,
                };
                let Some(incoming) = incoming else { return };
                let runner = Arc::clone(&self);
                crate::task::spawn(async move {
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
        let agent_id = self
            .deps
            .workspaces
            .get(&number.workspace_id)
            .await
            .map_err(|error| InboundError(error.to_string()))?
            .and_then(|workspace| workspace.chief_of_staff_agent_id)
            .ok_or_else(|| InboundError("this Workspace has no Chief of Staff".into()))?;
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
            .open(
                &number.workspace_id,
                &agent.id,
                &call_id,
                &incoming.from_e164,
            )
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
        let (agent, run) = match self.deps.calls.get(&number.workspace_id, &call_id).await {
            Ok(Some(call)) if call.run_id != run.id => {
                let agent = self
                    .deps
                    .agents
                    .get(&number.workspace_id, &call.agent_id)
                    .await
                    .map_err(|error| InboundError(error.to_string()))?
                    .ok_or_else(|| InboundError("Call Agent not found".into()))?;
                let run = self
                    .deps
                    .run_records
                    .get(&number.workspace_id, &call.run_id)
                    .await
                    .map_err(|error| InboundError(error.to_string()))?
                    .ok_or_else(|| InboundError("Call Run not found".into()))?;
                (agent, run)
            }
            _ => (agent, run),
        };
        let mut log = log;
        log.set_holder(agent.id.clone(), run.id.clone());
        self.ensure_standing_rule(&number, &agent.id).await?;
        let transcript_id = store_transcript(
            self.deps.files.as_ref(),
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

    /// The Standing Call Rule follows the settled holder, because a hand-off
    /// moves the Call and its follow-up work to that Agent's Run.
    async fn ensure_standing_rule(
        &self,
        number: &PhoneNumber,
        agent: &AgentId,
    ) -> Result<(), InboundError> {
        self.deps
            .standing_rule
            .create(number, agent)
            .await
            .map_err(|error| InboundError(error.to_string()))
    }

    /// Hand one finished call to the Trigger module.
    async fn report(
        &self,
        number: &PhoneNumber,
        agent_id: &AgentId,
        call_id: &CallId,
        from_e164: &str,
        report: &crate::bridge::CallReport,
        transcript_id: Option<&FileId>,
    ) {
        let at = now_ms();
        let batch = IngestBatch {
            workspace_id: number.workspace_id.clone(),
            source: pagis_core::EventSource::connection(number.connection_id.clone()),
            // Only the settled holder's rules read this Call.
            agent_id: Some(agent_id.clone()),
            event_kind: CALL_ENDED.to_string(),
            cursor: None,
            events: vec![call_ended_event(&SettledCall {
                call_id,
                direction: CallDirection::Inbound,
                number,
                remote_e164: from_e164,
                report,
                transcript_file_id: transcript_id,
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
        let person_name = pagis_core::honesty::person_name(
            self.deps.workspaces.as_ref(),
            self.deps.users.as_ref(),
            &run.workspace_id,
        )
        .await
        .map_err(|error| InboundError(error.to_string()))?;
        let brief = CallBrief::inbound(
            agent,
            &person_name,
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

    /// Read the active Workspace Number again before the Call runs.
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
            .filter(|number| number.status == pagis_core::PhoneNumberStatus::Active)
            .ok_or_else(|| InboundError(format!("{id} is not an active Workspace Number")))
    }
}
