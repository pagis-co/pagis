//! Events: the audit/outbox stream shared by every module. A publication
//! appends an event, and only a Forget changes one (ADR-0008).

use serde::{Deserialize, Serialize};

use crate::id::{AgentId, ChannelId, EventId, RunId, WorkspaceId};
use crate::time::UnixMillis;

/// An event as submitted for publication. The event log assigns id,
/// sequence number, and timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewEvent {
    pub workspace_id: WorkspaceId,
    pub event_type: String,
    pub agent_id: Option<AgentId>,
    pub run_id: Option<RunId>,
    pub channel_id: Option<ChannelId>,
    pub payload: serde_json::Value,
}

/// A persisted event. `seq` is the log position: strictly increasing,
/// assigned by the event log, the cursor for catch-up reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    pub seq: i64,
    pub workspace_id: WorkspaceId,
    pub event_type: String,
    pub agent_id: Option<AgentId>,
    pub run_id: Option<RunId>,
    pub channel_id: Option<ChannelId>,
    pub payload: serde_json::Value,
    pub created_at: UnixMillis,
}
