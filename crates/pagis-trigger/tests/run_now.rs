//! A Schedule that runs now, ahead of its cadence (ADR-0022).
//! Home asks for the Report this way. The call leaves the cadence
//! untouched, and it writes its own occurrence, so a Schedule that
//! already fired today is not swallowed by the occurrence key.

use crate::common;

use std::sync::Arc;

use pagis_core::{AgentId, ChannelId, WakeupState, WorkspaceId};
use pagis_trigger::{NewCron, Trigger};

const NOW: i64 = 1_800_000_000_000;
const MINUTE: i64 = 60_000;

async fn setup() -> (Arc<Trigger>, WorkspaceId, AgentId, ChannelId) {
    let world = common::world().await;
    (
        world.trigger,
        world.workspace_id,
        world.agent_id,
        world.channel_id,
    )
}

async fn daily_report(
    trigger: &Trigger,
    ids: (WorkspaceId, AgentId, ChannelId),
) -> pagis_core::Schedule {
    trigger
        .create_cron(NewCron {
            workspace_id: ids.0,
            agent_id: ids.1,
            name: "Daily report".to_string(),
            instruction: "Write the report for Home".to_string(),
            channel_id: ids.2,
            root_message_id: None,
            expression: "0 7 * * *".to_string(),
            timezone: "UTC".to_string(),
            now: NOW,
        })
        .await
        .expect("create the Report Schedule")
}

#[tokio::test]
async fn a_schedule_that_runs_now_keeps_its_cadence() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let schedule = daily_report(&trigger, (workspace_id, agent_id, channel_id)).await;
    let due = schedule.next_due_at;

    let wakeup = trigger
        .run_now(&schedule.workspace_id, &schedule.id, NOW)
        .await
        .expect("run now")
        .expect("a Wake-up waits");

    assert_eq!(wakeup.rule.schedule_id(), Some(&schedule.id));
    assert_eq!(wakeup.scheduled_at, NOW);
    assert_eq!(wakeup.state, WakeupState::Pending);
    let after = trigger
        .get_schedule(&schedule.workspace_id, &schedule.id)
        .await
        .expect("read the Schedule")
        .expect("the Schedule exists");
    assert_eq!(after.next_due_at, due);
}

#[tokio::test]
async fn asking_twice_returns_the_one_wake_up_that_waits() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let schedule = daily_report(&trigger, (workspace_id, agent_id, channel_id)).await;

    let first = trigger
        .run_now(&schedule.workspace_id, &schedule.id, NOW)
        .await
        .unwrap()
        .unwrap();
    let second = trigger
        .run_now(&schedule.workspace_id, &schedule.id, NOW + MINUTE)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(first.id, second.id);
    let wakeups = trigger
        .list_wakeups(&schedule.workspace_id, &schedule.id, None, 10)
        .await
        .expect("Wake-up history");
    assert_eq!(wakeups.len(), 1);
}

// The cadence keys an occurrence on its due instant, so a second
// occurrence at the same instant is dropped. A Report asked for after
// the cadence already fired must still reach the Agent.
#[tokio::test]
async fn a_report_asked_for_after_the_cadence_fired_gets_its_own_wake_up() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let schedule = daily_report(
        &trigger,
        (workspace_id.clone(), agent_id.clone(), channel_id),
    )
    .await;
    let due = schedule.next_due_at.expect("a cron Schedule is due");
    trigger.process_due(due).await.expect("fire the cadence");
    let claimed = trigger
        .claim_wakeups(
            &workspace_id,
            &agent_id,
            pagis_core::RunSlots {
                conversation: 1,
                arrival: 0,
            },
            due,
        )
        .await
        .expect("claim the fired Wake-up");
    assert_eq!(claimed.len(), 1);

    let wakeup = trigger
        .run_now(&schedule.workspace_id, &schedule.id, due + MINUTE)
        .await
        .expect("run now")
        .expect("a Wake-up waits");

    assert_ne!(wakeup.id, claimed[0].wakeup.id);
    assert_eq!(wakeup.scheduled_at, due + MINUTE);
    let occurrences = trigger
        .list_occurrences(&schedule.workspace_id, &schedule.id, None, 10)
        .await
        .expect("occurrence history");
    assert_eq!(occurrences.len(), 2);
}

#[tokio::test]
async fn an_unknown_schedule_has_nothing_to_run() {
    let (trigger, workspace_id, _, _) = setup().await;

    let wakeup = trigger
        .run_now(
            &workspace_id,
            &pagis_core::ScheduleId::from("sch_missing".to_string()),
            NOW,
        )
        .await
        .expect("run now");

    assert!(wakeup.is_none());
}
