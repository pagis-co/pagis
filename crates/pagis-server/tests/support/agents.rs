//! The stores and services an Embedder gives to the routers.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use pagis_agent::{
    AgentDeps, AgentLoopConfig, AgentSystem, CoreToolRuntime, ProgressHub, StreamHub,
    ToolRuntimeDeps,
};
use pagis_broker::{
    AuthorizedCall, Broker, BrokerDeps, HostPresence, NoCredentialDirectory, NoSessionStarts,
    ToolExecutor, ToolResult,
};
use pagis_core::fake::{FakeCatalogueProvider, FakeDatabase, FakeMemoryStore, MemorySecretStore};
use pagis_core::*;
use pagis_server::{AppState, Routers};

pub struct TestClock(std::sync::atomic::AtomicI64);
impl TestClock {
    #[allow(dead_code)]
    pub fn set(&self, at: i64) {
        self.0.store(at, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        match self.0.load(std::sync::atomic::Ordering::SeqCst) {
            i64::MIN => now_ms(),
            at => at,
        }
    }
}

pub struct Embedder {
    #[allow(dead_code)]
    pub clock: Arc<TestClock>,
    pub stores: Stores,
    pub routers: Routers,
    pub workspace: WorkspaceId,
    pub other_workspace: WorkspaceId,
    pub cookie: String,
    pub other_cookie: String,
    #[allow(dead_code)]
    pub mail_transport: Arc<pagis_mail::fake::FakeMailTransport>,
    #[allow(dead_code)]
    pub mailbox_host: Arc<pagis_mail::fake::FakeMailboxHost>,
    #[allow(dead_code)]
    pub mail: Arc<pagis_mail::fake::MemoryMailbox>,
    #[allow(dead_code)]
    pub plugin_states: Arc<pagis_plugin::fake::MemoryPluginStates>,
}

impl Embedder {
    #[allow(dead_code)]
    pub async fn new() -> Self {
        let services = Arc::new(UnusedServices);
        Self::with_model(services.clone(), services.clone(), services).await
    }

