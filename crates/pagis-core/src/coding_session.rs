//! The Coding Session records (ADR-0033): one ACP session of one Coding
//! Harness that one Agent owns, and its transcript.
//!
//! The record is written again whole as the session moves, as a Call
//! record is (ADR-0020). The transcript is append-only rows of
//! `coding_session_events`, with one exception: a chunk of a message
//! merges into the last row while that row holds the same message.
//! [`fold`] holds the rules of the transcript, and both stores apply it
//! in one write transaction.
//!
//! A Coding Session record and its transcript have no retention, a
//! Backup holds them, and Forget does not reach them. A prompt is the
//! Agent's own words, and the output of a harness is foreign text that
//! no Run read from a source.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::id::{AgentId, ChannelId, CodingSessionId, HostId, MessageId, RunId, WorkspaceId};
use crate::store::StoreError;
use crate::time::UnixMillis;

/// The largest payload of one transcript row: the text of a message
/// row, or the JSON of any other row.
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 16 * 1024;

/// A cut payload stops halving a string at this length.
const MIN_CUT_STRING_BYTES: usize = 256;

/// The fields that a payload keeps when the halving of its strings does
/// not make it fit.
const KEPT_FIELDS: [&str; 5] = ["sessionUpdate", "toolCallId", "kind", "status", "title"];

/// Where the harness of a Coding Session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodingSessionPlace {
    /// A Host: the Client App on the Person's machine.
    Host,
    /// The Computer of the Agent.
    Computer,
}

impl CodingSessionPlace {
    pub fn as_str(self) -> &'static str {
        match self {
            CodingSessionPlace::Host => "host",
            CodingSessionPlace::Computer => "computer",
        }
    }
}

impl std::str::FromStr for CodingSessionPlace {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "host" => Ok(CodingSessionPlace::Host),
            "computer" => Ok(CodingSessionPlace::Computer),
            other => Err(format!("unknown coding session place: {other}")),
        }
    }
}

/// Who answers a Harness Permission of a Coding Session.
// The variants go from the narrowest mode to the widest, and the derived
// order compares them.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SessionApprovalMode {
    /// The Person answers each one.
    Person,
    /// The supervising Agent answers each one.
    Agent,
}

impl SessionApprovalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionApprovalMode::Person => "person",
            SessionApprovalMode::Agent => "agent",
        }
    }

    /// True when this mode, as the widest mode of a host Grant, lets an
    /// Agent start a session in the requested mode: the mode itself or
    /// a narrower one.
    pub fn permits(self, requested: SessionApprovalMode) -> bool {
        requested <= self
    }
}

impl std::str::FromStr for SessionApprovalMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "person" => Ok(SessionApprovalMode::Person),
            "agent" => Ok(SessionApprovalMode::Agent),
            other => Err(format!("unknown session approval mode: {other}")),
        }
    }
}

/// A rule of a host Grant that lets its Agent start the sessions of one
/// Coding Harness in one directory of the Grant's machine, and in each
/// directory under it, with no card (ADR-0033). Only the Person writes
/// one: from the "Always allow" of a start card, or in Settings. It names
/// no mode, so the widest mode of the Grant bounds each session that it
/// lets start.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionAllowRule {
    /// The id of the harness in the Harness Catalog.
    pub harness: String,
    /// An absolute POSIX path, with `.` removed and `..` resolved.
    pub directory: String,
}

impl SessionAllowRule {
    /// The rule for `harness` in `directory`. It refuses a harness that
    /// is not in the Harness Catalog and a directory that is not an
    /// absolute POSIX path, and it stores the directory resolved, so the
    /// Person reads the directory that the rule covers.
    pub fn new(harness: &str, directory: &str) -> Result<Self, String> {
        if crate::harness::entry(harness).is_none() {
            return Err(format!(
                "{harness:?} is not a Coding Harness of the Harness Catalog"
            ));
        }
        let Some(components) = path_components(directory) else {
            return Err(format!("{directory:?} is not an absolute path"));
        };
        Ok(Self {
            harness: harness.to_string(),
            directory: format!("/{}", components.join("/")),
        })
    }

    /// True for the same harness in the rule's directory or a directory
    /// under it. The check is lexical, as the scope check of a Harness
    /// Permission is: the directories are on the Host.
    pub fn covers(&self, harness: &str, directory: &str) -> bool {
        self.harness == harness && path_is_inside(directory, &self.directory)
    }
}

