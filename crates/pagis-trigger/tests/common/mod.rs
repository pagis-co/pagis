//! Shared fixtures for the Trigger module's stored-behavior tests.
//!
//! Every test observes the module through its public interface and its
//! public stores. Nothing here reaches a private matcher or a query.

use std::collections::HashMap;
use std::sync::Arc;

use pagis_audit::AuditEventBus;
use pagis_core::{
    AgentId, ChannelId, CodingSessionId, Connection, ConnectionId, ConnectionStore, EventBus,
    EventCatalog, EventDeclaration, EventMatcher, Grant, GrantId, GrantStore, MemorySecretStore,
    MessageId, RunId, ScheduleStore, StoreError, TenantKeys, WorkspaceId,
};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteChannelStore, SqliteConnectionStore, SqliteEventLog,
    SqliteEventSubscriptionStore, SqliteGrantStore, SqliteMessageStore, SqliteScheduleStore,
    SqliteTriggerStore,
};
use pagis_trigger::{Trigger, TriggerDeps};
use sqlx::SqlitePool;

pub const NOW: i64 = 1_800_000_000_000;
pub const TEST_EVENT_KIND: &str = "testmail.message_received";
/// A kind the Agent receives as its own identity, with no Grant, the
/// way an Agent Mailbox does (ADR-0019).
pub const TEST_OWN_EVENT_KIND: &str = "testmail.own_message_received";
pub const TEST_CAPABILITY: &str = "mail_read";
pub const TEST_MATCHER: &str = "testmail";
/// The synced resource whose Source Items the events of
/// [`TEST_EVENT_KIND`] are. The Agent's own kind has none (ADR-0008).
pub const TEST_SOURCE_RESOURCE: &str = "testmail";
/// A kind that Pagis itself raises for a Coding Session, under the
/// provider `pagis` (ADR-0033).
pub const TEST_SESSION_EVENT_KIND: &str = "coding_session.turn_ended";
/// A Pagis kind whose declaration names a capability, which no Coding
/// Session can hold.
pub const TEST_SESSION_GRANTED_KIND: &str = "coding_session.granted";

/// The world one test runs in. Each test binary compiles the whole
/// module and uses the part it needs.
#[allow(dead_code)]
pub struct World {
    pub trigger: Arc<Trigger>,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub channel_id: ChannelId,
    pub connection_id: ConnectionId,
    pub grants: Arc<dyn GrantStore>,
    /// The database behind the stores. A test that needs a second
    /// Agent or a second Channel writes one straight into it.
    pub pool: SqlitePool,
    /// The Tenant Data Keys the Trigger module derives the suppression
    /// key from. A test that forgets an item passes the same keys.
    pub keys: Arc<TenantKeys>,
}

/// Two declared kinds, standing in for a provider manifest: one read
/// under a Grant and one the Agent receives as its own identity.
struct TestCatalog;

impl EventCatalog for TestCatalog {
    fn declaration(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        name: &str,
        provider: &str,
    ) -> Option<EventDeclaration> {
        let (owner, required_capability, source_resource) = match name {
            TEST_EVENT_KIND => (
                "testmail",
                TEST_CAPABILITY,
                Some(TEST_SOURCE_RESOURCE.to_string()),
            ),
            TEST_OWN_EVENT_KIND => ("testmail", "", None),
            TEST_SESSION_EVENT_KIND => ("pagis", "", None),
            TEST_SESSION_GRANTED_KIND => ("pagis", TEST_CAPABILITY, None),
            _ => return None,
        };
        (provider == owner).then(|| EventDeclaration {
            name: name.to_string(),
            metadata_schema: serde_json::json!({"type": "object"}),
            filter_schema: serde_json::json!({
                "type": "object",
                "properties": {"senders": {"type": "array", "items": {"type": "string"}}},
                "additionalProperties": false
            }),
            required_capability: required_capability.to_string(),
            matcher: TEST_MATCHER.to_string(),
            source_version: format!("{owner}-1"),
            provider: owner.to_string(),
            source_resource,
        })
    }
}

/// Matches on the `senders` field, the way a real typed matcher does:
/// an absent field narrows nothing, and values combine with OR.
pub struct TestMatcher;

impl EventMatcher for TestMatcher {
    fn matches(&self, filter: &serde_json::Value, metadata: &serde_json::Value) -> bool {
        let Some(senders) = filter["senders"]
            .as_array()
            .filter(|values| !values.is_empty())
        else {
            return true;
        };
        let from = metadata["from"].as_str().unwrap_or_default();
        senders
            .iter()
            .filter_map(|sender| sender.as_str())
            .any(|sender| sender.eq_ignore_ascii_case(from))
    }
}

