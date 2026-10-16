//! The inbound mail collector (ADR-0019).
//!
//! An IDLE task watches one `active` Agent Mailbox. It selects INBOX,
//! reads everything after the mailbox cursor, hands the pass to the
//! Trigger module, and then waits with IMAP IDLE where the server
//! announces it. Other mailboxes use a five-minute poll cycle. Every
//! reconnect starts with the same full read from the cursor, so a
//! session the host dropped loses no message.
//!
//! The collector stops at `ingest` (ADR-0006). It creates no Run, it
//! reads no subscription filter, and it never opens a message body:
//! the event carries the envelope, and the Agent reads the words with
//! a live `mail__get_message`.
//!
//! A refused login is the mailbox's own end: the record moves to
//! `unavailable` and the task stops until the user resets the password
//! (ADR-0019). Every other failure is a reconnect with the shared
//! backoff, one second doubling to five minutes.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{
    AgentMailbox, AgentMailboxId, AgentMailboxState, IngestBatch, MailboxCursor, NormalizedEvent,
    SentMail, SentMailStore, WakeupLanding, WorkspaceId, now_ms,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::desk::{MailboxDesk, MailboxError};
use crate::events::{MAIL_MESSAGE_RECEIVED, OWN_MAILBOX, message_event};
use crate::proof::Backoff;
use crate::sender::SenderEvidence;
use crate::transport::{INBOX, MailSession, MessageSummary, TransportError, TransportErrorCode};

/// How often the host polls mailboxes whose servers have no IDLE
/// (ADR-0019).
pub const POLL_INTERVAL: Duration = Duration::from_secs(300);

/// How often the collector reads the mailbox list, so a mailbox that
/// became `active` gets a task and one that left `active` loses it.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Where one collected pass goes. It is a seam, so the mail crate
/// stops at `ingest` and never reaches into the Trigger module
/// (ADR-0006).
#[async_trait]
pub trait MailIngest: Send + Sync {
    async fn ingest(&self, batch: IngestBatch) -> Result<(), IngestError>;
}

/// Why the Trigger module did not take one pass. The collector keeps
/// the cursor where it was and reads the same messages again, which is
/// safe because the pass deduplicates on the `Message-ID`.
#[derive(Debug, thiserror::Error)]
#[error("the trigger module refused a mail pass: {0}")]
pub struct IngestError(pub String);

/// Why one pass ended early.
#[derive(Debug, thiserror::Error)]
enum PassError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("the mail assessment failed: {0}")]
    Assessment(TransportError),
    #[error(transparent)]
    Ingest(#[from] IngestError),
    #[error(transparent)]
    Mailbox(#[from] MailboxError),
}

/// Everything the collector reads and writes.
pub struct MailCollectorDeps {
    pub desk: Arc<MailboxDesk>,
    /// The sent-message table, which lands a reply in the Thread that
    /// sent the message it answers (ADR-0019).
    pub sent: Arc<dyn SentMailStore>,
    pub ingest: Arc<dyn MailIngest>,
    /// One IDLE turn. The daemon owns the deadline (ADR-0019).
    pub idle_turn: Duration,
}

/// IDLE tasks and poll cycles for active Agent Mailboxes (ADR-0019).
pub struct MailCollector {
    desk: Arc<MailboxDesk>,
    sent: Arc<dyn SentMailStore>,
    ingest: Arc<dyn MailIngest>,
    idle_turn: Duration,
    state: Mutex<CollectorState>,
}

#[derive(Default)]
struct CollectorState {
    tasks: HashMap<AgentMailboxId, Watcher>,
    no_idle: HashSet<AgentMailboxId>,
    polling: HashSet<AgentMailboxId>,
}

/// Release ownership of a poll, including when its caller cancels it.
struct PollLease<'a> {
    state: &'a Mutex<CollectorState>,
    id: AgentMailboxId,
}

impl Drop for PollLease<'_> {
    fn drop(&mut self) {
        self.state
            .lock()
            .expect("collector state lock")
            .polling
            .remove(&self.id);
    }
}

/// One running mailbox task.
struct Watcher {
    workspace_id: WorkspaceId,
    cancel: CancellationToken,
    finished: oneshot::Receiver<()>,
}

impl Watcher {
    fn is_finished(&mut self) -> bool {
        !matches!(
            self.finished.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        )
    }
}

impl MailCollector {
    pub fn new(deps: MailCollectorDeps) -> Self {
        Self {
            desk: deps.desk,
            sent: deps.sent,
            ingest: deps.ingest,
            idle_turn: deps.idle_turn,
            state: Mutex::new(CollectorState::default()),
        }
    }

