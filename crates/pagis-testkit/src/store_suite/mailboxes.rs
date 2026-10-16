//! The Agent Mailbox record and the Address Ledger trait tests of the
//! suite (ADR-0019).
//!
//! The schema, not the caller, owns the two rules: one Agent holds one
//! live mailbox, and no address is ever reused. These bodies hold both
//! backends to them.
//!
//! Each body is one test. It takes a [`Backend`], reads the store set
//! from it, and never names a pool type, so it runs on both backends
//! from one text. Name every new body in `store_suite_mailboxes!`
//! below: the guard test of the parent module fails while one is
//! missing.

use pagis_core::{
    Agent, AgentId, AgentMailbox, AgentMailboxId, AgentMailboxState, ConnectionId, MailboxCursor,
    StoreError, Workspace, WorkspaceId, day_start, now_ms,
};

use super::Backend;
use crate::fixture::agent;

/// One Workspace with two Agents in it: a mailbox needs an Agent, and
/// the ledger rules need two.
async fn seed(backend: &Backend) -> (Workspace, Agent, Agent) {
    let ws = backend.seeded_workspace().await;
    let agents = &backend.stores().agents;
    let first = agent(&ws.id);
    agents.create(&first).await.unwrap();
    let second = Agent {
        id: AgentId::generate(),
        name: "Ada".to_string(),
        ..agent(&ws.id)
    };
    agents.create(&second).await.unwrap();
    (ws, first, second)
}

fn mailbox(
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    connection_id: &ConnectionId,
    address: &str,
) -> AgentMailbox {
    AgentMailbox {
        id: AgentMailboxId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        connection_id: connection_id.clone(),
        address: address.to_string(),
        state: AgentMailboxState::Provisioning,
        reason: None,
        outgoing_cap: 20,
        allow_rules: Vec::new(),
        cursor: None,
        sends_day: None,
        sends_today: 0,
        created_at: now_ms(),
        deleted_at: None,
    }
}

/// A send card's `Always allow` writes its recipient domains here,
/// because an Agent holds its own mailbox with no Grant (ADR-0019).
pub async fn allow_rules_are_added_once_and_keep_their_order(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let record = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");
    store.create(&record).await.unwrap();

    let held = store
        .add_allow_rules(&ws.id, &record.id, &["other.test".to_string()])
        .await
        .unwrap();
    assert_eq!(held, ["other.test"]);

    let held = store
        .add_allow_rules(
            &ws.id,
            &record.id,
            &["other.test".to_string(), "elsewhere.test".to_string()],
        )
        .await
        .unwrap();

    assert_eq!(held, ["other.test", "elsewhere.test"]);
    assert_eq!(
        store
            .get(&ws.id, &record.id)
            .await
            .unwrap()
            .expect("the record")
            .allow_rules,
        ["other.test", "elsewhere.test"]
    );
}

pub async fn a_deleted_mailbox_takes_no_allow_rule(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let record = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");
    store.create(&record).await.unwrap();
    store.delete(&ws.id, &record.id, now_ms()).await.unwrap();

    let refused = store
        .add_allow_rules(&ws.id, &record.id, &["other.test".to_string()])
        .await;

    assert!(
        matches!(refused, Err(StoreError::Conflict(_))),
        "{refused:?}"
    );
}

pub async fn a_mailbox_roundtrips_through_the_store(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let record = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");

    store.create(&record).await.unwrap();

    assert_eq!(
        store.get(&ws.id, &record.id).await.unwrap(),
        Some(record.clone())
    );
    assert_eq!(
        store.for_agent(&ws.id, &a.id).await.unwrap(),
        Some(record.clone())
    );
    assert_eq!(store.list(&ws.id).await.unwrap(), vec![record]);
}

pub async fn the_schema_refuses_a_second_live_mailbox_for_one_agent(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let connection = ConnectionId::generate();
    store
        .create(&mailbox(&ws.id, &a.id, &connection, "ada@example.com"))
        .await
        .unwrap();

    let second = store
        .create(&mailbox(&ws.id, &a.id, &connection, "ada2@example.com"))
        .await;

    assert!(matches!(second, Err(StoreError::Conflict(_))), "{second:?}");
}