pub async fn world() -> World {
    let pool = pagis_storage_sqlite::connect_memory()
        .await
        .expect("connect");
    pagis_storage_sqlite::MIGRATOR
        .run(&pool)
        .await
        .expect("migrate");
    let workspace_id = WorkspaceId::generate();
    let agent_id = AgentId::generate();
    let channel_id = ChannelId::generate();
    seed(&pool, &workspace_id, &agent_id, &channel_id).await;

    let connections = Arc::new(SqliteConnectionStore::new(pool.clone()));
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace_id.clone(),
        provider: "testmail".to_string(),
        alias: "work".to_string(),
        display_name: "Work mail".to_string(),
        status: Connection::CONNECTED.to_string(),
        authorized_capabilities: vec![TEST_CAPABILITY.to_string()],
        config: serde_json::json!({"account": "user@example.com", "client": "work"}),
        created_at: NOW,
    };
    connections.create(&connection).await.expect("connection");

    let grants: Arc<dyn GrantStore> = Arc::new(SqliteGrantStore::new(pool.clone()));
    grants
        .create(&Grant {
            id: GrantId::generate(),
            workspace_id: workspace_id.clone(),
            agent_id: agent_id.clone(),
            resource_kind: Grant::CONNECTION_KIND.to_string(),
            resource_id: Some(connection.id.to_string()),
            scope: Grant::connection_scope(&[TEST_CAPABILITY.to_string()]),
            revision: 1,
            created_at: NOW,
            revoked_at: None,
        })
        .await
        .expect("grant");

    let events: Arc<dyn EventBus> = Arc::new(AuditEventBus::new(Arc::new(SqliteEventLog::new(
        pool.clone(),
    ))));
    let schedules: Arc<dyn ScheduleStore> = Arc::new(SqliteScheduleStore::new(pool.clone()));
    let keys = Arc::new(TenantKeys::new(Arc::new(MemorySecretStore::default())));
    let trigger = Arc::new(Trigger::new(TriggerDeps {
        schedules,
        store: Arc::new(SqliteTriggerStore::new(pool.clone())),
        subscriptions: Arc::new(SqliteEventSubscriptionStore::new(pool.clone())),
        connections: connections as _,
        agents: Arc::new(SqliteAgentStore::new(pool.clone())),
        channels: Arc::new(SqliteChannelStore::new(pool.clone())),
        messages: Arc::new(SqliteMessageStore::new(pool.clone())),
        org_workspace_id: pagis_core::WorkspaceId::generate(),
        grants: Arc::clone(&grants),
        catalog: Arc::new(TestCatalog),
        matchers: HashMap::from([(
            TEST_MATCHER.to_string(),
            Arc::new(TestMatcher) as Arc<dyn EventMatcher>,
        )]),
        events,
        forget_keys: Arc::clone(&keys) as _,
    }));
    World {
        trigger,
        pool,
        keys,
        workspace_id,
        agent_id,
        channel_id,
        connection_id: connection.id,
        grants,
    }
}

