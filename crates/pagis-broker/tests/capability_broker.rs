use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_audit::AuditEventBus;
use pagis_broker::{
    AuthorizedCall, Broker, BrokerDeps, CapabilityManifest, EffectClass, ManifestError,
    ManifestTool, ToolDef, ToolExecutor, ToolResult, ToolRoute,
};
use pagis_core::{
    Agent, AgentId, AgentStatus, AgentStore, CapabilitySnapshotStore, Channel, ChannelId,
    ChannelKind, ChannelStore, EventBus, EventLog, Grant, GrantId, GrantStore, MessageId,
    RequestState, RequestStore, Run, RunId, RunState, RunStore, TriggerKind, Workspace,
    WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentMailboxStore, SqliteAgentStore, SqliteCapabilitySnapshotStore, SqliteChannelStore,
    SqliteConnectionStore, SqliteEventLog, SqliteGrantStore, SqlitePhoneNumberStore,
    SqliteRequestStore, SqliteRunStore, SqliteWorkspaceStore,
};
use sqlx::SqlitePool;

async fn workspace(pool: &SqlitePool) -> Workspace {
    let person =
        pagis_storage_sqlite::seed_org_and_administrator(pool, "Org", pagis_core::now_ms())
            .await
            .unwrap();
    Workspace {
        user_id: person.id,
        id: WorkspaceId::generate(),
        name: "Workspace".to_string(),
        timezone: "UTC".to_string(),
        created_at: now_ms(),
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    }
}

fn agent(workspace_id: &WorkspaceId) -> Agent {
    Agent {
        id: AgentId::generate(),
        workspace_id: workspace_id.clone(),
        name: "Sage".to_string(),
        job: "general assistant".to_string(),
        description: String::new(),
        personality: "warm, direct".to_string(),
        model_alias: "default".to_string(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: now_ms(),
        updated_at: now_ms(),
    }
}

fn channel(workspace_id: &WorkspaceId) -> Channel {
    Channel {
        id: ChannelId::generate(),
        workspace_id: workspace_id.clone(),
        kind: ChannelKind::Dm,
        title: Some("general".to_string()),
        created_at: now_ms(),
        updated_at: now_ms(),
    }
}

fn queued_run(workspace_id: &WorkspaceId, agent_id: &AgentId, channel_id: &ChannelId) -> Run {
    Run {
        id: RunId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        channel_id: Some(channel_id.clone()),
        root_message_id: None,
        trigger_kind: TriggerKind::Message,
        trigger_ref: Some(MessageId::generate().to_string()),
        hop_count: 0,
        origin: None,
        state: RunState::Queued,
        failure_kind: None,
        dismissed_at: None,
        error: None,
        started_at: None,
        ended_at: None,
        created_at: now_ms(),
    }
}

#[derive(Default)]
pub struct RecordingExecutor {
    pub calls: Mutex<Vec<AuthorizedCall>>,
    /// What the next call answers, when a test scripts it.
    reply: Mutex<Option<ToolResult>>,
}

impl RecordingExecutor {
    fn answers(&self, result: ToolResult) {
        *self.reply.lock().unwrap() = Some(result);
    }
}

#[async_trait]
impl ToolExecutor for RecordingExecutor {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        self.calls.lock().unwrap().push(call);
        self.reply
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| ToolResult::success("ok"))
    }
}

pub struct Harness {
    pub broker: Broker,
    pub executor: Arc<RecordingExecutor>,
    /// The machines of the workspace, and which of them are connected.
    hosts: Arc<pagis_storage_sqlite::SqliteHostStore>,
    presence: Arc<pagis_broker::HostPresence>,
    pub pool: SqlitePool,
    pub workspace: pagis_core::Workspace,
    pub agent: pagis_core::Agent,
    pub run: pagis_core::Run,
}

impl Harness {
    /// The machines of the installation.
    pub fn hosts(&self) -> &Arc<pagis_storage_sqlite::SqliteHostStore> {
        &self.hosts
    }

    /// Which machines are connected. A test connects one to make it
    /// present, and holds the connection for as long as it wants it.
    pub fn presence(&self) -> &Arc<pagis_broker::HostPresence> {
        &self.presence
    }
}

/// A second Workspace of the same Org, for the tests that prove one
/// person never reaches another person's records.
pub async fn second_workspace(harness: &Harness) -> Workspace {
    let workspace = Workspace {
        id: WorkspaceId::generate(),
        ..harness.workspace.clone()
    };
    SqliteWorkspaceStore::new(harness.pool.clone())
        .create(&workspace)
        .await
        .unwrap();
    workspace
}

pub async fn harness(pool: SqlitePool) -> Harness {
    harness_with(pool, None).await
}

/// The same daemon with another executor behind the broker, for the
/// tests that judge what the broker does with a result.
async fn harness_with(pool: SqlitePool, behind: Option<Arc<dyn ToolExecutor>>) -> Harness {
    let workspace = workspace(&pool).await;
    SqliteWorkspaceStore::new(pool.clone())
        .create(&workspace)
        .await
        .unwrap();
    let agent = agent(&workspace.id);
    SqliteAgentStore::new(pool.clone())
        .create(&agent)
        .await
        .unwrap();
    let channel = channel(&workspace.id);
    SqliteChannelStore::new(pool.clone())
        .create(&channel)
        .await
        .unwrap();
    let run = queued_run(&workspace.id, &agent.id, &channel.id);
    SqliteRunStore::new(pool.clone())
        .create(&run)
        .await
        .unwrap();
    let bus: Arc<dyn EventBus> = Arc::new(AuditEventBus::new(Arc::new(SqliteEventLog::new(
        pool.clone(),
    ))));
    let hosts = Arc::new(pagis_storage_sqlite::SqliteHostStore::new(pool.clone()));
    let presence = Arc::new(pagis_broker::HostPresence::new());
    let executor = Arc::new(RecordingExecutor::default());
    let behind = behind.unwrap_or_else(|| Arc::clone(&executor) as _);
    let broker = Broker::new(BrokerDeps {
        forget: Arc::new(pagis_storage_sqlite::SqliteForgetStore::new(pool.clone())),
        grants: Arc::new(SqliteGrantStore::new(pool.clone())),
        credentials: Arc::new(pagis_broker::NoCredentialDirectory),
        connections: Arc::new(SqliteConnectionStore::new(pool.clone())),
        requests: Arc::new(SqliteRequestStore::new(pool.clone())),
        snapshots: Arc::new(SqliteCapabilitySnapshotStore::new(pool.clone())),
        runs: Arc::new(SqliteRunStore::new(pool.clone())),
        conversation_evidence: Arc::new(
            pagis_storage_sqlite::SqliteConversationEvidenceStore::new(pool.clone()),
        ),
        bus,
        executor: behind,
        phone_numbers: Arc::new(SqlitePhoneNumberStore::new(pool.clone())),
        mailboxes: Arc::new(SqliteAgentMailboxStore::new(pool.clone())),
        hosts: Arc::clone(&hosts) as _,
        presence: Arc::clone(&presence),
    });
    Harness {
        broker,
        executor,
        hosts,
        presence,
        pool,
        workspace,
        agent,
        run,
    }
}