/// The Address Ledger spans every Mailbox Provider Connection: one
/// address is one address, whichever host carries it (ADR-0019).
pub async fn one_address_is_never_held_on_two_connections(backend: &Backend) {
    let (ws, a, b) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    store
        .create(&mailbox(
            &ws.id,
            &a.id,
            &ConnectionId::generate(),
            "ada@example.com",
        ))
        .await
        .unwrap();

    // A different Agent, a different Connection, the same address.
    let second = store
        .create(&mailbox(
            &ws.id,
            &b.id,
            &ConnectionId::generate(),
            "ada@example.com",
        ))
        .await;

    assert!(matches!(second, Err(StoreError::Conflict(_))), "{second:?}");
}

/// The tombstone is what makes the ledger a ledger (ADR-0019).
pub async fn a_deleted_address_is_never_given_out_again(backend: &Backend) {
    let (ws, a, b) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let first = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");
    store.create(&first).await.unwrap();
    assert!(store.delete(&ws.id, &first.id, now_ms()).await.unwrap());

    // The Agent holds none now, and the ledger still holds the address.
    assert_eq!(store.for_agent(&ws.id, &a.id).await.unwrap(), None);
    assert!(store.address_taken("ada@example.com").await.unwrap());
    let again = store
        .create(&mailbox(
            &ws.id,
            &b.id,
            &ConnectionId::generate(),
            "ada@example.com",
        ))
        .await;

    assert!(matches!(again, Err(StoreError::Conflict(_))), "{again:?}");
}

/// A delete frees the Agent, not the address (ADR-0019).
pub async fn an_agent_gets_a_fresh_mailbox_after_a_delete(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let connection = ConnectionId::generate();
    let first = mailbox(&ws.id, &a.id, &connection, "ada@example.com");
    store.create(&first).await.unwrap();
    store.delete(&ws.id, &first.id, now_ms()).await.unwrap();

    let fresh = mailbox(&ws.id, &a.id, &connection, "ada2@example.com");
    store.create(&fresh).await.unwrap();

    assert_eq!(store.for_agent(&ws.id, &a.id).await.unwrap(), Some(fresh));
}

pub async fn the_ledger_answers_for_every_address_ever(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    store
        .create(&mailbox(
            &ws.id,
            &a.id,
            &ConnectionId::generate(),
            "ada@example.com",
        ))
        .await
        .unwrap();

    assert!(store.address_taken("ada@example.com").await.unwrap());
    // The form types in any case; the ledger reads one shape.
    assert!(store.address_taken("Ada@Example.com").await.unwrap());
    assert!(!store.address_taken("grace@example.com").await.unwrap());
}

/// A host that refuses the create leaves the address free again, and
/// writes no tombstone (ADR-0019).
pub async fn a_dropped_reservation_frees_the_address(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let record = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");
    store.create(&record).await.unwrap();

    assert!(store.drop_reservation(&ws.id, &record.id).await.unwrap());

    assert!(!store.address_taken("ada@example.com").await.unwrap());
    assert_eq!(store.get(&ws.id, &record.id).await.unwrap(), None);
}

/// Only a mailbox the host never made goes away without a tombstone.
pub async fn a_mailbox_past_provisioning_keeps_its_row(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let record = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");
    store.create(&record).await.unwrap();
    store
        .set_state(&ws.id, &record.id, AgentMailboxState::Active, None)
        .await
        .unwrap();

    assert!(!store.drop_reservation(&ws.id, &record.id).await.unwrap());
    assert!(store.address_taken("ada@example.com").await.unwrap());
}

pub async fn a_state_carries_its_reason_and_a_later_one_clears_it(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let record = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");
    store.create(&record).await.unwrap();

    store
        .set_state(
            &ws.id,
            &record.id,
            AgentMailboxState::Unavailable,
            Some("refused"),
        )
        .await
        .unwrap();
    let unavailable = store.get(&ws.id, &record.id).await.unwrap().unwrap();
    assert_eq!(unavailable.state, AgentMailboxState::Unavailable);
    assert_eq!(unavailable.reason.as_deref(), Some("refused"));

    store
        .set_state(&ws.id, &record.id, AgentMailboxState::Active, None)
        .await
        .unwrap();
    let active = store.get(&ws.id, &record.id).await.unwrap().unwrap();
    assert_eq!(active.state, AgentMailboxState::Active);
    assert_eq!(active.reason, None);
}

