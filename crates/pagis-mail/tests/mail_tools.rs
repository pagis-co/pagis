//! The `mail__*` executor (ADR-0019) against the fake transport:
//! each tool, the answer caps, the truncation marker, the stale id, the
//! Outgoing Cap tally and the sent-mail record.
//!
//! The broker is not here. It resolves the mailbox, refuses a call the
//! mailbox cannot take and mints the card; those are the broker's own
//! tests. This is what runs after that.

use std::sync::Arc;
use std::time::Duration;

use pagis_audit::AuditEventBus;
use pagis_broker::{
    AuthorizedCall, MAIL_GET_MESSAGE, MAIL_GET_THREAD, MAIL_MODIFY_MESSAGE, MAIL_SEARCH, MAIL_SEND,
    MailTool, ToolExecutor, ToolResult, ToolRoute,
};
use pagis_core::{
    Agent, AgentId, AgentMailbox, AgentMailboxState, AgentMailboxStore, AgentStatus, AgentStore,
    Channel, ChannelId, ChannelKind, ChannelStore, Connection, ConnectionId, ConnectionStore,
    MemorySecretStore, Message, MessageId, MessageStatus, MessageStore, Run, RunId, RunState,
    RunStore, SecretStore, SentMailStore, TriggerKind, Workspace, WorkspaceId, WorkspaceStore,
    now_ms,
};
use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost, FakeStandingRule};
use pagis_mail::{
    Endpoint, MAIL_TRANSPORT, MAX_SUMMARIES, MAX_TEXT_BYTES, MAX_THREAD_MESSAGES, MIGADU_PROVIDER,
    MailBlockDeps, MailBlocks, MailToolDeps, MailToolRuntime, MailboxCapabilities, MailboxDesk,
    MailboxDeskDeps, MailboxHost, MailboxProvider, NewMailbox, ProofTiming, TRUNCATION_MARKER,
    host_api_key_secret_name,
};
use pagis_storage_sqlite::{
    SqliteAgentMailboxStore, SqliteAgentStore, SqliteChannelStore, SqliteConnectionStore,
    SqliteEventLog, SqliteMessageStore, SqliteRunStore, SqliteSentMailStore, SqliteTriggerStore,
    SqliteWorkspaceStore,
};
use sqlx::SqlitePool;

const ALIAS: &str = "mail";

struct World {
    runtime: MailToolRuntime,
    mailboxes: Arc<SqliteAgentMailboxStore>,
    messages: Arc<SqliteMessageStore>,
    sent: Arc<SqliteSentMailStore>,
    transport: Arc<FakeMailTransport>,
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    run_id: RunId,
    thread_id: MessageId,
    mailbox: AgentMailbox,
}