fn google_manifest() -> CapabilityManifest {
    CapabilityManifest {
        namespace: "google".to_string(),
        source_version: "gog-0.42.0".to_string(),
        tools: vec![
            ManifestTool {
                definition: ToolDef {
                    name: "google__calendar_events".to_string(),
                    description: "List calendar events.".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {"query": {"type": "string"}},
                        "required": ["query"]
                    }),
                },
                capability: Some("calendar.read".to_string()),
                effect: EffectClass::Free,
                presentation: None,
                route: ToolRoute::Connection {
                    provider: "google".to_string(),
                },
                call_timeout: None,
                visibility: Default::default(),
                widget: None,
            },
            ManifestTool {
                definition: ToolDef {
                    name: "google__calendar_create_event".to_string(),
                    description: "Create one calendar event.".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "calendar_id": {"type": "string"},
                            "summary": {"type": "string"}
                        },
                        "required": ["calendar_id", "summary"]
                    }),
                },
                capability: Some("calendar.write".to_string()),
                effect: EffectClass::Outbound,
                presentation: Some(pagis_broker::ApprovalPresentation {
                    action_title: "Create a calendar event".to_string(),
                    body_argument: Some("summary".to_string()),
                    allow_rule_builder: None,
                }),
                route: ToolRoute::Connection {
                    provider: "google".to_string(),
                },
                call_timeout: None,
                visibility: Default::default(),
                widget: None,
            },
        ],
        event_kinds: vec![],
    }
}

async fn add_google_connection(harness: &Harness, id: &str) {
    add_google_connection_as(harness, id, id, &["calendar.read".to_string()]).await;
}