    /// Match this Workspace's tasks to its active Agent Mailboxes.
    pub async fn sweep_workspace(
        self: &Arc<Self>,
        workspace_id: &WorkspaceId,
    ) -> Result<(), MailboxError> {
        let active: Vec<AgentMailbox> = self
            .desk
            .list(workspace_id)
            .await?
            .into_iter()
            .filter(|mailbox| mailbox.state == AgentMailboxState::Active)
            .collect();
        let mut state = self.state.lock().expect("collector state lock");
        let CollectorState {
            tasks,
            no_idle,
            polling,
        } = &mut *state;
        let start: Vec<AgentMailbox> = {
            // A mailbox that left `active`, and one whose task ended by
            // itself, both lose their entry. Cancelling is what stops
            // the task; it never stops in the middle of a pass.
            tasks.retain(|id, watcher| {
                if &watcher.workspace_id != workspace_id {
                    return true;
                }
                let live = active.iter().any(|mailbox| mailbox.id == *id);
                if !live || watcher.is_finished() {
                    watcher.cancel.cancel();
                    return false;
                }
                true
            });
            active
                .into_iter()
                .filter(|mailbox| {
                    !tasks.contains_key(&mailbox.id)
                        && !no_idle.contains(&mailbox.id)
                        && !polling.contains(&mailbox.id)
                })
                .collect()
        };
        for mailbox in start {
            let id = mailbox.id.clone();
            let cancel = CancellationToken::new();
            let collector = Arc::clone(self);
            let (finish, finished) = oneshot::channel();
            pagis_rt::spawn({
                let cancel = cancel.clone();
                async move {
                    collector.watch(mailbox, cancel).await;
                    let _ = finish.send(());
                }
            });
            tasks.insert(
                id,
                Watcher {
                    workspace_id: workspace_id.clone(),
                    cancel,
                    finished,
                },
            );
        }
        Ok(())
    }

    /// Read active Agent Mailboxes without an IDLE task, then log out.
    /// A failed mailbox does not stop the other mailboxes of this Workspace.
    pub async fn poll_workspace(&self, workspace_id: &WorkspaceId) -> Result<(), MailboxError> {
        for mailbox in self.desk.list(workspace_id).await? {
            if mailbox.state != AgentMailboxState::Active {
                continue;
            }
            let lease = {
                let mut state = self.state.lock().expect("collector state lock");
                if state
                    .tasks
                    .get_mut(&mailbox.id)
                    .is_some_and(|watcher| !watcher.is_finished())
                    || !state.polling.insert(mailbox.id.clone())
                {
                    continue;
                }
                PollLease {
                    state: &self.state,
                    id: mailbox.id.clone(),
                }
            };
            let result = self.poll_mailbox(&mailbox).await;
            if let Err(error) = result {
                if let PassError::Mailbox(error) = &error {
                    if let Some(code) = refusal(error) {
                        self.unavailable(&mailbox, code).await;
                    }
                } else if let PassError::Transport(TransportError(code)) = &error
                    && is_final(*code)
                {
                    self.unavailable(&mailbox, *code).await;
                }
                tracing::warn!(mailbox_id = %mailbox.id, %error, "mail poll failed");
            }
            drop(lease);
        }
        Ok(())
    }

    async fn poll_mailbox(&self, mailbox: &AgentMailbox) -> Result<(), PassError> {
        let mut session = self.desk.open_session(mailbox).await?;
        let mut cursor = mailbox.cursor.clone();
        let result = self.drain(session.as_mut(), mailbox, &mut cursor).await;
        let logout = session.logout().await;
        result?;
        logout?;
        Ok(())
    }

    /// Stop every task. The daemon calls it at shutdown.
    pub fn stop(&self) {
        for (_, watcher) in self
            .state
            .lock()
            .expect("collector state lock")
            .tasks
            .drain()
        {
            watcher.cancel.cancel();
        }
    }

    /// How many mailboxes are watched now.
    pub fn watched(&self) -> usize {
        self.state.lock().expect("collector state lock").tasks.len()
    }