/// A tombstone is written by a delete alone: nothing else may reach
/// `deleted`, and nothing changes after it (ADR-0019).
pub async fn a_tombstone_is_the_end_of_the_record(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let record = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");
    store.create(&record).await.unwrap();

    let refused = store
        .set_state(&ws.id, &record.id, AgentMailboxState::Deleted, None)
        .await;
    assert!(
        matches!(refused, Err(StoreError::Conflict(_))),
        "{refused:?}"
    );

    let at = now_ms();
    assert!(store.delete(&ws.id, &record.id, at).await.unwrap());
    let tombstone = store.get(&ws.id, &record.id).await.unwrap().unwrap();
    assert_eq!(tombstone.state, AgentMailboxState::Deleted);
    assert_eq!(tombstone.deleted_at, Some(at));
    assert_eq!(tombstone.address, "ada@example.com");
    assert_eq!(tombstone.agent_id, a.id);
    // A tombstone is out of every working read.
    assert_eq!(store.list(&ws.id).await.unwrap(), Vec::new());
    assert!(!store.delete(&ws.id, &record.id, now_ms()).await.unwrap());
    assert!(
        !store
            .set_state(&ws.id, &record.id, AgentMailboxState::Active, None)
            .await
            .unwrap()
    );
}

pub async fn archiving_the_agent_makes_its_mailbox_dormant(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let record = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");
    store.create(&record).await.unwrap();

    let dormant = store
        .make_dormant_for_agent(&ws.id, &a.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(dormant.state, AgentMailboxState::Dormant);
    // The mailbox stays with the Agent, unlike a phone number.
    let held = store.for_agent(&ws.id, &a.id).await.unwrap().unwrap();
    assert_eq!(held.state, AgentMailboxState::Dormant);
    assert_eq!(held.address, "ada@example.com");
}

pub async fn an_agent_with_no_mailbox_sleeps_alone(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;

    assert_eq!(
        store.make_dormant_for_agent(&ws.id, &a.id).await.unwrap(),
        None
    );
}

pub async fn the_cursor_is_kept_and_read_back(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let record = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");
    store.create(&record).await.unwrap();
    let cursor = MailboxCursor {
        folder: "INBOX".to_string(),
        uid_validity: 42,
        last_uid: 7,
    };

    assert!(store.set_cursor(&ws.id, &record.id, &cursor).await.unwrap());

    let kept = store.get(&ws.id, &record.id).await.unwrap().unwrap();
    assert_eq!(kept.cursor, Some(cursor));
}

pub async fn the_send_tally_counts_a_day_and_starts_again_on_the_next(backend: &Backend) {
    let (ws, a, _) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let record = mailbox(&ws.id, &a.id, &ConnectionId::generate(), "ada@example.com");
    store.create(&record).await.unwrap();
    let today = day_start(now_ms());
    let tomorrow = today + 24 * 60 * 60 * 1000;

    assert_eq!(store.note_send(&ws.id, &record.id, today).await.unwrap(), 1);
    assert_eq!(
        store
            .note_send(&ws.id, &record.id, today + 1000)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        store.note_send(&ws.id, &record.id, tomorrow).await.unwrap(),
        1
    );

    let counted = store.get(&ws.id, &record.id).await.unwrap().unwrap();
    assert_eq!(counted.sends_on(tomorrow), 1);
    // A tally from another day counts for nothing.
    assert_eq!(counted.sends_on(today), 0);
}

/// Removing a Mailbox Provider Connection reads this count (ADR-0019).
pub async fn only_live_mailboxes_hold_a_connection(backend: &Backend) {
    let (ws, a, b) = seed(backend).await;
    let store = &backend.stores().agent_mailboxes;
    let connection = ConnectionId::generate();
    let held = mailbox(&ws.id, &a.id, &connection, "ada@example.com");
    let deleted = mailbox(&ws.id, &b.id, &connection, "grace@example.com");
    store.create(&held).await.unwrap();
    store.create(&deleted).await.unwrap();

    assert_eq!(
        store
            .count_live_for_connection(&ws.id, &connection)
            .await
            .unwrap(),
        2
    );

    store.delete(&ws.id, &deleted.id, now_ms()).await.unwrap();
    assert_eq!(
        store
            .count_live_for_connection(&ws.id, &connection)
            .await
            .unwrap(),
        1
    );

    // A dormant mailbox still points at its Connection.
    store.make_dormant_for_agent(&ws.id, &a.id).await.unwrap();
    assert_eq!(
        store
            .count_live_for_connection(&ws.id, &connection)
            .await
            .unwrap(),
        1
    );

    store.delete(&ws.id, &held.id, now_ms()).await.unwrap();
    assert_eq!(
        store
            .count_live_for_connection(&ws.id, &connection)
            .await
            .unwrap(),
        0
    );
}

/// A Workspace has one mailbox, with an address in the shared ledger.
pub async fn workspace_mailbox_record_and_ledger(backend: &Backend) {
    let (ws, agent, _) = seed(backend).await;
    let store = &backend.stores().workspace_mailboxes;
    let record = pagis_core::WorkspaceMailbox {
        workspace_id: ws.id.clone(),
        connection_id: ConnectionId::generate(),
        address: "person@example.com".into(),
        spam_mode: pagis_core::MailScoringMode::ScoreOnly,
        outgoing_cap: 20,
        created_at: now_ms(),
    };
    assert!(store.get(&ws.id).await.unwrap().is_none());
    store.create(&record).await.unwrap();
    assert_eq!(store.get(&ws.id).await.unwrap(), Some(record.clone()));
    for _ in 0..20 {
        assert!(store.reserve_send(&ws.id, 1).await.unwrap());
    }
    assert!(!store.reserve_send(&ws.id, 1).await.unwrap());
    store.release_send(&ws.id, 1).await.unwrap();
    assert!(store.reserve_send(&ws.id, 1).await.unwrap());
    assert!(store.reserve_send(&ws.id, 2).await.unwrap());
    let other = backend.seeded_workspace().await;
    assert!(store.get(&other.id).await.unwrap().is_none());
    let second = pagis_core::WorkspaceMailbox {
        address: "second@example.com".into(),
        ..record.clone()
    };
    assert!(matches!(
        store.create(&second).await,
        Err(StoreError::Conflict(_))
    ));
    assert!(
        backend
            .stores()
            .agent_mailboxes
            .address_taken(&record.address)
            .await
            .unwrap()
    );
    let agent_record = mailbox(&ws.id, &agent.id, &record.connection_id, &record.address);
    assert!(matches!(
        backend.stores().agent_mailboxes.create(&agent_record).await,
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        store
            .create(&pagis_core::WorkspaceMailbox {
                workspace_id: other.id.clone(),
                ..record.clone()
            })
            .await,
        Err(StoreError::Conflict(_))
    ));
    assert!(
        !store
            .drop_reservation(&other.id, &record.address)
            .await
            .unwrap()
    );
    assert!(
        store
            .drop_reservation(&ws.id, &record.address)
            .await
            .unwrap()
    );
    backend
        .stores()
        .agent_mailboxes
        .create(&agent_record)
        .await
        .unwrap();
    assert!(matches!(
        store.create(&record).await,
        Err(StoreError::Conflict(_))
    ));
}

/// Verdict and Trust Tier are one record, isolated by Workspace and mailbox.
pub async fn mail_assessment_roundtrip(backend: &Backend) {
    let ws = backend.seeded_workspace().await;
    let other = backend.seeded_workspace().await;
    let store = &backend.stores().mail_assessments;
    let assessment = pagis_core::MailAssessment {
        verdict: pagis_core::SpamVerdict {
            score: 7200,
            action: pagis_core::SpamAction::AddHeader,
            reasons: vec!["GTUBE".into()],
            source: "mail_host".into(),
        },
        trust_tier: pagis_core::TrustTier::Unknown,
    };
    assert!(
        store
            .get(&ws.id, "one@example.com", "<stable@example.com>")
            .await
            .unwrap()
            .is_none()
    );
    store
        .put(
            &ws.id,
            "one@example.com",
            "<stable@example.com>",
            &assessment,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .get(&ws.id, "one@example.com", "<stable@example.com>")
            .await
            .unwrap(),
        Some(assessment.clone())
    );
    assert!(
        store
            .get(&other.id, "one@example.com", "<stable@example.com>")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get(&ws.id, "two@example.com", "<stable@example.com>")
            .await
            .unwrap()
            .is_none()
    );
    let changed = pagis_core::MailAssessment {
        trust_tier: pagis_core::TrustTier::Trusted,
        ..assessment
    };
    store
        .put(&ws.id, "one@example.com", "<stable@example.com>", &changed)
        .await
        .unwrap();
    assert_eq!(
        store
            .get(&ws.id, "one@example.com", "<stable@example.com>")
            .await
            .unwrap(),
        Some(changed)
    );
}

/// A host move and UIDVALIDITY change keep the stored Spam Verdict.
pub async fn mail_assessment_survives_a_folder_move(backend: &Backend) {
    use pagis_mail::fake::FakeMailTransport;
    use pagis_mail::{
        Endpoint, ImapWorkspaceMailStore, MailTransport, MailboxCredential, PageRequest,
        WorkspaceMailReader, WorkspaceMailWriter,
    };
    use std::sync::Arc;
    let ws = backend.seeded_workspace().await;
    let stores = backend.stores();
    let transport = FakeMailTransport::default();
    let id = transport.deliver_raw(b"Message-ID: <stable@example.test>\r\nFrom: sender@example.test\r\nX-Spam-Score: 7.2\r\n\r\nbody");
    let credential = MailboxCredential::new(
        "person@example.com",
        "password",
        Endpoint::new("imap.test", 993),
        Endpoint::new("smtp.test", 465),
    );
    let context = pagis_mail::MailReadContext {
        intake: Arc::new(pagis_mail::MailIntake {
            workspaces: stores.workspaces.clone(),
            agents: stores.agents.clone(),
            records: stores.mail_assessments.clone(),
            source: Arc::new(pagis_mail::HostSpamVerdicts),
            trust: Arc::new(pagis_mail::ListedSenders::new(
                stores.trust_list.clone(),
                stores.connections.clone(),
            )),
        }),
        workspace: ws.id.clone(),
        address: "person@example.com".into(),
        agent: None,
        authserv_ids: Vec::new(),
    };
    let records = context.intake.records.clone();
    let workspace = context.workspace.clone();
    let address = context.address.clone();
    let mut store =
        ImapWorkspaceMailStore::new(transport.login(&credential).await.unwrap(), context);
    let original = store.message(&id).await.unwrap().message.summary;
    let key = original.message_id.as_str();
    let saved = records.get(&workspace, &address, key).await.unwrap();
    assert_eq!(saved, original.assessment);
    assert!(
        records
            .get(&workspace, &address, id.as_str())
            .await
            .unwrap()
            .is_none()
    );
    store.create_folder("Filed").await.unwrap();
    store.move_message(&id, "Filed").await.unwrap();
    transport.renumber("Filed");
    let moved = store
        .messages("Filed", "", &PageRequest::new(None, 20))
        .await
        .unwrap()
        .items
        .remove(0)
        .summary;
    assert_ne!(moved.id, id);
    assert_eq!(moved.assessment, saved);
    assert_eq!(
        records
            .get(&workspace, &address, &moved.message_id)
            .await
            .unwrap(),
        saved
    );
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_mailboxes {
    ($emit:path) => {
        $emit!(
            mailboxes,
            workspace_mailbox_record_and_ledger,
            mail_assessment_roundtrip,
            mail_assessment_survives_a_folder_move,
            allow_rules_are_added_once_and_keep_their_order,
            a_deleted_mailbox_takes_no_allow_rule,
            a_mailbox_roundtrips_through_the_store,
            the_schema_refuses_a_second_live_mailbox_for_one_agent,
            one_address_is_never_held_on_two_connections,
            a_deleted_address_is_never_given_out_again,
            an_agent_gets_a_fresh_mailbox_after_a_delete,
            the_ledger_answers_for_every_address_ever,
            a_dropped_reservation_frees_the_address,
            a_mailbox_past_provisioning_keeps_its_row,
            a_state_carries_its_reason_and_a_later_one_clears_it,
            a_tombstone_is_the_end_of_the_record,
            archiving_the_agent_makes_its_mailbox_dormant,
            an_agent_with_no_mailbox_sleeps_alone,
            the_cursor_is_kept_and_read_back,
            the_send_tally_counts_a_day_and_starts_again_on_the_next,
            only_live_mailboxes_hold_a_connection,
        );
    };
}