impl World {
    async fn new(pool: SqlitePool) -> Self {
        let workspaces = SqliteWorkspaceStore::new(pool.clone());
        let person =
            pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", pagis_core::now_ms())
                .await
                .unwrap();
        let workspace = Workspace {
            user_id: person.id.clone(),
            id: WorkspaceId::generate(),
            name: "Home".to_string(),
            timezone: "UTC".to_string(),
            onboarded_at: None,
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
            home_exit_host_id: None,
            created_at: now_ms(),
        };
        workspaces.create(&workspace).await.unwrap();

        let agents = Arc::new(SqliteAgentStore::new(pool.clone()));
        let agent = Agent {
            id: AgentId::generate(),
            workspace_id: workspace.id.clone(),
            name: "Ada Lovelace".to_string(),
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
        agents.create(&agent).await.unwrap();

        let connections = Arc::new(SqliteConnectionStore::new(pool.clone()));
        let secrets = Arc::new(MemorySecretStore::default());
        let host = Arc::new(FakeMailboxHost::default());
        let connection = mail_connection(
            &workspace.id,
            MailboxCapabilities::of(host.capabilities(), MAIL_TRANSPORT),
        );
        connections.create(&connection).await.unwrap();
        secrets
            .set(&host_api_key_secret_name(ALIAS), "migadu-key")
            .unwrap();

        let mailboxes = Arc::new(SqliteAgentMailboxStore::new(pool.clone()));
        let transport = Arc::new(FakeMailTransport::default());
        let desk = Arc::new(MailboxDesk::new(MailboxDeskDeps {
            mailboxes: Arc::clone(&mailboxes) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: workspace.id.clone(),
            agents: Arc::clone(&agents) as _,
            host: Arc::clone(&host) as _,
            transport: Arc::clone(&transport) as _,
            secrets: Arc::clone(&secrets) as _,
            bus: Arc::new(AuditEventBus::new(Arc::new(SqliteEventLog::new(
                pool.clone(),
            )))),
            standing_rule: Arc::new(FakeStandingRule::default()),
            proof: ProofTiming {
                first_wait: Duration::from_millis(1),
                max_wait: Duration::from_millis(1),
                budget: Duration::ZERO,
            },
        }));
        let mailbox = desk
            .provision(
                &workspace.id,
                &agent.id,
                NewMailbox {
                    connection_id: connection.id.clone(),
                    local_part: "ada".to_string(),
                    outgoing_cap: Some(2),
                    password: None,
                },
            )
            .await
            .unwrap();
        let mailbox = settled(&mailboxes, &workspace.id, &mailbox).await;
        assert_eq!(mailbox.state, AgentMailboxState::Active);

        // A Run in a Thread, so a send is remembered where it came from.
        let channels = SqliteChannelStore::new(pool.clone());
        let channel = Channel {
            id: ChannelId::generate(),
            workspace_id: workspace.id.clone(),
            kind: ChannelKind::Group,
            title: Some("mail".to_string()),
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        channels.create(&channel).await.unwrap();
        let messages = Arc::new(SqliteMessageStore::new(pool.clone()));
        let root = Message {
            id: MessageId::generate(),
            workspace_id: workspace.id.clone(),
            channel_id: channel.id.clone(),
            parent_message_id: None,
            author_kind: pagis_core::AuthorKind::User,
            author_agent_id: None,
            run_id: None,
            status: MessageStatus::Complete,
            blocks: Vec::new(),
            text_content: "answer Bob".to_string(),
            pending_id: None,
            created_at: now_ms(),
            completed_at: Some(now_ms()),
        };
        messages.insert(&root).await.unwrap();
        let runs = Arc::new(SqliteRunStore::new(pool.clone()));
        let run = Run {
            title: "A message with an attachment".into(),
            id: RunId::generate(),
            workspace_id: workspace.id.clone(),
            agent_id: agent.id.clone(),
            channel_id: Some(channel.id.clone()),
            root_message_id: Some(root.id.clone()),
            trigger_kind: TriggerKind::Message,
            trigger_ref: None,
            hop_count: 0,
            origin: None,
            state: RunState::Running,
            failure_kind: None,
            dismissed_at: None,
            error: None,
            started_at: Some(now_ms()),
            ended_at: None,
            created_at: now_ms(),
        };
        runs.create(&run).await.unwrap();

        let sent = Arc::new(SqliteSentMailStore::new(pool.clone()));
        let blocks = Arc::new(MailBlocks::new(MailBlockDeps {
            grants: Arc::new(pagis_storage_sqlite::SqliteGrantStore::new(pool.clone())),
            messages: Arc::clone(&messages) as _,
            bus: Arc::new(AuditEventBus::new(Arc::new(SqliteEventLog::new(
                pool.clone(),
            )))),
            triggers: Arc::new(SqliteTriggerStore::new(pool.clone())),
            desk: Arc::clone(&desk),
        }));
        let runtime = MailToolRuntime::new(MailToolDeps {
            desk,
            runs: Arc::clone(&runs) as _,
            sent: Arc::clone(&sent) as _,
            blocks,
        });
        Self {
            runtime,
            mailboxes,
            messages,
            sent,
            transport,
            workspace_id: workspace.id,
            agent_id: agent.id,
            run_id: run.id,
            thread_id: root.id,
            mailbox,
        }
    }

    async fn call(&self, tool: MailTool, arguments: serde_json::Value) -> ToolResult {
        self.runtime
            .execute(AuthorizedCall {
                workspace_id: self.workspace_id.clone(),
                agent_id: self.agent_id.clone(),
                run_id: self.run_id.clone(),
                tool_name: tool.as_str().to_string(),
                source_version: "test".to_string(),
                route: ToolRoute::Mailbox { tool },
                arguments,
                selected_connection: None,
                grant_id: None,
                grant_revision: None,
                call_timeout: None,
                tool_call_id: None,
                host: None,
                approved_by_rule: false,
            })
            .await
    }

    /// The answer as JSON. A tool error fails the test with its code.
    async fn answer(&self, tool: MailTool, arguments: serde_json::Value) -> serde_json::Value {
        let result = self.call(tool, arguments).await;
        assert!(!result.is_error, "{tool:?} failed: {}", result.content);
        serde_json::from_str(&result.content).expect("the answer is JSON")
    }
}

fn mail_connection(workspace_id: &WorkspaceId, capabilities: MailboxCapabilities) -> Connection {
    let settings = MailboxProvider {
        account: Some("owner@example.com".to_string()),
        domain: "example.com".to_string(),
        imap: Endpoint::new("imap.example.net", 993),
        smtp: Endpoint::new("smtp.example.net", 465),
        capabilities,
    };
    Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace_id.clone(),
        provider: MIGADU_PROVIDER.to_string(),
        alias: ALIAS.to_string(),
        display_name: "Example mail".to_string(),
        status: Connection::CONNECTED.to_string(),
        auth_mode: Connection::AUTH_MODE_BYO.to_string(),
        authorized_capabilities: Vec::new(),
        config: settings.config(),
        created_at: now_ms(),
    }
}

async fn settled(
    mailboxes: &SqliteAgentMailboxStore,
    workspace_id: &WorkspaceId,
    mailbox: &AgentMailbox,
) -> AgentMailbox {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = mailboxes
                .get(workspace_id, &mailbox.id)
                .await
                .unwrap()
                .expect("the record stays");
            if record.state != AgentMailboxState::Provisioning {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the login proof settles")
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_search_answers_summaries_with_a_snippet(pool: SqlitePool) {
    let world = World::new(pool).await;
    world
        .transport
        .deliver("bob@other.test", "Invoice 42", "the invoice is late");
    world
        .transport
        .deliver("eve@other.test", "Lunch", "are you free");

    let answer = world
        .answer(
            MailTool::Search,
            serde_json::json!({"mailbox": "own", "query": "from:bob@other.test"}),
        )
        .await;

    let messages = answer["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["subject"], "Invoice 42");
    assert_eq!(messages[0]["snippet"], "the invoice is late");
    assert!(answer["page"].is_null());
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_search_answers_one_page_of_summaries_and_a_page_token(pool: SqlitePool) {
    let world = World::new(pool).await;
    for index in 0..(MAX_SUMMARIES + 5) {
        world
            .transport
            .deliver("bob@other.test", format!("Note {index}"), "text");
    }

    let first = world
        .answer(
            MailTool::Search,
            serde_json::json!({"mailbox": "own", "query": ""}),
        )
        .await;

    assert_eq!(
        first["messages"].as_array().unwrap().len(),
        MAX_SUMMARIES as usize
    );
    let page = first["page"].as_str().expect("a page token");

    let second = world
        .answer(
            MailTool::Search,
            serde_json::json!({"mailbox": "own", "query": "", "page": page}),
        )
        .await;

    assert_eq!(second["messages"].as_array().unwrap().len(), 5);
    assert!(second["page"].is_null());
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_message_over_the_text_cap_ends_with_the_truncation_marker(pool: SqlitePool) {
    let world = World::new(pool).await;
    let id = world
        .transport
        .deliver("bob@other.test", "Long", "x".repeat(MAX_TEXT_BYTES + 100));

    let answer = world
        .answer(
            MailTool::GetMessage,
            serde_json::json!({"mailbox": "own", "message_id": id.to_string()}),
        )
        .await;

    let message = &answer["message"];
    assert_eq!(message["truncated"], true);
    let text = message["text"].as_str().expect("text");
    assert!(text.ends_with(TRUNCATION_MARKER));
    assert_eq!(text.len(), MAX_TEXT_BYTES + TRUNCATION_MARKER.len());
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_thread_answers_its_newest_messages_and_a_page_token(pool: SqlitePool) {
    let world = World::new(pool).await;
    let root = world
        .transport
        .deliver("bob@other.test", "Invoice 42", "first");
    let summaries = world.transport.summaries("INBOX");
    let thread_id = summaries[0].thread_id.clone();
    for index in 0..MAX_THREAD_MESSAGES {
        world.transport.deliver_reply(
            &thread_id,
            "bob@other.test",
            "Re: Invoice 42",
            format!("reply {index}"),
        );
    }
    assert_eq!(root.folder, "INBOX");

    let answer = world
        .answer(
            MailTool::GetThread,
            serde_json::json!({"mailbox": "own", "thread_id": thread_id}),
        )
        .await;

    let messages = answer["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), MAX_THREAD_MESSAGES);
    // Oldest first inside the page, and the oldest message of the
    // thread is on the next page.
    assert_eq!(messages[0]["text"], "reply 0");
    assert_eq!(answer["page"].as_str(), Some("20"));

    let older = world
        .answer(
            MailTool::GetThread,
            serde_json::json!({"mailbox": "own", "thread_id": thread_id, "page": "20"}),
        )
        .await;

    assert_eq!(older["messages"].as_array().unwrap().len(), 1);
    assert_eq!(older["messages"][0]["text"], "first");
    assert!(older["page"].is_null());
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_send_leaves_the_mailbox_counts_against_the_cap_and_is_remembered(pool: SqlitePool) {
    let world = World::new(pool).await;

    let answer = world
        .answer(
            MailTool::Send,
            serde_json::json!({
                "mailbox": "own",
                "to": ["bob@other.test"],
                "subject": "Invoice 42",
                "body": "it is paid"
            }),
        )
        .await;

    let sent = world.transport.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, "Invoice 42");
    assert_eq!(answer["sends_today"], 1);
    assert_eq!(answer["outgoing_cap"], 2);

    let message_id = answer["message_id"].as_str().expect("a message id");
    let record = world
        .sent
        .get(&world.workspace_id, message_id)
        .await
        .unwrap()
        .expect("a record");
    assert_eq!(record.run_id, world.run_id);
    assert_eq!(record.thread_id.as_ref(), Some(&world.thread_id));
    assert_eq!(record.mailbox_id, world.mailbox.id);

    let after = world
        .mailboxes
        .for_agent(&world.workspace_id, &world.agent_id)
        .await
        .unwrap()
        .expect("the mailbox");
    assert_eq!(after.sends_today, 1);
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_send_mints_a_mail_block_in_the_thread_that_sent_it(pool: SqlitePool) {
    let world = World::new(pool).await;

    world
        .answer(
            MailTool::Send,
            serde_json::json!({
                "mailbox": "own",
                "to": ["bob@other.test"],
                "subject": "Invoice 42",
                "body": "it is paid"
            }),
        )
        .await;

    let thread = world
        .messages
        .list_thread(&world.workspace_id, &world.thread_id)
        .await
        .unwrap();
    let block = thread
        .iter()
        .flat_map(|message| message.blocks.iter())
        .find_map(|block| match block {
            pagis_core::Block::Known(pagis_core::KnownBlock::Mail { .. }) => Some(block.clone()),
            _ => None,
        })
        .expect("the send left a mail block");
    let json = serde_json::to_value(&block).unwrap();
    assert_eq!(json["direction"], "outbound");
    assert_eq!(json["mailbox"], world.mailbox.address);
    assert_eq!(json["counterpart"], "bob@other.test");
    assert_eq!(json["subject"], "Invoice 42");
    assert!(json["trust_tier"].is_null(), "a sent mail has no tier");
    // The words stay at the host: the block carries the envelope only.
    assert!(json.get("body").is_none());
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_reply_is_a_send_that_names_the_message_it_answers(pool: SqlitePool) {
    let world = World::new(pool).await;

    world
        .answer(
            MailTool::Send,
            serde_json::json!({
                "mailbox": "own",
                "to": ["bob@other.test"],
                "subject": "Re: Invoice 42",
                "body": "it is paid",
                "in_reply_to": "<first@other.test>"
            }),
        )
        .await;

    assert_eq!(
        world.transport.sent()[0].in_reply_to.as_deref(),
        Some("<first@other.test>")
    );
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_modify_marks_read_flags_and_archives_one_message(pool: SqlitePool) {
    let world = World::new(pool).await;
    let id = world.transport.deliver("bob@other.test", "Invoice", "text");

    let answer = world
        .answer(
            MailTool::ModifyMessage,
            serde_json::json!({
                "mailbox": "own",
                "message_id": id.to_string(),
                "mark_read": true,
                "flag": true,
                "archive": true
            }),
        )
        .await;

    assert_eq!(answer["archived"], true);
    assert_eq!(answer["folder"], "Archive");
    assert!(world.transport.summaries("INBOX").is_empty());
    let archived = world.transport.summaries("Archive");
    assert_eq!(archived.len(), 1);
    let flags = world
        .transport
        .flags_of(&archived[0].id)
        .expect("the flags follow the message");
    assert!(flags.seen && flags.flagged);
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_renumbered_folder_answers_stale_id(pool: SqlitePool) {
    let world = World::new(pool).await;
    let id = world.transport.deliver("bob@other.test", "Invoice", "text");
    world.transport.renumber("INBOX");

    let result = world
        .call(
            MailTool::GetMessage,
            serde_json::json!({"mailbox": "own", "message_id": id.to_string()}),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.code.as_deref(), Some("stale_id"));
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn an_id_that_is_not_a_folder_and_a_uid_answers_stale_id(pool: SqlitePool) {
    let world = World::new(pool).await;

    let result = world
        .call(
            MailTool::GetMessage,
            serde_json::json!({"mailbox": "own", "message_id": "not-an-id"}),
        )
        .await;

    assert_eq!(result.code.as_deref(), Some("stale_id"));
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_mail_host_that_refuses_the_login_answers_mailbox_unavailable(pool: SqlitePool) {
    let world = World::new(pool).await;
    world
        .transport
        .refuse_login_with(Some(pagis_mail::TransportErrorCode::Unauthorized));

    let result = world
        .call(
            MailTool::Search,
            serde_json::json!({"mailbox": "own", "query": ""}),
        )
        .await;

    assert_eq!(result.code.as_deref(), Some("mailbox_unavailable"));
}

#[test]
fn every_mail_tool_keeps_its_name() {
    assert_eq!(MailTool::Search.as_str(), MAIL_SEARCH);
    assert_eq!(MailTool::GetMessage.as_str(), MAIL_GET_MESSAGE);
    assert_eq!(MailTool::GetThread.as_str(), MAIL_GET_THREAD);
    assert_eq!(MailTool::Send.as_str(), MAIL_SEND);
    assert_eq!(MailTool::ModifyMessage.as_str(), MAIL_MODIFY_MESSAGE);
}
