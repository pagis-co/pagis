//! The Migadu host against a real Migadu account (ADR-0019). It
//! is never in the gate: it needs an account, it makes and deletes a
//! mailbox on a real domain, and it costs money. It runs only when the
//! environment names an account:
//!
//! ```text
//! PAGIS_LIVE_MIGADU=1 \
//! PAGIS_LIVE_MIGADU_DOMAIN=example.com \
//! PAGIS_LIVE_MIGADU_ACCOUNT=owner@example.com \
//! PAGIS_LIVE_MIGADU_API_KEY=... \
//! cargo test -p pagis-mail --test main -- --ignored live_migadu::
//! ```
//!
//! The test makes one mailbox with a name of its own, and it deletes
//! that mailbox at the end. It touches no other mailbox of the domain.

use pagis_mail::{
    Deletion, HostAccount, HostCapabilities, MailboxHost, MailboxPassword, MigaduHost,
};

fn variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The account the environment names, or nothing.
fn live_account() -> Option<HostAccount> {
    variable("PAGIS_LIVE_MIGADU")?;
    Some(HostAccount::new(
        variable("PAGIS_LIVE_MIGADU_DOMAIN").expect("PAGIS_LIVE_MIGADU_DOMAIN"),
        variable("PAGIS_LIVE_MIGADU_ACCOUNT").expect("PAGIS_LIVE_MIGADU_ACCOUNT"),
        variable("PAGIS_LIVE_MIGADU_API_KEY").expect("PAGIS_LIVE_MIGADU_API_KEY"),
    ))
}

/// A name no other run of this test holds.
fn local_part() -> String {
    format!("pagis-live-{}", pagis_core::now_ms())
}

#[tokio::test]
#[ignore = "needs a live Migadu account; set PAGIS_LIVE_MIGADU and run with --ignored"]
async fn a_live_migadu_account_makes_lists_resets_and_deletes_one_mailbox() {
    let Some(account) = live_account() else {
        eprintln!("PAGIS_LIVE_MIGADU is not set; the live Migadu test did nothing");
        return;
    };
    let host = MigaduHost::new().expect("the client builds");
    assert_eq!(
        host.capabilities(),
        HostCapabilities {
            outgoing_cap: true,
            delete_mailbox: true,
            reset_password: true,
        }
    );

    let local_part = local_part();
    let made = host
        .create(
            &account,
            &local_part,
            &MailboxPassword::new(format!("Pagis-live-{}-Aa1!", pagis_core::now_ms())),
            20,
        )
        .await
        .expect("the account makes the mailbox");
    assert_eq!(made.local_part, local_part);
    assert_eq!(made.domain, account.domain());

    // Everything after the create runs against a mailbox that exists,
    // so the delete runs even where a step in between fails.
    let outcome = live_steps(&host, &account, &made.address).await;
    let deletion = host
        .delete(&account, &made.address)
        .await
        .expect("the account deletes the mailbox");
    assert_eq!(deletion, Deletion::Removed);
    assert!(
        !host
            .list(&account)
            .await
            .expect("the list reads")
            .iter()
            .any(|mailbox| mailbox.address == made.address),
        "the deleted mailbox is gone from the host"
    );
    outcome.expect("the live steps pass");
}

/// The steps between the create and the delete, as a result, so one
/// failure does not leave a mailbox on the domain.
async fn live_steps(host: &MigaduHost, account: &HostAccount, address: &str) -> Result<(), String> {
    let listed = host.list(account).await.map_err(|e| e.to_string())?;
    if !listed.iter().any(|mailbox| mailbox.address == address) {
        return Err(format!("the list does not hold {address}"));
    }
    host.reset_password(
        account,
        address,
        &MailboxPassword::new(format!("Pagis-reset-{}-Aa1!", pagis_core::now_ms())),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
