//! The store of one Workspace Mailbox, independent of HTTP.
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

/// The write operations of a Workspace Mailbox.
#[async_trait]
pub trait WorkspaceMailWriter: Send {
    async fn mark_spam(&mut self, id: &MessageId, spam: bool) -> Result<(), TransportError>;
    async fn send(
        &mut self,
        message: &crate::OutgoingMessage,
    ) -> Result<crate::SentMessage, TransportError>;
    async fn create_folder(&mut self, name: &str) -> Result<(), TransportError>;
    async fn rename_folder(&mut self, name: &str, new_name: &str) -> Result<(), TransportError>;
    async fn delete_folder(&mut self, name: &str) -> Result<(), TransportError>;
    /// Move mail to Trash, or remove it for good when it is in Trash.
    async fn delete_message(&mut self, id: &MessageId) -> Result<(), TransportError>;
    async fn move_message(&mut self, id: &MessageId, folder: &str) -> Result<(), TransportError>;
    async fn set_label(
        &mut self,
        id: &MessageId,
        label: &str,
        apply: bool,
    ) -> Result<(), TransportError>;
}

/// The read operations and session lifetime of a Workspace Mailbox.
#[async_trait]
pub trait WorkspaceMailReader: Send {
    async fn folder_state(&mut self, folder: &str) -> Result<crate::FolderState, TransportError>;
    /// At most 50 messages above the cursor, in UID order, without assessment.
    /// A changed UIDVALIDITY returns `stale_id`.
    async fn messages_since(
        &mut self,
        cursor: &crate::Cursor,
    ) -> Result<(Vec<MessageId>, crate::Cursor), TransportError>;
    async fn quarantined_message(
        &mut self,
        id: &MessageId,
    ) -> Result<WorkspaceMailMessage, TransportError>;
    async fn junk_folder(&mut self) -> Result<String, TransportError>;
    async fn attachment(
        &mut self,
        id: &MessageId,
        index: usize,
    ) -> Result<crate::OutgoingAttachment, TransportError>;
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

/// One open store exposes both interfaces to the Product API.
pub trait WorkspaceMailStore: WorkspaceMailReader + WorkspaceMailWriter {}
impl<T: WorkspaceMailReader + WorkspaceMailWriter> WorkspaceMailStore for T {}

pub struct ImapWorkspaceMailStore {
    session: Option<Box<dyn MailSession>>,
    connection: Option<(
        std::sync::Arc<dyn crate::MailTransport>,
        crate::MailboxCredential,
    )>,
    intake: crate::MailReadContext,
}
impl ImapWorkspaceMailStore {
    pub fn new(session: Box<dyn MailSession>, intake: crate::MailReadContext) -> Self {
        Self {
            session: Some(session),
            connection: None,
            intake,
        }
    }
    pub(crate) fn connect(
        transport: std::sync::Arc<dyn crate::MailTransport>,
        credential: crate::MailboxCredential,
        intake: crate::MailReadContext,
    ) -> Self {
        Self {
            session: None,
            connection: Some((transport, credential)),
            intake,
        }
    }
    async fn session(&mut self) -> Result<&mut Box<dyn MailSession>, TransportError> {
        if self.session.is_none() {
            let (transport, credential) = self
                .connection
                .as_ref()
                .expect("an unopened store has a connection");
            self.session = Some(transport.login(credential).await?);
        }
        Ok(self
            .session
            .as_mut()
            .expect("the store has an open session"))
    }
    async fn assess(&mut self, summary: &mut MessageSummary) -> Result<(), TransportError> {
        let folder = summary.id.as_str().parse::<ImapMessageId>()?.folder;
        let in_junk = self
            .session()
            .await?
            .junk_folders()
            .await?
            .iter()
            .any(|f| f.eq_ignore_ascii_case(&folder));
        self.intake.assess(summary, in_junk).await
    }
    async fn ensure_trash(&mut self) -> Result<String, TransportError> {
        if let Some(folder) = self.session().await?.trash_folder().await? {
            return Ok(folder);
        }
        self.session().await?.create_folder("Trash").await?;
        Ok("Trash".into())
    }
    async fn mutable_folder(&mut self, name: &str) -> Result<(), TransportError> {
        valid_folder(name)?;
        if is_system_folder(name)
            || self
                .session()
                .await?
                .system_folders()
                .await?
                .iter()
                .any(|s| name.eq_ignore_ascii_case(s))
        {
            return Err(TransportError(TransportErrorCode::InvalidRequest));
        }
        Ok(())
    }
    async fn with_metadata(
        &mut self,
        mut message: Message,
    ) -> Result<WorkspaceMailMessage, TransportError> {
        self.assess(&mut message.summary).await?;
        message.summary.require_visible()?;
        let flags = self.session().await?.metadata(&message.summary.id).await?;
        Ok(WorkspaceMailMessage {
            labels: flags.labels.clone(),
            flags,
            message,
        })
    }
}
#[async_trait]
impl WorkspaceMailWriter for ImapWorkspaceMailStore {
    async fn mark_spam(&mut self, id: &MessageId, spam: bool) -> Result<(), TransportError> {
        self.intake.clone().mark_spam(self, id, spam).await
    }
    async fn send(
        &mut self,
        message: &crate::OutgoingMessage,
    ) -> Result<crate::SentMessage, TransportError> {
        self.session().await?.send(message).await
    }
    async fn create_folder(&mut self, name: &str) -> Result<(), TransportError> {
        valid_folder(name)?;
        self.session().await?.create_folder(name).await
    }
    async fn rename_folder(&mut self, name: &str, new_name: &str) -> Result<(), TransportError> {
        self.mutable_folder(name).await?;
        self.mutable_folder(new_name).await?;
        self.session().await?.rename_folder(name, new_name).await
    }
    async fn delete_folder(&mut self, name: &str) -> Result<(), TransportError> {
        self.mutable_folder(name).await?;
        // Check every message before the first move. Product reads omit quarantine.
        let mut before_uid = None;
        loop {
            self.session().await?.select(name).await?;
            let messages = self
                .session()
                .await?
                .search(&MailQuery {
                    limit: 100,
                    before_uid,
                    ..Default::default()
                })
                .await?;
            if messages.is_empty() {
                break;
            }
            before_uid = messages.iter().map(|message| uid(&message.id)).min();
            for message in messages {
                self.message(&message.id).await?;
            }
        }
        let trash = self.ensure_trash().await?;
        loop {
            self.session().await?.select(name).await?;
            let messages = self
                .session()
                .await?
                .search(&MailQuery {
                    limit: 100,
                    ..Default::default()
                })
                .await?;
            if messages.is_empty() {
                break;
            }
            for message in messages {
                self.move_message(&message.id, &trash).await?;
            }
        }
        self.session().await?.delete_folder(name).await
    }
    async fn delete_message(&mut self, id: &MessageId) -> Result<(), TransportError> {
        self.message(id).await?;
        let folder = id.as_str().parse::<ImapMessageId>()?.folder;
        let trash = self.ensure_trash().await?;
        if folder.eq_ignore_ascii_case(&trash) {
            self.session().await?.expunge_message(id).await
        } else {
            self.move_message(id, &trash).await
        }
    }
    async fn move_message(&mut self, id: &MessageId, folder: &str) -> Result<(), TransportError> {
        valid_folder(folder)?;
        let message = self.message(id).await?;
        let moved = self.session().await?.move_message(id, folder).await?;
        self.intake.relocate(&message.message.summary, &moved).await
    }
    async fn set_label(
        &mut self,
        id: &MessageId,
        label: &str,
        apply: bool,
    ) -> Result<(), TransportError> {
        self.message(id).await?;
        if label.is_empty()
            || !label
                .bytes()
                .all(|b| b.is_ascii_graphic() && !b"(){%*\"\\]".contains(&b))
        {
            return Err(TransportError(TransportErrorCode::InvalidRequest));
        }
        self.session().await?.set_label(id, label, apply).await
    }
}

#[async_trait]
impl WorkspaceMailReader for ImapWorkspaceMailStore {
    async fn folder_state(&mut self, folder: &str) -> Result<crate::FolderState, TransportError> {
        self.session().await?.select(folder).await
    }
    async fn messages_since(
        &mut self,
        cursor: &crate::Cursor,
    ) -> Result<(Vec<MessageId>, crate::Cursor), TransportError> {
        let (messages, next) = self.session().await?.fetch_since(cursor).await?;
        Ok((messages.into_iter().map(|m| m.id).collect(), next))
    }
    async fn quarantined_message(
        &mut self,
        id: &MessageId,
    ) -> Result<WorkspaceMailMessage, TransportError> {
        self.session()
            .await?
            .select(&id.as_str().parse::<ImapMessageId>()?.folder)
            .await?;
        let mut message = self.session().await?.fetch(id).await?;
        self.assess(&mut message.summary).await?;
        let flags = self.session().await?.metadata(id).await?;
        Ok(WorkspaceMailMessage {
            labels: flags.labels.clone(),
            flags,
            message,
        })
    }
    async fn junk_folder(&mut self) -> Result<String, TransportError> {
        let folders = self.session().await?.junk_folders().await?;
        if let Some(folder) = folders.first() {
            return Ok(folder.clone());
        }
        self.session().await?.create_folder("Junk").await?;
        Ok("Junk".into())
    }
    async fn attachment(
        &mut self,
        id: &MessageId,
        index: usize,
    ) -> Result<crate::OutgoingAttachment, TransportError> {
        self.message(id).await?;
        self.session().await?.attachment(id, index).await
    }
    async fn folders(&mut self) -> Result<Vec<String>, TransportError> {
        self.junk_folder().await?;
        self.session().await?.folders().await
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
        let moving = self.intake.moves_junk().await?;
        let junk = self.junk_folder().await?;
        let folders = if folder.is_empty() {
            self.session().await?.folders().await?
        } else {
            vec![if folder.eq_ignore_ascii_case("Junk") {
                self.junk_folder().await?
            } else {
                folder.into()
            }]
        };
        let mut pending = BTreeMap::new();
        for folder in folders {
            let state = self.session().await?.select(&folder).await?;
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
            let found = self.session().await?.search(&query).await?;
            pending.insert(folder, VecDeque::from(found));
        }
        let mut items = Vec::new();
        // Merge the newest messages of each folder. The UID bound advances
        // only for messages on this page, so other folders lose no mail.
        for _ in 0..page.limit {
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
            let mut summary = pending
                .get_mut(&folder)
                .and_then(VecDeque::pop_front)
                .ok_or_else(invalid)?;
            cursor.bounds.get_mut(&folder).ok_or_else(invalid)?.1 = uid(&summary.id);
            self.assess(&mut summary).await?;
            if moving && !folder.eq_ignore_ascii_case(&junk) && summary.is_junk() {
                let moved = self
                    .session()
                    .await?
                    .move_message(&summary.id, &junk)
                    .await?;
                self.intake.relocate(&summary, &moved).await?;
                continue;
            }
            if summary.require_visible().is_err() {
                continue;
            }
            let flags = self.session().await?.metadata(&summary.id).await?;
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
        self.session().await?.select(&folder).await?;
        let message = self.session().await?.fetch(id).await?;
        self.with_metadata(message).await
    }
    async fn thread(&mut self, id: &str) -> Result<Vec<WorkspaceMailMessage>, TransportError> {
        // Mail sent to the own address is in INBOX and in Sent with one
        // Message-ID. The thread shows each Message-ID once.
        let mut messages = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for folder in self.session().await?.folders().await? {
            self.session().await?.select(&folder).await?;
            for message in self.session().await?.fetch_thread(id).await? {
                let message_id = message.summary.message_id.clone();
                if message_id.is_empty() || seen.insert(message_id) {
                    match self.with_metadata(message).await {
                        Ok(message) => messages.push(message),
                        Err(TransportError(TransportErrorCode::StaleId)) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        messages.sort_by_key(|m| m.message.summary.date);
        Ok(messages)
    }
    async fn set_flags(&mut self, id: &MessageId, flags: FlagChange) -> Result<(), TransportError> {
        self.message(id).await?;
        self.session()
            .await?
            .select(&id.as_str().parse::<ImapMessageId>()?.folder)
            .await?;
        self.session().await?.set_flags(id, flags).await
    }
    async fn close(&mut self) -> Result<(), TransportError> {
        match self.session.as_mut() {
            Some(session) => session.logout().await,
            None => Ok(()),
        }
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

pub(crate) fn is_system_folder(name: &str) -> bool {
    [
        "INBOX",
        "Sent",
        "Sent Messages",
        "Sent Items",
        "Drafts",
        "Junk",
        "Trash",
    ]
    .iter()
    .any(|system| name.eq_ignore_ascii_case(system))
}

pub(crate) fn valid_folder(name: &str) -> Result<(), TransportError> {
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(TransportError(TransportErrorCode::InvalidRequest));
    }
    Ok(())
}

/// Reply to the sender, or to all visible recipients except this mailbox.
pub fn reply_message(original: &Message, own: &str, all: bool) -> crate::OutgoingMessage {
    use mail_parser::MessageParser;
    let raw = original
        .headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}\r\n"))
        .collect::<String>()
        + "\r\n";
    let parsed = MessageParser::default().parse(raw.as_bytes());
    let mut recipients = vec![original.summary.from.clone()];
    if all || original.summary.from.eq_ignore_ascii_case(own) {
        recipients.extend(original.summary.to.clone());
    }
    let mut cc = Vec::new();
    if all && let Some(addresses) = parsed.as_ref().and_then(|m| m.cc()) {
        cc.extend(
            addresses
                .iter()
                .filter_map(|a| a.address.as_ref().map(ToString::to_string)),
        );
    }
    let mut seen = std::collections::HashSet::from([own.to_lowercase()]);
    recipients.retain(|a| seen.insert(a.to_lowercase()));
    cc.retain(|a| seen.insert(a.to_lowercase()));
    let mut references = parsed
        .as_ref()
        .and_then(|m| m.references().as_text_list())
        .map(|r| r.iter().map(|id| format!("<{id}>")).collect::<Vec<_>>())
        .unwrap_or_default();
    if !original.summary.message_id.is_empty()
        && references.last() != Some(&original.summary.message_id)
    {
        references.push(original.summary.message_id.clone());
    }
    crate::OutgoingMessage {
        to: recipients,
        cc,
        subject: prefixed("Re:", &original.summary.subject),
        body: quoted_text(original),
        references,
        in_reply_to: (!original.summary.message_id.is_empty())
            .then(|| original.summary.message_id.clone()),
        ..Default::default()
    }
}

/// Forward starts a new thread and quotes the original text.
pub fn forward_message(original: &Message) -> crate::OutgoingMessage {
    crate::OutgoingMessage {
        subject: prefixed("Fwd:", &original.summary.subject),
        body: quoted_text(original),
        ..Default::default()
    }
}
fn prefixed(prefix: &str, subject: &str) -> String {
    if subject.to_lowercase().starts_with(&prefix.to_lowercase()) {
        subject.into()
    } else {
        format!("{prefix} {subject}")
    }
}
fn quoted_text(original: &Message) -> String {
    format!(
        "\n\nFrom: {}\nSubject: {}\n{}",
        original.summary.from,
        original.summary.subject,
        original
            .text
            .lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}
