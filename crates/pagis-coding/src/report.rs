//! The harness text of a Coding Session that `coding_session_read`
//! answers (ADR-0033): the asks that wait, the last agent message, the
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

/// The transcript rows that one read of the transcript takes.
const PAGE: u32 = 500;

/// The tool kinds of ACP that change a file.
const CHANGE_KINDS: [&str; 3] = ["edit", "delete", "move"];

/// The harness text of a session, as JSON.
pub(crate) async fn harness_output(
    store: &dyn CodingSessionStore,
    session: &CodingSession,
) -> Result<Value, StoreError> {
    let latest =
        |kinds: &'static [Kind]| store.latest_event(&session.workspace_id, &session.id, kinds);
    let last_message = latest(&[Kind::AgentMessage])
        .await?
        .and_then(|row| row.payload.get("text")?.as_str().map(cut_message));
    let plan = latest(&[Kind::Plan])
        .await?
        .and_then(|row| row.payload.get("entries").cloned());
    let (asks, files) = scan(store, session).await?;
    let pending = if session.state == CodingSessionState::NeedsDecision {
        asks.waiting()
    } else {
        Vec::new()
    };
    Ok(json!({
        "pending_decisions": pending,
        "last_message": last_message,
        "plan": plan,
        "end_detail": session.end_detail,
        "changed_files": files.newest(),
    }))
}

/// The asks that wait and the changed files, from one read of the whole
/// transcript. The transcript is read in pages, so a long session never
/// loads whole.
async fn scan(
    store: &dyn CodingSessionStore,
    session: &CodingSession,
) -> Result<(WaitingAsks, ChangedFiles), StoreError> {
    let mut asks = WaitingAsks::default();
    let mut files = ChangedFiles::default();
    let mut after = None;
    loop {
        let rows = store
            .list_events(&session.workspace_id, &session.id, after, PAGE)
            .await?;
        for row in &rows {
            asks.add(row);
            files.add(row);
        }
        match rows.last() {
            Some(last) if rows.len() == PAGE as usize => after = Some(last.seq),
            _ => return Ok((asks, files)),
        }
    }
}

/// The asks that have no answer row yet, oldest first. A turn that ends
/// or a new prompt clears them: the harness waits for no ask of a turn
/// that is over.
#[derive(Default)]
struct WaitingAsks {
    /// The `ask_id` of each ask and the ask as the read shows it.
    asks: Vec<(Value, Value)>,
}

impl WaitingAsks {
    fn add(&mut self, row: &CodingSessionEvent) {
        let ask_id = || row.payload.get("ask_id").cloned().unwrap_or(Value::Null);
        match row.kind {
            Kind::Permission | Kind::Question => self.asks.push((ask_id(), shown(row))),
            Kind::Decision | Kind::Answer => {
                let answered = ask_id();
                self.asks.retain(|(id, _)| *id != answered);
            }
            Kind::Prompt | Kind::TurnEnd => self.asks.clear(),
            _ => {}
        }
    }

    fn waiting(self) -> Vec<Value> {
        self.asks.into_iter().map(|(_, ask)| ask).collect()
    }
}

/// An ask as the read shows it: the title of a permission, its tool
/// kind, its command, its locations and its options, or the message of a
/// question and its form, and whom it waits for.
fn shown(row: &CodingSessionEvent) -> Value {
    let field = |name: &str| row.payload.get(name).cloned().unwrap_or(Value::Null);
    if row.kind == Kind::Question {
        return json!({
            "kind": "question",
            "message": field("message"),
            "form": field("schema"),
            "waits_for": field("waits_for"),
        });
    }
    json!({
        "kind": "permission",
        "title": field("title"),
        "tool_kind": field("kind"),
        // The shell command of an `execute`, as Pagis policy reads it.
        "command": row.payload["raw_input"]["command"].as_str(),
        "locations": field("locations"),
        "options": field("options"),
        "waits_for": field("waits_for"),
    })
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

/// The files that the tool calls of the kinds `edit`, `delete` and
/// `move` touched, in the order of the transcript.
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

    /// The unique touched files, newest first.
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
