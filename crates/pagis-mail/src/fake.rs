//! A mail host for tests (ADR-0019), one fake per seam. The mailbox
//! half keeps a domain in memory; the mail half keeps a scripted inbox
//! that a test pushes a message into, which wakes a pending `idle` and
//! is fetched like any other message. Both live beside the seams, so a
//! test of the mailbox lifecycle or of the collector starts no process
//! and reaches no network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{AgentMailbox, AgentMailboxId, IngestBatch, NormalizedEvent, now_ms};
use tokio::sync::watch;

use crate::collector::{IngestError, MailIngest};
use crate::host::{
    Deletion, HostAccount, HostCapabilities, HostError, HostErrorCode, HostedMailbox, MailboxHost,
    MailboxPassword,
};
use crate::manual::ManualHost;
use crate::mime::read_mime;
use crate::standing::{StandingMailRule, StandingRuleError};
pub use crate::transport::ARCHIVE;
use crate::transport::{
    Attachment, Cursor, FlagChange, FolderState, INBOX, IdleOutcome, MailQuery, MailSession,
    MailTransport, MailboxCredential, Message, MessageId, MessageSummary, OutgoingMessage,
    SentMessage, TransportCapabilities, TransportError, TransportErrorCode,
};

/// What the fake host was asked to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCall {
    Create {
        local_part: String,
        outgoing_cap: u32,
    },
    Delete(String),
    ResetPassword(String),
    List,
}

/// A mail host with a domain in memory. Wrap it in an `Arc` and script
/// it from the test.
pub struct FakeMailboxHost {
    capabilities: HostCapabilities,
    /// Address to password, as the host holds it.
    mailboxes: Mutex<HashMap<String, String>>,
    calls: Mutex<Vec<HostCall>>,
    fail_with: Mutex<Option<HostErrorCode>>,
}

impl Default for FakeMailboxHost {
    fn default() -> Self {
        Self::with_capabilities(HostCapabilities {
            outgoing_cap: true,
            delete_mailbox: true,
            reset_password: true,
        })
    }
}

impl FakeMailboxHost {
    /// A host that declares this capability set, so a test runs against
    /// a host that has a capability and against one that does not.
    pub fn with_capabilities(capabilities: HostCapabilities) -> Self {
        Self {
            capabilities,
            mailboxes: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            fail_with: Mutex::new(None),
        }
    }

    /// Every later request fails with this code, until it is cleared.
    pub fn fail_with(&self, code: Option<HostErrorCode>) {
        *self.fail_with.lock().unwrap() = code;
    }

    /// The addresses the host holds now, sorted.
    pub fn addresses(&self) -> Vec<String> {
        let mut addresses: Vec<String> = self.mailboxes.lock().unwrap().keys().cloned().collect();
        addresses.sort();
        addresses
    }

    /// The password the host holds for one address.
    pub fn password_of(&self, address: &str) -> Option<String> {
        self.mailboxes.lock().unwrap().get(address).cloned()
    }

    pub fn calls(&self) -> Vec<HostCall> {
        self.calls.lock().unwrap().clone()
    }

    fn refuse(&self) -> Result<(), HostError> {
        match *self.fail_with.lock().unwrap() {
            Some(code) => Err(HostError(code)),
            None => Ok(()),
        }
    }

