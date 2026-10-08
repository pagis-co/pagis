//! The `mail__*` tools at the broker (ADR-0019): installed
//! for an Agent that holds a mailbox or a granted mail account,
//! resolved to the mailbox the call names, gated per target, and
//! refused by the Outgoing Cap before any card appears.
//!
//! The two targets are one tool set and two routes. `own` is the
//! Agent's own mailbox, held with no Grant. An alias is one of the
//! user's mail accounts, and the call runs as a Connection call under
//! the Grant for that account, with the capability the tool acts with.
//!
//! What the tools do inside the mailbox is the mail crate's own test.
//! Here the executor only records the call it was handed.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_audit::AuditEventBus;
use pagis_broker::{
    AuthorizedCall, Broker, BrokerDeps, InvokeOutcome, MAIL_GET_MESSAGE, MAIL_GET_THREAD,
    MAIL_SEARCH, MAIL_SEND, MailTool, ToolCall, ToolExecutor, ToolResult, ToolRoute,
};
use pagis_core::{
    Agent, AgentId, AgentMailbox, AgentMailboxId, AgentMailboxState, AgentMailboxStore,
    AgentStatus, AgentStore, Channel, ChannelId, ChannelKind, ChannelStore, ConnectionId, EventBus,
    Grant, GrantId, GrantStore, MessageId, Request, RequestState, RequestStore, Run, RunId,
    RunState, RunStore, TriggerKind, Workspace, WorkspaceId, WorkspaceStore, day_start, now_ms,
};
use pagis_storage_sqlite::{
    SqliteAgentMailboxStore, SqliteAgentStore, SqliteCapabilitySnapshotStore, SqliteChannelStore,
    SqliteConnectionStore, SqliteEventLog, SqliteGrantStore, SqlitePhoneNumberStore,
    SqliteRequestStore, SqliteRunStore, SqliteWorkspaceStore,
};
use sqlx::SqlitePool;

const ADDRESS: &str = "ada@example.com";

/// The alias of the user's Gmail account in these tests.
const ALIAS: &str = "work";

#[derive(Default)]
struct RecordingExecutor {
    calls: Mutex<Vec<AuthorizedCall>>,
}

#[async_trait]
impl ToolExecutor for RecordingExecutor {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        self.calls.lock().unwrap().push(call);
        ToolResult::success(r#"{"messages": []}"#)
    }
}

