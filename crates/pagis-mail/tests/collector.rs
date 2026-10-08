//! The inbound mail collector and the `mail.message_received` event
//! (ADR-0019): a delivered message becomes an Incoming Event, the
//! sender's tier holds only on the Mailbox Provider's aligned DMARC
//! pass, a resync hands nothing over twice, a reply lands in the Thread
//! that sent the message it answers, the typed filter narrows, and the
//! task stops when the mailbox leaves `active`.
//!
//! Everything runs against the fake mail transport and real records,
//! so no process starts and no network is reached.

use std::sync::Arc;
use std::time::Duration;

use pagis_audit::AuditEventBus;
use pagis_core::OrgStore;
use pagis_core::{
    Agent, AgentId, AgentMailbox, AgentMailboxId, AgentMailboxState, AgentMailboxStore,
    AgentStatus, AgentStore, AuthorKind, Channel, ChannelId, ChannelKind, ChannelStore, Connection,
    ConnectionId, ConnectionStore, EventMatcher, MemorySecretStore, Message, MessageId,
    MessageStatus, MessageStore, Run, RunId, RunState, RunStore, SecretStore, SentMail,
    SentMailStore, TriggerKind, TrustEntry, TrustEntryId, TrustListStore, TrustTier, User,
    UserRole, UserStore, Workspace, WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_mail::fake::{
    FakeMailIngest, FakeMailTransport, FakeMailboxHost, FakeStandingRule, raw_message,
};
use pagis_mail::{
    Endpoint, ListedSenders, MAIL_MESSAGE_RECEIVED, MailCollector, MailCollectorDeps,
    MailMessageMatcher, MailboxCapabilities, MailboxDesk, MailboxDeskDeps, MailboxHost,
    MailboxProvider, NewMailbox, OWN_MAILBOX, ProofTiming, TransportCapabilities,
    TransportErrorCode, host_api_key_secret_name,
};
use pagis_storage_sqlite::{
    SqliteAgentMailboxStore, SqliteAgentStore, SqliteChannelStore, SqliteConnectionStore,
    SqliteEventLog, SqliteMessageStore, SqliteOrgStore, SqliteRunStore, SqliteSentMailStore,
    SqliteTrustListStore, SqliteUserStore, SqliteWorkspaceStore,
};
use sqlx::SqlitePool;

const ALIAS: &str = "mail";
/// A test never waits for a real IDLE turn or a real poll.
const TURN: Duration = Duration::from_millis(50);
/// How long a test waits for the collector to hand a pass over.
const PATIENCE: Duration = Duration::from_secs(5);

struct World {
    pool: SqlitePool,
    collector: Arc<MailCollector>,
    desk: Arc<MailboxDesk>,
    mailboxes: Arc<SqliteAgentMailboxStore>,
    sent: Arc<SqliteSentMailStore>,
    transport: Arc<FakeMailTransport>,
    ingest: Arc<FakeMailIngest>,
    trust_list: Arc<SqliteTrustListStore>,
    workspace_id: WorkspaceId,
    connection_id: ConnectionId,
    agent_id: AgentId,
    other_agent_id: AgentId,
    /// A Thread the Agent worked in, and the Run that worked in it, so
    /// a send has somewhere to have come from.
    channel_id: ChannelId,
    thread_id: MessageId,
    run_id: RunId,
    /// The Mailbox Provider is a manual host, where the user types the
    /// mailbox password.
    manual: bool,
}

impl World {
    async fn new(pool: SqlitePool) -> Self {
        Self::with_transport(pool, FakeMailTransport::default()).await
    }

    async fn with_transport(pool: SqlitePool, transport: FakeMailTransport) -> Self {
        Self::build(pool, transport, pagis_mail::MIGADU_PROVIDER).await
    }

    /// A world whose Mailbox Provider is a manual host.
    async fn on_manual_host(pool: SqlitePool) -> Self {
        Self::build(
            pool,
            FakeMailTransport::default(),
            pagis_mail::MANUAL_PROVIDER,
        )
        .await
    }

    async fn build(pool: SqlitePool, transport: FakeMailTransport, provider: &str) -> Self {
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
        let agent_id = seed_agent(&agents, &workspace.id, "Ada Lovelace").await;
        let other_agent_id = seed_agent(&agents, &workspace.id, "Grace Hopper").await;

        let connections = Arc::new(SqliteConnectionStore::new(pool.clone()));
        let trust_list = Arc::new(SqliteTrustListStore::new(pool.clone()));
        let secrets = Arc::new(MemorySecretStore::default());
        let host = Arc::new(FakeMailboxHost::default());
        let manual = provider == pagis_mail::MANUAL_PROVIDER;
        let capabilities = match manual {
            true => pagis_mail::ManualHost::new().capabilities(),
            false => FakeMailboxHost::default().capabilities(),
        };
        let connection = mail_connection(&workspace.id, provider, capabilities);
        connections.create(&connection).await.unwrap();
        secrets
            .set(&host_api_key_secret_name(ALIAS), "migadu-key")
            .unwrap();

        let mailboxes = Arc::new(SqliteAgentMailboxStore::new(pool.clone()));
        let sent = Arc::new(SqliteSentMailStore::new(pool.clone()));
        let transport = Arc::new(transport);
        let desk = Arc::new(MailboxDesk::new(MailboxDeskDeps {
            mailboxes: Arc::clone(&mailboxes) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: workspace.id.clone(),
            agents: agents as _,
            host: host as _,
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
        let channels = SqliteChannelStore::new(pool.clone());
        let channel = Channel {
            id: ChannelId::generate(),
            workspace_id: workspace.id.clone(),
            kind: ChannelKind::Dm,
            title: Some("Ada".to_string()),
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        channels.create(&channel).await.unwrap();
        let messages = SqliteMessageStore::new(pool.clone());
        let root = Message {
            id: MessageId::generate(),
            workspace_id: workspace.id.clone(),
            channel_id: channel.id.clone(),
            parent_message_id: None,
            author_kind: AuthorKind::User,
            author_agent_id: None,
            run_id: None,
            status: MessageStatus::Complete,
            blocks: Vec::new(),
            text_content: "ask the clinic".to_string(),
            pending_id: None,
            created_at: now_ms(),
            completed_at: Some(now_ms()),
        };
        messages.insert(&root).await.unwrap();
        let runs = SqliteRunStore::new(pool.clone());
        let run = Run {
            title: "A message with an attachment".into(),
            id: RunId::generate(),
            workspace_id: workspace.id.clone(),
            agent_id: agent_id.clone(),
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

        let ingest = Arc::new(FakeMailIngest::default());
        let collector = Arc::new(MailCollector::new(MailCollectorDeps {
            workspaces: Arc::new(SqliteWorkspaceStore::new(pool.clone())) as _,
            desk: Arc::clone(&desk),
            sent: Arc::clone(&sent) as _,
            trust: Arc::new(ListedSenders::new(
                Arc::clone(&trust_list) as _,
                Arc::clone(&connections) as _,
            )),
            ingest: Arc::clone(&ingest) as _,
            idle_turn: TURN,
            poll_interval: TURN,
        }));
        Self {
            pool: pool.clone(),
            collector,
            desk,
            mailboxes,
            sent,
            transport,
            ingest,
            trust_list,
            workspace_id: workspace.id,
            connection_id: connection.id,
            agent_id,
            other_agent_id,
            channel_id: channel.id,
            thread_id: root.id,
            run_id: run.id,
            manual,
        }
    }

    /// A second person in the same Org, with their own Workspace and one
    /// Agent, whose mailbox is on the Org's mail domain. The collector
    /// must watch the mailboxes of both people in one sweep.
    async fn second_person(&self) -> (WorkspaceId, AgentId) {
        let now = now_ms();
        let org = SqliteOrgStore::new(self.pool.clone())
            .list()
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("the org of the installation");
        let person = User {
            email: Some("grace@example.com".to_string()),
            name: Some("Grace".to_string()),
            ..User::new(org.id, UserRole::Member, now)
        };
        SqliteUserStore::new(self.pool.clone())
            .create(&person)
            .await
            .unwrap();
        let workspace = Workspace {
            id: WorkspaceId::generate(),
            user_id: person.id,
            name: "Grace".to_string(),
            timezone: "UTC".to_string(),
            onboarded_at: Some(now),
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
            home_exit_host_id: None,
            created_at: now,
        };
        SqliteWorkspaceStore::new(self.pool.clone())
            .create(&workspace)
            .await
            .unwrap();
        let agents = SqliteAgentStore::new(self.pool.clone());
        let agent_id = seed_agent(&agents, &workspace.id, "Grace Hopper").await;
        let mailbox = self
            .desk
            .provision(
                &workspace.id,
                &agent_id,
                NewMailbox {
                    connection_id: self.connection_id.clone(),
                    local_part: "grace".to_string(),
                    outgoing_cap: None,
                    password: None,
                },
            )
            .await
            .unwrap();
        // The login proof runs off the provision call, so the record
        // reaches `active` a moment later.
        tokio::time::timeout(PATIENCE, async {
            loop {
                let record = self
                    .mailboxes
                    .get(&workspace.id, &mailbox.id)
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
        .expect("the mailbox leaves provisioning");
        (workspace.id, agent_id)
    }

    /// One Workspace-wide Trust List row.
    async fn list(&self, value: &str, tier: TrustTier) {
        self.list_for(None, value, tier).await;
    }

    /// One Trust List row, Workspace-wide or on one Agent's list.
    async fn list_for(&self, agent_id: Option<&AgentId>, value: &str, tier: TrustTier) {
        let (subject, value) = pagis_core::parse_trust_subject(value).unwrap();
        self.trust_list
            .upsert(&TrustEntry {
                id: TrustEntryId::generate(),
                workspace_id: self.workspace_id.clone(),
                agent_id: agent_id.cloned(),
                subject,
                value,
                tier,
                label: String::new(),
                created_at: now_ms(),
            })
            .await
            .unwrap();
    }

    /// One mailbox on the Connection, proven and `active`.
    async fn provision(&self, agent_id: &AgentId, local_part: &str) -> AgentMailbox {
        let mailbox = self
            .desk
            .provision(
                &self.workspace_id,
                agent_id,
                NewMailbox {
                    connection_id: self.connection_id.clone(),
                    local_part: local_part.to_string(),
                    outgoing_cap: None,
                    // A manual host has no API, so the user types the
                    // password of the inbox they made.
                    password: self.manual.then(|| "typed-password".to_string()),
                },
            )
            .await
            .unwrap();
        tokio::time::timeout(PATIENCE, async {
            loop {
                let record = self.read(&mailbox).await;
                if record.state != AgentMailboxState::Provisioning {
                    return record;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the login proof settles")
    }

    async fn read(&self, mailbox: &AgentMailbox) -> AgentMailbox {
        self.mailboxes
            .get(&self.workspace_id, &mailbox.id)
            .await
            .unwrap()
            .expect("the record stays")
    }

    /// Wait until the mailbox cursor has passed a UID, or give up. The
    /// cursor is written after the pass, so a test that reads it right
    /// after the pass can outrun it.
    async fn wait_for_cursor(&self, mailbox: &AgentMailbox, last_uid: u32) {
        tokio::time::timeout(PATIENCE, async {
            loop {
                let cursor = self.read(mailbox).await.cursor;
                if cursor.is_some_and(|cursor| cursor.last_uid >= last_uid) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the cursor moves with the pass");
    }

    /// Wait until the mailbox reaches a state, or give up.
    async fn wait_for_state(&self, mailbox: &AgentMailbox, state: AgentMailboxState) {
        tokio::time::timeout(PATIENCE, async {
            loop {
                if self.read(mailbox).await.state == state {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the mailbox reaches {}", state.as_str()));
    }
}

async fn seed_agent(agents: &SqliteAgentStore, workspace_id: &WorkspaceId, name: &str) -> AgentId {
    let agent = Agent {
        id: AgentId::generate(),
        workspace_id: workspace_id.clone(),
        name: name.to_string(),
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
    agent.id
}

fn mail_connection(
    workspace_id: &WorkspaceId,
    provider: &str,
    host: pagis_mail::HostCapabilities,
) -> Connection {
    let settings = MailboxProvider {
        account: Some("owner@example.com".to_string()),
        domain: "example.com".to_string(),
        imap: Endpoint::new("imap.example.net", 993),
        smtp: Endpoint::new("smtp.example.net", 465),
        capabilities: MailboxCapabilities::of(host, TransportCapabilities { idle: true }),
    };
    Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace_id.clone(),
        provider: provider.to_string(),
        alias: ALIAS.to_string(),
        display_name: "Agent mail".to_string(),
        status: Connection::CONNECTED.to_string(),
        auth_mode: Connection::AUTH_MODE_BYO.to_string(),
        authorized_capabilities: Vec::new(),
        config: settings.config(),
        created_at: now_ms(),
    }
}

// ----- The event -----

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_delivered_message_becomes_one_incoming_event(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    let id = world
        .transport
        .deliver("Clinic <care@clinic.test>", "Your appointment", "Tuesday");

    let events = world.ingest.wait_for(1, PATIENCE).await;
    assert_eq!(events.len(), 1, "one message is one event");
    let metadata = &events[0].metadata;
    assert_eq!(metadata["mailbox"], OWN_MAILBOX);
    // The id the Agent hands to `mail__get_message`, never the body.
    assert_eq!(metadata["message_id"], id.to_string());
    assert_eq!(metadata["from"], "Clinic <care@clinic.test>");
    assert_eq!(metadata["from_domain"], "clinic.test");
    assert_eq!(metadata["subject"], "Your appointment");
    assert_eq!(metadata["has_attachments"], false);
    assert_eq!(metadata["trust_tier"], TrustTier::Unknown.as_str());
    assert!(
        metadata.get("snippet").is_none(),
        "no words reach the event"
    );
    assert!(metadata.get("body").is_none());

    let pass = &world.ingest.passes()[0];
    assert_eq!(pass.event_kind, MAIL_MESSAGE_RECEIVED);
    assert_eq!(
        pass.source,
        pagis_core::EventSource::connection(world.connection_id.clone())
    );
    // The mailbox is this Agent's own identity, so no other Agent's
    // rules see the pass.
    assert_eq!(pass.agent_id.as_ref(), Some(&world.agent_id));
    assert!(!pass.baseline);
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn mail_that_arrived_before_the_mailbox_wakes_nobody(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.transport.deliver("old@example.net", "Before", "");

    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;
    world.transport.deliver("new@example.net", "After", "");

    let events = world.ingest.wait_for(1, PATIENCE).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].metadata["subject"], "After");
    world.collector.stop();
}

// ----- Sender trust -----

/// The authserv-id Migadu writes on the mail its first exchanger
/// receives. A Migadu Connection trusts this result and no other.
const MIGADU: &str = "aspmx1.migadu.com";

/// One Authentication-Results value in the shape Migadu writes it: the
/// authserv-id, then DKIM, the DMARC verdict for one `From` domain, and
/// SPF with a comment that holds a colon and a semicolon.
fn result(authserv_id: &str, dmarc: &str, domain: &str) -> String {
    format!(
        "{authserv_id};\r\n\
         \tdkim=pass header.d={domain} header.s=key1 header.b=AbCd;\r\n\
         \tdmarc={dmarc} (policy=none) header.from={domain};\r\n\
         \tspf=pass ({authserv_id}: domain of bounce@{domain} designates 192.0.2.1 \
         as permitted sender; checked) smtp.mailfrom=bounce@{domain}"
    )
}

/// The metadata of the event the message with this subject made.
fn stamped(events: &[pagis_core::NormalizedEvent], subject: &str) -> serde_json::Value {
    events
        .iter()
        .find(|event| event.metadata["subject"] == subject)
        .unwrap_or_else(|| panic!("an event for {subject:?}"))
        .metadata
        .clone()
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_forged_connection_account_with_no_authentication_result_is_unknown(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    // The account of the user's mail Connection. Anyone can write it in
    // a `From` header.
    world
        .transport
        .deliver_raw(&raw_message("owner@example.com", "Book it", &[]));

    let events = world.ingest.wait_for(1, PATIENCE).await;
    let metadata = stamped(&events, "Book it");
    assert_eq!(metadata["trust_tier"], TrustTier::Unknown.as_str());
    assert_eq!(metadata["sender_verified"], false);
    assert_eq!(metadata["sender_verification"], "no_result");
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn forged_listed_senders_are_unknown_when_authentication_is_absent_or_failed(
    pool: SqlitePool,
) {
    let world = World::new(pool).await;
    world.list("boss@home.test", TrustTier::Owner).await;
    world.list("care@clinic.test", TrustTier::Trusted).await;
    world.list("bank.test", TrustTier::Trusted).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    let senders = [
        ("boss@home.test", "home.test"),
        ("care@clinic.test", "clinic.test"),
        ("billing@bank.test", "bank.test"),
    ];
    for (from, domain) in senders {
        world
            .transport
            .deliver_raw(&raw_message(from, &format!("{from} absent"), &[]));
        world.transport.deliver_raw(&raw_message(
            from,
            &format!("{from} failed"),
            &[&result(MIGADU, "fail", domain)],
        ));
    }

    let events = world.ingest.wait_for(6, PATIENCE).await;
    assert_eq!(events.len(), 6);
    for (from, _) in senders {
        let absent = stamped(&events, &format!("{from} absent"));
        assert_eq!(absent["trust_tier"], TrustTier::Unknown.as_str(), "{from}");
        assert_eq!(absent["sender_verified"], false);
        assert_eq!(absent["sender_verification"], "no_result");
        let failed = stamped(&events, &format!("{from} failed"));
        assert_eq!(failed["trust_tier"], TrustTier::Unknown.as_str(), "{from}");
        assert_eq!(failed["sender_verified"], false);
        assert_eq!(failed["sender_verification"], "no_dmarc_pass");
    }
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_dmarc_pass_under_a_foreign_authserv_id_is_unknown(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.list("boss@home.test", TrustTier::Owner).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    // Any host on the way, or the sender itself, can write this header.
    world.transport.deliver_raw(&raw_message(
        "boss@home.test",
        "Wire the money",
        &[&result("mx.home.test", "pass", "home.test")],
    ));

    let events = world.ingest.wait_for(1, PATIENCE).await;
    let metadata = stamped(&events, "Wire the money");
    assert_eq!(metadata["trust_tier"], TrustTier::Unknown.as_str());
    assert_eq!(metadata["sender_verification"], "no_result");
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_pass_below_a_failed_result_of_the_mailbox_provider_is_unknown(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.list("boss@home.test", TrustTier::Owner).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    world.transport.deliver_raw(&raw_message(
        "boss@home.test",
        "Wire the money",
        &[
            // Another authserv-id, which counts for nothing.
            &result("mx.home.test", "pass", "home.test"),
            // The topmost result of the Mailbox Provider.
            &result(MIGADU, "fail", "home.test"),
            // A copy below it, which the sender can write.
            &result(MIGADU, "pass", "home.test"),
        ],
    ));

    let events = world.ingest.wait_for(1, PATIENCE).await;
    let metadata = stamped(&events, "Wire the money");
    assert_eq!(metadata["trust_tier"], TrustTier::Unknown.as_str());
    assert_eq!(metadata["sender_verified"], false);
    assert_eq!(metadata["sender_verification"], "several_results");
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_forged_provider_result_above_the_providers_own_is_unknown(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.list("boss@home.test", TrustTier::Owner).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    // A host that writes its own result below the headers of the
    // message, and keeps the copy the sender wrote above them, leaves
    // two results with its authserv-id. The position does not tell
    // which copy the host wrote.
    for (subject, own) in [("Over a failure", "fail"), ("Over a pass", "pass")] {
        world.transport.deliver_raw(&raw_message(
            "boss@home.test",
            subject,
            &[
                // The copy the sender wrote.
                &result(MIGADU, "pass", "home.test"),
                // The copy the host wrote.
                &result(MIGADU, own, "home.test"),
            ],
        ));
    }

    let events = world.ingest.wait_for(2, PATIENCE).await;
    for subject in ["Over a failure", "Over a pass"] {
        let metadata = stamped(&events, subject);
        assert_eq!(
            metadata["trust_tier"],
            TrustTier::Unknown.as_str(),
            "{subject}"
        );
        assert_eq!(metadata["sender_verified"], false);
        assert_eq!(metadata["sender_verification"], "several_results");
    }
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_message_with_several_authors_is_unknown(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.list("boss@home.test", TrustTier::Owner).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    // The DMARC check reads one author and the tier another, so a
    // message with several authors has no one sender (RFC 7489 6.6.1).
    // Both authors are at the domain that passes.
    let pass = format!(
        "Authentication-Results: {}\r\n",
        result(MIGADU, "pass", "home.test")
    );
    world.transport.deliver_raw(
        format!(
            "{pass}Message-ID: <two-headers@home.test>\r\n\
             From: someone@home.test\r\n\
             From: boss@home.test\r\n\
             Subject: Two From headers\r\n\
             \r\n\
             Hello.\r\n"
        )
        .as_bytes(),
    );
    world.transport.deliver_raw(&raw_message(
        "boss@home.test, someone@home.test",
        "Two mailboxes",
        &[&result(MIGADU, "pass", "home.test")],
    ));

    let events = world.ingest.wait_for(2, PATIENCE).await;
    for subject in ["Two From headers", "Two mailboxes"] {
        let metadata = stamped(&events, subject);
        assert_eq!(
            metadata["trust_tier"],
            TrustTier::Unknown.as_str(),
            "{subject}"
        );
        assert_eq!(metadata["sender_verified"], false);
        assert_eq!(metadata["sender_verification"], "several_authors");
    }
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_forwarded_message_whose_dmarc_pass_does_not_align_is_unknown(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.list("boss@home.test", TrustTier::Owner).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    // The forwarder passes DMARC for its own domain, which is not the
    // domain in the `From` header.
    world.transport.deliver_raw(&raw_message(
        "Boss <boss@home.test>",
        "Fwd: the invoice",
        &[&result(MIGADU, "pass", "forwarder.test")],
    ));

    let events = world.ingest.wait_for(1, PATIENCE).await;
    let metadata = stamped(&events, "Fwd: the invoice");
    assert_eq!(metadata["trust_tier"], TrustTier::Unknown.as_str());
    assert_eq!(metadata["sender_verified"], false);
    assert_eq!(metadata["sender_verification"], "not_aligned");
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_aligned_dmarc_pass_confirms_the_listed_tier_and_no_more(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.list("care@clinic.test", TrustTier::Trusted).await;
    world.list("bank.test", TrustTier::Trusted).await;
    // The Connection account is owner with no row (ADR-0019). A row
    // the user wrote for that address is read first, so the user can
    // say the account is only trusted.
    world.list("owner@example.com", TrustTier::Trusted).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    world.transport.deliver_raw(&raw_message(
        "Clinic <care@clinic.test>",
        "Your appointment",
        &[&result(MIGADU, "pass", "clinic.test")],
    ));
    world.transport.deliver_raw(&raw_message(
        "Bank <billing@bank.test>",
        "Your statement",
        &[&result(MIGADU, "pass", "bank.test")],
    ));
    world.transport.deliver_raw(&raw_message(
        "owner@example.com",
        "Book it",
        &[&result(MIGADU, "pass", "example.com")],
    ));

    let events = world.ingest.wait_for(3, PATIENCE).await;
    for subject in ["Your appointment", "Your statement", "Book it"] {
        let metadata = stamped(&events, subject);
        assert_eq!(
            metadata["trust_tier"],
            TrustTier::Trusted.as_str(),
            "{subject}"
        );
        assert_eq!(metadata["sender_verified"], true);
        assert_eq!(metadata["sender_verification"], "aligned_dmarc_pass");
    }
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_aligned_dmarc_pass_from_a_connection_account_is_owner(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    world.transport.deliver_raw(&raw_message(
        "owner@example.com",
        "Book it",
        &[&result(MIGADU, "pass", "example.com")],
    ));

    let events = world.ingest.wait_for(1, PATIENCE).await;
    let metadata = stamped(&events, "Book it");
    assert_eq!(metadata["trust_tier"], TrustTier::Owner.as_str());
    assert_eq!(metadata["sender_verified"], true);
    assert_eq!(metadata["sender_verification"], "aligned_dmarc_pass");
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_exact_address_is_read_before_the_domain(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.list("clinic.test", TrustTier::Trusted).await;
    world.list("care@clinic.test", TrustTier::Owner).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    world.transport.deliver_raw(&raw_message(
        "CARE@Clinic.test",
        "Booked",
        &[&result(MIGADU, "pass", "clinic.test")],
    ));

    let events = world.ingest.wait_for(1, PATIENCE).await;
    assert_eq!(
        stamped(&events, "Booked")["trust_tier"],
        TrustTier::Owner.as_str()
    );
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn another_agents_entry_does_not_lift_this_agents_mail(pool: SqlitePool) {
    let world = World::new(pool).await;
    world
        .list_for(
            Some(&world.other_agent_id),
            "care@clinic.test",
            TrustTier::Owner,
        )
        .await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    world.transport.deliver_raw(&raw_message(
        "care@clinic.test",
        "Hello",
        &[&result(MIGADU, "pass", "clinic.test")],
    ));

    let events = world.ingest.wait_for(1, PATIENCE).await;
    let metadata = stamped(&events, "Hello");
    assert_eq!(metadata["trust_tier"], TrustTier::Unknown.as_str());
    // The sender is verified; this Agent's list holds no row for it.
    assert_eq!(metadata["sender_verified"], true);
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn every_sender_on_a_manual_host_is_unknown(pool: SqlitePool) {
    let world = World::on_manual_host(pool).await;
    world.list("boss@home.test", TrustTier::Owner).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    // Pagis knows no authserv-id for a manual host, so no result on
    // the message can be told from one the sender wrote.
    world.transport.deliver_raw(&raw_message(
        "boss@home.test",
        "Wire the money",
        &[&result(MIGADU, "pass", "home.test")],
    ));
    world.transport.deliver_raw(&raw_message(
        "owner@example.com",
        "Book it",
        &[&result("mx.example.net", "pass", "example.com")],
    ));

    let events = world.ingest.wait_for(2, PATIENCE).await;
    for subject in ["Wire the money", "Book it"] {
        let metadata = stamped(&events, subject);
        assert_eq!(
            metadata["trust_tier"],
            TrustTier::Unknown.as_str(),
            "{subject}"
        );
        assert_eq!(metadata["sender_verified"], false);
        assert_eq!(metadata["sender_verification"], "manual_host");
    }
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_forged_owner_message_does_not_pass_a_min_trust_filter_of_trusted(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.list("boss@home.test", TrustTier::Owner).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    world
        .transport
        .deliver_raw(&raw_message("boss@home.test", "Forged", &[]));
    world.transport.deliver_raw(&raw_message(
        "boss@home.test",
        "Verified",
        &[&result(MIGADU, "pass", "home.test")],
    ));

    let events = world.ingest.wait_for(2, PATIENCE).await;
    let filter = serde_json::json!({"mailbox": OWN_MAILBOX, "min_trust": "trusted"});
    assert!(
        !MailMessageMatcher.matches(&filter, &stamped(&events, "Forged")),
        "a forged Owner message does not wake the Agent"
    );
    assert!(
        MailMessageMatcher.matches(&filter, &stamped(&events, "Verified")),
        "the same sender, verified, wakes it"
    );
    world.collector.stop();
}

// ----- Deduplication and the resync -----

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_resync_after_a_reconnect_hands_no_message_over_twice(pool: SqlitePool) {
    let world = World::new(pool).await;
    let mailbox = world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;
    world.transport.deliver("care@clinic.test", "First", "");
    let first = world.ingest.wait_for(1, PATIENCE).await;
    world.wait_for_cursor(&mailbox, 1).await;

    // The host drops the session. The task connects again and resyncs
    // from the cursor, so the message it already read is not read
    // again.
    world
        .transport
        .fail_with(Some(TransportErrorCode::Unreachable));
    tokio::time::sleep(TURN * 3).await;
    world.transport.fail_with(None);
    world.transport.deliver("care@clinic.test", "Second", "");

    let events = world.ingest.wait_for(2, PATIENCE).await;
    assert_eq!(events.len(), 2, "the resync adds the new message only");
    assert_eq!(events[0].provider_event_id, first[0].provider_event_id);
    assert_ne!(events[1].provider_event_id, events[0].provider_event_id);
    // The identity is the `Message-ID` the sender wrote, so the same
    // message read twice is one occurrence.
    assert!(events[0].provider_event_id.contains("@fake.invalid"));
    assert!(events[0].provider_event_id.starts_with("ada@example.com "));
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_cursor_stays_where_it_was_when_a_pass_is_refused(pool: SqlitePool) {
    let world = World::new(pool).await;
    let mailbox = world.provision(&world.agent_id, "ada").await;
    let head = world.read(&mailbox).await.cursor.unwrap();
    world.ingest.refuse_with(Some("the store is busy"));
    world.collector.sweep().await;
    world.transport.deliver("care@clinic.test", "First", "");
    tokio::time::sleep(TURN * 3).await;

    assert_eq!(
        world.read(&mailbox).await.cursor.as_ref(),
        Some(&head),
        "a refused pass leaves the cursor, so the message is read again"
    );
    world.ingest.refuse_with(None);
    let events = world.ingest.wait_for(1, PATIENCE).await;
    assert_eq!(events.len(), 1);
    world.collector.stop();
}

// ----- The landing Thread -----

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_reply_lands_in_the_thread_that_sent_the_message_it_answers(pool: SqlitePool) {
    let world = World::new(pool).await;
    let mailbox = world.provision(&world.agent_id, "ada").await;
    let channel_id = world.channel_id.clone();
    let thread_id = world.thread_id.clone();
    world
        .sent
        .record(&SentMail {
            message_id: "<sent-1@example.com>".to_string(),
            workspace_id: world.workspace_id.clone(),
            agent_id: world.agent_id.clone(),
            mailbox_id: mailbox.id.clone(),
            run_id: world.run_id.clone(),
            channel_id: Some(channel_id.clone()),
            thread_id: Some(thread_id.clone()),
            sent_at: now_ms(),
        })
        .await
        .unwrap();
    world.collector.sweep().await;

    world
        .transport
        .deliver_answer("<sent-1@example.com>", "care@clinic.test", "Re: ask", "");
    world.transport.deliver("news@example.net", "Unrelated", "");

    let events = world.ingest.wait_for(2, PATIENCE).await;
    let answer = &events[0];
    assert_eq!(answer.metadata["in_reply_to"], "<sent-1@example.com>");
    let landing = answer.landing.clone().expect("the reply lands in a thread");
    assert_eq!(landing.channel_id, channel_id);
    assert_eq!(landing.root_message_id, Some(thread_id));
    // Mail that answers nothing lands where the rule says, which is
    // the Agent's Thread with the user.
    assert!(events[1].landing.is_none());
    world.collector.stop();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_reply_to_another_mailboxs_message_lands_where_the_rule_says(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.provision(&world.agent_id, "ada").await;
    world
        .sent
        .record(&SentMail {
            message_id: "<sent-2@example.com>".to_string(),
            workspace_id: world.workspace_id.clone(),
            agent_id: world.other_agent_id.clone(),
            // Another Agent's mailbox sent this message.
            mailbox_id: AgentMailboxId::generate(),
            run_id: world.run_id.clone(),
            channel_id: Some(world.channel_id.clone()),
            thread_id: None,
            sent_at: now_ms(),
        })
        .await
        .unwrap();
    world.collector.sweep().await;

    world
        .transport
        .deliver_answer("<sent-2@example.com>", "care@clinic.test", "Re: ask", "");

    let events = world.ingest.wait_for(1, PATIENCE).await;
    assert_eq!(events.len(), 1);
    assert!(
        events[0].landing.is_none(),
        "a Thread another Agent's mailbox opened is not this one's landing"
    );
    world.collector.stop();
}

// ----- The task set -----

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_task_stops_when_the_mailbox_leaves_active(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;
    assert_eq!(world.collector.watched(), 1);

    world
        .desk
        .make_dormant_for_agent(&world.workspace_id, &world.agent_id)
        .await
        .unwrap();
    world.collector.sweep().await;

    assert_eq!(
        world.collector.watched(),
        0,
        "a dormant mailbox is not read"
    );
    world
        .transport
        .deliver("care@clinic.test", "While asleep", "");
    tokio::time::sleep(TURN * 3).await;
    assert!(world.ingest.events().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_refused_login_makes_the_mailbox_unavailable_and_stops_the_task(pool: SqlitePool) {
    let world = World::new(pool).await;
    let mailbox = world.provision(&world.agent_id, "ada").await;
    world
        .transport
        .refuse_login_with(Some(TransportErrorCode::Unauthorized));
    world.collector.sweep().await;

    world
        .wait_for_state(&mailbox, AgentMailboxState::Unavailable)
        .await;
    let record = world.read(&mailbox).await;
    assert!(
        record
            .reason
            .is_some_and(|reason| reason.contains("refused")),
        "the desk says why the mailbox needs the user"
    );
    // The finished task leaves the set on the next sweep.
    world.collector.sweep().await;
    assert_eq!(world.collector.watched(), 0);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_transport_without_idle_polls_the_folder(pool: SqlitePool) {
    let world = World::with_transport(
        pool,
        FakeMailTransport::with_capabilities(TransportCapabilities { idle: false }),
    )
    .await;
    world.provision(&world.agent_id, "ada").await;
    world.collector.sweep().await;

    world.transport.deliver("care@clinic.test", "Polled", "");

    let events = world.ingest.wait_for(1, PATIENCE).await;
    assert_eq!(events.len(), 1, "the poll finds what IDLE would have");
    world.collector.stop();
}

// ----- The typed filter -----

fn metadata(from: &str, subject: &str, tier: TrustTier, answers: bool) -> serde_json::Value {
    let mut metadata = serde_json::json!({
        "mailbox": OWN_MAILBOX,
        "message_id": "INBOX:1",
        "from": from,
        "from_domain": pagis_mail::sender_domain(from).unwrap(),
        "subject": subject,
        "trust_tier": tier.as_str(),
    });
    if answers {
        metadata["in_reply_to"] = "<sent-1@example.com>".into();
    }
    metadata
}

#[test]
fn an_empty_filter_matches_every_message() {
    let matcher = MailMessageMatcher;
    assert!(matcher.matches(
        &serde_json::json!({}),
        &metadata("a@example.net", "Hello", TrustTier::Unknown, false)
    ));
}

#[test]
fn the_mailbox_field_keeps_a_rule_on_its_own_mailbox() {
    let matcher = MailMessageMatcher;
    let message = metadata("a@example.net", "Hello", TrustTier::Unknown, false);
    assert!(matcher.matches(&serde_json::json!({"mailbox": OWN_MAILBOX}), &message));
    assert!(!matcher.matches(&serde_json::json!({"mailbox": "work"}), &message));
}

#[test]
fn a_sender_is_an_address_or_a_bare_domain() {
    let matcher = MailMessageMatcher;
    let message = metadata(
        "Clinic <Care@Clinic.test>",
        "Hello",
        TrustTier::Unknown,
        false,
    );
    for senders in [
        serde_json::json!(["care@clinic.test"]),
        serde_json::json!(["clinic.test"]),
        serde_json::json!(["@clinic.test"]),
        serde_json::json!(["other@example.net", "care@clinic.test"]),
    ] {
        assert!(
            matcher.matches(&serde_json::json!({"senders": senders}), &message),
            "{senders} matches"
        );
    }
    assert!(!matcher.matches(
        &serde_json::json!({"senders": ["other@example.net"]}),
        &message
    ));
    // An empty list narrows nothing.
    assert!(matcher.matches(&serde_json::json!({"senders": []}), &message));
}

#[test]
fn subject_contains_reads_case_insensitively() {
    let matcher = MailMessageMatcher;
    let message = metadata(
        "a@example.net",
        "Your Appointment",
        TrustTier::Unknown,
        false,
    );
    assert!(matcher.matches(
        &serde_json::json!({"subject_contains": ["appointment"]}),
        &message
    ));
    assert!(!matcher.matches(
        &serde_json::json!({"subject_contains": ["invoice"]}),
        &message
    ));
}

#[test]
fn min_trust_stops_a_sender_below_the_tier_the_rule_asks_for() {
    let matcher = MailMessageMatcher;
    let owner = metadata("a@example.net", "Hi", TrustTier::Owner, false);
    let unknown = metadata("a@example.net", "Hi", TrustTier::Unknown, false);
    let trusted = metadata("a@example.net", "Hi", TrustTier::Trusted, false);
    let filter = serde_json::json!({"min_trust": "trusted"});
    assert!(matcher.matches(&filter, &owner));
    assert!(matcher.matches(&filter, &trusted));
    assert!(!matcher.matches(&filter, &unknown));
    assert!(matcher.matches(&serde_json::json!({"min_trust": "unknown"}), &unknown));
    assert!(!matcher.matches(&serde_json::json!({"min_trust": "owner"}), &trusted));
}

#[test]
fn replies_only_keeps_the_mail_that_answers_the_agent() {
    let matcher = MailMessageMatcher;
    let filter = serde_json::json!({"replies_only": true});
    assert!(matcher.matches(
        &filter,
        &metadata("a@example.net", "Re: ask", TrustTier::Unknown, true)
    ));
    assert!(!matcher.matches(
        &filter,
        &metadata("a@example.net", "News", TrustTier::Unknown, false)
    ));
}

#[test]
fn the_fields_of_a_filter_combine_with_and() {
    let matcher = MailMessageMatcher;
    let filter = serde_json::json!({
        "senders": ["clinic.test"],
        "subject_contains": ["appointment"],
        "min_trust": "unknown",
    });
    assert!(matcher.matches(
        &filter,
        &metadata(
            "care@clinic.test",
            "Your appointment",
            TrustTier::Unknown,
            false
        )
    ));
    assert!(
        !matcher.matches(
            &filter,
            &metadata(
                "care@clinic.test",
                "Your invoice",
                TrustTier::Unknown,
                false
            )
        ),
        "one field that does not match is enough to refuse"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn one_sweep_watches_the_mailboxes_of_two_people(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.provision(&world.agent_id, "ada").await;
    let (_workspace, _agent) = world.second_person().await;

    world.collector.sweep().await;

    assert_eq!(
        world.collector.watched(),
        2,
        "one pass reads the tenant off each mailbox row"
    );
    world.collector.stop();
}