async fn add_google_connection_as(
    harness: &Harness,
    id: &str,
    alias: &str,
    capabilities: &[String],
) {
    sqlx::query(
        "INSERT INTO connections \
         (id, workspace_id, provider, alias, display_name, status, auth_mode, config, \
         created_at) \
         VALUES (?, ?, 'google', ?, 'Work Google', 'connected', 'byo', '{}', ?)",
    )
    .bind(id)
    .bind(harness.workspace.id.as_str())
    .bind(alias)
    .bind(now_ms())
    .execute(&harness.pool)
    .await
    .unwrap();
    SqliteGrantStore::new(harness.pool.clone())
        .create(&Grant {
            id: GrantId::generate(),
            workspace_id: harness.workspace.id.clone(),
            agent_id: harness.agent.id.clone(),
            resource_kind: Grant::CONNECTION_KIND.to_string(),
            resource_id: Some(id.to_string()),
            scope: Grant::connection_scope(capabilities),
            revision: 1,
            created_at: now_ms(),
            revoked_at: None,
        })
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn snapshots_are_stable_frozen_and_sorted(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, google_manifest())
        .unwrap();

    let before = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    assert_eq!(
        before
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [
            "host_shell",
            "computer_shell",
            "send_message",
            "memory_read",
            "memory_search",
            "memory_write",
            "memory_delete",
            "memory_review",
            "conversation_page",
            "conversation_search",
            "conversation_read",
            "skill_load",
            "schedule_create",
            "schedule_list",
            "schedule_update",
            "event_subscription_create",
            "event_subscription_list",
            "event_subscription_update",
            "software_publish",
            "tool_search",
            "software_fork",
            "software_contribute",
            "contribution_view",
            "contribution_close",
            "add_block",
            "ask_user",
        ]
    );

    add_google_connection(&harness, "google-work").await;
    let second_channel = channel(&harness.workspace.id);
    SqliteChannelStore::new(harness.pool.clone())
        .create(&second_channel)
        .await
        .unwrap();
    let second_run = queued_run(&harness.workspace.id, &harness.agent.id, &second_channel.id);
    SqliteRunStore::new(harness.pool.clone())
        .create(&second_run)
        .await
        .unwrap();
    let after = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &second_run.id)
        .await
        .unwrap();

    assert_eq!(before.tools.len(), 26, "the first snapshot stays frozen");
    assert_eq!(after.tools.last().unwrap().name, "google__calendar_events");

    let third_channel = channel(&harness.workspace.id);
    SqliteChannelStore::new(harness.pool.clone())
        .create(&third_channel)
        .await
        .unwrap();
    let third_run = queued_run(&harness.workspace.id, &harness.agent.id, &third_channel.id);
    SqliteRunStore::new(harness.pool.clone())
        .create(&third_run)
        .await
        .unwrap();
    let repeated = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &third_run.id)
        .await
        .unwrap();
    assert_eq!(after.id, repeated.id);
    assert_eq!(after.hash, repeated.hash);
    assert_eq!(after.tools, repeated.tools);
    assert!(
        SqliteCapabilitySnapshotStore::new(harness.pool)
            .for_run(&harness.workspace.id, &second_run.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn core_tool_definitions_keep_their_contract(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    let path = serde_json::json!({
        "type": "string",
        "description": "The memory file path: shared/<file> for workspace memory every agent reads, or private/<file> for your own memory. A [[<path>]] link of a page is such a path with brackets, and it names the page it points at."
    });
    let revision = serde_json::json!({
        "type": "string",
        "description": "The memory revision from the current system context, or missing for an empty repository."
    });
    let expected = vec![
        ToolDef {
            name: "host_shell".to_string(),
            description: "Run a shell command on one of the user's own computers, as the user, through the Pagis client running on it. It never runs on the server. A command outside the granted allow rules asks the user for an approval before it executes, and the user is asked which computer when more than one of theirs is connected.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"command": {"type": "string", "description": "The shell command to run."}},
                "required": ["command"]
            }),
        },
        ToolDef {
            name: "send_message".to_string(),
            description: "Send a message to another of the user's sprites, or to your user. Set `to` to the sprite's name to delegate work in your DM channel with that sprite; the sprite answers there and its reply comes back to you. Set `to` to \"user\" to post in your DM with the user.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "to": {"type": "string", "description": "The recipient: an agent's name, or \"user\"."},
                    "text": {"type": "string", "description": "The message text, in markdown."}
                },
                "required": ["to", "text"]
            }),
        },
        ToolDef {
            name: "memory_read".to_string(),
            description: "Read one memory file. The MEMORY.md indexes are already in your context; use this for the fact files they link.".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {"path": path.clone()}, "required": ["path"]}),
        },
        ToolDef {
            name: "memory_write".to_string(),
            description: "Write one whole memory file. Keep facts one per file and keep the scope's MEMORY.md index line for it current. Changes land as one commit when the run completes.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": path.clone(),
                    "content": {"type": "string", "description": "The complete new file content."},
                    "expected_memory_revision": revision.clone()
                },
                "required": ["path", "content", "expected_memory_revision"]
            }),
        },
        ToolDef {
            name: "memory_delete".to_string(),
            description: "Delete one memory file. Remove its MEMORY.md index line too.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"path": path, "expected_memory_revision": revision},
                "required": ["path", "expected_memory_revision"]
            }),
        },
    ];

    // Other tools sit between these; the five core tools keep their
    // definitions and their order.
    let core: Vec<ToolDef> = snapshot
        .tools
        .iter()
        .filter(|tool| expected.iter().any(|def| def.name == tool.name))
        .cloned()
        .collect();
    assert_eq!(core, expected);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_manifest_installs_with_provider_safe_qualified_tool_names(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, google_manifest())
        .unwrap();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn manifest_install_rejects_names_a_provider_would_refuse(pool: SqlitePool) {
    let harness = harness(pool).await;
    let mut invalid_namespace = google_manifest();
    invalid_namespace.namespace = "bad.namespace".to_string();
    invalid_namespace.tools[0].definition.name = "bad.namespace__calendar_events".to_string();
    invalid_namespace.tools[1].definition.name = "bad.namespace__calendar_create_event".to_string();
    assert!(matches!(
        harness.broker.install_manifest(&harness.workspace.id, invalid_namespace),
        Err(ManifestError::InvalidNamespace(namespace)) if namespace == "bad.namespace"
    ));

    let mut invalid_character = google_manifest();
    invalid_character.tools[0].definition.name = "google__calendar.events".to_string();
    invalid_character.tools[1].definition.name = "google__calendar_create_event".to_string();
    assert!(matches!(
        harness.broker.install_manifest(&harness.workspace.id, invalid_character),
        Err(ManifestError::InvalidToolName(name)) if name == "google__calendar.events"
    ));

    let mut too_long = google_manifest();
    let name = format!("google__{}", "a".repeat(57));
    too_long.tools[0].definition.name = name.clone();
    too_long.tools[1].definition.name = "google__calendar_create_event".to_string();
    assert_eq!(name.len(), 65);
    assert!(matches!(
        harness.broker.install_manifest(&harness.workspace.id, too_long),
        Err(ManifestError::InvalidToolName(rejected)) if rejected == name
    ));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn live_authorization_request_and_audit_use_the_run_snapshot(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, google_manifest())
        .unwrap();
    add_google_connection(&harness, "google-work").await;
    let grants = SqliteGrantStore::new(harness.pool.clone());
    let grant = grants
        .list_live_for_agent(&harness.workspace.id, &harness.agent.id)
        .await
        .unwrap()
        .remove(0);
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    assert!(
        snapshot
            .tools
            .iter()
            .all(|tool| tool.name != "google__calendar_create_event")
    );

    grants
        .set_scope(
            &harness.workspace.id,
            &grant.id,
            &Grant::connection_scope(&["calendar.read".to_string(), "calendar.write".to_string()]),
        )
        .await
        .unwrap();
    let read = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("google__calendar_events", r#"{"query":"status"}"#),
        )
        .await
        .unwrap();
    assert!(matches!(read, pagis_broker::InvokeOutcome::Completed(_)));
    assert_eq!(harness.executor.calls.lock().unwrap().len(), 1);

    let channel = channel(&harness.workspace.id);
    SqliteChannelStore::new(harness.pool.clone())
        .create(&channel)
        .await
        .unwrap();
    let next_run = queued_run(&harness.workspace.id, &harness.agent.id, &channel.id);
    SqliteRunStore::new(harness.pool.clone())
        .create(&next_run)
        .await
        .unwrap();
    let next_snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &next_run.id)
        .await
        .unwrap();
    assert!(
        next_snapshot
            .tools
            .iter()
            .any(|tool| tool.name == "google__calendar_create_event")
    );
    let waiting = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &next_run.id,
            &next_snapshot.id,
            pagis_broker::ToolCall::new(
                "google__calendar_create_event",
                r#"{"calendar_id":"primary","summary":"Status"}"#,
            ),
        )
        .await
        .unwrap();
    let pagis_broker::InvokeOutcome::Waiting(request) = waiting else {
        panic!("send must wait for request");
    };
    assert_eq!(request.request.kind, pagis_core::Request::TOOL_ACTION_KIND);
    assert_eq!(
        request.request.payload["tool_name"],
        "google__calendar_create_event"
    );
    assert_eq!(
        request.request.payload["proposed_rules"],
        serde_json::json!([])
    );

    SqliteRequestStore::new(harness.pool.clone())
        .decide(
            &harness.workspace.id,
            &request.request.id,
            RequestState::Approved,
            None,
            now_ms(),
        )
        .await
        .unwrap();
    grants
        .revoke(&harness.workspace.id, &grant.id, now_ms())
        .await
        .unwrap();
    let rejected = harness
        .broker
        .resolve(&request.request.id, RequestState::Approved)
        .await
        .unwrap();
    assert_eq!(
        rejected.result().unwrap().code.as_deref(),
        Some("permission_revoked")
    );
    assert_eq!(harness.executor.calls.lock().unwrap().len(), 1);

    let events = SqliteEventLog::new(harness.pool)
        .list_by_types(&harness.workspace.id, &["tool.completed"], None, 10)
        .await
        .unwrap();
    let read_audit = events
        .iter()
        .find(|event| event.payload["qualified_name"] == "google__calendar_events")
        .unwrap();
    assert_eq!(read_audit.payload["snapshot_hash"], snapshot.hash);
    assert_eq!(read_audit.payload["grant_revision"], 2);
    assert_eq!(read_audit.payload["authorization"], "free");
    assert_eq!(read_audit.payload["outcome"], "completed");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn one_granted_connection_needs_no_alias(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, google_manifest())
        .unwrap();
    add_google_connection_as(
        &harness,
        "conn-work",
        "work",
        &["calendar.read".to_string()],
    )
    .await;

    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    let search = snapshot
        .tools
        .iter()
        .find(|tool| tool.name == "google__calendar_events")
        .unwrap();
    assert_eq!(
        search.parameters["properties"]["connection"]["enum"],
        serde_json::json!(["work"])
    );

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("google__calendar_events", r#"{"query":"is:unread"}"#),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, pagis_broker::InvokeOutcome::Completed(_)));
    let calls = harness.executor.calls.lock().unwrap();
    assert_eq!(calls[0].selected_connection.as_deref(), Some("conn-work"));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn two_granted_connections_ask_instead_of_guessing(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, google_manifest())
        .unwrap();
    add_google_connection_as(
        &harness,
        "conn-work",
        "work",
        &["calendar.read".to_string()],
    )
    .await;
    add_google_connection_as(
        &harness,
        "conn-home",
        "home",
        &["calendar.read".to_string()],
    )
    .await;

    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    // A second account adds an alias, never a second tool.
    let google: Vec<&str> = snapshot
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .filter(|name| name.starts_with("google__"))
        .collect();
    assert_eq!(google, ["google__calendar_events"]);

    let ambiguous = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("google__calendar_events", r#"{"query":"is:unread"}"#),
        )
        .await
        .unwrap();
    let result = ambiguous.result().unwrap();
    assert_eq!(result.code.as_deref(), Some("connection_required"));
    let mut offered: Vec<&str> = result.metadata["connections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|alias| alias.as_str().unwrap())
        .collect();
    offered.sort();
    assert_eq!(offered, ["home", "work"]);
    assert!(harness.executor.calls.lock().unwrap().is_empty());

    let named = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new(
                "google__calendar_events",
                r#"{"query":"is:unread","connection":"home"}"#,
            ),
        )
        .await
        .unwrap();
    assert!(matches!(named, pagis_broker::InvokeOutcome::Completed(_)));
    let calls = harness.executor.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].selected_connection.as_deref(), Some("conn-home"));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_alias_outside_the_run_snapshot_never_reaches_a_provider(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, google_manifest())
        .unwrap();
    add_google_connection_as(
        &harness,
        "conn-work",
        "work",
        &["calendar.read".to_string()],
    )
    .await;

    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    // The alias exists in the workspace, but this agent holds no grant
    // on it, so the run's own tool list does not offer it.
    add_google_connection_as(&harness, "conn-other", "other", &[]).await;

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new(
                "google__calendar_events",
                r#"{"query":"is:unread","connection":"other"}"#,
            ),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, pagis_broker::InvokeOutcome::Rejected(_)));
    assert!(harness.executor.calls.lock().unwrap().is_empty());
}

/// The `ui` manifest: both tools sit in every run's snapshot,
/// neither needs a grant, and no plugin can claim the namespace.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_ui_tools_need_no_grant_and_the_namespace_is_reserved(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    let ui: Vec<&ToolDef> = snapshot
        .tools
        .iter()
        .filter(|tool| tool.name == "add_block" || tool.name == "ask_user")
        .collect();
    assert_eq!(ui.len(), 2, "no grant is needed for either tool");
    assert!(ui[0].description.contains("table"), "{:?}", ui[0]);
    assert!(ui[1].description.contains("answer"), "{:?}", ui[1]);

    let mut claimed = google_manifest();
    claimed.namespace = "ui".to_string();
    claimed.tools[0].definition.name = "ui.add_block".to_string();
    assert!(matches!(
        harness.broker.install_manifest(&harness.workspace.id, claimed),
        Err(ManifestError::ReservedNamespace(namespace)) if namespace == "ui"
    ));
}

