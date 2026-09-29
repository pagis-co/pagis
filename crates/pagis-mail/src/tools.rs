//! The `mail__*` executor (ADR-0019). The broker has already
//! resolved the mailbox, refused a call the mailbox cannot take,
//! counted the Outgoing Cap and minted the card; this is what runs
//! after that.
//!
//! Every answer is capped here, because a mailbox is unbounded and a
//! tool answer is not: 25 summaries a page, 32 KB of text for one
//! message and for one thread, and 20 messages of a thread a page.
//! Text that is cut ends with a marker, so the model reads that it has
//! only part of the words.
//!
//! An id is the folder and the IMAP UID (`INBOX:1234`). It holds while
//! the host keeps its numbering; a host that renumbers a folder makes
//! every id of it stale, and the answer is `stale_id`.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, MailTool, ToolExecutor, ToolResult, ToolRoute};
use pagis_core::{
    AgentMailbox, RunStore, SentMail, SentMailStore, UnixMillis, WorkspaceId, now_ms,
};

use crate::blocks::MailBlocks;
use crate::desk::{MailboxDesk, MailboxError};
use crate::transport::{
    ARCHIVE, FlagChange, INBOX, MailQuery, MailSession, Message, MessageId, MessageSummary,
    OutgoingMessage, SentMessage, TransportError, TransportErrorCode,
};

/// The most summaries one search page answers with (ADR-0019).
pub const MAX_SUMMARIES: u32 = 25;

/// The most text one message or one thread answers with (ADR-0019).
pub const MAX_TEXT_BYTES: usize = 32 * 1024;

/// The most messages of one thread a page answers with (ADR-0019).
pub const MAX_THREAD_MESSAGES: usize = 20;

/// What the answer says where the text was cut.
pub const TRUNCATION_MARKER: &str = "\n[the text is cut here: the rest stays at the mail host]";

/// The stable code of a call that names a message or a folder the host
/// has renumbered or does not hold.
pub const STALE_ID: &str = "stale_id";

/// What became of one reserved send. The two cases are different
/// facts: one says the message did not leave, the other says nobody knows.
#[derive(Debug, Clone)]
pub enum ReservedSendError {
    /// The message was not handed to the host.
    NotSent(ToolResult),
    /// The host was handed the message and did not confirm it.
    Uncertain(ToolResult),
}

pub struct MailToolDeps {
    /// The one path to a mailbox: it owns the password and the
    /// Connection settings, so nothing here reads a secret.
    pub desk: Arc<MailboxDesk>,
    /// The Run record, for the Thread a send is remembered in.
    pub runs: Arc<dyn RunStore>,
    pub sent: Arc<dyn SentMailStore>,
    /// The `mail` block of a sent message (ADR-0019).
    pub blocks: Arc<MailBlocks>,
}

pub struct MailToolRuntime {
    deps: MailToolDeps,
}

impl MailToolRuntime {
    pub fn new(deps: MailToolDeps) -> Self {
        Self { deps }
    }

    /// Whether a route is this runtime's to execute.
    pub fn owns(route: &ToolRoute) -> bool {
        matches!(route, ToolRoute::Mailbox { .. })
    }

    async fn run(
        &self,
        tool: MailTool,
        call: &AuthorizedCall,
    ) -> Result<serde_json::Value, ToolResult> {
        let mailbox = self
            .deps
            .desk
            .for_agent(&call.workspace_id, &call.agent_id)
            .await
            .map_err(desk_error)?
            .ok_or_else(|| {
                ToolResult::error(pagis_broker::MAILBOX_UNAVAILABLE, "you hold no mailbox now")
            })?;
        let mut session = self
            .deps
            .desk
            .open_session(&mailbox)
            .await
            .map_err(desk_error)?;
        match tool {
            MailTool::Search => search(session.as_mut(), &mailbox, &call.arguments).await,
            MailTool::GetMessage => get_message(session.as_mut(), &mailbox, &call.arguments).await,
            MailTool::GetThread => get_thread(session.as_mut(), &mailbox, &call.arguments).await,
            MailTool::ModifyMessage => {
                modify_message(session.as_mut(), &mailbox, &call.arguments).await
            }
            MailTool::Send => {
                self.send(session.as_mut(), &mailbox, call, &call.workspace_id)
                    .await
            }
        }
    }

