use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ToolKind;

/// Answers the permission requests and the questions of a harness.
///
/// The daemon implements it, so all policy stays outside the crate: the
/// Session Approval Mode, the scope checks, the Host Allow Rules, the
/// Person and the supervising Agent. A call can wait for hours. The crate
/// drops the future of a call when the session is cancelled or the harness
/// withdraws the request, and it then sends `SessionEvent::AskWithdrawn`.
#[async_trait]
pub trait AskHandler: Send + Sync {
    /// Answers one ACP `session/request_permission`.
    async fn permission(&self, ask: PermissionAsk) -> PermissionAnswer;

    /// Answers one ACP `elicitation/create` in form mode.
    async fn question(&self, ask: QuestionAsk) -> QuestionAnswer;
}

/// One permission request of the harness, for one tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionAsk {
    /// The JSON-RPC id of the request, as text.
    pub ask_id: String,
    pub tool_call_id: String,
    /// The title of the tool call, or `None` when the harness sends none.
    pub title: Option<String>,
    /// The ACP tool kind. ACP reads a missing kind as `other`.
    pub kind: ToolKind,
    /// The files that the tool call reads or changes.
    pub locations: Vec<PathBuf>,
    /// The input of the tool call, as the harness sends it.
    pub raw_input: Option<Value>,
}

/// The answer to a permission request. The crate answers with the offered
/// option of the same kind, and never with an "always" option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAnswer {
    AllowOnce,
    RejectOnce,
}

/// One question of the harness, in form mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionAsk {
    /// The JSON-RPC id of the request, as text.
    pub ask_id: String,
    pub message: String,
    /// The `requestedSchema` of the form, as JSON.
    pub schema: Value,
}

/// The answer to a question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionAnswer {
    /// The values of the form. A value that ACP cannot carry (an object, a
    /// null or an array of other than text) makes the answer `cancel`.
    Accept(Map<String, Value>),
    Decline,
    Cancel,
}
