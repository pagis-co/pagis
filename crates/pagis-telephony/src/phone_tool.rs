//! The `phone_call` executor (ADR-0020). The broker has already
//! validated the arguments and minted the approval card; this is what
//! runs after the user approves. It guards the number, writes the Call
//! Brief, binds the session tools, marks the line live, hands the call
//! to the bridge and turns the report into the tool result. The Run
//! stays alive for the length of the call, because this call blocks
//! until the bridge returns.

use std::sync::Arc;

use async_trait::async_trait;
use object_store::{ObjectStore, ObjectStoreExt as _};
use pagis_broker::{AuthorizedCall, CoreTool, ToolDef, ToolExecutor, ToolResult, ToolRoute};
use pagis_core::{
    AgentId, AgentStore, Artifact, ArtifactId, ArtifactOutcome, ArtifactStore, AuthorKind, Block,
    CallId, CallStore, EventBus, KnownBlock, Message, MessageId, MessageStatus, MessageStore,
    NewEvent, PhoneNumber, RunId, RunStore, TrustListStore, Untrusted, WorkspaceId, blocks_text,
    now_ms, render_transcript,
};

use crate::bridge::{CallBridge, CallLog, CallReport, PlacedCall};
use crate::brief::{CallArguments, CallBrief, REMOTE_PARTY_SOURCE};
use crate::emergency::EmergencyRefused;
use crate::live::LiveCalls;
use crate::numbers::{NumberDesk, NumberError};
use crate::session::session_tools;
use crate::tiers::candidate_tier;
use crate::transport::CallTransport;

/// The stable reason a call attempt fails before it is placed: the
/// Agent holds no number (ADR-0018).
pub const PHONE_NOT_ASSIGNED: &str = "phone_not_assigned";
/// The stable reason a call attempt fails while another call runs on
/// the same line (ADR-0020).
pub const CALL_ACTIVE: &str = "call_active";
/// The stable reason the bridge did not carry the call to an end.
pub const CALL_FAILED: &str = "call_failed";
/// The stable reason a transcript read names a Call the Agent does not
/// hold, or no Call at all.
pub const CALL_NOT_FOUND: &str = "call_not_found";

/// The tools of one Run that need no approval. The broker answers
/// from the Run's Capability Snapshot; the call tool never sees the
/// snapshot itself.
#[async_trait]
pub trait RunTools: Send + Sync {
    async fn free_tools(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        run_id: &RunId,
    ) -> Result<Vec<ToolDef>, String>;
}

pub struct PhoneToolDeps {
    pub desk: Arc<NumberDesk>,
    pub agents: Arc<dyn AgentStore>,
    pub runs: Arc<dyn RunTools>,
    /// The Run record, for the Thread the call was sent from.
    pub run_records: Arc<dyn RunStore>,
    /// The Thread the `call` block is minted in (ADR-0020).
    pub messages: Arc<dyn MessageStore>,
    /// The Call records. `call_transcript` reads one of them: the
    /// record is the one source of truth for a call, transcript and
    /// tier together.
    pub calls: Arc<dyn CallStore>,
    /// The Trust List (ADR-0021). The dialed number's listed tier
    /// holds on an outbound call, with no further proof.
    pub tiers: Arc<dyn TrustListStore>,
    pub transport: Arc<dyn CallTransport>,
    pub bridge: Arc<dyn CallBridge>,
    pub live: Arc<LiveCalls>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub blobs: Arc<dyn ObjectStore>,
    pub bus: Arc<dyn EventBus>,
}

pub struct PhoneToolRuntime {
    deps: PhoneToolDeps,
}

impl PhoneToolRuntime {
    pub fn new(deps: PhoneToolDeps) -> Self {
        Self { deps }
    }

    /// Whether a route is this runtime's to execute.
    pub fn owns(route: &ToolRoute) -> bool {
        matches!(
            route,
            ToolRoute::Core {
                tool: CoreTool::PhoneCall | CoreTool::CallTranscript
            }
        )
    }

    /// Read what was said on one of the Agent's own Calls (ADR-0020,
    /// ADR-0021). A Call the Agent answered reports itself through a
    /// Wake-up that carries the envelope alone, so this is where the
    /// words come from. They arrive inside the untrusted envelope with
    /// the tier they were spoken at, exactly as an outbound call's
    /// transcript does.
    async fn transcript(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let call_id = call.arguments["call_id"].as_str().ok_or_else(|| {
            ToolResult::error(
                "invalid_request",
                "call_id must be the id of one of your calls",
            )
        })?;
        let record = self
            .deps
            .calls
            .get(&call.workspace_id, &CallId::from(call_id.to_string()))
            .await
            .map_err(unavailable)?
            // An Agent reads its own calls and no other Agent's. A
            // Call of another Agent reads as one that does not exist,
            // so the tool tells a caller nothing about it.
            .filter(|record| record.agent_id == call.agent_id)
            .ok_or_else(|| {
                ToolResult::error(CALL_NOT_FOUND, format!("you have no call {call_id}"))
            })?;
        Ok(ToolResult::success(
            serde_json::json!({
                "call_id": record.id.as_str(),
                "direction": record.direction,
                "from": record.remote_e164,
                "purpose": record.purpose,
                "tier": record.tier.as_str(),
                "outcome": record.outcome,
                "ended_reason": record.ended_reason,
                "answered_at": record.answered_at,
                "ended_at": record.ended_at,
                "transcript": Untrusted::text(
                    REMOTE_PARTY_SOURCE,
                    record.tier.as_str(),
                    &render_transcript(&record.transcript),
                ),
                "recording_artifact_id": record.recording_artifact_id.as_ref().map(ArtifactId::as_str),
            })
            .to_string(),
        ))
    }

