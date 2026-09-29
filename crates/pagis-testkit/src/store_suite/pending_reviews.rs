//! The Pending Evidence trait tests of the suite.
//!
//! Each body is one test. It takes a [`Backend`], reads the store set
//! from it, and never names a pool type, so it runs on both backends
//! from one text. Name every new body in `store_suite_pending_reviews!`
//! below: the guard test of the parent module fails while one is
//! missing.

use pagis_core::{
    AgentId, ChannelId, MessageId, PendingEvidence, PendingUrgency, Workspace, WorkspaceId,
};

use super::{Backend, Bind};

/// The one Agent the seed writes.
fn agent() -> AgentId {
    AgentId::from("a".to_string())
}

/// A Workspace with one Agent, two conversations and five messages in
/// it. No trait writes a message, so the rows go in directly.
async fn seeded(backend: &Backend) -> Workspace {
    let ws = backend.seeded_workspace().await;
    let id = ws.id.as_str();
    for (sql, binds) in [
        (
            "INSERT INTO model_aliases(id,workspace_id,alias,candidates,created_at,updated_at) \
             VALUES('ma',?,'default','[]',1,1)",
            vec![Bind::from(id)],
        ),
        (
            "INSERT INTO agents(id,workspace_id,name,job,personality,model_alias,status,\
             created_at,updated_at) \
             VALUES('a',?,'A','Help','Plain','default','active',1,1)",
            vec![Bind::from(id)],
        ),
        (
            "INSERT INTO channels(id,workspace_id,kind,title,created_at,updated_at) \
             VALUES('c',?,'dm',NULL,1,1)",
            vec![Bind::from(id)],
        ),
        (
            "INSERT INTO channels(id,workspace_id,kind,title,created_at,updated_at) \
             VALUES('c2',?,'dm',NULL,1,1)",
            vec![Bind::from(id)],
        ),
        (
            "INSERT INTO messages(id,workspace_id,channel_id,author_kind,status,blocks,\
             text_content,created_at) \
             VALUES('00',?,'c2','user','complete','[]','older other conversation',1)",
            vec![Bind::from(id)],
        ),
        (
            "INSERT INTO messages(id,workspace_id,channel_id,author_kind,status,blocks,\
             text_content,created_at) VALUES('01',?,'c','user','complete','[]','one',1)",
            vec![Bind::from(id)],
        ),
        (
            "INSERT INTO messages(id,workspace_id,channel_id,author_kind,status,blocks,\
             text_content,created_at) VALUES('02',?,'c','user','complete','[]','two',2)",
            vec![Bind::from(id)],
        ),
        (
            "INSERT INTO messages(id,workspace_id,channel_id,author_kind,status,blocks,\
             text_content,created_at) VALUES('03',?,'c','user','complete','[]','three',3)",
            vec![Bind::from(id)],
        ),
        (
            "INSERT INTO messages(id,workspace_id,channel_id,author_kind,status,blocks,\
             text_content,created_at) VALUES('04',?,'c','user','complete','[]','four',4)",
            vec![Bind::from(id)],
        ),
    ] {
        backend
            .execute(sql, &binds)
            .await
            .expect("plant the seed rows");
    }
    ws
}

fn evidence(workspace_id: &WorkspaceId, subject: &str, through: &str, now: i64) -> PendingEvidence {
    evidence_in(workspace_id, "c", subject, through, now)
}

fn evidence_in(
    workspace_id: &WorkspaceId,
    channel: &str,
    subject: &str,
    through: &str,
    now: i64,
) -> PendingEvidence {
    PendingEvidence {
        workspace_id: workspace_id.clone(),
        agent_id: agent(),
        channel_id: ChannelId::from(channel.to_string()),
        root_message_id: None,
        subject: subject.to_string(),
        after_exclusive: None,
        through_inclusive: MessageId::from(through.to_string()),
        source_message_ids: vec![MessageId::from(through.to_string())],
        exposures: Vec::new(),
        reason: format!("review {subject}"),
        urgency: PendingUrgency::Normal,
        created_at: now,
    }
}

pub async fn urgent_work_claims_an_arrival_slot_before_due_normal_work(backend: &Backend) {
    let ws = seeded(backend).await;
    let store = &backend.stores().pending_evidence;
    store
        .record(evidence(&ws.id, "normal", "01", 1_000))
        .await
        .unwrap()
        .unwrap();
    let mut urgent = evidence(&ws.id, "urgent", "02", 2_000);
    urgent.urgency = PendingUrgency::Urgent;
    store.record(urgent).await.unwrap().unwrap();

    let claims = store.claim(&ws.id, &agent(), 1, 302_000).await.unwrap();

    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].evidence.subject, "urgent");
    assert_eq!(claims[0].run.trigger_kind, pagis_core::TriggerKind::Review);
    assert_eq!(claims[0].run.channel_id.as_ref().unwrap().as_str(), "c");
}