/// True when `path` is `directory` or a path under it, after `.` is
/// removed and `..` is resolved in each. The paths are compared by
/// components, so `/work/pagis2` is not under `/work/pagis`. A relative
/// path is outside, and a relative directory holds nothing.
pub fn path_is_inside(path: &str, directory: &str) -> bool {
    match (path_components(path), path_components(directory)) {
        (Some(path), Some(directory)) => path.starts_with(&directory),
        _ => false,
    }
}

/// The components of an absolute POSIX path, with `.` removed and `..`
/// resolved. A `..` at the root stays at the root, as in POSIX. A
/// relative path gives `None`.
fn path_components(path: &str) -> Option<Vec<&str>> {
    let rest = path.strip_prefix('/')?;
    let mut components = Vec::new();
    for component in rest.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            name => components.push(name),
        }
    }
    Some(components)
}

/// Where a Coding Session is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodingSessionState {
    /// The harness starts and opens its ACP session.
    Starting,
    /// A turn runs.
    Working,
    /// A Harness Permission or a question waits for an answer.
    NeedsDecision,
    /// The turn ended, and the session takes a prompt.
    Idle,
    /// The place was lost. The session can resume.
    Interrupted,
    /// The session ended normally.
    Closed,
    /// The harness failed.
    Failed,
}

impl CodingSessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            CodingSessionState::Starting => "starting",
            CodingSessionState::Working => "working",
            CodingSessionState::NeedsDecision => "needs_decision",
            CodingSessionState::Idle => "idle",
            CodingSessionState::Interrupted => "interrupted",
            CodingSessionState::Closed => "closed",
            CodingSessionState::Failed => "failed",
        }
    }

    /// Whether the session has ended. A terminal session has an end
    /// reason and an end time, and it does not count as open.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            CodingSessionState::Closed | CodingSessionState::Failed
        )
    }
}

impl std::str::FromStr for CodingSessionState {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "starting" => Ok(CodingSessionState::Starting),
            "working" => Ok(CodingSessionState::Working),
            "needs_decision" => Ok(CodingSessionState::NeedsDecision),
            "idle" => Ok(CodingSessionState::Idle),
            "interrupted" => Ok(CodingSessionState::Interrupted),
            "closed" => Ok(CodingSessionState::Closed),
            "failed" => Ok(CodingSessionState::Failed),
            other => Err(format!("unknown coding session state: {other}")),
        }
    }
}

/// What one transcript row holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodingSessionEventKind {
    /// A prompt that Pagis sent.
    Prompt,
    /// Text of the harness to its client.
    AgentMessage,
    /// Reasoning text of the harness.
    Thought,
    ToolCall,
    ToolCallUpdate,
    Plan,
    Usage,
    /// A Harness Permission.
    Permission,
    /// The answer to a Harness Permission.
    Decision,
    /// A question of the harness.
    Question,
    /// The answer to a question.
    Answer,
    TurnEnd,
}

impl CodingSessionEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CodingSessionEventKind::Prompt => "prompt",
            CodingSessionEventKind::AgentMessage => "agent_message",
            CodingSessionEventKind::Thought => "thought",
            CodingSessionEventKind::ToolCall => "tool_call",
            CodingSessionEventKind::ToolCallUpdate => "tool_call_update",
            CodingSessionEventKind::Plan => "plan",
            CodingSessionEventKind::Usage => "usage",
            CodingSessionEventKind::Permission => "permission",
            CodingSessionEventKind::Decision => "decision",
            CodingSessionEventKind::Question => "question",
            CodingSessionEventKind::Answer => "answer",
            CodingSessionEventKind::TurnEnd => "turn_end",
        }
    }

    /// Whether the payload is `{"text", "message_id"}`.
    fn is_text(self) -> bool {
        matches!(
            self,
            CodingSessionEventKind::Prompt
                | CodingSessionEventKind::AgentMessage
                | CodingSessionEventKind::Thought
        )
    }

    /// Whether a chunk merges into the last row of the same message.
    /// Pagis sends each prompt whole, so a prompt never merges.
    fn merges(self) -> bool {
        matches!(
            self,
            CodingSessionEventKind::AgentMessage | CodingSessionEventKind::Thought
        )
    }
}

impl std::str::FromStr for CodingSessionEventKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "prompt" => Ok(CodingSessionEventKind::Prompt),
            "agent_message" => Ok(CodingSessionEventKind::AgentMessage),
            "thought" => Ok(CodingSessionEventKind::Thought),
            "tool_call" => Ok(CodingSessionEventKind::ToolCall),
            "tool_call_update" => Ok(CodingSessionEventKind::ToolCallUpdate),
            "plan" => Ok(CodingSessionEventKind::Plan),
            "usage" => Ok(CodingSessionEventKind::Usage),
            "permission" => Ok(CodingSessionEventKind::Permission),
            "decision" => Ok(CodingSessionEventKind::Decision),
            "question" => Ok(CodingSessionEventKind::Question),
            "answer" => Ok(CodingSessionEventKind::Answer),
            "turn_end" => Ok(CodingSessionEventKind::TurnEnd),
            other => Err(format!("unknown coding session event kind: {other}")),
        }
    }
}