    async fn place(&self, call: &AuthorizedCall) -> Result<ToolResult, ToolResult> {
        let arguments: CallArguments =
            serde_json::from_value(call.arguments.clone()).map_err(|error| {
                ToolResult::error("invalid_request", format!("{}: {error}", call.tool_name))
            })?;
        let number = self.guarded_number(call, &arguments.to).await?;
        let agent = self
            .deps
            .agents
            .get(&call.workspace_id, &call.agent_id)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| ToolResult::error("temporarily_unavailable", "the Agent is gone"))?;
        let free_tools = self
            .deps
            .runs
            .free_tools(&call.workspace_id, &call.run_id)
            .await
            .map_err(|reason| ToolResult::error("temporarily_unavailable", reason))?;
        let tier = candidate_tier(
            self.deps.tiers.as_ref(),
            &call.workspace_id,
            &call.agent_id,
            &arguments.to,
        )
        .await
        .map_err(unavailable)?;
        let brief = CallBrief::outbound(&agent, &number, &arguments, tier, free_tools);
        let tools = session_tools(&brief, self.deps.transport.capabilities(), None);
        let placed = PlacedCall {
            id: CallId::generate(),
            workspace_id: call.workspace_id.clone(),
            run_id: call.run_id.clone(),
            brief,
            tools,
        };
        let _live = self
            .deps
            .live
            .begin(
                &placed.workspace_id,
                &number.id,
                &placed.id,
                placed.tools.clone(),
            )
            .map_err(|active| {
                ToolResult::error(
                    CALL_ACTIVE,
                    format!("this line is on call {} already", active.call_id),
                )
            })?;
        let log = CallLog::new(Arc::clone(&self.deps.bus), &placed);
        log.placed(&placed).await.map_err(unavailable)?;
        let call_id = placed.id.clone();
        mint_strip(
            self.deps.messages.as_ref(),
            self.deps.run_records.as_ref(),
            self.deps.bus.as_ref(),
            &call.workspace_id,
            &call.agent_id,
            &call.run_id,
            &call_id,
        )
        .await;
        let tier = placed.brief.tier;
        let report = match self.deps.bridge.place(placed, &log).await {
            Ok(report) => report,
            Err(error) => {
                log.failed(&error).await.map_err(unavailable)?;
                return Err(ToolResult::error(CALL_FAILED, error.to_string()));
            }
        };
        let transcript_id = store_transcript(
            self.deps.artifacts.as_ref(),
            self.deps.blobs.as_ref(),
            &call.workspace_id,
            &call.agent_id,
            &call.run_id,
            &report,
        )
        .await
        .map_err(unavailable)?;
        log.ended(&report, Some(&transcript_id))
            .await
            .map_err(unavailable)?;
        Ok(ToolResult::success(
            serde_json::json!({
                "call_id": call_id.as_str(),
                "outcome": report.outcome,
                "ended_reason": report.ended_reason,
                "classification": report.classification,
                "message_left": report.message_left,
                "duration_s": report.duration.as_secs(),
                "transcript_artifact_id": transcript_id.as_str(),
                "tier": tier.as_str(),
                "transcript": Untrusted::text(
                    REMOTE_PARTY_SOURCE,
                    tier.as_str(),
                    &render_transcript(&report.transcript),
                ),
                "recording_artifact_id": report.recording_artifact_id.as_ref().map(ArtifactId::as_str),
            })
            .to_string(),
        ))
    }

    /// The held number, once both guards of ADR-0018 pass: the Agent
    /// holds a line, and the dialed number is not an emergency number.
    async fn guarded_number(
        &self,
        call: &AuthorizedCall,
        to: &str,
    ) -> Result<PhoneNumber, ToolResult> {
        self.deps
            .desk
            .guard_call_tool(&call.workspace_id, &call.agent_id, &call.run_id, to)
            .await
            .map_err(|error| match error {
                NumberError::EmergencyRefused(refused) => ToolResult::error(
                    EmergencyRefused::CODE,
                    format!(
                        "{refused} Pagis never calls an emergency number. The person must dial \
                         it themselves, from a telephone they hold."
                    ),
                ),
                NumberError::Validation(reason) => ToolResult::error(PHONE_NOT_ASSIGNED, reason),
                other => ToolResult::error("temporarily_unavailable", other.to_string()),
            })
    }
}