pub async fn evidence_arriving_during_a_lease_remains_pending_after_the_claim_completes(
    backend: &Backend,
) {
    let ws = seeded(backend).await;
    let store = &backend.stores().pending_evidence;
    store
        .record(evidence(&ws.id, "trip", "01", 1_000))
        .await
        .unwrap()
        .unwrap();
    let claim = store
        .claim(&ws.id, &agent(), 1, 301_000)
        .await
        .unwrap()
        .remove(0);

    let later = store
        .record(evidence(&ws.id, "trip", "03", 302_000))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(later.id, claim.evidence.id);
    assert!(
        store
            .complete(
                &ws.id,
                &claim.run.id,
                claim.lease_revision,
                Some("git-sha"),
                303_000
            )
            .await
            .unwrap()
    );

    let still_pending = store
        .for_run(&ws.id, &claim.run.id)
        .await
        .unwrap()
        .expect("completed claim stays auditable");
    assert_eq!(
        still_pending.state,
        pagis_core::PendingEvidenceState::Completed
    );
    assert_eq!(later.state, pagis_core::PendingEvidenceState::Pending);
    assert_eq!(later.through_inclusive.as_str(), "03");
}

pub async fn compaction_settles_only_pending_work_inside_its_fixed_origin_range(backend: &Backend) {
    let ws = seeded(backend).await;
    let store = &backend.stores().pending_evidence;
    let covered_record = store
        .record(evidence(&ws.id, "trip", "02", 1_000))
        .await
        .unwrap()
        .unwrap();
    let newer = store
        .record(evidence(&ws.id, "plans", "04", 2_000))
        .await
        .unwrap()
        .unwrap();

    let compaction = evidence(&ws.id, "conversation_compaction", "03", 3_000);
    store
        .settle_compaction(&compaction, Some("memory-sha"), 4_000)
        .await
        .unwrap();

    let claims = store
        .claim(&ws.id, &agent(), 2, i64::MAX / 2)
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].evidence.id, newer.id);
    assert_ne!(claims[0].evidence.id, covered_record.id);
    assert_eq!(
        store
            .review_cursor(
                &ws.id,
                &agent(),
                &ChannelId::from("c".to_string()),
                None,
                "trip",
            )
            .await
            .unwrap()
            .unwrap()
            .as_str(),
        "02"
    );
    assert_eq!(
        store
            .review_cursor(
                &ws.id,
                &agent(),
                &ChannelId::from("c".to_string()),
                None,
                "conversation_compaction",
            )
            .await
            .unwrap()
            .unwrap()
            .as_str(),
        "03"
    );
}

pub async fn failures_retry_after_one_and_five_minutes_then_stay_failed(backend: &Backend) {
    let ws = seeded(backend).await;
    let store = &backend.stores().pending_evidence;
    store
        .record(evidence(&ws.id, "trip", "01", 1_000))
        .await
        .unwrap()
        .unwrap();
    let agent = agent();

    let first = store
        .claim(&ws.id, &agent, 1, 301_000)
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .fail(&ws.id, &first.run.id, first.lease_revision, "one", 302_000)
            .await
            .unwrap()
    );
    assert!(
        store
            .claim(&ws.id, &agent, 1, 361_999)
            .await
            .unwrap()
            .is_empty()
    );
    let second = store
        .claim(&ws.id, &agent, 1, 362_000)
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .fail(
                &ws.id,
                &second.run.id,
                second.lease_revision,
                "two",
                363_000
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .claim(&ws.id, &agent, 1, 662_999)
            .await
            .unwrap()
            .is_empty()
    );
    let third = store
        .claim(&ws.id, &agent, 1, 663_000)
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .fail(
                &ws.id,
                &third.run.id,
                third.lease_revision,
                "three",
                664_000
            )
            .await
            .unwrap()
    );

    assert!(
        store
            .claim(&ws.id, &agent, 1, 2_000_000)
            .await
            .unwrap()
            .is_empty()
    );
    let failed = store.for_run(&ws.id, &third.run.id).await.unwrap().unwrap();
    assert_eq!(failed.state, pagis_core::PendingEvidenceState::Failed);
    assert_eq!(failed.attempt_count, 3);
    assert_eq!(failed.error.as_deref(), Some("three"));
}