    /// One mailbox, until it stops being this Agent's mailbox.
    async fn watch(self: Arc<Self>, mailbox: AgentMailbox, cancel: CancellationToken) {
        let mut backoff = Backoff::reconnect();
        // One task owns the cursor of its mailbox, so it carries it
        // across a reconnect rather than reading the record again.
        let mut cursor = mailbox.cursor.clone();
        while !cancel.is_cancelled() {
            let session = match self.desk.open_session(&mailbox).await {
                Ok(session) => session,
                Err(error) => {
                    if let Some(code) = refusal(&error) {
                        self.unavailable(&mailbox, code).await;
                        return;
                    }
                    if !self.wait(&cancel, backoff.wait()).await {
                        return;
                    }
                    continue;
                }
            };
            backoff.reset();
            if !session.idle_supported() {
                self.state
                    .lock()
                    .expect("collector state lock")
                    .no_idle
                    .insert(mailbox.id.clone());
                let mut session = session;
                if let Err(error) = session.logout().await {
                    tracing::warn!(mailbox_id = %mailbox.id, %error, "mail logout failed");
                }
                return;
            }
            if let Err(error) = self
                .watch_session(session, &mailbox, &mut cursor, &cancel)
                .await
            {
                if let PassError::Transport(TransportError(code)) = &error
                    && is_final(*code)
                {
                    self.unavailable(&mailbox, *code).await;
                    return;
                }
                tracing::warn!(
                    address = mailbox.address,
                    %error,
                    "the mail session ended; connecting again"
                );
                if !self.wait(&cancel, backoff.wait()).await {
                    return;
                }
            }
        }
    }

    /// One open session: read from the cursor, then wait for the next
    /// change. Every turn starts with the same full read, so a
    /// reconnect is a resync and nothing is missed.
    async fn watch_session(
        &self,
        mut session: Box<dyn MailSession>,
        mailbox: &AgentMailbox,
        cursor: &mut Option<MailboxCursor>,
        cancel: &CancellationToken,
    ) -> Result<(), PassError> {
        let result: Result<(), PassError> = async {
            session.select(INBOX).await?;
            loop {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                self.drain(session.as_mut(), mailbox, cursor).await?;
                let waited = cancel
                    .run_until_cancelled(session.idle(self.idle_turn))
                    .await
                    .transpose()?
                    .is_some();
                if !waited {
                    return Ok(());
                }
            }
        }
        .await;
        let logout = session.logout().await;
        result?;
        logout?;
        Ok(())
    }