    fn record(&self, call: HostCall) {
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl MailboxHost for FakeMailboxHost {
    fn capabilities(&self) -> HostCapabilities {
        self.capabilities
    }

    async fn create(
        &self,
        account: &HostAccount,
        local_part: &str,
        password: &MailboxPassword,
        outgoing_cap: u32,
    ) -> Result<HostedMailbox, HostError> {
        self.record(HostCall::Create {
            local_part: local_part.to_string(),
            outgoing_cap,
        });
        self.refuse()?;
        let mailbox = HostedMailbox::new(local_part, account.domain());
        let mut mailboxes = self.mailboxes.lock().unwrap();
        if mailboxes.contains_key(&mailbox.address) {
            return Err(HostError(HostErrorCode::AddressTaken));
        }
        mailboxes.insert(mailbox.address.clone(), password.expose().to_string());
        Ok(mailbox)
    }

    async fn delete(&self, _account: &HostAccount, address: &str) -> Result<Deletion, HostError> {
        self.record(HostCall::Delete(address.to_string()));
        self.refuse()?;
        if !self.capabilities.delete_mailbox {
            return Ok(Deletion::UserMustDelete {
                notice: ManualHost::delete_notice(address),
            });
        }
        // A mailbox the host does not hold is deleted already.
        self.mailboxes.lock().unwrap().remove(address);
        Ok(Deletion::Removed)
    }

    async fn reset_password(
        &self,
        _account: &HostAccount,
        address: &str,
        password: &MailboxPassword,
    ) -> Result<(), HostError> {
        self.record(HostCall::ResetPassword(address.to_string()));
        self.refuse()?;
        let mut mailboxes = self.mailboxes.lock().unwrap();
        match mailboxes.get_mut(address) {
            Some(held) => {
                *held = password.expose().to_string();
                Ok(())
            }
            None => Err(HostError(HostErrorCode::MailboxUnknown)),
        }
    }

    async fn list(&self, account: &HostAccount) -> Result<Vec<HostedMailbox>, HostError> {
        self.record(HostCall::List);
        self.refuse()?;
        let mut found: Vec<HostedMailbox> = self
            .mailboxes
            .lock()
            .unwrap()
            .keys()
            .filter_map(|address| {
                let (local_part, domain) = address.rsplit_once('@')?;
                domain
                    .eq_ignore_ascii_case(account.domain())
                    .then(|| HostedMailbox::new(local_part, domain))
            })
            .collect();
        found.sort_by(|left, right| left.address.cmp(&right.address));
        Ok(found)
    }
}

/// The bytes of one message as a host stores it, for
/// [`FakeMailTransport::deliver_raw`]. The Authentication-Results
/// values lead the headers, topmost first, in the order the list names
/// them.
///
/// The `Date` is two seconds after now. A header date has whole
/// seconds, so a date of now can fall before a rule that was made in
/// the same second, and the rule would not read the message.
pub fn raw_message(from: &str, subject: &str, authentication_results: &[&str]) -> Vec<u8> {
    let mut raw = String::new();
    for result in authentication_results {
        raw.push_str(&format!("Authentication-Results: {result}\r\n"));
    }
    let date = (chrono::Utc::now() + chrono::TimeDelta::seconds(2)).to_rfc2822();
    raw.push_str(&format!(
        "Message-ID: <{}@sender.invalid>\r\n\
         Date: {date}\r\n\
         From: {from}\r\n\
         Subject: {subject}\r\n\
         \r\n\
         Hello.\r\n",
        ulid::Ulid::new().to_string().to_lowercase()
    ));
    raw.into_bytes()
}

/// The flags one message carries in the fake inbox.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoredFlags {
    pub seen: bool,
    pub flagged: bool,
}

struct StoredMessage {
    message: Message,
    flags: StoredFlags,
}

struct Folder {
    uid_validity: u32,
    next_uid: u32,
    messages: Vec<StoredMessage>,
}

impl Default for Folder {
    fn default() -> Self {
        Self {
            uid_validity: 1,
            next_uid: 1,
            messages: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Inbox {
    folders: HashMap<String, Folder>,
    sent: Vec<OutgoingMessage>,
    logins: Vec<String>,
}

impl Inbox {
    fn folder(&mut self, name: &str) -> &mut Folder {
        self.folders.entry(name.to_string()).or_default()
    }
}

/// A mailbox for tests: a scripted inbox and the messages it sent.
/// Deliver a message and the session that idles on the folder returns.
/// Every session it opens shares the one inbox.
pub struct FakeMailTransport {
    capabilities: TransportCapabilities,
    /// The state every session of this mailbox shares.
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    inbox: Mutex<Inbox>,
    /// Bumped once per change, so a session that idles wakes.
    changes: watch::Sender<u64>,
    refuse_login_with: Mutex<Option<TransportErrorCode>>,
    fail_with: Mutex<Option<TransportErrorCode>>,
}

impl Default for FakeMailTransport {
    fn default() -> Self {
        Self::with_capabilities(TransportCapabilities { idle: true })
    }
}

impl FakeMailTransport {
    /// A transport that declares this capability set.
    pub fn with_capabilities(capabilities: TransportCapabilities) -> Self {
        Self {
            capabilities,
            shared: Arc::new(Shared::default()),
        }
    }

    /// Every later login fails with this code, until it is cleared.
    pub fn refuse_login_with(&self, code: Option<TransportErrorCode>) {
        *self.shared.refuse_login_with.lock().unwrap() = code;
    }

    /// Every later session request fails with this code, until it is
    /// cleared.
    pub fn fail_with(&self, code: Option<TransportErrorCode>) {
        *self.shared.fail_with.lock().unwrap() = code;
    }

    /// Deliver one message to the inbox. A session that idles on the
    /// folder returns, as a host does when mail arrives.
    pub fn deliver(
        &self,
        from: impl Into<String>,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> MessageId {
        self.deliver_into(None, None, from, subject, body)
    }

    /// Deliver one message that belongs to a thread that already
    /// exists, as a reply does.
    pub fn deliver_reply(
        &self,
        thread_id: &str,
        from: impl Into<String>,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> MessageId {
        self.deliver_into(Some(thread_id.to_string()), None, from, subject, body)
    }

    /// Deliver one message that answers a `Message-ID`, as the answer
    /// to mail the Agent sent does.
    pub fn deliver_answer(
        &self,
        in_reply_to: &str,
        from: impl Into<String>,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> MessageId {
        self.deliver_into(
            Some(in_reply_to.to_string()),
            Some(in_reply_to.to_string()),
            from,
            subject,
            body,
        )
    }

    /// Deliver the bytes of one message, as a host stores them. The
    /// message reads through the same MIME reader as the real transport,
    /// so every header the bytes carry reaches the collector.
    pub fn deliver_raw(&self, raw: &[u8]) -> MessageId {
        let id = {
            let mut inbox = self.shared.inbox.lock().unwrap();
            let folder = inbox.folder(INBOX);
            let uid = folder.next_uid;
            folder.next_uid += 1;
            let id = MessageId::new(INBOX, uid);
            let message = read_mime(raw, id.clone()).expect("the test message reads");
            folder.messages.push(StoredMessage {
                message,
                flags: StoredFlags::default(),
            });
            id
        };
        self.shared.changes.send_modify(|version| *version += 1);
        id
    }

    fn deliver_into(
        &self,
        thread: Option<String>,
        in_reply_to: Option<String>,
        from: impl Into<String>,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> MessageId {
        let from = from.into();
        let subject = subject.into();
        let body = body.into();
        let id = {
            let mut inbox = self.shared.inbox.lock().unwrap();
            let folder = inbox.folder(INBOX);
            let uid = folder.next_uid;
            folder.next_uid += 1;
            let id = MessageId::new(INBOX, uid);
            let message_id = format!("<{uid}@fake.invalid>");
            let summary = MessageSummary {
                id: id.clone(),
                message_id: message_id.clone(),
                thread_id: thread.clone().unwrap_or_else(|| message_id.clone()),
                in_reply_to: in_reply_to.clone(),
                from: from.clone(),
                from_mailboxes: 1,
                to: Vec::new(),
                date: now_ms(),
                subject: subject.clone(),
                snippet: body.chars().take(200).collect(),
                has_attachments: false,
                authentication_results: Vec::new(),
            };
            let mut headers = vec![
                ("Message-ID".to_string(), message_id),
                ("From".to_string(), from),
                ("Subject".to_string(), subject),
            ];
            if let Some(answered) = in_reply_to {
                headers.push(("In-Reply-To".to_string(), answered));
            }
            folder.messages.push(StoredMessage {
                message: Message {
                    summary,
                    headers,
                    text: body,
                    attachments: Vec::<Attachment>::new(),
                },
                flags: StoredFlags::default(),
            });
            id
        };
        self.shared.changes.send_modify(|version| *version += 1);
        id
    }

    /// The messages this mailbox sent, in order.
    pub fn sent(&self) -> Vec<OutgoingMessage> {
        self.shared.inbox.lock().unwrap().sent.clone()
    }

    /// The addresses that logged in, in order.
    pub fn logins(&self) -> Vec<String> {
        self.shared.inbox.lock().unwrap().logins.clone()
    }

    /// The summaries one folder holds, oldest first.
    pub fn summaries(&self, folder: &str) -> Vec<MessageSummary> {
        let mut inbox = self.shared.inbox.lock().unwrap();
        inbox
            .folder(folder)
            .messages
            .iter()
            .map(|stored| stored.message.summary.clone())
            .collect()
    }

    /// The flags one message carries now.
    pub fn flags_of(&self, id: &MessageId) -> Option<StoredFlags> {
        let mut inbox = self.shared.inbox.lock().unwrap();
        inbox
            .folder(&id.folder)
            .messages
            .iter()
            .find(|stored| stored.message.summary.id == *id)
            .map(|stored| stored.flags)
    }

    /// Renumber one folder, as a host does when it rebuilds a mailbox.
    /// Every id and cursor of that folder becomes stale (ADR-0019).
    pub fn renumber(&self, folder: &str) {
        let mut inbox = self.shared.inbox.lock().unwrap();
        inbox.folder(folder).uid_validity += 1;
    }
}

#[async_trait]
impl MailTransport for FakeMailTransport {
    fn capabilities(&self) -> TransportCapabilities {
        self.capabilities
    }

    async fn login(
        &self,
        credential: &MailboxCredential,
    ) -> Result<Box<dyn MailSession>, TransportError> {
        if let Some(code) = *self.shared.refuse_login_with.lock().unwrap() {
            return Err(TransportError(code));
        }
        self.shared
            .inbox
            .lock()
            .unwrap()
            .logins
            .push(credential.address().to_string());
        Ok(Box::new(FakeMailSession {
            shared: Arc::clone(&self.shared),
            changes: self.shared.changes.subscribe(),
            selected: INBOX.to_string(),
        }))
    }
}

struct FakeMailSession {
    shared: Arc<Shared>,
    changes: watch::Receiver<u64>,
    selected: String,
}

impl FakeMailSession {
    fn refuse(&self) -> Result<(), TransportError> {
        match *self.shared.fail_with.lock().unwrap() {
            Some(code) => Err(TransportError(code)),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl MailSession for FakeMailSession {
    async fn select(&mut self, folder: &str) -> Result<FolderState, TransportError> {
        self.refuse()?;
        self.selected = folder.to_string();
        let mut inbox = self.shared.inbox.lock().unwrap();
        let state = inbox.folder(folder);
        Ok(FolderState {
            folder: folder.to_string(),
            uid_validity: state.uid_validity,
            next_uid: state.next_uid,
            exists: state.messages.len() as u32,
        })
    }

    async fn fetch_since(
        &mut self,
        cursor: &Cursor,
    ) -> Result<(Vec<MessageSummary>, Cursor), TransportError> {
        self.refuse()?;
        let mut inbox = self.shared.inbox.lock().unwrap();
        let folder = inbox.folder(&cursor.folder);
        if folder.uid_validity != cursor.uid_validity {
            return Err(TransportError(TransportErrorCode::StaleId));
        }
        let found: Vec<MessageSummary> = folder
            .messages
            .iter()
            .filter(|stored| stored.message.summary.id.uid > cursor.last_uid)
            .map(|stored| stored.message.summary.clone())
            .collect();
        let last_uid = found
            .last()
            .map(|summary| summary.id.uid)
            .unwrap_or(cursor.last_uid);
        Ok((
            found,
            Cursor {
                folder: cursor.folder.clone(),
                uid_validity: folder.uid_validity,
                last_uid,
            },
        ))
    }

    async fn fetch(&mut self, id: &MessageId) -> Result<Message, TransportError> {
        self.refuse()?;
        let mut inbox = self.shared.inbox.lock().unwrap();
        inbox
            .folder(&id.folder)
            .messages
            .iter()
            .find(|stored| stored.message.summary.id == *id)
            .map(|stored| stored.message.clone())
            // The message is not in the folder any more.
            .ok_or(TransportError(TransportErrorCode::StaleId))
    }

    async fn search(&mut self, query: &MailQuery) -> Result<Vec<MessageSummary>, TransportError> {
        self.refuse()?;
        let mut inbox = self.shared.inbox.lock().unwrap();
        let folder = inbox.folder(&self.selected);
        let matches = |summary: &MessageSummary, text: &str| -> bool {
            let contains = |field: &str, wanted: &Option<String>| match wanted {
                Some(wanted) => field.to_lowercase().contains(&wanted.to_lowercase()),
                None => true,
            };
            contains(&summary.from, &query.from)
                && contains(&summary.to.join(","), &query.to)
                && contains(&summary.subject, &query.subject)
                && contains(text, &query.text)
                && query.since.is_none_or(|since| summary.date >= since)
                && query.before.is_none_or(|before| summary.date < before)
        };
        let found: Vec<MessageSummary> = folder
            .messages
            .iter()
            .rev()
            .filter(|stored| matches(&stored.message.summary, &stored.message.text))
            .map(|stored| stored.message.summary.clone())
            .take(query.limit as usize)
            .collect();
        Ok(found)
    }

    async fn fetch_thread(&mut self, thread_id: &str) -> Result<Vec<Message>, TransportError> {
        self.refuse()?;
        let mut inbox = self.shared.inbox.lock().unwrap();
        let folder = inbox.folder(&self.selected);
        let mut found: Vec<Message> = folder
            .messages
            .iter()
            .filter(|stored| stored.message.summary.thread_id == thread_id)
            .map(|stored| stored.message.clone())
            .collect();
        found.sort_by_key(|message| message.summary.date);
        Ok(found)
    }

    async fn idle(&mut self, deadline: Duration) -> Result<IdleOutcome, TransportError> {
        self.refuse()?;
        if self.changes.has_changed().unwrap_or(false) {
            self.changes.borrow_and_update();
            return Ok(IdleOutcome::Changed);
        }
        tokio::select! {
            changed = self.changes.changed() => {
                if changed.is_err() {
                    return Err(TransportError(TransportErrorCode::Unreachable));
                }
                Ok(IdleOutcome::Changed)
            }
            _ = tokio::time::sleep(deadline) => Ok(IdleOutcome::Deadline),
        }
    }

    async fn send(&mut self, message: &OutgoingMessage) -> Result<SentMessage, TransportError> {
        self.refuse()?;
        let mut inbox = self.shared.inbox.lock().unwrap();
        inbox.sent.push(message.clone());
        Ok(SentMessage {
            message_id: format!("<sent-{}@fake.invalid>", inbox.sent.len()),
        })
    }

    async fn set_flags(&mut self, id: &MessageId, flags: FlagChange) -> Result<(), TransportError> {
        self.refuse()?;
        let mut inbox = self.shared.inbox.lock().unwrap();
        let stored = inbox
            .folder(&id.folder)
            .messages
            .iter_mut()
            .find(|stored| stored.message.summary.id == *id)
            .ok_or(TransportError(TransportErrorCode::StaleId))?;
        if let Some(seen) = flags.seen {
            stored.flags.seen = seen;
        }
        if let Some(flagged) = flags.flagged {
            stored.flags.flagged = flagged;
        }
        Ok(())
    }

    async fn move_to_archive(&mut self, id: &MessageId) -> Result<(), TransportError> {
        self.refuse()?;
        {
            let mut inbox = self.shared.inbox.lock().unwrap();
            let folder = inbox.folder(&id.folder);
            let at = folder
                .messages
                .iter()
                .position(|stored| stored.message.summary.id == *id)
                .ok_or(TransportError(TransportErrorCode::StaleId))?;
            let mut stored = folder.messages.remove(at);
            let archive = inbox.folder(ARCHIVE);
            let uid = archive.next_uid;
            archive.next_uid += 1;
            stored.message.summary.id = MessageId::new(ARCHIVE, uid);
            archive.messages.push(stored);
        }
        self.shared.changes.send_modify(|version| *version += 1);
        Ok(())
    }
}

/// The Standing Mail Rule seam, in memory. It records the mailboxes
/// provisioning asked a rule for, and refuses on demand, so a test of
/// the desk needs no Trigger module.
#[derive(Default)]
pub struct FakeStandingRule {
    created: Mutex<Vec<AgentMailboxId>>,
    refuse_with: Mutex<Option<String>>,
}

impl FakeStandingRule {
    /// Every later request fails with this reason, until it is cleared.
    pub fn refuse_with(&self, reason: Option<&str>) {
        *self.refuse_with.lock().unwrap() = reason.map(str::to_string);
    }

    /// The mailboxes a rule was created for, in order.
    pub fn created(&self) -> Vec<AgentMailboxId> {
        self.created.lock().unwrap().clone()
    }
}

#[async_trait]
impl StandingMailRule for FakeStandingRule {
    async fn create(&self, mailbox: &AgentMailbox) -> Result<(), StandingRuleError> {
        if let Some(reason) = self.refuse_with.lock().unwrap().clone() {
            return Err(StandingRuleError(reason));
        }
        self.created.lock().unwrap().push(mailbox.id.clone());
        Ok(())
    }
}

/// The `ingest` seam, in memory. It keeps every pass the collector
/// handed over, so a test reads the Incoming Events a message made
/// without a Trigger module or a database.
#[derive(Default)]
pub struct FakeMailIngest {
    passes: Mutex<Vec<IngestBatch>>,
    refuse_with: Mutex<Option<String>>,
    /// Bumped once per pass, so a test waits instead of polling.
    changes: watch::Sender<u64>,
}

impl FakeMailIngest {
    /// Every later pass fails with this reason, until it is cleared.
    pub fn refuse_with(&self, reason: Option<&str>) {
        *self.refuse_with.lock().unwrap() = reason.map(str::to_string);
    }

    /// The passes the collector handed over, in order.
    pub fn passes(&self) -> Vec<IngestBatch> {
        self.passes.lock().unwrap().clone()
    }

    /// Every event of every pass, in order.
    pub fn events(&self) -> Vec<NormalizedEvent> {
        self.passes
            .lock()
            .unwrap()
            .iter()
            .flat_map(|batch| batch.events.clone())
            .collect()
    }

    /// Wait until the collector has handed over at least `count`
    /// events, or give up. It answers the events it saw.
    pub async fn wait_for(&self, count: usize, within: Duration) -> Vec<NormalizedEvent> {
        let mut changes = self.changes.subscribe();
        let _ = tokio::time::timeout(within, async {
            loop {
                if self.events().len() >= count {
                    return;
                }
                if changes.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
        self.events()
    }
}

#[async_trait]
impl MailIngest for FakeMailIngest {
    async fn ingest(&self, batch: IngestBatch) -> Result<(), IngestError> {
        if let Some(reason) = self.refuse_with.lock().unwrap().clone() {
            return Err(IngestError(reason));
        }
        self.passes.lock().unwrap().push(batch);
        self.changes.send_modify(|version| *version += 1);
        Ok(())
    }
}