struct World {
    broker: Broker,
    executor: Arc<RecordingExecutor>,
    mailboxes: Arc<SqliteAgentMailboxStore>,
    requests: Arc<SqliteRequestStore>,
    pool: SqlitePool,
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
            home_exit_host_id: None,
        };
        SqliteWorkspaceStore::new(pool.clone())
            .create(&workspace)
            .await
            .unwrap();
        let agent = Agent {
            id: AgentId::generate(),
            workspace_id: workspace.id.clone(),
            name: "Ada".to_string(),
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
        let mailboxes = Arc::new(SqliteAgentMailboxStore::new(pool.clone()));
        let requests = Arc::new(SqliteRequestStore::new(pool.clone()));
        let broker = Broker::new(BrokerDeps {
            hosts: Arc::new(pagis_storage_sqlite::SqliteHostStore::new(pool.clone())),
            presence: Arc::new(pagis_broker::HostPresence::new()),
            forget: Arc::new(pagis_storage_sqlite::SqliteForgetStore::new(pool.clone())),
            grants: Arc::new(SqliteGrantStore::new(pool.clone())),
            credentials: Arc::new(pagis_broker::NoCredentialDirectory),
            connections: Arc::new(SqliteConnectionStore::new(pool.clone())),
            requests: Arc::clone(&requests) as _,
            snapshots: Arc::new(SqliteCapabilitySnapshotStore::new(pool.clone())),
            runs: Arc::new(SqliteRunStore::new(pool.clone())),
            conversation_evidence: Arc::new(
                pagis_storage_sqlite::SqliteConversationEvidenceStore::new(pool.clone()),
            ),
            bus,
            executor: Arc::clone(&executor) as _,
            phone_numbers: Arc::new(SqlitePhoneNumberStore::new(pool.clone())),
            mailboxes: Arc::clone(&mailboxes) as _,
            session_starts: Arc::new(pagis_broker::NoSessionStarts),
        });
        Self {
            broker,
            executor,
            mailboxes,
            requests,
            pool,
            workspace_id: workspace.id,
            agent_id: agent.id,
            run_id: run.id,
        }
    }

    /// Connect one of the user's Gmail accounts and grant this Agent
    /// the named capabilities on it.
    async fn grant_gmail(&self, capabilities: &[&str]) {
        let connection_id = ConnectionId::generate();
        sqlx::query(
            "INSERT INTO connections \
             (id, workspace_id, provider, alias, display_name, status, auth_mode, config, \
             created_at) \
             VALUES (?, ?, 'google', ?, 'Work Google', 'connected', 'byo', '{}', ?)",
        )
        .bind(connection_id.as_str())
        .bind(self.workspace_id.as_str())
        .bind(ALIAS)
        .bind(now_ms())
        .execute(&self.pool)
        .await
        .unwrap();
        SqliteGrantStore::new(self.pool.clone())
            .create(&Grant {
                id: GrantId::generate(),
                workspace_id: self.workspace_id.clone(),
                agent_id: self.agent_id.clone(),
                resource_kind: Grant::CONNECTION_KIND.to_string(),
                resource_id: Some(connection_id.to_string()),
                scope: Grant::connection_scope(
                    &capabilities
                        .iter()
                        .map(|name| (*name).to_string())
                        .collect::<Vec<_>>(),
                ),
                revision: 1,
                created_at: now_ms(),
                revoked_at: None,
            })
            .await
            .unwrap();
    }

    /// Give the Agent a mailbox in one state, with one Outgoing Cap.
    async fn hold_mailbox(&self, state: AgentMailboxState, outgoing_cap: u32) -> AgentMailbox {
        let mailbox = AgentMailbox {
            id: AgentMailboxId::generate(),
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent_id.clone(),
            connection_id: ConnectionId::generate(),
            address: ADDRESS.to_string(),
            state: AgentMailboxState::Provisioning,
            reason: None,
            outgoing_cap,
            allow_rules: Vec::new(),
            cursor: None,
            sends_day: None,
            sends_today: 0,
            created_at: now_ms(),
            deleted_at: None,
        };
        self.mailboxes.create(&mailbox).await.unwrap();
        let reason = (state == AgentMailboxState::Unavailable)
            .then_some("the host refused the login (unauthorized)");
        self.mailboxes
            .set_state(&self.workspace_id, &mailbox.id, state, reason)
            .await
            .unwrap();
        self.mailboxes
            .for_agent(&self.workspace_id, &self.agent_id)
            .await
            .unwrap()
            .expect("the mailbox")
    }

    async fn snapshot(&self) -> pagis_broker::CapabilitySnapshot {
        self.broker
            .prepare_run(&self.workspace_id, &self.agent_id, &self.run_id)
            .await
            .unwrap()
    }

    async fn invoke(&self, tool: MailTool, arguments: serde_json::Value) -> InvokeOutcome {
        let snapshot = self.snapshot().await;
        self.broker
            .invoke(
                &self.workspace_id,
                &self.run_id,
                &snapshot.id,
                ToolCall::new(tool.as_str(), arguments.to_string()),
            )
            .await
            .unwrap()
    }
}

fn send(to: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "mailbox": "own",
        "to": to,
        "subject": "Invoice 42",
        "body": "it is paid"
    })
}

fn card(outcome: &InvokeOutcome) -> &Request {
    match outcome {
        InvokeOutcome::Waiting(pending) => &pending.request,
        other => panic!("the send waits for the user, not {other:?}"),
    }
}

