//! The `mail__*` executor (ADR-0019) against MemoryMailbox:
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
use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost, FakeStandingRule, MemoryMailbox};
use pagis_mail::{
    Endpoint, MAX_SUMMARIES, MAX_TEXT_BYTES, MAX_THREAD_MESSAGES, MIGADU_PROVIDER, MailBlockDeps,
    MailBlocks, MailToolDeps, MailToolRuntime, MailboxCapabilities, MailboxDesk, MailboxDeskDeps,
    MailboxHost, MailboxProvider, NewMailbox, ProofTiming, TRUNCATION_MARKER,
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
    desk: Arc<MailboxDesk>,
    transport: Arc<FakeMailTransport>,
    connection_id: ConnectionId,
    runtime: MailToolRuntime,
    mailboxes: Arc<SqliteAgentMailboxStore>,
    messages: Arc<SqliteMessageStore>,
    sent: Arc<SqliteSentMailStore>,
    memory: Arc<MemoryMailbox>,
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    run_id: RunId,
    thread_id: MessageId,
    mailbox: AgentMailbox,
}

impl World {
    async fn new(pool: SqlitePool) -> Self {
        Self::with_endpoints(pool, true).await
    }

    async fn with_endpoints(pool: SqlitePool, endpoints: bool) -> Self {
        let workspaces = SqliteWorkspaceStore::new(pool.clone());
        let person =
            pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", pagis_core::now_ms())
                .await
                .unwrap();
        let workspace = Workspace {
            delegate_everything: false,
            kind: pagis_core::WorkspaceKind::Private,
            legal_hold: None,
            monthly_spend_cap_usd: None,
            user_id: Some(person.id.clone()),
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
        let mut connection = mail_connection(
            &workspace.id,
            MailboxCapabilities::of(host.capabilities(), true),
        );
        if !endpoints {
            connection.provider = "embedder-mail".to_string();
            for key in ["imap_host", "imap_port", "smtp_host", "smtp_port"] {
                connection.config.as_object_mut().unwrap().remove(key);
            }
        }
        connections.create(&connection).await.unwrap();
        secrets
            .set(&host_api_key_secret_name(ALIAS), "migadu-key")
            .unwrap();

        let mailboxes = Arc::new(SqliteAgentMailboxStore::new(pool.clone()));
        let workspace_mailboxes = Arc::new(pagis_storage_sqlite::SqliteWorkspaceMailboxStore::new(
            pool.clone(),
        ));
        let transport = Arc::new(FakeMailTransport::default());
        if !endpoints {
            transport.refuse_login_with(Some(pagis_mail::TransportErrorCode::Unreachable));
        }
        let desk = Arc::new(MailboxDesk::new(MailboxDeskDeps {
            workspace_mail: Arc::new(pagis_mail::ImapWorkspaceMailHost::new(
                Arc::clone(&host) as _,
                Arc::clone(&transport) as _,
                Arc::clone(&secrets) as _,
            )),
            grants: Arc::new(pagis_storage_sqlite::SqliteGrantStore::new(pool.clone())),
            intake: Arc::new(pagis_mail::MailIntake {
                workspace_mailboxes: workspace_mailboxes.clone(),
                junk: Arc::new(pagis_storage_sqlite::SqliteJunkStore::new(pool.clone())),
                opinions: std::sync::Arc::new(pagis_mail::fake::FakeMailOpinions::default()),
                calendar: Arc::new(pagis_storage_sqlite::SqliteCalendarStore::new(pool.clone())),
                workspaces: Arc::new(pagis_storage_sqlite::SqliteWorkspaceStore::new(
                    pool.clone(),
                )),
                agents: agents.clone(),
                source: Arc::new(pagis_mail::HostSpamVerdicts),
                records: Arc::new(pagis_storage_sqlite::SqliteMailAssessmentStore::new(
                    pool.clone(),
                )),
                trust: Arc::new(pagis_mail::ListedSenders::new(
                    Arc::new(pagis_storage_sqlite::SqliteTrustListStore::new(
                        pool.clone(),
                    )),
                    connections.clone(),
                )),
            }),
            workspaces: Arc::new(pagis_storage_sqlite::SqliteWorkspaceStore::new(
                pool.clone(),
            )),
            workspace_mailboxes,
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
            for_person_only: false,
            author_workspace_id: None,
            id: MessageId::generate(),
            workspace_id: workspace.id.clone(),
            channel_id: channel.id.clone(),
            parent_message_id: None,
            author: pagis_core::Author::Person(workspace.user_id.clone().unwrap()),
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
        let memory = Arc::new(MemoryMailbox::default());
        let runtime = MailToolRuntime::new(MailToolDeps {
            calendar: Arc::new(pagis_storage_sqlite::SqliteCalendarStore::new(pool.clone())),
            users: Arc::new(pagis_storage_sqlite::SqliteUserStore::new(pool.clone())),
            grants: Arc::new(pagis_storage_sqlite::SqliteGrantStore::new(pool.clone())),
            workspaces: Arc::new(pagis_storage_sqlite::SqliteWorkspaceStore::new(
                pool.clone(),
            )),
            workspace: desk.clone(),
            mailboxes: Arc::clone(&memory) as _,
            records: Arc::clone(&mailboxes) as _,
            runs: Arc::clone(&runs) as _,
            sent: Arc::clone(&sent) as _,
            blocks,
        });
        Self {
            desk,
            transport,
            connection_id: connection.id,
            runtime,
            mailboxes,
            messages,
            sent,
            memory,
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
        imap: Some(Endpoint::new("imap.example.net", 993)),
        smtp: Some(Endpoint::new("smtp.example.net", 465)),
        capabilities,
    };
    Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace_id.clone(),
        provider: MIGADU_PROVIDER.to_string(),
        alias: ALIAS.to_string(),
        display_name: "Example mail".to_string(),
        status: Connection::CONNECTED.to_string(),
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
        .memory
        .deliver("bob@other.test", "Invoice 42", "the invoice is late");
    world
        .memory
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
            .memory
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
        .memory
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
        .memory
        .deliver("bob@other.test", "Invoice 42", "first");
    let summaries = world.memory.summaries(false);
    let thread_id = summaries[0].thread_id.clone();
    for index in 0..MAX_THREAD_MESSAGES {
        world.memory.deliver_reply(
            &thread_id,
            "bob@other.test",
            "Re: Invoice 42",
            format!("reply {index}"),
        );
    }
    assert!(!root.to_string().contains(':'));

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
    let page = answer["page"].as_str().expect("a page token");
    assert!(page.parse::<u32>().is_err());

    let older = world
        .answer(
            MailTool::GetThread,
            serde_json::json!({"mailbox": "own", "thread_id": thread_id, "page": page}),
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

    let sent = world.memory.sent();
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
        world.memory.sent()[0].in_reply_to.as_deref(),
        Some("<first@other.test>")
    );
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_modify_marks_read_flags_and_archives_one_message(pool: SqlitePool) {
    let world = World::new(pool).await;
    let id = world.memory.deliver("bob@other.test", "Invoice", "text");

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
    assert_eq!(answer["message_id"], id.to_string());
    assert!(world.memory.summaries(false).is_empty());
    let archived = world.memory.summaries(true);
    assert_eq!(archived.len(), 1);
    let flags = world
        .memory
        .flags_of(&archived[0].id)
        .expect("the flags follow the message");
    assert!(flags.seen && flags.flagged);
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_mailbox_decides_when_an_id_is_stale(pool: SqlitePool) {
    let world = World::new(pool).await;
    let id = world.memory.deliver("bob@other.test", "Invoice", "text");
    world.memory.remove(&id);

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
async fn an_unknown_opaque_id_answers_stale_id(pool: SqlitePool) {
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
async fn a_mailbox_that_refuses_to_open_answers_mailbox_unavailable(pool: SqlitePool) {
    let world = World::new(pool).await;
    world
        .memory
        .fail_with(Some(pagis_mail::TransportErrorCode::Unauthorized));

    let result = world
        .call(
            MailTool::Search,
            serde_json::json!({"mailbox": "own", "query": ""}),
        )
        .await;

    assert_eq!(result.code.as_deref(), Some("mailbox_unavailable"));
    assert!(
        result
            .content
            .contains("Tell your Person to reset its password.")
    );
}

#[test]
fn every_mail_tool_keeps_its_name() {
    assert_eq!(MailTool::Search.as_str(), MAIL_SEARCH);
    assert_eq!(MailTool::GetMessage.as_str(), MAIL_GET_MESSAGE);
    assert_eq!(MailTool::GetThread.as_str(), MAIL_GET_THREAD);
    assert_eq!(MailTool::Send.as_str(), MAIL_SEND);
    assert_eq!(MailTool::ModifyMessage.as_str(), MAIL_MODIFY_MESSAGE);
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_message_reads_an_opaque_id_and_keeps_headers_and_attachments(pool: SqlitePool) {
    let world = World::new(pool).await;
    let id = world.memory.deliver("bob@other.test", "Invoice", "paid");
    world.memory.attach(
        &id,
        pagis_mail::Attachment {
            name: "invoice.pdf".into(),
            bytes: 42,
        },
    );
    let answer = world
        .answer(
            MailTool::GetMessage,
            serde_json::json!({"message_id": id.to_string()}),
        )
        .await;
    assert_eq!(answer["message"]["id"], id.to_string());
    assert_eq!(answer["message"]["text"], "paid");
    assert_eq!(answer["message"]["truncated"], false);
    assert_eq!(answer["message"]["headers"][0]["name"], "Message-ID");
    assert_eq!(answer["message"]["attachments"][0]["name"], "invoice.pdf");
    assert_eq!(answer["message"]["attachments"][0]["bytes"], 42);
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_thread_spends_its_text_cap_on_the_newest_messages(pool: SqlitePool) {
    let world = World::new(pool).await;
    world
        .memory
        .deliver("bob@other.test", "Long", "é".repeat(MAX_TEXT_BYTES));
    let thread = world.memory.summaries(false)[0].thread_id.clone();
    world.memory.deliver_reply(
        &thread,
        "bob@other.test",
        "Re: Long",
        "z".repeat(MAX_TEXT_BYTES - 1),
    );
    let answer = world
        .answer(
            MailTool::GetThread,
            serde_json::json!({"thread_id": thread}),
        )
        .await;
    assert_eq!(answer["messages"][0]["text"], TRUNCATION_MARKER);
    assert_eq!(answer["messages"][0]["truncated"], true);
    assert_eq!(
        answer["messages"][1]["text"].as_str().unwrap().len(),
        MAX_TEXT_BYTES - 1
    );
    assert_eq!(answer["messages"][1]["truncated"], false);
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_search_clamps_an_explicit_max_and_accepts_an_opaque_page(pool: SqlitePool) {
    let world = World::new(pool).await;
    for index in 0..30 {
        world
            .memory
            .deliver("bob@other.test", format!("Note {index}"), "text");
    }
    for max in [26_u64, u64::MAX, 4_294_967_297] {
        let answer = world
            .answer(MailTool::Search, serde_json::json!({"max": max}))
            .await;
        assert_eq!(
            answer["messages"].as_array().unwrap().len(),
            MAX_SUMMARIES as usize
        );
        assert!(answer["page"].as_str().unwrap().parse::<u32>().is_err());
    }
    let answer = world
        .answer(MailTool::Search, serde_json::json!({"max": 0}))
        .await;
    assert_eq!(answer["messages"].as_array().unwrap().len(), 1);
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_modify_can_clear_flags_without_archiving(pool: SqlitePool) {
    let world = World::new(pool).await;
    let id = world.memory.deliver("bob@other.test", "Invoice", "text");
    for value in [true, false] {
        world.answer(MailTool::ModifyMessage, serde_json::json!({"message_id": id.to_string(), "mark_read": value, "flag": value})).await;
        let flags = world.memory.flags_of(&id).unwrap();
        assert_eq!(flags.seen, value);
        assert_eq!(flags.flagged, value);
        assert_eq!(world.memory.summaries(false).len(), 1);
    }
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_reserved_send_keeps_its_identity_and_classifies_a_failed_send(pool: SqlitePool) {
    let world = World::new(pool).await;
    let message = pagis_mail::OutgoingMessage {
        message_id: Some("reserved@pagis.test".into()),
        ..Default::default()
    };
    let sent = world
        .runtime
        .send_reserved(&world.workspace_id, &world.agent_id, &message)
        .await
        .unwrap();
    assert_eq!(sent.message_id, "<reserved@pagis.test>");
    world
        .memory
        .fail_with(Some(pagis_mail::TransportErrorCode::Unreachable));
    assert!(matches!(
        world
            .runtime
            .send_reserved(&world.workspace_id, &world.agent_id, &message)
            .await,
        Err(pagis_mail::ReservedSendError::NotSent(_))
    ));
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn message_and_thread_ids_pass_to_the_mailbox_without_changes(pool: SqlitePool) {
    let world = World::new(pool).await;
    let id = world.memory.deliver("bob@other.test", "Invoice", "text");
    let mut memory = world.memory.as_ref().clone();
    let mut message = pagis_mail::Mailbox::message(&mut memory, &id)
        .await
        .unwrap();
    message.summary.id = pagis_mail::MessageId::new("  opaque://a:b:c  ");
    message.summary.thread_id = "  opaque thread  ".into();
    world.memory.insert(message.clone());
    let answer = world
        .answer(
            MailTool::GetMessage,
            serde_json::json!({"message_id": message.summary.id.as_str()}),
        )
        .await;
    assert_eq!(answer["message"]["id"], message.summary.id.as_str());
    let answer = world
        .answer(
            MailTool::GetThread,
            serde_json::json!({"thread_id": message.summary.thread_id}),
        )
        .await;
    assert_eq!(answer["messages"].as_array().unwrap().len(), 1);
    world
        .answer(
            MailTool::ModifyMessage,
            serde_json::json!({"message_id": message.summary.id.as_str(), "flag": true}),
        )
        .await;
    assert!(world.memory.flags_of(&message.summary.id).unwrap().flagged);
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_thread_orders_messages_by_date_when_they_arrive_out_of_order(pool: SqlitePool) {
    let world = World::new(pool).await;
    let id = world.memory.deliver("bob@other.test", "Invoice", "newer");
    let mut memory = world.memory.as_ref().clone();
    let mut older = pagis_mail::Mailbox::message(&mut memory, &id)
        .await
        .unwrap();
    older.summary.id = pagis_mail::MessageId::new("older");
    older.summary.date -= 1;
    older.text = "older".into();
    let thread_id = older.summary.thread_id.clone();
    world.memory.insert(older);
    let answer = world
        .answer(
            MailTool::GetThread,
            serde_json::json!({"thread_id": thread_id}),
        )
        .await;
    assert_eq!(answer["messages"][0]["text"], "older");
    assert_eq!(answer["messages"][1]["text"], "newer");
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_provider_without_endpoints_reads_and_sends_through_mailboxes(pool: SqlitePool) {
    let world = World::with_endpoints(pool, false).await;
    let id = world
        .memory
        .deliver("bob@other.test", "Invoice", "Please check this invoice.");
    let read = world
        .answer(
            MailTool::GetMessage,
            serde_json::json!({"mailbox": "own", "message_id": id.to_string()}),
        )
        .await;
    assert_eq!(read["message"]["text"], "Please check this invoice.");
    let sent = world.answer(MailTool::Send, serde_json::json!({"mailbox": "own", "to": ["bob@other.test"], "subject": "Invoice", "body": "Checked."})).await;
    assert_eq!(sent["sends_today"], 1);
    assert_eq!(world.memory.sent().len(), 1);
    assert_eq!(world.memory.sent()[0].body, "Checked.");
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn workspace_mailbox_reply_resolves_headers_and_cc_before_approval(pool: SqlitePool) {
    use pagis_core::{Grant, GrantId, GrantStore};
    let world = World::new(pool.clone()).await;
    world
        .desk
        .provision_workspace(
            &world.workspace_id,
            world
                .desk
                .prepare_workspace(NewMailbox {
                    connection_id: world.connection_id.clone(),
                    local_part: "person".into(),
                    outgoing_cap: Some(1),
                    password: None,
                })
                .await
                .unwrap(),
            pagis_core::Author::System,
        )
        .await
        .unwrap();
    let grants = pagis_storage_sqlite::SqliteGrantStore::new(pool);
    let grant = Grant {
        created_by: pagis_core::Author::System,
        id: GrantId::generate(),
        workspace_id: world.workspace_id.clone(),
        agent_id: world.agent_id.clone(),
        resource_kind: Grant::WORKSPACE_MAILBOX_KIND.into(),
        resource_id: Some(world.workspace_id.to_string()),
        scope: Grant::allow_scope(&[]),
        revision: 1,
        created_at: now_ms(),
        revoked_at: None,
    };
    let id = world.transport.deliver_raw(b"Message-ID: <question@other.test>\r\nReferences: <root@other.test>\r\nFrom: sender@other.test\r\nTo: person@example.com\r\nSubject: Question\r\n\r\nHello");
    let route = ToolRoute::Mailbox {
        tool: MailTool::Send,
    };
    let args = serde_json::json!({"mailbox":"own","to":["wrong@elsewhere.test"],"bcc":["hidden@elsewhere.test"],"subject":"Ignored","body":"Answer","reply_to_workspace_message_id":id.to_string()});
    assert!(
        world
            .runtime
            .prepare(&world.workspace_id, &world.agent_id, &route, args.clone())
            .await
            .is_err()
    );
    grants.create(&grant).await.unwrap();
    let prepared = world
        .runtime
        .prepare(&world.workspace_id, &world.agent_id, &route, args)
        .await
        .unwrap();
    assert_eq!(prepared["to"], serde_json::json!(["sender@other.test"]));
    assert_eq!(prepared["cc"], serde_json::json!(["person@example.com"]));
    assert_eq!(prepared["bcc"], serde_json::json!([]));
    assert_eq!(prepared["subject"], "Re: Question");
    assert_eq!(prepared["in_reply_to"], "<question@other.test>");
    assert_eq!(
        prepared["references"],
        serde_json::json!(["<root@other.test>", "<question@other.test>"])
    );
    assert_eq!(prepared["body"], "Answer");
    let answer = world.answer(MailTool::Send, prepared.clone()).await;
    assert_eq!(answer["from"], world.mailbox.address);
    let sent = world.memory.sent();
    assert_eq!(sent[0].cc, ["person@example.com"]);
    assert_eq!(
        sent[0].references,
        ["<root@other.test>", "<question@other.test>"]
    );
    assert_eq!(
        sent[0].in_reply_to.as_deref(),
        Some("<question@other.test>")
    );
    grants
        .revoke(&world.workspace_id, &grant.id, now_ms())
        .await
        .unwrap();
    let result = world.call(MailTool::Send, prepared).await;
    assert!(result.is_error);
    assert_eq!(world.memory.sent().len(), 1);
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn workspace_mailbox_tools_read_and_send_with_the_outgoing_cap(pool: SqlitePool) {
    use pagis_broker::WorkspaceMailTool;
    let world = World::new(pool.clone()).await;
    SqliteWorkspaceStore::new(pool)
        .set_chief_of_staff(&world.workspace_id, Some(&world.agent_id))
        .await
        .unwrap();
    world
        .desk
        .provision_workspace(
            &world.workspace_id,
            world
                .desk
                .prepare_workspace(NewMailbox {
                    connection_id: world.connection_id.clone(),
                    local_part: "person".into(),
                    outgoing_cap: Some(1),
                    password: None,
                })
                .await
                .unwrap(),
            pagis_core::Author::System,
        )
        .await
        .unwrap();
    let id = world.transport.deliver_raw(b"Message-ID: <question@example.com>\r\nFrom: sender@example.com\r\nTo: person@example.com\r\nSubject: Question\r\n\r\nHello");
    for (tool, args) in [
        (
            WorkspaceMailTool::Escalate,
            serde_json::json!({"message_id":id.to_string()}),
        ),
        (
            WorkspaceMailTool::Search,
            serde_json::json!({"query":"Question"}),
        ),
        (
            WorkspaceMailTool::GetMessage,
            serde_json::json!({"message_id":id.to_string()}),
        ),
        (
            WorkspaceMailTool::GetThread,
            serde_json::json!({"thread_id":"<question@example.com>"}),
        ),
        (
            WorkspaceMailTool::Reply,
            serde_json::json!({"message_id":id.to_string(),"body":"Answer"}),
        ),
        (
            WorkspaceMailTool::Send,
            serde_json::json!({"to":["sender@example.com"],"subject":"Again","body":"Answer"}),
        ),
    ] {
        let route = ToolRoute::WorkspaceMailbox { tool };
        let arguments = world
            .runtime
            .prepare(&world.workspace_id, &world.agent_id, &route, args)
            .await
            .unwrap();
        let result = world
            .runtime
            .execute(AuthorizedCall {
                workspace_id: world.workspace_id.clone(),
                agent_id: world.agent_id.clone(),
                run_id: world.run_id.clone(),
                tool_name: "workspace_mail".into(),
                source_version: "test".into(),
                route,
                arguments,
                selected_connection: None,
                grant_id: None,
                grant_revision: None,
                call_timeout: None,
                tool_call_id: None,
                host: None,
                approved_by_rule: false,
            })
            .await;
        if tool == WorkspaceMailTool::Escalate {
            assert!(
                world
                    .desk
                    .workspace_mailbox_records()
                    .is_escalated(&world.workspace_id, "<question@example.com>")
                    .await
                    .unwrap()
            );
        }
        if tool == WorkspaceMailTool::Send {
            assert!(result.content.contains("outgoing_cap_reached"));
        } else {
            assert!(!result.is_error, "{}", result.content);
        }
    }
    let sent = world.transport.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, "Re: Question");
    assert_eq!(
        sent[0].in_reply_to.as_deref(),
        Some("<question@example.com>")
    );
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn calendar_tool_creates_changes_and_deletes_with_agent_author(pool: SqlitePool) {
    use pagis_core::{CalendarStore, Grant, GrantId, GrantStore};
    let world = World::new(pool.clone()).await;
    let grants = pagis_storage_sqlite::SqliteGrantStore::new(pool.clone());
    let grant = Grant {
        id: GrantId::generate(),
        workspace_id: world.workspace_id.clone(),
        agent_id: world.agent_id.clone(),
        resource_kind: Grant::CALENDAR_KIND.into(),
        resource_id: Some(world.workspace_id.to_string()),
        scope: serde_json::json!({}),
        revision: 1,
        created_by: pagis_core::Author::System,
        created_at: now_ms(),
        revoked_at: None,
    };
    grants.create(&grant).await.unwrap();
    let mut data = serde_json::json!({
        "title": "Review",
        "description": "Plan",
        "location": "Office",
        "start": "2026-03-08T09:00:00",
        "end": "2026-03-08T10:00:00",
        "timezone": "UTC",
        "all_day": false,
        "rrule": null,
        "exdates": [],
        "attendees": [],
        "reminders": [],
        "meeting_url": null
    });
    let call = |tool, arguments| AuthorizedCall {
        workspace_id: world.workspace_id.clone(),
        agent_id: world.agent_id.clone(),
        run_id: world.run_id.clone(),
        tool_name: "calendar".into(),
        source_version: "test".into(),
        route: ToolRoute::Calendar { tool },
        arguments,
        selected_connection: None,
        grant_id: Some(grant.id.clone()),
        grant_revision: Some(1),
        call_timeout: None,
        tool_call_id: None,
        host: None,
        approved_by_rule: false,
    };
    let mut id = String::new();
    for tool in [
        pagis_broker::CalendarTool::Create,
        pagis_broker::CalendarTool::Update,
        pagis_broker::CalendarTool::Delete,
    ] {
        let route = ToolRoute::Calendar { tool };
        let arguments = world
            .runtime
            .prepare(
                &world.workspace_id,
                &world.agent_id,
                &route,
                serde_json::json!({
                    "event": data,
                    "event_id": id,
                    "to": ["spoof@outside.test"]
                }),
            )
            .await
            .unwrap();
        assert_eq!(arguments["to"], serde_json::json!([]));
        let result = world.runtime.execute(call(tool, arguments)).await;
        assert!(!result.is_error, "{}", result.content);
        let event: pagis_core::CalendarEvent = serde_json::from_str(&result.content).unwrap();
        assert_eq!(
            event.created_by,
            pagis_core::Author::Agent(world.agent_id.clone())
        );
        id = event.id.to_string();
        let stored = pagis_storage_sqlite::SqliteCalendarStore::new(pool.clone())
            .get(&world.workspace_id, &event.id)
            .await
            .unwrap();
        if tool == pagis_broker::CalendarTool::Delete {
            assert!(stored.is_none());
        } else {
            assert_eq!(stored, Some(event));
        }
        data["title"] = serde_json::json!("Changed");
    }
    grants
        .revoke(&world.workspace_id, &grant.id, now_ms())
        .await
        .unwrap();
    assert!(
        world
            .runtime
            .prepare(
                &world.workspace_id,
                &world.agent_id,
                &ToolRoute::Calendar {
                    tool: pagis_broker::CalendarTool::Create
                },
                serde_json::json!({"event": data})
            )
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn calendar_tool_outside_attendees_need_mailbox_grant_and_send_imip(pool: SqlitePool) {
    use pagis_core::{Grant, GrantId, GrantStore};
    let world = World::new(pool.clone()).await;
    let grants = pagis_storage_sqlite::SqliteGrantStore::new(pool.clone());
    let mut grant = Grant {
        id: GrantId::generate(),
        workspace_id: world.workspace_id.clone(),
        agent_id: world.agent_id.clone(),
        resource_kind: Grant::CALENDAR_KIND.into(),
        resource_id: Some(world.workspace_id.to_string()),
        scope: serde_json::json!({}),
        revision: 1,
        created_by: pagis_core::Author::System,
        created_at: now_ms(),
        revoked_at: None,
    };
    grants.create(&grant).await.unwrap();
    let route = ToolRoute::Calendar {
        tool: pagis_broker::CalendarTool::Create,
    };
    let args = serde_json::json!({
        "event": {
            "title": "Review",
            "description": "Plan",
            "location": "Office",
            "start": "2026-03-08T09:00:00",
            "end": "2026-03-08T10:00:00",
            "timezone": "UTC",
            "all_day": false,
            "rrule": null,
            "exdates": [],
            "attendees": [{
                    "email": "guest@other.test",
                    "name": "Guest",
                    "status": "needs-action"
                }],
            "reminders": [],
            "meeting_url": null
        }
    });
    assert!(
        world
            .runtime
            .prepare(&world.workspace_id, &world.agent_id, &route, args.clone())
            .await
            .unwrap_err()
            .content
            .contains("Create a Workspace Mailbox")
    );
    world
        .desk
        .provision_workspace(
            &world.workspace_id,
            world
                .desk
                .prepare_workspace(NewMailbox {
                    connection_id: world.connection_id.clone(),
                    local_part: "team".into(),
                    outgoing_cap: Some(20),
                    password: None,
                })
                .await
                .unwrap(),
            pagis_core::Author::System,
        )
        .await
        .unwrap();
    assert!(
        world
            .runtime
            .prepare(&world.workspace_id, &world.agent_id, &route, args.clone())
            .await
            .is_err()
    );
    grant.id = GrantId::generate();
    grant.resource_kind = Grant::WORKSPACE_MAILBOX_KIND.into();
    grants.create(&grant).await.unwrap();
    let arguments = world
        .runtime
        .prepare(&world.workspace_id, &world.agent_id, &route, args)
        .await
        .unwrap();
    assert_eq!(arguments["to"], serde_json::json!(["guest@other.test"]));
    let result = world
        .runtime
        .execute(AuthorizedCall {
            workspace_id: world.workspace_id.clone(),
            agent_id: world.agent_id.clone(),
            run_id: world.run_id.clone(),
            tool_name: "calendar_create_event".into(),
            source_version: "test".into(),
            route,
            arguments,
            selected_connection: None,
            grant_id: None,
            grant_revision: None,
            call_timeout: None,
            tool_call_id: None,
            host: None,
            approved_by_rule: false,
        })
        .await;
    assert!(!result.is_error, "{}", result.content);
    use pagis_core::CalendarStore;
    let created: pagis_core::CalendarEvent = serde_json::from_str(&result.content).unwrap();
    let mut incoming = pagis_core::CalendarEvent::new(
        world.workspace_id.clone(),
        pagis_core::Author::System,
        created.data,
    );
    incoming.data.invitation.as_mut().unwrap().organizer = "guest@other.test".into();
    incoming.data.attendees = vec![pagis_core::CalendarAttendee {
        email: "team@example.com".into(),
        name: "Team".into(),
        status: pagis_core::ParticipationStatus::NeedsAction,
    }];
    let calendar = pagis_storage_sqlite::SqliteCalendarStore::new(pool);
    calendar.create(&incoming).await.unwrap();
    for status in ["accepted", "tentative", "declined"] {
        let route = ToolRoute::Calendar {
            tool: pagis_broker::CalendarTool::Respond,
        };
        let arguments = world
            .runtime
            .prepare(
                &world.workspace_id,
                &world.agent_id,
                &route,
                serde_json::json!({
                    "event_id": incoming.id,
                    "status": status
                }),
            )
            .await
            .unwrap();
        let reply = world
            .runtime
            .execute(AuthorizedCall {
                workspace_id: world.workspace_id.clone(),
                agent_id: world.agent_id.clone(),
                run_id: world.run_id.clone(),
                tool_name: "calendar_respond".into(),
                source_version: "test".into(),
                route,
                arguments,
                selected_connection: None,
                grant_id: None,
                grant_revision: None,
                call_timeout: None,
                tool_call_id: None,
                host: None,
                approved_by_rule: false,
            })
            .await;
        assert!(!reply.is_error, "{}", reply.content);
        let stored = calendar
            .get(&world.workspace_id, &incoming.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.created_by,
            pagis_core::Author::Agent(world.agent_id.clone())
        );
        assert_eq!(
            serde_json::to_value(stored.data.attendees[0].status.clone()).unwrap(),
            status
        );
        assert!(
            String::from_utf8_lossy(&world.transport.sent().last().unwrap().attachments[0].data)
                .contains("METHOD:REPLY")
        );
    }
    let received = world.transport.received("guest@other.test");
    assert_eq!(received[0].summary.from, "team@example.com");
    let sent = world.transport.sent();
    assert_eq!(
        sent[0].attachments[0].content_type,
        "text/calendar; method=REQUEST; charset=utf-8"
    );
    assert!(String::from_utf8_lossy(&sent[0].attachments[0].data).contains("Review"));
}
