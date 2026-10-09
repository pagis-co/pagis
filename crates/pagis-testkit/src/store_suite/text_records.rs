//! The Text Record and the texting fields of an Agent Phone Number
//! trait tests of the suite (ADR-0020).
//!
//! There is no conversation table: these bodies hold the store to the
//! Text Conversation it derives, to the newest-first page the tools
//! read, and to the seven-day landing rule that puts an inbound text
//! in the Thread the last outbound one was sent from.
//!
//! Each body is one test. It takes a [`Backend`], reads the store set
//! from it, and never names a pool type, so it runs on both backends
//! from one text. Name every new body in `store_suite_text_records!`
//! below: the guard test of the parent module fails while one is
//! missing.

use pagis_core::{
    Agent, AgentId, ArtifactId, ChannelId, ConnectionId, MessageId, MessagingReadiness,
    PhoneNumber, PhoneNumberId, Run, RunId, RunState, TEXT_THREAD_WINDOW_MS, TelnyxRelayState,
    TextDeliveryStatus, TextDirection, TextRecord, TextRecordId, TriggerKind, TrustTier,
    UnixMillis, Workspace, WorkspaceId, day_start, now_ms,
};

use super::Backend;
use crate::fixture::agent;

/// One Workspace, two Agents and one Run: a text needs a holder, and
/// an outbound record needs the Run that sent it.
async fn seed(backend: &Backend) -> (Workspace, Agent, Agent, RunId) {
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
    let run = Run {
        title: "A message with an attachment".into(),
        id: RunId::generate(),
        workspace_id: ws.id.clone(),
        agent_id: first.id.clone(),
        channel_id: None,
        root_message_id: None,
        trigger_kind: TriggerKind::Message,
        trigger_ref: None,
        hop_count: 0,
        state: RunState::Queued,
        origin: None,
        failure_kind: None,
        dismissed_at: None,
        error: None,
        created_at: now_ms(),
        started_at: None,
        ended_at: None,
    };
    backend.stores().runs.create(&run).await.unwrap();
    (ws, first, second, run.id)
}

/// One number of the Workspace, held by the Agent.
fn number(workspace_id: &WorkspaceId, agent_id: &AgentId) -> PhoneNumber {
    PhoneNumber::new(
        PhoneNumberId::generate(),
        workspace_id.clone(),
        ConnectionId::generate(),
        "+14155550123".to_string(),
        "carrier-14155550123".to_string(),
        Some(agent_id.clone()),
        now_ms(),
    )
}

/// One inbound text of the pair.
fn inbound(
    ws: &Workspace,
    agent_id: &AgentId,
    number_id: &PhoneNumberId,
    counterpart: &str,
    body: &str,
    at: UnixMillis,
) -> TextRecord {
    TextRecord {
        id: TextRecordId::generate(),
        workspace_id: ws.id.clone(),
        agent_id: agent_id.clone(),
        phone_number_id: number_id.clone(),
        counterpart_e164: counterpart.to_string(),
        direction: TextDirection::Inbound,
        tier: TrustTier::Unknown,
        body: body.to_string(),
        segments: 1,
        media_artifact_ids: Vec::new(),
        carrier_message_id: format!("carrier-{body}"),
        run_id: None,
        channel_id: None,
        thread_id: None,
        delivery_status: None,
        occurred_at: at,
    }
}

/// One outbound text of the pair, sent from `thread`.
#[allow(clippy::too_many_arguments)]
fn outbound(
    ws: &Workspace,
    agent_id: &AgentId,
    number_id: &PhoneNumberId,
    run_id: &RunId,
    counterpart: &str,
    body: &str,
    thread: Option<MessageId>,
    at: UnixMillis,
) -> TextRecord {
    TextRecord {
        direction: TextDirection::Outbound,
        run_id: Some(run_id.clone()),
        thread_id: thread,
        delivery_status: Some(TextDeliveryStatus::Queued),
        ..inbound(ws, agent_id, number_id, counterpart, body, at)
    }
}

