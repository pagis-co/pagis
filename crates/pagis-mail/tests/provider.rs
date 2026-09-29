//! The Mailbox Provider Connection contract (ADR-0019): what
//! the record carries, what it declares, and what it never holds.

use pagis_mail::{
    Endpoint, HostCapabilities, MANUAL_PROVIDER, MIGADU_PROVIDER, MailboxCapabilities,
    MailboxProvider, TransportCapabilities, authserv_ids, is_mailbox_provider, mailbox_provider,
};

use pagis_core::{Connection, ConnectionId, WorkspaceId};

fn connection(provider: &str, config: serde_json::Value) -> Connection {
    Connection {
        id: ConnectionId::generate(),
        workspace_id: WorkspaceId::from("ws-1".to_string()),
        provider: provider.to_string(),
        alias: "mail".to_string(),
        display_name: "Mail".to_string(),
        status: Connection::CONNECTED.to_string(),
        auth_mode: Connection::AUTH_MODE_BYO.to_string(),
        authorized_capabilities: Vec::new(),
        config,
        created_at: 0,
    }
}

fn migadu_settings() -> MailboxProvider {
    MailboxProvider {
        account: Some("owner@example.com".to_string()),
        domain: "example.com".to_string(),
        imap: Endpoint::new("imap.migadu.com", 993),
        smtp: Endpoint::new("smtp.migadu.com", 465),
        capabilities: MailboxCapabilities {
            idle: true,
            outgoing_cap: true,
            delete_mailbox: true,
            reset_password: true,
        },
    }
}

#[test]
fn the_record_carries_the_domain_the_endpoints_and_what_the_seams_declare() {
    let settings = migadu_settings();
    let read = mailbox_provider(&connection(MIGADU_PROVIDER, settings.config())).expect("settings");

    assert_eq!(read, settings);
}

#[test]
fn a_manual_record_names_its_own_endpoints_and_no_account() {
    let settings = MailboxProvider {
        account: None,
        domain: "example.org".to_string(),
        imap: Endpoint::new("imap.example.org", 993),
        smtp: Endpoint::new("smtp.example.org", 587),
        capabilities: MailboxCapabilities {
            idle: true,
            outgoing_cap: false,
            delete_mailbox: false,
            reset_password: false,
        },
    };

    let read = mailbox_provider(&connection(MANUAL_PROVIDER, settings.config())).expect("settings");

    assert_eq!(read, settings);
    assert_eq!(read.account, None);
}

#[test]
fn the_record_holds_only_what_the_desk_may_read() {
    let config = migadu_settings().config();

    let mut keys: Vec<&str> = config
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    // The host API key and the mailbox passwords are in the secret
    // store, so no field here can carry one (ADR-0013).
    assert_eq!(
        keys,
        [
            "account",
            "capabilities",
            "domain",
            "imap_host",
            "imap_port",
            "smtp_host",
            "smtp_port"
        ]
    );
}

#[test]
fn the_capability_set_is_the_two_seams_together() {
    let capabilities = MailboxCapabilities::of(
        HostCapabilities {
            outgoing_cap: true,
            delete_mailbox: false,
            reset_password: false,
        },
        TransportCapabilities { idle: true },
    );

    assert!(capabilities.idle);
    assert!(capabilities.outgoing_cap);
    // What is absent stays absent, so the page can say so.
    assert!(!capabilities.delete_mailbox);
    assert!(!capabilities.reset_password);
}

#[test]
fn a_connection_of_another_provider_has_no_mail_settings() {
    assert!(!is_mailbox_provider("google"));
    assert!(mailbox_provider(&connection("google", migadu_settings().config())).is_none());
}

#[test]
fn a_record_without_endpoints_reads_as_no_settings() {
    let incomplete = serde_json::json!({ "domain": "example.com" });

    assert!(mailbox_provider(&connection(MIGADU_PROVIDER, incomplete)).is_none());
}

/// Migadu writes the host name of the exchanger that took the message
/// as the authserv-id: the first or the second MX of every Migadu
/// domain. A manual host is any host, so Pagis knows no authserv-id
/// for it (ADR-0019).
#[test]
fn a_migadu_connection_trusts_the_results_of_its_two_exchangers_and_a_manual_one_none() {
    assert_eq!(
        authserv_ids(MIGADU_PROVIDER),
        ["aspmx1.migadu.com", "aspmx2.migadu.com"]
    );
    assert!(authserv_ids(MANUAL_PROVIDER).is_empty());
    assert!(authserv_ids("google").is_empty());
}
