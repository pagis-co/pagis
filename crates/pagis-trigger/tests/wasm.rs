#![cfg(target_arch = "wasm32")]

use std::collections::HashMap;
use std::sync::Arc;

use pagis_core::fake::{FakeDatabase, FakeForgetKeys};
use pagis_core::{
    Agent, AgentId, AgentStatus, Channel, ChannelId, ChannelKind, Connection, ConnectionId,
    EventCatalog, EventDeclaration, EventMatcher, EventSource, EventSubscriptionState, IngestBatch,
    NormalizedEvent, WakeupState, WorkspaceId,
};
use pagis_trigger::{NewSubscription, Trigger, TriggerDeps};
use wasm_bindgen_test::wasm_bindgen_test;

struct Catalog;

impl EventCatalog for Catalog {
    fn declaration(&self, _: &WorkspaceId, name: &str, provider: &str) -> Option<EventDeclaration> {
        (name == "test.message_received" && provider == "test").then(|| EventDeclaration {
            name: name.into(),
            provider: provider.into(),
            source_version: "test-1".into(),
            required_capability: String::new(),
            source_resource: None,
            matcher: "test".into(),
            metadata_schema: serde_json::json!({"type": "object"}),
            filter_schema: serde_json::json!({"type": "object", "additionalProperties": false}),
        })
    }
}

struct Matcher;

impl EventMatcher for Matcher {
    fn matches(&self, _: &serde_json::Value, _: &serde_json::Value) -> bool {
        true
    }
}

#[wasm_bindgen_test]
async fn a_subscription_and_an_ingested_batch_create_one_wakeup() {
    let database = FakeDatabase::default();
    let stores = database.stores();
    let workspace = WorkspaceId::generate();
    let agent = Agent {
        id: AgentId::generate(),
        workspace_id: workspace.clone(),
        name: "Ada".into(),
        job: "assistant".into(),
        description: String::new(),
        personality: String::new(),
        model_alias: "default".into(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: 0,
        updated_at: 0,
    };
    stores.agents.create(&agent).await.unwrap();
    let channel = Channel {
        id: ChannelId::generate(),
        workspace_id: workspace.clone(),
        kind: ChannelKind::Dm,
        title: None,
        created_at: 0,
        updated_at: 0,
    };
    stores.channels.create(&channel).await.unwrap();
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace.clone(),
        provider: "test".into(),
        alias: "inbox".into(),
        display_name: "Inbox".into(),
        status: Connection::CONNECTED.into(),
        authorized_capabilities: vec![],
        config: serde_json::json!({}),
        created_at: 0,
    };
    stores.connections.create(&connection).await.unwrap();
    let trigger = Trigger::new(TriggerDeps {
        schedules: stores.schedules.clone(),
        store: stores.triggers.clone(),
        subscriptions: stores.subscriptions.clone(),
        connections: stores.connections.clone(),
        agents: stores.agents.clone(),
        channels: stores.channels.clone(),
        messages: stores.messages.clone(),
        org_workspace_id: WorkspaceId::generate(),
        grants: stores.grants.clone(),
        catalog: Arc::new(Catalog),
        matchers: HashMap::from([("test".into(), Arc::new(Matcher) as Arc<dyn EventMatcher>)]),
        events: Arc::new(database.bus()),
        forget_keys: Arc::new(FakeForgetKeys::default()),
    });
    let rule = trigger
        .create_subscription(NewSubscription {
            created_by: pagis_core::Author::System,
            workspace_id: workspace.clone(),
            agent_id: agent.id.clone(),
            source: EventSource::connection(connection.id),
            event_kind: "test.message_received".into(),
            name: "Inbox".into(),
            instruction: "Read the messages".into(),
            channel_id: channel.id.clone(),
            root_message_id: None,
            filter: serde_json::json!({}),
            now: 100,
        })
        .await
        .unwrap();
    assert_eq!(rule.state, EventSubscriptionState::Active);
    let outcome = trigger
        .ingest(IngestBatch {
            workspace_id: workspace.clone(),
            source: rule.source.clone(),
            agent_id: Some(agent.id),
            event_kind: rule.event_kind.clone(),
            cursor: Some("cursor-1".into()),
            events: ["first", "second"]
                .into_iter()
                .map(|id| NormalizedEvent {
                    provider_event_id: id.into(),
                    metadata: serde_json::json!({"subject": id}),
                    occurred_at: 101,
                    landing: None,
                })
                .collect(),
            received_at: 102,
            baseline: false,
        })
        .await
        .unwrap();
    assert_eq!(outcome.events.len(), 2);
    assert_eq!(outcome.created.len(), 1);
    let wakeup = trigger
        .get_wakeup(&workspace, &outcome.created[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wakeup.source_count, 2);
    assert_eq!(outcome.created[0].state, WakeupState::Pending);
    assert_eq!(outcome.created[0].channel_id, Some(channel.id));
    let cursor = trigger
        .cursor(
            &workspace,
            rule.source.connection_id().unwrap(),
            &rule.event_kind,
        )
        .await
        .unwrap();
    assert_eq!(cursor.as_deref(), Some("cursor-1"));
    let events = trigger
        .list_subscription_events(&workspace, &rule.id, None, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
}
