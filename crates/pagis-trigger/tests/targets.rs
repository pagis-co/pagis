//! The target of a Schedule or an Event Subscription (ADR-0006).
//!
//! Every create and every edit of both rules goes through one target
//! check. In the rule's Workspace the Agent exists and is Active, the
//! Channel exists, and a Thread root is a top-level message of that
//! Channel. An id of another Workspace gets the same error as an id
//! that names nothing, and a refused call stores nothing.

use crate::common;

use pagis_core::{
    AgentId, ChannelId, CreatorKind, EventSubscription, MessageId, Schedule, WorkspaceId,
};
use pagis_trigger::{
    NewSchedule, NewSubscription, ScheduleEdit, ScheduleTiming, SubscriptionAction, TriggerError,
};

use common::{NOW, TEST_EVENT_KIND, TEST_OWN_EVENT_KIND, World};

fn new_schedule(
    world: &World,
    agent_id: &AgentId,
    channel_id: &ChannelId,
    root_message_id: Option<&MessageId>,
) -> NewSchedule {
    NewSchedule {
        workspace_id: world.workspace_id.clone(),
        agent_id: agent_id.clone(),
        name: "Morning plan".to_string(),
        instruction: "Write the plan".to_string(),
        subject_page_path: None,
        channel_id: channel_id.clone(),
        root_message_id: root_message_id.cloned(),
        timing: cron(),
        creator: CreatorKind::User,
        creating_run_id: None,
        now: NOW,
    }
}

fn cron() -> ScheduleTiming {
    ScheduleTiming::Cron {
        expression: "0 7 * * *".to_string(),
        timezone: "UTC".to_string(),
    }
}

fn edit(
    schedule: &Schedule,
    agent_id: &AgentId,
    channel_id: &ChannelId,
    root_message_id: Option<Option<&MessageId>>,
) -> ScheduleEdit {
    ScheduleEdit {
        expected_revision: schedule.revision,
        agent_id: agent_id.clone(),
        name: schedule.name.clone(),
        instruction: schedule.instruction.clone(),
        channel_id: channel_id.clone(),
        root_message_id: root_message_id.map(|root| root.cloned()),
        timing: cron(),
    }
}

fn new_subscription(
    world: &World,
    event_kind: &str,
    agent_id: &AgentId,
    channel_id: &ChannelId,
    root_message_id: Option<&MessageId>,
) -> NewSubscription {
    NewSubscription {
        workspace_id: world.workspace_id.clone(),
        agent_id: agent_id.clone(),
        connection_id: world.connection_id.clone(),
        event_kind: event_kind.to_string(),
        name: "Invoices".to_string(),
        instruction: "File the invoice".to_string(),
        channel_id: channel_id.clone(),
        root_message_id: root_message_id.cloned(),
        filter: serde_json::json!({}),
        creator: CreatorKind::User,
        now: NOW,
    }
}

fn move_to(
    channel_id: Option<&ChannelId>,
    root_message_id: Option<Option<&MessageId>>,
) -> SubscriptionAction {
    SubscriptionAction::Edit {
        name: None,
        instruction: None,
        channel_id: channel_id.cloned(),
        root_message_id: root_message_id.map(|root| root.cloned()),
        filter: None,
    }
}

/// An Agent of the world's own Workspace that is archived.
async fn archived_agent(world: &World) -> AgentId {
    let agent_id = AgentId::generate();
    sqlx::query(
        "INSERT INTO agents (id, workspace_id, name, job, personality, model_alias, status, created_at, updated_at) \
         VALUES (?, ?, 'Old', 'assistant', 'plain', 'default', 'archived', ?, ?)",
    )
    .bind(agent_id.as_str())
    .bind(world.workspace_id.as_str())
    .bind(NOW)
    .bind(NOW)
    .execute(&world.pool)
    .await
    .expect("archived agent");
    agent_id
}

async fn schedules_of(world: &World, workspace_id: &WorkspaceId) -> Vec<Schedule> {
    world
        .trigger
        .list_schedules(workspace_id, None, 100)
        .await
        .expect("list Schedules")
}