/// Mint the daemon-owned `call` block in the Thread of the Run
/// (ADR-0020): the Thread that sent the Agent to make the call, or
/// the Agent's Thread with the user for a call it answered
/// (ADR-0022). The block is the strip while the call runs and the
/// settled record afterwards, because it carries the Call id only and
/// the UI reads the record.
///
/// A Run with no channel — a Schedule, an Event Subscription — has
/// no Thread to write in, and the call runs with no strip.
pub(crate) async fn mint_strip(
    messages: &dyn MessageStore,
    run_records: &dyn RunStore,
    bus: &dyn EventBus,
    workspace_id: &pagis_core::WorkspaceId,
    agent_id: &AgentId,
    run_id: &RunId,
    call_id: &CallId,
) {
    let run = match run_records.get(workspace_id, run_id).await {
        Ok(Some(run)) => run,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(%error, "the Run of the call was not read");
            return;
        }
    };
    let Some(channel_id) = run.channel_id.clone() else {
        return;
    };
    let now = now_ms();
    let blocks = vec![Block::from(KnownBlock::Call {
        call_id: call_id.to_string(),
    })];
    let strip = Message {
        id: MessageId::generate(),
        workspace_id: run.workspace_id.clone(),
        channel_id: channel_id.clone(),
        parent_message_id: run.root_message_id.clone(),
        author_kind: AuthorKind::System,
        author_agent_id: None,
        run_id: Some(run_id.clone()),
        status: MessageStatus::Complete,
        text_content: blocks_text(&blocks),
        blocks,
        pending_id: None,
        created_at: now,
        completed_at: Some(now),
    };
    // This strip contains only the daemon's call identity, not conversation text.
    if let Err(error) = messages.insert_stamped(&strip, &[]).await {
        tracing::error!(%error, "the call block was not minted");
        return;
    }
    let published = bus
        .publish(NewEvent {
            workspace_id: strip.workspace_id.clone(),
            event_type: "message.completed".to_string(),
            agent_id: Some(agent_id.clone()),
            run_id: Some(run_id.clone()),
            channel_id: Some(channel_id),
            payload: serde_json::json!({
                "message_id": strip.id.as_str(),
                "parent_message_id": strip.parent_message_id.as_ref().map(MessageId::as_str),
                "author_kind": AuthorKind::System,
            }),
        })
        .await;
    if let Err(error) = published {
        tracing::error!(%error, "the call block did not reach the Thread");
    }
}

/// The transcript is an ordinary Artifact (ADR-0020): bytes to the
/// blob store, metadata to the row.
pub(crate) async fn store_transcript(
    artifacts: &dyn ArtifactStore,
    blobs: &dyn ObjectStore,
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    run_id: &RunId,
    report: &CallReport,
) -> Result<ArtifactId, String> {
    use sha2::Digest as _;

    let transcript = render_transcript(&report.transcript);
    let bytes = transcript.as_bytes();
    let sha256 = format!("{:x}", sha2::Sha256::digest(bytes));
    let storage_key = format!("{workspace_id}/{sha256}");
    let artifact = Artifact {
        id: ArtifactId::generate(),
        workspace_id: workspace_id.clone(),
        creator_agent_id: Some(agent_id.clone()),
        run_id: Some(run_id.clone()),
        kind: pagis_core::ArtifactKind::CallTranscript,
        filename: Some("transcript.txt".to_string()),
        mime: "text/plain".to_string(),
        size_bytes: bytes.len() as i64,
        sha256,
        storage_key: storage_key.clone(),
        created_at: now_ms(),
    };
    blobs
        .put(
            &object_store::path::Path::from(storage_key),
            bytes.to_vec().into(),
        )
        .await
        .map_err(|error| error.to_string())?;
    match artifacts
        .insert(&artifact)
        .await
        .map_err(|error| error.to_string())?
    {
        ArtifactOutcome::Created(artifact) | ArtifactOutcome::Deduplicated(artifact) => {
            Ok(artifact.id)
        }
    }
}

fn unavailable(error: impl std::fmt::Display) -> ToolResult {
    ToolResult::error("temporarily_unavailable", error.to_string())
}

#[async_trait]
impl ToolExecutor for PhoneToolRuntime {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        if !Self::owns(&call.route) {
            return ToolResult::error(
                "invalid_request",
                format!("{} is not a phone tool", call.tool_name),
            );
        }
        let outcome = match call.route {
            ToolRoute::Core {
                tool: CoreTool::CallTranscript,
            } => self.transcript(&call).await,
            _ => self.place(&call).await,
        };
        match outcome {
            Ok(result) | Err(result) => result,
        }
    }
}
