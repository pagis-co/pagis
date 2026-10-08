//! The inbound mail collector (ADR-0019).
//!
//! One task watches one `active` Agent Mailbox. It selects INBOX,
//! reads everything after the mailbox cursor, hands the pass to the
//! Trigger module, and then waits: with IMAP IDLE where the transport
//! declares it, and with a five-minute poll where it does not. Every
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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{
    AgentMailbox, AgentMailboxId, AgentMailboxState, IngestBatch, MailboxCursor, NormalizedEvent,
    SentMail, SentMailStore, WakeupLanding, WorkspaceStore, now_ms,
};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::desk::{MailboxDesk, MailboxError};
use crate::events::{MAIL_MESSAGE_RECEIVED, OWN_MAILBOX, SenderTrust, message_event};
use crate::proof::Backoff;
use crate::sender::SenderEvidence;
use crate::transport::{INBOX, MailSession, MessageSummary, TransportError, TransportErrorCode};

/// How often a transport without `idle` reads the folder again
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
    #[error(transparent)]
    Ingest(#[from] IngestError),
    #[error(transparent)]
    Mailbox(#[from] MailboxError),
}

/// Everything the collector reads and writes.
pub struct MailCollectorDeps {
    /// Every Workspace on this installation. One sweep covers them all:
    /// the tenant comes off each mailbox row.
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub desk: Arc<MailboxDesk>,
    /// The sent-message table, which lands a reply in the Thread that
    /// sent the message it answers (ADR-0019).
    pub sent: Arc<dyn SentMailStore>,
    pub trust: Arc<dyn SenderTrust>,
    pub ingest: Arc<dyn MailIngest>,
    /// One IDLE turn. The daemon owns the deadline (ADR-0019).
    pub idle_turn: Duration,
    /// How long a transport without `idle` waits between two reads.
    pub poll_interval: Duration,
}

/// One task per `active` own mailbox (ADR-0019).
pub struct MailCollector {
    workspaces: Arc<dyn WorkspaceStore>,
    desk: Arc<MailboxDesk>,
    sent: Arc<dyn SentMailStore>,
    trust: Arc<dyn SenderTrust>,
    ingest: Arc<dyn MailIngest>,
    idle_turn: Duration,
    poll_interval: Duration,
    tasks: Mutex<HashMap<AgentMailboxId, Watcher>>,
}

/// One running mailbox task.
struct Watcher {
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl MailCollector {
    pub fn new(deps: MailCollectorDeps) -> Self {
        Self {
            workspaces: deps.workspaces,
            desk: deps.desk,
            sent: deps.sent,
            trust: deps.trust,
            ingest: deps.ingest,
            idle_turn: deps.idle_turn,
            poll_interval: deps.poll_interval,
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Watch the mailboxes, and keep watching them. The sweep is what
    /// starts a task for a mailbox that became `active` and stops the
    /// task of one that became `dormant`, `unavailable` or deleted.
    /// The daemon's token ends the loop, and every mailbox task with
    /// it.
    pub fn spawn(self: &Arc<Self>, sweep: Duration, cancel: CancellationToken) -> JoinHandle<()> {
        let collector = Arc::clone(self);
        tokio::spawn(async move {
            while !cancel.is_cancelled() {
                collector.sweep().await;
                if cancel
                    .run_until_cancelled(tokio::time::sleep(sweep))
                    .await
                    .is_none()
                {
                    break;
                }
            }
            collector.stop();
        })
    }

    /// Match the running tasks to the `active` mailboxes, once.
    pub async fn sweep(self: &Arc<Self>) {
        let workspaces = match self.workspaces.list().await {
            Ok(workspaces) => workspaces,
            Err(error) => {
                tracing::error!(%error, "listing the workspaces failed");
                return;
            }
        };
        let mut active: Vec<AgentMailbox> = Vec::new();
        for workspace in workspaces {
            match self.desk.list(&workspace.id).await {
                Ok(mailboxes) => active.extend(
                    mailboxes
                        .into_iter()
                        .filter(|mailbox| mailbox.state == AgentMailboxState::Active),
                ),
                Err(error) => {
                    tracing::error!(%error, workspace = %workspace.id, "listing the agent mailboxes failed");
                }
            }
        }
        let start: Vec<AgentMailbox> = {
            let mut tasks = self.tasks.lock().expect("collector task lock");
            // A mailbox that left `active`, and one whose task ended by
            // itself, both lose their entry. Cancelling is what stops
            // the task; it never stops in the middle of a pass.
            tasks.retain(|id, watcher| {
                let live = active.iter().any(|mailbox| mailbox.id == *id);
                if !live || watcher.handle.is_finished() {
                    watcher.cancel.cancel();
                    return false;
                }
                true
            });
            active
                .into_iter()
                .filter(|mailbox| !tasks.contains_key(&mailbox.id))
                .collect()
        };
        for mailbox in start {
            let id = mailbox.id.clone();
            let cancel = CancellationToken::new();
            let collector = Arc::clone(self);
            let handle = tokio::spawn({
                let cancel = cancel.clone();
                async move { collector.watch(mailbox, cancel).await }
            });
            self.tasks
                .lock()
                .expect("collector task lock")
                .insert(id, Watcher { cancel, handle });
        }
    }

    /// Stop every task. The daemon calls it at shutdown.
    pub fn stop(&self) {
        for (_, watcher) in self.tasks.lock().expect("collector task lock").drain() {
            watcher.cancel.cancel();
        }
    }

    /// How many mailboxes are watched now.
    pub fn watched(&self) -> usize {
        self.tasks.lock().expect("collector task lock").len()
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
        session.select(INBOX).await?;
        let idle = self.desk.transport_capabilities().idle;
        loop {
            // A stopped task reads nothing more. The check comes first
            // because a wait that ends at the moment the task stops
            // answers with the change, not with the stop.
            if cancel.is_cancelled() {
                return Ok(());
            }
            self.drain(session.as_mut(), mailbox, cursor).await?;
            let waited = match idle {
                true => cancel
                    .run_until_cancelled(session.idle(self.idle_turn))
                    .await
                    .transpose()?
                    .is_some(),
                false => self.wait(cancel, self.poll_interval).await,
            };
            if !waited {
                return Ok(());
            }
        }
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
            self.pass(mailbox, &summaries).await?;
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
        mailbox: &AgentMailbox,
        summaries: &[MessageSummary],
    ) -> Result<(), PassError> {
        // The host behind a mailbox never changes, so one read serves
        // the whole page.
        let authserv_ids = self.desk.authserv_ids(mailbox).await?;
        let mut events = Vec::with_capacity(summaries.len());
        for summary in summaries {
            events.push(self.event(mailbox, authserv_ids, summary).await?);
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
        mailbox: &AgentMailbox,
        authserv_ids: &[&str],
        summary: &MessageSummary,
    ) -> Result<NormalizedEvent, PassError> {
        let sender = SenderEvidence::of_message(summary, authserv_ids);
        let tier = self
            .trust
            .tier(&mailbox.workspace_id, &mailbox.agent_id, &sender)
            .await
            .map_err(MailboxError::from)?;
        let mut event = message_event(
            OWN_MAILBOX,
            &mailbox.address,
            summary,
            sender.verification,
            tier,
        );
        event.landing = self.landing(mailbox, summary).await?;
        Ok(event)
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
            .run_until_cancelled(tokio::time::sleep(wait))
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
