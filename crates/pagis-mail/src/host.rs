//! The mailbox seam (ADR-0019): create, delete, reset and list the
//! mailboxes of one domain. It uses the host API key, and it never
//! reads or sends mail. The mail seam, [`crate::MailTransport`], is the
//! other half and uses the mailbox's own password instead, so the API
//! key stays out of the mail path.

use async_trait::async_trait;

/// One mail account at one host: the domain the mailboxes live on, and
/// the credential that administers it. A manual host has no API
/// credential and uses [`HostAccount::manual`].
///
/// The key has no accessor that prints it and no `Debug` that shows it,
/// so a key cannot reach a log line by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct HostAccount {
    domain: String,
    account: String,
    api_key: String,
}

impl HostAccount {
    /// `account` is the login the host knows, for example the Migadu
    /// account email address; `api_key` is the key it issued.
    pub fn new(
        domain: impl Into<String>,
        account: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            domain: domain.into(),
            account: account.into(),
            api_key: api_key.into(),
        }
    }

    /// A domain with no host API. The manual host ignores the
    /// credential, because the user does the work at the host.
    pub fn manual(domain: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            account: String::new(),
            api_key: String::new(),
        }
    }

    /// The mail domain, for example `example.com`.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// The login the host knows.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// The one reader. Only a host client calls it.
    pub fn expose_api_key(&self) -> &str {
        &self.api_key
    }
}

impl std::fmt::Debug for HostAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostAccount")
            .field("domain", &self.domain)
            .field("account", &self.account)
            .field("api_key", &"redacted")
            .finish()
    }
}

/// The password of one mailbox. The daemon generates it, gives it to
/// the host and to the transport, and never shows it (ADR-0019).
#[derive(Clone, PartialEq, Eq)]
pub struct MailboxPassword(String);

impl MailboxPassword {
    pub fn new(password: impl Into<String>) -> Self {
        Self(password.into())
    }

    /// The one reader. Only a host client or a transport calls it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for MailboxPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MailboxPassword(redacted)")
    }
}

/// What a host can and cannot do (ADR-0005, ADR-0019). A capability
/// that is absent is absent: the daemon shows the difference on the
/// Connection page instead of hiding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostCapabilities {
    /// Whether the host applies the Outgoing Cap to the mailbox itself.
    /// The daemon enforces the cap for every host either way.
    pub outgoing_cap: bool,
    /// Whether the host deletes the mailbox. A manual host cannot: it
    /// forgets the record and the user deletes the mailbox at the host.
    pub delete_mailbox: bool,
    /// Whether the daemon mints a new password through the host API. A
    /// manual host cannot: the user pastes the new password.
    pub reset_password: bool,
}

/// One mailbox as the host holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedMailbox {
    /// The full address, for example `ava@example.com`.
    pub address: String,
    pub local_part: String,
    pub domain: String,
}

impl HostedMailbox {
    pub fn new(local_part: impl Into<String>, domain: impl Into<String>) -> Self {
        let local_part = local_part.into();
        let domain = domain.into();
        Self {
            address: format!("{local_part}@{domain}"),
            local_part,
            domain,
        }
    }
}

/// What a `delete` did. A host that holds the mailbox removes it; a
/// manual host forgets its record and returns the message the desk
/// shows the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deletion {
    /// The host removed the mailbox and its mail.
    Removed,
    /// Pagis forgot the mailbox. The mailbox is still at the host, and
    /// `notice` tells the user to delete it there.
    UserMustDelete { notice: String },
}

/// Why the host did not do what it was asked. The codes are stable,
/// because they reach the user and the tests; no upstream text is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostErrorCode {
    /// The address is in use at the host.
    AddressTaken,
    /// The host refused the API key.
    Unauthorized,
    /// The account is at the mailbox or message limit of its plan.
    CapReached,
    /// The host does not hold this mailbox.
    MailboxUnknown,
    /// The host refused for a reason of its own.
    Refused,
    /// The host did not answer, or answered with a failure it calls
    /// temporary.
    TemporarilyUnavailable,
    /// The host answered with something this client cannot read.
    Unreadable,
}

impl HostErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            HostErrorCode::AddressTaken => "address_taken",
            HostErrorCode::Unauthorized => "unauthorized",
            HostErrorCode::CapReached => "cap_reached",
            HostErrorCode::MailboxUnknown => "mailbox_unknown",
            HostErrorCode::Refused => "refused",
            HostErrorCode::TemporarilyUnavailable => "temporarily_unavailable",
            HostErrorCode::Unreadable => "unreadable",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the mail host did not complete this: {}", .0.as_str())]
pub struct HostError(pub HostErrorCode);

/// Create, delete, reset and list the mailboxes of one domain
/// (ADR-0019).
///
/// One implementation serves every Connection of its provider, so the
/// account travels with each call: the API key comes from the secret
/// store under the Connection alias, and the domain comes from the
/// Connection.
#[async_trait]
pub trait MailboxHost: Send + Sync {
    /// What this host declares. The desk reads it; nothing else decides
    /// what a mailbox may do.
    fn capabilities(&self) -> HostCapabilities;

    /// Make one mailbox with the password the daemon generated.
    /// `outgoing_cap` is the messages a day the mailbox may send; a
    /// host that declares `outgoing_cap` false ignores it, and the
    /// daemon still enforces it on every send.
    async fn create(
        &self,
        account: &HostAccount,
        local_part: &str,
        password: &MailboxPassword,
        outgoing_cap: u32,
    ) -> Result<HostedMailbox, HostError>;

    /// Delete one mailbox and its mail.
    async fn delete(&self, account: &HostAccount, address: &str) -> Result<Deletion, HostError>;

    /// Give the mailbox a new password. The transport logs in again
    /// with it.
    async fn reset_password(
        &self,
        account: &HostAccount,
        address: &str,
        password: &MailboxPassword,
    ) -> Result<(), HostError>;

    /// Every mailbox the host holds on the domain. A Migadu Connection
    /// is `connected` once the key answers this call.
    async fn list(&self, account: &HostAccount) -> Result<Vec<HostedMailbox>, HostError>;
}

/// The local part of an address on this account's domain. An address on
/// another domain is not this account's to touch.
pub(crate) fn local_part_on(account: &HostAccount, address: &str) -> Result<String, HostError> {
    match address.rsplit_once('@') {
        Some((local_part, domain))
            if !local_part.is_empty() && domain.eq_ignore_ascii_case(account.domain()) =>
        {
            Ok(local_part.to_string())
        }
        _ => Err(HostError(HostErrorCode::MailboxUnknown)),
    }
}
