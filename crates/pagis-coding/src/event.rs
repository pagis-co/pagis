use std::path::PathBuf;

use agent_client_protocol::schema::v1 as acp;
use serde::{Deserialize, Serialize};

/// One event of a Coding Session, in the order that the harness sent it.
///
/// One `AcpSession` is one harness process and one ACP session, so an
/// event carries no session id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// One chunk of an agent message. Chunks of one message share their
    /// `message_id` when the harness gives one.
    AgentMessage {
        message_id: Option<String>,
        text: String,
    },
    /// One chunk of the agent's reasoning.
    Thought { text: String },
    /// A new tool call. `raw` is the ACP JSON of the call, with its
    /// content and its diffs.
    ToolCall {
        id: String,
        title: String,
        tool_kind: ToolKind,
        status: ToolStatus,
        locations: Vec<Location>,
        raw: serde_json::Value,
    },
    /// A change to a tool call. A field that is `None` did not change.
    /// `raw` is the ACP JSON of the update.
    ToolCallUpdate {
        id: String,
        title: Option<String>,
        tool_kind: Option<ToolKind>,
        status: Option<ToolStatus>,
        locations: Option<Vec<Location>>,
        raw: serde_json::Value,
    },
    /// The full plan of the agent. Each plan replaces the one before it.
    Plan { entries: Vec<PlanEntry> },
    /// The context window: the tokens in use, its size, and the cost of
    /// the session when the harness reports it.
    Usage {
        used: u64,
        size: u64,
        cost: Option<Cost>,
    },
    /// The turn ended. The updates of the turn always come before it.
    TurnEnded { stop_reason: StopReason },
    /// The prompt request failed. The session can take a new prompt.
    TurnFailed { message: String },
    /// The prompt request failed because the harness needs a Harness
    /// Sign-In.
    SignInRequired,
    /// A permission request or a question that waits for its answer ended
    /// without one: the session was cancelled, or the harness withdrew the
    /// request. `ask_id` is the `ask_id` of its ask.
    AskWithdrawn { ask_id: String },
    /// The connection ended: the incoming side of the stream closed, or
    /// `close` ended it. No event follows.
    Closed,
}

/// The kind of a tool call, as ACP names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

/// The state of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// A file that a tool call reads or changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub path: PathBuf,
    pub line: Option<u32>,
}

/// One entry of the agent's plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub priority: PlanPriority,
    pub status: PlanStatus,
}

/// How much a plan entry matters to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanPriority {
    High,
    Medium,
    Low,
}

/// The state of a plan entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

/// The cost of a session, in the currency that the harness gives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub amount: f64,
    pub currency: String,
}

/// Why a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

impl SessionEvent {
    /// Translates one ACP session update. The updates that Pagis does not
    /// show give `None`.
    pub(crate) fn from_update(update: acp::SessionUpdate) -> Option<Self> {
        match update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => Some(Self::AgentMessage {
                message_id: chunk.message_id.map(|id| id.0.to_string()),
                text: block_text(chunk.content),
            }),
            acp::SessionUpdate::AgentThoughtChunk(chunk) => Some(Self::Thought {
                text: block_text(chunk.content),
            }),
            acp::SessionUpdate::ToolCall(call) => {
                let raw = raw_json(&call);
                Some(Self::ToolCall {
                    id: call.tool_call_id.0.to_string(),
                    title: call.title,
                    tool_kind: ToolKind::from_acp(call.kind),
                    // ACP gives `pending` when a harness leaves the status out.
                    status: ToolStatus::from_acp(call.status).unwrap_or(ToolStatus::Pending),
                    locations: call.locations.into_iter().map(Location::from_acp).collect(),
                    raw,
                })
            }
            acp::SessionUpdate::ToolCallUpdate(update) => {
                let raw = raw_json(&update);
                let fields = update.fields;
                Some(Self::ToolCallUpdate {
                    id: update.tool_call_id.0.to_string(),
                    title: fields.title,
                    tool_kind: fields.kind.map(ToolKind::from_acp),
                    status: fields.status.and_then(ToolStatus::from_acp),
                    locations: fields
                        .locations
                        .map(|locations| locations.into_iter().map(Location::from_acp).collect()),
                    raw,
                })
            }
            acp::SessionUpdate::Plan(plan) => Some(Self::Plan {
                entries: plan
                    .entries
                    .into_iter()
                    .map(|entry| PlanEntry {
                        content: entry.content,
                        priority: PlanPriority::from_acp(&entry.priority),
                        status: PlanStatus::from_acp(&entry.status),
                    })
                    .collect(),
            }),
            acp::SessionUpdate::UsageUpdate(usage) => Some(Self::Usage {
                used: usage.used,
                size: usage.size,
                cost: usage.cost.map(|cost| Cost {
                    amount: cost.amount,
                    currency: cost.currency,
                }),
            }),
            // The prompt, the commands, the mode, the config options and
            // the session title are not part of the transcript.
            _ => None,
        }
    }
}

