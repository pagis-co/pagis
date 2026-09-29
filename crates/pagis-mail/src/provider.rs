//! The Mailbox Provider as a Connection (ADR-0019): what the record
//! holds, and what the provider declares it can do.
//!
//! The record holds the domain the mailboxes live on, the IMAP and
//! SMTP endpoints their transport uses, and the capability set of the
//! two seams. It holds no secret: the host API key is in the secret
//! store under the Connection alias (ADR-0013), and each mailbox
//! password is under the mailbox address.

use pagis_core::Connection;
use serde::{Deserialize, Serialize};

use crate::host::HostCapabilities;
use crate::transport::{Endpoint, TransportCapabilities};

/// The login the host knows, for a host that has an API. It is the
/// account the Connection card names, as it is for Google.
pub const ACCOUNT_KEY: &str = "account";
/// Where the mailboxes of this Connection live, e.g. `example.com`.
pub const DOMAIN_KEY: &str = "domain";
/// The IMAP host and port the transport reads from.
pub const IMAP_HOST_KEY: &str = "imap_host";
pub const IMAP_PORT_KEY: &str = "imap_port";
/// The SMTP host and port the transport sends through.
pub const SMTP_HOST_KEY: &str = "smtp_host";
pub const SMTP_PORT_KEY: &str = "smtp_port";
/// What the two seams of this Connection declare.
pub const CAPABILITIES_KEY: &str = "capabilities";

/// Where a Migadu mailbox reads and sends (ADR-0019). Every Migadu
/// Connection uses these, so the user gives the domain alone.
pub const MIGADU_IMAP: (&str, u16) = ("imap.migadu.com", 993);
pub const MIGADU_SMTP: (&str, u16) = ("smtp.migadu.com", 465);

/// The authserv-ids Migadu writes in the Authentication-Results header
/// of the mail it receives (RFC 8601): the host name of the exchanger
/// that took the message, which is the first or the second MX of every
/// Migadu domain.
const MIGADU_AUTHSERV_IDS: &[&str] = &["aspmx1.migadu.com", "aspmx2.migadu.com"];

/// The authserv-ids of the host behind one Mailbox Provider. The
/// collector trusts an Authentication-Results header with one of them
/// and no other (ADR-0019). A manual host can be any host, so Pagis
/// knows none for it, and no sender on it is verified.
pub fn authserv_ids(provider: &str) -> &'static [&'static str] {
    match provider {
        crate::MIGADU_PROVIDER => MIGADU_AUTHSERV_IDS,
        _ => &[],
    }
}

/// What the daemon's mail path offers every Mailbox Provider. One
/// transport serves every Connection, so a Connection records this
/// beside what its own host declares.
pub const MAIL_TRANSPORT: TransportCapabilities = TransportCapabilities { idle: true };

/// What one Mailbox Provider can and cannot do (ADR-0005, ADR-0019):
/// the mail seam's half and the mailbox seam's half in one set. The
/// Connection page shows what is absent instead of hiding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailboxCapabilities {
    /// The transport waits for new mail on the connection. Without it
    /// the collector polls the mailbox.
    pub idle: bool,
    /// The host applies the Outgoing Cap to the mailbox itself. The
    /// daemon enforces the cap for every host either way.
    pub outgoing_cap: bool,
    /// The host deletes the mailbox. Without it the user deletes the
    /// mailbox at the host and Pagis forgets its record.
    pub delete_mailbox: bool,
    /// The daemon mints a new mailbox password through the host API.
    /// Without it the user pastes the new password.
    pub reset_password: bool,
}

impl MailboxCapabilities {
    /// What the two seams of one Connection declare together.
    pub fn of(host: HostCapabilities, transport: TransportCapabilities) -> Self {
        Self {
            idle: transport.idle,
            outgoing_cap: host.outgoing_cap,
            delete_mailbox: host.delete_mailbox,
            reset_password: host.reset_password,
        }
    }
}

/// The mail settings of one Mailbox Provider Connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxProvider {
    /// The login the host API knows, for example the Migadu account
    /// email address. A manual host has none.
    pub account: Option<String>,
    pub domain: String,
    pub imap: Endpoint,
    pub smtp: Endpoint,
    pub capabilities: MailboxCapabilities,
}

impl MailboxProvider {
    /// The `config` document the Connection record carries. It holds
    /// no secret, so it is safe to read back to the desk.
    pub fn config(&self) -> serde_json::Value {
        serde_json::json!({
            ACCOUNT_KEY: self.account,
            DOMAIN_KEY: self.domain,
            IMAP_HOST_KEY: self.imap.host,
            IMAP_PORT_KEY: self.imap.port,
            SMTP_HOST_KEY: self.smtp.host,
            SMTP_PORT_KEY: self.smtp.port,
            CAPABILITIES_KEY: serde_json::to_value(self.capabilities)
                .expect("capability flags serialize"),
        })
    }
}

/// True for the Connections whose mailboxes an Agent Mailbox uses.
pub fn is_mailbox_provider(provider: &str) -> bool {
    provider == crate::MIGADU_PROVIDER || provider == crate::MANUAL_PROVIDER
}

/// The mail settings of a Connection, when it is a Mailbox Provider
/// and its record is complete.
pub fn mailbox_provider(connection: &Connection) -> Option<MailboxProvider> {
    if !is_mailbox_provider(&connection.provider) {
        return None;
    }
    let config = &connection.config;
    Some(MailboxProvider {
        account: config[ACCOUNT_KEY].as_str().map(str::to_string),
        domain: config[DOMAIN_KEY].as_str()?.to_string(),
        imap: endpoint(config, IMAP_HOST_KEY, IMAP_PORT_KEY)?,
        smtp: endpoint(config, SMTP_HOST_KEY, SMTP_PORT_KEY)?,
        capabilities: serde_json::from_value(config[CAPABILITIES_KEY].clone()).ok()?,
    })
}

fn endpoint(config: &serde_json::Value, host_key: &str, port_key: &str) -> Option<Endpoint> {
    let host = config[host_key].as_str()?;
    let port = u16::try_from(config[port_key].as_u64()?).ok()?;
    Some(Endpoint::new(host, port))
}
