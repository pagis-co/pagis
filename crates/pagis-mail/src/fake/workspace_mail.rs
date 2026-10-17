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
    stores: Mutex<HashMap<WorkspaceId, (String, MemoryWorkspaceMailStore)>>,
}

impl MemoryWorkspaceMailHost {
    pub fn addresses(&self) -> Vec<String> {
        let mut addresses: Vec<_> = self
            .stores
            .lock()
            .unwrap()
            .values()
            .map(|(address, _)| address.clone())
            .collect();
        addresses.sort();
        addresses
    }

    pub fn store(&self, workspace: &WorkspaceId) -> Option<MemoryWorkspaceMailStore> {
        self.stores
            .lock()
            .unwrap()
            .get(workspace)
            .map(|(_, store)| store.clone())
    }
}

#[async_trait]
impl WorkspaceMailHost for MemoryWorkspaceMailHost {
    fn supports_automatic_provisioning(&self, _connection: &Connection) -> bool {
        true
    }

    async fn check(
        &self,
        connection: &Connection,
        _request: &NewMailbox,
    ) -> Result<Vec<crate::HostedMailbox>, MailboxError> {
        let settings = crate::mailbox_provider(connection).ok_or(MailboxError::NoProvider)?;
        Ok(self
            .stores
            .lock()
            .unwrap()
            .values()
            .filter_map(|(address, _)| {
                let (local, domain) = address.rsplit_once('@')?;
                domain
                    .eq_ignore_ascii_case(&settings.domain)
                    .then(|| crate::HostedMailbox::new(local, domain))
            })
            .collect())
    }

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
        self.stores.lock().unwrap().insert(
            mailbox.workspace_id.clone(),
            (mailbox.address.clone(), store.clone()),
        );
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
    numbering: HashMap<String, (u32, u32)>,
    fetched_messages: Vec<MessageId>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            folders: BTreeSet::from(["INBOX".into()]),
            entries: HashMap::new(),
            numbering: HashMap::new(),
            fetched_messages: Vec::new(),
        }
    }
}
struct Entry {
    uid: u32,
    folder: String,
    flags: MailMetadata,
    attachments: Vec<OutgoingAttachment>,
}

impl State {
    fn assign_uid(&mut self, folder: &str) -> u32 {
        let (_, next_uid) = self.numbering.entry(folder.into()).or_insert((1, 1));
        let uid = *next_uid;
        *next_uid += 1;
        uid
    }
}

impl MemoryWorkspaceMailStore {
    pub fn fetched_messages(&self) -> Vec<MessageId> {
        self.state.lock().unwrap().fetched_messages.clone()
    }

    pub fn renumber(&self, folder: &str) {
        self.state
            .lock()
            .unwrap()
            .numbering
            .entry(folder.into())
            .or_insert((1, 1))
            .0 += 1;
    }

    fn with_context(mut self, context: MailReadContext) -> Self {
        self.context = Some(context);
        self
    }

