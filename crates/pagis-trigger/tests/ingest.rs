//! Event ingestion through the Trigger module's public interface
//! (ADR-0006). Each test calls `ingest` and observes cursor
//! commits, deduplication, matching, and Wake-ups through the stores.

use crate::common;

use pagis_core::{
    AgentId, ChannelId, EventSubscription, EventSubscriptionState, IngestBatch, NormalizedEvent,
    SourceBatchOutcome, WakeupLanding, WakeupState,
};
use pagis_trigger::{NewSubscription, SubscriptionAction};

use common::{NOW, TEST_EVENT_KIND, World};

fn mail(id: &str, from: &str, at: i64) -> NormalizedEvent {
    NormalizedEvent {
        provider_event_id: id.to_string(),
        metadata: serde_json::json!({"from": from, "subject": "Hello"}),
        occurred_at: at,
        landing: None,
    }
}

fn batch(world: &World, events: Vec<NormalizedEvent>, cursor: &str, at: i64) -> IngestBatch {
    IngestBatch {
        workspace_id: world.workspace_id.clone(),
        connection_id: world.connection_id.clone(),
        agent_id: None,
        event_kind: TEST_EVENT_KIND.to_string(),
        cursor: Some(cursor.to_string()),
        events,
        received_at: at,
        baseline: false,
    }
}

async fn subscribe(world: &World, name: &str, senders: &[&str]) -> EventSubscription {
    world
        .trigger
        .create_subscription(NewSubscription {
            workspace_id: world.workspace_id.clone(),
            agent_id: world.agent_id.clone(),
            connection_id: world.connection_id.clone(),
            event_kind: TEST_EVENT_KIND.to_string(),
            name: name.to_string(),
            instruction: format!("Handle {name}"),
            channel_id: world.channel_id.clone(),
            root_message_id: None,
            filter: serde_json::json!({"senders": senders}),
            creator: pagis_core::CreatorKind::User,
            now: NOW,
        })
        .await
        .expect("create subscription")
}

#[tokio::test]
async fn three_rules_on_one_connection_take_one_pass_and_three_wakeups() {
    let world = common::world().await;
    subscribe(&world, "first", &["a@example.com"]).await;
    subscribe(&world, "second", &["a@example.com"]).await;
    subscribe(&world, "third", &["a@example.com"]).await;

    let outcome = world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("m1", "a@example.com", NOW + 10)],
            "cursor-1",
            NOW + 20,
        ))
        .await
        .expect("ingest");

    assert_eq!(outcome.events.len(), 1, "one pass stores the mail once");
    assert_eq!(outcome.created.len(), 3, "each rule gets its own Wake-up");
    assert_eq!(outcome.batch.outcome, SourceBatchOutcome::Collected);

    let claims = world
        .trigger
        .claim_wakeups(
            &world.workspace_id,
            &world.agent_id,
            pagis_core::RunSlots {
                conversation: 10,
                arrival: 10,
            },
            NOW + 30,
        )
        .await
        .expect("claim");
    assert_eq!(claims.len(), 3);
    assert!(
        claims
            .iter()
            .all(|claim| claim.run.trigger_kind == pagis_core::TriggerKind::Event)
    );
}

#[tokio::test]
async fn re_reading_an_overlapping_window_adds_no_duplicate() {
    let world = common::world().await;
    let subscription = subscribe(&world, "inbox", &["a@example.com"]).await;
    let events = vec![mail("m1", "a@example.com", NOW + 10)];

    let first = world
        .trigger
        .ingest(batch(&world, events.clone(), "cursor-1", NOW + 20))
        .await
        .expect("first pass");
    let second = world
        .trigger
        .ingest(batch(&world, events, "cursor-2", NOW + 80))
        .await
        .expect("overlapping pass");

    assert_eq!(first.created.len(), 1);
    assert!(
        second.events.is_empty(),
        "the provider id is already stored"
    );
    assert!(second.created.is_empty());
    assert!(second.combined.is_empty());
    assert_eq!(
        world
            .trigger
            .list_subscription_wakeups(&subscription.workspace_id, &subscription.id, None, 10)
            .await
            .expect("wakeups")
            .len(),
        1
    );
    assert_eq!(
        world
            .trigger
            .cursor(&world.workspace_id, &world.connection_id, TEST_EVENT_KIND)
            .await
            .expect("cursor")
            .as_deref(),
        Some("cursor-2"),
        "each pass commits its own cursor"
    );
}