/// Pagis keeps the body, the media and the Thread, because no carrier
/// is a mailbox to fetch from later (ADR-0020).
pub async fn an_outbound_record_round_trips_with_its_thread_and_media(backend: &Backend) {
    let (ws, a, _, run_id) = seed(backend).await;
    let line = number(&ws.id, &a.id);
    backend.stores().phone_numbers.create(&line).await.unwrap();
    let store = &backend.stores().text_records;

    let thread = MessageId::generate();
    let channel = ChannelId::generate();
    let media = ArtifactId::generate();
    let record = TextRecord {
        channel_id: Some(channel.clone()),
        media_artifact_ids: vec![media.clone()],
        segments: 3,
        tier: TrustTier::Owner,
        ..outbound(
            &ws,
            &a.id,
            &line.id,
            &run_id,
            "+14155550999",
            "on my way",
            Some(thread.clone()),
            1_000,
        )
    };
    store.insert(&record).await.unwrap();

    let read = store.get(&ws.id, &record.id).await.unwrap().unwrap();
    assert_eq!(read, record);
    assert_eq!(read.thread_id, Some(thread));
    assert_eq!(read.channel_id, Some(channel));
    assert_eq!(read.media_artifact_ids, [media]);
    assert_eq!(read.segments, 3);
    assert_eq!(read.tier, TrustTier::Owner);
}

/// An inbound record carries no Run, no Thread and no delivery state.
pub async fn an_inbound_record_round_trips_without_a_run(backend: &Backend) {
    let (ws, a, _, _) = seed(backend).await;
    let store = &backend.stores().text_records;
    let record = inbound(
        &ws,
        &a.id,
        &PhoneNumberId::generate(),
        "+14155550999",
        "are you there",
        7,
    );
    store.insert(&record).await.unwrap();

    let read = store.get(&ws.id, &record.id).await.unwrap().unwrap();
    assert_eq!(read, record);
    assert_eq!(read.run_id, None);
    assert_eq!(read.delivery_status, None);
}

/// The daemon polls the carrier and writes what it hears (ADR-0020).
pub async fn the_delivery_status_of_an_outbound_record_is_written(backend: &Backend) {
    let (ws, a, _, run_id) = seed(backend).await;
    let store = &backend.stores().text_records;
    let line_id = PhoneNumberId::generate();
    let record = outbound(
        &ws,
        &a.id,
        &line_id,
        &run_id,
        "+14155550999",
        "hello",
        None,
        5,
    );
    store.insert(&record).await.unwrap();

    assert!(
        store
            .set_delivery_status(&ws.id, &record.id, &TextDeliveryStatus::Delivered)
            .await
            .unwrap()
    );
    let read = store.get(&ws.id, &record.id).await.unwrap().unwrap();
    assert_eq!(read.delivery_status, Some(TextDeliveryStatus::Delivered));

    let failed = TextDeliveryStatus::Failed {
        code: Some("30007".to_string()),
        reason: Some("the carrier filtered the message".to_string()),
    };
    assert!(
        store
            .set_delivery_status(&ws.id, &record.id, &failed)
            .await
            .unwrap()
    );
    let read = store.get(&ws.id, &record.id).await.unwrap().unwrap();
    assert_eq!(read.delivery_status, Some(failed));

    // An inbound record has no delivery state and takes none.
    let arrived = inbound(&ws, &a.id, &line_id, "+14155550999", "hi", 6);
    store.insert(&arrived).await.unwrap();
    assert!(
        !store
            .set_delivery_status(&ws.id, &arrived.id, &TextDeliveryStatus::Sent)
            .await
            .unwrap()
    );
    assert!(
        !store
            .set_delivery_status(&ws.id, &TextRecordId::generate(), &TextDeliveryStatus::Sent)
            .await
            .unwrap()
    );
}

