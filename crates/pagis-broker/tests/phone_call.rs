//! The `phone_call` tool at the broker (ADR-0020): offered only
//! to an Agent that holds a number, gated by an approval card on every
//! call, and E.164 only. The session tools are not in the snapshot at
//! all, because a Run that is not on a call has nothing to hang up.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_audit::AuditEventBus;
use pagis_broker::{
    AuthorizedCall, Broker, BrokerDeps, CoreTool, InvokeOutcome, PHONE_CALL, ToolCall,
    ToolExecutor, ToolResult, ToolRoute,
};
use pagis_core::{
    Agent, AgentId, AgentStatus, AgentStore, Channel, ChannelId, ChannelKind, ChannelStore,
    ConnectionId, EventBus, MessageId, PhoneNumber, PhoneNumberId, PhoneNumberStore,
    PurchaseIntent, PurchaseIntentId, PurchaseIntentState, RequestState, Run, RunId, RunState,
    RunStore, TriggerKind, Workspace, WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentMailboxStore, SqliteAgentStore, SqliteCapabilitySnapshotStore, SqliteChannelStore,
    SqliteConnectionStore, SqliteEventLog, SqliteGrantStore, SqlitePhoneNumberStore,
    SqliteRequestStore, SqliteRunStore, SqliteWorkspaceStore,
};
use sqlx::SqlitePool;

const CALL: &str = r#"{"to": "+14155550199", "brief": "Ask if the table for four is still free at seven.", "success_criteria": "A yes or a no about the table."}"#;

#[derive(Default)]
struct RecordingExecutor {
    calls: Mutex<Vec<AuthorizedCall>>,
}

#[async_trait]
impl ToolExecutor for RecordingExecutor {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        self.calls.lock().unwrap().push(call);
        ToolResult::success(r#"{"outcome": "answered"}"#)
    }
}

struct World {
    broker: Broker,
    executor: Arc<RecordingExecutor>,
    numbers: Arc<SqlitePhoneNumberStore>,
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    run_id: RunId,
}

