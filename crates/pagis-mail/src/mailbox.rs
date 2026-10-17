//! The operations the Mail Tools use, independent of a mail protocol.

use async_trait::async_trait;
use pagis_core::AgentMailbox;

use crate::{
    FlagChange, MailQuery, MailboxError, Message, MessageSummary, OutgoingMessage, SentMessage,
    TransportError,
};

/// An opaque message identity. Only the Mailbox reads its contents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageId(String);

impl MessageId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One request for a page. The Mailbox makes and reads the token.
#[derive(Debug, Clone)]
pub struct PageRequest {
    pub token: Option<String>,
    pub limit: usize,
}

impl PageRequest {
    pub fn new(token: Option<String>, limit: usize) -> Self {
        Self { token, limit }
    }
}

/// A page in the Mailbox's order, with the token for the next page.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

/// Reversible flags and an archive operation. Flags change before archive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MessageChange {
    pub flags: FlagChange,
    pub archive: bool,
}

/// One open mailbox. No operation requires a selected folder or a UID.
#[async_trait]
pub trait Mailbox: Send {
    /// Matching summaries, newest first.
    async fn search(
        &mut self,
        query: &MailQuery,
        page: &PageRequest,
    ) -> Result<Page<MessageSummary>, TransportError>;
    async fn message(&mut self, id: &MessageId) -> Result<Message, TransportError>;
    /// One message with its flags, quarantined or not. Only the
    /// Administrator's quarantine read uses it.
    async fn quarantined_message(
        &mut self,
        id: &MessageId,
    ) -> Result<crate::WorkspaceMailMessage, TransportError>;
    /// The newest page of a thread, oldest first within the page.
    async fn thread(
        &mut self,
        id: &str,
        page: &PageRequest,
    ) -> Result<Page<Message>, TransportError>;
    async fn modify(&mut self, id: &MessageId, change: MessageChange)
    -> Result<(), TransportError>;
    async fn send(&mut self, message: &OutgoingMessage) -> Result<SentMessage, TransportError>;
}

/// Open mail as the identity of an Agent Mailbox.
#[async_trait]
pub trait Mailboxes: Send + Sync {
    async fn open(&self, mailbox: &AgentMailbox) -> Result<Box<dyn Mailbox>, MailboxError>;
}
