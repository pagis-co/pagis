use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::*;
use pagis_mail::MailboxHost;
use pagis_testkit::fixture;

struct Connections {
    inner: Arc<dyn ConnectionStore>,
    failed: WorkspaceId,
    reads: Mutex<Vec<WorkspaceId>>,
    hold: AtomicBool,
    release: tokio::sync::Notify,
}

#[async_trait]
impl ConnectionStore for Connections {
    async fn create(&self, row: &Connection) -> Result<(), StoreError> {
        self.inner.create(row).await
    }
    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Connection>, StoreError> {
        self.reads.lock().unwrap().push(ws.clone());
        if ws == &self.failed && self.hold.load(Ordering::SeqCst) {
            self.release.notified().await;
        }
        if ws == &self.failed {
            return Err(StoreError::Corrupt("Workspace store failed".into()));
        }
        self.inner.list(ws).await
    }
    async fn get(
        &self,
        ws: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Option<Connection>, StoreError> {
        self.reads.lock().unwrap().push(ws.clone());
        if ws == &self.failed {
            return Err(StoreError::Corrupt("Workspace store failed".into()));
        }
        self.inner.get(ws, id).await
    }
    async fn set_status(
        &self,
        ws: &WorkspaceId,
        id: &ConnectionId,
        state: &str,
    ) -> Result<bool, StoreError> {
        self.inner.set_status(ws, id, state).await
    }
    async fn set_authorization(
        &self,
        ws: &WorkspaceId,
        id: &ConnectionId,
        caps: &[String],
    ) -> Result<bool, StoreError> {
        self.inner.set_authorization(ws, id, caps).await
    }
    async fn set_config(
        &self,
        ws: &WorkspaceId,
        id: &ConnectionId,
        config: &serde_json::Value,
    ) -> Result<bool, StoreError> {
        self.inner.set_config(ws, id, config).await
    }
    async fn delete_and_revoke(
        &self,
        ws: &WorkspaceId,
        id: &ConnectionId,
        at: i64,
    ) -> Result<bool, StoreError> {
        self.inner.delete_and_revoke(ws, id, at).await
    }
    async fn set_refresh_token(
        &self,
        ws: &WorkspaceId,
        id: &ConnectionId,
        token: Option<&SealedSecret>,
    ) -> Result<bool, StoreError> {
        self.inner.set_refresh_token(ws, id, token).await
    }
    async fn refresh_token(
        &self,
        ws: &WorkspaceId,
        id: &ConnectionId,
    ) -> Result<Option<SealedSecret>, StoreError> {
        self.inner.refresh_token(ws, id).await
    }
}
struct Catalog;
impl EventCatalog for Catalog {
    fn declaration(&self, _: &WorkspaceId, name: &str, provider: &str) -> Option<EventDeclaration> {
        Some(EventDeclaration {
            name: name.into(),
            provider: provider.into(),
            source_version: "1".into(),
            metadata_schema: serde_json::json!({}),
            filter_schema: serde_json::json!({}),
            required_capability: "".into(),
            matcher: "mail".into(),
            source_resource: None,
        })
    }
}

struct Gog;
#[async_trait]
impl pagis_google::GogRunner for Gog {
    async fn run(
        &self,
        _: &pagis_google::GogCommand,
    ) -> Result<pagis_google::ProcessOutput, pagis_google::ProcessFailure> {
        panic!("no provider call is due")
    }
}

struct World {
    _home: tempfile::TempDir,
    stores: Stores,
    connections: Arc<Connections>,
    worker: Arc<pagis::knowledge::Worker>,
    sweeps: Arc<pagis::sweeps::Sweeps>,
    workspaces: [WorkspaceId; 2],
    rules: [EventSubscription; 2],
    mail_transport: Arc<pagis_mail::fake::FakeMailTransport>,
    mail_ingest: Arc<pagis_mail::fake::FakeMailIngest>,
    secrets: Arc<dyn SecretStore>,
}
impl World {
    async fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let pool = pagis_storage_sqlite::connect(&home.path().join("db.sqlite"))
            .await
            .unwrap();
        pagis_storage_sqlite::MIGRATOR.run(&pool).await.unwrap();
        let stores = pagis_storage_sqlite::stores(pool.clone());
        let first = fixture::seeded_workspace(&pool).await;
        stores.workspaces.create(&first).await.unwrap();
        let second = Workspace {
            legal_hold: None,
            monthly_spend_cap_usd: None,
            user_id: first.user_id.clone(),
            ..fixture::workspace()
        };
        // Each Workspace belongs to a separate Person.
        let org = stores.orgs.list().await.unwrap().remove(0);
        let person = User::new(org.id, UserRole::Member, now_ms());
        stores.users.create(&person).await.unwrap();
        let second = Workspace {
            legal_hold: None,
            monthly_spend_cap_usd: None,
            user_id: Some(person.id),
            ..second
        };
        stores.workspaces.create(&second).await.unwrap();
        let mut rules = Vec::new();
        for ws in [&first.id, &second.id] {
            let agent = fixture::agent(ws);
            let channel = fixture::channel(ws);
            stores.agents.create(&agent).await.unwrap();
            stores.channels.create(&channel).await.unwrap();
            let connection = Connection {
                id: ConnectionId::generate(),
                workspace_id: ws.clone(),
                provider: "google".into(),
                alias: "mail".into(),
                display_name: "Mail".into(),
                status: Connection::CONNECTED.into(),
                authorized_capabilities: vec![],
                config: serde_json::json!({}),
                created_at: 0,
            };
            stores.connections.create(&connection).await.unwrap();
            let rule = EventSubscription {
                created_by: pagis_core::Author::System,
                id: EventSubscriptionId::generate(),
                workspace_id: ws.clone(),
                agent_id: agent.id,
                source: EventSource::connection(connection.id.clone()),
                event_kind: pagis_broker::MAIL_MESSAGE_RECEIVED.into(),
                source_version: "1".into(),
                name: "Mail".into(),
                instruction: "Read mail".into(),
                channel_id: channel.id,
                root_message_id: None,
                filter: serde_json::json!({}),
                state: EventSubscriptionState::Active,
                revision: 1,
                approved_revision: Some(1),
                watermark_at: None,
                blocked_reason: None,
                failures: 0,
                next_attempt_at: 0,
                created_at: 0,
                updated_at: 0,
                archived_at: None,
            };
            stores.subscriptions.create(&rule).await.unwrap();
            sqlx::query("INSERT INTO knowledge_invalidations(workspace_id,connection_id,resource,reason) VALUES(?,?,?,'source_reset')").bind(ws.as_str()).bind(connection.id.as_str()).bind("mail").execute(&pool).await.unwrap();
            rules.push(rule);
        }
        let connections = Arc::new(Connections {
            inner: stores.connections.clone(),
            failed: first.id.clone(),
            reads: Mutex::new(Vec::new()),
            hold: AtomicBool::new(false),
            release: tokio::sync::Notify::new(),
        });
        let bus: Arc<dyn EventBus> =
            Arc::new(pagis_audit::AuditEventBus::new(stores.events.clone()));
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        let keys = Arc::new(TenantKeys::new(secrets.clone()));
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let google = Arc::new(pagis::connections::GoogleBinder::new(
            Arc::new(Gog),
            home.path(),
            Arc::new(pagis_connect::GoogleBroker::new(
                pagis_connect::GoogleBrokerDeps {
                    connections: connections.clone(),
                    client: Arc::new(pagis_connect::OrgWebClient::new(
                        stores.orgs.clone(),
                        secrets.clone(),
                    )),
                    oauth: Arc::new(pagis_google::GoogleOAuth::new()),
                    keys: keys.clone(),
                    sessions: stores.sessions.clone(),
                    members: stores.workspace_members.clone(),
                    clock: clock.clone(),
                    public_origin: "http://localhost".to_string(),
                },
            )),
        ));
        let trigger = Arc::new(pagis_trigger::Trigger::new(pagis_trigger::TriggerDeps {
            schedules: stores.schedules.clone(),
            store: stores.triggers.clone(),
            subscriptions: stores.subscriptions.clone(),
            connections: connections.clone(),
            agents: stores.agents.clone(),
            channels: stores.channels.clone(),
            messages: stores.messages.clone(),
            org_workspace_id: WorkspaceId::generate(),
            grants: stores.grants.clone(),
            catalog: Arc::new(Catalog),
            matchers: HashMap::new(),
            events: bus.clone(),
            forget_keys: keys.clone(),
        }));
        let trust: Arc<dyn pagis_mail::SenderTrust> = Arc::new(pagis_mail::ListedSenders::new(
            stores.trust_list.clone(),
            connections.clone(),
        ));
        let worker = Arc::new(pagis::knowledge::Worker {
            bus: bus.clone(),
            store: stores.knowledge.clone(),
            forget_keys: keys,
            connections: connections.clone(),
            connection_state: Arc::new(pagis::connections::ConnectionState::new(
                connections.clone(),
                bus.clone(),
            )),
            grants: stores.grants.clone(),
            agents: stores.agents.clone(),
            google: google.clone(),
            trigger: trigger.clone(),
            memory: Arc::new(pagis_memory_git::GitMemoryStore::new(
                home.path().join("memory"),
                stores.forget.clone(),
                stores.memory_pages.clone(),
            )),
            clock,
            trust: trust.clone(),
        });
        let mail_transport = Arc::new(pagis_mail::fake::FakeMailTransport::with_idle(false));
        let mail_ingest = Arc::new(pagis_mail::fake::FakeMailIngest::default());
        let desk = Arc::new(pagis_mail::MailboxDesk::new(pagis_mail::MailboxDeskDeps {
            intake: Arc::new(pagis_mail::MailIntake {
                workspaces: stores.workspaces.clone(),
                agents: stores.agents.clone(),
                source: Arc::new(pagis_mail::HostSpamVerdicts),
                records: stores.mail_assessments.clone(),
                trust: Arc::new(pagis_mail::ListedSenders::new(
                    stores.trust_list.clone(),
                    stores.connections.clone(),
                )),
            }),
            workspaces: stores.workspaces.clone(),
            workspace_mailboxes: stores.workspace_mailboxes.clone(),
            mailboxes: stores.agent_mailboxes.clone(),
            connections: connections.clone(),
            org_workspace_id: org.workspace_id.clone(),
            agents: stores.agents.clone(),
            host: Arc::new(pagis_mail::fake::FakeMailboxHost::default()),
            transport: mail_transport.clone(),
            secrets: secrets.clone(),
            bus: bus.clone(),
            standing_rule: Arc::new(pagis_mail::fake::FakeStandingRule::default()),
            proof: pagis_mail::ProofTiming::default(),
        }));
        let mail = Arc::new(pagis_mail::MailCollector::new(
            pagis_mail::MailCollectorDeps {
                desk,
                sent: stores.sent_mail.clone(),
                ingest: mail_ingest.clone(),
                idle_turn: Duration::from_secs(60),
            },
        ));
        let sweeps = Arc::new(pagis::sweeps::Sweeps {
            workspaces: stores.workspaces.clone(),
            bus,
            mail,
            knowledge: worker.clone(),
            trigger,
            connections: connections.clone(),
            google,
            collector_interval: Duration::from_secs(60),
        });
        Self {
            _home: home,
            stores,
            connections,
            worker,
            sweeps,
            workspaces: [first.id, second.id],
            rules: rules.try_into().unwrap(),
            mail_transport,
            mail_ingest,
            secrets,
        }
    }
}