#[tokio::test]
async fn a_large_batch_makes_one_run_per_rule_not_one_per_message() {
    let world = common::world().await;
    subscribe(&world, "inbox", &["a@example.com"]).await;
    let mail: Vec<_> = (0..25)
        .map(|index| {
            self::mail(
                &format!("m{index}"),
                "a@example.com",
                NOW + 10 + i64::from(index),
            )
        })
        .collect();

    let outcome = world
        .trigger
        .ingest(batch(&world, mail, "cursor-1", NOW + 100))
        .await
        .expect("ingest");

    assert_eq!(outcome.events.len(), 25);
    assert_eq!(outcome.created.len(), 1);
    assert_eq!(outcome.created[0].source_count, 25);
    let claims = world
        .trigger
        .claim_wakeups(
            &world.workspace_id,
            &world.agent_id,
            pagis_core::RunSlots {
                conversation: 10,
                arrival: 10,
            },
            NOW + 120,
        )
        .await
        .expect("claim");
    assert_eq!(claims.len(), 1, "one Run carries the whole batch");
}

#[tokio::test]
async fn later_mail_joins_the_pending_wakeup() {
    let world = common::world().await;
    subscribe(&world, "inbox", &["a@example.com"]).await;
    world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("m1", "a@example.com", NOW + 10)],
            "cursor-1",
            NOW + 20,
        ))
        .await
        .expect("first");

    let second = world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("m2", "a@example.com", NOW + 30)],
            "cursor-2",
            NOW + 40,
        ))
        .await
        .expect("second");

    assert!(second.created.is_empty());
    assert_eq!(second.combined.len(), 1);
    assert_eq!(second.combined[0].source_count, 2);
}

#[tokio::test]
async fn a_filter_that_does_not_match_wakes_nobody() {
    let world = common::world().await;
    subscribe(&world, "inbox", &["wanted@example.com"]).await;

    let outcome = world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("m1", "other@example.com", NOW + 10)],
            "cursor-1",
            NOW + 20,
        ))
        .await
        .expect("ingest");

    assert_eq!(outcome.events.len(), 1, "the event is still recorded");
    assert!(outcome.created.is_empty());
}

#[tokio::test]
async fn the_first_collection_is_a_baseline_that_wakes_nobody() {
    let world = common::world().await;
    subscribe(&world, "inbox", &["a@example.com"]).await;

    let outcome = world
        .trigger
        .ingest(IngestBatch {
            baseline: true,
            ..batch(
                &world,
                vec![mail("old", "a@example.com", NOW - 100_000)],
                "cursor-1",
                NOW + 20,
            )
        })
        .await
        .expect("baseline");

    assert_eq!(outcome.batch.outcome, SourceBatchOutcome::Baseline);
    assert!(outcome.events.is_empty(), "old mail is not news");
    assert!(outcome.created.is_empty());
    assert_eq!(
        world
            .trigger
            .cursor(&world.workspace_id, &world.connection_id, TEST_EVENT_KIND)
            .await
            .expect("cursor")
            .as_deref(),
        Some("cursor-1"),
        "the baseline still commits its cursor"
    );
}