/// The conversation reads newest first, one page at a time
/// (ADR-0020: `max` is at most 50 and `before` names the page).
pub async fn a_conversation_pages_newest_first(backend: &Backend) {
    let (ws, a, _, run_id) = seed(backend).await;
    let store = &backend.stores().text_records;
    let line_id = PhoneNumberId::generate();
    let counterpart = "+14155550999";
    for step in 1..=5u32 {
        let record = match step % 2 {
            0 => inbound(
                &ws,
                &a.id,
                &line_id,
                counterpart,
                &format!("text {step}"),
                i64::from(step) * 1_000,
            ),
            _ => outbound(
                &ws,
                &a.id,
                &line_id,
                &run_id,
                counterpart,
                &format!("text {step}"),
                None,
                i64::from(step) * 1_000,
            ),
        };
        store.insert(&record).await.unwrap();
    }
    // Another counterpart of the same number stays out of the page.
    store
        .insert(&inbound(
            &ws,
            &a.id,
            &line_id,
            "+14155550888",
            "elsewhere",
            4_500,
        ))
        .await
        .unwrap();

    let first = store
        .list_conversation(&ws.id, &a.id, &line_id, counterpart, 2, None)
        .await
        .unwrap();
    let bodies: Vec<&str> = first.iter().map(|r| r.body.as_str()).collect();
    assert_eq!(bodies, ["text 5", "text 4"]);

    let next = store
        .list_conversation(&ws.id, &a.id, &line_id, counterpart, 2, Some(&first[1].id))
        .await
        .unwrap();
    let bodies: Vec<&str> = next.iter().map(|r| r.body.as_str()).collect();
    assert_eq!(bodies, ["text 3", "text 2"]);

    let last = store
        .list_conversation(&ws.id, &a.id, &line_id, counterpart, 2, Some(&next[1].id))
        .await
        .unwrap();
    let bodies: Vec<&str> = last.iter().map(|r| r.body.as_str()).collect();
    assert_eq!(bodies, ["text 1"]);
}

/// The desk lists conversations by grouping records, and each one
/// carries the time and the direction of its last text (ADR-0020).
pub async fn conversations_group_by_counterpart_with_the_last_text(backend: &Backend) {
    let (ws, a, _, run_id) = seed(backend).await;
    let store = &backend.stores().text_records;
    let line_id = PhoneNumberId::generate();

    store
        .insert(&inbound(&ws, &a.id, &line_id, "+14155550111", "first", 100))
        .await
        .unwrap();
    store
        .insert(&outbound(
            &ws,
            &a.id,
            &line_id,
            &run_id,
            "+14155550111",
            "answer",
            None,
            200,
        ))
        .await
        .unwrap();
    store
        .insert(&inbound(&ws, &a.id, &line_id, "+14155550222", "later", 300))
        .await
        .unwrap();
    store
        .insert(&inbound(&ws, &a.id, &line_id, "+14155550333", "oldest", 50))
        .await
        .unwrap();

    let conversations = store
        .list_conversations(&ws.id, &a.id, &line_id, 25)
        .await
        .unwrap();
    let seen: Vec<(&str, i64, TextDirection)> = conversations
        .iter()
        .map(|c| (c.counterpart_e164.as_str(), c.last_at, c.last_direction))
        .collect();
    assert_eq!(
        seen,
        [
            ("+14155550222", 300, TextDirection::Inbound),
            ("+14155550111", 200, TextDirection::Outbound),
            ("+14155550333", 50, TextDirection::Inbound),
        ]
    );

    let capped = store
        .list_conversations(&ws.id, &a.id, &line_id, 1)
        .await
        .unwrap();
    assert_eq!(capped.len(), 1);
    assert_eq!(capped[0].counterpart_e164, "+14155550222");
}

/// An inbound text lands in the Thread of the last outbound text of
/// the pair that is under seven days old (ADR-0020). These are the
/// two sides of that boundary.
pub async fn the_landing_thread_stops_at_seven_days(backend: &Backend) {
    let (ws, a, _, run_id) = seed(backend).await;
    let store = &backend.stores().text_records;
    let line_id = PhoneNumberId::generate();
    let counterpart = "+14155550999";
    let now = 100 * TEXT_THREAD_WINDOW_MS;

    let fresh_thread = MessageId::generate();
    store
        .insert(&outbound(
            &ws,
            &a.id,
            &line_id,
            &run_id,
            counterpart,
            "just inside",
            Some(fresh_thread.clone()),
            now - TEXT_THREAD_WINDOW_MS + 1,
        ))
        .await
        .unwrap();
    assert_eq!(
        store
            .landing_thread(&ws.id, &a.id, &line_id, counterpart, now)
            .await
            .unwrap(),
        Some(fresh_thread)
    );

    // One millisecond older is seven days old, and lands nowhere.
    assert_eq!(
        store
            .landing_thread(&ws.id, &a.id, &line_id, counterpart, now + 1)
            .await
            .unwrap(),
        None
    );
}