    /// Send one message, count it against the Outgoing Cap, and
    /// remember its `Message-ID` with the Thread the Run works in, so
    /// a reply lands where the send came from.
    async fn send(
        &self,
        session: &mut dyn MailSession,
        mailbox: &AgentMailbox,
        call: &AuthorizedCall,
        workspace_id: &WorkspaceId,
    ) -> Result<serde_json::Value, ToolResult> {
        let message = OutgoingMessage {
            to: strings(&call.arguments, "to"),
            cc: strings(&call.arguments, "cc"),
            bcc: strings(&call.arguments, "bcc"),
            subject: text(&call.arguments, "subject"),
            body: text(&call.arguments, "body"),
            in_reply_to: call.arguments["in_reply_to"]
                .as_str()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
            message_id: None,
        };
        let sent = session.send(&message).await.map_err(transport_error)?;
        let at = now_ms();
        // The tally is counted after the host accepted the message:
        // a send that never left costs no allowance.
        let sends_today = self
            .deps
            .desk
            .note_send(workspace_id, &mailbox.id, at)
            .await
            .map_err(desk_error)?;
        self.remember(&message, &sent.message_id, mailbox, call, workspace_id, at)
            .await;
        Ok(serde_json::json!({
            "message_id": sent.message_id,
            "from": mailbox.address,
            "to": message.to,
            "sends_today": sends_today,
            "outgoing_cap": mailbox.outgoing_cap
        }))
    }

    /// Send one message the daemon already authorized and reserved, outside
    /// any Run. The reservation names the `Message-ID`, so the message
    /// the host accepted is the one the reservation names. Nothing is retried
    /// here: the caller reconciles the outcome it is given.
    pub async fn send_reserved(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &pagis_core::AgentId,
        message: &OutgoingMessage,
    ) -> Result<SentMessage, ReservedSendError> {
        let refused = |result: ToolResult| ReservedSendError::NotSent(result);
        let mailbox = self
            .deps
            .desk
            .for_agent(workspace_id, agent_id)
            .await
            .map_err(|error| refused(desk_error(error)))?
            .ok_or_else(|| {
                refused(ToolResult::error(
                    pagis_broker::MAILBOX_UNAVAILABLE,
                    "the Agent holds no mailbox now",
                ))
            })?;
        if mailbox.sends_today >= mailbox.outgoing_cap {
            return Err(refused(ToolResult::error(
                pagis_broker::OUTGOING_CAP_REACHED,
                format!(
                    "{} has sent its {} messages for today",
                    mailbox.address, mailbox.outgoing_cap
                ),
            )));
        }
        let mut session = self
            .deps
            .desk
            .open_session(&mailbox)
            .await
            .map_err(|error| refused(desk_error(error)))?;
        let sent = match session.send(message).await {
            Ok(sent) => sent,
            Err(error) => {
                // A host that stopped answering may have taken the message
                // before it stopped. A refusal did not.
                return Err(match error.0 {
                    TransportErrorCode::Unreachable | TransportErrorCode::Unreadable => {
                        ReservedSendError::Uncertain(transport_error(error))
                    }
                    _ => refused(transport_error(error)),
                });
            }
        };
        // The message has left. A bookkeeping failure after that does not
        // make it unsent, so it is logged and not reported as a failure.
        if let Err(error) = self
            .deps
            .desk
            .note_send(workspace_id, &mailbox.id, now_ms())
            .await
        {
            tracing::error!(%error, "counting a reserved send failed");
        }
        Ok(sent)
    }

