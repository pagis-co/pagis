//! The Mailbox Provider as a Connection (ADR-0019): what the record
//! holds, and what the provider declares it can do.
//!
//! The record holds the domain the mailboxes live on, the IMAP and
//! SMTP endpoints when it uses that transport, and the capability set of the
//! two seams. It holds no secret: the host API key is in the secret
//! store under the Connection alias (ADR-0013), and each mailbox
//! password is under the mailbox address.

use pagis_core::Connection;
use serde::{Deserialize, Serialize};

use crate::host::HostCapabilities;
use crate::transport::Endpoint;

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

/// What one Mailbox Provider can and cannot do (ADR-0005, ADR-0019):
/// the mail seam's half and the mailbox seam's half in one set. The
/// Connection page shows what is absent instead of hiding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailboxCapabilities {
    /// The IDLE display flag on the Connection page. The server's
    /// CAPABILITY response selects IDLE or polling for each session.
    pub idle: bool,
    /// The host applies the Outgoing Cap to the mailbox itself. The
    /// daemon enforces the cap for every host either way.
    pub outgoing_cap: bool,
    /// The host deletes the mailbox. Without it the user deletes the
    /// mailbox at the host and Pagis forgets its record.
    pub delete_mailbox: bool,
    /// The daemon mints a new mailbox password through the host API.
    /// With a login and without it, the Person pastes the new password.
    pub reset_password: bool,
}

impl MailboxCapabilities {
    /// What the two seams of one Connection declare together.
    pub fn of(host: HostCapabilities, idle: bool) -> Self {
        Self {
            idle,
            outgoing_cap: host.outgoing_cap,
            delete_mailbox: host.delete_mailbox,
            reset_password: host.reset_password,
        }
    }
}

/// How an Agent Mailbox gets its password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxPasswordKind {
    Minted,
    Typed,
    None,
}

/// The mail settings of one Mailbox Provider Connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxProvider {
    /// The login the host API knows, for example the Migadu account
    /// email address. A manual host has none.
    pub account: Option<String>,
    pub domain: String,
    /// Absent when the Embedder supplies `Mailboxes`.
    pub imap: Option<Endpoint>,
    /// Absent when the Embedder supplies `Mailboxes`.
    pub smtp: Option<Endpoint>,
    pub capabilities: MailboxCapabilities,
}

impl MailboxProvider {
    /// A mailbox with neither endpoint has no login and no password.
    /// With endpoints, the host mints it or the Person types it.
    pub fn password(&self) -> MailboxPasswordKind {
        if self.imap.is_none() && self.smtp.is_none() {
            MailboxPasswordKind::None
        } else if self.capabilities.reset_password {
            MailboxPasswordKind::Minted
        } else {
            MailboxPasswordKind::Typed
        }
    }

    /// The `config` document the Connection record carries. It holds
    /// no secret, so it is safe to read back to the desk.
    pub fn config(&self) -> serde_json::Value {
        let mut config = serde_json::json!({
            ACCOUNT_KEY: self.account,
            DOMAIN_KEY: self.domain,
            CAPABILITIES_KEY: serde_json::to_value(self.capabilities)
                .expect("capability flags serialize"),
        });
        if let Some(imap) = &self.imap {
            config[IMAP_HOST_KEY] = imap.host.clone().into();
            config[IMAP_PORT_KEY] = imap.port.into();
        }
        if let Some(smtp) = &self.smtp {
            config[SMTP_HOST_KEY] = smtp.host.clone().into();
            config[SMTP_PORT_KEY] = smtp.port.into();
        }
        config
    }
}

/// The mail settings of a Connection, when it is a Mailbox Provider
/// and its record is complete. The settings identify it; the Embedder
/// names the provider. Both endpoints can be absent.
pub fn mailbox_provider(connection: &Connection) -> Option<MailboxProvider> {
    let config = &connection.config;
    let imap = endpoint(config, IMAP_HOST_KEY, IMAP_PORT_KEY)?;
    let smtp = endpoint(config, SMTP_HOST_KEY, SMTP_PORT_KEY)?;
    if imap.is_some() != smtp.is_some() {
        return None;
    }
    Some(MailboxProvider {
        account: config[ACCOUNT_KEY].as_str().map(str::to_string),
        domain: config[DOMAIN_KEY].as_str()?.to_string(),
        imap,
        smtp,
        capabilities: serde_json::from_value(config[CAPABILITIES_KEY].clone()).ok()?,
    })
}

fn endpoint(
    config: &serde_json::Value,
    host_key: &str,
    port_key: &str,
) -> Option<Option<Endpoint>> {
    if config[host_key].is_null() && config[port_key].is_null() {
        return Some(None);
    }
    let host = config[host_key].as_str()?;
    let port = u16::try_from(config[port_key].as_u64()?).ok()?;
    Some(Some(Endpoint::new(host, port)))
}