/// `add_block` dispatches to the executor like any other free tool,
/// and its schema refuses a block the daemon owns.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn add_block_dispatches_and_refuses_a_daemon_owned_block(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    let added = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new(
                "add_block",
                r#"{"block":{"type":"table","columns":[{"key":"a","label":"A"}],"rows":[[{"kind":"text","text":"one"}]]}}"#,
            ),
        )
        .await
        .unwrap();
    assert!(matches!(added, pagis_broker::InvokeOutcome::Completed(_)));
    let call = harness.executor.calls.lock().unwrap().remove(0);
    assert_eq!(
        call.route,
        ToolRoute::Ui {
            tool: pagis_broker::UiTool::AddBlock
        }
    );

    let refused = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new(
                "add_block",
                r#"{"block":{"type":"approval_card","request_id":"r","title":"t","body":"b"}}"#,
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        refused.result().unwrap().code.as_deref(),
        Some("invalid_request")
    );
    assert!(
        harness.executor.calls.lock().unwrap().is_empty(),
        "a daemon-owned block never reaches the executor"
    );
}

/// `ask_user` mints the Request row itself and returns the answer as
/// the call's result. No executor runs.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn ask_user_mints_a_form_and_answers_the_call(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    let waiting = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new(
                "ask_user",
                r#"{"title":"Book the room","fields":[{"key":"who","label":"Who","kind":"text","required":true}],"submit_label":"Book"}"#,
            ),
        )
        .await
        .unwrap();
    let pagis_broker::InvokeOutcome::Waiting(question) = waiting else {
        panic!("ask_user parks the run");
    };
    assert_eq!(question.request.kind, pagis_core::Request::FORM_KIND);
    assert_eq!(question.request.payload["title"], "Book the room");
    assert_eq!(question.request.payload["fields"][0]["key"], "who");
    assert_eq!(question.request.payload["submit_label"], "Book");
    assert_eq!(question.title, "Book the room");
    assert_eq!(question.request.run_id.as_ref(), Some(&harness.run.id));

    let requests = SqliteRequestStore::new(harness.pool.clone());
    assert!(
        requests
            .get(&harness.workspace.id, &question.request.id)
            .await
            .unwrap()
            .is_some()
    );
    requests
        .decide(
            &harness.workspace.id,
            &question.request.id,
            RequestState::Approved,
            Some(serde_json::json!({"who": "Ada"})),
            now_ms(),
        )
        .await
        .unwrap();
    let answered = harness
        .broker
        .resolve(&question.request.id, RequestState::Approved)
        .await
        .unwrap();
    let pagis_broker::InvokeOutcome::Completed(result) = answered else {
        panic!("an answered question completes the call");
    };
    assert_eq!(result.content, r#"{"who":"Ada"}"#);
    assert!(
        harness.executor.calls.lock().unwrap().is_empty(),
        "no executor runs an ask_user call"
    );

    let events = SqliteEventLog::new(harness.pool)
        .list_by_types(&harness.workspace.id, &["tool.completed"], None, 10)
        .await
        .unwrap();
    let audit = events
        .iter()
        .find(|event| event.payload["qualified_name"] == "ask_user")
        .expect("the call produced the same audit fact any tool does");
    assert_eq!(audit.payload["authorization"], "answered");
    assert_eq!(audit.payload["outcome"], "completed");
    assert_eq!(audit.payload["request_id"], question.request.id.as_str());
}

/// A choice is one tap; a dismissal is "no answer", not a denial; and
/// naming both shapes, or neither, is a tool error.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn ask_user_takes_one_shape_and_a_dismissal_is_no_answer(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    let ask = |arguments: &'static str| {
        harness.broker.invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("ask_user", arguments),
        )
    };

    let waiting = ask(r#"{"title":"Which date?","body":"Both work.","options":[{"value":"tue","label":"Tuesday"}]}"#)
        .await
        .unwrap();
    let pagis_broker::InvokeOutcome::Waiting(question) = waiting else {
        panic!("ask_user parks the run");
    };
    assert_eq!(question.request.kind, pagis_core::Request::CHOICE_KIND);
    assert_eq!(question.request.payload["options"][0]["value"], "tue");
    assert_eq!(question.body, "Both work.");

    let dismissed = harness
        .broker
        .resolve(&question.request.id, RequestState::Denied)
        .await
        .unwrap();
    let result = dismissed.result().unwrap();
    assert_eq!(result.code.as_deref(), Some("unanswered"));
    assert!(result.content.contains("did not answer"), "{result:?}");

    for arguments in [
        r#"{"title":"Which?","fields":[{"key":"a","label":"A","kind":"text"}],"options":[{"value":"x","label":"X"}]}"#,
        r#"{"title":"Which?"}"#,
    ] {
        let refused = ask(arguments).await.unwrap();
        assert_eq!(
            refused.result().unwrap().code.as_deref(),
            Some("invalid_request")
        );
    }
}

/// One published Software Package, as its Capability Manifest reaches
/// the broker. Its tools are deferred.
fn package_manifest(package: &str) -> CapabilityManifest {
    let tool = |name: &str, description: &str| ManifestTool {
        definition: ToolDef {
            name: format!("{package}__{name}"),
            description: description.to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
        capability: None,
        effect: EffectClass::Free,
        presentation: None,
        route: ToolRoute::Software {
            package: package.to_string(),
            tool: name.to_string(),
        },
        call_timeout: None,
        visibility: Default::default(),
        widget: None,
    };
    CapabilityManifest {
        namespace: package.to_string(),
        source_version: "v3".to_string(),
        tools: vec![
            tool("forecast", "The forecast of one city."),
            tool("alerts", "The severe alerts of one city."),
        ],
        event_kinds: vec![],
    }
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn deferred_software_tools_stay_out_of_the_array_and_change_the_hash(pool: SqlitePool) {
    let harness = harness(pool).await;
    let bare = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();

    harness
        .broker
        .install_manifest(&harness.workspace.id, package_manifest("weather"))
        .unwrap();
    let second_channel = channel(&harness.workspace.id);
    SqliteChannelStore::new(harness.pool.clone())
        .create(&second_channel)
        .await
        .unwrap();
    let second_run = queued_run(&harness.workspace.id, &harness.agent.id, &second_channel.id);
    SqliteRunStore::new(harness.pool.clone())
        .create(&second_run)
        .await
        .unwrap();
    let with_package = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &second_run.id)
        .await
        .unwrap();

    // The package is in the snapshot, content-addressed with the rest,
    // and out of the model's array until a search loads it.
    assert_ne!(bare.hash, with_package.hash);
    assert_eq!(with_package.tools, bare.tools);
    assert_eq!(
        with_package
            .deferred
            .iter()
            .map(|tool| tool.definition.name.as_str())
            .collect::<Vec<_>>(),
        ["weather__alerts", "weather__forecast"]
    );
    assert_eq!(with_package.package_count(), 1);
    assert_eq!(bare.package_count(), 0);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn loaded_tools_are_the_prefix_then_the_packages_in_load_order(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, package_manifest("weather"))
        .unwrap();
    harness
        .broker
        .install_manifest(&harness.workspace.id, package_manifest("abacus"))
        .unwrap();
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();

    let mut loaded = pagis_broker::LoadedSet::default();
    assert_eq!(snapshot.loaded_tools(&loaded), snapshot.tools);

    loaded.load("weather");
    loaded.load("abacus");
    // A repeat load is a no-op: the array only grows at its end.
    loaded.load("weather");
    let tools = snapshot.loaded_tools(&loaded);

    assert_eq!(&tools[..snapshot.tools.len()], &snapshot.tools[..]);
    assert_eq!(
        tools[snapshot.tools.len()..]
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [
            "weather__alerts",
            "weather__forecast",
            "abacus__alerts",
            "abacus__forecast"
        ]
    );
    assert_eq!(loaded.packages(), ["weather", "abacus"]);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_unloaded_deferred_tool_is_refused_and_a_loaded_one_dispatches(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, package_manifest("weather"))
        .unwrap();
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();

    let refused = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weather__forecast", "{}"),
        )
        .await
        .unwrap();

    assert_eq!(
        refused.result().unwrap().code.as_deref(),
        Some("tool_not_loaded")
    );
    assert!(harness.executor.calls.lock().unwrap().is_empty());

    let mut loaded = pagis_broker::LoadedSet::default();
    loaded.load("weather");
    let dispatched = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weather__forecast", "{}").with_loaded(&loaded),
        )
        .await
        .unwrap();

    assert!(!dispatched.result().unwrap().is_error);
    let calls = harness.executor.calls.lock().unwrap();
    let call = calls.first().expect("the call reached the executor");
    assert_eq!(
        call.route,
        ToolRoute::Software {
            package: "weather".to_string(),
            tool: "forecast".to_string()
        }
    );
    // The Version the run may call is the one its snapshot froze.
    assert_eq!(call.source_version, "v3");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_live_call_is_offered_no_deferred_tool(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, package_manifest("weather"))
        .unwrap();
    harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();

    let free = harness
        .broker
        .free_tools(&harness.workspace.id, &harness.run.id)
        .await
        .unwrap();

    assert!(
        free.iter().all(|tool| !tool.name.starts_with("weather__")),
        "a Remote Party cannot wait for a search"
    );
    assert!(
        free.iter().all(|tool| tool.name != "tool_search"),
        "a search a live call cannot act on is noise"
    );
}