    /// Write the sent-mail record and mint the `mail` block of the
    /// send. A failure here does not fail the send: the message has
    /// already left, and saying it failed would invite the model to
    /// send it again.
    async fn remember(
        &self,
        sent: &OutgoingMessage,
        message_id: &str,
        mailbox: &AgentMailbox,
        call: &AuthorizedCall,
        workspace_id: &WorkspaceId,
        at: UnixMillis,
    ) {
        let run = match self.deps.runs.get(&call.workspace_id, &call.run_id).await {
            Ok(run) => run,
            Err(error) => {
                tracing::error!(%error, "reading the run of a sent message failed");
                return;
            }
        };
        if let Some(run) = &run {
            self.deps
                .blocks
                .on_send(run, mailbox, message_id, &sent.to, &sent.subject)
                .await;
        }
        let record = SentMail {
            message_id: message_id.to_string(),
            workspace_id: workspace_id.clone(),
            agent_id: call.agent_id.clone(),
            mailbox_id: mailbox.id.clone(),
            run_id: call.run_id.clone(),
            channel_id: run.as_ref().and_then(|run| run.channel_id.clone()),
            thread_id: run.and_then(|run| run.root_message_id),
            sent_at: at,
        };
        if let Err(error) = self.deps.sent.record(&record).await {
            tracing::error!(%error, "remembering a sent message failed");
        }
    }
}

#[async_trait]
impl ToolExecutor for MailToolRuntime {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        let ToolRoute::Mailbox { tool } = call.route else {
            return ToolResult::error(
                "invalid_request",
                format!("{} is not a mail tool", call.tool_name),
            );
        };
        match self.run(tool, &call).await {
            Ok(answer) => ToolResult::success(answer.to_string()),
            Err(result) => result,
        }
    }
}

/// Open one folder and refuse a stale id. A host that renumbers a
/// folder gives it a new UIDVALIDITY, and every id and cursor of that
/// folder then names another message (ADR-0019). The mailbox's own
/// cursor is the numbering Pagis last saw, so a folder that has moved
/// away from it answers `stale_id` before anything is read.
async fn select_checked(
    session: &mut dyn MailSession,
    mailbox: &AgentMailbox,
    folder: &str,
) -> Result<(), ToolResult> {
    let state = session.select(folder).await.map_err(transport_error)?;
    let renumbered = mailbox
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.folder == folder && cursor.uid_validity != state.uid_validity);
    match renumbered {
        true => Err(transport_error(TransportError(TransportErrorCode::StaleId))),
        false => Ok(()),
    }
}