#[tokio::test]
async fn a_pause_withdraws_pending_work_and_a_resume_does_not_catch_up() {
    let world = common::world().await;
    let subscription = subscribe(&world, "inbox", &["a@example.com"]).await;
    world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("m1", "a@example.com", NOW + 10)],
            "cursor-1",
            NOW + 20,
        ))
        .await
        .expect("ingest");

    world
        .trigger
        .update_subscription(
            &subscription.workspace_id,
            &subscription.id,
            SubscriptionAction::Pause,
            NOW + 30,
        )
        .await
        .expect("pause");
    let wakeups = world
        .trigger
        .list_subscription_wakeups(&subscription.workspace_id, &subscription.id, None, 10)
        .await
        .expect("wakeups");
    assert_eq!(wakeups[0].state, WakeupState::Withdrawn);

    // A paused rule is not in the collector's work list at all.
    assert!(world.trigger.collector_targets().await.unwrap().is_empty());

    let resumed = world
        .trigger
        .update_subscription(
            &subscription.workspace_id,
            &subscription.id,
            SubscriptionAction::Resume,
            NOW + 40,
        )
        .await
        .expect("resume");
    assert_eq!(resumed.state, EventSubscriptionState::Active);
    assert_eq!(resumed.watermark_at, Some(NOW + 40));

    // Mail from the pause is older than the new watermark, so it never
    // becomes a Wake-up.
    let after = world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("m2", "a@example.com", NOW + 35)],
            "cursor-2",
            NOW + 50,
        ))
        .await
        .expect("after resume");
    assert!(after.created.is_empty());
}

#[tokio::test]
async fn revoking_the_grant_withdraws_pending_work_and_blocks_the_rule() {
    let world = common::world().await;
    let subscription = subscribe(&world, "inbox", &["a@example.com"]).await;
    world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("m1", "a@example.com", NOW + 10)],
            "cursor-1",
            NOW + 20,
        ))
        .await
        .expect("ingest");
    common::revoke_grant(&world).await.expect("revoke");

    let after = world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("m2", "a@example.com", NOW + 30)],
            "cursor-2",
            NOW + 40,
        ))
        .await
        .expect("ingest after revocation");

    assert!(after.created.is_empty(), "a revoked grant wakes nobody");
    let reloaded = world
        .trigger
        .get_subscription(&subscription.workspace_id, &subscription.id)
        .await
        .expect("read")
        .expect("subscription");
    assert_eq!(reloaded.state, EventSubscriptionState::Blocked);
    let wakeups = world
        .trigger
        .list_subscription_wakeups(&subscription.workspace_id, &subscription.id, None, 10)
        .await
        .expect("wakeups");
    assert!(
        wakeups
            .iter()
            .all(|wakeup| wakeup.state == WakeupState::Withdrawn),
        "pending work is withdrawn"
    );
    assert!(
        world
            .trigger
            .claim_wakeups(
                &world.workspace_id,
                &world.agent_id,
                pagis_core::RunSlots {
                    conversation: 10,
                    arrival: 10
                },
                NOW + 50
            )
            .await
            .expect("claim")
            .is_empty()
    );
}

#[tokio::test]
async fn a_connection_block_keeps_the_cursor_and_reauthorization_catches_up() {
    let world = common::world().await;
    let subscription = subscribe(&world, "inbox", &["a@example.com"]).await;
    world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("m1", "a@example.com", NOW + 10)],
            "cursor-1",
            NOW + 20,
        ))
        .await
        .expect("ingest");

    let blocked = world
        .trigger
        .block_connection(
            &world.workspace_id,
            &world.connection_id,
            pagis_core::BlockReason::ReauthRequired,
            NOW + 30,
        )
        .await
        .expect("block");
    assert_eq!(blocked, 1);
    assert!(world.trigger.collector_targets().await.unwrap().is_empty());
    assert_eq!(
        world
            .trigger
            .cursor(&world.workspace_id, &world.connection_id, TEST_EVENT_KIND)
            .await
            .expect("cursor")
            .as_deref(),
        Some("cursor-1"),
        "the cursor survives so the catch-up is bounded"
    );

    let restored = world
        .trigger
        .unblock_connection(&world.workspace_id, &world.connection_id, NOW + 40)
        .await
        .expect("unblock");
    assert_eq!(restored, 1);
    let reloaded = world
        .trigger
        .get_subscription(&subscription.workspace_id, &subscription.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.state, EventSubscriptionState::Active);
    assert_eq!(
        reloaded.watermark_at, subscription.watermark_at,
        "reauthorization catches up rather than starting over"
    );

    let catch_up = world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("m2", "a@example.com", NOW + 35)],
            "cursor-2",
            NOW + 50,
        ))
        .await
        .expect("catch-up");
    assert_eq!(catch_up.created.len(), 1);
}