/// The Capability Manifest of one installed Plugin (ADR-0017):
/// one `Host` tool that offers the rule naming itself.
fn plugin_manifest(plugin_id: &str) -> CapabilityManifest {
    CapabilityManifest {
        namespace: "weatherplugin".to_string(),
        source_version: "v1".to_string(),
        tools: vec![ManifestTool {
            definition: ToolDef {
                name: "weatherplugin__forecast".to_string(),
                description: "The forecast of one place.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {"place": {"type": "string"}},
                }),
            },
            capability: None,
            effect: EffectClass::Host,
            presentation: Some(pagis_broker::ApprovalPresentation {
                action_title: "Run weatherplugin__forecast".to_string(),
                body_argument: None,
                allow_rule_builder: Some(pagis_broker::AllowRuleBuilder::PluginTool),
            }),
            route: ToolRoute::Plugin {
                plugin: plugin_id.to_string(),
                server: "today".to_string(),
                tool: "forecast".to_string(),
            },
            call_timeout: None,
            visibility: Default::default(),
            widget: None,
        }],
        event_kinds: vec![],
    }
}

/// One Plugin Grant, with the allow rules it carries.
async fn grant_plugin(harness: &Harness, plugin_id: &str, rules: &[String]) {
    SqliteGrantStore::new(harness.pool.clone())
        .create(&Grant {
            id: GrantId::generate(),
            workspace_id: harness.workspace.id.clone(),
            agent_id: harness.agent.id.clone(),
            resource_kind: Grant::PLUGIN_KIND.to_string(),
            resource_id: Some(plugin_id.to_string()),
            scope: Grant::allow_scope(rules),
            revision: 1,
            created_at: now_ms(),
            revoked_at: None,
        })
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_plugin_tool_is_absent_without_the_plugin_grant(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, plugin_manifest("plugin-1"))
        .unwrap();

    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();

    assert!(
        snapshot
            .tools
            .iter()
            .all(|tool| tool.name != "weatherplugin__forecast"),
        "an agent that holds no plugin grant sees no plugin tool"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_granted_plugin_tool_waits_for_a_card_that_offers_its_own_rule(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, plugin_manifest("plugin-1"))
        .unwrap();
    grant_plugin(&harness, "plugin-1", &[]).await;

    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    assert!(
        snapshot
            .tools
            .iter()
            .any(|tool| tool.name == "weatherplugin__forecast")
    );

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weatherplugin__forecast", r#"{"place":"Oslo"}"#),
        )
        .await
        .unwrap();

    let pagis_broker::InvokeOutcome::Waiting(pending) = outcome else {
        panic!("a Host tool waits for a card: {outcome:?}");
    };
    assert_eq!(pending.request.payload["effect_class"], "host");
    assert_eq!(
        pending.request.payload["proposed_rules"],
        serde_json::json!(["weatherplugin__forecast"])
    );
    assert_eq!(pending.request.payload["plugin_id"], "plugin-1");
    assert!(harness.executor.calls.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_allow_rule_that_names_the_tool_runs_it_without_a_card(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, plugin_manifest("plugin-1"))
        .unwrap();
    grant_plugin(
        &harness,
        "plugin-1",
        &["weatherplugin__forecast".to_string()],
    )
    .await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weatherplugin__forecast", r#"{"place":"Oslo"}"#),
        )
        .await
        .unwrap();

    assert!(
        matches!(outcome, pagis_broker::InvokeOutcome::Completed(_)),
        "{outcome:?}"
    );
    let calls = harness.executor.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].route,
        ToolRoute::Plugin {
            plugin: "plugin-1".to_string(),
            server: "today".to_string(),
            tool: "forecast".to_string(),
        }
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_revoked_plugin_grant_stops_the_next_dispatch(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, plugin_manifest("plugin-1"))
        .unwrap();
    grant_plugin(
        &harness,
        "plugin-1",
        &["weatherplugin__forecast".to_string()],
    )
    .await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    let grants = SqliteGrantStore::new(harness.pool.clone());
    for grant in grants.list_live(&harness.workspace.id).await.unwrap() {
        if grant.resource_kind == Grant::PLUGIN_KIND {
            grants
                .revoke(&harness.workspace.id, &grant.id, now_ms())
                .await
                .unwrap();
        }
    }

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weatherplugin__forecast", r#"{"place":"Oslo"}"#),
        )
        .await
        .unwrap();

    let pagis_broker::InvokeOutcome::Rejected(result) = outcome else {
        panic!("a revoked grant stops the call: {outcome:?}");
    };
    assert_eq!(result.code.as_deref(), Some("permission_revoked"));
}

#[test]
fn a_result_inside_the_cap_is_untouched() {
    let content = "a".repeat(pagis_broker::MAX_RESULT_CHARS);

    assert_eq!(pagis_broker::cap_output(content.clone()), content);
}

