use crate::common;

use std::sync::Arc;

use pagis_core::{AgentId, ChannelId, WorkspaceId};
use pagis_trigger::{NewCron, NewInterval, NewSchedule, ScheduleEdit, ScheduleTiming, Trigger};

const NOW: i64 = 1_800_000_000_000;
const MINUTE: i64 = 60_000;

#[tokio::test]
async fn a_subject_schedule_refuses_a_reschedule_inside_its_cooldown() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let due = NOW + MINUTE;
    let schedule = trigger
        .create_schedule(NewSchedule {
            workspace_id: workspace_id.clone(),
            agent_id: agent_id.clone(),
            name: "School follow-up".into(),
            instruction: "Check whether the date changed".into(),
            subject_page_path: Some("private/subjects/gmail/school.md".into()),
            channel_id: channel_id.clone(),
            root_message_id: None,
            timing: ScheduleTiming::OneShot {
                local_time: "2027-01-15T08:01:00".into(),
                timezone: "UTC".into(),
            },
            creator: pagis_core::CreatorKind::Agent,
            creating_run_id: None,
            now: NOW,
        })
        .await
        .expect("create subject Schedule");
    trigger.process_due(due).await.expect("fire Schedule");

    let error = trigger
        .edit_schedule(
            &schedule.workspace_id,
            &schedule.id,
            ScheduleEdit {
                expected_revision: 1,
                agent_id,
                name: schedule.name,
                instruction: schedule.instruction,
                channel_id,
                root_message_id: None,
                timing: ScheduleTiming::OneShot {
                    local_time: "2027-01-16T08:01:00".into(),
                    timezone: "UTC".into(),
                },
            },
            due,
        )
        .await
        .expect_err("the cooldown refuses an early reschedule");

    assert!(error.to_string().contains("14-day cooldown"), "{error}");
}

async fn setup() -> (Arc<Trigger>, WorkspaceId, AgentId, ChannelId) {
    let world = common::world().await;
    (
        world.trigger,
        world.workspace_id,
        world.agent_id,
        world.channel_id,
    )
}

#[tokio::test]
async fn interval_uses_its_anchor_when_processing_is_late() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let anchor = NOW + MINUTE;
    let schedule = trigger
        .create_interval(NewInterval {
            workspace_id,
            agent_id,
            name: "Mailbox review".to_string(),
            instruction: "Review new mail".to_string(),
            channel_id,
            root_message_id: None,
            every_ms: 5 * MINUTE,
            anchor,
            now: NOW,
        })
        .await
        .expect("create interval");

    assert_eq!(schedule.next_due_at, Some(anchor));
    trigger
        .process_due(anchor + 2 * MINUTE)
        .await
        .expect("process first occurrence");

    let schedule = trigger
        .get_schedule(&schedule.workspace_id, &schedule.id)
        .await
        .expect("read schedule")
        .expect("schedule exists");
    assert_eq!(schedule.next_due_at, Some(anchor + 5 * MINUTE));
}

#[tokio::test]
async fn interval_downtime_creates_one_latest_occurrence_without_a_backlog() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let anchor = NOW + MINUTE;
    let schedule = trigger
        .create_interval(NewInterval {
            workspace_id,
            agent_id,
            name: "Mailbox review".to_string(),
            instruction: "Review new mail".to_string(),
            channel_id,
            root_message_id: None,
            every_ms: 5 * MINUTE,
            anchor,
            now: NOW,
        })
        .await
        .expect("create interval");
    let returned_at = anchor + 7 * 24 * 60 * MINUTE + 2 * MINUTE;

    let processed = trigger
        .process_due(returned_at)
        .await
        .expect("process after downtime");
    assert_eq!(processed.created_occurrences, 1);
    assert_eq!(processed.created_wakeups, 1);

    let occurrences = trigger
        .list_occurrences(&schedule.workspace_id, &schedule.id, None, 10)
        .await
        .expect("occurrence history");
    assert_eq!(occurrences.len(), 1);
    assert_eq!(occurrences[0].scheduled_at, anchor + 7 * 24 * 60 * MINUTE);

    let schedule = trigger
        .get_schedule(&schedule.workspace_id, &schedule.id)
        .await
        .expect("read schedule")
        .expect("schedule exists");
    assert_eq!(
        schedule.next_due_at,
        Some(anchor + 7 * 24 * 60 * MINUTE + 5 * MINUTE)
    );
    assert!(
        trigger
            .process_due(returned_at)
            .await
            .expect("repeat due processing")
            .created_occurrences
            == 0
    );
}