/// A Coding Session of the world's Agent, in its own Channel, with the
/// Run that started it. It runs in the Agent's Computer, so it names no
/// Host.
#[allow(dead_code)]
pub async fn coding_session(world: &World) -> CodingSessionId {
    let run_id = RunId::generate();
    sqlx::query(
        "INSERT INTO runs (id, workspace_id, agent_id, trigger_kind, state, created_at) \
         VALUES (?, ?, ?, 'message', 'completed', ?)",
    )
    .bind(run_id.as_str())
    .bind(world.workspace_id.as_str())
    .bind(world.agent_id.as_str())
    .bind(NOW)
    .execute(&world.pool)
    .await
    .expect("run");
    let id = CodingSessionId::generate();
    sqlx::query(
        "INSERT INTO coding_sessions (id, workspace_id, agent_id, harness_id, harness_version, \
         place, directory, approval_mode, title, state, channel_id, root_message_id, \
         message_id, run_id, created_at, updated_at) \
         VALUES (?, ?, ?, 'claude', '1.0.0', 'computer', '/work', 'auto', 'Fix it', 'working', \
         ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.as_str())
    .bind(world.workspace_id.as_str())
    .bind(world.agent_id.as_str())
    .bind(world.channel_id.as_str())
    .bind(MessageId::generate().as_str())
    .bind(MessageId::generate().as_str())
    .bind(run_id.as_str())
    .bind(NOW)
    .bind(NOW)
    .execute(&world.pool)
    .await
    .expect("coding session");
    id
}

/// Revoke the Agent's live grant on the Connection.
#[allow(dead_code)]
pub async fn revoke_grant(world: &World) -> Result<(), StoreError> {
    let grant = world
        .grants
        .live_for_resource(
            &world.workspace_id,
            &world.agent_id,
            Grant::CONNECTION_KIND,
            world.connection_id.as_str(),
        )
        .await?
        .expect("live grant");
    world
        .grants
        .revoke(&world.workspace_id, &grant.id, NOW)
        .await?;
    Ok(())
}

/// Give the Agent its Connection grant back.
#[allow(dead_code)]
pub async fn restore_grant(world: &World, at: i64) -> Result<(), StoreError> {
    world
        .grants
        .create(&Grant {
            id: GrantId::generate(),
            workspace_id: world.workspace_id.clone(),
            agent_id: world.agent_id.clone(),
            resource_kind: Grant::CONNECTION_KIND.to_string(),
            resource_id: Some(world.connection_id.to_string()),
            scope: Grant::connection_scope(&[TEST_CAPABILITY.to_string()]),
            revision: 2,
            created_at: at,
            revoked_at: None,
        })
        .await
}

/// An Agent, a Channel and a top-level message of that Channel, in
/// one Workspace: everything a rule can name as its target.
#[allow(dead_code)]
pub struct Target {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub channel_id: ChannelId,
    pub root_message_id: MessageId,
}

/// A second Workspace, with an Agent, a Channel and a Thread root of
/// its own.
#[allow(dead_code)]
pub async fn other_workspace(world: &World) -> Target {
    let workspace_id = WorkspaceId::generate();
    let agent_id = AgentId::generate();
    let channel_id = ChannelId::generate();
    seed(&world.pool, &workspace_id, &agent_id, &channel_id).await;
    let root_message_id = message(&world.pool, &workspace_id, &channel_id, None).await;
    Target {
        workspace_id,
        agent_id,
        channel_id,
        root_message_id,
    }
}

/// A top-level message in the world's own Channel.
#[allow(dead_code)]
pub async fn thread_root(world: &World) -> MessageId {
    message(&world.pool, &world.workspace_id, &world.channel_id, None).await
}

/// A second Channel of the world's own Workspace, with a top-level
/// message in it.
#[allow(dead_code)]
pub async fn other_channel(world: &World) -> (ChannelId, MessageId) {
    let channel_id = ChannelId::generate();
    sqlx::query(
        "INSERT INTO channels (id, workspace_id, kind, title, created_at, updated_at) \
         VALUES (?, ?, 'group', ?, ?, ?)",
    )
    .bind(channel_id.as_str())
    .bind(world.workspace_id.as_str())
    .bind("Planning")
    .bind(NOW)
    .bind(NOW)
    .execute(&world.pool)
    .await
    .expect("channel");
    let root = message(&world.pool, &world.workspace_id, &channel_id, None).await;
    (channel_id, root)
}

/// One message of the user in a Channel: a top-level message, or a
/// reply under `parent`.
#[allow(dead_code)]
pub async fn message(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
    channel_id: &ChannelId,
    parent: Option<&MessageId>,
) -> MessageId {
    let id = MessageId::generate();
    sqlx::query(
        "INSERT INTO messages \
         (id, workspace_id, channel_id, parent_message_id, author_kind, status, blocks, \
          text_content, created_at, completed_at) \
         VALUES (?, ?, ?, ?, 'user', 'complete', '[]', ?, ?, ?)",
    )
    .bind(id.as_str())
    .bind(workspace_id.as_str())
    .bind(channel_id.as_str())
    .bind(parent.map(MessageId::as_str))
    .bind("Let us talk here")
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("message");
    id
}

async fn seed(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
    agent_id: &AgentId,
    channel_id: &ChannelId,
) {
    sqlx::query("INSERT INTO workspaces (id, name, timezone, created_at) VALUES (?, ?, ?, ?)")
        .bind(workspace_id.as_str())
        .bind("Test")
        .bind("America/Los_Angeles")
        .bind(NOW)
        .execute(pool)
        .await
        .expect("workspace");
    sqlx::query(
        "INSERT INTO model_aliases (id, workspace_id, alias, candidates, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(format!("model-{workspace_id}"))
    .bind(workspace_id.as_str())
    .bind("default")
    .bind("[]")
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("model alias");
    sqlx::query(
        "INSERT INTO agents (id, workspace_id, name, job, personality, model_alias, status, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?)",
    )
    .bind(agent_id.as_str())
    .bind(workspace_id.as_str())
    .bind("Sage")
    .bind("assistant")
    .bind("plain")
    .bind("default")
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("agent");
    sqlx::query(
        "INSERT INTO channels (id, workspace_id, kind, title, created_at, updated_at) \
         VALUES (?, ?, 'dm', ?, ?, ?)",
    )
    .bind(channel_id.as_str())
    .bind(workspace_id.as_str())
    .bind("Sage")
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("channel");
}