#[test]
fn a_result_over_the_cap_is_cut_with_a_marker_that_names_its_size() {
    let content = "a".repeat(pagis_broker::MAX_RESULT_CHARS + 250);

    let capped = pagis_broker::cap_output(content);

    assert!(capped.starts_with(&"a".repeat(pagis_broker::MAX_RESULT_CHARS)));
    assert!(
        capped.contains(&format!(
            "{} characters",
            pagis_broker::MAX_RESULT_CHARS + 250
        )),
        "the marker names the whole size: {}",
        &capped[capped.len() - 100..]
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn every_route_takes_the_output_cap(pool: SqlitePool) {
    // The executor answers with more than the cap allows, as a plugin
    // server that returns a large blob does (ADR-0005).
    struct Blob;

    #[async_trait]
    impl ToolExecutor for Blob {
        async fn execute(&self, _call: AuthorizedCall) -> ToolResult {
            ToolResult::success("b".repeat(pagis_broker::MAX_RESULT_CHARS + 500))
        }
    }

    let harness = harness_with(pool, Some(Arc::new(Blob))).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, plugin_manifest("plugin-1"))
        .unwrap();
    grant_plugin(
        &harness,
        "plugin-1",
        &["weatherplugin__forecast".to_string()],
    )
    .await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weatherplugin__forecast", r#"{"place":"Oslo"}"#),
        )
        .await
        .unwrap();

    let result = outcome.result().expect("the call answered");
    assert!(
        result.content.contains(&format!(
            "{} characters",
            pagis_broker::MAX_RESULT_CHARS + 500
        )),
        "the marker names the whole size"
    );
    // The envelope of an untrusted result is around the capped text,
    // so the whole result is a little longer than the cap.
    assert!(
        result.content.chars().count() < pagis_broker::MAX_RESULT_CHARS + 1000,
        "the result is cut"
    );
}

// ------------------------------------------ source reads (ADR-0008)

/// The synced message that the scripted reads of these tests name.
fn gmail_read() -> pagis_core::knowledge::SourceRead {
    pagis_core::knowledge::SourceRead::Item {
        resource: "gmail".to_string(),
        id: "m-3f9a".to_string(),
    }
}

/// Call the calendar read of the one granted Connection `conn-work`.
async fn read_calendar(harness: &Harness) -> pagis_broker::InvokeOutcome {
    harness
        .broker
        .install_manifest(&harness.workspace.id, google_manifest())
        .unwrap();
    add_google_connection(harness, "conn-work").await;
    let snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("google__calendar_events", r#"{"query":"invoice"}"#),
        )
        .await
        .unwrap()
}

/// The rows of `run_source_reads`, as the kind and the id of each read.
async fn recorded_reads(pool: &SqlitePool) -> Vec<(String, String, String)> {
    sqlx::query_as("SELECT run_id, connection_id, source_id FROM run_source_reads")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// A result that names the synced content it holds records the read
/// for its Run, on the Connection of the call. The names leave the
/// result before the audit record, so no event holds a source id.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_result_records_its_source_reads_for_the_run(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .executor
        .answers(ToolResult::success("The invoice is due.").reading(&[gmail_read()]));

    let outcome = read_calendar(&harness).await;

    let result = outcome.result().expect("the call answered");
    assert!(!result.is_error, "{}", result.content);
    assert!(result.metadata.get("source_reads").is_none());
    assert_eq!(
        recorded_reads(&harness.pool).await,
        [(
            harness.run.id.to_string(),
            "conn-work".to_string(),
            "m-3f9a".to_string()
        )]
    );
    let events: Vec<String> = sqlx::query_scalar("SELECT payload FROM events")
        .fetch_all(&harness.pool)
        .await
        .unwrap();
    assert!(
        events.iter().all(|payload| !payload.contains("m-3f9a")),
        "an audit event holds the source id: {events:?}"
    );
}

/// A Forget that begins while the call runs blocks the record of its
/// read. The broker then gives the model an error in place of the
/// result, so the words that the call read never reach the Run.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_read_that_a_forget_blocks_during_the_call_does_not_reach_the_model(pool: SqlitePool) {
    struct ForgetDuringTheCall {
        pool: SqlitePool,
    }

    #[async_trait]
    impl ToolExecutor for ForgetDuringTheCall {
        async fn execute(&self, call: AuthorizedCall) -> ToolResult {
            sqlx::query(
                "INSERT INTO forget_operations(id,workspace_id,target,phase,created_at,connection_id) \
                 VALUES('forget-1',?,'{}','blocked',1,'conn-work')",
            )
            .bind(call.workspace_id.as_str())
            .execute(&self.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO forget_suppressions(operation_id,workspace_id,connection_id,resource,identity) \
                 VALUES('forget-1',?,'conn-work','gmail','keyed')",
            )
            .bind(call.workspace_id.as_str())
            .execute(&self.pool)
            .await
            .unwrap();
            ToolResult::success("The zqxforgot3f9a invoice is due.").reading(&[gmail_read()])
        }
    }

    let harness = harness_with(pool.clone(), Some(Arc::new(ForgetDuringTheCall { pool }))).await;

    let outcome = read_calendar(&harness).await;

    let result = outcome.result().expect("the call answered");
    assert!(result.is_error, "the result reached the model");
    assert!(
        !result.content.contains("zqxforgot3f9a"),
        "{}",
        result.content
    );
    assert_eq!(recorded_reads(&harness.pool).await, []);
}

// ------------------------------------------------- widgets (ADR-0016)