async fn subscriptions_of(world: &World, workspace_id: &WorkspaceId) -> Vec<EventSubscription> {
    world
        .trigger
        .list_subscriptions(workspace_id, None, None, 100)
        .await
        .expect("list Event Subscriptions")
}

#[tokio::test]
async fn a_schedule_is_created_only_for_a_target_of_its_own_workspace() {
    let world = common::world().await;
    let other = common::other_workspace(&world).await;
    let (_, other_channel_root) = common::other_channel(&world).await;
    let root = common::thread_root(&world).await;
    let reply = common::message(
        &world.pool,
        &world.workspace_id,
        &world.channel_id,
        Some(&root),
    )
    .await;
    let archived = archived_agent(&world).await;

    for (case, input, expected) in [
        (
            "an Agent of another Workspace",
            new_schedule(&world, &other.agent_id, &world.channel_id, None),
            "agent not found",
        ),
        (
            "an archived Agent",
            new_schedule(&world, &archived, &world.channel_id, None),
            "agent not found",
        ),
        (
            "a Channel of another Workspace",
            new_schedule(&world, &world.agent_id, &other.channel_id, None),
            "channel not found",
        ),
        (
            "a Thread root of another Workspace",
            new_schedule(
                &world,
                &world.agent_id,
                &world.channel_id,
                Some(&other.root_message_id),
            ),
            "thread root not found",
        ),
        (
            "a Thread root of another Channel",
            new_schedule(
                &world,
                &world.agent_id,
                &world.channel_id,
                Some(&other_channel_root),
            ),
            "thread root not found",
        ),
        (
            "a reply as the Thread root",
            new_schedule(&world, &world.agent_id, &world.channel_id, Some(&reply)),
            "thread root not found",
        ),
    ] {
        let error = world.trigger.create_schedule(input).await.expect_err(case);
        assert_eq!(error.to_string(), expected, "{case}");
    }
    assert!(schedules_of(&world, &world.workspace_id).await.is_empty());
    assert!(schedules_of(&world, &other.workspace_id).await.is_empty());

    let schedule = world
        .trigger
        .create_schedule(new_schedule(
            &world,
            &world.agent_id,
            &world.channel_id,
            Some(&root),
        ))
        .await
        .expect("a target of the Workspace makes a Schedule");
    assert_eq!(schedule.root_message_id, Some(root));
}

#[tokio::test]
async fn a_schedule_edit_to_a_target_outside_its_workspace_changes_nothing() {
    let world = common::world().await;
    let other = common::other_workspace(&world).await;
    let (_, other_channel_root) = common::other_channel(&world).await;
    let schedule = world
        .trigger
        .create_schedule(new_schedule(
            &world,
            &world.agent_id,
            &world.channel_id,
            None,
        ))
        .await
        .expect("create Schedule");

    for (case, change, expected) in [
        (
            "an Agent of another Workspace",
            edit(&schedule, &other.agent_id, &world.channel_id, None),
            "agent not found",
        ),
        (
            "a Channel of another Workspace",
            edit(&schedule, &world.agent_id, &other.channel_id, None),
            "channel not found",
        ),
        (
            "a Thread root of another Workspace",
            edit(
                &schedule,
                &world.agent_id,
                &world.channel_id,
                Some(Some(&other.root_message_id)),
            ),
            "thread root not found",
        ),
        (
            "a Thread root of another Channel of the Workspace",
            edit(
                &schedule,
                &world.agent_id,
                &world.channel_id,
                Some(Some(&other_channel_root)),
            ),
            "thread root not found",
        ),
    ] {
        let error = world
            .trigger
            .edit_schedule(&world.workspace_id, &schedule.id, change, NOW + 1)
            .await
            .expect_err(case);
        assert_eq!(error.to_string(), expected, "{case}");
    }

    let stored = world
        .trigger
        .get_schedule(&world.workspace_id, &schedule.id)
        .await
        .expect("read Schedule");
    assert_eq!(stored, Some(schedule.clone()));
    let revisions = world
        .trigger
        .list_revisions(&world.workspace_id, &schedule.id)
        .await
        .expect("revisions");
    assert_eq!(revisions.len(), 1, "a refused edit writes no revision");
}

