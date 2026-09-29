//! The Continuation Checkpoint trait tests of the suite.
//!
//! Each body is one test. It takes a [`Backend`], reads the store set
//! from it, and never names a pool type, so it runs on both backends
//! from one text. Name every new body in `store_suite_continuation!`
//! below: the guard test of the parent module fails while one is
//! missing.

use pagis_core::{
    ContinuationCheckpoint, ContinuationKey, ContinuationState, Grant, GrantId, MemoryExposure,
    MessageId,
};

use crate::fixture::{agent, channel, user_message};

use super::{Backend, Bind};

pub async fn replacement_is_atomic_compare_and_swap_and_survives_restart(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let agent = agent(&workspace.id);
    let channel = channel(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    backend.stores().channels.create(&channel).await.unwrap();
    let messages = &backend.stores().messages;
    for id in [
        "01J00000000000000000000001",
        "01J00000000000000000000020",
        "01J00000000000000000000040",
    ] {
        let mut message = user_message(&workspace.id, &channel.id, id);
        message.id = MessageId::from(id.to_string());
        messages.insert(&message).await.unwrap();
    }
    let grant = Grant {
        id: GrantId::from("grant-1".to_string()),
        workspace_id: workspace.id.clone(),
        agent_id: agent.id.clone(),
        resource_kind: Grant::CONNECTION_KIND.to_string(),
        resource_id: None,
        scope: serde_json::json!({}),
        revision: 3,
        created_at: 1,
        revoked_at: None,
    };
    backend.stores().grants.create(&grant).await.unwrap();

    let key = ContinuationKey {
        workspace_id: workspace.id,
        agent_id: agent.id,
        channel_id: channel.id,
        root_message_id: None,
    };
    let first = ContinuationCheckpoint {
        key: key.clone(),
        revision: 1,
        after_exclusive: None,
        through_inclusive: MessageId::from("01J00000000000000000000020".to_string()),
        state: ContinuationState {
            active_goal: vec!["Ship the compacting request path".into()],
            constraints: vec!["Do not send an oversized request".into()],
            corrections: vec![],
            accepted_decisions: vec![],
            proposals: vec![],
            completed_work: vec![],
            failed_attempts: vec![],
            open_questions: vec![],
            next_steps: vec!["Run the stored scenario".into()],
            references: vec!["message:01J00000000000000000000001".into()],
        },
        source_message_ids: vec![
            MessageId::from("01J00000000000000000000001".to_string()),
            MessageId::from("01J00000000000000000000020".to_string()),
        ],
        exposures: vec![MemoryExposure {
            grant_id: GrantId::from("grant-1".to_string()),
            revision: 3,
        }],
        created_at: 100,
    };
    let store = &backend.stores().continuations;
    assert!(store.replace(&first, None).await.unwrap());

    let mut winner = first.clone();
    winner.revision = 2;
    winner.after_exclusive = Some(first.through_inclusive.clone());
    winner.through_inclusive = MessageId::from("01J00000000000000000000040".to_string());
    winner
        .source_message_ids
        .push(MessageId::from("01J00000000000000000000040".to_string()));
    winner.created_at = 200;
    assert!(store.replace(&winner, Some(1)).await.unwrap());

    let mut stale = winner.clone();
    stale.state.active_goal = vec!["Stale writer".into()];
    assert!(!store.replace(&stale, Some(1)).await.unwrap());

    // The store holds a pool and no state of its own, so a read here is
    // the read a restarted daemon makes: the checkpoint comes back from
    // the rows, not from memory.
    let loaded = store.get(&key).await.unwrap().unwrap();
    assert_eq!(loaded, winner);

    backend
        .execute(
            "INSERT INTO forgotten_messages(message_id) VALUES (?)",
            &[Bind::from("01J00000000000000000000001")],
        )
        .await
        .expect("mark the source message forgotten");
    assert_eq!(store.get(&key).await.unwrap(), None);
    let mut invalid = winner.clone();
    invalid.revision = 1;
    assert!(store.replace(&invalid, None).await.is_err());

    backend
        .execute(
            "DELETE FROM forgotten_messages WHERE message_id = ?",
            &[Bind::from("01J00000000000000000000001")],
        )
        .await
        .expect("take the message back out of the forgotten set");
    let mut recreated = winner.clone();
    recreated.revision = 1;
    assert!(store.replace(&recreated, None).await.unwrap());
    backend
        .execute(
            "UPDATE grants SET revision = revision + 1 WHERE id = ?",
            &[Bind::from("grant-1")],
        )
        .await
        .expect("move the Grant revision on");
    assert_eq!(store.get(&key).await.unwrap(), None);
    recreated.exposures[0].revision = 3;
    assert!(store.replace(&recreated, None).await.is_err());
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_continuation {
    ($emit:path) => {
        $emit!(
            continuation,
            replacement_is_atomic_compare_and_swap_and_survives_restart,
        );
    };
}
