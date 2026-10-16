//! Workspace mail in memory, with opaque message ids.
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_core::{Connection, WorkspaceId, WorkspaceMailbox, WorkspaceMailboxRecordStore};

use super::MemoryMailbox;
use crate::{
    FlagChange, MailMetadata, MailReadContext, Mailbox, MailboxError, MessageId, NewMailbox,
    OutgoingAttachment, OutgoingMessage, Page, PageRequest, SentMessage, TransportError,
    TransportErrorCode, WorkspaceMailHost, WorkspaceMailMessage, WorkspaceMailReader,
    WorkspaceMailStore, WorkspaceMailSummary, WorkspaceMailWriter,
};

#[derive(Default)]
pub struct MemoryWorkspaceMailHost {
    stores: Mutex<HashMap<WorkspaceId, MemoryWorkspaceMailStore>>,
}

impl MemoryWorkspaceMailHost {
    pub fn store(&self, workspace: &WorkspaceId) -> Option<MemoryWorkspaceMailStore> {
        self.stores.lock().unwrap().get(workspace).cloned()
    }
}

#[async_trait]
impl WorkspaceMailHost for MemoryWorkspaceMailHost {
    async fn provision(
        &self,
        _connection: &Connection,
        mailbox: &WorkspaceMailbox,
        _request: &NewMailbox,
        context: MailReadContext,
        records: &dyn WorkspaceMailboxRecordStore,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError> {
        records
            .create(mailbox)
            .await
            .map_err(crate::desk::store_error)?;
        let store = MemoryWorkspaceMailStore::default();
        self.stores
            .lock()
            .unwrap()
            .insert(mailbox.workspace_id.clone(), store.clone());
        Ok(Box::new(store.with_context(context)))
    }

    async fn open(
        &self,
        _connection: &Connection,
        mailbox: &WorkspaceMailbox,
        context: MailReadContext,
    ) -> Result<Box<dyn WorkspaceMailStore>, MailboxError> {
        let store = self
            .store(&mailbox.workspace_id)
            .ok_or(MailboxError::NotFound)?;
        Ok(Box::new(store.with_context(context)))
    }
}

#[derive(Clone, Default)]
pub struct MemoryWorkspaceMailStore {
    mail: MemoryMailbox,
    state: Arc<Mutex<State>>,
    context: Option<MailReadContext>,
}

struct State {
    folders: BTreeSet<String>,
    entries: HashMap<String, Entry>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            folders: BTreeSet::from(["INBOX".into()]),
            entries: HashMap::new(),
        }
    }
}
struct Entry {
    folder: String,
    flags: MailMetadata,
    attachments: Vec<OutgoingAttachment>,
}

impl MemoryWorkspaceMailStore {
    fn with_context(mut self, context: MailReadContext) -> Self {
        self.context = Some(context);
        self
    }

    pub fn deliver(&self, from: &str, subject: &str, body: &str) -> MessageId {
        let id = self.mail.deliver(from, subject, body);
        self.state.lock().unwrap().entries.insert(
            id.to_string(),
            Entry {
                folder: "INBOX".into(),
                flags: MailMetadata::default(),
                attachments: vec![],
            },
        );
        id
    }

    pub fn deliver_raw(&self, raw: &[u8]) -> Result<MessageId, TransportError> {
        let id = MessageId::new(ulid::Ulid::new().to_string());
        let message = crate::read_mime(raw, id.clone())?;
        let attachments = (0..message.attachments.len())
            .map(|index| crate::mime::read_attachment(raw, index))
            .collect::<Result<Vec<_>, _>>()?;
        self.mail.insert(message);
        self.state.lock().unwrap().entries.insert(
            id.to_string(),
            Entry {
                folder: "INBOX".into(),
                flags: MailMetadata::default(),
                attachments,
            },
        );
        Ok(id)
    }

    pub fn attach(&self, id: &MessageId, attachment: OutgoingAttachment) {
        self.mail.attach(
            id,
            crate::Attachment {
                name: attachment.name.clone(),
                bytes: attachment.data.len() as u64,
            },
        );
        self.state
            .lock()
            .unwrap()
            .entries
            .get_mut(id.as_str())
            .expect("the message exists")
            .attachments
            .push(attachment);
    }
}

fn invalid() -> TransportError {
    TransportError(TransportErrorCode::InvalidRequest)
}
fn stale() -> TransportError {
    TransportError(TransportErrorCode::StaleId)
}