    /// Read every message after the cursor and hand each page to the
    /// Trigger module. `fetch_since` answers one page at a time, so the
    /// loop ends when a page is empty.
    async fn drain(
        &self,
        session: &mut dyn MailSession,
        mailbox: &AgentMailbox,
        cursor: &mut Option<MailboxCursor>,
    ) -> Result<(), PassError> {
        if cursor.is_none() {
            // A mailbox with no cursor starts at the folder head, so
            // mail from before the collector wakes nobody.
            let state = session.select(INBOX).await?;
            *cursor = Some(state.cursor_here());
            self.desk
                .set_cursor(&mailbox.workspace_id, &mailbox.id, &state.cursor_here())
                .await?;
            return Ok(());
        }
        loop {
            let at = cursor.as_ref().expect("the cursor is set above");
            let (summaries, next) = match session.fetch_since(at).await {
                Ok(page) => page,
                // The host renumbered the folder, so every UID before
                // it is gone. Start again at the head: mail whose
                // numbering was thrown away wakes nobody (ADR-0019).
                Err(TransportError(TransportErrorCode::StaleId)) => {
                    let state = session.select(INBOX).await?;
                    *cursor = Some(state.cursor_here());
                    self.desk
                        .set_cursor(&mailbox.workspace_id, &mailbox.id, &state.cursor_here())
                        .await?;
                    tracing::warn!(
                        address = mailbox.address,
                        "the mail folder was renumbered; the cursor starts again at its head"
                    );
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            };
            if summaries.is_empty() {
                return Ok(());
            }
            self.pass(session, mailbox, &summaries).await?;
            // The cursor moves only once the pass is stored, so a
            // failure reads the same messages again.
            self.desk
                .set_cursor(&mailbox.workspace_id, &mailbox.id, &next)
                .await?;
            *cursor = Some(next);
        }
    }

    /// One page of messages, as one collection pass.
    async fn pass(
        &self,
        session: &mut dyn MailSession,
        mailbox: &AgentMailbox,
        summaries: &[MessageSummary],
    ) -> Result<(), PassError> {
        // The host behind a mailbox never changes, so one read serves
        // the whole page.
        let authserv_ids = self.desk.authserv_ids(mailbox).await?;
        let mut events = Vec::with_capacity(summaries.len());
        for summary in summaries {
            if let Some(event) = self.event(session, mailbox, authserv_ids, summary).await? {
                events.push(event);
            }
        }
        self.ingest
            .ingest(IngestBatch {
                workspace_id: mailbox.workspace_id.clone(),
                source: pagis_core::EventSource::connection(mailbox.connection_id.clone()),
                // The mailbox is this Agent's own identity, so no other
                // Agent on the Connection sees the pass (ADR-0019).
                agent_id: Some(mailbox.agent_id.clone()),
                event_kind: MAIL_MESSAGE_RECEIVED.to_string(),
                // The Agent Mailbox record carries the cursor, because
                // one Connection holds the mailboxes of several Agents
                // (ADR-0019).
                cursor: None,
                events,
                received_at: now_ms(),
                baseline: false,
            })
            .await?;
        Ok(())
    }

    /// One message as an Incoming Event: the envelope, the sender's
    /// verified tier with its evidence, and where a Wake-up it makes
    /// must land.
    async fn event(
        &self,
        session: &mut dyn MailSession,
        mailbox: &AgentMailbox,
        authserv_ids: &[&str],
        summary: &MessageSummary,
    ) -> Result<Option<NormalizedEvent>, PassError> {
        let mut summary = summary.clone();
        let context = self.desk.read_context(mailbox).await?;
        context
            .assess(&mut summary, false)
            .await
            .map_err(PassError::Assessment)?;
        if summary.require_visible().is_err() {
            return Ok(None);
        }
        if summary.is_junk() {
            let folder = match session.junk_folders().await?.into_iter().next() {
                Some(folder) => folder,
                None => {
                    session.create_folder("Junk").await?;
                    "Junk".into()
                }
            };
            let moved = session.move_message(&summary.id, &folder).await?;
            context
                .relocate(&summary, &moved)
                .await
                .map_err(PassError::Assessment)?;
            return Ok(None);
        }
        let sender = SenderEvidence::of_message(&summary, authserv_ids);
        let assessment = summary
            .assessment
            .as_ref()
            .expect("intake stores the assessment");
        let tier = assessment.trust_tier;
        let mut event = message_event(
            OWN_MAILBOX,
            &mailbox.address,
            &summary,
            sender.verification,
            tier,
        );
        event.metadata["spam_verdict"] =
            serde_json::to_value(&assessment.verdict).expect("a Spam Verdict serializes");
        event.landing = self.landing(mailbox, &summary).await?;
        Ok(Some(event))
    }

    /// Where a Wake-up for this message lands. A reply to mail the
    /// Agent sent from a Thread lands in that Thread; everything else
    /// lands where the rule says, which is the Agent's Thread with the
    /// user (ADR-0019).
    async fn landing(
        &self,
        mailbox: &AgentMailbox,
        summary: &MessageSummary,
    ) -> Result<Option<WakeupLanding>, PassError> {
        let Some(answered) = summary.in_reply_to.as_deref() else {
            return Ok(None);
        };
        let sent = self
            .sent
            .get(&mailbox.workspace_id, answered)
            .await
            .map_err(MailboxError::from)?;
        Ok(sent.and_then(|sent| thread_of(&sent, mailbox)))
    }

    /// The host refuses this mailbox, so it is the user's to fix
    /// (ADR-0019).
    async fn unavailable(&self, mailbox: &AgentMailbox, code: TransportErrorCode) {
        let reason = format!("the mail host refused the mailbox ({})", code.as_str());
        if let Err(error) = self
            .desk
            .mark_unavailable(&mailbox.workspace_id, &mailbox.id, &reason)
            .await
        {
            tracing::error!(%error, "marking a mailbox unavailable failed");
        }
    }

    /// Wait, unless the task is stopped first. `false` says the task
    /// is stopped.
    async fn wait(&self, cancel: &CancellationToken, wait: Duration) -> bool {
        cancel
            .run_until_cancelled(pagis_rt::sleep(wait))
            .await
            .is_some()
    }
}

/// The Thread one sent message belongs to, when this mailbox sent it
/// from a Thread. A send with no Channel was not a conversation, so
/// its answer lands where the rule says.
fn thread_of(sent: &SentMail, mailbox: &AgentMailbox) -> Option<WakeupLanding> {
    (sent.mailbox_id == mailbox.id)
        .then(|| sent.channel_id.clone())
        .flatten()
        .map(|channel_id| WakeupLanding {
            channel_id,
            root_message_id: sent.thread_id.clone(),
        })
}

/// The refusal a login carries, when the host's answer is its last
/// word on this mailbox.
fn refusal(error: &MailboxError) -> Option<TransportErrorCode> {
    match error {
        MailboxError::Transport(TransportError(code)) if is_final(*code) => Some(*code),
        _ => None,
    }
}

/// Whether the host's answer ends the mailbox rather than the session.
/// A refused password and a mailbox the host does not hold both ask the
/// user for a password reset; everything else is worth another
/// connection.
fn is_final(code: TransportErrorCode) -> bool {
    matches!(
        code,
        TransportErrorCode::Unauthorized | TransportErrorCode::MailboxGone
    )
}
