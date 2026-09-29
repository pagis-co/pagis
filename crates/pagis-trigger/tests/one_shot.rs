use crate::common;

use pagis_core::{AgentId, ChannelId, WorkspaceId};
use pagis_trigger::{NewOneShot, Trigger};

use common::NOW;

async fn setup() -> (std::sync::Arc<Trigger>, WorkspaceId, AgentId, ChannelId) {
    let world = common::world().await;
    (
        world.trigger,
        world.workspace_id,
        world.agent_id,
        world.channel_id,
    )
}

fn input(workspace_id: WorkspaceId, agent_id: AgentId, channel_id: ChannelId) -> NewOneShot {
    NewOneShot {
        workspace_id,
        agent_id,
        name: "Morning plan".to_string(),
        instruction: "Prepare today's plan".to_string(),
        channel_id,
        root_message_id: None,
        local_time: "2027-01-15T09:00:00".to_string(),
        timezone: "America/Los_Angeles".to_string(),
        now: NOW,
    }
}

#[tokio::test]
async fn a_late_one_shot_creates_and_claims_one_wakeup_once() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let schedule = trigger
        .create_one_shot(input(workspace_id.clone(), agent_id.clone(), channel_id))
        .await
        .expect("create one-shot");

    let due_at = schedule.next_due_at.expect("future due time");
    let first = trigger.process_due(due_at + 60_000).await.expect("process");
    let second = trigger.process_due(due_at + 60_000).await.expect("repeat");
    assert_eq!(first.created_occurrences, 1);
    assert_eq!(first.created_wakeups, 1);
    assert_eq!(second.created_occurrences, 0);
    assert_eq!(second.created_wakeups, 0);

    let claims = trigger
        .claim_wakeups(
            &workspace_id,
            &agent_id,
            pagis_core::RunSlots {
                conversation: 1,
                arrival: 0,
            },
            due_at + 60_000,
        )
        .await
        .expect("claim");
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].wakeup.rule.schedule_id(), Some(&schedule.id));
    assert_eq!(
        claims[0].run.trigger_ref.as_deref(),
        Some(claims[0].wakeup.id.as_str())
    );
    assert!(
        trigger
            .claim_wakeups(
                &workspace_id,
                &agent_id,
                pagis_core::RunSlots {
                    conversation: 1,
                    arrival: 0
                },
                due_at + 60_000
            )
            .await
            .expect("claim again")
            .is_empty()
    );
}

#[tokio::test]
async fn invalid_timezone_and_past_instants_are_rejected() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let mut invalid = input(workspace_id.clone(), agent_id.clone(), channel_id.clone());
    invalid.timezone = "Mars/Olympus".to_string();
    assert!(matches!(
        trigger.create_one_shot(invalid).await,
        Err(pagis_trigger::TriggerError::InvalidTimezone(_))
    ));

    let mut past = input(workspace_id.clone(), agent_id, channel_id);
    past.local_time = "2020-01-01T09:00:00".to_string();
    assert!(matches!(
        trigger.create_one_shot(past).await,
        Err(pagis_trigger::TriggerError::PastInstant)
    ));
    assert!(
        trigger
            .list_schedules(&workspace_id, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn archiving_before_due_withdraws_the_one_shot() {
    let (trigger, workspace_id, agent_id, channel_id) = setup().await;
    let schedule = trigger
        .create_one_shot(input(workspace_id, agent_id, channel_id))
        .await
        .unwrap();
    trigger
        .archive_schedule(&schedule.workspace_id, &schedule.id, NOW + 1)
        .await
        .unwrap();

    let processed = trigger
        .process_due(schedule.scheduled_at + 60_000)
        .await
        .unwrap();
    assert_eq!(processed.created_occurrences, 0);
    assert_eq!(processed.created_wakeups, 0);
    assert_eq!(
        trigger
            .get_schedule(&schedule.workspace_id, &schedule.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        pagis_core::ScheduleState::Archived
    );
}