#[tokio::test]
async fn a_schedule_edit_to_another_channel_drops_the_thread() {
    let world = common::world().await;
    let (other_channel, _) = common::other_channel(&world).await;
    let root = common::thread_root(&world).await;
    let schedule = world
        .trigger
        .create_schedule(new_schedule(
            &world,
            &world.agent_id,
            &world.channel_id,
            Some(&root),
        ))
        .await
        .expect("create Schedule");

    let renamed = world
        .trigger
        .edit_schedule(
            &world.workspace_id,
            &schedule.id,
            edit(&schedule, &world.agent_id, &world.channel_id, None),
            NOW + 1,
        )
        .await
        .expect("edit in the same Channel")
        .expect("the Schedule exists");
    assert_eq!(
        renamed.root_message_id,
        Some(root),
        "an edit that stays in the Channel keeps the Thread"
    );

    let moved = world
        .trigger
        .edit_schedule(
            &world.workspace_id,
            &schedule.id,
            edit(&renamed, &world.agent_id, &other_channel, None),
            NOW + 2,
        )
        .await
        .expect("move to another Channel")
        .expect("the Schedule exists");

    assert_eq!(moved.channel_id, other_channel);
    assert_eq!(moved.root_message_id, None);
}

#[tokio::test]
async fn a_schedule_edit_can_clear_the_thread() {
    let world = common::world().await;
    let root = common::thread_root(&world).await;
    let schedule = world
        .trigger
        .create_schedule(new_schedule(
            &world,
            &world.agent_id,
            &world.channel_id,
            Some(&root),
        ))
        .await
        .expect("create Schedule");

    let cleared = world
        .trigger
        .edit_schedule(
            &world.workspace_id,
            &schedule.id,
            edit(&schedule, &world.agent_id, &world.channel_id, Some(None)),
            NOW + 1,
        )
        .await
        .expect("clear the Thread")
        .expect("the Schedule exists");

    assert_eq!(cleared.channel_id, world.channel_id);
    assert_eq!(cleared.root_message_id, None);
}

#[tokio::test]
async fn an_event_subscription_is_created_only_for_a_target_of_its_own_workspace() {
    let world = common::world().await;
    let other = common::other_workspace(&world).await;
    let (_, other_channel_root) = common::other_channel(&world).await;
    let root = common::thread_root(&world).await;

    for (case, input, expected) in [
        (
            "an Agent of another Workspace",
            new_subscription(
                &world,
                TEST_EVENT_KIND,
                &other.agent_id,
                &world.channel_id,
                None,
            ),
            "agent not found",
        ),
        (
            "a Channel of another Workspace",
            new_subscription(
                &world,
                TEST_EVENT_KIND,
                &world.agent_id,
                &other.channel_id,
                None,
            ),
            "channel not found",
        ),
        (
            "a Thread root of another Workspace",
            new_subscription(
                &world,
                TEST_EVENT_KIND,
                &world.agent_id,
                &world.channel_id,
                Some(&other.root_message_id),
            ),
            "thread root not found",
        ),
        (
            "a Thread root of another Channel",
            new_subscription(
                &world,
                TEST_EVENT_KIND,
                &world.agent_id,
                &world.channel_id,
                Some(&other_channel_root),
            ),
            "thread root not found",
        ),
    ] {
        let error = world
            .trigger
            .create_subscription(input)
            .await
            .expect_err(case);
        assert_eq!(error.to_string(), expected, "{case}");
    }
    assert!(
        subscriptions_of(&world, &world.workspace_id)
            .await
            .is_empty()
    );

    let subscription = world
        .trigger
        .create_subscription(new_subscription(
            &world,
            TEST_EVENT_KIND,
            &world.agent_id,
            &world.channel_id,
            Some(&root),
        ))
        .await
        .expect("a target of the Workspace makes an Event Subscription");
    assert_eq!(subscription.root_message_id, Some(root));
}