#[tokio::test]
async fn interval_rejects_a_cadence_below_one_minute() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let result = trigger
        .create_interval(NewInterval {
            workspace_id,
            agent_id,
            name: "Too frequent".to_string(),
            instruction: "Check now".to_string(),
            channel_id,
            root_message_id: None,
            every_ms: MINUTE - 1,
            anchor: NOW + MINUTE,
            now: NOW,
        })
        .await;

    assert!(matches!(
        result,
        Err(pagis_trigger::TriggerError::IntervalTooShort)
    ));
}

#[tokio::test]
async fn cron_moves_a_spring_gap_to_the_first_valid_local_instant() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let now = chrono::DateTime::parse_from_rfc3339("2027-03-13T12:00:00Z")
        .unwrap()
        .timestamp_millis();
    let schedule = trigger
        .create_cron(NewCron {
            workspace_id,
            agent_id,
            name: "Daily review".to_string(),
            instruction: "Review the day".to_string(),
            channel_id,
            root_message_id: None,
            expression: "30 2 * * *".to_string(),
            timezone: "America/Los_Angeles".to_string(),
            now,
        })
        .await
        .expect("create cron Schedule");

    let expected = chrono::DateTime::parse_from_rfc3339("2027-03-14T10:00:00Z")
        .unwrap()
        .timestamp_millis();
    assert_eq!(schedule.next_due_at, Some(expected));
}

#[tokio::test]
async fn cron_uses_the_first_instant_in_a_repeated_hour() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let now = chrono::DateTime::parse_from_rfc3339("2027-11-06T12:00:00Z")
        .unwrap()
        .timestamp_millis();
    let schedule = trigger
        .create_cron(NewCron {
            workspace_id,
            agent_id,
            name: "Daily review".to_string(),
            instruction: "Review the day".to_string(),
            channel_id,
            root_message_id: None,
            expression: "30 1 * * *".to_string(),
            timezone: "America/Los_Angeles".to_string(),
            now,
        })
        .await
        .expect("create cron Schedule");

    let first = chrono::DateTime::parse_from_rfc3339("2027-11-07T08:30:00Z")
        .unwrap()
        .timestamp_millis();
    assert_eq!(schedule.next_due_at, Some(first));
}

#[tokio::test]
async fn cron_downtime_creates_only_the_latest_due_occurrence() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let now = chrono::DateTime::parse_from_rfc3339("2027-01-01T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    let schedule = trigger
        .create_cron(NewCron {
            workspace_id,
            agent_id,
            name: "Daily review".to_string(),
            instruction: "Review the day".to_string(),
            channel_id,
            root_message_id: None,
            expression: "0 9 * * *".to_string(),
            timezone: "UTC".to_string(),
            now,
        })
        .await
        .expect("create cron Schedule");
    let returned_at = chrono::DateTime::parse_from_rfc3339("2027-01-08T12:00:00Z")
        .unwrap()
        .timestamp_millis();

    let processed = trigger.process_due(returned_at).await.expect("process due");
    assert_eq!(processed.created_occurrences, 1);
    assert_eq!(processed.created_wakeups, 1);
    let occurrences = trigger
        .list_occurrences(&schedule.workspace_id, &schedule.id, None, 10)
        .await
        .expect("occurrence history");
    let latest = chrono::DateTime::parse_from_rfc3339("2027-01-08T09:00:00Z")
        .unwrap()
        .timestamp_millis();
    let next = chrono::DateTime::parse_from_rfc3339("2027-01-09T09:00:00Z")
        .unwrap()
        .timestamp_millis();
    assert_eq!(occurrences[0].scheduled_at, latest);
    assert_eq!(
        trigger
            .get_schedule(&schedule.workspace_id, &schedule.id)
            .await
            .unwrap()
            .unwrap()
            .next_due_at,
        Some(next)
    );
}

