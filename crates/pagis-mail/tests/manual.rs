//! The manual host (ADR-0019): the user makes the mailbox at
//! the host, and Pagis records what the user typed.

use pagis_mail::{Deletion, HostAccount, HostErrorCode, MailboxHost, MailboxPassword, ManualHost};

fn account() -> HostAccount {
    HostAccount::manual("example.com")
}

#[test]
fn the_host_declares_no_delete_and_no_password_reset() {
    let capabilities = ManualHost::new().capabilities();

    assert!(!capabilities.delete_mailbox);
    assert!(!capabilities.reset_password);
    assert!(!capabilities.outgoing_cap);
}

#[tokio::test]
async fn create_records_the_address_and_the_password_the_user_typed() {
    let host = ManualHost::new();

    let made = host
        .create(&account(), "ava", &MailboxPassword::new("typed"), 20)
        .await
        .unwrap();

    assert_eq!(made.address, "ava@example.com");
    assert_eq!(made.local_part, "ava");
    assert_eq!(made.domain, "example.com");
}

#[tokio::test]
async fn create_without_a_password_is_refused() {
    let host = ManualHost::new();

    let refused = host
        .create(&account(), "ava", &MailboxPassword::new(""), 20)
        .await
        .unwrap_err();

    assert_eq!(refused.0, HostErrorCode::Refused);
}

#[tokio::test]
async fn delete_forgets_the_mailbox_and_tells_the_user_to_delete_it() {
    let host = ManualHost::new();

    let deleted = host.delete(&account(), "ava@example.com").await.unwrap();

    assert_eq!(
        deleted,
        Deletion::UserMustDelete {
            notice: "Pagis forgot ava@example.com. Delete the mailbox at your mail host."
                .to_string(),
        }
    );
}

#[tokio::test]
async fn reset_password_takes_the_password_the_user_pasted() {
    let host = ManualHost::new();

    host.reset_password(
        &account(),
        "ava@example.com",
        &MailboxPassword::new("pasted"),
    )
    .await
    .unwrap();

    let refused = host
        .reset_password(&account(), "ava@example.com", &MailboxPassword::new(""))
        .await
        .unwrap_err();
    assert_eq!(refused.0, HostErrorCode::Refused);
}

#[tokio::test]
async fn an_address_on_another_domain_is_not_this_accounts() {
    let host = ManualHost::new();

    assert_eq!(
        host.delete(&account(), "ava@other.com")
            .await
            .unwrap_err()
            .0,
        HostErrorCode::MailboxUnknown
    );
    assert_eq!(
        host.reset_password(&account(), "ava@other.com", &MailboxPassword::new("p"))
            .await
            .unwrap_err()
            .0,
        HostErrorCode::MailboxUnknown
    );
}

#[tokio::test]
async fn a_manual_host_has_no_directory_to_read() {
    let host = ManualHost::new();

    assert!(host.list(&account()).await.unwrap().is_empty());
}