/// The landing reads the last outbound record that carries a Thread.
/// An inbound record, an outbound record with no Thread and another
/// pair are all no answer.
pub async fn the_landing_thread_reads_the_last_outbound_text_with_a_thread(backend: &Backend) {
    let (ws, a, _, run_id) = seed(backend).await;
    let store = &backend.stores().text_records;
    let line_id = PhoneNumberId::generate();
    let counterpart = "+14155550999";

    let older = MessageId::generate();
    let newer = MessageId::generate();
    store
        .insert(&outbound(
            &ws,
            &a.id,
            &line_id,
            &run_id,
            counterpart,
            "older",
            Some(older),
            1_000,
        ))
        .await
        .unwrap();
    store
        .insert(&outbound(
            &ws,
            &a.id,
            &line_id,
            &run_id,
            counterpart,
            "newer",
            Some(newer.clone()),
            2_000,
        ))
        .await
        .unwrap();
    // A later send with no Thread does not take the landing away.
    store
        .insert(&outbound(
            &ws,
            &a.id,
            &line_id,
            &run_id,
            counterpart,
            "no thread",
            None,
            3_000,
        ))
        .await
        .unwrap();
    store
        .insert(&inbound(&ws, &a.id, &line_id, counterpart, "reply", 4_000))
        .await
        .unwrap();

    assert_eq!(
        store
            .landing_thread(&ws.id, &a.id, &line_id, counterpart, 5_000)
            .await
            .unwrap(),
        Some(newer)
    );
    assert_eq!(
        store
            .landing_thread(&ws.id, &a.id, &line_id, "+14155550888", 5_000)
            .await
            .unwrap(),
        None
    );
}