impl World {
    async fn build(pool: SqlitePool) -> Self {
        let person =
            pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", pagis_core::now_ms())
                .await
                .unwrap();
        let workspace = Workspace {
            user_id: person.id.clone(),
            id: WorkspaceId::generate(),
            name: "Workspace".to_string(),
            timezone: "UTC".to_string(),
            created_at: now_ms(),
            onboarded_at: None,
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
        };
        SqliteWorkspaceStore::new(pool.clone())
            .create(&workspace)
            .await
            .unwrap();
        let agent = Agent {
            id: AgentId::generate(),
            workspace_id: workspace.id.clone(),
            name: "Robin".to_string(),
            job: "assistant".to_string(),
            description: String::new(),
            personality: "warm".to_string(),
            model_alias: "default".to_string(),
            avatar: Default::default(),
            voice: None,
            standing_brief: None,
            status: AgentStatus::Active,
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        SqliteAgentStore::new(pool.clone())
            .create(&agent)
            .await
            .unwrap();
        let channel = Channel {
            id: ChannelId::generate(),
            workspace_id: workspace.id.clone(),
            kind: ChannelKind::Dm,
            title: Some("general".to_string()),
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        SqliteChannelStore::new(pool.clone())
            .create(&channel)
            .await
            .unwrap();
        let run = Run {
            id: RunId::generate(),
            workspace_id: workspace.id.clone(),
            agent_id: agent.id.clone(),
            channel_id: Some(channel.id.clone()),
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
        };
        SqliteRunStore::new(pool.clone())
            .create(&run)
            .await
            .unwrap();
        let bus: Arc<dyn EventBus> = Arc::new(AuditEventBus::new(Arc::new(SqliteEventLog::new(
            pool.clone(),
        ))));
        let executor = Arc::new(RecordingExecutor::default());
        let numbers = Arc::new(SqlitePhoneNumberStore::new(pool.clone()));
        let broker = Broker::new(BrokerDeps {
            hosts: Arc::new(pagis_storage_sqlite::SqliteHostStore::new(pool.clone())),
            presence: Arc::new(pagis_broker::HostPresence::new()),
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
            executor: Arc::clone(&executor) as _,
            phone_numbers: Arc::clone(&numbers) as _,
            mailboxes: Arc::new(SqliteAgentMailboxStore::new(pool.clone())),
        });
        Self {
            broker,
            executor,
            numbers,
            workspace_id: workspace.id,
            agent_id: agent.id,
            run_id: run.id,
        }
    }

    /// Give the Agent a desk line, written the way a purchase writes
    /// it.
    async fn hold_number(&self) {
        let at = now_ms();
        let number = PhoneNumber::new(
            PhoneNumberId::generate(),
            self.workspace_id.clone(),
            ConnectionId::generate(),
            "+14155550123".to_string(),
            "carrier-14155550123".to_string(),
            Some(self.agent_id.clone()),
            at,
        );
        let intent = PurchaseIntent {
            id: PurchaseIntentId::generate(),
            workspace_id: self.workspace_id.clone(),
            connection_id: number.connection_id.clone(),
            e164: number.e164.clone(),
            agent_id: Some(self.agent_id.clone()),
            state: PurchaseIntentState::Pending,
            created_at: at,
            settled_at: None,
        };
        self.numbers.create_intent(&intent).await.unwrap();
        self.numbers
            .confirm_purchase(&self.workspace_id, &intent.id, &number, at)
            .await
            .unwrap();
    }

    async fn snapshot(&self) -> pagis_broker::CapabilitySnapshot {
        self.broker
            .prepare_run(&self.workspace_id, &self.agent_id, &self.run_id)
            .await
            .unwrap()
    }

    async fn invoke(&self, snapshot_id: &str, name: &str, arguments: &str) -> InvokeOutcome {
        self.broker
            .invoke(
                &self.workspace_id,
                &self.run_id,
                snapshot_id,
                ToolCall::new(name, arguments),
            )
            .await
            .unwrap()
    }
}

fn names(snapshot: &pagis_broker::CapabilitySnapshot) -> Vec<&str> {
    snapshot
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect()
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_tool_is_absent_without_a_number(pool: SqlitePool) {
    let world = World::build(pool).await;
    let snapshot = world.snapshot().await;
    assert!(!names(&snapshot).contains(&PHONE_CALL));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_tool_is_offered_to_a_number_holder(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_number().await;
    let snapshot = world.snapshot().await;
    assert!(names(&snapshot).contains(&PHONE_CALL));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_session_tool_is_absent_outside_a_call(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_number().await;
    let snapshot = world.snapshot().await;
    assert!(!names(&snapshot).contains(&"hang_up"));
    assert!(!names(&snapshot).contains(&"send_digits"));
    let outcome = world.invoke(&snapshot.id, "hang_up", "{}").await;
    let InvokeOutcome::Rejected(result) = outcome else {
        panic!("a session tool outside a call is unknown: {outcome:?}");
    };
    assert_eq!(result.code.as_deref(), Some("invalid_request"));
    assert!(world.executor.calls.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_is_approved(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_number().await;
    let snapshot = world.snapshot().await;
    let outcome = world.invoke(&snapshot.id, PHONE_CALL, CALL).await;
    let InvokeOutcome::Waiting(pending) = outcome else {
        panic!("every call mints a card: {outcome:?}");
    };
    assert_eq!(pending.title, "Place a phone call");
    assert_eq!(
        pending.body,
        "To: +14155550199\nFor: Ask if the table for four is still free at seven."
    );
    assert_eq!(pending.request.payload["effect_class"], "outbound");
    assert_eq!(
        pending.request.payload["proposed_rules"],
        serde_json::json!([])
    );

    let outcome = world
        .broker
        .resolve(&pending.request.id, RequestState::Approved)
        .await
        .unwrap();
    let InvokeOutcome::Completed(result) = outcome else {
        panic!("an approved call runs: {outcome:?}");
    };
    assert_eq!(result.content, r#"{"outcome": "answered"}"#);
    let calls = world.executor.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].route,
        ToolRoute::Core {
            tool: CoreTool::PhoneCall
        }
    );
    assert_eq!(calls[0].arguments["to"], "+14155550199");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_call_is_refused(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_number().await;
    let snapshot = world.snapshot().await;
    let InvokeOutcome::Waiting(pending) = world.invoke(&snapshot.id, PHONE_CALL, CALL).await else {
        panic!("every call mints a card");
    };
    let outcome = world
        .broker
        .resolve(&pending.request.id, RequestState::Denied)
        .await
        .unwrap();
    let InvokeOutcome::Rejected(result) = outcome else {
        panic!("a refused call does not run: {outcome:?}");
    };
    assert_eq!(result.code.as_deref(), Some("permission_denied"));
    assert!(world.executor.calls.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_repeated_call_asks_again(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_number().await;
    let snapshot = world.snapshot().await;
    let InvokeOutcome::Waiting(first) = world.invoke(&snapshot.id, PHONE_CALL, CALL).await else {
        panic!("every call mints a card");
    };
    world
        .broker
        .resolve(&first.request.id, RequestState::Approved)
        .await
        .unwrap();
    // The same number, the same brief: there is no allow rule for a
    // telephone number, so the second call asks again.
    let outcome = world.invoke(&snapshot.id, PHONE_CALL, CALL).await;
    let InvokeOutcome::Waiting(second) = outcome else {
        panic!("a repeated call asks again: {outcome:?}");
    };
    assert_ne!(first.request.id, second.request.id);
    assert_eq!(world.executor.calls.lock().unwrap().len(), 1);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn to_takes_e164_only(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_number().await;
    let snapshot = world.snapshot().await;
    for to in [
        "415-555-0199",
        "4155550199",
        "+0155550199",
        "+1 415 555 0199",
    ] {
        let arguments = serde_json::json!({
            "to": to,
            "brief": "Ask about the table.",
            "success_criteria": "A yes or a no."
        })
        .to_string();
        let outcome = world.invoke(&snapshot.id, PHONE_CALL, &arguments).await;
        let InvokeOutcome::Rejected(result) = outcome else {
            panic!("{to} is not E.164: {outcome:?}");
        };
        assert_eq!(result.code.as_deref(), Some("invalid_request"));
    }
    assert!(world.executor.calls.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_free_tools_of_a_run_leave_out_every_gate(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_number().await;
    world.snapshot().await;
    let free = world
        .broker
        .free_tools(&world.workspace_id, &world.run_id)
        .await
        .unwrap();
    let names: Vec<&str> = free.iter().map(|tool| tool.name.as_str()).collect();
    assert!(names.contains(&"memory_read"));
    assert!(names.contains(&"memory_write"));
    assert!(!names.contains(&PHONE_CALL));
    assert!(!names.contains(&"host_shell"));
    assert!(!names.contains(&"schedule_create"));
    // A question parks the Run on a card, which a Remote Party cannot
    // wait for.
    assert!(!names.contains(&"ask_user"));
}
