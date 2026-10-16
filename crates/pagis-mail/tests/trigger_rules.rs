//! The mail seams over the Trigger module, on native and wasm targets.

use super::*;
use pagis_core::{
    Channel, ChannelId, ChannelKind, ChannelParticipant, EventCatalog, EventDeclaration,
    EventMatcher, EventSource, EventSubscriptionState, IngestBatch, NormalizedEvent, ParticipantId,
    ParticipantKind,
};
use pagis_mail::{
    DeferredStandingRule, MAIL_MATCHER, MAIL_MESSAGE_RECEIVED, MailIngest, MailMessageMatcher,
    STANDING_MAIL_INSTRUCTION, STANDING_MAIL_RULE_NAME, StandingMailRule, TriggerIngest,
};
use pagis_trigger::{Trigger, TriggerDeps};

struct Catalog;

impl EventCatalog for Catalog {
    fn declaration(&self, _: &WorkspaceId, name: &str, provider: &str) -> Option<EventDeclaration> {
        if name != MAIL_MESSAGE_RECEIVED || provider != MIGADU_PROVIDER {
            return None;
        }
        let kind = pagis_broker::message_received(provider);
        Some(EventDeclaration {
            name: kind.name,
            metadata_schema: kind.metadata_schema,
            filter_schema: kind.filter_schema,
            required_capability: kind.required_capability,
            matcher: kind.matcher,
            provider: provider.into(),
            source_version: "mail-1".into(),
            source_resource: kind.source_resource,
        })
    }
}

pub(super) fn trigger(stores: &Stores, database: &FakeDatabase, org: WorkspaceId) -> Arc<Trigger> {
    Arc::new(Trigger::new(TriggerDeps {
        schedules: stores.schedules.clone(),
        store: stores.triggers.clone(),
        subscriptions: stores.subscriptions.clone(),
        connections: stores.connections.clone(),
        agents: stores.agents.clone(),
        channels: stores.channels.clone(),
        messages: stores.messages.clone(),
        org_workspace_id: org,
        grants: stores.grants.clone(),
        catalog: Arc::new(Catalog),
        matchers: std::collections::HashMap::from([(
            MAIL_MATCHER.into(),
            Arc::new(MailMessageMatcher) as Arc<dyn EventMatcher>,
        )]),
        events: Arc::new(database.bus()),
        forget_keys: Arc::new(pagis_core::fake::FakeForgetKeys::default()),
    }))
}

async fn user_channel(world: &World) -> ChannelId {
    let channel = Channel {
        id: ChannelId::generate(),
        workspace_id: world.workspace.clone(),
        kind: ChannelKind::Dm,
        title: None,
        created_at: 0,
        updated_at: 0,
    };
    world.stores.channels.create(&channel).await.unwrap();
    for (kind, agent_id) in [
        (ParticipantKind::Person, None),
        (ParticipantKind::Agent, Some(world.agent.clone())),
    ] {
        world
            .stores
            .participants
            .create(&ChannelParticipant {
                person_id: if kind == ParticipantKind::Person {
                    Some(
                        world
                            .stores
                            .workspaces
                            .get(&world.workspace)
                            .await
                            .unwrap()
                            .unwrap()
                            .user_id,
                    )
                } else {
                    None
                },
                id: ParticipantId::generate(),
                workspace_id: world.workspace.clone(),
                channel_id: channel.id.clone(),
                kind,
                agent_id,
                joined_at: 0,
            })
            .await
            .unwrap();
    }
    channel.id
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn provisioning_creates_one_user_rule_on_the_own_mailbox_in_the_user_channel() {
    let world = World::with_trigger(true).await;
    let channel = user_channel(&world).await;
    world.make().await;
    let rules = world
        .trigger
        .list_subscriptions(&world.workspace, Some(&world.agent), None, 10)
        .await
        .unwrap();
    assert_eq!(rules.len(), 1);
    let rule = &rules[0];
    assert_eq!(
        rule.source,
        EventSource::connection(world.connection.clone())
    );
    assert_eq!(rule.event_kind, MAIL_MESSAGE_RECEIVED);
    assert_eq!(rule.filter, serde_json::json!({"mailbox": "own"}));
    assert_eq!(rule.channel_id, channel);
    assert_eq!(rule.root_message_id, None);
    assert_eq!(rule.created_by, pagis_core::Author::System);
    assert_eq!(rule.revision, 1);
    assert_eq!(rule.state, EventSubscriptionState::Active);
    assert_eq!(rule.name, STANDING_MAIL_RULE_NAME);
    assert_eq!(rule.instruction, STANDING_MAIL_INSTRUCTION);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn an_agent_with_no_user_channel_fails_with_the_message() {
    let world = World::with_trigger(true).await;
    let error = world
        .desk
        .provision(&world.workspace, &world.agent, world.request("ada"))
        .await
        .unwrap_err();
    assert!(
        matches!(error, MailboxError::StandingRule(ref cause) if cause.to_string() == "that Agent has no thread with your Person"),
        "{error}"
    );
    assert!(
        world
            .trigger
            .list_subscriptions(&world.workspace, None, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!world.desk.address_taken("ada@example.com").await.unwrap());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn an_empty_deferred_rule_fails_with_the_message() {
    let world = World::new().await;
    let mailbox = world.make().await;
    let error = DeferredStandingRule::default()
        .create(&mailbox)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "the trigger module is not ready on this daemon"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn trigger_ingest_passes_the_batch_to_the_trigger_module() {
    let world = World::with_trigger(true).await;
    let channel = user_channel(&world).await;
    world.make().await;
    let rule = world
        .trigger
        .list_subscriptions(&world.workspace, None, None, 10)
        .await
        .unwrap()
        .remove(0);
    let at = rule.created_at + 1;
    TriggerIngest(world.trigger.clone())
        .ingest(IngestBatch {
            workspace_id: world.workspace.clone(),
            source: rule.source.clone(),
            agent_id: Some(world.agent.clone()),
            event_kind: MAIL_MESSAGE_RECEIVED.into(),
            cursor: Some("mail-cursor".into()),
            events: ["first", "second"]
                .into_iter()
                .map(|id| NormalizedEvent {
                    provider_event_id: id.into(),
                    metadata: serde_json::json!({"mailbox": "own", "subject": id}),
                    occurred_at: at,
                    landing: None,
                })
                .collect(),
            received_at: at,
            baseline: false,
        })
        .await
        .unwrap();
    let wakeups = world
        .trigger
        .list_subscription_wakeups(&world.workspace, &rule.id, None, 10)
        .await
        .unwrap();
    assert_eq!(wakeups.len(), 1);
    assert_eq!(wakeups[0].source_count, 2);
    assert_eq!(wakeups[0].channel_id, Some(channel));
    let cursor = world
        .trigger
        .cursor(&world.workspace, &world.connection, MAIL_MESSAGE_RECEIVED)
        .await
        .unwrap();
    assert_eq!(cursor.as_deref(), Some("mail-cursor"));
}