pub async fn an_expired_terminal_lease_is_retried_without_losing_evidence(backend: &Backend) {
    let ws = seeded(backend).await;
    let store = &backend.stores().pending_evidence;
    store
        .record(evidence(&ws.id, "trip", "01", 1_000))
        .await
        .unwrap()
        .unwrap();
    let agent = agent();
    let claim = store
        .claim(&ws.id, &agent, 1, 301_000)
        .await
        .unwrap()
        .remove(0);
    let expiry = 301_000 + 10 * 60_000;
    backend
        .execute(
            "UPDATE runs SET state='failed',ended_at=? WHERE id=?",
            &[Bind::from(expiry), Bind::from(claim.run.id.as_str())],
        )
        .await
        .expect("end the lease");

    assert!(
        store
            .claim(&ws.id, &agent, 1, expiry)
            .await
            .unwrap()
            .is_empty()
    );
    let retry = store
        .claim(&ws.id, &agent, 1, expiry + 60_000)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        retry.evidence.source_message_ids,
        claim.evidence.source_message_ids
    );
    assert_eq!(retry.evidence.attempt_count, 2);
}

pub async fn same_subject_evidence_from_two_conversations_shares_one_pending_review(
    backend: &Backend,
) {
    let ws = seeded(backend).await;
    let store = &backend.stores().pending_evidence;
    let first = store
        .record(evidence(&ws.id, "trip", "01", 1_000))
        .await
        .unwrap()
        .unwrap();
    let second = store
        .record(evidence_in(&ws.id, "c2", "trip", "00", 2_000))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(second.id, first.id);
    assert_eq!(second.source_message_ids.len(), 2);
}

pub async fn a_newer_conversation_cursor_does_not_hide_older_unseen_evidence_elsewhere(
    backend: &Backend,
) {
    let ws = seeded(backend).await;
    let store = &backend.stores().pending_evidence;
    store
        .record(evidence(&ws.id, "trip", "03", 1_000))
        .await
        .unwrap()
        .unwrap();
    let claim = store
        .claim(&ws.id, &agent(), 1, 301_000)
        .await
        .unwrap()
        .remove(0);
    assert!(
        store
            .complete(
                &ws.id,
                &claim.run.id,
                claim.lease_revision,
                Some("git-sha"),
                302_000
            )
            .await
            .unwrap()
    );

    assert!(
        store
            .record(evidence(&ws.id, "trip", "03", 303_000))
            .await
            .unwrap()
            .is_none(),
        "the completed origin must not queue the same range twice"
    );
    let unseen = store
        .record(evidence_in(&ws.id, "c2", "trip", "00", 303_000))
        .await
        .unwrap()
        .expect("another origin's older lexical id is still new evidence");
    assert_eq!(unseen.through_inclusive.as_str(), "00");
}

pub async fn same_subject_evidence_debounces_without_moving_the_maximum_due_time(
    backend: &Backend,
) {
    let ws = seeded(backend).await;
    let store = &backend.stores().pending_evidence;

    let first = store
        .record(evidence(&ws.id, "trip", "01", 1_000))
        .await
        .unwrap()
        .unwrap();
    let second = store
        .record(evidence(&ws.id, "trip", "02", 61_000))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(first.eligible_at, 301_000);
    assert_eq!(first.maximum_due_at, 1_801_000);
    assert_eq!(second.id, first.id);
    assert_eq!(second.eligible_at, 361_000);
    assert_eq!(second.maximum_due_at, 1_801_000);
    assert_eq!(second.through_inclusive.as_str(), "02");
    assert_eq!(second.source_message_ids.len(), 2);
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_pending_reviews {
    ($emit:path) => {
        $emit!(
            pending_reviews,
            urgent_work_claims_an_arrival_slot_before_due_normal_work,
            evidence_arriving_during_a_lease_remains_pending_after_the_claim_completes,
            compaction_settles_only_pending_work_inside_its_fixed_origin_range,
            failures_retry_after_one_and_five_minutes_then_stay_failed,
            an_expired_terminal_lease_is_retried_without_losing_evidence,
            same_subject_evidence_from_two_conversations_shares_one_pending_review,
            a_newer_conversation_cursor_does_not_hide_older_unseen_evidence_elsewhere,
            same_subject_evidence_debounces_without_moving_the_maximum_due_time,
        );
    };
}