#[tokio::test]
async fn occurrences_during_an_active_run_share_one_pending_wakeup() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let anchor = NOW + MINUTE;
    let schedule = trigger
        .create_interval(NewInterval {
            workspace_id: workspace_id.clone(),
            agent_id: agent_id.clone(),
            name: "Mailbox review".to_string(),
            instruction: "Review new mail".to_string(),
            channel_id,
            root_message_id: None,
            every_ms: MINUTE,
            anchor,
            now: NOW,
        })
        .await
        .expect("create interval");

    trigger.process_due(anchor).await.expect("first due");
    let active = trigger
        .claim_wakeups(
            &workspace_id,
            &agent_id,
            pagis_core::RunSlots {
                conversation: 1,
                arrival: 0,
            },
            anchor,
        )
        .await
        .expect("claim first Wake-up");
    assert_eq!(active.len(), 1);

    let second = trigger
        .process_due(anchor + MINUTE)
        .await
        .expect("second due");
    let third = trigger
        .process_due(anchor + 2 * MINUTE)
        .await
        .expect("third due");
    assert_eq!(second.created_wakeups, 1);
    assert_eq!(third.created_wakeups, 0);
    assert!(
        trigger
            .claim_wakeups(
                &workspace_id,
                &agent_id,
                pagis_core::RunSlots {
                    conversation: 1,
                    arrival: 0
                },
                anchor + 2 * MINUTE
            )
            .await
            .expect("claim while first Run is active")
            .is_empty()
    );

    let wakeups = trigger
        .list_wakeups(&schedule.workspace_id, &schedule.id, None, 10)
        .await
        .expect("Wake-up history");
    assert_eq!(wakeups.len(), 2);
    let pending = wakeups
        .iter()
        .find(|wakeup| wakeup.state == pagis_core::WakeupState::Pending)
        .expect("combined pending Wake-up");
    assert_eq!(pending.source_count.saturating_sub(1), 1);
    let occurrences = trigger
        .list_occurrences(&schedule.workspace_id, &schedule.id, None, 10)
        .await
        .expect("occurrence history");
    assert_eq!(occurrences.len(), 3);
    assert_eq!(
        occurrences
            .iter()
            .filter(|occurrence| occurrence.wakeup_id.as_ref() == Some(&pending.id))
            .count(),
        2
    );
}

#[tokio::test]
async fn pause_resume_and_skip_next_do_not_catch_up() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let anchor = NOW + MINUTE;
    let schedule = trigger
        .create_interval(NewInterval {
            workspace_id,
            agent_id,
            name: "Mailbox review".to_string(),
            instruction: "Review new mail".to_string(),
            channel_id,
            root_message_id: None,
            every_ms: MINUTE,
            anchor,
            now: NOW,
        })
        .await
        .expect("create interval");

    let paused = trigger
        .pause_schedule(&schedule.workspace_id, &schedule.id, NOW + 1)
        .await
        .expect("pause")
        .expect("schedule exists");
    assert_eq!(paused.state, pagis_core::ScheduleState::Paused);
    assert_eq!(paused.next_due_at, None);
    assert_eq!(
        trigger
            .process_due(anchor + 10 * MINUTE)
            .await
            .expect("process while paused")
            .created_occurrences,
        0
    );

    let resumed_at = anchor + 10 * MINUTE;
    let resumed = trigger
        .resume_schedule(&schedule.workspace_id, &schedule.id, resumed_at)
        .await
        .expect("resume")
        .expect("schedule exists");
    assert_eq!(resumed.state, pagis_core::ScheduleState::Active);
    assert_eq!(resumed.next_due_at, Some(resumed_at + MINUTE));

    let skipped = trigger
        .skip_next(
            &schedule.workspace_id,
            &schedule.id,
            resumed.next_due_at.unwrap(),
            resumed_at + 1,
        )
        .await
        .expect("skip next");
    assert_eq!(skipped.outcome, "skipped");
    assert_eq!(skipped.scheduled_at, resumed_at + MINUTE);
    let after_skip = trigger
        .get_schedule(&schedule.workspace_id, &schedule.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_skip.next_due_at, Some(resumed_at + 2 * MINUTE));
    assert_eq!(
        trigger
            .process_due(resumed_at + MINUTE)
            .await
            .expect("process skipped time")
            .created_occurrences,
        0
    );
}