/// One package that ships a Widget: a tool that renders it, an
/// app-only tool the Widget refreshes itself with, and a tool that
/// asks the user through the Widget.
fn widget_manifest(package: &str) -> CapabilityManifest {
    let tool = |name: &str,
                visibility: pagis_broker::ToolVisibility,
                widget: Option<&str>,
                awaits: bool| {
        ManifestTool {
            definition: ToolDef {
                name: format!("{package}__{name}"),
                description: format!("The {name} of one city."),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
            capability: None,
            effect: EffectClass::Free,
            presentation: None,
            route: ToolRoute::Software {
                package: package.to_string(),
                tool: name.to_string(),
            },
            call_timeout: None,
            visibility,
            widget: widget.map(|name| pagis_broker::ToolWidget {
                name: name.to_string(),
                awaits_input: awaits,
            }),
        }
    };
    let both = pagis_broker::ToolVisibility::default();
    let app_only = pagis_broker::ToolVisibility {
        model: false,
        app: true,
    };
    let model_only = pagis_broker::ToolVisibility {
        model: true,
        app: false,
    };
    CapabilityManifest {
        namespace: package.to_string(),
        source_version: "v3".to_string(),
        tools: vec![
            tool("chart", both, Some("chart"), false),
            tool("pick", both, Some("chart"), true),
            tool("page", app_only, None, false),
            tool("private", model_only, None, false),
        ],
        event_kinds: vec![],
    }
}

/// The result a Widget tool's executor gives back: the author's
/// projection, with the data half on the metadata.
fn widget_result(package: &str, widget: &str) -> ToolResult {
    ToolResult::success("Tomorrow reaches 21 degrees.").with_metadata(serde_json::json!({
        pagis_broker::WIDGET_RESULT: {
            "package": package,
            "version": "v3",
            "widget": widget,
            "structured_content": {"high": 21},
        }
    }))
}

async fn widget_snapshot(harness: &Harness) -> pagis_broker::CapabilitySnapshot {
    harness
        .broker
        .install_manifest(&harness.workspace.id, widget_manifest("weather"))
        .unwrap();
    harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap()
}

/// The model's loaded set, with the widget package in it.
fn loaded() -> pagis_broker::LoadedSet {
    let mut loaded = pagis_broker::LoadedSet::default();
    loaded.load("weather");
    loaded
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_app_only_tool_never_reaches_the_model(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = widget_snapshot(&harness).await;

    let tools = snapshot.loaded_tools(&loaded());
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();

    assert!(names.contains(&"weather__chart"), "{names:?}");
    assert!(names.contains(&"weather__pick"), "{names:?}");
    assert!(names.contains(&"weather__private"), "{names:?}");
    assert!(!names.contains(&"weather__page"), "{names:?}");
    // A live call is offered the same set (ADR-0020).
    let free = harness
        .broker
        .free_tools(&harness.workspace.id, &harness.run.id)
        .await
        .unwrap();
    assert!(free.iter().all(|tool| tool.name != "weather__page"));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_widget_tool_mints_a_block_fixed_to_its_version(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = widget_snapshot(&harness).await;
    harness.executor.answers(widget_result("weather", "chart"));

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weather__chart", "{}")
                .with_loaded(&loaded())
                .with_tool_call_id("call_1"),
        )
        .await
        .unwrap();

    let result = outcome.result().expect("the call completed");
    let block = &result.metadata[pagis_broker::WIDGET_BLOCK];
    assert_eq!(block["package"], "weather");
    assert_eq!(block["version"], "v3");
    assert_eq!(block["widget"], "chart");
    assert_eq!(block["tool_call_id"], "call_1");
    assert!(block["request_id"].is_null());
    // The model reads the projection alone, labelled untrusted.
    assert!(result.content.contains("Tomorrow reaches 21 degrees."));
    assert!(!result.content.contains("\"high\""), "{}", result.content);
    // The view holds the data half.
    let view = harness
        .broker
        .widget_view(&harness.workspace.id, "call_1")
        .expect("a live view");
    assert_eq!(view.structured_content, serde_json::json!({"high": 21}));
    assert_eq!(view.package, "weather");
    assert_eq!(view.snapshot_id, snapshot.id);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_result_the_executor_did_not_split_mints_no_block(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = widget_snapshot(&harness).await;

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weather__chart", "{}")
                .with_loaded(&loaded())
                .with_tool_call_id("call_1"),
        )
        .await
        .unwrap();

    let result = outcome.result().expect("the call completed");
    assert!(result.metadata[pagis_broker::WIDGET_BLOCK].is_null());
    assert!(
        harness
            .broker
            .widget_view(&harness.workspace.id, "call_1")
            .is_none()
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_widget_that_asks_parks_the_run_on_a_widget_request(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = widget_snapshot(&harness).await;
    harness.executor.answers(widget_result("weather", "chart"));

    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weather__pick", "{}")
                .with_loaded(&loaded())
                .with_tool_call_id("call_2"),
        )
        .await
        .unwrap();

    let pagis_broker::InvokeOutcome::Waiting(pending) = outcome else {
        panic!("the run parks on the widget");
    };
    assert_eq!(pending.request.kind, "widget");
    assert_eq!(pending.request.payload["tool_call_id"], "call_2");
    assert_eq!(pending.request.payload["package"], "weather");
    assert_eq!(pending.request.payload["version"], "v3");
    assert_eq!(pending.body, "Tomorrow reaches 21 degrees.");
    let view = harness
        .broker
        .widget_view(&harness.workspace.id, "call_2")
        .expect("a live view");
    assert_eq!(view.request_id.as_ref(), Some(&pending.request.id));

    // The answer reaches the run inside the untrusted envelope.
    SqliteRequestStore::new(harness.pool.clone())
        .decide(
            &harness.workspace.id,
            &pending.request.id,
            RequestState::Approved,
            Some(serde_json::json!({"text": "the second row", "value": {"row": 2}})),
            now_ms(),
        )
        .await
        .unwrap();
    let settled = harness
        .broker
        .resolve(&pending.request.id, RequestState::Approved)
        .await
        .unwrap();
    let result = settled.result().expect("the call completed");
    assert!(result.content.contains("BEGIN UNTRUSTED"), "{result:?}");
    assert!(
        result.content.contains("widget:weather/chart"),
        "{result:?}"
    );
    assert!(result.content.contains("the second row"), "{result:?}");
    assert!(
        result.content.contains("Tomorrow reaches 21 degrees."),
        "{result:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_widget_that_is_not_answered_still_tells_the_model_what_it_showed(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = widget_snapshot(&harness).await;
    harness.executor.answers(widget_result("weather", "chart"));
    let outcome = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weather__pick", "{}")
                .with_loaded(&loaded())
                .with_tool_call_id("call_2"),
        )
        .await
        .unwrap();
    let pagis_broker::InvokeOutcome::Waiting(pending) = outcome else {
        panic!("the run parks on the widget");
    };

    let settled = harness
        .broker
        .resolve(&pending.request.id, RequestState::Denied)
        .await
        .unwrap();

    let result = settled.result().expect("the call completed");
    assert!(result.content.contains("did not answer"), "{result:?}");
    assert!(
        result.content.contains("Tomorrow reaches 21 degrees."),
        "{result:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_widget_reaches_its_own_package_and_nothing_else(pool: SqlitePool) {
    let harness = harness(pool).await;
    harness
        .broker
        .install_manifest(&harness.workspace.id, widget_manifest("abacus"))
        .unwrap();
    let snapshot = widget_snapshot(&harness).await;

    // Its own app-visible tool runs, and no loaded set is needed: the
    // view is already rendered.
    let allowed = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weather__page", "{}").from_widget("weather"),
        )
        .await
        .unwrap();
    assert!(matches!(allowed, pagis_broker::InvokeOutcome::Completed(_)));

    // Another package's tool is refused.
    let crossed = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("abacus__page", "{}").from_widget("weather"),
        )
        .await
        .unwrap();
    let refused = crossed.result().expect("a refusal");
    assert!(refused.is_error);
    assert!(refused.content.contains("belongs to abacus"), "{refused:?}");

    // A tool its author kept for the model is refused too.
    let hidden = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weather__private", "{}").from_widget("weather"),
        )
        .await
        .unwrap();
    let refused = hidden.result().expect("a refusal");
    assert!(refused.is_error);
    assert!(
        refused.content.contains("not visible to a widget"),
        "{refused:?}"
    );

    // A tool that is not a package tool at all is refused.
    let native = harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("host_shell", r#"{"command":"id"}"#).from_widget("weather"),
        )
        .await
        .unwrap();
    let refused = native.result().expect("a refusal");
    assert!(refused.is_error);
    assert!(
        refused.content.contains("its own package's tools"),
        "{refused:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_widget_call_mounts_no_second_view(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = widget_snapshot(&harness).await;
    harness.executor.answers(widget_result("weather", "chart"));

    harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weather__chart", "{}")
                .from_widget("weather")
                .with_tool_call_id("call_3"),
        )
        .await
        .unwrap();

    assert!(
        harness
            .broker
            .widget_view(&harness.workspace.id, "call_3")
            .is_none()
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_run_ending_tears_its_views_down(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = widget_snapshot(&harness).await;
    harness.executor.answers(widget_result("weather", "chart"));
    harness
        .broker
        .invoke(
            &harness.workspace.id,
            &harness.run.id,
            &snapshot.id,
            pagis_broker::ToolCall::new("weather__chart", "{}")
                .with_loaded(&loaded())
                .with_tool_call_id("call_1"),
        )
        .await
        .unwrap();

    let torn = harness.broker.teardown_widgets(&harness.run.id);

    assert_eq!(torn, ["call_1"]);
    assert!(
        harness
            .broker
            .widget_view(&harness.workspace.id, "call_1")
            .is_none()
    );
    assert!(harness.broker.teardown_widgets(&harness.run.id).is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_run_keeps_at_most_twenty_live_views(pool: SqlitePool) {
    let harness = harness(pool).await;
    let snapshot = widget_snapshot(&harness).await;
    harness.executor.answers(widget_result("weather", "chart"));

    for index in 0..pagis_broker::MAX_LIVE_WIDGETS + 3 {
        harness
            .broker
            .invoke(
                &harness.workspace.id,
                &harness.run.id,
                &snapshot.id,
                pagis_broker::ToolCall::new("weather__chart", "{}")
                    .with_loaded(&loaded())
                    .with_tool_call_id(format!("call_{index}")),
            )
            .await
            .unwrap();
    }

    assert!(
        harness
            .broker
            .widget_view(&harness.workspace.id, "call_0")
            .is_none()
    );
    assert!(
        harness
            .broker
            .widget_view(&harness.workspace.id, "call_2")
            .is_none()
    );
    assert!(
        harness
            .broker
            .widget_view(&harness.workspace.id, "call_3")
            .is_some()
    );
    assert_eq!(
        harness.broker.teardown_widgets(&harness.run.id).len(),
        pagis_broker::MAX_LIVE_WIDGETS
    );
}

/// A second tenant of one daemon: the Workspace is the tenant,
/// so a test about two tenants needs a whole second Workspace with its
/// own Agent and its own queued Run.
struct Tenant {
    workspace: Workspace,
    agent: Agent,
    run: Run,
}

async fn second_tenant(harness: &Harness) -> Tenant {
    let workspace = Workspace {
        id: WorkspaceId::generate(),
        name: "Second Workspace".to_string(),
        ..harness.workspace.clone()
    };
    SqliteWorkspaceStore::new(harness.pool.clone())
        .create(&workspace)
        .await
        .unwrap();
    let agent = agent(&workspace.id);
    SqliteAgentStore::new(harness.pool.clone())
        .create(&agent)
        .await
        .unwrap();
    let channel = channel(&workspace.id);
    SqliteChannelStore::new(harness.pool.clone())
        .create(&channel)
        .await
        .unwrap();
    let run = queued_run(&workspace.id, &agent.id, &channel.id);
    SqliteRunStore::new(harness.pool.clone())
        .create(&run)
        .await
        .unwrap();
    Tenant {
        workspace,
        agent,
        run,
    }
}

/// The same package at the version one tenant installed.
fn package_manifest_at(package: &str, version: &str) -> CapabilityManifest {
    CapabilityManifest {
        source_version: version.to_string(),
        ..package_manifest(package)
    }
}

/// A second manifest of a source the installation owns, under its own
/// namespace.
fn vault_manifest() -> CapabilityManifest {
    let mut manifest = google_manifest();
    manifest.namespace = "pagisvault".to_string();
    manifest.source_version = "vault-1".to_string();
    manifest.tools[0].definition.name = "pagisvault__credential_list".to_string();
    manifest.tools[1].definition.name = "pagisvault__credential_fill".to_string();
    manifest
}

fn loaded_package(package: &str) -> pagis_broker::LoadedSet {
    let mut loaded = pagis_broker::LoadedSet::default();
    loaded.load(package);
    loaded
}

/// Two tenants hold two versions of one namespace, and neither reads
/// what the other installed.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn two_tenants_hold_their_own_version_of_one_namespace(pool: SqlitePool) {
    let harness = harness(pool).await;
    let second = second_tenant(&harness).await;

    harness
        .broker
        .install_manifest(&harness.workspace.id, package_manifest_at("weather", "v3"))
        .unwrap();
    // The same namespace at another version is no conflict: it is
    // another tenant's registry.
    harness
        .broker
        .install_manifest(&second.workspace.id, package_manifest_at("weather", "v4"))
        .unwrap();
    harness
        .broker
        .install_manifest(&second.workspace.id, package_manifest("abacus"))
        .unwrap();

    let first_snapshot = harness
        .broker
        .prepare_run(&harness.workspace.id, &harness.agent.id, &harness.run.id)
        .await
        .unwrap();
    let second_snapshot = harness
        .broker
        .prepare_run(&second.workspace.id, &second.agent.id, &second.run.id)
        .await
        .unwrap();

    let names = |snapshot: &pagis_broker::CapabilitySnapshot| {
        snapshot
            .deferred
            .iter()
            .map(|tool| tool.definition.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&first_snapshot),
        ["weather__alerts", "weather__forecast"]
    );
    assert_eq!(
        names(&second_snapshot),
        [
            "abacus__alerts",
            "abacus__forecast",
            "weather__alerts",
            "weather__forecast"
        ],
        "a package one tenant installed is absent from the other tenant"
    );

    // Each snapshot froze the version of its own tenant.
    for (workspace_id, run_id, snapshot) in [
        (&harness.workspace.id, &harness.run.id, &first_snapshot),
        (&second.workspace.id, &second.run.id, &second_snapshot),
    ] {
        harness
            .broker
            .invoke(
                workspace_id,
                run_id,
                &snapshot.id,
                pagis_broker::ToolCall::new("weather__forecast", "{}")
                    .with_loaded(&loaded_package("weather")),
            )
            .await
            .unwrap();
    }
    let calls = harness.executor.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .map(|call| call.source_version.as_str())
            .collect::<Vec<_>>(),
        ["v3", "v4"]
    );
}

/// A tool name is claimed inside one tenant and stays free in the next.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_claimed_tool_name_stops_one_tenant_and_not_another(pool: SqlitePool) {
    let harness = harness(pool).await;
    let second = second_tenant(&harness).await;
    let mut first = google_manifest();
    first.namespace = "a".to_string();
    first.source_version = "1".to_string();
    first.tools[0].definition.name = "a__b__search".to_string();
    first.tools[1].definition.name = "a__b__send".to_string();
    let mut collision = google_manifest();
    collision.namespace = "a__b".to_string();
    collision.source_version = "1".to_string();
    collision.tools[0].definition.name = "a__b__search".to_string();
    collision.tools[1].definition.name = "a__b__send".to_string();

    harness
        .broker
        .install_manifest(&harness.workspace.id, first.clone())
        .unwrap();
    assert!(matches!(
        harness
            .broker
            .install_manifest(&harness.workspace.id, collision.clone()),
        Err(ManifestError::DuplicateTool(name)) if name == "a__b__search"
    ));
    // The name is claimed in one tenant alone, so the namespace the
    // first tenant refused installs here.
    harness
        .broker
        .install_manifest(&second.workspace.id, collision)
        .unwrap();
    // A second claim inside one tenant still fails.
    assert!(matches!(
        harness.broker.install_manifest(&second.workspace.id, first),
        Err(ManifestError::DuplicateTool(name)) if name == "a__b__search"
    ));
}

/// An installation manifest reaches the tenants that exist and the
/// tenants created after it.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_installation_manifest_reaches_every_tenant(pool: SqlitePool) {
    let harness = harness(pool).await;
    // Before any tenant registry exists.
    harness
        .broker
        .install_installation_manifest(google_manifest())
        .unwrap();
    harness
        .broker
        .install_manifest(&harness.workspace.id, package_manifest("weather"))
        .unwrap();
    let second = second_tenant(&harness).await;

    assert!(
        harness
            .broker
            .namespaces(&harness.workspace.id)
            .contains("google")
    );
    assert!(
        harness
            .broker
            .namespaces(&second.workspace.id)
            .contains("google"),
        "a tenant created after the install holds it too"
    );
    assert!(
        !harness
            .broker
            .namespaces(&second.workspace.id)
            .contains("weather")
    );

    // After a tenant registry exists.
    harness
        .broker
        .install_installation_manifest(vault_manifest())
        .unwrap();
    for workspace_id in [&harness.workspace.id, &second.workspace.id] {
        assert!(
            harness
                .broker
                .namespaces(workspace_id)
                .contains("pagisvault")
        );
    }
    assert_eq!(
        harness
            .broker
            .connection_capabilities(&harness.workspace.id, "google"),
        harness
            .broker
            .connection_capabilities(&second.workspace.id, "google")
    );
}