    pub fn deliver(&self, from: &str, subject: &str, body: &str) -> MessageId {
        let id = self.mail.deliver(from, subject, body);
        let mut state = self.state.lock().unwrap();
        let uid = state.assign_uid("INBOX");
        state.entries.insert(
            id.to_string(),
            Entry {
                uid,
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
        let mut state = self.state.lock().unwrap();
        let uid = state.assign_uid("INBOX");
        state.entries.insert(
            id.to_string(),
            Entry {
                uid,
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

fn trash_folder(state: &mut State) -> String {
    if let Some(name) = state
        .folders
        .iter()
        .find(|name| name.eq_ignore_ascii_case("Trash"))
    {
        return name.clone();
    }
    state.folders.insert("Trash".into());
    "Trash".into()
}

fn invalid() -> TransportError {
    TransportError(TransportErrorCode::InvalidRequest)
}
fn stale() -> TransportError {
    TransportError(TransportErrorCode::StaleId)
}

#[async_trait]
impl WorkspaceMailReader for MemoryWorkspaceMailStore {
    async fn folder_state(&mut self, folder: &str) -> Result<crate::FolderState, TransportError> {
        let mut state = self.state.lock().unwrap();
        if !state.folders.contains(folder) {
            return Err(invalid());
        }
        let (uid_validity, next_uid) = *state.numbering.entry(folder.into()).or_insert((1, 1));
        Ok(crate::FolderState {
            folder: folder.into(),
            uid_validity,
            next_uid,
            exists: state
                .entries
                .values()
                .filter(|e| e.folder == folder)
                .count() as u32,
        })
    }
    async fn messages_since(
        &mut self,
        cursor: &crate::Cursor,
    ) -> Result<(Vec<MessageId>, crate::Cursor), TransportError> {
        let head = self.folder_state(&cursor.folder).await?;
        if head.uid_validity != cursor.uid_validity {
            return Err(stale());
        }
        let state = self.state.lock().unwrap();
        let mut found = state
            .entries
            .iter()
            .filter(|(_, entry)| entry.folder == cursor.folder && entry.uid > cursor.last_uid)
            .map(|(id, entry)| (entry.uid, MessageId::new(id.clone())))
            .collect::<Vec<_>>();
        found.sort_by_key(|(uid, _)| *uid);
        found.truncate(crate::transport::FETCH_BATCH);
        let next = crate::Cursor {
            last_uid: found.last().map_or(cursor.last_uid, |(uid, _)| *uid),
            ..cursor.clone()
        };
        Ok((found.into_iter().map(|(_, id)| id).collect(), next))
    }
    async fn quarantined_message(
        &mut self,
        id: &MessageId,
    ) -> Result<WorkspaceMailMessage, TransportError> {
        self.state.lock().unwrap().fetched_messages.push(id.clone());
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
            labels: flags.labels.clone(),
            flags,
            message,
        })
    }
    async fn junk_folder(&mut self) -> Result<String, TransportError> {
        self.state.lock().unwrap().folders.insert("Junk".into());
        Ok("Junk".into())
    }
    async fn attachment(
        &mut self,
        id: &MessageId,
        index: usize,
    ) -> Result<OutgoingAttachment, TransportError> {
        self.message(id).await?;
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
        self.junk_folder().await?;
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
            let message = match self.message(&summary.id).await {
                Ok(message) => message,
                Err(TransportError(TransportErrorCode::StaleId)) => continue,
                Err(error) => return Err(error),
            };
            if message.message.summary.is_junk()
                && let Some(context) = &self.context
                && context.moves_junk().await?
            {
                let junk = self.junk_folder().await?;
                let in_junk = self
                    .state
                    .lock()
                    .unwrap()
                    .entries
                    .get(summary.id.as_str())
                    .is_some_and(|entry| entry.folder == junk);
                if !in_junk {
                    self.move_message(&summary.id, &junk).await?;
                }
            }
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
        self.state.lock().unwrap().fetched_messages.push(id.clone());
        let mut message = Mailbox::message(&mut self.mail, id).await?;
        if let Some(context) = &self.context {
            let in_junk = self
                .state
                .lock()
                .unwrap()
                .entries
                .get(id.as_str())
                .is_some_and(|e| e.folder == "Junk");
            context.assess(&mut message.summary, in_junk).await?;
            message.summary.require_visible()?;
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
                match self.message(&summary.id).await {
                    Ok(message) => messages.push(message),
                    Err(TransportError(TransportErrorCode::StaleId)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        messages.sort_by_key(|message| message.message.summary.date);
        Ok(messages)
    }
    async fn set_flags(&mut self, id: &MessageId, flags: FlagChange) -> Result<(), TransportError> {
        self.message(id).await?;
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
    async fn mark_spam(&mut self, id: &MessageId, spam: bool) -> Result<(), TransportError> {
        let context = self.context.clone().ok_or_else(invalid)?;
        context.mark_spam(self, id, spam).await
    }
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
        if crate::workspace_store::is_system_folder(name)
            || crate::workspace_store::is_system_folder(new_name)
            || state.folders.contains(new_name)
            || !state.folders.remove(name)
        {
            return Err(invalid());
        }
        state.folders.insert(new_name.into());
        if let Some(numbering) = state.numbering.remove(name) {
            state.numbering.insert(new_name.into(), numbering);
        }
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
        let ids: Vec<_> = {
            let state = self.state.lock().unwrap();
            if crate::workspace_store::is_system_folder(name) || !state.folders.contains(name) {
                return Err(invalid());
            }
            state
                .entries
                .iter()
                .filter(|(_, entry)| entry.folder == name)
                .map(|(id, _)| MessageId::new(id))
                .collect()
        };
        for id in &ids {
            self.message(id).await?;
        }
        let trash = trash_folder(&mut self.state.lock().unwrap());
        for id in ids {
            self.move_message(&id, &trash).await?;
        }
        let mut state = self.state.lock().unwrap();
        state.numbering.remove(name);
        state.folders.remove(name);
        Ok(())
    }
    async fn delete_message(&mut self, id: &MessageId) -> Result<MessageId, TransportError> {
        self.message(id).await?;
        let (folder, trash) = {
            let mut state = self.state.lock().unwrap();
            let folder = state
                .entries
                .get(id.as_str())
                .ok_or_else(stale)?
                .folder
                .clone();
            (folder, trash_folder(&mut state))
        };
        if folder == trash {
            self.state.lock().unwrap().entries.remove(id.as_str());
            self.mail.remove(id);
            Ok(id.clone())
        } else {
            self.move_message(id, &trash).await
        }
    }
    async fn move_message(
        &mut self,
        id: &MessageId,
        folder: &str,
    ) -> Result<MessageId, TransportError> {
        let message = self.message(id).await?;
        {
            let mut state = self.state.lock().unwrap();
            if !state.folders.contains(folder) {
                return Err(invalid());
            }
            let uid = state.assign_uid(folder);
            let entry = state.entries.get_mut(id.as_str()).ok_or_else(stale)?;
            entry.folder = folder.into();
            entry.uid = uid;
        }
        if let Some(context) = &self.context {
            context.relocate(&message.message.summary, id).await?;
        }
        Ok(id.clone())
    }
    async fn set_label(
        &mut self,
        id: &MessageId,
        label: &str,
        apply: bool,
    ) -> Result<(), TransportError> {
        self.message(id).await?;
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