#[tokio::test]
async fn a_restored_grant_delivers_forward_and_never_replays_the_gap() {
    let world = common::world().await;
    let subscription = subscribe(&world, "inbox", &["a@example.com"]).await;
    common::revoke_grant(&world).await.expect("revoke");
    world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("gap", "a@example.com", NOW + 10)],
            "cursor-1",
            NOW + 20,
        ))
        .await
        .expect("ingest while blocked");

    // The rule is blocked and stays blocked while the grant is gone.
    assert_eq!(
        world
            .trigger
            .unblock_connection(&world.workspace_id, &world.connection_id, NOW + 30)
            .await
            .unwrap(),
        0
    );

    common::restore_grant(&world, NOW + 40)
        .await
        .expect("regrant");
    let restored = world
        .trigger
        .unblock_connection(&world.workspace_id, &world.connection_id, NOW + 40)
        .await
        .expect("unblock");
    assert_eq!(restored, 1);
    let reloaded = world
        .trigger
        .get_subscription(&subscription.workspace_id, &subscription.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.state, EventSubscriptionState::Active);
    assert_eq!(
        reloaded.watermark_at,
        Some(NOW + 40),
        "a grant gap moves the watermark, so the gap never replays"
    );

    // Mail from the gap is behind the new watermark and wakes nobody.
    let after = world
        .trigger
        .ingest(batch(
            &world,
            vec![mail("gap2", "a@example.com", NOW + 35)],
            "cursor-2",
            NOW + 50,
        ))
        .await
        .expect("ingest after regrant");
    assert!(after.created.is_empty());
}

#[tokio::test]
async fn a_pagis_audit_event_is_not_a_subscription_source() {
    let world = common::world().await;

    let refused = world
        .trigger
        .create_subscription(NewSubscription {
            workspace_id: world.workspace_id.clone(),
            agent_id: world.agent_id.clone(),
            connection_id: world.connection_id.clone(),
            event_kind: "core.run.completed".to_string(),
            name: "audit".to_string(),
            instruction: "watch runs".to_string(),
            channel_id: world.channel_id.clone(),
            root_message_id: None,
            filter: serde_json::json!({}),
            creator: pagis_core::CreatorKind::User,
            now: NOW,
        })
        .await;

    assert!(matches!(
        refused,
        Err(pagis_trigger::TriggerError::NativeEventKind(_))
    ));
}

#[tokio::test]
async fn a_filter_outside_the_declared_schema_is_refused() {
    let world = common::world().await;

    let refused = world
        .trigger
        .create_subscription(NewSubscription {
            workspace_id: world.workspace_id.clone(),
            agent_id: world.agent_id.clone(),
            connection_id: world.connection_id.clone(),
            event_kind: TEST_EVENT_KIND.to_string(),
            name: "raw query".to_string(),
            instruction: "watch mail".to_string(),
            channel_id: world.channel_id.clone(),
            root_message_id: None,
            filter: serde_json::json!({"gmail_query": "in:anywhere"}),
            creator: pagis_core::CreatorKind::User,
            now: NOW,
        })
        .await;

    assert!(matches!(
        refused,
        Err(pagis_trigger::TriggerError::InvalidFilter(_))
    ));
}

