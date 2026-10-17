//! The IMAP rules behind the Mailbox operations.

use async_trait::async_trait;

use crate::{
    Cursor, INBOX, MailQuery, MailSession, Mailbox, Message, MessageChange, MessageId,
    MessageSummary, OutgoingMessage, Page, PageRequest, SentMessage, TransportError,
    TransportErrorCode,
};

/// One folder and IMAP UID. Only the IMAP path reads this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImapMessageId {
    pub folder: String,
    pub uid: u32,
}

impl ImapMessageId {
    pub fn new(folder: impl Into<String>, uid: u32) -> Self {
        Self {
            folder: folder.into(),
            uid,
        }
    }
}

impl std::fmt::Display for ImapMessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.folder, self.uid)
    }
}

impl std::str::FromStr for ImapMessageId {
    type Err = TransportError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let stale = || TransportError(TransportErrorCode::StaleId);
        let (folder, uid) = text.rsplit_once(':').ok_or_else(stale)?;
        let uid = uid.parse::<u32>().map_err(|_| stale())?;
        if folder.is_empty() || uid == 0 {
            return Err(stale());
        }
        Ok(Self::new(folder, uid))
    }
}

impl From<ImapMessageId> for MessageId {
    fn from(id: ImapMessageId) -> Self {
        Self::new(id.to_string())
    }
}

/// An IMAP session with the numbering the collector last sees.
pub struct ImapMailbox {
    session: Box<dyn MailSession>,
    cursor: Option<Cursor>,
    intake: crate::MailReadContext,
}

impl ImapMailbox {
    pub fn new(
        session: Box<dyn MailSession>,
        cursor: Option<Cursor>,
        intake: crate::MailReadContext,
    ) -> Self {
        Self {
            session,
            cursor,
            intake,
        }
    }

    async fn assess(&mut self, summary: &mut MessageSummary) -> Result<(), TransportError> {
        let folder = summary.id.as_str().parse::<ImapMessageId>()?.folder;
        let in_junk = self
            .session
            .junk_folders()
            .await?
            .iter()
            .any(|f| f.eq_ignore_ascii_case(&folder));
        self.intake.assess(summary, in_junk).await
    }

    async fn select_checked(&mut self, folder: &str) -> Result<(), TransportError> {
        let state = self.session.select(folder).await?;
        if self.cursor.as_ref().is_some_and(|cursor| {
            cursor.folder == folder && cursor.uid_validity != state.uid_validity
        }) {
            return Err(TransportError(TransportErrorCode::StaleId));
        }
        Ok(())
    }
}

#[async_trait]
impl Mailbox for ImapMailbox {
    async fn search(
        &mut self,
        query: &MailQuery,
        page: &PageRequest,
    ) -> Result<Page<MessageSummary>, TransportError> {
        let skip = offset(page)?;
        self.select_checked(INBOX).await?;
        let mut query = query.clone();
        query.limit =
            u32::try_from(skip.saturating_add(page.limit).saturating_add(1)).unwrap_or(u32::MAX);
        let mut found = self.session.search(&query).await?;
        for summary in &mut found {
            self.assess(summary).await?;
        }
        let next = (found.len() > skip.saturating_add(page.limit))
            .then(|| skip.saturating_add(page.limit).to_string());
        Ok(Page {
            items: found
                .into_iter()
                .filter(|m| m.require_visible().is_ok())
                .skip(skip)
                .take(page.limit)
                .collect(),
            next,
        })
    }

    async fn message(&mut self, id: &MessageId) -> Result<Message, TransportError> {
        let imap = id.as_str().parse::<ImapMessageId>()?;
        self.select_checked(&imap.folder).await?;
        let mut message = self.session.fetch(id).await?;
        self.assess(&mut message.summary).await?;
        message.summary.require_visible()?;
        Ok(message)
    }

    async fn quarantined_message(
        &mut self,
        id: &MessageId,
    ) -> Result<crate::WorkspaceMailMessage, TransportError> {
        let imap = id.as_str().parse::<ImapMessageId>()?;
        self.session.select(&imap.folder).await?;
        let mut message = self.session.fetch(id).await?;
        self.assess(&mut message.summary).await?;
        let flags = self.session.metadata(id).await?;
        Ok(crate::WorkspaceMailMessage {
            labels: flags.labels.clone(),
            flags,
            message,
        })
    }

    async fn thread(
        &mut self,
        id: &str,
        page: &PageRequest,
    ) -> Result<Page<Message>, TransportError> {
        let skip = offset(page)?;
        self.select_checked(INBOX).await?;
        let mut found = self.session.fetch_thread(id).await?;
        for message in &mut found {
            self.assess(&mut message.summary).await?;
        }
        found.retain(|m| m.summary.require_visible().is_ok());
        let end = found.len().saturating_sub(skip);
        let start = end.saturating_sub(page.limit);
        let next = (start > 0).then(|| skip.saturating_add(end - start).to_string());
        Ok(Page {
            items: found.drain(start..end).collect(),
            next,
        })
    }

    async fn modify(
        &mut self,
        id: &MessageId,
        change: MessageChange,
    ) -> Result<(), TransportError> {
        self.message(id).await?;
        if change.flags.seen.is_some() || change.flags.flagged.is_some() {
            self.session.set_flags(id, change.flags).await?;
        }
        if change.archive {
            self.session.move_to_archive(id).await?;
        }
        Ok(())
    }

    async fn send(&mut self, message: &OutgoingMessage) -> Result<SentMessage, TransportError> {
        self.session.send(message).await
    }
}

fn offset(page: &PageRequest) -> Result<usize, TransportError> {
    match page.token.as_deref() {
        None => Ok(0),
        Some(token) => token
            .parse()
            .map_err(|_| TransportError(TransportErrorCode::InvalidRequest)),
    }
}
