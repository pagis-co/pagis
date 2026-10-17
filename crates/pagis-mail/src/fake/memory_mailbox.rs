//! A Mailbox in memory, with opaque ids and page tokens.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_core::{AgentMailbox, now_ms};

use super::StoredFlags;
use crate::{
    Attachment, MailQuery, Mailbox, MailboxError, Mailboxes, Message, MessageChange, MessageId,
    MessageSummary, OutgoingMessage, Page, PageRequest, SentMessage, TransportError,
    TransportErrorCode,
};

#[derive(Clone, Default)]
pub struct MemoryMailbox {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    messages: Vec<StoredMessage>,
    sent: Vec<OutgoingMessage>,
    fail_with: Option<TransportErrorCode>,
}

struct StoredMessage {
    message: Message,
    flags: StoredFlags,
    archived: bool,
}

impl MemoryMailbox {
    pub fn deliver(
        &self,
        from: impl Into<String>,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> MessageId {
        self.deliver_into(None, from.into(), subject.into(), body.into())
    }

    pub fn deliver_reply(
        &self,
        thread_id: &str,
        from: impl Into<String>,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> MessageId {
        self.deliver_into(Some(thread_id), from.into(), subject.into(), body.into())
    }

    fn deliver_into(
        &self,
        thread: Option<&str>,
        from: String,
        subject: String,
        body: String,
    ) -> MessageId {
        let id = MessageId::new(ulid::Ulid::new().to_string());
        let message_id = format!("<{}@memory.invalid>", id);
        let summary = MessageSummary {
            id: id.clone(),
            message_id: message_id.clone(),
            thread_id: thread.unwrap_or(&message_id).to_string(),
            in_reply_to: thread.map(str::to_string),
            from: from.clone(),
            from_mailboxes: 1,
            to: Vec::new(),
            date: now_ms(),
            subject: subject.clone(),
            snippet: body.chars().take(200).collect(),
            has_attachments: false,
            spam_headers: Vec::new(),
            assessment: None,
            authentication_results: Vec::new(),
        };
        self.state.lock().unwrap().messages.push(StoredMessage {
            message: Message {
                calendar: None,
                summary,
                headers: vec![
                    ("Message-ID".into(), message_id),
                    ("From".into(), from),
                    ("Subject".into(), subject),
                ],
                text: body,
                attachments: Vec::new(),
            },
            flags: StoredFlags::default(),
            archived: false,
        });
        id
    }

    /// Store a message with the id and headers supplied by the test.
    pub fn insert(&self, message: Message) {
        let mut state = self.state.lock().unwrap();
        state
            .messages
            .retain(|stored| stored.message.summary.id != message.summary.id);
        state.messages.push(StoredMessage {
            message,
            flags: StoredFlags::default(),
            archived: false,
        });
    }

    pub fn attach(&self, id: &MessageId, attachment: Attachment) {
        let mut state = self.state.lock().unwrap();
        let stored = state
            .messages
            .iter_mut()
            .find(|stored| stored.message.summary.id == *id)
            .expect("the message exists");
        stored.message.summary.has_attachments = true;
        stored.message.attachments.push(attachment);
    }

    pub fn remove(&self, id: &MessageId) {
        self.state
            .lock()
            .unwrap()
            .messages
            .retain(|stored| stored.message.summary.id != *id);
    }

    /// Every operation, including open, fails until the code is cleared.
    pub fn fail_with(&self, code: Option<TransportErrorCode>) {
        self.state.lock().unwrap().fail_with = code;
    }

    pub fn sent(&self) -> Vec<OutgoingMessage> {
        self.state.lock().unwrap().sent.clone()
    }

    /// Summaries in arrival order, in the archive or outside it.
    pub fn summaries(&self, archived: bool) -> Vec<MessageSummary> {
        self.state
            .lock()
            .unwrap()
            .messages
            .iter()
            .filter(|stored| stored.archived == archived)
            .map(|stored| stored.message.summary.clone())
            .collect()
    }

    pub fn flags_of(&self, id: &MessageId) -> Option<StoredFlags> {
        self.state
            .lock()
            .unwrap()
            .messages
            .iter()
            .find(|stored| stored.message.summary.id == *id)
            .map(|stored| stored.flags)
    }
}

impl State {
    fn check(&self) -> Result<(), TransportError> {
        match self.fail_with {
            Some(code) => Err(TransportError(code)),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl Mailboxes for MemoryMailbox {
    async fn open(&self, _mailbox: &AgentMailbox) -> Result<Box<dyn Mailbox>, MailboxError> {
        self.state.lock().unwrap().check()?;
        Ok(Box::new(self.clone()))
    }
}

#[async_trait]
impl Mailbox for MemoryMailbox {
    async fn search(
        &mut self,
        query: &MailQuery,
        page: &PageRequest,
    ) -> Result<Page<MessageSummary>, TransportError> {
        let skip = offset(page)?;
        let state = self.state.lock().unwrap();
        state.check()?;
        let contains = |field: &str, wanted: &Option<String>| {
            wanted
                .as_ref()
                .is_none_or(|wanted| field.to_lowercase().contains(&wanted.to_lowercase()))
        };
        let found: Vec<_> = state
            .messages
            .iter()
            .rev()
            .filter(|stored| {
                let summary = &stored.message.summary;
                !stored.archived
                    && contains(&summary.from, &query.from)
                    && contains(&summary.to.join(","), &query.to)
                    && contains(&summary.subject, &query.subject)
                    && (contains(&stored.message.text, &query.text)
                        || stored.message.headers.iter().any(|(name, value)| {
                            contains(&format!("{name}: {value}"), &query.text)
                        }))
                    && query.since.is_none_or(|since| summary.date >= since)
                    && query.before.is_none_or(|before| summary.date < before)
            })
            .map(|stored| stored.message.summary.clone())
            .collect();
        let next = (found.len() > skip.saturating_add(page.limit))
            .then(|| format!("after/{}", skip.saturating_add(page.limit)));
        Ok(Page {
            items: found.into_iter().skip(skip).take(page.limit).collect(),
            next,
        })
    }

    async fn message(&mut self, id: &MessageId) -> Result<Message, TransportError> {
        let state = self.state.lock().unwrap();
        state.check()?;
        state
            .messages
            .iter()
            .find(|stored| stored.message.summary.id == *id)
            .map(|stored| stored.message.clone())
            .ok_or(TransportError(TransportErrorCode::StaleId))
    }

    async fn quarantined_message(
        &mut self,
        id: &MessageId,
    ) -> Result<crate::WorkspaceMailMessage, TransportError> {
        let state = self.state.lock().unwrap();
        state.check()?;
        state
            .messages
            .iter()
            .find(|stored| stored.message.summary.id == *id)
            .map(|stored| crate::WorkspaceMailMessage {
                message: stored.message.clone(),
                flags: crate::MailMetadata {
                    seen: stored.flags.seen,
                    flagged: stored.flags.flagged,
                    labels: Vec::new(),
                },
                labels: Vec::new(),
            })
            .ok_or(TransportError(TransportErrorCode::StaleId))
    }

    async fn thread(
        &mut self,
        id: &str,
        page: &PageRequest,
    ) -> Result<Page<Message>, TransportError> {
        let skip = offset(page)?;
        let state = self.state.lock().unwrap();
        state.check()?;
        let mut found: Vec<_> = state
            .messages
            .iter()
            .filter(|stored| stored.message.summary.thread_id == id)
            .map(|stored| stored.message.clone())
            .collect();
        found.sort_by_key(|message| message.summary.date);
        let end = found.len().saturating_sub(skip);
        let start = end.saturating_sub(page.limit);
        let next = (start > 0).then(|| format!("after/{}", skip.saturating_add(end - start)));
        Ok(Page {
            items: found[start..end].to_vec(),
            next,
        })
    }

    async fn modify(
        &mut self,
        id: &MessageId,
        change: MessageChange,
    ) -> Result<(), TransportError> {
        let mut state = self.state.lock().unwrap();
        state.check()?;
        let stored = state
            .messages
            .iter_mut()
            .find(|stored| stored.message.summary.id == *id)
            .ok_or(TransportError(TransportErrorCode::StaleId))?;
        if let Some(seen) = change.flags.seen {
            stored.flags.seen = seen;
        }
        if let Some(flagged) = change.flags.flagged {
            stored.flags.flagged = flagged;
        }
        if change.archive {
            stored.archived = true;
        }
        Ok(())
    }

    async fn send(&mut self, message: &OutgoingMessage) -> Result<SentMessage, TransportError> {
        let mut state = self.state.lock().unwrap();
        state.check()?;
        state.sent.push(message.clone());
        Ok(SentMessage {
            message_id: format!(
                "<{}>",
                message
                    .message_id
                    .clone()
                    .unwrap_or_else(|| format!("{}@memory.invalid", ulid::Ulid::new()))
            ),
        })
    }
}

fn offset(page: &PageRequest) -> Result<usize, TransportError> {
    match page.token.as_deref() {
        None => Ok(0),
        Some(token) => token
            .strip_prefix("after/")
            .and_then(|offset| offset.parse().ok())
            .ok_or(TransportError(TransportErrorCode::InvalidRequest)),
    }
}
