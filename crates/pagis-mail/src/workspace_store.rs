//! The read store of one Workspace Mailbox, independent of HTTP.
use crate::{
    FlagChange, ImapMessageId, MailQuery, MailSession, Message, MessageId, MessageSummary, Page,
    PageRequest, TransportError, TransportErrorCode,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// The host's flags and custom IMAP keywords. System flags are not labels.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailMetadata {
    pub seen: bool,
    pub flagged: bool,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceMailSummary {
    pub summary: MessageSummary,
    pub flags: MailMetadata,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceMailMessage {
    pub message: Message,
    pub flags: MailMetadata,
    pub labels: Vec<String>,
}

/// One open Workspace Mailbox. The store has no send operation.
#[async_trait]
pub trait WorkspaceMailStore: Send {
    async fn folders(&mut self) -> Result<Vec<String>, TransportError>;
    /// Newest first. An empty folder reads all folders. Search covers
    /// the sender, subject and body.
    async fn messages(
        &mut self,
        folder: &str,
        q: &str,
        page: &PageRequest,
    ) -> Result<Page<WorkspaceMailSummary>, TransportError>;
    async fn message(&mut self, id: &MessageId) -> Result<WorkspaceMailMessage, TransportError>;
    /// Every folder contributes to the thread, oldest first.
    async fn thread(&mut self, id: &str) -> Result<Vec<WorkspaceMailMessage>, TransportError>;
    async fn set_flags(&mut self, id: &MessageId, flags: FlagChange) -> Result<(), TransportError>;
    async fn close(&mut self) -> Result<(), TransportError>;
}

pub struct ImapWorkspaceMailStore {
    session: Box<dyn MailSession>,
}
impl ImapWorkspaceMailStore {
    pub fn new(session: Box<dyn MailSession>) -> Self {
        Self { session }
    }
    async fn with_metadata(
        &mut self,
        message: Message,
    ) -> Result<WorkspaceMailMessage, TransportError> {
        let flags = self.session.metadata(&message.summary.id).await?;
        Ok(WorkspaceMailMessage {
            labels: flags.labels.clone(),
            flags,
            message,
        })
    }
}
#[async_trait]
impl WorkspaceMailStore for ImapWorkspaceMailStore {
    async fn folders(&mut self) -> Result<Vec<String>, TransportError> {
        self.session.folders().await
    }
    async fn messages(
        &mut self,
        folder: &str,
        q: &str,
        page: &PageRequest,
    ) -> Result<Page<WorkspaceMailSummary>, TransportError> {
        let invalid = || TransportError(TransportErrorCode::InvalidRequest);
        if page.limit == 0 || page.limit > 100 {
            return Err(invalid());
        }
        let mut cursor = match &page.token {
            Some(token) => serde_json::from_str::<MailPageCursor>(token).map_err(|_| invalid())?,
            None => MailPageCursor {
                folder: folder.into(),
                q: q.into(),
                bounds: BTreeMap::new(),
            },
        };
        if cursor.folder != folder || cursor.q != q {
            return Err(invalid());
        }
        let folders = if folder.is_empty() {
            self.session.folders().await?
        } else {
            vec![folder.into()]
        };
        let mut pending = BTreeMap::new();
        for folder in folders {
            let state = self.session.select(&folder).await?;
            let (validity, before) = *cursor
                .bounds
                .entry(folder.clone())
                .or_insert((state.uid_validity, state.next_uid));
            if validity != state.uid_validity {
                return Err(invalid());
            }
            let query = MailQuery {
                limit: (page.limit + 1) as u32,
                before_uid: Some(before),
                text: (!q.is_empty()).then(|| q.into()),
                ..Default::default()
            };
            let found = self.session.search(&query).await?;
            pending.insert(folder, VecDeque::from(found));
        }
        let mut items = Vec::new();
        // Merge the newest messages of each folder. The UID bound advances
        // only for messages on this page, so other folders lose no mail.
        while items.len() < page.limit {
            let folder = pending
                .iter()
                .filter_map(|(folder, messages)| {
                    messages
                        .front()
                        .map(|m| (folder.clone(), (m.date, uid(&m.id))))
                })
                .max_by_key(|(_, order)| *order)
                .map(|(folder, _)| folder);
            let Some(folder) = folder else {
                break;
            };
            let summary = pending
                .get_mut(&folder)
                .and_then(VecDeque::pop_front)
                .ok_or_else(invalid)?;
            cursor.bounds.get_mut(&folder).ok_or_else(invalid)?.1 = uid(&summary.id);
            let flags = self.session.metadata(&summary.id).await?;
            items.push(WorkspaceMailSummary {
                labels: flags.labels.clone(),
                flags,
                summary,
            });
        }
        let next = if pending.values().any(|messages| !messages.is_empty()) {
            Some(serde_json::to_string(&cursor).map_err(|_| invalid())?)
        } else {
            None
        };
        Ok(Page { items, next })
    }
    async fn message(&mut self, id: &MessageId) -> Result<WorkspaceMailMessage, TransportError> {
        let folder = id.as_str().parse::<ImapMessageId>()?.folder;
        self.session.select(&folder).await?;
        let message = self.session.fetch(id).await?;
        self.with_metadata(message).await
    }
    async fn thread(&mut self, id: &str) -> Result<Vec<WorkspaceMailMessage>, TransportError> {
        let mut messages = Vec::new();
        for folder in self.session.folders().await? {
            self.session.select(&folder).await?;
            for message in self.session.fetch_thread(id).await? {
                messages.push(self.with_metadata(message).await?);
            }
        }
        messages.sort_by_key(|m| m.message.summary.date);
        Ok(messages)
    }
    async fn set_flags(&mut self, id: &MessageId, flags: FlagChange) -> Result<(), TransportError> {
        self.session
            .select(&id.as_str().parse::<ImapMessageId>()?.folder)
            .await?;
        self.session.set_flags(id, flags).await
    }
    async fn close(&mut self) -> Result<(), TransportError> {
        self.session.logout().await
    }
}

#[derive(Serialize, Deserialize)]
struct MailPageCursor {
    folder: String,
    q: String,
    bounds: BTreeMap<String, (u32, u32)>,
}
fn uid(id: &MessageId) -> u32 {
    id.as_str()
        .parse::<ImapMessageId>()
        .map(|id| id.uid)
        .unwrap_or(0)
}