    #[allow(dead_code)]
    pub async fn with_model(
        brain: Arc<dyn pagis_agent::Brain>,
        executor: Arc<dyn ToolExecutor>,
        gate: Arc<dyn pagis_agent::ModelCallGate>,
    ) -> Self {
        Self::with_services(
            brain,
            executor,
            gate,
            Arc::new(pagis_plugin::fake::FixedPackageSource::new(
                Default::default(),
            )),
            Arc::new(pagis_plugins::NoStdioServers),
            Arc::new(pagis_plugins::ReqwestMcpHttp::default()),
            Arc::new(pagis_server::NoOrgMenu),
            Default::default(),
            Arc::new(pagis_server::NoPersonNotice),
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn with_plugins(
        source: Arc<dyn pagis_plugin::PackageSource>,
        stdio: Arc<dyn pagis_plugins::StdioServers>,
        http: Arc<dyn pagis_plugins::McpHttp>,
    ) -> Self {
        let services = Arc::new(UnusedServices);
        Self::with_services(
            services.clone(),
            services.clone(),
            services,
            source,
            stdio,
            http,
            Arc::new(pagis_server::NoOrgMenu),
            Default::default(),
            Arc::new(pagis_server::NoPersonNotice),
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn with_org_menu(org_menu: Arc<dyn pagis_server::OrgMenu>) -> Self {
        let services = Arc::new(UnusedServices);
        Self::with_services(
            services.clone(),
            services.clone(),
            services,
            Arc::new(pagis_plugin::fake::FixedPackageSource::new(
                Default::default(),
            )),
            Arc::new(pagis_plugins::NoStdioServers),
            Arc::new(pagis_plugins::ReqwestMcpHttp::default()),
            org_menu,
            Default::default(),
            Arc::new(pagis_server::NoPersonNotice),
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn with_administration_sections(
        sections: pagis_server::AdministrationSections,
    ) -> Self {
        let services = Arc::new(UnusedServices);
        Self::with_services(
            services.clone(),
            services.clone(),
            services,
            Arc::new(pagis_plugin::fake::FixedPackageSource::new(
                Default::default(),
            )),
            Arc::new(pagis_plugins::NoStdioServers),
            Arc::new(pagis_plugins::ReqwestMcpHttp::default()),
            Arc::new(pagis_server::NoOrgMenu),
            sections,
            Arc::new(pagis_server::NoPersonNotice),
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn with_person_notice(notice: Arc<dyn pagis_server::PersonNotice>) -> Self {
        let services = Arc::new(UnusedServices);
        Self::with_services(
            services.clone(),
            services.clone(),
            services,
            Arc::new(pagis_plugin::fake::FixedPackageSource::new(
                Default::default(),
            )),
            Arc::new(pagis_plugins::NoStdioServers),
            Arc::new(pagis_plugins::ReqwestMcpHttp::default()),
            Arc::new(pagis_server::NoOrgMenu),
            Default::default(),
            notice,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn with_services(
        brain: Arc<dyn pagis_agent::Brain>,
        executor: Arc<dyn ToolExecutor>,
        gate: Arc<dyn pagis_agent::ModelCallGate>,
        source: Arc<dyn pagis_plugin::PackageSource>,
        stdio: Arc<dyn pagis_plugins::StdioServers>,
        http: Arc<dyn pagis_plugins::McpHttp>,
        org_menu: Arc<dyn pagis_server::OrgMenu>,
        administration_sections: pagis_server::AdministrationSections,
        person_notice: Arc<dyn pagis_server::PersonNotice>,
    ) -> Self {
        Self::build(
            brain,
            executor,
            gate,
            source,
            stdio,
            http,
            org_menu,
            administration_sections,
            person_notice,
            None,
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn with_workspace_mail(host: Arc<dyn pagis_mail::WorkspaceMailHost>) -> Self {
        let services = Arc::new(UnusedServices);
        Self::build(
            services.clone(),
            services.clone(),
            services,
            Arc::new(pagis_plugin::fake::FixedPackageSource::new(
                Default::default(),
            )),
            Arc::new(pagis_plugins::NoStdioServers),
            Arc::new(pagis_plugins::ReqwestMcpHttp::default()),
            Arc::new(pagis_server::NoOrgMenu),
            Default::default(),
            Arc::new(pagis_server::NoPersonNotice),
            Some(host),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn build(
        brain: Arc<dyn pagis_agent::Brain>,
        executor: Arc<dyn ToolExecutor>,
        gate: Arc<dyn pagis_agent::ModelCallGate>,
        source: Arc<dyn pagis_plugin::PackageSource>,
        stdio: Arc<dyn pagis_plugins::StdioServers>,
        http: Arc<dyn pagis_plugins::McpHttp>,
        org_menu: Arc<dyn pagis_server::OrgMenu>,
        administration_sections: pagis_server::AdministrationSections,
        person_notice: Arc<dyn pagis_server::PersonNotice>,
        workspace_mail: Option<Arc<dyn pagis_mail::WorkspaceMailHost>>,
    ) -> Self {
        let database = FakeDatabase::default();
        let stores = database.stores();
        let person = seed_org_and_administrator(
            stores.orgs.as_ref(),
            stores.users.as_ref(),
            "Org",
            now_ms(),
        )
        .await
        .unwrap();
        let mut other_person = person.clone();
        other_person.id = UserId::generate();
        stores.users.create(&other_person).await.unwrap();
        let workspace = Self::workspace(&stores, &person).await;
        let other_workspace = Self::workspace(&stores, &other_person).await;
        let cookie = Self::session(&stores, &person, "first-secret").await;
        let other_cookie = Self::session(&stores, &other_person, "second-secret").await;
        let org_workspace_id = stores.orgs.list().await.unwrap().remove(0).workspace_id;
        let memory = Arc::new(FakeMemoryStore::new(stores.forget.clone()));
        let ring = Arc::new(pagis_server::EventRing::new(Default::default()));
        let bus: Arc<dyn EventBus> = Arc::new(pagis_server::RingedBus::new(
            Arc::new(database.bus()),
            ring.clone(),
        ));
        let secrets = Arc::new(MemorySecretStore::default());
        let services = Arc::new(UnusedServices);
        let config = AgentLoopConfig {
            computer: false,
            ..Default::default()
        };
        let presence = Arc::new(HostPresence::default());
        let skills = Arc::new(NoSkills);
        let clock = Arc::new(TestClock(std::sync::atomic::AtomicI64::new(i64::MIN)));
        let runtime = Arc::new(CoreToolRuntime::new(ToolRuntimeDeps {
            workspaces: stores.workspaces.clone(),
            agents: stores.agents.clone(),
            channels: stores.channels.clone(),
            messages: stores.messages.clone(),
            runs: stores.runs.clone(),
            participants: stores.participants.clone(),
            memory: memory.clone(),
            pending_evidence: stores.pending_evidence.clone(),
            conversation_evidence: stores.conversation_evidence.clone(),
            grants: stores.grants.clone(),
            skills: skills.clone(),
            computers: services.clone(),
            bus: bus.clone(),
            clock: clock.clone(),
            context_messages: config.context_messages,
            presence: presence.clone(),
        }));
        let executor = Arc::new(RouteExecutor {
            core: runtime.clone(),
            external: executor,
            plugins: OnceLock::new(),
        });
        let broker = Arc::new(Broker::new(BrokerDeps {
            numbers: stores.phone_numbers.clone(),
            run_journal: stores.run_journal.clone(),
            forget: stores.forget.clone(),
            grants: stores.grants.clone(),
            credentials: Arc::new(NoCredentialDirectory),
            connections: stores.connections.clone(),
            requests: stores.requests.clone(),
            snapshots: stores.capability_snapshots.clone(),
            runs: stores.runs.clone(),
            conversation_evidence: stores.conversation_evidence.clone(),
            bus: bus.clone(),
            executor: executor.clone(),

            mailboxes: stores.agent_mailboxes.clone(),
            hosts: stores.hosts.clone(),
            presence: presence.clone(),
            session_starts: Arc::new(NoSessionStarts),
            computer: false,
        }));
        let keys = Arc::new(ProviderKeys::with_env(
            |_| None,
            HashMap::from([(Provider::OpenAi, "test-key".into())]),
            secrets.clone(),
        ));
        let person_activity = Arc::new(pagis_needs_you::PersonActivity::new());
        let deps = AgentDeps {
            person_activity: person_activity.clone(),
            run_journal: stores.run_journal.clone(),
            forget: stores.forget.clone(),
            workspaces: stores.workspaces.clone(),
            users: stores.users.clone(),
            workspace_members: stores.workspace_members.clone(),
            agents: stores.agents.clone(),
            channels: stores.channels.clone(),
            messages: stores.messages.clone(),
            text_records: stores.text_records.clone(),
            runs: stores.runs.clone(),
            model_aliases: stores.model_aliases.clone(),
            usage: stores.usage.clone(),
            capture: Arc::new(CaptureSetting::default()),
            model_request_captures: stores.model_request_captures.clone(),
            participants: stores.participants.clone(),
            requests: stores.requests.clone(),
            grants: stores.grants.clone(),
            connections: stores.connections.clone(),
            files: stores.files.clone(),
            blobs: Arc::new(object_store::memory::InMemory::new()),
            memory: memory.clone(),
            pending_evidence: stores.pending_evidence.clone(),
            continuations: stores.continuations.clone(),
            briefs: stores.briefs.clone(),
            schedules: stores.schedules.clone(),
            skills,
            computers: services.clone(),
            bus: bus.clone(),
            brain,
            gate,
            models: Arc::new(pagis_agent::ModelCatalog::new(keys.clone())),
            broker: broker.clone(),
            tool_runtime: runtime,
            hub: Arc::new(StreamHub::default()),
            progress: Arc::new(ProgressHub::default()),
            triggers: stores.triggers.clone(),
            clock: clock.clone(),
            config,
        };
        let agents = AgentSystem::start(deps.clone()).await;
        let mail_transport = Arc::new(pagis_mail::fake::FakeMailTransport::default());
        let mailbox_host = Arc::new(pagis_mail::fake::FakeMailboxHost::default());
        let mailbox_desk = Arc::new(pagis_mail::MailboxDesk::new(pagis_mail::MailboxDeskDeps {
            workspace_mail: workspace_mail.unwrap_or_else(|| {
                Arc::new(pagis_mail::ImapWorkspaceMailHost::new(
                    mailbox_host.clone(),
                    mail_transport.clone(),
                    secrets.clone(),
                ))
            }),
            grants: stores.grants.clone(),
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
            connections: stores.connections.clone(),
            org_workspace_id: org_workspace_id.clone(),
            agents: stores.agents.clone(),
            host: mailbox_host.clone(),
            transport: mail_transport.clone(),
            secrets: secrets.clone(),
            bus: bus.clone(),
            standing_rule: Arc::new(pagis_mail::fake::FakeStandingRule::default()),
            proof: Default::default(),
        }));
        let trigger = Arc::new(pagis_trigger::Trigger::new(pagis_trigger::TriggerDeps {
            schedules: stores.schedules.clone(),
            store: stores.triggers.clone(),
            subscriptions: stores.subscriptions.clone(),
            connections: stores.connections.clone(),
            agents: stores.agents.clone(),
            channels: stores.channels.clone(),
            messages: stores.messages.clone(),
            org_workspace_id: org_workspace_id.clone(),
            grants: stores.grants.clone(),
            catalog: Arc::new(pagis_broker::WorkspaceEventCatalog::new(broker.clone())),
            matchers: HashMap::new(),
            events: bus.clone(),
            forget_keys: Arc::new(TenantKeys::new(secrets.clone())),
        }));
        let needs_you = Arc::new(pagis_needs_you::NeedsYou::new(&stores));
        let origin = "http://127.0.0.1:4400".to_string();
        let notifications = Arc::new(pagis_needs_you::Notifications::new(
            services.clone(),
            origin.clone(),
            stores.push_subscriptions.clone(),
            stores.agents.clone(),
            needs_you.clone(),
            deps.clock.clone(),
        ));
        stores
            .model_aliases
            .create(&ModelAlias {
                id: ModelAliasId::generate(),
                workspace_id: workspace.clone(),
                alias: pagis_voice::SPEAK_ALIAS.into(),
                candidates: vec!["openai/gpt-4o-mini-tts".into()],
                created_at: 0,
                updated_at: 0,
            })
            .await
            .unwrap();
        let mail = Arc::new(pagis_mail::fake::MemoryMailbox::default());
        let plugin_states = Arc::new(pagis_plugin::fake::MemoryPluginStates::default());
        let plugin_hosts = pagis_plugins::PluginHosts::new(pagis_plugins::PluginHostsDeps {
            org_workspace_id: org_workspace_id.clone(),
            workspaces: stores.workspaces.clone(),
            plugins: stores.plugins.clone(),
            catalogs: stores.plugin_tools.clone(),
            connections: stores.connections.clone(),
            grants: stores.grants.clone(),
            secrets: secrets.clone(),
            tokens: Arc::new(pagis_plugins::NoConnectionTokens),
            states: plugin_states.clone(),
            manifests: broker.clone(),
            bus: bus.clone(),
            stdio,
            http,
        });
        assert!(
            executor
                .plugins
                .set(pagis_plugins::PluginToolRuntime::new(plugin_hosts.clone()))
                .is_ok()
        );
        let plugins = Arc::new(pagis_plugin::Plugins::new(pagis_plugin::PluginsDeps {
            plugins: stores.plugins.clone(),
            connections: stores.connections.clone(),
            grants: stores.grants.clone(),
            secrets: secrets.clone(),
            states: plugin_states.clone(),
            source,
            namespaces: broker.clone(),
            servers: plugin_hosts.clone(),
            bus: bus.clone(),
        }));
        let state = AppState {
            governance: stores.governance.clone(),
            audit: stores.audit.clone(),
            conversation_evidence: stores.conversation_evidence.clone(),
            workspace_members: stores.workspace_members.clone(),
            plugins,
            plugin_hosts,
            channels: stores.channels.clone(),
            participants: stores.participants.clone(),
            messages: stores.messages.clone(),
            runs: stores.runs.clone(),
            pending_evidence: stores.pending_evidence.clone(),
            model_aliases: stores.model_aliases.clone(),
            schedules: stores.schedules.clone(),
            onboarding: stores.onboarding.clone(),
            retention_policies: stores.retention_policies.clone(),
            connections: stores.connections.clone(),
            credentials: stores.credentials.clone(),
            requests: stores.requests.clone(),
            grants: stores.grants.clone(),
            hosts: stores.hosts.clone(),
            calls: stores.calls.clone(),
            live_call_control: Arc::new(pagis_server::calls::NoLiveCallControl),
            texts: stores.text_records.clone(),
            trust_list: stores.trust_list.clone(),
            calendar: stores.calendar.clone(),
            keypad_failures: stores.keypad_failures.clone(),
            files: stores.files.clone(),
            events: stores.events.clone(),
            knowledge: stores.knowledge.clone(),
            forget: stores.forget.clone(),
            software: stores.software.clone(),
            contributions: stores.contributions.clone(),
            workspaces: stores.workspaces.clone(),
            orgs: stores.orgs.clone(),
            users: stores.users.clone(),
            usage: stores.usage.clone(),
            model_request_captures: stores.model_request_captures.clone(),
            sessions: stores.sessions.clone(),
            push_subscriptions: stores.push_subscriptions.clone(),
            sign_in_links: stores.sign_in_links.clone(),
            agent_store: stores.agents.clone(),
            coding_session_store: stores.coding_sessions.clone(),
            bus,
            trigger,
            vault: Arc::new(pagis_vault::Vault::new(
                stores.credentials.clone(),
                services,
                Arc::new(TenantKeys::new(secrets.clone())),
            )),
            host_presence: presence,
            host_sessions: Arc::new(pagis_broker::HostSessions::default()),
            broker,
            mailbox_desk,
            mailboxes: mail.clone(),
            secrets: secrets.clone(),
            needs_you,
            #[cfg(not(target_arch = "wasm32"))]
            voice: Arc::new(pagis_voice::fake::FakeVoice::default()),
            #[cfg(target_arch = "wasm32")]
            voice: send_wrapper::SendWrapper::new(
                Arc::new(pagis_voice::fake::FakeVoice::default()),
            ),
            blobs: deps.blobs.clone(),
            file_max_bytes: 1024 * 1024,
            memory,
            catalogues: Arc::new(FakeCatalogueProvider::default()),
            forget_keys: Arc::new(TenantKeys::new(secrets)),
            org_workspace_id,
            capture: deps.capture.clone(),
            notifications,
            person_activity,
            live_connections: pagis_server::LiveConnections::new(deps.clock.clone()),
            sign_in_limits: Arc::new(pagis_server::SignInLimits::default()),
            password_verifier: Arc::new(pagis_server::PasswordVerifier::argon2()),
            client_credential: None,
            runtime_port: 0,
            port_override: None,
            supervised: false,
            started_at: now_ms(),
            public_origin: origin,
            local_origin: None,
            remote_access: false,
            administration: pagis_server::AdministrationAddress::of(
                "127.0.0.1:4401".parse().unwrap(),
                "http://127.0.0.1:4400",
                "",
            ),
            product_app_links: Vec::new(),
            org_menu,
            person_notice,
            administration_sections,
            proxy: pagis_server::TrustedProxy::none(),
            ring,
            hub: deps.hub.clone(),
            progress: deps.progress.clone(),
            agents,
            keys,
            models: deps.models.clone(),
            database_backend: "fake".into(),
            clock: deps.clock,
        };
        Self {
            clock,
            stores,
            routers: pagis_server::routers(state, axum::Router::new()),
            workspace,
            other_workspace,
            cookie,
            other_cookie,
            mail_transport,
            mailbox_host,
            mail,
            plugin_states,
        }
    }

    async fn workspace(stores: &Stores, person: &User) -> WorkspaceId {
        let workspace = Workspace {
            delegate_everything: false,
            kind: pagis_core::WorkspaceKind::Private,
            legal_hold: None,
            monthly_spend_cap_usd: None,
            id: WorkspaceId::generate(),
            user_id: Some(person.id.clone()),
            name: "Desk".into(),
            timezone: "UTC".into(),
            created_at: now_ms(),
            onboarded_at: None,
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
            home_exit_host_id: None,
        };
        stores.workspaces.create(&workspace).await.unwrap();
        workspace.id
    }

    async fn session(stores: &Stores, person: &User, secret: &str) -> String {
        let now = now_ms();
        stores
            .sessions
            .create(&Session {
                id: SessionId::generate(),
                user_id: person.id.clone(),
                token_hash: pagis_server::hash_secret(secret),
                client_kind: ClientKind::Browser,
                client_name: None,
                created_at: now,
                last_used_at: now,
                expires_at: now + SESSION_LIFETIME_MS,
            })
            .await
            .unwrap();
        format!("{}={secret}", pagis_server::SESSION_COOKIE)
    }

    pub async fn agent(&self, workspace: &WorkspaceId, name: &str) -> Agent {
        let agent = Agent {
            id: AgentId::generate(),
            workspace_id: workspace.clone(),
            name: name.into(),
            job: "Assistant".into(),
            description: String::new(),
            personality: String::new(),
            model_alias: DEFAULT_MODEL_ALIAS.into(),
            avatar: Default::default(),
            voice: None,
            standing_brief: None,
            status: AgentStatus::Active,
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        self.stores.agents.create(&agent).await.unwrap();
        agent
    }
}

// These routes do not use a Computer, model call or Notification. A call
// into one of these services is a test failure.
struct UnusedServices;
impl Computers for UnusedServices {
    fn get(&self, _: &WorkspaceId) -> Arc<dyn Computer> {
        panic!("the route needs no Computer")
    }
}
#[async_trait::async_trait]
impl ToolExecutor for UnusedServices {
    async fn execute(&self, _: AuthorizedCall) -> ToolResult {
        panic!("the route needs no tool call")
    }
}
#[async_trait::async_trait]
impl pagis_agent::Brain for UnusedServices {
    async fn turn(
        &self,
        _: pagis_agent::TurnRequest,
    ) -> Result<pagis_agent::TurnStream, pagis_agent::BrainError> {
        panic!("the route needs no model call")
    }
}
#[async_trait::async_trait]
impl pagis_agent::ModelCallGate for UnusedServices {
    async fn before_call(
        &self,
        _: &Run,
        _: &pagis_agent::ModelCallBound,
    ) -> Result<(), pagis_agent::GateStop> {
        panic!("the route needs no model call")
    }
}
#[async_trait::async_trait]
impl pagis_needs_you::NotificationSender for UnusedServices {
    async fn send(
        &self,
        _: &pagis_push::Subscription,
        _: &[u8],
        _: pagis_push::Options,
    ) -> pagis_push::Outcome {
        panic!("the route needs no Notification")
    }
}

/// Core and Plugin tools use their runtimes. Other calls use the test service.
struct RouteExecutor {
    core: Arc<CoreToolRuntime>,
    external: Arc<dyn ToolExecutor>,
    // PluginHosts reads the Broker's manifests, so the runtime is set
    // after the Broker exists.
    plugins: OnceLock<pagis_plugins::PluginToolRuntime>,
}

#[async_trait::async_trait]
impl ToolExecutor for RouteExecutor {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        match call.route {
            pagis_broker::ToolRoute::Core { .. } | pagis_broker::ToolRoute::Ui { .. } => {
                self.core.execute(call).await
            }
            pagis_broker::ToolRoute::Plugin { .. } => {
                self.plugins
                    .get()
                    .expect("the Embedder has a Plugin runtime")
                    .execute(call)
                    .await
            }
            _ => self.external.execute(call).await,
        }
    }
}