#[async_trait]
impl WorkspaceMailReader for MemoryWorkspaceMailStore {
    async fn attachment(
        &mut self,
        id: &MessageId,
        index: usize,
    ) -> Result<OutgoingAttachment, TransportError> {
        self.state
            .lock()
            .unwrap()
            .entries
            .get(id.as_str())
            .and_then(|entry| entry.attachments.get(index))
            .cloned()
            .ok_or_else(stale)
    }
    async fn folders(&mut self) -> Result<Vec<String>, TransportError> {
        Ok(self.state.lock().unwrap().folders.iter().cloned().collect())
    }
    async fn messages(
        &mut self,
        folder: &str,
        q: &str,
        page: &PageRequest,
    ) -> Result<Page<WorkspaceMailSummary>, TransportError> {
        if page.limit == 0 || page.limit > 100 {
            return Err(invalid());
        }
        let offset = match page.token.as_deref() {
            None => 0,
            Some(token) => token
                .strip_prefix("after/")
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(invalid)?,
        };
        let mut found = Vec::new();
        for summary in self.mail.summaries(false).into_iter().rev() {
            let in_folder = self
                .state
                .lock()
                .unwrap()
                .entries
                .get(summary.id.as_str())
                .is_some_and(|entry| folder.is_empty() || folder == entry.folder);
            if !in_folder {
                continue;
            }
            let message = self.message(&summary.id).await?;
            if !format!(
                "{} {} {}",
                summary.from, summary.subject, message.message.text
            )
            .to_lowercase()
            .contains(&q.to_lowercase())
            {
                continue;
            }
            found.push(WorkspaceMailSummary {
                summary: message.message.summary,
                flags: message.flags,
                labels: message.labels,
            });
        }
        let end = offset.saturating_add(page.limit);
        Ok(Page {
            next: (found.len() > end).then(|| format!("after/{end}")),
            items: found.into_iter().skip(offset).take(page.limit).collect(),
        })
    }
    async fn message(&mut self, id: &MessageId) -> Result<WorkspaceMailMessage, TransportError> {
        let mut message = Mailbox::message(&mut self.mail, id).await?;
        if let Some(context) = &self.context {
            context.assess(&mut message.summary, false).await?;
        }
        let flags = self
            .state
            .lock()
            .unwrap()
            .entries
            .get(id.as_str())
            .ok_or_else(stale)?
            .flags
            .clone();
        Ok(WorkspaceMailMessage {
            message,
            labels: flags.labels.clone(),
            flags,
        })
    }
    async fn thread(&mut self, id: &str) -> Result<Vec<WorkspaceMailMessage>, TransportError> {
        let mut messages = Vec::new();
        for summary in self.mail.summaries(false) {
            if summary.thread_id == id {
                messages.push(self.message(&summary.id).await?);
            }
        }
        messages.sort_by_key(|message| message.message.summary.date);
        Ok(messages)
    }
    async fn set_flags(&mut self, id: &MessageId, flags: FlagChange) -> Result<(), TransportError> {
        let mut state = self.state.lock().unwrap();
        let entry = state.entries.get_mut(id.as_str()).ok_or_else(stale)?;
        if let Some(seen) = flags.seen {
            entry.flags.seen = seen;
        }
        if let Some(flagged) = flags.flagged {
            entry.flags.flagged = flagged;
        }
        Ok(())
    }
    async fn close(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}

#[async_trait]
impl WorkspaceMailWriter for MemoryWorkspaceMailStore {
    async fn send(&mut self, message: &OutgoingMessage) -> Result<SentMessage, TransportError> {
        Mailbox::send(&mut self.mail, message).await
    }
    async fn create_folder(&mut self, name: &str) -> Result<(), TransportError> {
        crate::workspace_store::valid_folder(name)?;
        if !self.state.lock().unwrap().folders.insert(name.into()) {
            return Err(invalid());
        }
        Ok(())
    }
    async fn rename_folder(&mut self, name: &str, new_name: &str) -> Result<(), TransportError> {
        crate::workspace_store::valid_folder(new_name)?;
        let mut state = self.state.lock().unwrap();
        if name == "INBOX" || state.folders.contains(new_name) || !state.folders.remove(name) {
            return Err(invalid());
        }
        state.folders.insert(new_name.into());
        for entry in state
            .entries
            .values_mut()
            .filter(|entry| entry.folder == name)
        {
            entry.folder = new_name.into();
        }
        Ok(())
    }
    async fn delete_folder(&mut self, name: &str) -> Result<(), TransportError> {
        let mut state = self.state.lock().unwrap();
        if name == "INBOX" || !state.folders.remove(name) {
            return Err(invalid());
        }
        let removed: Vec<_> = state
            .entries
            .iter()
            .filter(|(_, entry)| entry.folder == name)
            .map(|(id, _)| id.clone())
            .collect();
        for id in removed {
            state.entries.remove(&id);
            self.mail.remove(&MessageId::new(id));
        }
        Ok(())
    }
    async fn move_message(&mut self, id: &MessageId, folder: &str) -> Result<(), TransportError> {
        let mut state = self.state.lock().unwrap();
        if !state.folders.contains(folder) {
            return Err(invalid());
        }
        state.entries.get_mut(id.as_str()).ok_or_else(stale)?.folder = folder.into();
        Ok(())
    }
    async fn set_label(
        &mut self,
        id: &MessageId,
        label: &str,
        apply: bool,
    ) -> Result<(), TransportError> {
        if label.is_empty() {
            return Err(invalid());
        }
        let mut state = self.state.lock().unwrap();
        let labels = &mut state
            .entries
            .get_mut(id.as_str())
            .ok_or_else(stale)?
            .flags
            .labels;
        labels.retain(|value| value != label);
        if apply {
            labels.push(label.into());
        }
        Ok(())
    }
}