/// What the harness last reported of its context and its cost. Each
/// field is `None` until the harness reports it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CodingSessionUsage {
    /// The tokens in the context window.
    pub context_used: Option<u64>,
    /// The size of the context window, in tokens.
    pub context_size: Option<u64>,
    /// The cost of the session so far.
    pub cost_amount: Option<f64>,
    /// The ISO 4217 code of `cost_amount`.
    pub cost_currency: Option<String>,
}

/// One Harness Mode as the harness names it when a session opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessModeInfo {
    /// The id that `session/set_mode` names.
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

/// One Coding Session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodingSession {
    pub id: CodingSessionId,
    pub workspace_id: WorkspaceId,
    /// The Agent that owns and supervises the session.
    pub agent_id: AgentId,
    /// The id of the Coding Harness in the Harness Catalog.
    pub harness_id: String,
    pub harness_version: String,
    pub place: CodingSessionPlace,
    /// The Host of a `host` session, and `None` for a `computer` one.
    pub host_id: Option<HostId>,
    /// The directory that the Agent named and the card showed.
    pub directory: String,
    /// The directory that the process runs in: the worktree path when
    /// the session has a worktree. `None` until the place opens it.
    pub working_directory: Option<String>,
    pub worktree_branch: Option<String>,
    pub approval_mode: SessionApprovalMode,
    pub title: String,
    pub state: CodingSessionState,
    /// Why the session ended. A terminal session has one, and no other
    /// session has one.
    pub end_reason: Option<String>,
    /// The exit code and the last 4 KB of stderr of a failed harness, or
    /// the message of the request that it failed. It is harness text.
    pub end_detail: Option<String>,
    /// The harness's own ACP session id, which a resume names.
    pub acp_session_id: Option<String>,
    /// The Channel of the Thread that shows the session.
    pub channel_id: ChannelId,
    /// The root message of that Thread.
    pub root_message_id: MessageId,
    /// The message that holds the block of the session.
    pub message_id: MessageId,
    /// The Run that started the session.
    pub run_id: RunId,
    pub usage: CodingSessionUsage,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
    pub ended_at: Option<UnixMillis>,
}

/// The session that holds a token of the Harness Model Endpoint, and
/// what a model call of its harness is charged to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTokenOwner {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub session_id: CodingSessionId,
    /// The Run that started the session. A Usage Record of the session
    /// names it.
    pub run_id: RunId,
}

/// One row of the transcript of a Coding Session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodingSessionEvent {
    pub workspace_id: WorkspaceId,
    pub coding_session_id: CodingSessionId,
    /// The place of the row in its session, from 1.
    pub seq: i64,
    /// The time of the first chunk of the row.
    pub at: UnixMillis,
    pub kind: CodingSessionEventKind,
    pub payload: Value,
}

/// One update of a harness, before [`fold`] places it in the
/// transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct NewCodingSessionEvent {
    pub at: UnixMillis,
    pub kind: CodingSessionEventKind,
    /// `{"text", "message_id"}` for a text kind, and the ACP object with
    /// the ACP field names for each other kind.
    pub payload: Value,
}

/// One write to the transcript that [`fold`] answers.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptWrite {
    /// Write the payload of the last row again. The row keeps its `seq`,
    /// its time and its kind.
    Replace { seq: i64, payload: Value },
    /// Add a row after the last.
    Append {
        seq: i64,
        at: UnixMillis,
        kind: CodingSessionEventKind,
        payload: Value,
    },
}