#[tokio::test]
async fn skip_next_conflicts_when_due_processing_won() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let anchor = NOW + MINUTE;
    let schedule = trigger
        .create_interval(NewInterval {
            workspace_id,
            agent_id,
            name: "Mailbox review".to_string(),
            instruction: "Review new mail".to_string(),
            channel_id,
            root_message_id: None,
            every_ms: MINUTE,
            anchor,
            now: NOW,
        })
        .await
        .expect("create interval");
    let expected_due_at = schedule.next_due_at.unwrap();

    trigger
        .process_due(anchor)
        .await
        .expect("due processing wins");
    let result = trigger
        .skip_next(
            &schedule.workspace_id,
            &schedule.id,
            expected_due_at,
            anchor + 1,
        )
        .await;

    assert!(matches!(
        result,
        Err(pagis_trigger::TriggerError::Store(
            pagis_core::StoreError::Conflict(_)
        ))
    ));
}

#[tokio::test]
async fn edit_preserves_revisions_and_withdraws_only_pending_old_work() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let anchor = NOW + MINUTE;
    let schedule = trigger
        .create_interval(NewInterval {
            workspace_id: workspace_id.clone(),
            agent_id: agent_id.clone(),
            name: "Mailbox review".to_string(),
            instruction: "Review new mail".to_string(),
            channel_id: channel_id.clone(),
            root_message_id: None,
            every_ms: MINUTE,
            anchor,
            now: NOW,
        })
        .await
        .expect("create interval");
    trigger.process_due(anchor).await.expect("first due");
    trigger
        .claim_wakeups(
            &workspace_id,
            &agent_id,
            pagis_core::RunSlots {
                conversation: 1,
                arrival: 0,
            },
            anchor,
        )
        .await
        .expect("start first Run");
    trigger
        .process_due(anchor + MINUTE)
        .await
        .expect("create pending overlap");

    let edited = trigger
        .edit_schedule(
            &schedule.workspace_id,
            &schedule.id,
            ScheduleEdit {
                expected_revision: 1,
                agent_id,
                name: "Mailbox review".to_string(),
                instruction: "Review mail and draft replies".to_string(),
                channel_id,
                root_message_id: None,
                timing: ScheduleTiming::Interval {
                    every_ms: 2 * MINUTE,
                    anchor: anchor + 2 * MINUTE,
                },
            },
            anchor + MINUTE + 1,
        )
        .await
        .expect("edit Schedule")
        .expect("Schedule exists");
    assert_eq!(edited.revision, 2);
    assert_eq!(edited.approved_revision, Some(2));
    assert_eq!(edited.instruction, "Review mail and draft replies");

    let revisions = trigger
        .list_revisions(&schedule.workspace_id, &schedule.id)
        .await
        .expect("revision history");
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0].revision, 1);
    assert_eq!(revisions[0].instruction, "Review new mail");
    assert_eq!(revisions[1].revision, 2);
    assert_eq!(revisions[1].instruction, "Review mail and draft replies");

    let wakeups = trigger
        .list_wakeups(&schedule.workspace_id, &schedule.id, None, 10)
        .await
        .expect("Wake-up history");
    assert_eq!(
        wakeups
            .iter()
            .filter(|wakeup| wakeup.state == pagis_core::WakeupState::Started)
            .count(),
        1
    );
    assert_eq!(
        wakeups
            .iter()
            .filter(|wakeup| wakeup.state == pagis_core::WakeupState::Withdrawn)
            .count(),
        1
    );
}
