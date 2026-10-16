//! Call transcript reads and Call file storage.

use std::sync::Arc;

use async_trait::async_trait;
use object_store::{ObjectStore, ObjectStoreExt as _};
use pagis_broker::{AuthorizedCall, CoreTool, ToolDef, ToolExecutor, ToolResult, ToolRoute};
use pagis_core::{
    AgentId, Author, Block, CallId, CallStore, EventBus, File, FileId, FileOutcome, FileStore,
    KnownBlock, Message, MessageId, MessageStatus, MessageStore, NewEvent, RunId, RunStore,
    Untrusted, WorkspaceId, blocks_text, now_ms, render_transcript,
};

use crate::bridge::CallReport;
use crate::brief::REMOTE_PARTY_SOURCE;
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
    pub calls: Arc<dyn CallStore>,
    pub blobs: Arc<dyn ObjectStore>,
    pub files: Arc<dyn FileStore>,
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
                tool: CoreTool::CallTranscript
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
                "recording_file_id": record.recording_file_id.as_ref().map(FileId::as_str),
            })
            .to_string(),
        ))
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
        author: Author::System,
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
                "author": {"kind": "system", "id": null, "name": "Pagis"},
            }),
        })
        .await;
    if let Err(error) = published {
        tracing::error!(%error, "the call block did not reach the Thread");
    }
}

/// The transcript is an ordinary File (ADR-0020): bytes to the
/// blob store, metadata to the row.
pub(crate) async fn store_transcript(
    files: &dyn FileStore,
    blobs: &dyn ObjectStore,
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    run_id: &RunId,
    report: &CallReport,
) -> Result<FileId, String> {
    use sha2::Digest as _;

    let transcript = render_transcript(&report.transcript);
    let bytes = transcript.as_bytes();
    let sha256 = format!("{:x}", sha2::Sha256::digest(bytes));
    let storage_key = format!("{workspace_id}/{sha256}");
    let file = File {
        id: FileId::generate(),
        workspace_id: workspace_id.clone(),
        creator_agent_id: Some(agent_id.clone()),
        run_id: Some(run_id.clone()),
        kind: pagis_core::FileKind::CallTranscript,
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
    match files
        .insert(&file)
        .await
        .map_err(|error| error.to_string())?
    {
        FileOutcome::Created(file) | FileOutcome::Deduplicated(file) => Ok(file.id),
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
            _ => unreachable!(),
        };
        match outcome {
            Ok(result) | Err(result) => result,
        }
    }
}
