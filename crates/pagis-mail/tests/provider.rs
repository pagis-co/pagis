//! The Mailbox Provider Connection contract (ADR-0019): what
//! the record carries, what it declares, and what it never holds.

use pagis_mail::{
    Endpoint, HostCapabilities, MANUAL_PROVIDER, MIGADU_PROVIDER, MailboxCapabilities,
    MailboxProvider, authserv_ids, mailbox_provider,
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
        authorized_capabilities: Vec::new(),
        config,
        created_at: 0,
    }
}

fn migadu_settings() -> MailboxProvider {
    MailboxProvider {
        account: Some("owner@example.com".to_string()),
        domain: "example.com".to_string(),
        imap: Some(Endpoint::new("imap.migadu.com", 993)),
        smtp: Some(Endpoint::new("smtp.migadu.com", 465)),
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
        imap: Some(Endpoint::new("imap.example.org", 993)),
        smtp: Some(Endpoint::new("smtp.example.org", 587)),
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
        true,
    );

    assert!(capabilities.idle);
    assert!(capabilities.outgoing_cap);
    // What is absent stays absent, so the page can say so.
    assert!(!capabilities.delete_mailbox);
    assert!(!capabilities.reset_password);
}

#[test]
fn a_connection_without_mail_settings_is_not_a_mailbox_provider() {
    assert!(
        mailbox_provider(&connection(
            "google",
            serde_json::json!({"account": "owner@example.com"})
        ))
        .is_none()
    );
}

#[test]
fn a_record_without_capabilities_reads_as_no_settings() {
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

#[test]
fn an_embedder_names_a_provider_without_endpoints() {
    let mut config = migadu_settings().config();
    for key in ["imap_host", "imap_port", "smtp_host", "smtp_port"] {
        config.as_object_mut().unwrap().remove(key);
    }
    let read = mailbox_provider(&connection("embedder-mail", config))
        .expect("mail settings without endpoints");
    assert_eq!(read.domain, "example.com");
    assert_eq!(read.imap, None);
    assert_eq!(read.smtp, None);
    assert_eq!(
        mailbox_provider(&connection("embedder-mail", read.config())),
        Some(read)
    );
}

#[test]
fn malformed_or_partial_endpoints_are_not_mail_settings() {
    for (key, value) in [
        ("imap_host", serde_json::json!(42)),
        ("imap_port", serde_json::json!(70000)),
        ("smtp_port", serde_json::json!("465")),
    ] {
        let mut config = migadu_settings().config();
        config[key] = value;
        assert!(mailbox_provider(&connection("embedder-mail", config)).is_none());
    }
    for keys in [vec!["imap_port"], vec!["smtp_host", "smtp_port"]] {
        let mut config = migadu_settings().config();
        for key in keys {
            config.as_object_mut().unwrap().remove(key);
        }
        assert!(mailbox_provider(&connection("embedder-mail", config)).is_none());
    }
}

#[test]
fn an_embedder_names_a_provider_with_endpoints() {
    let settings = migadu_settings();
    assert_eq!(
        mailbox_provider(&connection("embedder-mail", settings.config())),
        Some(settings)
    );
}