#[tokio::test]
async fn per_workspace_functions_do_not_read_or_change_another_workspace() {
    let mut world = World::new().await;
    for index in 0..2 {
        world.rules[index].state = EventSubscriptionState::Blocked;
        world.rules[index].blocked_reason = Some(BlockReason::ReauthRequired);
        world
            .stores
            .subscriptions
            .update(&world.rules[index])
            .await
            .unwrap();
        world
            .stores
            .knowledge
            .configure(
                pagis_core::knowledge::SyncConfig {
                    workspace_id: world.workspaces[index].clone(),
                    connection_id: world.rules[index].source.connection_id().unwrap().clone(),
                    resource: pagis_broker::MAIL_SYNC_RESOURCE.into(),
                    agent_id: world.rules[index].agent_id.clone(),
                    required_capability: "gmail_read".into(),
                    enabled: true,
                    since: 0,
                    filter: pagis_google::gmail_filter::default_filter(),
                },
                0,
            )
            .await
            .unwrap();
    }
    let ws = &world.workspaces[1];
    world.worker.publish_workspace_changes(ws).await.unwrap();
    assert_eq!(
        world
            .stores
            .knowledge
            .pending_invalidations(&world.workspaces[0], 100)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        world
            .stores
            .knowledge
            .pending_invalidations(ws, 100)
            .await
            .unwrap()
            .is_empty()
    );
    world
        .worker
        .collect_workspace_pass(ws, &mut HashSet::new())
        .await
        .unwrap();
    pagis::collectors::unblock_connected(&world.sweeps.trigger, world.connections.as_ref(), ws)
        .await
        .unwrap();
    assert_eq!(
        world
            .stores
            .subscriptions
            .get(ws, &world.rules[1].id)
            .await
            .unwrap()
            .unwrap()
            .state,
        EventSubscriptionState::Active
    );
    for index in 0..2 {
        let status = world
            .stores
            .knowledge
            .status(
                &pagis_core::knowledge::SourceKey {
                    workspace_id: world.workspaces[index].clone(),
                    connection_id: world.rules[index].source.connection_id().unwrap().clone(),
                    resource: pagis_broker::MAIL_SYNC_RESOURCE.into(),
                },
                now_ms(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.acquisition_error.is_some(), index == 1);
    }
    pagis::collectors::collect_workspace_targets(
        &world.sweeps.trigger,
        world.connections.as_ref(),
        &world.sweeps.google,
        ws,
        &HashSet::new(),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(
        world
            .connections
            .reads
            .lock()
            .unwrap()
            .iter()
            .all(|read| read == ws)
    );
    assert_eq!(
        world
            .stores
            .subscriptions
            .get(&world.workspaces[0], &world.rules[0].id)
            .await
            .unwrap()
            .unwrap(),
        world.rules[0]
    );
}

#[tokio::test]
async fn a_workspace_store_failure_does_not_stop_the_other_workspace() {
    let world = World::new().await;
    world.sweeps.pass().await.unwrap();
    assert!(
        world
            .connections
            .reads
            .lock()
            .unwrap()
            .contains(&world.workspaces[0])
    );
    assert!(
        world
            .connections
            .reads
            .lock()
            .unwrap()
            .contains(&world.workspaces[1])
    );
    assert!(
        world
            .stores
            .knowledge
            .pending_invalidations(&world.workspaces[1], 100)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_failing_target_keeps_its_backoff_after_a_collector_restart() {
    let world = World::new().await;
    let ws = &world.workspaces[0];
    pagis::collectors::collect_workspace_targets(
        &world.sweeps.trigger,
        world.connections.as_ref(),
        &world.sweeps.google,
        ws,
        &HashSet::new(),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    let stored = world
        .stores
        .subscriptions
        .get(ws, &world.rules[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.failures, 1);
    assert!(stored.next_attempt_at > now_ms());
    world.connections.reads.lock().unwrap().clear();
    // A new store handle reads the same record after the collector stops.
    let restarted = Arc::new(pagis_storage_sqlite::SqliteEventSubscriptionStore::new(
        pagis_storage_sqlite::connect(&world._home.path().join("db.sqlite"))
            .await
            .unwrap(),
    ));
    assert_eq!(
        restarted.collector_targets(ws).await.unwrap()[0].next_attempt_at,
        stored.next_attempt_at
    );
    let trigger = pagis_trigger::Trigger::new(pagis_trigger::TriggerDeps {
        schedules: world.stores.schedules.clone(),
        store: world.stores.triggers.clone(),
        subscriptions: restarted,
        connections: world.connections.clone(),
        agents: world.stores.agents.clone(),
        channels: world.stores.channels.clone(),
        messages: world.stores.messages.clone(),
        org_workspace_id: WorkspaceId::generate(),
        grants: world.stores.grants.clone(),
        catalog: Arc::new(Catalog),
        matchers: HashMap::new(),
        events: world.worker.bus.clone(),
        forget_keys: world.worker.forget_keys.clone(),
    });
    pagis::collectors::collect_workspace_targets(
        &trigger,
        world.connections.as_ref(),
        &world.sweeps.google,
        ws,
        &HashSet::new(),
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    assert!(
        world.connections.reads.lock().unwrap().is_empty(),
        "a restart does not poll the failing Connection at once"
    );
}

#[tokio::test]
async fn catchup_does_not_start_an_unbounded_chain_of_sweeps() {
    let world = World::new().await;
    let blocked = EventSubscription {
        state: EventSubscriptionState::Blocked,
        blocked_reason: Some(BlockReason::ReauthRequired),
        ..world.rules[1].clone()
    };
    world.stores.subscriptions.update(&blocked).await.unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = world.sweeps.spawn(cancel.clone()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();
    task.await.unwrap();
    let reads = world.connections.reads.lock().unwrap();
    let count = reads
        .iter()
        .filter(|ws| *ws == &world.workspaces[1])
        .count();
    assert!(count > 0, "catch-up runs at startup");
    assert!(
        count <= 10,
        "the collector's own state events do not start a retry loop: {count} reads"
    );
}

#[tokio::test]
async fn a_slow_workspace_does_not_delay_the_others_or_change_publication() {
    let world = World::new().await;
    world.connections.hold.store(true, Ordering::SeqCst);
    let sweeps = world.sweeps.clone();
    let pass = tokio::spawn(async move { sweeps.pass().await });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let healthy = world
                .stores
                .subscriptions
                .get(&world.workspaces[1], &world.rules[1].id)
                .await
                .unwrap()
                .unwrap();
            let changes = world
                .stores
                .knowledge
                .pending_invalidations(&world.workspaces[1], 100)
                .await
                .unwrap();
            if healthy.state == EventSubscriptionState::Blocked && changes.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the healthy Workspace completes while the other store waits");
    assert!(!pass.is_finished());
    world.connections.hold.store(false, Ordering::SeqCst);
    world.connections.release.notify_one();
    pass.await.unwrap().unwrap();
}

#[tokio::test]
async fn an_event_sweeps_only_its_workspace_before_the_next_interval() {
    let world = World::new().await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = world.sweeps.spawn(cancel.clone()).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let failed = world
                .stores
                .subscriptions
                .get(&world.workspaces[0], &world.rules[0].id)
                .await
                .unwrap()
                .unwrap();
            let healthy = world
                .stores
                .subscriptions
                .get(&world.workspaces[1], &world.rules[1].id)
                .await
                .unwrap()
                .unwrap();
            if failed.failures == 1 && healthy.state == EventSubscriptionState::Blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("startup work completes");
    world.connections.reads.lock().unwrap().clear();
    world
        .worker
        .bus
        .publish(NewEvent {
            workspace_id: world.workspaces[1].clone(),
            event_type: "sync.changed".into(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({}),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while world.connections.reads.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the Event Bus wakes this Workspace before the 60-second interval");
    cancel.cancel();
    task.await.unwrap();
    assert!(
        world
            .connections
            .reads
            .lock()
            .unwrap()
            .iter()
            .all(|ws| ws == &world.workspaces[1])
    );
}

#[tokio::test]
async fn an_installation_connection_event_does_not_sweep_the_orgs_workspace() {
    let world = World::new().await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = world.sweeps.spawn(cancel.clone()).await;
    let org = world.stores.orgs.list().await.unwrap().remove(0);
    world
        .worker
        .bus
        .publish(NewEvent {
            workspace_id: org.workspace_id.clone(),
            event_type: "connection.changed".into(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({"status": "connected"}),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();
    task.await.unwrap();
    assert!(
        !world
            .connections
            .reads
            .lock()
            .unwrap()
            .contains(&org.workspace_id)
    );
}

#[tokio::test]
async fn mail_poll_cycles_run_for_each_workspace_every_five_minutes() {
    let world = World::new().await;
    let org = world.stores.orgs.list().await.unwrap().remove(0);
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: org.workspace_id,
        provider: pagis_mail::MANUAL_PROVIDER.into(),
        alias: "own_mail".into(),
        display_name: "Own mail".into(),
        status: Connection::CONNECTED.into(),
        authorized_capabilities: vec![],
        created_at: now_ms(),
        config: pagis_mail::MailboxProvider {
            account: None,
            domain: "example.test".into(),
            imap: Some(pagis_mail::Endpoint::new("localhost", 143)),
            smtp: Some(pagis_mail::Endpoint::new("localhost", 25)),
            capabilities: pagis_mail::MailboxCapabilities::of(
                pagis_mail::ManualHost::new().capabilities(),
                true,
            ),
        }
        .config(),
    };
    world.stores.connections.create(&connection).await.unwrap();
    for (index, ws) in world.workspaces.iter().enumerate() {
        let address = format!("agent{index}@example.test");
        world
            .secrets
            .set(
                &pagis_mail::mailbox_password_secret_name(ws, &address),
                "password",
            )
            .unwrap();
        world
            .stores
            .agent_mailboxes
            .create(&AgentMailbox {
                id: AgentMailboxId::generate(),
                workspace_id: ws.clone(),
                agent_id: world.rules[index].agent_id.clone(),
                connection_id: connection.id.clone(),
                address,
                state: AgentMailboxState::Active,
                reason: None,
                outgoing_cap: 20,
                allow_rules: vec![],
                cursor: Some(MailboxCursor {
                    folder: "INBOX".into(),
                    uid_validity: 1,
                    last_uid: 0,
                }),
                sends_day: None,
                sends_today: 0,
                created_at: now_ms(),
                deleted_at: None,
            })
            .await
            .unwrap();
        // Discover that the server has no IDLE before the timer starts.
        world.sweeps.mail.sweep_workspace(ws).await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            for ws in &world.workspaces {
                world.sweeps.mail.sweep_workspace(ws).await.unwrap();
            }
            if world.sweeps.mail.watched() == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    world
        .mail_transport
        .deliver("care@example.test", "First", "");
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = world.sweeps.spawn(cancel.clone()).await;
    // Real time bounds this assertion, so a missing timer fails promptly.
    let events = world.mail_ingest.wait_for(2, Duration::from_secs(2)).await;
    assert_eq!(events.len(), 2, "the startup poll reads each Workspace");
    tokio::time::timeout(Duration::from_secs(2), async {
        while world.mail_transport.open_sessions() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    world
        .mail_transport
        .deliver("care@example.test", "Next", "");
    tokio::time::pause();
    tokio::time::advance(pagis_mail::POLL_INTERVAL - Duration::from_secs(1)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(world.mail_ingest.events().len(), 2);
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::time::resume();
    let events = world.mail_ingest.wait_for(4, Duration::from_secs(2)).await;
    assert_eq!(events.len(), 4, "the next cycle reads both Workspaces");
    assert!(
        world
            .mail_ingest
            .passes()
            .iter()
            .any(|pass| pass.workspace_id == world.workspaces[0])
    );
    assert!(
        world
            .mail_ingest
            .passes()
            .iter()
            .any(|pass| pass.workspace_id == world.workspaces[1])
    );
    cancel.cancel();
    task.await.unwrap();
    assert_eq!(world.mail_transport.open_sessions(), 0);
}