/// Records stay with the Agent that made them, so a new holder of the
/// number reads none of the previous holder's conversations
/// (ADR-0020).
pub async fn a_new_holder_of_the_number_reads_none_of_the_old_texts(backend: &Backend) {
    let (ws, first, second, run_id) = seed(backend).await;
    let store = &backend.stores().text_records;
    let line_id = PhoneNumberId::generate();
    let counterpart = "+14155550999";
    let thread = MessageId::generate();

    store
        .insert(&outbound(
            &ws,
            &first.id,
            &line_id,
            &run_id,
            counterpart,
            "mine",
            Some(thread),
            1_000,
        ))
        .await
        .unwrap();

    assert!(
        store
            .list_conversation(&ws.id, &second.id, &line_id, counterpart, 50, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_conversations(&ws.id, &second.id, &line_id, 25)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .landing_thread(&ws.id, &second.id, &line_id, counterpart, 2_000)
            .await
            .unwrap(),
        None
    );
}

/// Every texting field of the number record survives the round trip,
/// and each write reads back (ADR-0020).
pub async fn every_texting_field_of_a_number_round_trips(backend: &Backend) {
    let (ws, a, _, _) = seed(backend).await;
    let store = &backend.stores().phone_numbers;
    let line = number(&ws.id, &a.id);
    store.create(&line).await.unwrap();

    // A new number starts with the default cap and nothing else.
    let read = store.get(&ws.id, &line.id).await.unwrap().unwrap();
    assert_eq!(read.outgoing_cap, PhoneNumber::DEFAULT_OUTGOING_CAP);
    assert_eq!(read.outgoing_cap, 50);
    assert!(read.allow_rules.is_empty());
    assert_eq!(read.sends_day, None);
    assert_eq!(read.sends_today, 0);
    assert_eq!(read.messaging_readiness, MessagingReadiness::Unknown);
    assert_eq!(read.messaging_readiness_at, None);
    assert_eq!(read.messaging_readiness_error, None);
    assert_eq!(read.text_cursor, None);
    assert_eq!(read.messaging_object_id, None);
    assert_eq!(read.relay_state, TelnyxRelayState::Absent);
    assert!(!read.relay_consent);

    assert!(store.set_outgoing_cap(&ws.id, &line.id, 12).await.unwrap());
    let held = store
        .add_allow_rules(
            &ws.id,
            &line.id,
            &["+14155550999".to_string(), "+14155550888".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(held, ["+14155550999", "+14155550888"]);
    // A rule already there is not written twice, and the order stays.
    let held = store
        .add_allow_rules(&ws.id, &line.id, &["+14155550999".to_string()])
        .await
        .unwrap();
    assert_eq!(held, ["+14155550999", "+14155550888"]);

    let rejected = MessagingReadiness::Rejected {
        reason: "the brand has no tax id".to_string(),
    };
    assert!(
        store
            .set_messaging_readiness(
                &ws.id,
                &line.id,
                &rejected,
                4_242,
                Some("the carrier timed out")
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .set_text_cursor(&ws.id, &line.id, "SM7:1700")
            .await
            .unwrap()
    );
    assert!(
        store
            .set_messaging_object_id(&ws.id, &line.id, "MG0123456789")
            .await
            .unwrap()
    );
    let failed = TelnyxRelayState::Failed {
        reason: "the CLI refused the ship".to_string(),
        cli_lines: vec!["ship: building".to_string(), "ship: error 1".to_string()],
    };
    assert!(
        store
            .set_relay_state(&ws.id, &line.id, &failed)
            .await
            .unwrap()
    );
    assert!(
        store
            .set_relay_consent(&ws.id, &line.id, true)
            .await
            .unwrap()
    );

    let read = store.get(&ws.id, &line.id).await.unwrap().unwrap();
    assert_eq!(read.outgoing_cap, 12);
    assert_eq!(read.allow_rules, ["+14155550999", "+14155550888"]);
    assert_eq!(read.messaging_readiness, rejected);
    assert_eq!(read.messaging_readiness_at, Some(4_242));
    assert_eq!(
        read.messaging_readiness_error.as_deref(),
        Some("the carrier timed out")
    );
    assert_eq!(read.text_cursor.as_deref(), Some("SM7:1700"));
    assert_eq!(read.messaging_object_id.as_deref(), Some("MG0123456789"));
    assert_eq!(read.relay_state, failed);
    assert!(read.relay_consent);

    // A read that succeeds clears the error the last one left.
    assert!(
        store
            .set_messaging_readiness(&ws.id, &line.id, &MessagingReadiness::Ready, 5_000, None)
            .await
            .unwrap()
    );
    let read = store.get(&ws.id, &line.id).await.unwrap().unwrap();
    assert_eq!(read.messaging_readiness, MessagingReadiness::Ready);
    assert!(read.messaging_readiness.is_ready());
    assert_eq!(read.messaging_readiness_error, None);

    // No such number takes a write.
    let gone = PhoneNumberId::generate();
    assert!(!store.set_outgoing_cap(&ws.id, &gone, 1).await.unwrap());
    assert!(!store.set_text_cursor(&ws.id, &gone, "x").await.unwrap());
    assert!(!store.set_relay_consent(&ws.id, &gone, true).await.unwrap());
}

/// The Outgoing Cap is a day's allowance, so the tally starts again
/// on a new day (ADR-0020).
pub async fn the_text_tally_counts_one_day_at_a_time(backend: &Backend) {
    let (ws, a, _, _) = seed(backend).await;
    let store = &backend.stores().phone_numbers;
    let line = number(&ws.id, &a.id);
    store.create(&line).await.unwrap();

    let monday = day_start(now_ms());
    assert_eq!(
        store
            .note_text_send(&ws.id, &line.id, monday)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .note_text_send(&ws.id, &line.id, monday + 3_600_000)
            .await
            .unwrap(),
        2
    );
    let read = store.get(&ws.id, &line.id).await.unwrap().unwrap();
    assert_eq!(read.sends_day, Some(monday));
    assert_eq!(read.sends_today, 2);
    assert_eq!(read.sends_on(monday), 2);

    let tuesday = monday + 24 * 60 * 60 * 1000;
    assert_eq!(
        store
            .note_text_send(&ws.id, &line.id, tuesday)
            .await
            .unwrap(),
        1
    );
    let read = store.get(&ws.id, &line.id).await.unwrap().unwrap();
    assert_eq!(read.sends_day, Some(tuesday));
    assert_eq!(read.sends_today, 1);
    // A tally from an earlier day counts for nothing.
    assert_eq!(read.sends_on(monday), 0);
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_text_records {
    ($emit:path) => {
        $emit!(
            text_records,
            an_outbound_record_round_trips_with_its_thread_and_media,
            an_inbound_record_round_trips_without_a_run,
            the_delivery_status_of_an_outbound_record_is_written,
            a_conversation_pages_newest_first,
            conversations_group_by_counterpart_with_the_last_text,
            the_landing_thread_stops_at_seven_days,
            the_landing_thread_reads_the_last_outbound_text_with_a_thread,
            a_new_holder_of_the_number_reads_none_of_the_old_texts,
            every_texting_field_of_a_number_round_trips,
            the_text_tally_counts_one_day_at_a_time,
        );
    };
}
