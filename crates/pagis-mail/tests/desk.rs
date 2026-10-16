//! The Agent Mailbox lifecycle (ADR-0019): provision, prove,
//! recover, sleep and delete, against real records and a mail host and
//! transport that answer from memory. `#[sqlx::test]` gives each test a
//! fresh database with the migrations applied.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use pagis_audit::AuditEventBus;
use pagis_core::{
    Agent, AgentId, AgentMailbox, AgentMailboxState, AgentMailboxStore, AgentStatus, AgentStore,
    Connection, ConnectionId, ConnectionStore, EventLog, MemorySecretStore, SecretStore, Workspace,
    WorkspaceId, WorkspaceStore, now_ms,
};
use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost, FakeStandingRule, HostCall};
use pagis_mail::{
    DEFAULT_OUTGOING_CAP, Endpoint, HostCapabilities, MANUAL_PROVIDER, MIGADU_PROVIDER,
    MailboxCapabilities, MailboxDesk, MailboxDeskDeps, MailboxError, MailboxHost, MailboxProvider,
    NewMailbox, ProofTiming, TransportErrorCode, host_api_key_secret_name,
    mailbox_password_secret_name,
};
use pagis_storage_sqlite::{
    SqliteAgentMailboxStore, SqliteAgentStore, SqliteConnectionStore, SqliteEventLog,
    SqliteWorkspaceStore,
};
use sqlx::SqlitePool;

const MIGADU_ALIAS: &str = "mail";
const MANUAL_ALIAS: &str = "own-host";

/// Record each secret read, including reads of a name with no value.
#[derive(Default)]
struct RecordingSecrets {
    values: MemorySecretStore,
    reads: Mutex<Vec<String>>,
}

impl SecretStore for RecordingSecrets {
    fn get(&self, name: &str) -> Result<Option<String>, pagis_core::SecretError> {
        self.reads.lock().unwrap().push(name.to_string());
        self.values.get(name)
    }

    fn set(&self, name: &str, value: &str) -> Result<(), pagis_core::SecretError> {
        self.values.set(name, value)
    }

    fn get_or_insert(&self, name: &str, value: &str) -> Result<String, pagis_core::SecretError> {
        self.values.get_or_insert(name, value)
    }

    fn delete(&self, name: &str) -> Result<(), pagis_core::SecretError> {
        self.values.delete(name)
    }
}

/// The desk, its records and the two seams behind it.
struct World {
    desk: Arc<MailboxDesk>,
    mailboxes: Arc<SqliteAgentMailboxStore>,
    secrets: Arc<RecordingSecrets>,
    events: Arc<SqliteEventLog>,
    host: Arc<FakeMailboxHost>,
    transport: Arc<FakeMailTransport>,
    standing_rule: Arc<FakeStandingRule>,
    workspace_id: WorkspaceId,
    migadu_id: ConnectionId,
    manual_id: ConnectionId,
    agent_id: AgentId,
    other_agent_id: AgentId,
    archived_agent_id: AgentId,
}

impl World {
    /// A Workspace with a Migadu and a manual Mailbox Provider, three
    /// Agents, and a proof that gives up at once so no test waits.
    async fn new(pool: SqlitePool) -> Self {
        Self::with_endpoints(pool, true).await
    }

    async fn with_endpoints(pool: SqlitePool, endpoints: bool) -> Self {
        Self::with_host(pool, endpoints, FakeMailboxHost::default()).await
    }