/// Place one update in the transcript after its last row.
///
/// - A chunk of `agent_message` or `thought` merges into the last row
///   when that row has the same kind and the same `message_id` (an
///   absent id on both sides is the same). A `prompt` never merges.
/// - A row holds at most [`MAX_EVENT_PAYLOAD_BYTES`] of text, cut at a
///   character boundary. The rest goes on in new rows of the same kind
///   with `"continued": true`, so the transcript keeps every word.
/// - The JSON of each other kind is cut to fit the same cap: its longest
///   strings are halved, and if that is not enough it keeps only the
///   fields that name the update. A cut payload carries
///   `"truncated": true`.
pub fn fold(last: Option<&CodingSessionEvent>, new: NewCodingSessionEvent) -> Vec<TranscriptWrite> {
    let next_seq = last.map_or(1, |row| row.seq + 1);
    if !new.kind.is_text() {
        return vec![TranscriptWrite::Append {
            seq: next_seq,
            at: new.at,
            kind: new.kind,
            payload: cut_to_fit(new.payload),
        }];
    }

    let id = message_id(&new.payload);
    let chunk = text(&new.payload);
    let merge_into = last
        .filter(|row| new.kind.merges() && row.kind == new.kind && message_id(&row.payload) == id);
    let mut writes = Vec::new();
    let rest = match merge_into {
        Some(row) => {
            let before = text(&row.payload);
            let joined = format!("{before}{chunk}");
            let (head, rest) = split_at_cap(&joined);
            if head != before {
                let continued = row.payload.get("continued") == Some(&Value::Bool(true));
                writes.push(TranscriptWrite::Replace {
                    seq: row.seq,
                    payload: text_payload(head, id, continued),
                });
            }
            rest.to_string()
        }
        None => {
            let (head, rest) = split_at_cap(chunk);
            writes.push(TranscriptWrite::Append {
                seq: next_seq,
                at: new.at,
                kind: new.kind,
                payload: text_payload(head, id, false),
            });
            rest.to_string()
        }
    };

    let mut seq = next_seq + i64::from(merge_into.is_none());
    let mut rest = rest.as_str();
    while !rest.is_empty() {
        let (head, tail) = split_at_cap(rest);
        writes.push(TranscriptWrite::Append {
            seq,
            at: new.at,
            kind: new.kind,
            payload: text_payload(head, id, true),
        });
        seq += 1;
        rest = tail;
    }
    writes
}