async fn search(
    session: &mut dyn MailSession,
    mailbox: &AgentMailbox,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, ToolResult> {
    let limit = arguments["max"]
        .as_u64()
        .map(|max| (max as u32).clamp(1, MAX_SUMMARIES))
        .unwrap_or(MAX_SUMMARIES);
    let skip = page_offset(arguments)?;
    select_checked(session, mailbox, INBOX).await?;
    let mut query = parse_query(&text(arguments, "query"))?;
    // One more than the page needs, so the answer knows whether a next
    // page exists without a second search.
    query.limit = skip.saturating_add(limit).saturating_add(1);
    let found = session.search(&query).await.map_err(transport_error)?;
    let more = found.len() as u32 > skip.saturating_add(limit);
    let page: Vec<&MessageSummary> = found
        .iter()
        .skip(skip as usize)
        .take(limit as usize)
        .collect();
    Ok(serde_json::json!({
        "messages": page.iter().map(|summary| summary_json(summary)).collect::<Vec<_>>(),
        "page": more.then(|| (skip + limit).to_string())
    }))
}

async fn get_message(
    session: &mut dyn MailSession,
    mailbox: &AgentMailbox,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, ToolResult> {
    let id = message_id(arguments)?;
    select_checked(session, mailbox, &id.folder).await?;
    let message = session.fetch(&id).await.map_err(transport_error)?;
    let (body, truncated) = cut(&message.text, MAX_TEXT_BYTES);
    Ok(serde_json::json!({"message": message_json(&message, &body, truncated)}))
}

async fn get_thread(
    session: &mut dyn MailSession,
    mailbox: &AgentMailbox,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, ToolResult> {
    let thread_id = text(arguments, "thread_id");
    let skip = page_offset(arguments)? as usize;
    select_checked(session, mailbox, INBOX).await?;
    let messages = session
        .fetch_thread(&thread_id)
        .await
        .map_err(transport_error)?;
    // The newest messages are the ones the Agent needs, so a page
    // starts at the end of the thread and walks back.
    let end = messages.len().saturating_sub(skip);
    let start = end.saturating_sub(MAX_THREAD_MESSAGES);
    let window = &messages[start..end];

    // The budget is spent on the newest messages first, and the answer
    // still reads oldest first.
    let mut budget = MAX_TEXT_BYTES;
    let mut bodies: Vec<(String, bool)> = Vec::with_capacity(window.len());
    for message in window.iter().rev() {
        let (body, truncated) = cut(&message.text, budget);
        budget -= body.len().min(budget);
        bodies.push((body, truncated));
    }
    bodies.reverse();

    Ok(serde_json::json!({
        "thread_id": thread_id,
        "messages": window
            .iter()
            .zip(bodies.iter())
            .map(|(message, (body, truncated))| message_json(message, body, *truncated))
            .collect::<Vec<_>>(),
        "page": (start > 0).then(|| (skip + window.len()).to_string())
    }))
}

async fn modify_message(
    session: &mut dyn MailSession,
    mailbox: &AgentMailbox,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, ToolResult> {
    let id = message_id(arguments)?;
    let flags = FlagChange {
        seen: arguments["mark_read"].as_bool(),
        flagged: arguments["flag"].as_bool(),
    };
    select_checked(session, mailbox, &id.folder).await?;
    if flags.seen.is_some() || flags.flagged.is_some() {
        session
            .set_flags(&id, flags)
            .await
            .map_err(transport_error)?;
    }
    // The archive move is last: it changes the id, so the flags are set
    // while the message is still where the caller named it.
    let archived = arguments["archive"].as_bool().unwrap_or(false);
    if archived {
        session
            .move_to_archive(&id)
            .await
            .map_err(transport_error)?;
    }
    Ok(serde_json::json!({
        "message_id": id.to_string(),
        "mark_read": flags.seen,
        "flag": flags.flagged,
        "archived": archived,
        "folder": if archived { ARCHIVE } else { id.folder.as_str() }
    }))
}

fn summary_json(summary: &MessageSummary) -> serde_json::Value {
    serde_json::json!({
        "id": summary.id.to_string(),
        "thread_id": summary.thread_id,
        "from": summary.from,
        "to": summary.to,
        "date": summary.date,
        "subject": summary.subject,
        "snippet": summary.snippet,
        "has_attachments": summary.has_attachments
    })
}

fn message_json(message: &Message, body: &str, truncated: bool) -> serde_json::Value {
    let mut json = summary_json(&message.summary);
    let text = match truncated {
        true => format!("{body}{TRUNCATION_MARKER}"),
        false => body.to_string(),
    };
    if let Some(object) = json.as_object_mut() {
        object.remove("snippet");
        object.insert(
            "headers".to_string(),
            serde_json::json!(
                message
                    .headers
                    .iter()
                    .map(|(name, value)| serde_json::json!({"name": name, "value": value}))
                    .collect::<Vec<_>>()
            ),
        );
        object.insert("text".to_string(), serde_json::json!(text));
        object.insert("truncated".to_string(), serde_json::json!(truncated));
        object.insert(
            "attachments".to_string(),
            serde_json::json!(
                message
                    .attachments
                    .iter()
                    .map(|attachment| {
                        serde_json::json!({"name": attachment.name, "bytes": attachment.bytes})
                    })
                    .collect::<Vec<_>>()
            ),
        );
    }
    json
}

/// Cut text to a byte budget on a character boundary. It says whether
/// anything was left behind.
fn cut(text: &str, budget: usize) -> (String, bool) {
    if text.len() <= budget {
        return (text.to_string(), false);
    }
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

fn message_id(arguments: &serde_json::Value) -> Result<MessageId, ToolResult> {
    text(arguments, "message_id")
        .parse::<MessageId>()
        .map_err(|error| ToolResult::error(STALE_ID, format!("{error}")))
}

/// The page token of an earlier answer: how many messages to step over.
fn page_offset(arguments: &serde_json::Value) -> Result<u32, ToolResult> {
    match arguments["page"].as_str().map(str::trim) {
        None | Some("") => Ok(0),
        Some(page) => page.parse::<u32>().map_err(|_| {
            ToolResult::error(
                "invalid_request",
                "page takes the token of an earlier answer",
            )
        }),
    }
}

fn text(arguments: &serde_json::Value, field: &str) -> String {
    arguments[field]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn strings(arguments: &serde_json::Value, field: &str) -> Vec<String> {
    arguments[field]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Read one search query (ADR-0019). `from:`, `to:`, `subject:`,
/// `since:` and `before:` narrow the search; every other word is text
/// to find in the body. A value with spaces goes in double quotes.
pub fn parse_query(query: &str) -> Result<MailQuery, ToolResult> {
    let mut found = MailQuery::default();
    let mut words: Vec<String> = Vec::new();
    for token in tokens(query) {
        let Some((field, value)) = token.split_once(':') else {
            words.push(token);
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        if value.is_empty() {
            words.push(token);
            continue;
        }
        match field.to_ascii_lowercase().as_str() {
            "from" => found.from = Some(value),
            "to" => found.to = Some(value),
            "subject" => found.subject = Some(value),
            "since" => found.since = Some(day(&value)?),
            "before" => found.before = Some(day(&value)?),
            _ => words.push(token),
        }
    }
    if !words.is_empty() {
        found.text = Some(words.join(" "));
    }
    Ok(found)
}

/// One `YYYY-MM-DD` date, as the first instant of that day in UTC.
fn day(value: &str) -> Result<UnixMillis, ToolResult> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|at| at.and_utc().timestamp_millis())
        .ok_or_else(|| {
            ToolResult::error(
                "invalid_request",
                format!("{value:?} is not a date; write it as 2026-01-31"),
            )
        })
}

/// Split a query into words, keeping a double-quoted value whole.
fn tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in query.chars() {
        match character {
            '"' => {
                quoted = !quoted;
                current.push(character);
            }
            character if character.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            character => current.push(character),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
        .into_iter()
        .map(|token| token.trim_matches('"').to_string())
        .filter(|token| !token.is_empty())
        .collect()
}

/// The stable code one transport failure answers with.
fn transport_error(error: TransportError) -> ToolResult {
    let (code, message) = match error.0 {
        TransportErrorCode::StaleId => (
            STALE_ID,
            "the mail host has renumbered that folder, so that id is gone. Search again.",
        ),
        TransportErrorCode::Unauthorized | TransportErrorCode::MailboxGone => (
            pagis_broker::MAILBOX_UNAVAILABLE,
            "the mail host refused the mailbox. Tell the user to reset its password.",
        ),
        TransportErrorCode::Unreachable | TransportErrorCode::Unreadable => (
            "temporarily_unavailable",
            "the mail host did not answer. Try again later.",
        ),
        TransportErrorCode::Rejected => (
            "rejected",
            "the mail host refused the message. Check the addresses.",
        ),
    };
    ToolResult::error(code, message)
}

fn desk_error(error: MailboxError) -> ToolResult {
    match error {
        MailboxError::Transport(error) => transport_error(error),
        MailboxError::NotFound | MailboxError::Conflict(_) => {
            ToolResult::error(pagis_broker::MAILBOX_UNAVAILABLE, error.to_string())
        }
        error => ToolResult::error("temporarily_unavailable", error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_reads_its_fields_and_keeps_the_rest_as_body_text() {
        let query = parse_query("from:alice@example.com subject:\"the invoice\" late payment")
            .expect("a query");

        assert_eq!(query.from.as_deref(), Some("alice@example.com"));
        assert_eq!(query.subject.as_deref(), Some("the invoice"));
        assert_eq!(query.text.as_deref(), Some("late payment"));
    }

    #[test]
    fn a_date_field_becomes_the_first_instant_of_that_day() {
        let query = parse_query("since:2026-01-31").expect("a query");

        assert_eq!(query.since, Some(1_769_817_600_000));
        assert!(parse_query("since:yesterday").is_err());
    }

    #[test]
    fn an_empty_query_is_the_browse_call() {
        let query = parse_query("   ").expect("a query");

        assert_eq!(query, MailQuery::default());
    }

    #[test]
    fn text_is_cut_on_a_character_boundary() {
        let (body, truncated) = cut("aé", 2);

        assert_eq!(body, "a");
        assert!(truncated);
        assert_eq!(cut("ab", 8), ("ab".to_string(), false));
    }
}
