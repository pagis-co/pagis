//! The `mail.message_received` Incoming Event (ADR-0019): what
//! one message carries into the Trigger module, and the trusted matcher
//! that reads a rule's typed filter against it.
//!
//! The metadata is the envelope of a message and never its words: no
//! body, no snippet and no attachment. The Agent reaches those with a
//! live `mail__get_message`, which wraps them in the untrusted envelope.
//!
//! The verified tier of the sender stamps the event before the Agent
//! reads a word. It gates what the words are worth and never whether
//! the Agent wakes: only a `min_trust` filter stops a wake. The event
//! records the evidence beside it: whether the sender is verified, and
//! why.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{
    AgentId, Connection, ConnectionStore, EventMatcher, NormalizedEvent, StoreError,
    TrustListStore, TrustSubject, TrustTier, WorkspaceId,
};

use crate::provider::ACCOUNT_KEY;
use crate::sender::{SenderEvidence, SenderVerification};
use crate::transport::MessageSummary;

/// The `mailbox` value of the Agent's own mailbox. A rule that names
/// it watches the Agent's own identity and nothing else.
pub use pagis_broker::OWN_MAILBOX;
pub use pagis_broker::{MAIL_MATCHER, MAIL_MESSAGE_RECEIVED};

/// What one inbound message tells the Trigger module (ADR-0019).
/// `mailbox` is `own` or the alias of one of the user's accounts.
///
/// `message_id` is the id `mail__get_message` takes, and not the
/// `Message-ID` header: the Agent reads the words with a live tool
/// call, and the metadata carries the envelope alone.
pub fn message_event(
    mailbox: &str,
    address: &str,
    summary: &MessageSummary,
    verification: SenderVerification,
    trust_tier: TrustTier,
) -> NormalizedEvent {
    let mut metadata = serde_json::Map::new();
    metadata.insert("mailbox".into(), mailbox.into());
    metadata.insert("message_id".into(), summary.id.to_string().into());
    metadata.insert("thread_id".into(), summary.thread_id.clone().into());
    if let Some(answered) = &summary.in_reply_to {
        metadata.insert("in_reply_to".into(), answered.clone().into());
    }
    metadata.insert("from".into(), summary.from.clone().into());
    if let Some(domain) = sender_domain(&summary.from) {
        metadata.insert("from_domain".into(), domain.into());
    }
    metadata.insert("to".into(), summary.to.clone().into());
    metadata.insert("subject".into(), summary.subject.clone().into());
    metadata.insert("has_attachments".into(), summary.has_attachments.into());
    metadata.insert("received_at".into(), summary.date.into());
    metadata.insert("trust_tier".into(), trust_tier.as_str().into());
    metadata.insert("sender_verified".into(), verification.is_verified().into());
    metadata.insert("sender_verification".into(), verification.as_str().into());
    NormalizedEvent {
        provider_event_id: provider_event_id(address, summary),
        metadata: serde_json::Value::Object(metadata),
        occurred_at: summary.date,
        landing: None,
    }
}

/// What deduplication keys on: the `Message-ID` the sender wrote, and
/// the folder and UID where a message carries none (ADR-0019). A
/// resync after a reconnect therefore never wakes the Agent twice.
///
/// The mailbox address leads it, because one Connection carries the
/// mailboxes of several Agents, and one message sent to two of them is
/// two occurrences.
pub fn provider_event_id(address: &str, summary: &MessageSummary) -> String {
    let id = if summary.message_id.is_empty() {
        summary.id.to_string()
    } else {
        summary.message_id.clone()
    };
    format!("{address} {id}")
}

/// Reads a typed mail filter against one message's metadata
/// (ADR-0006): different fields combine with AND, values inside one
/// field combine with OR, and text matching is case-insensitive.
#[derive(Debug, Default, Clone, Copy)]
pub struct MailMessageMatcher;