    async fn with_host(pool: SqlitePool, endpoints: bool, host: FakeMailboxHost) -> Self {
        let workspaces = SqliteWorkspaceStore::new(pool.clone());
        let person =
            pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", pagis_core::now_ms())
                .await
                .unwrap();
        let workspace = Workspace {
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
        let agent_id =
            seed_agent(&agents, &workspace.id, "Ada Lovelace", AgentStatus::Active).await;
        let other_agent_id =
            seed_agent(&agents, &workspace.id, "Grace Hopper", AgentStatus::Active).await;
        let archived_agent_id =
            seed_agent(&agents, &workspace.id, "Alan Turing", AgentStatus::Archived).await;

        let connections = Arc::new(SqliteConnectionStore::new(pool.clone()));
        let secrets = Arc::new(RecordingSecrets::default());
        let host = Arc::new(host);
        let mut migadu = mail_connection(
            &workspace.id,
            MIGADU_PROVIDER,
            MIGADU_ALIAS,
            "example.com",
            MailboxCapabilities::of(host.capabilities(), true),
        );
        if !endpoints {
            migadu.provider = "embedder-mail".into();
            for key in ["imap_host", "imap_port", "smtp_host", "smtp_port"] {
                migadu.config.as_object_mut().unwrap().remove(key);
            }
        }
        connections.create(&migadu).await.unwrap();
        if host.capabilities().api_key {
            secrets
                .set(&host_api_key_secret_name(MIGADU_ALIAS), "migadu-key")
                .unwrap();
        }
        let manual = mail_connection(
            &workspace.id,
            MANUAL_PROVIDER,
            MANUAL_ALIAS,
            "example.org",
            MailboxCapabilities::of(
                HostCapabilities {
                    api_key: false,
                    outgoing_cap: false,
                    delete_mailbox: false,
                    reset_password: false,
                },
                true,
            ),
        );
        connections.create(&manual).await.unwrap();

        let mailboxes = Arc::new(SqliteAgentMailboxStore::new(pool.clone()));
        let workspace_mailboxes = Arc::new(pagis_storage_sqlite::SqliteWorkspaceMailboxStore::new(
            pool.clone(),
        ));
        let events = Arc::new(SqliteEventLog::new(pool.clone()));
        let transport = Arc::new(FakeMailTransport::default());
        let standing_rule = Arc::new(FakeStandingRule::default());
        let desk = Arc::new(MailboxDesk::new(MailboxDeskDeps {
            intake: Arc::new(pagis_mail::MailIntake {
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
            agents: agents as _,
            host: Arc::clone(&host) as _,
            transport: Arc::clone(&transport) as _,
            secrets: Arc::clone(&secrets) as _,
            bus: Arc::new(AuditEventBus::new(Arc::clone(&events) as _)),
            standing_rule: Arc::clone(&standing_rule) as _,
            // One attempt, then the reason. A test never waits three
            // minutes for the real budget.
            proof: ProofTiming {
                first_wait: Duration::from_millis(1),
                max_wait: Duration::from_millis(1),
                budget: Duration::ZERO,
            },
        }));
        Self {
            desk,
            mailboxes,
            secrets,
            events,
            host,
            transport,
            standing_rule,
            workspace_id: workspace.id,
            migadu_id: migadu.id,
            manual_id: manual.id,
            agent_id,
            other_agent_id,
            archived_agent_id,
        }
    }

    /// One mailbox on the Migadu provider, proven and `active`.
    async fn provision_active(&self, agent_id: &AgentId, local_part: &str) -> AgentMailbox {
        let mailbox = self
            .desk
            .provision(&self.workspace_id, agent_id, self.hosted(local_part))
            .await
            .unwrap();
        assert_eq!(mailbox.state, AgentMailboxState::Provisioning);
        self.settled(&mailbox).await
    }

    fn hosted(&self, local_part: &str) -> NewMailbox {
        NewMailbox {
            connection_id: self.migadu_id.clone(),
            local_part: local_part.to_string(),
            outgoing_cap: None,
            password: None,
        }
    }

    fn assigned(&self, local_part: &str, password: &str) -> NewMailbox {
        NewMailbox {
            connection_id: self.manual_id.clone(),
            local_part: local_part.to_string(),
            outgoing_cap: None,
            password: Some(password.to_string()),
        }
    }

    /// The record once the background proof has left `provisioning`.
    async fn settled(&self, mailbox: &AgentMailbox) -> AgentMailbox {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let record = self
                    .mailboxes
                    .get(&self.workspace_id, &mailbox.id)
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

    /// Wait for one audit line. The state is written before the event,
    /// so a settled record can outrun its own audit by a moment.
    async fn wait_for_event(&self, event_type: &str) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self
                    .event_types()
                    .await
                    .iter()
                    .any(|seen| seen == event_type)
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{event_type} is audited"));
    }

    async fn event_types(&self) -> Vec<String> {
        self.events
            .list_after(None, 0, 100)
            .await
            .unwrap()
            .into_iter()
            .map(|event| event.event_type)
            .collect()
    }
}

async fn seed_agent(
    agents: &SqliteAgentStore,
    workspace_id: &WorkspaceId,
    name: &str,
    status: AgentStatus,
) -> AgentId {
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
        status,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    agents.create(&agent).await.unwrap();
    agent.id
}

fn mail_connection(
    workspace_id: &WorkspaceId,
    provider: &str,
    alias: &str,
    domain: &str,
    capabilities: MailboxCapabilities,
) -> Connection {
    let settings = MailboxProvider {
        account: Some("owner@example.com".to_string()),
        domain: domain.to_string(),
        imap: Some(Endpoint::new("imap.example.net", 993)),
        smtp: Some(Endpoint::new("smtp.example.net", 465)),
        capabilities,
    };
    Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace_id.clone(),
        provider: provider.to_string(),
        alias: alias.to_string(),
        display_name: "Agent mail".to_string(),
        status: Connection::CONNECTED.to_string(),
        authorized_capabilities: Vec::new(),
        config: settings.config(),
        created_at: now_ms(),
    }
}

// The five states, in the order a mailbox meets them.

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_new_mailbox_provisions_and_then_proves_its_login(pool: SqlitePool) {
    let world = World::new(pool).await;

    let mailbox = world
        .desk
        .provision(&world.workspace_id, &world.agent_id, world.hosted("ada"))
        .await
        .unwrap();

    // The answer arrives once the host has made the mailbox: the login
    // is not yet proven (ADR-0019).
    assert_eq!(mailbox.state, AgentMailboxState::Provisioning);
    assert_eq!(mailbox.address, "ada@example.com");
    assert_eq!(mailbox.outgoing_cap, DEFAULT_OUTGOING_CAP);
    assert_eq!(
        world.host.calls(),
        vec![HostCall::Create {
            local_part: "ada".to_string(),
            outgoing_cap: DEFAULT_OUTGOING_CAP,
        }]
    );
    // The password the daemon generated is in the secret store and
    // nowhere else (ADR-0013).
    let password = world
        .secrets
        .get(&mailbox_password_secret_name(
            &world.workspace_id,
            "ada@example.com",
        ))
        .unwrap()
        .expect("the mailbox password is kept");
    assert_eq!(
        world.host.password_of("ada@example.com").as_deref(),
        Some(password.as_str())
    );

    let active = world.settled(&mailbox).await;
    assert_eq!(active.state, AgentMailboxState::Active);
    assert_eq!(active.reason, None);
    assert_eq!(
        world.transport.logins(),
        vec!["ada@example.com".to_string()]
    );
    world.wait_for_event("agent_mailbox.active").await;
    assert_eq!(
        world.event_types().await,
        vec![
            "agent_mailbox.provisioned".to_string(),
            "agent_mailbox.active".to_string(),
        ]
    );
}

/// The cursor starts where the folder stands, so mail that arrived
/// before the mailbox was the Agent's wakes nobody (ADR-0019).
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_cursor_starts_at_the_first_proven_login(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.transport.deliver("old@example.net", "Before", "body");
    world
        .transport
        .deliver("older@example.net", "Also before", "body");

    let active = world.provision_active(&world.agent_id, "ada").await;

    let cursor = active.cursor.expect("the cursor is set at the proof");
    assert_eq!(cursor.folder, "INBOX");
    // Both messages are behind the cursor, so they are readable and
    // wake nobody.
    assert_eq!(cursor.last_uid, 2);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_login_the_host_refuses_leaves_the_mailbox_unavailable(pool: SqlitePool) {
    let world = World::new(pool).await;
    world
        .transport
        .refuse_login_with(Some(TransportErrorCode::Unauthorized));

    let mailbox = world
        .desk
        .provision(&world.workspace_id, &world.agent_id, world.hosted("ada"))
        .await
        .unwrap();

    let unavailable = world.settled(&mailbox).await;
    assert_eq!(unavailable.state, AgentMailboxState::Unavailable);
    // The reason reaches the desk, and it is the stable code.
    assert!(
        unavailable
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("unauthorized"),
        "{:?}",
        unavailable.reason
    );
    world.wait_for_event("agent_mailbox.unavailable").await;
}

/// Recovery: a new password, the login proven again, the cursor
/// unchanged (ADR-0019).
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_password_reset_returns_an_unavailable_mailbox_to_active(pool: SqlitePool) {
    let world = World::new(pool).await;
    let active = world.provision_active(&world.agent_id, "ada").await;
    let cursor = active.cursor.clone().expect("the cursor is set");
    let first_password = world
        .secrets
        .get(&mailbox_password_secret_name(
            &world.workspace_id,
            "ada@example.com",
        ))
        .unwrap()
        .unwrap();
    world
        .transport
        .refuse_login_with(Some(TransportErrorCode::Unauthorized));
    world
        .mailboxes
        .set_state(
            &world.workspace_id,
            &active.id,
            AgentMailboxState::Unavailable,
            Some("refused"),
        )
        .await
        .unwrap();
    world.transport.refuse_login_with(None);

    let reset = world
        .desk
        .reset_password(&world.workspace_id, &active.id, None)
        .await
        .unwrap();

    assert_eq!(reset.state, AgentMailboxState::Provisioning);
    let recovered = world.settled(&reset).await;
    assert_eq!(recovered.state, AgentMailboxState::Active);
    assert_eq!(recovered.reason, None);
    // The cursor does not move, so nothing that arrived while the
    // mailbox was unavailable is skipped.
    assert_eq!(recovered.cursor, Some(cursor));
    // The host minted a new password and the store holds it.
    let second_password = world
        .secrets
        .get(&mailbox_password_secret_name(
            &world.workspace_id,
            "ada@example.com",
        ))
        .unwrap()
        .unwrap();
    assert_ne!(first_password, second_password);
    assert!(
        world
            .host
            .calls()
            .contains(&HostCall::ResetPassword("ada@example.com".to_string()))
    );
}

/// A manual host has no API, so the user pastes the new password.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_manual_host_takes_the_password_the_user_pastes(pool: SqlitePool) {
    let world = World::new(pool).await;
    let mailbox = world
        .desk
        .provision(
            &world.workspace_id,
            &world.agent_id,
            world.assigned("ada", "first-password"),
        )
        .await
        .unwrap();
    world.settled(&mailbox).await;

    let refused = world
        .desk
        .reset_password(&world.workspace_id, &mailbox.id, None)
        .await;
    assert!(
        matches!(refused, Err(MailboxError::Validation(_))),
        "{refused:?}"
    );

    let reset = world
        .desk
        .reset_password(
            &world.workspace_id,
            &mailbox.id,
            Some("second-password".to_string()),
        )
        .await
        .unwrap();

    assert_eq!(world.settled(&reset).await.state, AgentMailboxState::Active);
    assert_eq!(
        world
            .secrets
            .get(&mailbox_password_secret_name(
                &world.workspace_id,
                "ada@example.org"
            ))
            .unwrap()
            .as_deref(),
        Some("second-password")
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn archiving_the_agent_makes_the_mailbox_dormant(pool: SqlitePool) {
    let world = World::new(pool).await;
    let active = world.provision_active(&world.agent_id, "ada").await;

    let dormant = world
        .desk
        .make_dormant_for_agent(&world.workspace_id, &world.agent_id)
        .await
        .unwrap()
        .expect("the Agent holds a mailbox");

    assert_eq!(dormant.state, AgentMailboxState::Dormant);
    // The mailbox stays with the Agent: unlike a phone number it never
    // returns to the Workspace (ADR-0019).
    let held = world
        .desk
        .for_agent(&world.workspace_id, &world.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.id, active.id);
    assert_eq!(held.address, "ada@example.com");
    world.wait_for_event("agent_mailbox.dormant").await;
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn deleting_a_mailbox_destroys_its_secret_and_writes_a_tombstone(pool: SqlitePool) {
    let world = World::new(pool).await;
    let active = world.provision_active(&world.agent_id, "ada").await;

    let deleted = world
        .desk
        .delete(&world.workspace_id, &active.id, "ada@example.com")
        .await
        .unwrap();

    assert_eq!(deleted.mailbox.state, AgentMailboxState::Deleted);
    assert!(deleted.mailbox.deleted_at.is_some());
    // A Migadu host deletes the mailbox itself, so there is no notice.
    assert_eq!(deleted.notice, None);
    assert!(
        world
            .host
            .calls()
            .contains(&HostCall::Delete("ada@example.com".to_string()))
    );
    assert_eq!(
        world
            .secrets
            .get(&mailbox_password_secret_name(
                &world.workspace_id,
                "ada@example.com"
            ))
            .unwrap(),
        None
    );
    assert_eq!(
        world
            .desk
            .for_agent(&world.workspace_id, &world.agent_id)
            .await
            .unwrap(),
        None
    );
    world.wait_for_event("agent_mailbox.deleted").await;
}

/// The user types the address to confirm the delete, because the
/// host's mail goes with the mailbox (ADR-0019).
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_delete_needs_the_address_typed_back(pool: SqlitePool) {
    let world = World::new(pool).await;
    let active = world.provision_active(&world.agent_id, "ada").await;

    let refused = world
        .desk
        .delete(&world.workspace_id, &active.id, "grace@example.com")
        .await;

    assert!(
        matches!(refused, Err(MailboxError::Validation(_))),
        "{refused:?}"
    );
    // Nothing happened: the mailbox and its password are still there.
    assert!(
        world
            .desk
            .for_agent(&world.workspace_id, &world.agent_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        world
            .secrets
            .get(&mailbox_password_secret_name(
                &world.workspace_id,
                "ada@example.com"
            ))
            .unwrap()
            .is_some()
    );
    // The same address in another case confirms it.
    world
        .desk
        .delete(&world.workspace_id, &active.id, " ADA@Example.com ")
        .await
        .unwrap();
}

/// A manual host forgets the record and tells the user to delete the
/// mailbox at the host (ADR-0019).
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn deleting_a_manual_mailbox_says_the_user_must_finish_it(pool: SqlitePool) {
    let world = World::new(pool).await;
    let mailbox = world
        .desk
        .provision(
            &world.workspace_id,
            &world.agent_id,
            world.assigned("ada", "pasted"),
        )
        .await
        .unwrap();

    let deleted = world
        .desk
        .delete(&world.workspace_id, &mailbox.id, "ada@example.org")
        .await
        .unwrap();

    assert!(deleted.notice.is_some(), "the desk tells your Person");
}

// The Address Ledger.

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_deleted_address_is_never_given_to_another_agent(pool: SqlitePool) {
    let world = World::new(pool).await;
    let active = world.provision_active(&world.agent_id, "ada").await;
    world
        .desk
        .delete(&world.workspace_id, &active.id, "ada@example.com")
        .await
        .unwrap();

    let again = world
        .desk
        .provision(
            &world.workspace_id,
            &world.other_agent_id,
            world.hosted("ada"),
        )
        .await;

    assert!(matches!(again, Err(MailboxError::Conflict(_))), "{again:?}");
    // The Agent may hold a fresh mailbox with another address.
    let fresh = world.provision_active(&world.agent_id, "ada2").await;
    assert_eq!(fresh.address, "ada2@example.com");
}

/// The ledger spans every Connection: one address is one address,
/// whichever host carries it (ADR-0019).
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_suggestion_steps_around_the_ledger(pool: SqlitePool) {
    let world = World::new(pool).await;

    let offers = world.desk.offers("Ada Lovelace").await.unwrap();
    assert_eq!(offers.len(), 2, "both providers are connected");
    let migadu = offers
        .iter()
        .find(|offer| offer.connection.id == world.migadu_id)
        .unwrap();
    assert_eq!(
        migadu.settings.password(),
        pagis_mail::MailboxPasswordKind::Minted
    );
    assert_eq!(migadu.suggested_local_part, "ada.lovelace");

    world
        .provision_active(&world.agent_id, "ada.lovelace")
        .await;

    let offers = world.desk.offers("Ada Lovelace").await.unwrap();
    let migadu = offers
        .iter()
        .find(|offer| offer.connection.id == world.migadu_id)
        .unwrap();
    assert_eq!(migadu.suggested_local_part, "ada.lovelace2");
    // The other domain is untouched: the collision is per address.
    let manual = offers
        .iter()
        .find(|offer| offer.connection.id == world.manual_id)
        .unwrap();
    assert_eq!(
        manual.settings.password(),
        pagis_mail::MailboxPasswordKind::Typed
    );
    assert_eq!(manual.suggested_local_part, "ada.lovelace");
}

// The form errors.

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_form_refuses_a_reserved_name(pool: SqlitePool) {
    let world = World::new(pool).await;

    let refused = world
        .desk
        .provision(
            &world.workspace_id,
            &world.agent_id,
            world.hosted("postmaster"),
        )
        .await;

    assert!(
        matches!(refused, Err(MailboxError::Validation(_))),
        "{refused:?}"
    );
    // The host was never asked.
    assert_eq!(world.host.calls(), Vec::new());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_form_refuses_a_second_mailbox_for_one_agent(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.provision_active(&world.agent_id, "ada").await;

    let refused = world
        .desk
        .provision(&world.workspace_id, &world.agent_id, world.hosted("ada2"))
        .await;

    assert!(
        matches!(refused, Err(MailboxError::Conflict(_))),
        "{refused:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_archived_agent_gets_no_mailbox(pool: SqlitePool) {
    let world = World::new(pool).await;

    let refused = world
        .desk
        .provision(
            &world.workspace_id,
            &world.archived_agent_id,
            world.hosted("alan"),
        )
        .await;

    assert!(
        matches!(refused, Err(MailboxError::Validation(_))),
        "{refused:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_manual_provider_needs_the_password_and_a_host_api_refuses_one(pool: SqlitePool) {
    let world = World::new(pool).await;

    let missing = world
        .desk
        .provision(
            &world.workspace_id,
            &world.agent_id,
            NewMailbox {
                password: None,
                ..world.assigned("ada", "")
            },
        )
        .await;
    assert!(
        matches!(missing, Err(MailboxError::Validation(_))),
        "{missing:?}"
    );

    let unwanted = world
        .desk
        .provision(
            &world.workspace_id,
            &world.agent_id,
            NewMailbox {
                password: Some("typed".to_string()),
                ..world.hosted("ada")
            },
        )
        .await;
    assert!(
        matches!(unwanted, Err(MailboxError::Validation(_))),
        "{unwanted:?}"
    );
}

/// A host that refuses the create answers the form, and the address it
/// refused is free again (ADR-0019).
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_host_refusal_drops_the_reservation(pool: SqlitePool) {
    let world = World::new(pool).await;
    world
        .host
        .fail_with(Some(pagis_mail::HostErrorCode::AddressTaken));

    let refused = world
        .desk
        .provision(&world.workspace_id, &world.agent_id, world.hosted("ada"))
        .await;

    assert!(matches!(refused, Err(MailboxError::Host(_))), "{refused:?}");
    // The ledger holds nothing, so the same name works once the host
    // is willing.
    assert!(!world.desk.address_taken("ada@example.com").await.unwrap());
    assert_eq!(
        world
            .desk
            .for_agent(&world.workspace_id, &world.agent_id)
            .await
            .unwrap(),
        None
    );
    world.host.fail_with(None);
    let mailbox = world.provision_active(&world.agent_id, "ada").await;
    assert_eq!(mailbox.address, "ada@example.com");
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn no_mailbox_is_made_through_a_provider_that_is_not_connected(pool: SqlitePool) {
    let world = World::new(pool).await;
    let refused = world
        .desk
        .provision(
            &world.workspace_id,
            &world.agent_id,
            NewMailbox {
                connection_id: ConnectionId::generate(),
                ..world.hosted("ada")
            },
        )
        .await;

    assert!(
        matches!(refused, Err(MailboxError::NoProvider)),
        "{refused:?}"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn an_outgoing_cap_of_zero_is_refused(pool: SqlitePool) {
    let world = World::new(pool).await;

    let refused = world
        .desk
        .provision(
            &world.workspace_id,
            &world.agent_id,
            NewMailbox {
                outgoing_cap: Some(0),
                ..world.hosted("ada")
            },
        )
        .await;

    assert!(
        matches!(refused, Err(MailboxError::Validation(_))),
        "{refused:?}"
    );
}

/// Agent creation runs the checks before it writes the Agent, so a
/// name the ledger holds makes no Agent (ADR-0019).
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_check_reads_the_ledger_before_an_agent_exists(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.provision_active(&world.agent_id, "ada").await;

    let refused = world.desk.check(&world.hosted("ada")).await;

    assert!(
        matches!(refused, Err(MailboxError::Conflict(_))),
        "{refused:?}"
    );
    world.desk.check(&world.hosted("grace")).await.unwrap();
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_connection_a_mailbox_points_at_is_counted(pool: SqlitePool) {
    let world = World::new(pool).await;
    world.provision_active(&world.agent_id, "ada").await;
    world.provision_active(&world.other_agent_id, "grace").await;

    assert_eq!(
        world
            .desk
            .connection_in_use(&world.workspace_id, &world.migadu_id)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        world
            .desk
            .connection_in_use(&world.workspace_id, &world.manual_id)
            .await
            .unwrap(),
        0
    );
}

// The Standing Mail Rule (ADR-0019).

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn provisioning_a_mailbox_creates_its_standing_mail_rule(pool: SqlitePool) {
    let world = World::new(pool).await;

    let mailbox = world
        .desk
        .provision(&world.workspace_id, &world.agent_id, world.hosted("ada"))
        .await
        .unwrap();

    assert_eq!(world.standing_rule.created(), vec![mailbox.id.clone()]);
    world.settled(&mailbox).await;
}

/// A mailbox that wakes nobody is no mailbox, so the provision undoes
/// itself and the address stays free.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_mailbox_whose_standing_rule_fails_is_not_made(pool: SqlitePool) {
    let world = World::new(pool).await;
    world
        .standing_rule
        .refuse_with(Some("that Agent has no thread with your Person"));

    let refused = world
        .desk
        .provision(&world.workspace_id, &world.agent_id, world.hosted("ada"))
        .await
        .unwrap_err();

    assert!(matches!(refused, MailboxError::StandingRule(_)));
    assert!(!world.desk.address_taken("ada@example.com").await.unwrap());
    assert!(
        world.host.addresses().is_empty(),
        "the host mailbox is gone"
    );
    assert!(
        world
            .secrets
            .get(&mailbox_password_secret_name(
                &world.workspace_id,
                "ada@example.com"
            ))
            .unwrap()
            .is_none(),
        "the password is forgotten with the mailbox"
    );
    assert!(
        world
            .desk
            .for_agent(&world.workspace_id, &world.agent_id)
            .await
            .unwrap()
            .is_none(),
        "the Agent may try again"
    );
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn the_desk_opens_an_imap_mailbox_with_its_cursor(pool: SqlitePool) {
    let world = World::new(pool).await;
    let record = world.provision_active(&world.agent_id, "ada").await;
    let id = world
        .transport
        .deliver("bob@example.test", "Invoice", "text");
    let mailboxes: Arc<dyn pagis_mail::Mailboxes> = world.desk.clone();
    let mut mailbox = mailboxes.open(&record).await.unwrap();
    assert_eq!(mailbox.message(&id).await.unwrap().summary.id, id);
    world.transport.renumber("INBOX");
    assert_eq!(
        mailbox.message(&id).await.unwrap_err().0,
        TransportErrorCode::StaleId
    );
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_mailbox_without_endpoints_has_no_password(pool: SqlitePool) {
    let world = World::with_endpoints(pool, false).await;
    let mailbox = world
        .desk
        .provision(&world.workspace_id, &world.agent_id, world.hosted("ada"))
        .await
        .unwrap();
    assert_eq!(mailbox.state, AgentMailboxState::Active);
    assert_eq!(world.host.addresses(), ["ada@example.com"]);
    assert_eq!(world.host.password_of(&mailbox.address), None);
    assert_eq!(
        world
            .secrets
            .get(&mailbox_password_secret_name(
                &world.workspace_id,
                &mailbox.address
            ))
            .unwrap(),
        None
    );
    assert!(world.transport.logins().is_empty());
}

#[sqlx::test(migrations = "./../pagis-storage-sqlite/migrations")]
async fn a_mailbox_without_endpoints_refuses_password_reset(pool: SqlitePool) {
    let world = World::with_endpoints(pool, false).await;
    let mailbox = world
        .desk
        .provision(&world.workspace_id, &world.agent_id, world.hosted("ada"))
        .await
        .unwrap();
    let calls = world.host.calls();
    for pasted in [None, Some("typed-secret".into())] {
        let error = world
            .desk
            .reset_password(&world.workspace_id, &mailbox.id, pasted)
            .await
            .unwrap_err();
        let MailboxError::Host(error) = error else {
            panic!("expected a HostError")
        };
        assert_eq!(error.code(), pagis_mail::HostErrorCode::Refused);
        assert_eq!(error.message(), Some("this mailbox has no login"));
    }
    assert_eq!(world.host.calls(), calls);
    assert_eq!(
        world
            .desk
            .for_agent(&world.workspace_id, &world.agent_id)
            .await
            .unwrap(),
        Some(mailbox.clone())
    );
    assert_eq!(
        world
            .secrets
            .get(&mailbox_password_secret_name(
                &world.workspace_id,
                &mailbox.address
            ))
            .unwrap(),
        None
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_host_without_a_key_provisions_lists_and_deletes_with_no_secret_read(pool: SqlitePool) {
    let host = FakeMailboxHost::with_capabilities(HostCapabilities {
        api_key: false,
        outgoing_cap: true,
        delete_mailbox: true,
        reset_password: true,
    });
    let world = World::with_host(pool, false, host).await;
    world
        .secrets
        .delete(&host_api_key_secret_name(MIGADU_ALIAS))
        .unwrap();

    let mailbox = world
        .desk
        .provision(&world.workspace_id, &world.agent_id, world.hosted("ada"))
        .await
        .expect("a host with no key creates the mailbox");
    assert_eq!(mailbox.state, AgentMailboxState::Active);
    assert_eq!(
        world.desk.list(&world.workspace_id).await.unwrap(),
        vec![mailbox.clone()]
    );
    let found = world
        .host
        .list(&pagis_mail::HostAccount::new("example.com", "", None))
        .await
        .unwrap();
    assert_eq!(found[0].address, mailbox.address);
    let refused = world
        .desk
        .reset_password(&world.workspace_id, &mailbox.id, None)
        .await
        .unwrap_err();
    assert!(
        matches!(refused, MailboxError::Host(error) if error.message() == Some("this mailbox has no login"))
    );
    world
        .desk
        .delete(&world.workspace_id, &mailbox.id, &mailbox.address)
        .await
        .unwrap();
    assert!(world.host.addresses().is_empty());
    assert!(world.secrets.reads.lock().unwrap().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_host_without_a_key_can_reset_a_mailbox_with_a_login(pool: SqlitePool) {
    let host = FakeMailboxHost::with_capabilities(HostCapabilities {
        api_key: false,
        outgoing_cap: true,
        delete_mailbox: true,
        reset_password: true,
    });
    let world = World::with_host(pool, true, host).await;
    world
        .secrets
        .delete(&host_api_key_secret_name(MIGADU_ALIAS))
        .unwrap();
    let mailbox = world.provision_active(&world.agent_id, "ada").await;
    let previous = world.host.password_of(&mailbox.address).unwrap();
    let reset = world
        .desk
        .reset_password(&world.workspace_id, &mailbox.id, None)
        .await
        .unwrap();
    let active = world.settled(&reset).await;
    assert_eq!(active.state, AgentMailboxState::Active);
    assert_eq!(active.cursor, mailbox.cursor);
    assert_ne!(world.host.password_of(&mailbox.address).unwrap(), previous);
    assert!(
        world
            .host
            .calls()
            .contains(&HostCall::ResetPassword(mailbox.address.clone()))
    );
    world
        .desk
        .delete(&world.workspace_id, &mailbox.id, &mailbox.address)
        .await
        .unwrap();
    assert!(world.host.addresses().is_empty());
    assert!(
        world
            .secrets
            .reads
            .lock()
            .unwrap()
            .iter()
            .all(|name| name != &host_api_key_secret_name(MIGADU_ALIAS))
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_host_that_needs_a_key_is_unavailable_without_its_secret(pool: SqlitePool) {
    let world = World::new(pool).await;
    world
        .secrets
        .delete(&host_api_key_secret_name(MIGADU_ALIAS))
        .unwrap();
    let result = world
        .desk
        .provision(&world.workspace_id, &world.agent_id, world.hosted("ada"))
        .await;
    assert!(
        matches!(result, Err(MailboxError::ProviderUnavailable(status)) if status == Connection::UNAVAILABLE)
    );
    assert_eq!(
        *world.secrets.reads.lock().unwrap(),
        vec![host_api_key_secret_name(MIGADU_ALIAS)]
    );
    assert!(world.host.calls().is_empty());
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn workspace_mailbox_provision_uses_the_host_and_secret_without_a_standing_rule(
    pool: SqlitePool,
) {
    let world = World::new(pool).await;
    let mailbox = world
        .desk
        .provision_workspace(&world.workspace_id, world.hosted("person"))
        .await
        .unwrap();
    assert_eq!(mailbox.address, "person@example.com");
    assert!(
        world
            .secrets
            .get(&mailbox_password_secret_name(
                &world.workspace_id,
                &mailbox.address
            ))
            .unwrap()
            .is_some()
    );
    assert!(world.standing_rule.created().is_empty());
    assert!(matches!(
        world
            .desk
            .provision_workspace(&world.workspace_id, world.hosted("other"))
            .await,
        Err(MailboxError::Conflict(_))
    ));
    assert_eq!(world.host.addresses(), ["person@example.com"]);
    assert!(
        world
            .mailboxes
            .address_taken("person@example.com")
            .await
            .unwrap()
    );
    assert_eq!(
        world
            .desk
            .connection_in_use(&world.workspace_id, &world.migadu_id)
            .await
            .unwrap(),
        1
    );
    let mut store = world
        .desk
        .open_workspace(&world.workspace_id)
        .await
        .unwrap();
    assert!(store.folders().await.unwrap().contains(&"INBOX".into()));
    store.close().await.unwrap();
    assert_eq!(world.transport.logins(), ["person@example.com"]);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn workspace_mailbox_host_refusal_releases_both_reservations(pool: SqlitePool) {
    let world = World::new(pool).await;
    world
        .host
        .fail_with(Some(pagis_mail::HostErrorCode::CapReached));
    assert!(matches!(
        world
            .desk
            .provision_workspace(&world.workspace_id, world.hosted("person"))
            .await,
        Err(MailboxError::Host(_))
    ));
    assert!(
        world
            .desk
            .workspace_mailbox(&world.workspace_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !world
            .mailboxes
            .address_taken("person@example.com")
            .await
            .unwrap()
    );
    world.host.fail_with(None);
    world
        .desk
        .provision_workspace(&world.workspace_id, world.hosted("person"))
        .await
        .unwrap();
}
