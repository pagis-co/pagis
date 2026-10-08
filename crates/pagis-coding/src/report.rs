//! The harness text of a Coding Session that `coding_session_read`
//! answers (ADR-0033): the pending decision, the last agent message, the
//! plan, the end detail and the changed files. All of it is foreign text,
//! so the caller puts it inside one untrusted envelope (ADR-0005).

use std::collections::{HashMap, HashSet};

use pagis_core::{
    CodingSession, CodingSessionEvent, CodingSessionEventKind as Kind, CodingSessionState,
    CodingSessionStore, StoreError,
};
use serde_json::{Value, json};

/// The longest last agent message, in characters.
const MESSAGE_CHARS: usize = 4_000;

/// The most changed files, newest first.
const CHANGED_FILES: usize = 50;

/// The transcript rows that one read of the changed files takes.
const PAGE: u32 = 500;

/// The rows of an ask and of its answer.
const DECISION_KINDS: [Kind; 4] = [
    Kind::Permission,
    Kind::Question,
    Kind::Decision,
    Kind::Answer,
];

/// The tool kinds of ACP that change a file.
const CHANGE_KINDS: [&str; 3] = ["edit", "delete", "move"];

/// The harness text of a session, as JSON.
pub(crate) async fn harness_output(
    store: &dyn CodingSessionStore,
    session: &CodingSession,
) -> Result<Value, StoreError> {
    let latest =
        |kinds: &'static [Kind]| store.latest_event(&session.workspace_id, &session.id, kinds);
    let pending = if session.state == CodingSessionState::NeedsDecision {
        latest(&DECISION_KINDS)
            .await?
            .as_ref()
            .and_then(pending_decision)
    } else {
        None
    };
    let last_message = latest(&[Kind::AgentMessage])
        .await?
        .and_then(|row| row.payload.get("text")?.as_str().map(cut_message));
    let plan = latest(&[Kind::Plan])
        .await?
        .and_then(|row| row.payload.get("entries").cloned());
    Ok(json!({
        "pending_decision": pending,
        "last_message": last_message,
        "plan": plan,
        "end_detail": session.end_detail,
        "changed_files": changed_files(store, session).await?,
    }))
}

/// The ask that waits, from the last row of an ask or of its answer: the
/// title of a permission and its options, or the message of a question
/// and its form. An answer row means that no ask waits.
fn pending_decision(row: &CodingSessionEvent) -> Option<Value> {
    let field = |name: &str| row.payload.get(name).cloned().unwrap_or(Value::Null);
    match row.kind {
        Kind::Permission => Some(json!({
            "kind": "permission",
            "title": field("title"),
            "options": field("options"),
        })),
        Kind::Question => Some(json!({
            "kind": "question",
            "title": field("message"),
            "form": field("schema"),
        })),
        _ => None,
    }
}

/// The first [`MESSAGE_CHARS`] characters of a message, with a marker
/// that counts the characters after the cut.
fn cut_message(text: &str) -> String {
    match text.char_indices().nth(MESSAGE_CHARS) {
        None => text.to_string(),
        Some((cut, _)) => {
            let rest = text[cut..].chars().count();
            format!("{}… [{rest} more characters]", &text[..cut])
        }
    }
}

/// The unique files that the tool calls of the kinds `edit`, `delete`
/// and `move` touched, newest first. The transcript is read in pages, so
/// a long session never loads whole.
async fn changed_files(
    store: &dyn CodingSessionStore,
    session: &CodingSession,
) -> Result<Vec<String>, StoreError> {
    let mut files = ChangedFiles::default();
    let mut after = None;
    loop {
        let rows = store
            .list_events(&session.workspace_id, &session.id, after, PAGE)
            .await?;
        for row in &rows {
            files.add(row);
        }
        match rows.last() {
            Some(last) if rows.len() == PAGE as usize => after = Some(last.seq),
            _ => return Ok(files.newest()),
        }
    }
}

/// The files that the tool calls touched, in the order of the
/// transcript.
#[derive(Default)]
struct ChangedFiles {
    /// The kind of each tool call, by its `toolCallId`. An update that
    /// names no kind keeps the kind of its call.
    kinds: HashMap<String, String>,
    touched: Vec<String>,
}

impl ChangedFiles {
    fn add(&mut self, row: &CodingSessionEvent) {
        if !matches!(row.kind, Kind::ToolCall | Kind::ToolCallUpdate) {
            return;
        }
        let payload = &row.payload;
        let Some(id) = payload.get("toolCallId").and_then(Value::as_str) else {
            return;
        };
        if let Some(kind) = payload.get("kind").and_then(Value::as_str) {
            self.kinds.insert(id.to_string(), kind.to_string());
        }
        let changes = self
            .kinds
            .get(id)
            .is_some_and(|kind| CHANGE_KINDS.contains(&kind.as_str()));
        if !changes {
            return;
        }
        let paths = payload
            .get("locations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|location| location.get("path")?.as_str());
        self.touched.extend(paths.map(str::to_string));
    }

    fn newest(self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.touched
            .into_iter()
            .rev()
            .filter(|path| seen.insert(path.clone()))
            .take(CHANGED_FILES)
            .collect()
    }
}