#[tokio::test]
async fn a_failed_collection_is_recorded_without_moving_the_cursor() {
    let world = common::world().await;
    subscribe(&world, "inbox", &["a@example.com"]).await;
    world
        .trigger
        .ingest(batch(&world, Vec::new(), "cursor-1", NOW + 20))
        .await
        .expect("ingest");

    world
        .trigger
        .record_collection_failure(
            &world.workspace_id,
            &world.connection_id,
            TEST_EVENT_KIND,
            "temporarily_unavailable",
            NOW + 60,
        )
        .await
        .expect("record failure");

    let (last, succeeded) = world
        .trigger
        .collector_health(&world.workspace_id, &world.connection_id, TEST_EVENT_KIND)
        .await
        .expect("health");
    assert_eq!(last.unwrap().outcome, SourceBatchOutcome::Failed);
    assert_eq!(succeeded.unwrap().outcome, SourceBatchOutcome::Collected);
    assert_eq!(
        world
            .trigger
            .cursor(&world.workspace_id, &world.connection_id, TEST_EVENT_KIND)
            .await
            .expect("cursor")
            .as_deref(),
        Some("cursor-1")
    );
}

// The Agent-scoped pass and the landing Thread (ADR-0019).

/// One Channel that is not the rule's own, so a landing can differ.
async fn other_channel(world: &World) -> ChannelId {
    let channel_id = ChannelId::generate();
    sqlx::query(
        "INSERT INTO channels (id, workspace_id, kind, title, created_at, updated_at) \
         VALUES (?, ?, 'group', 'Clinic', ?, ?)",
    )
    .bind(channel_id.as_str())
    .bind(world.workspace_id.as_str())
    .bind(NOW)
    .bind(NOW)
    .execute(&world.pool)
    .await
    .expect("channel");
    channel_id
}

/// An Agent Mailbox is one Agent's mail, so a pass that names an Agent
/// reaches that Agent's rules alone.
#[tokio::test]
async fn a_pass_that_names_one_agent_skips_another_agents_rules() {
    let world = common::world().await;
    subscribe(&world, "inbox", &["a@example.com"]).await;

    let elsewhere = world
        .trigger
        .ingest(IngestBatch {
            agent_id: Some(AgentId::generate()),
            ..batch(
                &world,
                vec![mail("m1", "a@example.com", NOW + 10)],
                "cursor-1",
                NOW + 20,
            )
        })
        .await
        .expect("ingest");
    assert_eq!(elsewhere.events.len(), 1, "the event is still stored");
    assert!(
        elsewhere.created.is_empty(),
        "another Agent's mail wakes nobody here"
    );

    let mine = world
        .trigger
        .ingest(IngestBatch {
            agent_id: Some(world.agent_id.clone()),
            ..batch(
                &world,
                vec![mail("m2", "a@example.com", NOW + 30)],
                "cursor-2",
                NOW + 40,
            )
        })
        .await
        .expect("ingest");
    assert_eq!(mine.created.len(), 1, "its own Agent's mail wakes it");
}

/// A burst joins one Wake-up, but only where the whole burst lands
/// together: two replies that continue two conversations are two.
#[tokio::test]
async fn events_that_land_in_different_threads_make_different_wakeups() {
    let world = common::world().await;
    subscribe(&world, "inbox", &["a@example.com"]).await;
    let elsewhere = other_channel(&world).await;
    let landing = WakeupLanding {
        channel_id: elsewhere.clone(),
        root_message_id: None,
    };

    let mut answer = mail("m1", "a@example.com", NOW + 10);
    answer.landing = Some(landing.clone());
    let mut second_answer = mail("m2", "a@example.com", NOW + 11);
    second_answer.landing = Some(landing);
    let outcome = world
        .trigger
        .ingest(batch(
            &world,
            vec![answer, second_answer, mail("m3", "a@example.com", NOW + 12)],
            "cursor-1",
            NOW + 20,
        ))
        .await
        .expect("ingest");

    assert_eq!(outcome.created.len(), 2, "one Wake-up per landing");
    let answered = outcome
        .created
        .iter()
        .find(|wakeup| wakeup.channel_id.as_ref() == Some(&elsewhere))
        .expect("the answers land where the Agent asked");
    assert_eq!(answered.source_count, 2, "the two answers join one Wake-up");
    let rest = outcome
        .created
        .iter()
        .find(|wakeup| wakeup.channel_id.as_ref() == Some(&world.channel_id))
        .expect("everything else lands where the rule says");
    assert_eq!(rest.source_count, 1);
}