fn error_code(outcome: &InvokeOutcome) -> Option<String> {
    outcome.result().and_then(|result| result.code.clone())
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_agent_with_no_mailbox_sees_no_mail_tool(pool: SqlitePool) {
    let world = World::build(pool).await;

    let names: Vec<String> = world
        .snapshot()
        .await
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect();

    assert!(!names.iter().any(|name| name.starts_with("mail__")));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_mailbox_installs_the_five_tools_and_names_the_address(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_mailbox(AgentMailboxState::Active, 20).await;

    let tools = world.snapshot().await.tools;

    let mail: Vec<&pagis_broker::ToolDef> = tools
        .iter()
        .filter(|tool| tool.name.starts_with("mail__"))
        .collect();
    assert_eq!(mail.len(), 5);
    let search = mail
        .iter()
        .find(|tool| tool.name == MAIL_SEARCH)
        .expect("the search tool");
    assert!(search.description.contains(ADDRESS));
    assert_eq!(
        search.parameters["properties"]["mailbox"]["enum"],
        serde_json::json!(["own"])
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_read_runs_free_and_comes_back_inside_the_untrusted_envelope(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_mailbox(AgentMailboxState::Active, 20).await;

    let outcome = world
        .invoke(
            MailTool::Search,
            serde_json::json!({"mailbox": "own", "query": "from:bob@other.test"}),
        )
        .await;

    let InvokeOutcome::Completed(result) = &outcome else {
        panic!("a read needs no card: {outcome:?}");
    };
    // Mail is written by whoever sent it, so the answer reaches the
    // model inside the untrusted envelope.
    assert!(
        result.content.starts_with("[BEGIN UNTRUSTED source="),
        "{}",
        result.content
    );
    let calls = world.executor.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].route,
        ToolRoute::Mailbox {
            tool: MailTool::Search
        }
    );
    // The mailbox is the Agent's own identity, so no Grant is bound.
    assert!(calls[0].grant_id.is_none());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_send_waits_for_a_card_that_names_the_address_and_the_domains(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_mailbox(AgentMailboxState::Active, 20).await;

    let outcome = world
        .invoke(MailTool::Send, send(&["bob@mail.other.test"]))
        .await;

    let request = card(&outcome);
    assert_eq!(request.kind, Request::TOOL_ACTION_KIND);
    assert_eq!(
        request.payload["action_title"],
        format!("Send an email from {ADDRESS}")
    );
    assert_eq!(request.payload["body"], "Invoice 42");
    assert_eq!(
        request.payload["proposed_rules"],
        serde_json::json!(["other.test"])
    );
    assert!(world.executor.calls.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_approved_send_dispatches_and_a_denied_one_does_not(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_mailbox(AgentMailboxState::Active, 20).await;

    let waiting = world
        .invoke(MailTool::Send, send(&["bob@other.test"]))
        .await;
    let approved = world
        .broker
        .resolve(&card(&waiting).id, RequestState::Approved)
        .await
        .unwrap();

    assert!(matches!(approved, InvokeOutcome::Completed(_)));
    assert_eq!(world.executor.calls.lock().unwrap().len(), 1);

    let waiting = world
        .invoke(MailTool::Send, send(&["bob@other.test"]))
        .await;
    let denied = world
        .broker
        .resolve(&card(&waiting).id, RequestState::Denied)
        .await
        .unwrap();

    assert_eq!(error_code(&denied).as_deref(), Some("permission_denied"));
    assert_eq!(world.executor.calls.lock().unwrap().len(), 1);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_allow_rule_on_the_mailbox_sends_with_no_card(pool: SqlitePool) {
    let world = World::build(pool).await;
    let mailbox = world.hold_mailbox(AgentMailboxState::Active, 20).await;
    world
        .mailboxes
        .add_allow_rules(
            &world.workspace_id,
            &mailbox.id,
            &["other.test".to_string()],
        )
        .await
        .unwrap();

    let outcome = world
        .invoke(
            MailTool::Send,
            send(&["bob@other.test", "eve@mail.other.test"]),
        )
        .await;

    assert!(
        matches!(outcome, InvokeOutcome::Completed(_)),
        "the rule covers every recipient: {outcome:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_rule_does_not_widen_from_the_arguments_of_a_later_send(pool: SqlitePool) {
    let world = World::build(pool).await;
    let mailbox = world.hold_mailbox(AgentMailboxState::Active, 20).await;
    world
        .mailboxes
        .add_allow_rules(
            &world.workspace_id,
            &mailbox.id,
            &["other.test".to_string()],
        )
        .await
        .unwrap();

    // One recipient outside the rules sends the whole call to the card,
    // so an added address cannot ride on an approved domain.
    let outcome = world
        .invoke(
            MailTool::Send,
            send(&["bob@other.test", "eve@elsewhere.test"]),
        )
        .await;

    assert_eq!(
        card(&outcome).payload["proposed_rules"],
        serde_json::json!(["other.test", "elsewhere.test"])
    );
    assert!(world.executor.calls.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_outgoing_cap_answers_before_any_card(pool: SqlitePool) {
    let world = World::build(pool).await;
    let mailbox = world.hold_mailbox(AgentMailboxState::Active, 1).await;
    world
        .mailboxes
        .note_send(&world.workspace_id, &mailbox.id, now_ms())
        .await
        .unwrap();

    let outcome = world
        .invoke(MailTool::Send, send(&["bob@other.test"]))
        .await;

    assert_eq!(
        error_code(&outcome).as_deref(),
        Some("outgoing_cap_reached")
    );
    let pending = world
        .requests
        .list_by_state(&world.workspace_id, RequestState::Pending, None)
        .await
        .unwrap();
    assert!(pending.is_empty(), "the cap answers before any card");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_tally_from_an_earlier_day_costs_no_allowance(pool: SqlitePool) {
    let world = World::build(pool).await;
    let mailbox = world.hold_mailbox(AgentMailboxState::Active, 1).await;
    let yesterday = day_start(now_ms()) - 1;
    world
        .mailboxes
        .note_send(&world.workspace_id, &mailbox.id, yesterday)
        .await
        .unwrap();

    let outcome = world
        .invoke(MailTool::Send, send(&["bob@other.test"]))
        .await;

    assert!(matches!(outcome, InvokeOutcome::Waiting(_)));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_dormant_mailbox_keeps_the_tools_and_answers_mailbox_unavailable(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_mailbox(AgentMailboxState::Dormant, 20).await;

    let names: Vec<String> = world
        .snapshot()
        .await
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    assert!(names.iter().any(|name| name == MAIL_SEND));

    let outcome = world
        .invoke(
            MailTool::Search,
            serde_json::json!({"mailbox": "own", "query": ""}),
        )
        .await;

    assert_eq!(error_code(&outcome).as_deref(), Some("mailbox_unavailable"));
    assert!(world.executor.calls.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_unavailable_mailbox_answers_with_the_reason(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_mailbox(AgentMailboxState::Unavailable, 20).await;

    let outcome = world
        .invoke(MailTool::Send, send(&["bob@other.test"]))
        .await;

    let result = outcome.result().expect("a refusal");
    assert_eq!(result.code.as_deref(), Some("mailbox_unavailable"));
    assert!(result.content.contains("the host refused the login"));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_send_with_no_recipient_is_a_tool_error(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_mailbox(AgentMailboxState::Active, 20).await;

    let outcome = world
        .invoke(
            MailTool::Send,
            serde_json::json!({
                "mailbox": "own",
                "to": ["not-an-address"],
                "subject": "Invoice 42",
                "body": "it is paid"
            }),
        )
        .await;

    assert_eq!(error_code(&outcome).as_deref(), Some("invalid_request"));
}

// ----- the user's own mail accounts -----

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_granted_account_installs_the_tools_that_capability_reaches(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.grant_gmail(&["gmail_read"]).await;

    let tools = world.snapshot().await.tools;

    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .filter(|name| name.starts_with("mail__"))
        .collect();
    // `gmail_read` reaches the three reads and nothing else: a send
    // needs `gmail_send`, and a state change needs `gmail_modify`.
    assert_eq!(
        names,
        [MAIL_SEARCH, MAIL_GET_MESSAGE, MAIL_GET_THREAD],
        "one capability installs the tools it reaches"
    );
    let search = tools
        .iter()
        .find(|tool| tool.name == MAIL_SEARCH)
        .expect("the search tool");
    // The Agent holds no mailbox of its own, so `own` is not a choice.
    assert_eq!(
        search.parameters["properties"]["mailbox"]["enum"],
        serde_json::json!([ALIAS])
    );
    assert!(search.description.contains("`work`"));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn both_identities_are_named_in_one_tool(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_mailbox(AgentMailboxState::Active, 20).await;
    world.grant_gmail(&["gmail_read", "gmail_send"]).await;

    let tools = world.snapshot().await.tools;

    let send = tools
        .iter()
        .find(|tool| tool.name == MAIL_SEND)
        .expect("the send tool");
    assert_eq!(
        send.parameters["properties"]["mailbox"]["enum"],
        serde_json::json!(["own", ALIAS])
    );
    assert!(send.description.contains(ADDRESS) && send.description.contains("`work`"));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_read_in_a_granted_account_runs_on_that_connection_under_its_grant(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.grant_gmail(&["gmail_read"]).await;

    let outcome = world
        .invoke(
            MailTool::Search,
            serde_json::json!({"mailbox": ALIAS, "query": "invoice"}),
        )
        .await;

    assert!(
        matches!(outcome, InvokeOutcome::Completed(_)),
        "{outcome:?}"
    );
    let calls = world.executor.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    // The alias resolves to the Connection route, so the adapter that
    // owns the account runs the call (ADR-0019).
    assert_eq!(
        calls[0].route,
        ToolRoute::Connection {
            provider: "google".to_string()
        }
    );
    assert!(calls[0].selected_connection.is_some());
    assert!(
        calls[0].grant_id.is_some(),
        "the account is held by a Grant"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_send_from_an_account_is_approved_once_and_names_the_alias(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.grant_gmail(&["gmail_send"]).await;

    let outcome = world
        .invoke(
            MailTool::Send,
            serde_json::json!({
                "mailbox": ALIAS,
                "to": ["bob@other.test"],
                "subject": "Invoice 42",
                "body": "it is paid"
            }),
        )
        .await;

    let request = card(&outcome);
    assert_eq!(
        request.payload["action_title"],
        format!("Send an email from the user's Gmail ({ALIAS})")
    );
    assert_eq!(request.payload["body"], "Invoice 42");
    // A recipient-domain rule lives on the Agent's own mailbox record.
    // A send from the user's account offers `Approve once` and `Deny`.
    assert_eq!(request.payload["proposed_rules"], serde_json::json!([]));
    assert!(request.payload["mailbox_id"].is_null());

    let approved = world
        .broker
        .resolve(&request.id, RequestState::Approved)
        .await
        .unwrap();
    assert!(matches!(approved, InvokeOutcome::Completed(_)));
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_rule_on_the_own_mailbox_never_relaxes_a_send_from_an_account(pool: SqlitePool) {
    let world = World::build(pool).await;
    let mailbox = world.hold_mailbox(AgentMailboxState::Active, 20).await;
    world.grant_gmail(&["gmail_send"]).await;
    world
        .mailboxes
        .add_allow_rules(
            &world.workspace_id,
            &mailbox.id,
            &["other.test".to_string()],
        )
        .await
        .unwrap();

    // The same recipients from the Agent's own mailbox run with no card.
    let own = world
        .invoke(MailTool::Send, send(&["bob@other.test"]))
        .await;
    assert!(matches!(own, InvokeOutcome::Completed(_)), "{own:?}");

    let account = world
        .invoke(
            MailTool::Send,
            serde_json::json!({
                "mailbox": ALIAS,
                "to": ["bob@other.test"],
                "subject": "Invoice 42",
                "body": "it is paid"
            }),
        )
        .await;

    assert!(
        matches!(account, InvokeOutcome::Waiting(_)),
        "a rule on one identity cannot speak for the other: {account:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_outgoing_cap_holds_the_own_mailbox_and_not_the_users_account(pool: SqlitePool) {
    let world = World::build(pool).await;
    let mailbox = world.hold_mailbox(AgentMailboxState::Active, 1).await;
    world.grant_gmail(&["gmail_send"]).await;
    world
        .mailboxes
        .note_send(&world.workspace_id, &mailbox.id, day_start(now_ms()))
        .await
        .unwrap();

    let own = world
        .invoke(MailTool::Send, send(&["bob@other.test"]))
        .await;
    assert_eq!(
        error_code(&own).as_deref(),
        Some(pagis_broker::OUTGOING_CAP_REACHED)
    );

    let account = world
        .invoke(
            MailTool::Send,
            serde_json::json!({
                "mailbox": ALIAS,
                "to": ["bob@other.test"],
                "subject": "Invoice 42",
                "body": "it is paid"
            }),
        )
        .await;
    assert!(
        matches!(account, InvokeOutcome::Waiting(_)),
        "the cap is the Agent's own mailbox's, not the user's account's: {account:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_mailbox_name_the_agent_does_not_hold_is_a_schema_error(pool: SqlitePool) {
    let world = World::build(pool).await;
    world.hold_mailbox(AgentMailboxState::Active, 20).await;

    let outcome = world
        .invoke(
            MailTool::Search,
            serde_json::json!({"mailbox": "somebody-elses", "query": "invoice"}),
        )
        .await;

    assert_eq!(error_code(&outcome).as_deref(), Some("invalid_request"));
    assert!(world.executor.calls.lock().unwrap().is_empty());
}