fn text(payload: &Value) -> &str {
    payload
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The message id of a text payload. A `null` id and an absent id are
/// the same: no id.
fn message_id(payload: &Value) -> Option<&str> {
    payload.get("message_id").and_then(Value::as_str)
}

fn text_payload(text: &str, message_id: Option<&str>, continued: bool) -> Value {
    let mut payload = Map::new();
    payload.insert("text".into(), Value::from(text));
    payload.insert("message_id".into(), Value::from(message_id));
    if continued {
        payload.insert("continued".into(), Value::Bool(true));
    }
    Value::Object(payload)
}

/// The text that one row holds, and the rest. The cut falls on a
/// character boundary, so a character is never split.
fn split_at_cap(text: &str) -> (&str, &str) {
    text.split_at(text.floor_char_boundary(MAX_EVENT_PAYLOAD_BYTES))
}

fn encoded_len(payload: &Value) -> usize {
    serde_json::to_vec(payload).map_or(usize::MAX, |bytes| bytes.len())
}

/// Make the JSON of a payload fit [`MAX_EVENT_PAYLOAD_BYTES`].
///
/// The longest string value is halved, with `…` at its end, until the
/// JSON fits or no string is longer than 256 bytes. If it still does not
/// fit, the payload keeps only the fields that name the update. A cut
/// payload carries `"truncated": true`.
fn cut_to_fit(payload: Value) -> Value {
    if encoded_len(&payload) <= MAX_EVENT_PAYLOAD_BYTES {
        return payload;
    }
    let mut object = match payload {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    object.insert("truncated".into(), Value::Bool(true));
    let mut payload = Value::Object(object);
    while encoded_len(&payload) > MAX_EVENT_PAYLOAD_BYTES {
        let longest = longest_string(&payload);
        if longest <= MIN_CUT_STRING_BYTES {
            break;
        }
        halve_string_of_len(&mut payload, longest);
    }
    if encoded_len(&payload) <= MAX_EVENT_PAYLOAD_BYTES {
        return payload;
    }
    let mut kept: Map<String, Value> = KEPT_FIELDS
        .iter()
        .filter_map(|field| Some((field.to_string(), payload.get(*field)?.clone())))
        .collect();
    kept.insert("truncated".into(), Value::Bool(true));
    Value::Object(kept)
}

/// The byte length of the longest string value in a JSON value.
fn longest_string(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len(),
        Value::Array(items) => items.iter().map(longest_string).max().unwrap_or(0),
        Value::Object(fields) => fields.values().map(longest_string).max().unwrap_or(0),
        _ => 0,
    }
}

/// Halve the first string value of `len` bytes, at a character boundary,
/// and end it with `…`. Answer whether a string was halved.
fn halve_string_of_len(value: &mut Value, len: usize) -> bool {
    match value {
        Value::String(text) if text.len() == len => {
            text.truncate(text.floor_char_boundary(len / 2));
            text.push('…');
            true
        }
        Value::Array(items) => items.iter_mut().any(|item| halve_string_of_len(item, len)),
        Value::Object(fields) => fields
            .values_mut()
            .any(|field| halve_string_of_len(field, len)),
        _ => false,
    }
}

#[async_trait]
pub trait CodingSessionStore: Send + Sync {
    async fn insert(&self, session: &CodingSession) -> Result<(), StoreError>;
    /// Write the whole record again. `false` when the Workspace holds no
    /// such session.
    async fn update(&self, session: &CodingSession) -> Result<bool, StoreError>;
    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &CodingSessionId,
    ) -> Result<Option<CodingSession>, StoreError>;
    /// One page of sessions in a Workspace, newest first. Every optional
    /// filter is combined with the others. `before` is exclusive.
    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        state: Option<CodingSessionState>,
        before: Option<&CodingSessionId>,
        limit: u32,
    ) -> Result<Vec<CodingSession>, StoreError>;
    /// The sessions of one Agent that are not terminal.
    async fn count_open(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<u32, StoreError>;
    /// Every session that is not terminal, in every Workspace, oldest
    /// first. The daemon reads them at its start.
    async fn list_open(&self) -> Result<Vec<CodingSession>, StoreError>;
    /// Read the last row of the session, apply [`fold`] and write, in one
    /// write transaction. Answer the rows that it wrote or rewrote. A
    /// session that the Workspace does not hold gets no row and an empty
    /// answer.
    async fn append_event(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        event: NewCodingSessionEvent,
    ) -> Result<Vec<CodingSessionEvent>, StoreError>;
    /// The rows with a `seq` greater than `after`, oldest first. A merge
    /// rewrites a row in place, so a reader that holds row N reads from
    /// `after = N - 1` to see it grow.
    async fn list_events(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<CodingSessionEvent>, StoreError>;
    /// The row with the highest `seq` of the named kinds.
    async fn latest_event(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        kinds: &[CodingSessionEventKind],
    ) -> Result<Option<CodingSessionEvent>, StoreError>;
    /// Write the hash of the token of the Harness Model Endpoint of one
    /// session, in place of the hash that it held. `false` when the
    /// Workspace holds no such session.
    async fn set_model_token(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        hash: &str,
    ) -> Result<bool, StoreError>;
    /// The session that holds the token of `hash`, while its harness
    /// runs: in `starting`, `working`, `needs_decision` or `idle`.
    ///
    /// It is the one read of this store that does not name a Workspace,
    /// because the token is what names it, as the token of a Computer
    /// names the Computer at the exit listener.
    async fn model_token_owner(&self, hash: &str) -> Result<Option<ModelTokenOwner>, StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use CodingSessionEventKind as Kind;

    /// Apply `fold` to an in-memory transcript, as a store does.
    fn apply(rows: &mut Vec<CodingSessionEvent>, new: NewCodingSessionEvent) {
        for write in fold(rows.last(), new) {
            match write {
                TranscriptWrite::Replace { seq, payload } => {
                    let row = rows.last_mut().expect("a replace has a last row");
                    assert_eq!(row.seq, seq);
                    row.payload = payload;
                }
                TranscriptWrite::Append {
                    seq,
                    at,
                    kind,
                    payload,
                } => {
                    assert_eq!(seq, rows.last().map_or(1, |row| row.seq + 1));
                    rows.push(CodingSessionEvent {
                        workspace_id: WorkspaceId::from("w".to_string()),
                        coding_session_id: CodingSessionId::from("s".to_string()),
                        seq,
                        at,
                        kind,
                        payload,
                    });
                }
            }
        }
    }

    fn transcript(
        events: impl IntoIterator<Item = NewCodingSessionEvent>,
    ) -> Vec<CodingSessionEvent> {
        let mut rows = Vec::new();
        for event in events {
            apply(&mut rows, event);
        }
        rows
    }

    fn chunk(
        at: UnixMillis,
        kind: Kind,
        text: &str,
        message_id: Option<&str>,
    ) -> NewCodingSessionEvent {
        NewCodingSessionEvent {
            at,
            kind,
            payload: json!({"text": text, "message_id": message_id}),
        }
    }

    fn text(row: &CodingSessionEvent) -> &str {
        row.payload["text"].as_str().expect("a text row")
    }

    #[test]
    fn two_chunks_of_one_message_make_one_row() {
        let rows = transcript([
            chunk(10, Kind::AgentMessage, "Hello, ", Some("m1")),
            chunk(20, Kind::AgentMessage, "world.", Some("m1")),
        ]);

        assert_eq!(rows.len(), 1);
        assert_eq!(text(&rows[0]), "Hello, world.");
        assert_eq!(rows[0].seq, 1);
        assert_eq!(rows[0].at, 10, "a merge keeps the time of the first chunk");
        assert_eq!(rows[0].payload["message_id"], "m1");
    }

    #[test]
    fn chunks_with_no_message_id_merge() {
        let rows = transcript([
            chunk(10, Kind::Thought, "I read ", None),
            chunk(20, Kind::Thought, "the file.", None),
        ]);

        assert_eq!(rows.len(), 1);
        assert_eq!(text(&rows[0]), "I read the file.");
    }

    #[test]
    fn a_merge_answers_one_replace_of_the_last_row() {
        let rows = transcript([chunk(10, Kind::AgentMessage, "Hello", Some("m1"))]);

        let writes = fold(rows.last(), chunk(20, Kind::AgentMessage, "!", Some("m1")));

        assert_eq!(
            writes,
            vec![TranscriptWrite::Replace {
                seq: 1,
                payload: json!({"text": "Hello!", "message_id": "m1"}),
            }]
        );
    }

    #[test]
    fn a_new_message_id_makes_a_new_row() {
        let rows = transcript([
            chunk(10, Kind::AgentMessage, "First.", Some("m1")),
            chunk(20, Kind::AgentMessage, "Second.", Some("m2")),
        ]);

        assert_eq!(rows.len(), 2);
        assert_eq!(text(&rows[0]), "First.");
        assert_eq!(text(&rows[1]), "Second.");
        assert_eq!(rows[1].seq, 2);
        assert_eq!(rows[1].at, 20);
    }

    #[test]
    fn a_message_and_a_thought_do_not_merge() {
        let rows = transcript([
            chunk(10, Kind::Thought, "Plan.", None),
            chunk(20, Kind::AgentMessage, "Done.", None),
        ]);

        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn a_tool_call_between_two_message_chunks_makes_three_rows() {
        let rows = transcript([
            chunk(10, Kind::AgentMessage, "I run the tests.", Some("m1")),
            NewCodingSessionEvent {
                at: 20,
                kind: Kind::ToolCall,
                payload: json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "cargo test"}),
            },
            chunk(30, Kind::AgentMessage, " They pass.", Some("m1")),
        ]);

        let kinds: Vec<Kind> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            [Kind::AgentMessage, Kind::ToolCall, Kind::AgentMessage]
        );
        assert_eq!(text(&rows[2]), " They pass.");
        assert_eq!(rows[1].payload["toolCallId"], "t1");
    }

    #[test]
    fn a_prompt_never_merges() {
        let rows = transcript([
            chunk(10, Kind::Prompt, "Fix the bug.", None),
            chunk(20, Kind::Prompt, "Add a test.", None),
        ]);

        assert_eq!(rows.len(), 2);
        assert_eq!(text(&rows[0]), "Fix the bug.");
        assert_eq!(text(&rows[1]), "Add a test.");
        assert_eq!(
            rows[0].payload,
            json!({"text": "Fix the bug.", "message_id": null})
        );
    }

    #[test]
    fn a_text_past_the_cap_goes_on_in_a_continued_row_and_keeps_every_character() {
        let first = "a".repeat(MAX_EVENT_PAYLOAD_BYTES - 10);
        let second = "b".repeat(30);
        let rows = transcript([
            chunk(10, Kind::AgentMessage, &first, Some("m1")),
            chunk(20, Kind::AgentMessage, &second, Some("m1")),
        ]);

        assert_eq!(rows.len(), 2);
        assert_eq!(text(&rows[0]).len(), MAX_EVENT_PAYLOAD_BYTES);
        assert_eq!(rows[0].payload.get("continued"), None);
        assert_eq!(rows[1].kind, Kind::AgentMessage);
        assert_eq!(rows[1].payload["continued"], true);
        assert_eq!(rows[1].payload["message_id"], "m1");
        assert_eq!(rows[1].at, 20);
        let joined: String = rows.iter().map(text).collect();
        assert_eq!(joined, format!("{first}{second}"));
    }

    #[test]
    fn a_long_prompt_goes_on_in_continued_rows() {
        let prompt = "p".repeat(MAX_EVENT_PAYLOAD_BYTES * 2 + 5);
        let rows = transcript([chunk(10, Kind::Prompt, &prompt, None)]);

        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|row| row.kind == Kind::Prompt));
        assert!(
            rows.iter()
                .all(|row| text(row).len() <= MAX_EVENT_PAYLOAD_BYTES)
        );
        assert_eq!(rows[0].payload.get("continued"), None);
        assert_eq!(rows[1].payload["continued"], true);
        assert_eq!(rows[2].payload["continued"], true);
        let joined: String = rows.iter().map(text).collect();
        assert_eq!(joined, prompt);
    }

    #[test]
    fn a_chunk_after_a_continued_row_merges_into_it() {
        let rows = transcript([
            chunk(
                10,
                Kind::AgentMessage,
                &"a".repeat(MAX_EVENT_PAYLOAD_BYTES + 1),
                Some("m1"),
            ),
            chunk(20, Kind::AgentMessage, "b", Some("m1")),
        ]);

        assert_eq!(rows.len(), 2);
        assert_eq!(text(&rows[1]), "ab");
        assert_eq!(rows[1].payload["continued"], true);
    }

    #[test]
    fn a_multibyte_character_at_the_cut_stays_whole() {
        // "é" is two bytes, so the cap falls inside the last one.
        let message = format!(
            "{}{}",
            "a".repeat(MAX_EVENT_PAYLOAD_BYTES - 1),
            "é".repeat(4)
        );
        let rows = transcript([chunk(10, Kind::AgentMessage, &message, None)]);

        assert_eq!(rows.len(), 2);
        assert_eq!(text(&rows[0]).len(), MAX_EVENT_PAYLOAD_BYTES - 1);
        assert_eq!(text(&rows[1]), "éééé");
        let joined: String = rows.iter().map(text).collect();
        assert_eq!(joined, message);
    }

    #[test]
    fn a_tool_call_with_a_large_diff_is_cut_to_fit_and_keeps_its_id() {
        let diff = "+ line\n".repeat(40 * 1024 / 7);
        let rows = transcript([NewCodingSessionEvent {
            at: 10,
            kind: Kind::ToolCall,
            payload: json!({
                "sessionUpdate": "tool_call",
                "toolCallId": "t1",
                "title": "Edit main.rs",
                "kind": "edit",
                "status": "completed",
                "content": [{"type": "diff", "path": "src/main.rs", "oldText": "", "newText": diff}],
            }),
        }]);

        assert_eq!(rows.len(), 1);
        let payload = &rows[0].payload;
        assert!(serde_json::to_vec(payload).unwrap().len() <= MAX_EVENT_PAYLOAD_BYTES);
        assert_eq!(payload["toolCallId"], "t1");
        assert_eq!(payload["title"], "Edit main.rs");
        assert_eq!(payload["truncated"], true);
        let cut = payload["content"][0]["newText"].as_str().unwrap();
        assert!(cut.ends_with('…'), "the cut string ends with an ellipsis");
    }

    #[test]
    fn a_payload_of_many_short_strings_keeps_only_its_named_fields() {
        let lines: Vec<String> = (0..4_000).map(|n| format!("line {n}")).collect();
        let rows = transcript([NewCodingSessionEvent {
            at: 10,
            kind: Kind::ToolCallUpdate,
            payload: json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "t1",
                "status": "completed",
                "rawOutput": lines,
            }),
        }]);

        assert_eq!(
            rows[0].payload,
            json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "t1",
                "status": "completed",
                "truncated": true,
            })
        );
    }

    #[test]
    fn a_payload_that_fits_is_kept_as_it_is() {
        let payload =
            json!({"sessionUpdate": "plan", "entries": [{"content": "Read", "status": "pending"}]});
        let rows = transcript([NewCodingSessionEvent {
            at: 10,
            kind: Kind::Plan,
            payload: payload.clone(),
        }]);

        assert_eq!(rows[0].payload, payload);
    }

    #[test]
    fn each_place_reads_back_from_its_name() {
        for place in [CodingSessionPlace::Host, CodingSessionPlace::Computer] {
            assert_eq!(place.as_str().parse::<CodingSessionPlace>(), Ok(place));
        }
        assert!("phone".parse::<CodingSessionPlace>().is_err());
    }

    #[test]
    fn each_approval_mode_reads_back_from_its_name() {
        for mode in [SessionApprovalMode::Person, SessionApprovalMode::Agent] {
            assert_eq!(mode.as_str().parse::<SessionApprovalMode>(), Ok(mode));
        }
        for unknown in ["always", "auto"] {
            assert!(unknown.parse::<SessionApprovalMode>().is_err(), "{unknown}");
            assert!(
                serde_json::from_value::<SessionApprovalMode>(json!(unknown)).is_err(),
                "{unknown}"
            );
        }
    }

    #[test]
    fn each_state_reads_back_from_its_name_and_only_closed_and_failed_are_terminal() {
        use CodingSessionState as State;
        let all = [
            (State::Starting, "starting", false),
            (State::Working, "working", false),
            (State::NeedsDecision, "needs_decision", false),
            (State::Idle, "idle", false),
            (State::Interrupted, "interrupted", false),
            (State::Closed, "closed", true),
            (State::Failed, "failed", true),
        ];
        for (state, name, terminal) in all {
            assert_eq!(state.as_str(), name);
            assert_eq!(name.parse::<State>(), Ok(state));
            assert_eq!(state.is_terminal(), terminal, "{name}");
        }
        assert!("ended".parse::<State>().is_err());
    }

    #[test]
    fn each_event_kind_reads_back_from_its_name() {
        let all = [
            (Kind::Prompt, "prompt"),
            (Kind::AgentMessage, "agent_message"),
            (Kind::Thought, "thought"),
            (Kind::ToolCall, "tool_call"),
            (Kind::ToolCallUpdate, "tool_call_update"),
            (Kind::Plan, "plan"),
            (Kind::Usage, "usage"),
            (Kind::Permission, "permission"),
            (Kind::Decision, "decision"),
            (Kind::Question, "question"),
            (Kind::Answer, "answer"),
            (Kind::TurnEnd, "turn_end"),
        ];
        for (kind, name) in all {
            assert_eq!(kind.as_str(), name);
            assert_eq!(name.parse::<Kind>(), Ok(kind));
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
        assert!("message".parse::<Kind>().is_err());
    }

    #[test]
    fn a_mode_permits_itself_and_each_narrower_mode() {
        use SessionApprovalMode::{Agent, Person};
        let pairs = [
            (Person, Person, true),
            (Person, Agent, false),
            (Agent, Person, true),
            (Agent, Agent, true),
        ];
        for (widest, requested, permitted) in pairs {
            assert_eq!(
                widest.permits(requested),
                permitted,
                "{widest:?} permits {requested:?}"
            );
        }
    }

    fn rule(harness: &str, directory: &str) -> SessionAllowRule {
        SessionAllowRule::new(harness, directory).expect("a valid rule")
    }

    #[test]
    fn a_session_allow_rule_covers_its_directory_and_each_directory_under_it() {
        let rule = rule("claude", "/work/pagis");

        assert!(rule.covers("claude", "/work/pagis"));
        assert!(rule.covers("claude", "/work/pagis/"));
        assert!(rule.covers("claude", "/work/pagis/crates/core"));
        assert!(rule.covers("claude", "/work/./pagis/crates/../ui"));
    }

    #[test]
    fn a_session_allow_rule_does_not_cover_a_sibling_with_a_shared_prefix() {
        assert!(!rule("claude", "/work/pagis").covers("claude", "/work/pagis2"));
    }

    #[test]
    fn a_session_allow_rule_does_not_cover_a_path_whose_dot_dot_leaves_it() {
        let rule = rule("claude", "/work/pagis");

        assert!(!rule.covers("claude", "/work/pagis/../secrets"));
        assert!(!rule.covers("claude", "/work/pagis/crates/../../secrets"));
        assert!(!rule.covers("claude", "work/pagis"));
    }

    #[test]
    fn a_session_allow_rule_does_not_cover_another_harness() {
        assert!(!rule("claude", "/work/pagis").covers("codex", "/work/pagis"));
    }

    #[test]
    fn a_new_session_allow_rule_holds_the_resolved_directory() {
        assert_eq!(
            rule("codex", "/work/./pagis/ui/../").directory,
            "/work/pagis"
        );
        assert_eq!(rule("codex", "/").directory, "/");
        assert_eq!(rule("codex", "/..").directory, "/");
    }

    #[test]
    fn a_new_session_allow_rule_needs_a_catalog_harness_and_an_absolute_directory() {
        let unknown = SessionAllowRule::new("vim", "/work/pagis").unwrap_err();
        assert!(unknown.contains("vim"), "{unknown}");
        let relative = SessionAllowRule::new("claude", "work/pagis").unwrap_err();
        assert!(relative.contains("work/pagis"), "{relative}");
        assert!(SessionAllowRule::new("claude", "C:\\work\\pagis").is_err());
    }

    #[test]
    fn a_path_is_inside_a_directory_after_dot_and_dot_dot_are_resolved() {
        assert!(path_is_inside("/repo/a.rs", "/repo"));
        assert!(path_is_inside("/repo", "/repo/./"));
        assert!(path_is_inside("/anything", "/"));
        assert!(!path_is_inside("/repo/../etc", "/repo"));
        assert!(!path_is_inside("repo/a.rs", "/repo"));
        assert!(!path_is_inside("/repo/a.rs", "repo"));
    }
}
