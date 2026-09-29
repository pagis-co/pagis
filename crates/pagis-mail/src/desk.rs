//! The mailbox desk (ADR-0019): the one path that provisions, proves,
//! recovers, sleeps and deletes an Agent Mailbox.
//!
//! Creation waits for the host create alone, so a host refusal lands in
//! the creation form. The first login is proven afterwards, in the
//! background, because a host propagates a new mailbox slowly: the
//! record stays `provisioning` until a login succeeds, and becomes
//! `unavailable` with the reason when none does inside the budget.
//!
//! The desk writes the record before it asks the host. The row is the
//! reservation in the Address Ledger, so two forms cannot take one
//! address, and a host that refuses the create drops the reservation
//! again. Every other end of a mailbox is a tombstone, and the ledger
//! keeps it, so an address is never given out a second time.

use std::sync::Arc;
use std::time::Duration;

use pagis_core::{
    AgentId, AgentMailbox, AgentMailboxId, AgentMailboxState, AgentMailboxStore, AgentStatus,
    AgentStore, Connection, ConnectionId, ConnectionStore, EventBus, MailboxCursor, NewEvent,
    SecretStore, StoreError, UnixMillis, WorkspaceId, now_ms,
};
use rand::Rng;

use crate::address::{
    LocalPartError, mailbox_address, suggest_local_part, validate_local_part, with_suffix,
};
use crate::host::{Deletion, HostAccount, HostError, HostErrorCode, MailboxHost, MailboxPassword};
use crate::manual::ManualHost;
use crate::provider::{MailboxProvider, authserv_ids, mailbox_provider};
use crate::standing::{StandingMailRule, StandingRuleError};
use crate::transport::{
    INBOX, MailSession, MailTransport, MailboxCredential, TransportCapabilities, TransportError,
};
use crate::{
    DEFAULT_OUTGOING_CAP, MANUAL_PROVIDER, host_api_key_secret_name, mailbox_password_secret_name,
};

/// The characters a generated mailbox password is made of. Every mail
/// host accepts them, and none of them needs escaping in a URL or a
/// configuration file.
const PASSWORD_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// How long a generated mailbox password is. The daemon is the only
/// reader, so it is long rather than memorable.
const PASSWORD_LENGTH: usize = 32;

/// How many suffixes the desk tries before it gives up on a name. The
/// user then types another one.
const MAX_SUFFIX: u32 = 99;

/// How the login proof waits (ADR-0019). A host propagates a
/// new mailbox slowly, so the proof retries for about three minutes
/// before it calls the mailbox unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProofTiming {
    /// How long the proof waits before the second attempt.
    pub first_wait: Duration,
    /// The longest wait between two attempts.
    pub max_wait: Duration,
    /// How long the proof keeps trying in total.
    pub budget: Duration,
}