impl EventMatcher for MailMessageMatcher {
    fn matches(&self, filter: &serde_json::Value, metadata: &serde_json::Value) -> bool {
        if let Some(mailbox) = filter["mailbox"].as_str()
            && metadata["mailbox"].as_str() != Some(mailbox)
        {
            return false;
        }
        if let Some(senders) = values(filter, "senders")
            && !sender_matches(&senders, metadata)
        {
            return false;
        }
        if let Some(needles) = values(filter, "subject_contains") {
            let subject = metadata["subject"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase();
            if !needles
                .iter()
                .any(|needle| subject.contains(&needle.to_lowercase()))
            {
                return false;
            }
        }
        if let Some(wanted) = filter["min_trust"].as_str().and_then(tier) {
            // The tiers are ordered by authority, so "at least this
            // much" is one comparison. An unreadable tier is unknown.
            let held = metadata["trust_tier"]
                .as_str()
                .and_then(tier)
                .unwrap_or(TrustTier::Unknown);
            if held < wanted {
                return false;
            }
        }
        if filter["replies_only"].as_bool() == Some(true) && metadata["in_reply_to"].is_null() {
            return false;
        }
        true
    }
}

/// The tier one message's sender has (ADR-0019). The `From` address
/// selects a candidate tier, and the candidate holds only when the
/// evidence is verified: any other evidence is Unknown. The tier stamps
/// the event and gates what the words are worth, never whether the
/// Agent wakes.
#[async_trait]
pub trait SenderTrust: Send + Sync {
    async fn tier(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        sender: &SenderEvidence,
    ) -> Result<TrustTier, StoreError>;
}

/// The tier of one sender (ADR-0019): Unknown unless the sender is
/// verified, and then the candidate from the Trust List first and the
/// addresses of the user's own mail Connections after it.
///
/// The order of the candidate is exact address, then bare domain, then
/// the Connection addresses, else unknown. An entry the user wrote
/// therefore says more than the domain it sits in, and the addresses of
/// the user's own accounts are owner with no entry at all: the user
/// cannot remove them, because they are not rows.
pub struct ListedSenders {
    list: Arc<dyn TrustListStore>,
    connections: Arc<dyn ConnectionStore>,
}

impl ListedSenders {
    pub fn new(list: Arc<dyn TrustListStore>, connections: Arc<dyn ConnectionStore>) -> Self {
        Self { list, connections }
    }

    /// Whether the address is the account of one of the user's mail
    /// Connections.
    async fn is_own_account(
        &self,
        workspace_id: &WorkspaceId,
        address: &str,
    ) -> Result<bool, StoreError> {
        Ok(self
            .connections
            .list(workspace_id)
            .await?
            .iter()
            .filter_map(connection_account)
            .any(|account| account == address))
    }
}

#[async_trait]
impl SenderTrust for ListedSenders {
    async fn tier(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        sender: &SenderEvidence,
    ) -> Result<TrustTier, StoreError> {
        // An unverified `From` is text the sender wrote, so it selects
        // nothing and the list is not read.
        if !sender.verification.is_verified() {
            return Ok(TrustTier::Unknown);
        }
        let from = sender.from.as_str();
        let Some(address) = bare_address(from) else {
            return Ok(TrustTier::Unknown);
        };
        if let Some(tier) = self
            .list
            .candidate(workspace_id, agent_id, TrustSubject::Address, &address)
            .await?
        {
            return Ok(tier);
        }
        if let Some(domain) = sender_domain(from)
            && let Some(tier) = self
                .list
                .candidate(workspace_id, agent_id, TrustSubject::Domain, &domain)
                .await?
        {
            return Ok(tier);
        }
        Ok(match self.is_own_account(workspace_id, &address).await? {
            true => TrustTier::Owner,
            false => TrustTier::Unknown,
        })
    }
}

/// The address one Connection acts as. A Connection is an account the
/// user holds, so the address on it is the user's own, whatever the
/// provider behind it. It is the owner candidate with no Trust List
/// row, and the Trusted contacts tab greys it for that reason
/// (ADR-0019).
pub fn connection_account(connection: &Connection) -> Option<String> {
    connection.config[ACCOUNT_KEY]
        .as_str()
        .and_then(bare_address)
}

/// A sender value is an address or a bare domain, and either one
/// matches the message's own address or its domain.
fn sender_matches(senders: &[String], metadata: &serde_json::Value) -> bool {
    let from = metadata["from"].as_str().unwrap_or_default();
    let address = bare_address(from);
    let domain = metadata["from_domain"]
        .as_str()
        .map(str::to_string)
        .or_else(|| sender_domain(from));
    senders.iter().any(|sender| {
        let sender = sender.trim().trim_start_matches('@').to_lowercase();
        if sender.contains('@') {
            address.as_deref() == Some(sender.as_str())
        } else {
            domain.as_deref() == Some(sender.as_str())
        }
    })
}

/// The bare address inside a `From` header value, lowercased. It is
/// untrusted text: it is compared and nothing else.
pub fn bare_address(from: &str) -> Option<String> {
    let address = from
        .rsplit_once('<')
        .map(|(_, rest)| rest.trim_end_matches('>'))
        .unwrap_or(from);
    let address = address.trim().to_lowercase();
    address.contains('@').then_some(address)
}

/// The domain of a `From` header value, lowercased.
pub fn sender_domain(from: &str) -> Option<String> {
    let address = bare_address(from)?;
    let domain = crate::address::address_domain(&address)?;
    (!domain.is_empty()).then(|| domain.to_string())
}

fn tier(value: &str) -> Option<TrustTier> {
    value.parse().ok()
}

/// A filter field's values, or `None` where the field is absent or
/// empty: an empty list narrows nothing.
fn values(filter: &serde_json::Value, name: &str) -> Option<Vec<String>> {
    let values: Vec<String> = filter[name]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (!values.is_empty()).then_some(values)
}