/// A kind that needs no capability needs no Grant, so the Grant check
/// accepts any Agent. The target check still refuses an Agent of
/// another Workspace.
#[tokio::test]
async fn a_kind_with_no_capability_refuses_an_agent_of_another_workspace() {
    let world = common::world().await;
    let other = common::other_workspace(&world).await;

    let refused = world
        .trigger
        .create_subscription(new_subscription(
            &world,
            TEST_OWN_EVENT_KIND,
            &other.agent_id,
            &world.channel_id,
            None,
        ))
        .await;

    assert!(
        matches!(refused, Err(TriggerError::AgentNotFound)),
        "{refused:?}"
    );
    assert!(
        subscriptions_of(&world, &world.workspace_id)
            .await
            .is_empty()
    );
    world
        .trigger
        .create_subscription(new_subscription(
            &world,
            TEST_OWN_EVENT_KIND,
            &world.agent_id,
            &world.channel_id,
            None,
        ))
        .await
        .expect("the Workspace's own Agent needs no Grant for this kind");
}

#[tokio::test]
async fn an_event_subscription_edit_to_a_target_outside_its_workspace_changes_nothing() {
    let world = common::world().await;
    let other = common::other_workspace(&world).await;
    let (_, other_channel_root) = common::other_channel(&world).await;
    let subscription = world
        .trigger
        .create_subscription(new_subscription(
            &world,
            TEST_EVENT_KIND,
            &world.agent_id,
            &world.channel_id,
            None,
        ))
        .await
        .expect("create Event Subscription");

    for (case, action, expected) in [
        (
            "a Channel of another Workspace",
            move_to(Some(&other.channel_id), None),
            "channel not found",
        ),
        (
            "a Thread root of another Workspace",
            move_to(None, Some(Some(&other.root_message_id))),
            "thread root not found",
        ),
        (
            "a Thread root of another Channel of the Workspace",
            move_to(None, Some(Some(&other_channel_root))),
            "thread root not found",
        ),
    ] {
        let error = world
            .trigger
            .update_subscription(&world.workspace_id, &subscription.id, action, NOW + 1)
            .await
            .expect_err(case);
        assert_eq!(error.to_string(), expected, "{case}");
    }

    let stored = world
        .trigger
        .get_subscription(&world.workspace_id, &subscription.id)
        .await
        .expect("read Event Subscription");
    assert_eq!(stored, Some(subscription));
}

#[tokio::test]
async fn an_event_subscription_edit_to_another_channel_drops_the_thread() {
    let world = common::world().await;
    let (other_channel, _) = common::other_channel(&world).await;
    let root = common::thread_root(&world).await;
    let subscription = world
        .trigger
        .create_subscription(new_subscription(
            &world,
            TEST_EVENT_KIND,
            &world.agent_id,
            &world.channel_id,
            Some(&root),
        ))
        .await
        .expect("create Event Subscription");

    let moved = world
        .trigger
        .update_subscription(
            &world.workspace_id,
            &subscription.id,
            move_to(Some(&other_channel), None),
            NOW + 1,
        )
        .await
        .expect("move to another Channel");

    assert_eq!(moved.channel_id, other_channel);
    assert_eq!(moved.root_message_id, None);
}

#[tokio::test]
async fn an_event_subscription_edit_can_clear_the_thread() {
    let world = common::world().await;
    let root = common::thread_root(&world).await;
    let subscription = world
        .trigger
        .create_subscription(new_subscription(
            &world,
            TEST_EVENT_KIND,
            &world.agent_id,
            &world.channel_id,
            Some(&root),
        ))
        .await
        .expect("create Event Subscription");

    let cleared = world
        .trigger
        .update_subscription(
            &world.workspace_id,
            &subscription.id,
            move_to(None, Some(None)),
            NOW + 1,
        )
        .await
        .expect("clear the Thread");

    assert_eq!(cleared.channel_id, world.channel_id);
    assert_eq!(cleared.root_message_id, None);
}