impl Default for ProofTiming {
    fn default() -> Self {
        Self {
            first_wait: Duration::from_secs(5),
            max_wait: Duration::from_secs(30),
            budget: Duration::from_secs(180),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MailboxError {
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The form may fix this. The message is the one the user reads.
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Conflict(String),
    #[error("agent mailbox not found")]
    NotFound,
    /// The Workspace has no Mailbox Provider Connection at all.
    #[error("this workspace has no mailbox provider")]
    NoProvider,
    /// The Mailbox Provider Connection is not `connected`.
    #[error("the mailbox provider is {0}")]
    ProviderUnavailable(String),
    #[error(transparent)]
    Host(#[from] HostError),
    /// The mail host refused the connection, or did not answer it.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// The Standing Mail Rule the mailbox needs was not created, so
    /// the mailbox would wake nobody (ADR-0019).
    #[error("the standing mail rule was not created: {0}")]
    StandingRule(#[from] StandingRuleError),
}

/// What one new mailbox is made of. The domain is the Connection's, so
/// the form names the local part alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMailbox {
    pub connection_id: ConnectionId,
    /// The name before the `@`, as the user accepted it.
    pub local_part: String,
    /// The most messages a day; `None` takes the default.
    pub outgoing_cap: Option<u32>,
    /// The password of an inbox the user made at a manual host. A host
    /// with an API mints its own, and refuses a pasted one.
    pub password: Option<String>,
}

/// One Mailbox Provider Connection the creation form can offer, with
/// the free name this Agent would get on it (ADR-0019).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxOffer {
    pub connection: Connection,
    pub settings: MailboxProvider,
    /// The local part the form fills in: the Agent's name, with the
    /// ledger collision suffix when the plain name is taken.
    pub suggested_local_part: String,
}

/// What a delete did. `notice` carries the words a manual host asks the
/// desk to show, because that mailbox is still at the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxDeletion {
    pub mailbox: AgentMailbox,
    pub notice: Option<String>,
}

/// One request the desk has read and accepted. It exists so Agent
/// creation runs every check before it writes the Agent, and the
/// provision that follows reads the same parts.
struct CheckedMailbox {
    connection: Connection,
    settings: MailboxProvider,
    local_part: String,
    address: String,
    outgoing_cap: u32,
    password: MailboxPassword,
}

/// Everything the desk needs. Every collaborator is a seam, so a test
/// drives the whole lifecycle with no network.
pub struct MailboxDeskDeps {
    pub mailboxes: Arc<dyn AgentMailboxStore>,
    pub connections: Arc<dyn ConnectionStore>,
    /// The Org's Workspace, which holds the mail domain Connections.
    /// One mail domain serves every person, and each mailbox lives in
    /// the Workspace of the person whose Agent holds it (ADR-0019).
    pub org_workspace_id: WorkspaceId,
    pub agents: Arc<dyn AgentStore>,
    pub host: Arc<dyn MailboxHost>,
    pub transport: Arc<dyn MailTransport>,
    pub secrets: Arc<dyn SecretStore>,
    pub bus: Arc<dyn EventBus>,
    pub standing_rule: Arc<dyn StandingMailRule>,
    pub proof: ProofTiming,
}

pub struct MailboxDesk {
    mailboxes: Arc<dyn AgentMailboxStore>,
    connections: Arc<dyn ConnectionStore>,
    org_workspace_id: WorkspaceId,
    agents: Arc<dyn AgentStore>,
    host: Arc<dyn MailboxHost>,
    /// The manual host is the daemon's own: it has no API to reach, so
    /// it is never injected.
    manual_host: ManualHost,
    transport: Arc<dyn MailTransport>,
    secrets: Arc<dyn SecretStore>,
    bus: Arc<dyn EventBus>,
    standing_rule: Arc<dyn StandingMailRule>,
    proof: ProofTiming,
}

impl MailboxDesk {
    pub fn new(deps: MailboxDeskDeps) -> Self {
        Self {
            mailboxes: deps.mailboxes,
            connections: deps.connections,
            org_workspace_id: deps.org_workspace_id,
            agents: deps.agents,
            host: deps.host,
            manual_host: ManualHost,
            transport: deps.transport,
            secrets: deps.secrets,
            bus: deps.bus,
            standing_rule: deps.standing_rule,
            proof: deps.proof,
        }
    }

    /// The mailbox one Agent holds now, when it holds one.
    pub async fn for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<AgentMailbox>, MailboxError> {
        Ok(self.mailboxes.for_agent(workspace_id, agent_id).await?)
    }

    /// The mailboxes of the Workspace that are not deleted. The Agent
    /// roster reads it once and joins each Agent to its address.
    pub async fn list(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<AgentMailbox>, MailboxError> {
        Ok(self.mailboxes.list(workspace_id).await?)
    }

    /// The Org's Mailbox Provider Connections a mailbox can be made on
    /// now, each with the free name this Agent would take (ADR-0019).
    /// The creation form and the mailbox card read this.
    pub async fn offers(&self, agent_name: &str) -> Result<Vec<MailboxOffer>, MailboxError> {
        let mut offers = Vec::new();
        for connection in self.connections.list(&self.org_workspace_id).await? {
            if connection.status != Connection::CONNECTED {
                continue;
            }
            let Some(settings) = mailbox_provider(&connection) else {
                continue;
            };
            let suggested_local_part = self.free_local_part(agent_name, &settings.domain).await?;
            offers.push(MailboxOffer {
                connection,
                settings,
                suggested_local_part,
            });
        }
        Ok(offers)
    }

    /// Whether the Address Ledger already holds an address. The
    /// creation form asks this as the user types.
    pub async fn address_taken(&self, address: &str) -> Result<bool, MailboxError> {
        Ok(self
            .mailboxes
            .address_taken(&address.to_lowercase())
            .await?)
    }

    /// Everything a new mailbox needs that does not need the Agent:
    /// the provider, the name, the cap, the password and the Address
    /// Ledger. Agent creation runs it before it writes the Agent, so
    /// a taken address or a reserved name answers the form and leaves
    /// no Agent behind (ADR-0019).
    pub async fn check(&self, request: &NewMailbox) -> Result<(), MailboxError> {
        self.checked(request).await.map(|_| ())
    }

    /// Make one Agent Mailbox. The reservation is written first, the
    /// host is asked once, and the login proof runs afterwards, so a
    /// host refusal lands in the form and slow propagation does not
    /// (ADR-0019).
    pub async fn provision(
        self: &Arc<Self>,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        request: NewMailbox,
    ) -> Result<AgentMailbox, MailboxError> {
        self.active_agent(workspace_id, agent_id).await?;
        if self
            .mailboxes
            .for_agent(workspace_id, agent_id)
            .await?
            .is_some()
        {
            return Err(MailboxError::Conflict(
                "that Agent already holds a mailbox. Delete it first.".to_string(),
            ));
        }
        let CheckedMailbox {
            connection,
            settings,
            local_part,
            address,
            outgoing_cap,
            password,
        } = self.checked(&request).await?;

        let mailbox = AgentMailbox {
            id: AgentMailboxId::generate(),
            workspace_id: workspace_id.clone(),
            agent_id: agent_id.clone(),
            connection_id: connection.id.clone(),
            address: address.clone(),
            state: AgentMailboxState::Provisioning,
            reason: None,
            outgoing_cap,
            allow_rules: Vec::new(),
            cursor: None,
            sends_day: None,
            sends_today: 0,
            created_at: now_ms(),
            deleted_at: None,
        };
        // The row is the reservation: the Address Ledger, not a later
        // check, decides who gets the address.
        self.mailboxes.create(&mailbox).await.map_err(store_error)?;

        let host = self.host_of(&connection);
        let account = self.host_account(&connection, &settings)?;
        if let Err(error) = host
            .create(&account, &local_part, &password, outgoing_cap)
            .await
        {
            // The host made no mailbox, so the address stays free.
            self.mailboxes
                .drop_reservation(workspace_id, &mailbox.id)
                .await?;
            return Err(MailboxError::Host(error));
        }
        if self
            .secrets
            .set(
                &mailbox_password_secret_name(&mailbox.workspace_id, &address),
                password.expose(),
            )
            .is_err()
        {
            // A mailbox whose password Pagis cannot keep is no
            // mailbox: undo the host create and free the address.
            let _ = host.delete(&account, &address).await;
            self.mailboxes
                .drop_reservation(workspace_id, &mailbox.id)
                .await?;
            return Err(MailboxError::Validation(
                "the daemon could not store the mailbox password".to_string(),
            ));
        }
        // A mailbox that wakes nobody is no mailbox either, so the
        // Standing Mail Rule is part of the provision (ADR-0019).
        if let Err(error) = self.standing_rule.create(&mailbox).await {
            let _ = host.delete(&account, &address).await;
            let _ = self.secrets.delete(&mailbox_password_secret_name(
                &mailbox.workspace_id,
                &address,
            ));
            self.mailboxes
                .drop_reservation(workspace_id, &mailbox.id)
                .await?;
            return Err(MailboxError::StandingRule(error));
        }
        self.audit("agent_mailbox.provisioned", &mailbox).await?;
        self.spawn_proof(&mailbox);
        Ok(mailbox)
    }

    /// Prove the first login, and keep proving it until the budget runs
    /// out (ADR-0019). A proven login makes the mailbox `active` and,
    /// the first time, starts the cursor where the folder stands, so
    /// mail that arrived before the mailbox was the Agent's wakes
    /// nobody. A budget that runs out makes it `unavailable` with the
    /// reason. Recovery runs the same proof.
    pub async fn prove_login(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<AgentMailbox, MailboxError> {
        let mailbox = self.live_mailbox(workspace_id, id).await?;
        let (_, settings) = self.provider_of(&mailbox.connection_id).await?;
        let credential = self.credential_of(&mailbox, &settings)?;

        let deadline = std::time::Instant::now() + self.proof.budget;
        let mut wait = self.proof.first_wait;
        // The cursor starts where the folder stands the first time, so
        // mail that arrived before the mailbox was the Agent's wakes
        // nobody. Recovery keeps the cursor it already has.
        let start_cursor = mailbox.cursor.is_none();
        let last = loop {
            let attempt = match self.transport.login(&credential).await {
                Err(error) => Err(error),
                Ok(mut session) => match start_cursor {
                    true => session.select(INBOX).await.map(Some),
                    false => Ok(None),
                },
            };
            match attempt {
                Ok(folder) => {
                    if let Some(folder) = folder {
                        self.mailboxes
                            .set_cursor(workspace_id, id, &folder.cursor_here())
                            .await?;
                    }
                    self.mailboxes
                        .set_state(workspace_id, id, AgentMailboxState::Active, None)
                        .await?;
                    let active = self.read(workspace_id, id).await?;
                    self.audit("agent_mailbox.active", &active).await?;
                    return Ok(active);
                }
                // Every failure is retried, even a refusal: a host that
                // has not finished making the mailbox refuses the login
                // it accepts a minute later.
                Err(error) => {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() {
                        break error;
                    }
                    tokio::time::sleep(wait.min(left)).await;
                    wait = (wait * 2).min(self.proof.max_wait);
                }
            }
        };

        let reason = format!(
            "the mail host did not accept the login ({})",
            last.0.as_str()
        );
        self.mailboxes
            .set_state(
                workspace_id,
                id,
                AgentMailboxState::Unavailable,
                Some(&reason),
            )
            .await?;
        let unavailable = self.read(workspace_id, id).await?;
        self.audit("agent_mailbox.unavailable", &unavailable)
            .await?;
        Ok(unavailable)
    }

    /// Give the mailbox a new password and prove the login again
    /// (ADR-0019). The daemon mints it where the host declares
    /// `reset_password`, and the user pastes it otherwise. The cursor
    /// does not move.
    pub async fn reset_password(
        self: &Arc<Self>,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        pasted: Option<String>,
    ) -> Result<AgentMailbox, MailboxError> {
        let mailbox = self.live_mailbox(workspace_id, id).await?;
        let (connection, settings) = self.provider_of(&mailbox.connection_id).await?;
        let account = self.host_account(&connection, &settings)?;
        let password = match settings.capabilities.reset_password {
            true => {
                if pasted.is_some() {
                    return Err(MailboxError::Validation(
                        "this mail host mints the mailbox password itself".to_string(),
                    ));
                }
                let password = MailboxPassword::new(generate_password());
                self.host_of(&connection)
                    .reset_password(&account, &mailbox.address, &password)
                    .await?;
                password
            }
            false => match pasted.filter(|password| !password.trim().is_empty()) {
                Some(password) => MailboxPassword::new(password),
                None => {
                    return Err(MailboxError::Validation(
                        "this mail host has no API, so the new mailbox password is required"
                            .to_string(),
                    ));
                }
            },
        };
        self.secrets
            .set(
                &mailbox_password_secret_name(&mailbox.workspace_id, &mailbox.address),
                password.expose(),
            )
            .map_err(|error| {
                tracing::error!(%error, "storing the mailbox password failed");
                MailboxError::Validation(
                    "the daemon could not store the mailbox password".to_string(),
                )
            })?;
        self.mailboxes
            .set_state(workspace_id, id, AgentMailboxState::Provisioning, None)
            .await?;
        let provisioning = self.read(workspace_id, id).await?;
        self.audit("agent_mailbox.password_reset", &provisioning)
            .await?;
        self.spawn_proof(&provisioning);
        Ok(provisioning)
    }

    /// The Agent is archived, so its mailbox sleeps with it (ADR-0019).
    /// The mail stays at the host and the mailbox stays with the Agent:
    /// unlike a phone number it never returns to the Workspace.
    pub async fn make_dormant_for_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<Option<AgentMailbox>, MailboxError> {
        let Some(dormant) = self
            .mailboxes
            .make_dormant_for_agent(workspace_id, agent_id)
            .await?
        else {
            return Ok(None);
        };
        self.audit("agent_mailbox.dormant", &dormant).await?;
        Ok(Some(dormant))
    }

    /// Delete the mailbox, its host mail and its password (ADR-0019).
    /// It is the user's act alone, and `confirm_address` is the address
    /// the user typed to confirm it. The tombstone stays in the Address
    /// Ledger, so the address is never reused.
    pub async fn delete(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        confirm_address: &str,
    ) -> Result<MailboxDeletion, MailboxError> {
        let mailbox = self.live_mailbox(workspace_id, id).await?;
        if !confirm_address
            .trim()
            .eq_ignore_ascii_case(&mailbox.address)
        {
            return Err(MailboxError::Validation(format!(
                "type {} to confirm the delete",
                mailbox.address
            )));
        }
        // The Connection cannot be gone: its removal is refused while a
        // mailbox points at it.
        let (connection, settings) = self.provider_of(&mailbox.connection_id).await?;
        let account = self.host_account(&connection, &settings)?;
        let notice = match self
            .host_of(&connection)
            .delete(&account, &mailbox.address)
            .await?
        {
            Deletion::Removed => None,
            Deletion::UserMustDelete { notice } => Some(notice),
        };
        if let Err(error) = self.secrets.delete(&mailbox_password_secret_name(
            &mailbox.workspace_id,
            &mailbox.address,
        )) {
            tracing::error!(%error, "destroying the mailbox password failed");
        }
        let at = now_ms();
        if !self.mailboxes.delete(workspace_id, id, at).await? {
            return Err(MailboxError::NotFound);
        }
        let tombstone = AgentMailbox {
            state: AgentMailboxState::Deleted,
            reason: None,
            deleted_at: Some(at),
            ..mailbox
        };
        self.audit("agent_mailbox.deleted", &tombstone).await?;
        Ok(MailboxDeletion {
            mailbox: tombstone,
            notice,
        })
    }

    /// What one mailbox logs in with: its own password and the two
    /// endpoints of its Connection. The password leaves the secret
    /// store only for the transport (ADR-0013).
    fn credential_of(
        &self,
        mailbox: &AgentMailbox,
        settings: &MailboxProvider,
    ) -> Result<MailboxCredential, MailboxError> {
        let password = self
            .secrets
            .get(&mailbox_password_secret_name(
                &mailbox.workspace_id,
                &mailbox.address,
            ))
            .ok()
            .flatten()
            .ok_or_else(|| {
                MailboxError::Conflict(
                    "the mailbox password is gone. Reset it to use this mailbox.".to_string(),
                )
            })?;
        Ok(MailboxCredential::new(
            &mailbox.address,
            password,
            settings.imap.clone(),
            settings.smtp.clone(),
        ))
    }

    /// What the mail transport of every Connection declares. The
    /// collector reads it to know whether to wait with IDLE or to poll
    /// (ADR-0019).
    pub fn transport_capabilities(&self) -> TransportCapabilities {
        self.transport.capabilities()
    }

    /// Remember where the collector reached (ADR-0019). The cursor is
    /// the mailbox's own, so a restart reads on from it.
    pub async fn set_cursor(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        cursor: &MailboxCursor,
    ) -> Result<(), MailboxError> {
        self.mailboxes.set_cursor(workspace_id, id, cursor).await?;
        Ok(())
    }

    /// The mail host refused this mailbox for good: it refused the
    /// login or reports the mailbox gone (ADR-0019). The collector
    /// stops and the desk asks the user for a password reset.
    pub async fn mark_unavailable(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        reason: &str,
    ) -> Result<AgentMailbox, MailboxError> {
        self.mailboxes
            .set_state(
                workspace_id,
                id,
                AgentMailboxState::Unavailable,
                Some(reason),
            )
            .await?;
        let unavailable = self.read(workspace_id, id).await?;
        self.audit("agent_mailbox.unavailable", &unavailable)
            .await?;
        Ok(unavailable)
    }

    /// Open one connection to a mailbox, for the mail tools.
    /// The desk owns the credential, so nothing above it reads a
    /// password.
    pub async fn open_session(
        &self,
        mailbox: &AgentMailbox,
    ) -> Result<Box<dyn MailSession>, MailboxError> {
        let (_, settings) = self.provider_of(&mailbox.connection_id).await?;
        let credential = self.credential_of(mailbox, &settings)?;
        Ok(self.transport.login(&credential).await?)
    }

    /// The authserv-ids of the Mailbox Provider that holds one mailbox.
    /// The collector verifies a sender against them (ADR-0019).
    pub async fn authserv_ids(
        &self,
        mailbox: &AgentMailbox,
    ) -> Result<&'static [&'static str], MailboxError> {
        let (connection, _) = self.provider_of(&mailbox.connection_id).await?;
        Ok(authserv_ids(&connection.provider))
    }

    /// Add Mail Recipient Domain allow rules to one mailbox, from an
    /// approved send card (ADR-0019). It answers the rules the mailbox
    /// holds after the write.
    pub async fn allow_recipient_domains(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        domains: &[String],
    ) -> Result<Vec<String>, MailboxError> {
        Ok(self
            .mailboxes
            .add_allow_rules(workspace_id, id, domains)
            .await?)
    }

    /// Count one send against today's tally and answer the new count.
    /// The mail tools call it before they send, so the Outgoing Cap
    /// holds whatever the host allows.
    pub async fn note_send(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
        at: UnixMillis,
    ) -> Result<u32, MailboxError> {
        Ok(self.mailboxes.note_send(workspace_id, id, at).await?)
    }

    /// How many mailboxes of this Workspace that are not deleted point
    /// at one Connection. Removing a Mailbox Provider Connection
    /// refuses while a mailbox of any Workspace does.
    pub async fn connection_in_use(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
    ) -> Result<u32, MailboxError> {
        Ok(self
            .mailboxes
            .count_live_for_connection(workspace_id, connection_id)
            .await?)
    }

    /// Read one request, refuse it where it is wrong, and answer with
    /// the parts a provision needs. It reserves nothing: the row does
    /// that, and only the row keeps two forms apart.
    async fn checked(&self, request: &NewMailbox) -> Result<CheckedMailbox, MailboxError> {
        let (connection, settings) = self.connected_provider(&request.connection_id).await?;
        let local_part = validate_local_part(&request.local_part).map_err(validation)?;
        let address = mailbox_address(&local_part, &settings.domain);
        if self.mailboxes.address_taken(&address).await? {
            return Err(MailboxError::Conflict(format!(
                "{address} is in the address ledger. Choose another name."
            )));
        }
        let outgoing_cap = self.outgoing_cap(request.outgoing_cap)?;
        let manual = connection.provider == MANUAL_PROVIDER;
        let password = match (manual, request.password.as_deref()) {
            // The user made the inbox at the host, so only the user
            // knows its password.
            (true, Some(password)) if !password.trim().is_empty() => MailboxPassword::new(password),
            (true, _) => {
                return Err(MailboxError::Validation(
                    "this mail host has no API, so the mailbox password is required".to_string(),
                ));
            }
            (false, Some(_)) => {
                return Err(MailboxError::Validation(
                    "this mail host mints the mailbox password itself".to_string(),
                ));
            }
            (false, None) => MailboxPassword::new(generate_password()),
        };
        Ok(CheckedMailbox {
            connection,
            settings,
            local_part,
            address,
            outgoing_cap,
            password,
        })
    }

    /// The name the form fills in: the Agent's name as a local part,
    /// with `2`, `3` and so on while the Address Ledger holds it.
    async fn free_local_part(
        &self,
        agent_name: &str,
        domain: &str,
    ) -> Result<String, MailboxError> {
        let base = suggest_local_part(agent_name);
        if base.is_empty() {
            return Ok(String::new());
        }
        for suffix in 1..=MAX_SUFFIX {
            let local_part = match suffix {
                1 => base.clone(),
                suffix => with_suffix(&base, suffix),
            };
            let address = mailbox_address(&local_part, domain);
            if !self.mailboxes.address_taken(&address).await? {
                return Ok(local_part);
            }
        }
        Ok(String::new())
    }

    /// The proof runs on its own, so the creation form answers as soon
    /// as the host has made the mailbox.
    fn spawn_proof(self: &Arc<Self>, mailbox: &AgentMailbox) {
        let desk = Arc::clone(self);
        let workspace_id = mailbox.workspace_id.clone();
        let id = mailbox.id.clone();
        tokio::spawn(async move {
            if let Err(error) = desk.prove_login(&workspace_id, &id).await {
                tracing::error!(%error, "proving the mailbox login failed");
            }
        });
    }

    async fn read(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<AgentMailbox, MailboxError> {
        self.mailboxes
            .get(workspace_id, id)
            .await?
            .ok_or(MailboxError::NotFound)
    }

    /// A mailbox of this Workspace that is not a tombstone.
    async fn live_mailbox(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentMailboxId,
    ) -> Result<AgentMailbox, MailboxError> {
        let mailbox = self.read(workspace_id, id).await?;
        match mailbox.state.is_live() {
            true => Ok(mailbox),
            false => Err(MailboxError::NotFound),
        }
    }

    async fn active_agent(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<(), MailboxError> {
        let agent = self
            .agents
            .get(workspace_id, agent_id)
            .await?
            .ok_or_else(|| MailboxError::Validation("that Agent is not here".to_string()))?;
        if agent.status != AgentStatus::Active {
            return Err(MailboxError::Validation(
                "an archived Agent gets no mailbox".to_string(),
            ));
        }
        Ok(())
    }

    /// The Org's Mailbox Provider Connection and its mail settings.
    async fn provider_of(
        &self,
        connection_id: &ConnectionId,
    ) -> Result<(Connection, MailboxProvider), MailboxError> {
        let connection = self
            .connections
            .get(&self.org_workspace_id, connection_id)
            .await?
            .ok_or(MailboxError::NoProvider)?;
        let settings = mailbox_provider(&connection).ok_or_else(|| {
            MailboxError::Validation("that connection is not a mailbox provider".to_string())
        })?;
        Ok((connection, settings))
    }

    /// The same, and it must be `connected`: no mailbox is made through
    /// a provider whose key the host refuses.
    async fn connected_provider(
        &self,
        connection_id: &ConnectionId,
    ) -> Result<(Connection, MailboxProvider), MailboxError> {
        let (connection, settings) = self.provider_of(connection_id).await?;
        if connection.status != Connection::CONNECTED {
            return Err(MailboxError::ProviderUnavailable(connection.status.clone()));
        }
        Ok((connection, settings))
    }

    /// The mailbox seam of one Connection. A manual host has no API
    /// and does its work in the daemon, so it is not the injected one
    /// (ADR-0019).
    fn host_of(&self, connection: &Connection) -> &dyn MailboxHost {
        match connection.provider == MANUAL_PROVIDER {
            true => &self.manual_host,
            false => self.host.as_ref(),
        }
    }

    /// The host account of one Connection. A manual host has no API
    /// key at all; every other host reads one from the secret store.
    fn host_account(
        &self,
        connection: &Connection,
        settings: &MailboxProvider,
    ) -> Result<HostAccount, MailboxError> {
        if connection.provider == MANUAL_PROVIDER {
            return Ok(HostAccount::manual(&settings.domain));
        }
        match self
            .secrets
            .get(&host_api_key_secret_name(&connection.alias))
        {
            Ok(Some(api_key)) => Ok(HostAccount::new(
                &settings.domain,
                settings.account.clone().unwrap_or_default(),
                api_key,
            )),
            Ok(None) => Err(MailboxError::ProviderUnavailable(
                Connection::UNAVAILABLE.to_string(),
            )),
            Err(error) => {
                tracing::error!(%error, "reading the mail host API key failed");
                Err(MailboxError::ProviderUnavailable(
                    Connection::UNAVAILABLE.to_string(),
                ))
            }
        }
    }

    fn outgoing_cap(&self, asked: Option<u32>) -> Result<u32, MailboxError> {
        match asked {
            None => Ok(DEFAULT_OUTGOING_CAP),
            Some(0) => Err(MailboxError::Validation(
                "an outgoing cap of zero sends nothing. Leave it out for the default.".to_string(),
            )),
            Some(cap) => Ok(cap),
        }
    }

    /// One audit line for each act (ADR-0019). It carries the mailbox,
    /// the Agent and the state, and never the password.
    async fn audit(&self, event_type: &str, mailbox: &AgentMailbox) -> Result<(), MailboxError> {
        self.bus
            .publish(NewEvent {
                workspace_id: mailbox.workspace_id.clone(),
                event_type: event_type.to_string(),
                agent_id: Some(mailbox.agent_id.clone()),
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({
                    "agent_mailbox_id": mailbox.id.as_str(),
                    "address": mailbox.address,
                    "agent_id": mailbox.agent_id.as_str(),
                    "state": mailbox.state.as_str(),
                    "reason": mailbox.reason,
                }),
            })
            .await?;
        Ok(())
    }
}

/// A password only the daemon reads. It is generated here and stored
/// once; nothing shows it, and no tool returns it (ADR-0013).
fn generate_password() -> String {
    let mut rng = rand::rng();
    (0..PASSWORD_LENGTH)
        .map(|_| PASSWORD_ALPHABET[rng.random_range(0..PASSWORD_ALPHABET.len())] as char)
        .collect()
}

fn validation(error: LocalPartError) -> MailboxError {
    MailboxError::Validation(error.to_string())
}

/// A uniqueness the schema refused is a form error, not a failure.
fn store_error(error: StoreError) -> MailboxError {
    match error {
        StoreError::Conflict(message) => MailboxError::Conflict(message),
        other => MailboxError::Store(other),
    }
}

/// A host refusal the user can act on keeps its words; the rest report
/// the stable code and nothing from upstream.
pub fn host_message(error: &HostError) -> String {
    match error.0 {
        HostErrorCode::AddressTaken => {
            "the mail host already holds that address. Choose another name.".to_string()
        }
        HostErrorCode::Unauthorized => {
            "the mail host refused its API key. Reconnect the mailbox provider.".to_string()
        }
        HostErrorCode::CapReached => {
            "the mail account is at the mailbox limit of its plan".to_string()
        }
        HostErrorCode::MailboxUnknown => "the mail host does not hold that mailbox".to_string(),
        code => format!("the mail host did not complete this ({})", code.as_str()),
    }
}