impl ToolKind {
    pub(crate) fn from_acp(kind: acp::ToolKind) -> Self {
        match kind {
            acp::ToolKind::Read => Self::Read,
            acp::ToolKind::Edit => Self::Edit,
            acp::ToolKind::Delete => Self::Delete,
            acp::ToolKind::Move => Self::Move,
            acp::ToolKind::Search => Self::Search,
            acp::ToolKind::Execute => Self::Execute,
            acp::ToolKind::Think => Self::Think,
            acp::ToolKind::Fetch => Self::Fetch,
            acp::ToolKind::SwitchMode => Self::SwitchMode,
            // ACP reads an unknown kind as `other` too.
            _ => Self::Other,
        }
    }
}

impl ToolStatus {
    /// `None` for a status that a later ACP release adds.
    fn from_acp(status: acp::ToolCallStatus) -> Option<Self> {
        match status {
            acp::ToolCallStatus::Pending => Some(Self::Pending),
            acp::ToolCallStatus::InProgress => Some(Self::InProgress),
            acp::ToolCallStatus::Completed => Some(Self::Completed),
            acp::ToolCallStatus::Failed => Some(Self::Failed),
            _ => None,
        }
    }
}

impl PlanStatus {
    fn from_acp(status: &acp::PlanEntryStatus) -> Self {
        match status {
            acp::PlanEntryStatus::InProgress => Self::InProgress,
            acp::PlanEntryStatus::Completed => Self::Completed,
            // `pending`, and a status that a later ACP release adds.
            _ => Self::Pending,
        }
    }
}

impl PlanPriority {
    fn from_acp(priority: &acp::PlanEntryPriority) -> Self {
        match priority {
            acp::PlanEntryPriority::High => Self::High,
            acp::PlanEntryPriority::Low => Self::Low,
            // `medium`, and a priority that a later ACP release adds.
            _ => Self::Medium,
        }
    }
}

impl Location {
    fn from_acp(location: acp::ToolCallLocation) -> Self {
        Self {
            path: location.path,
            line: location.line,
        }
    }
}

impl StopReason {
    /// `None` for a stop reason that a later ACP release adds.
    pub(crate) fn from_acp(reason: acp::StopReason) -> Option<Self> {
        match reason {
            acp::StopReason::EndTurn => Some(Self::EndTurn),
            acp::StopReason::MaxTokens => Some(Self::MaxTokens),
            acp::StopReason::MaxTurnRequests => Some(Self::MaxTurnRequests),
            acp::StopReason::Refusal => Some(Self::Refusal),
            acp::StopReason::Cancelled => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// The text of a content block. A block that is not text gives one marker
/// line.
fn block_text(block: acp::ContentBlock) -> String {
    match block {
        acp::ContentBlock::Text(text) => text.text,
        acp::ContentBlock::Image(_) => "[image]".to_owned(),
        acp::ContentBlock::Audio(_) => "[audio]".to_owned(),
        acp::ContentBlock::ResourceLink(link) => format!("[resource {}]", link.uri),
        acp::ContentBlock::Resource(resource) => match resource.resource {
            acp::EmbeddedResourceResource::TextResourceContents(contents) => {
                format!("[resource {}]", contents.uri)
            }
            acp::EmbeddedResourceResource::BlobResourceContents(contents) => {
                format!("[resource {}]", contents.uri)
            }
            _ => "[resource]".to_owned(),
        },
        _ => "[content]".to_owned(),
    }
}

/// The ACP JSON of a schema value, such as a tool call.
pub(crate) fn raw_json(value: &impl Serialize) -> serde_json::Value {
    // A schema type has string keys only, so the conversion does not fail.
    serde_json::to_value(value).unwrap_or_else(|error| {
        tracing::warn!(%error, "an ACP value did not convert to JSON");
        serde_json::Value::Null
    })
}
