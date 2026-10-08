//! Assemble the axum app from a booted daemon: SQLite stores behind the
//! trait seams, the audit event bus (ring-wrapped for WS resume), the
//! agent loop, and the server router.

use std::sync::Arc;

use futures::StreamExt;
use object_store::ObjectStore;
use object_store::local::LocalFileSystem;
use pagis_agent::{
    AgentDeps, AgentLoopConfig, AgentSystem, Brain, CoreToolRuntime, ProgressHub, StreamHub,
    ToolRuntimeDeps,
};
use pagis_audit::AuditEventBus;
use pagis_core::{EventBus, EventScope, ProviderKeys, WorkspaceStore};
use pagis_server::{AppState, EventRing, RestartSwitch, RingConfig, RingedBus, SystemConfigFile};
use tokio_util::sync::CancellationToken;

use crate::boot::Booted;

/// How often the daemon looks for plugin servers to stop. The
/// idle time itself is ten minutes, so a minute of granularity is
/// enough and costs one wake-up a minute.
const IDLE_REAP_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// App assembly knobs. `main` uses [`AppOptions::production`]; the
/// testkit injects a scripted brain, fake keys, and a fake Docker
/// probe.
pub struct AppOptions {
    /// The port the server identity proof binds to.
    pub runtime_port: u16,
    /// What sets the port of this run over `config.toml`: `--port` or
    /// `PAGIS_PORT`. `None` when the file sets it.
    pub port_override: Option<&'static str>,
    /// Whether a supervisor starts the daemon again after it exits for
    /// a restart: the Client App, or the restart policy of the compose
    /// deployment. Each of them sets `PAGIS_SUPERVISED`.
    pub supervised: bool,
    /// The origin a browser reaches this installation at. `main`
    /// derives it from the config and the port it actually serves on.
    pub public_origin: String,
    /// Where a browser reaches the Administration Interface. `main`
    /// binds the configured address, and the testkit its own.
    pub administration: pagis_server::AdministrationAddress,
    /// The reverse proxy whose forwarded headers the daemon believes.
    /// A local installation trusts none.
    pub proxy: pagis_server::TrustedProxy,
    /// Whether the daemon runs in Remote Access (ADR-0028). `main` reads
    /// `[remote_access] enabled` and `PAGIS_REMOTE_ACCESS`.
    pub remote_access: bool,
    /// The `tailscale` command that the Remote Access switch drives.
    /// Production runs the command of this machine; the testkit injects
    /// a fake, because no test may change a tailnet.
    pub tailscale: Arc<dyn pagis_server::Tailscale>,
    /// The loopback port of the TURN server of Remote Access (ADR-0028):
    /// `[screen] remote_access_turn_port`. The switch publishes it with
    /// Funnel on port 8443.
    pub remote_access_turn_port: u16,
    /// The listener of that port. `main` binds it while Remote Access is
    /// on, and the testkit binds an ephemeral port. The daemon serves the
    /// TURN server on it, and does not run in Remote Access without it.
    pub remote_access_turn_listener: Option<tokio::net::TcpListener>,
    /// The listener of the exit port of a Server (ADR-0029): `[computer]
    /// exit_port`. `main` binds it on a Server, and the testkit binds a
    /// loopback port for a Server. The Exit Proxy of each Agent's
    /// Computer in Home mode sends its connections there. A Local
    /// Installation has none, and its Computers run in Direct mode alone.
    pub exit_listener: Option<tokio::net::TcpListener>,
    /// The listener of the Harness Model Endpoint (ADR-0033): `[computer]
    /// model_port`. `main` binds it on both kinds of installation, and the
    /// testkit binds a loopback port. The harness of a Coding Session in a
    /// Computer sends its model requests there.
    pub model_listener: Option<tokio::net::TcpListener>,
    pub ring: RingConfig,
    /// `None` builds the production router brain over `keys`.
    pub brain: Option<Arc<dyn Brain>>,
    pub agents: AgentLoopConfig,
    pub keys: Arc<ProviderKeys>,
    /// Base URL overrides per provider, for the Provider Model Lists and
    /// the Harness Model Endpoint. Production keeps the provider
    /// defaults; the testkit points every provider at a local address,
    /// because no test may reach a real provider.
    pub provider_base_urls: std::collections::HashMap<pagis_core::Provider, String>,
    /// Docker discovery (ADR-0024). One instance serves the computer
    /// runtime, the onboarding status and the System tab.
    pub docker_discovery: Arc<pagis_computer::DockerDiscovery>,
    /// The config file the System tab reads and writes.
    pub system: Arc<dyn SystemConfigFile>,
    /// The restart the System tab asks for. `main` waits on it and
    /// exits with the reserved code.
    pub restart: Arc<RestartSwitch>,
    /// The artifact upload size cap. Production reads it from
    /// the config file; tests shrink it for the 413 scenario.
    pub artifact_max_bytes: u64,
    /// The computer runtime seam: `None` uses bollard; tests
    /// inject a fake.
    pub computer: Option<Arc<dyn pagis_computer::ComputerRuntime>>,
    /// How long an awake computer sits idle before it stops.
    pub computer_idle_stop: std::time::Duration,
    /// What one container may take of the host.
    pub computer_limits: pagis_computer::ComputerLimits,
    /// How many Computers may be awake at once, per tenant and
    /// for the whole server.
    pub computer_awake_caps: pagis_computer::AwakeCaps,
    /// How media reaches a browser (ADR-0014): the `[screen]`
    /// section picks the relay, and the screen path calls nothing else.
    pub media_relay: Arc<dyn pagis_computer::MediaRelay>,
    /// The same relay as the System Settings name it.
    pub screen: pagis_server::ScreenRelay,
    /// The platform secret store. It holds the provider keys and
    /// the Tenant Data Key of each Workspace; nothing else opens a
    /// sealed secret.
    pub secrets: Arc<dyn pagis_core::SecretStore>,
    /// How a Connection becomes a live provider; `None` builds
    /// the Google provider over the distributed `gog`. Tests inject a
    /// fake so no process runs.
    pub connection_providers: Option<Arc<dyn crate::connections::ConnectionProviderFactory>>,
    /// How long one provider call may run before the daemon stops
    /// waiting for it.
    pub connection_call_timeout: std::time::Duration,
    /// Google's OAuth endpoints for the brokered flow. `None`
    /// uses Google's own, which no test may reach.
    pub google_oauth: Option<Arc<pagis_google::GoogleOAuth>>,
    /// How the connect flow reaches `gog`; `None` starts the
    /// distributed binary. Tests inject a fake so no process runs.
    pub gog: Option<Arc<dyn pagis_google::GogRunner>>,
    /// How long the daemon waits for the user to finish at Google.
    pub authorize_timeout: std::time::Duration,
    /// How often each Connection with a live Event Subscription is
    /// polled. Tests shrink it.
    pub collector_interval: std::time::Duration,
    /// The number half of every carrier (ADR-0020); `None`
    /// builds the REST client of each provider. Tests inject a fake so
    /// no network is used.
    pub number_catalogs: Option<Arc<pagis_telephony::NumberCatalogs>>,
    /// The text half of every carrier (ADR-0020); `None` builds the
    /// text client of each provider that carries texts. A provider
    /// with no entry carries none. Tests inject a fake so no network
    /// is used.
    pub text_transports: Option<Arc<pagis_telephony::TextTransports>>,
    /// The host API of a Mailbox Provider (ADR-0019); `None` builds the
    /// Migadu client. Tests inject a fake so no network is used.
    pub mail_host: Option<Arc<dyn pagis_mail::MailboxHost>>,
    /// The mail half of a Mailbox Provider (ADR-0019); `None` reads
    /// and sends over the standard IMAP and SMTP client. Tests inject
    /// the fake, so no test reaches a mail host.
    pub mail_transport: Option<Arc<dyn pagis_mail::MailTransport>>,
    /// How the mailbox login proof waits (ADR-0019). Tests shrink it.
    pub mailbox_proof: pagis_mail::ProofTiming,
    /// The voice seam (ADR-0020); `None` builds the router voice
    /// over `keys` and the `transcribe` and `speak` aliases. Tests
    /// inject a fake so no provider is reached.
    pub voice: Option<Arc<dyn pagis_voice::VoiceProvider>>,
    /// The carrier's call half (ADR-0020); `None` builds the SIP
    /// transport every carrier registers over. Tests inject a fake so no socket opens.
    pub call_transport: Option<Arc<dyn pagis_telephony::CallTransport>>,
    /// What executes one call (ADR-0020); `None` builds the
    /// realtime bridge over the Provider Keys. Tests inject the
    /// scripted fake.
    pub call_bridge: Option<Arc<dyn pagis_telephony::CallBridge>>,
    /// The media hubs of the calls that run right now; `None` builds a
    /// fresh registry. Tests keep an `Arc` so they can make a call
    /// live the way the bridge does.
    pub live_calls: Option<Arc<pagis_telephony::LiveCalls>>,
    /// The tier gates of the calls that run right now; `None` builds a
    /// fresh registry. Tests keep an `Arc` for the same reason as
    /// `live_calls`.
    pub live_tiers: Option<Arc<pagis_telephony::LiveTiers>>,
    /// The password checks of a password sign-in. Production checks
    /// argon2id with one verification permit for each core; a test
    /// injects a check that counts or holds its calls.
    pub password_verifier: Arc<pagis_server::PasswordVerifier>,
    /// The logical now of the knowledge worker's background passes.
    /// Production runs on the wall clock; the evaluation
    /// driver injects the fixture clock of the chronology it replays.
    pub clock: Arc<dyn pagis_core::Clock>,
    /// The stop signal of every background loop the daemon starts.
    /// `main` cancels it when the daemon stops; a test daemon cancels it
    /// when it restarts and when it drops.
    pub cancel: CancellationToken,
    /// Which endpoints a Notification may go to (ADR-0030). Production
    /// sends only to a public `https` endpoint; a test also reaches a
    /// push service on loopback.
    pub push_policy: pagis_push::Policy,
    /// Where the daemon sends anonymous analytics, or why it sends
    /// none (ADR-0026). Production reads the project this build holds;
    /// a test daemon sends nothing unless a test points it at a fake.
    pub analytics: crate::analytics::AnalyticsOptions,
}

/// The variable a supervisor sets on the daemon it starts again after a
/// restart.
pub const SUPERVISED: &str = "PAGIS_SUPERVISED";

impl AppOptions {
    /// Production assembly: keys resolve env, then the config file,
    /// then the platform secret store; Docker is probed via bollard.
    pub fn production(booted: &Booted, restart: Arc<RestartSwitch>) -> anyhow::Result<Self> {
        let secrets = crate::secrets::platform_secret_store(
            &booted.home,
            &booted.config,
            booted.installation(),
        )?;
        let docker_discovery = Arc::new(pagis_computer::DockerDiscovery::production(
            booted.config.docker_endpoint(),
        ));
        Ok(Self {
            runtime_port: booted.config.port,
            port_override: std::env::var_os("PAGIS_PORT").map(|_| "PAGIS_PORT"),
            supervised: std::env::var(SUPERVISED)
                .is_ok_and(|value| matches!(value.trim(), "1" | "true")),
            public_origin: booted.config.public_origin(booted.config.port),
            administration: pagis_server::AdministrationAddress::of(std::net::SocketAddr::new(
                booted.config.administration.bind_address()?,
                booted.config.administration.port,
            )),
            proxy: booted.config.trusted_proxy()?,
            remote_access: booted.config.remote_access.enabled,
            tailscale: Arc::new(crate::tailscale::TailscaleCommand::of_this_machine()),
            remote_access_turn_port: booted.config.screen.remote_access_turn_port()?,
            // `main` binds it, after the other two listeners.
            remote_access_turn_listener: None,
            // `main` binds it on a Server.
            exit_listener: None,
            // `main` binds it.
            model_listener: None,
            ring: RingConfig::default(),
            brain: None,
            agents: AgentLoopConfig::default(),
            keys: Arc::new(ProviderKeys::new(
                booted.config.provider_keys(),
                Arc::clone(&secrets),
            )),
            provider_base_urls: std::collections::HashMap::new(),
            docker_discovery,
            system: Arc::new(crate::system::FileSystemConfig::new(&booted.home)),
            restart,
            artifact_max_bytes: booted.config.artifact_max_bytes,
            computer: None,
            computer_idle_stop: booted.config.computer.idle_stop(),
            computer_limits: booted.config.computer.limits(),
            computer_awake_caps: booted.config.computer.caps(),
            media_relay: booted.config.screen.media_relay()?,
            screen: booted.config.screen.screen_relay()?,
            secrets,
            connection_providers: None,
            connection_call_timeout: crate::connections::DEFAULT_CALL_TIMEOUT,
            google_oauth: None,
            gog: None,
            authorize_timeout: pagis_connect::DEFAULT_AUTHORIZE_TIMEOUT,
            collector_interval: crate::collectors::DEFAULT_POLL_INTERVAL,
            number_catalogs: None,
            text_transports: None,
            mail_host: None,
            mail_transport: None,
            mailbox_proof: pagis_mail::ProofTiming::default(),
            voice: None,
            call_transport: None,
            call_bridge: None,
            live_calls: None,
            live_tiers: None,
            password_verifier: Arc::new(pagis_server::PasswordVerifier::argon2()),
            clock: Arc::new(pagis_core::SystemClock),
            cancel: CancellationToken::new(),
            push_policy: pagis_push::Policy::Public,
            analytics: crate::analytics::AnalyticsOptions::production(),
        })
    }
}

/// The two interfaces of one daemon, each ready to serve on its
/// own listener: the product, and the Administration Interface. They are
/// built over one state, because one process serves both.
pub struct Interfaces {
    pub product: axum::Router,
    pub administration: axum::Router,
    /// The Computers of every tenant, which a daemon that stops for good
    /// stops.
    pub computers: Arc<pagis_computer::ComputerManagers>,
    /// The exit sockets of the Hosts, which carry the connections of
    /// their Person's Computers as the Home Exit (ADR-0029).
    pub home_exits: Arc<pagis_computer::HomeExits>,
    /// The session sockets of the Hosts, which carry one stream for each
    /// Coding Session (ADR-0033).
    pub host_sessions: Arc<pagis_broker::HostSessions>,
    /// The Coding Sessions that run on those sockets.
    pub coding_sessions: Arc<pagis_coding::CodingSessions>,
}

pub async fn app(booted: &Booted, mut options: AppOptions) -> anyhow::Result<Interfaces> {
    let mut stores = booted.stores.clone();
    // The deployment's own setup: a start that finds nobody who
    // can sign in reads `PAGIS_ADMIN_EMAIL`, `PAGIS_ADMIN_PASSWORD` and
    // the provider key variables and makes the first Administrator. A
    // start that finds one does nothing, so the variables may stay in
    // the deployment's configuration for ever.
    if pagis_server::setup::from_environment(
        &stores,
        options.secrets.as_ref(),
        |name| std::env::var(name).ok(),
        options.clock.now_ms(),
    )
    .await?
    {
        tracing::info!("the environment configured this installation's administrator");
    }
    let workspaces = stores.workspaces.clone();
    // Every tenant this daemon serves. The boot work that touches
    // each tenant's own records loops over this list; no subsystem takes
    // the first row and calls it the installation's.
    let tenants: Vec<pagis_core::WorkspaceId> = workspaces
        .list()
        .await?
        .into_iter()
        .map(|workspace| workspace.id)
        .collect();
    if tenants.is_empty() {
        anyhow::bail!("no workspace; boot seeding failed");
    }
    // The Org's Workspace, which no person owns. It holds the installed
    // Plugins (ADR-0017) and the Installation Connections, and every
    // person reads them from here.
    let org = stores
        .orgs
        .list()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no org; boot seeding failed"))?;
    let org_workspace_id = org.workspace_id.clone();

    // One ring-wrapped bus for the server and the agent loop, so every
    // published event is resumable over `last_seq`.
    let ring = Arc::new(EventRing::new(options.ring));
    let bus: Arc<dyn EventBus> = Arc::new(RingedBus::new(
        Arc::new(AuditEventBus::new(stores.events.clone())),
        Arc::clone(&ring),
    ));
    // Each write of a Coding Session record or transcript reports itself
    // to the clients, so the store is wrapped before anything reads it.
    stores.coding_sessions = Arc::new(pagis_server::CodingSessionFeed::new(
        stores.coding_sessions.clone(),
        Arc::clone(&bus),
        Arc::clone(&options.clock),
    ));
    let messages = stores.messages.clone();
    let requests = stores.requests.clone();
    let grants = stores.grants.clone();
    let agent_store = stores.agents.clone();
    let artifacts = stores.artifacts.clone();
    let runs = stores.runs.clone();
    let model_aliases = stores.model_aliases.clone();
    let onboarding = stores.onboarding.clone();
    let connections = stores.connections.clone();
    let knowledge = stores.knowledge.clone();
    let credentials = stores.credentials.clone();
    let schedule_store = stores.schedules.clone();
    let trigger_store = stores.triggers.clone();
    let subscription_store = stores.subscriptions.clone();
    // Artifact bytes live under the data directory, behind the
    // ObjectStore seam.
    let blob_dir = booted.home.join("artifacts");
    std::fs::create_dir_all(&blob_dir)?;
    let blobs: Arc<dyn ObjectStore> = Arc::new(LocalFileSystem::new_with_prefix(&blob_dir)?);
    // Memory repositories live under the data directory:
    // one git repo per workspace, daemon-side only.
    let memory = pagis_memory::GitMemoryStore::new(
        booted.home.join("memory"),
        stores.forget.clone(),
        stores.memory_pages.clone(),
    );
    // The Page Index is one tenant's work, so the boot syncs it for
    // every tenant.
    for workspace_id in &tenants {
        memory.sync_page_index_later(workspace_id);
    }
    let memory: Arc<dyn pagis_core::MemoryStore> = Arc::new(memory);
    let hub = Arc::new(StreamHub::default());
    let progress = Arc::new(ProgressHub::default());
    let keys = options.keys;
    // The Provider Model Lists: one cache for the installation's keys,
    // refreshed in the background, which the brain, the context budget,
    // the Spend Cap and the Models settings all read.
    let models = Arc::new(options.provider_base_urls.clone().into_iter().fold(
        pagis_agent::ModelCatalog::new(Arc::clone(&keys)),
        |catalog, (provider, base_url)| catalog.with_base_url(provider, base_url),
    ));
    models.spawn_refresher(options.cancel.clone());
    let brain = options.brain.unwrap_or_else(|| {
        Arc::new(pagis_agent::RouterBrain::new(
            Arc::clone(&keys),
            Arc::clone(&models),
        ))
    });

    let events = stores.events.clone();
    // The agent computer lifecycle: bollard against the pinned
    // image, screenshots under the data directory, idle-stop sweeper.
    let computer_runtime = options.computer.unwrap_or_else(|| {
        Arc::new(pagis_computer::BollardRuntime::new(
            Arc::clone(&options.docker_discovery),
            pagis_computer::RuntimeOptions {
                limits: options.computer_limits.clone(),
                // The screend token of each container, one file per
                // (tenant, Agent) under the data directory.
                tokens_dir: booted.home.join(crate::boot::COMPUTER_TOKENS_DIR),
                labels: Vec::new(),
            },
        ))
    });
    // The Skills of the installed Plugins (ADR-0017). The
    // catalogue reads the checkouts the install path writes, so it is
    // built before the Computer, which mounts them, and before the
    // Agent loop, which lists and loads them.
    let plugin_store = stores.plugins.clone();
    let plugin_git = Arc::new(pagis_plugin::PluginGitStore::new(
        booted.home.join("plugins"),
    ));
    let skills = Arc::new(pagis_plugin::SkillCatalog::new(
        pagis_plugin::SkillCatalogDeps {
            plugins: Arc::clone(&plugin_store) as _,
            grants: Arc::clone(&grants) as _,
            git: Arc::clone(&plugin_git),
        },
    ));
    // The exit listener of a Server (ADR-0029). A Local Installation
    // leaves from the owner's own connection, so it opens none, and its
    // Computers run their Exit Proxy in Direct mode alone.
    if booted.installation() == crate::Installation::Local && options.exit_listener.is_some() {
        anyhow::bail!("a Local Installation opens no exit listener");
    }
    // The Home Exits of the People: the exit socket of each Host, the
    // System Setting that an Administrator turns off, and on a Server the
    // exit listener that sends a Computer's connections through them.
    let home_exits = pagis_computer::HomeExits::new(Arc::clone(&workspaces) as _);
    home_exits.set_enabled(booted.config.computer.home_exit);
    // The Model Request Capture System Setting (ADR-0031). The agent loop
    // reads it at each model request, and its route changes it.
    let capture = Arc::new(pagis_core::CaptureSetting::new(
        booted.config.model_request_capture.enabled,
        booted.config.model_request_capture.retention_days,
    ));
    let exit = options
        .exit_listener
        .as_ref()
        .map(|listener| listener.local_addr())
        .transpose()?
        .map(|address| pagis_computer::ComputerExit {
            daemon: pagis_computer::exit_daemon(address.port()),
            home_exits: Arc::clone(&home_exits),
        });
    // One Computer manager per tenant: the manager names and
    // labels every Docker object, so a container of one Workspace is
    // never a container of another. Each manager's idle sweeper starts
    // with it.
    let computers = pagis_computer::ComputerManagers::new(pagis_computer::ComputerManagersDeps {
        runtime: computer_runtime,
        skills: Arc::clone(&skills) as _,
        workspaces: Arc::clone(&workspaces) as _,
        bus: Arc::clone(&bus),
        screens_dir: booted.home.join("screens"),
        agents: Arc::clone(&stores.agents) as _,
        idle_stop: options.computer_idle_stop,
        relay: options.media_relay,
        caps: options.computer_awake_caps,
        cancel: options.cancel.clone(),
        exit,
    });
    if let Some(listener) = options.exit_listener.take() {
        let address = listener.local_addr()?;
        let exit =
            pagis_computer::ExitListener::new(Arc::clone(&computers) as _, Arc::clone(&home_exits));
        tokio::spawn(exit.serve(listener, options.cancel.child_token()));
        tracing::info!(%address, "the exit listener carries the connections of the Computers");
    }
    // The Harness Model Endpoint (ADR-0033): the model API of the harness
    // of a Coding Session in a Computer, on its own listener.
    if let Some(listener) = options.model_listener.take() {
        let address = listener.local_addr()?;
        let endpoint = pagis_server::harness_model::router(pagis_server::HarnessModelDeps {
            sessions: stores.coding_sessions.clone(),
            keys: Arc::clone(&keys),
            provider_base_urls: options.provider_base_urls.clone(),
            workspaces: Arc::clone(&workspaces) as _,
            users: stores.users.clone(),
            usage: stores.usage.clone(),
            models: Arc::clone(&models),
            clock: Arc::clone(&options.clock),
        });
        let stop = options.cancel.child_token();
        tokio::spawn(async move {
            let served = axum::serve(listener, endpoint)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await;
            if let Err(error) = served {
                tracing::error!(%error, "the Harness Model Endpoint stopped");
            }
        });
        tracing::info!(%address, "the Harness Model Endpoint serves the harnesses of the Computers");
    }
    // The Computers that a restart left running belong to this daemon
    // now, also the Computer of a Person who does not come back: the
    // idle sweep and the stop for good include them, and the awake cap
    // counts them. Only the Workspaces of this installation are adopted,
    // never those of another installation on the same Docker host. A
    // Docker host that does not answer gives a warning, and the boot
    // continues.
    computers.adopt_all(&tenants).await;
    // The Computer Image of the release (ADR-0027). When the pinned
    // image is absent, its one pull for the installation starts now, and
    // each wake joins it. Then the old images that no container uses go.
    // The boot does not wait for it. The adoption comes first, so a
    // Computer of an old image is stopped and its image is free.
    tokio::spawn({
        let computers = Arc::clone(&computers);
        async move { computers.prepare_image().await }
    });
    // The retention sweep, daily. It deletes nothing until the
    // user sets a window for a class.
    let retention_policies = stores.retention_policies.clone();
    crate::retention::spawn_sweeper(
        crate::retention::RetentionDeps {
            workspaces: Arc::clone(&workspaces) as _,
            policies: Arc::clone(&retention_policies) as _,
            artifacts: Arc::clone(&artifacts) as _,
            blobs: Arc::clone(&blobs),
            capture: Arc::clone(&capture),
            captures: stores.model_request_captures.clone(),
        },
        options.cancel.clone(),
    );
    let channels = stores.channels.clone();
    let participants = stores.participants.clone();
    let gog = match options.gog {
        Some(gog) => gog,
        None => crate::connections::distributed_gog(Arc::clone(&options.secrets))?,
    };
    let connection_state = Arc::new(crate::connections::ConnectionState::new(
        Arc::clone(&connections) as _,
        Arc::clone(&bus),
    ));
    let pending_evidence = stores.pending_evidence.clone();
    let conversation_evidence = stores.conversation_evidence.clone();
    let continuations = stores.continuations.clone();
    recover_pending_reviews(
        pending_evidence.as_ref(),
        memory.as_ref(),
        options.clock.now_ms(),
    )
    .await?;
    // Which of the person's machines are connected. One registry
    // serves the WebSocket that holds the clients, the broker that names
    // the machine of a host action, and the tool that dispatches to it.
    let host_presence = Arc::new(pagis_broker::HostPresence::new());
    // The session sockets of the same machines, which carry the Coding
    // Sessions (ADR-0033).
    let host_sessions = Arc::new(pagis_broker::HostSessions::new());
    // The Coding Sessions on those sockets. Pagis policy answers each
    // Harness Permission, and each question is cancelled.
    let coding_sessions = Arc::new(pagis_coding::CodingSessions::new(
        pagis_coding::CodingSessionsDeps {
            sessions: stores.coding_sessions.clone(),
            runs: Arc::clone(&runs) as _,
            hosts: stores.hosts.clone(),
            messages: Arc::clone(&messages) as _,
            bus: Arc::clone(&bus),
            place: Arc::clone(&host_sessions) as _,
            decisions: Arc::new(pagis_coding::PolicyDecisions::new(
                Arc::clone(&grants) as _,
                Arc::clone(&bus),
            )),
            clock: Arc::clone(&options.clock),
            cancel: options.cancel.clone(),
        },
    ));
    // The checks of a Coding Session start. The broker asks them before
    // the card, and the tool runtime again at the start.
    let session_starts = Arc::new(pagis_coding::CodingSessionStarts::new(
        stores.coding_sessions.clone(),
        Arc::clone(&grants) as _,
    ));
    let tool_runtime = Arc::new(CoreToolRuntime::new(ToolRuntimeDeps {
        presence: Arc::clone(&host_presence),
        agents: Arc::clone(&agent_store) as _,
        channels: Arc::clone(&channels) as _,
        messages: Arc::clone(&messages) as _,
        runs: Arc::clone(&runs) as _,
        participants: Arc::clone(&participants) as _,
        memory: Arc::clone(&memory),
        pending_evidence: Arc::clone(&pending_evidence) as _,
        conversation_evidence: Arc::clone(&conversation_evidence) as _,
        grants: Arc::clone(&grants) as _,
        skills: Arc::clone(&skills) as _,
        computers: Arc::clone(&computers),
        bus: Arc::clone(&bus),
        clock: Arc::clone(&options.clock),
        context_messages: options.agents.context_messages,
    }));
    // The brokered Google half. The Org's Web OAuth client is
    // what decides whether a new Google Connection is `brokered` or
    // `byo`; the tokens of a brokered one are sealed with the Tenant
    // Data Key of the person who consented. It reads the Sessions on
    // the clock that `auth::resolve` reads, so an authorization ends
    // with the Session that started it.
    let gog_root = booted.home.join("gog");
    // One holder of the Tenant Data Keys serves the Google broker, the
    // vault and Forget, so one Workspace has one key in memory, and it
    // is the key that `secrets.enc` holds.
    let tenant_keys = Arc::new(pagis_core::TenantKeys::new(Arc::clone(&options.secrets)));
    let google_broker = Arc::new(pagis_connect::GoogleBroker::new(
        Arc::clone(&connections) as _,
        Arc::new(pagis_connect::OrgWebClient::new(
            stores.orgs.clone() as _,
            Arc::clone(&options.secrets),
        )),
        options
            .google_oauth
            .unwrap_or_else(|| Arc::new(pagis_google::GoogleOAuth::new())),
        Arc::clone(&tenant_keys),
        stores.sessions.clone(),
        Arc::clone(&options.clock),
        options.public_origin.clone(),
    ));
    // One binder serves the tool dispatch, the collectors and the
    // knowledge worker, so every path reaches Google under the calling
    // Workspace's own `GOG_HOME` and with the right token.
    let google_binder = Arc::new(crate::connections::GoogleBinder::new(
        Arc::clone(&gog),
        gog_root.clone(),
        Arc::clone(&google_broker),
    ));
    // Provider dispatch: one instance per Connection, built on
    // first use and shared by every agent granted it.
    // One `gog` runner serves the dispatch path and the connect
    // flow, so an account is reached the same way it was bound.
    let connection_runtime = Arc::new(crate::connections::ConnectionRuntime::new(
        Arc::clone(&connections) as _,
        options.connection_providers.unwrap_or_else(|| {
            Arc::new(crate::connections::GoogleProviderFactory::new(Arc::clone(
                &google_binder,
            )))
        }),
        Arc::clone(&connection_state),
        options.connection_call_timeout,
    ));
    // The carrier's number half: search, buy and release, over
    // the REST API key. The call half arrives with the carrier's line.
    let number_catalogs = match options.number_catalogs {
        Some(catalogs) => catalogs,
        None => Arc::new(
            pagis_telephony::NumberCatalogs::new()
                .with(
                    pagis_telephony::TELNYX_PROVIDER,
                    Arc::new(
                        pagis_telephony::TelnyxNumberCatalog::new().map_err(|error| {
                            anyhow::anyhow!("the telephony client failed to start: {error}")
                        })?,
                    ),
                )
                .with(
                    pagis_telephony::TWILIO_PROVIDER,
                    Arc::new(
                        pagis_telephony::TwilioNumberCatalog::new().map_err(|error| {
                            anyhow::anyhow!("the telephony client failed to start: {error}")
                        })?,
                    ),
                )
                .with(
                    pagis_telephony::PLIVO_PROVIDER,
                    Arc::new(pagis_telephony::PlivoNumberCatalog::new().map_err(|error| {
                        anyhow::anyhow!("the telephony client failed to start: {error}")
                    })?),
                ),
        ),
    };
    // The carrier's text half (ADR-0020): send a text, poll the texts
    // that came in, and hold the carrier's messaging object in place,
    // over the same REST API key. Plivo never returns the body of an
    // inbound text, so it gets the transport that carries none; a
    // carrier with no entry carries none either.
    let text_transports = match options.text_transports {
        Some(transports) => transports,
        None => Arc::new(
            pagis_telephony::TextTransports::new()
                .with(
                    pagis_telephony::TELNYX_PROVIDER,
                    Arc::new(
                        pagis_telephony::TelnyxTextTransport::new().map_err(|error| {
                            anyhow::anyhow!("the telephony text client failed to start: {error}")
                        })?,
                    ),
                )
                .with(
                    pagis_telephony::TWILIO_PROVIDER,
                    Arc::new(
                        pagis_telephony::TwilioTextTransport::new().map_err(|error| {
                            anyhow::anyhow!("the telephony text client failed to start: {error}")
                        })?,
                    ),
                )
                .with(
                    pagis_telephony::PLIVO_PROVIDER,
                    Arc::new(pagis_telephony::NoTextTransport),
                ),
        ),
    };
    // The mailbox half of a Mailbox Provider (ADR-0019): make, delete
    // and list the mailboxes of one domain with the host API key.
    let mail_host: Arc<dyn pagis_mail::MailboxHost> =
        match options.mail_host {
            Some(host) => host,
            None => Arc::new(pagis_mail::MigaduHost::new().map_err(|error| {
                anyhow::anyhow!("the mail host client failed to start: {error}")
            })?),
        };
    // The mail half (ADR-0019): read over IMAP and send over SMTP with
    // each mailbox's own password.
    let mail_transport: Arc<dyn pagis_mail::MailTransport> = options
        .mail_transport
        .unwrap_or_else(|| Arc::new(pagis_mail::StandardTransport::new()));
    let connector = Arc::new(pagis_connect::Connector::new(
        pagis_connect::ConnectorDeps {
            connections: Arc::clone(&connections) as _,
            runner: Arc::clone(&gog),
            catalogs: Arc::clone(&number_catalogs),
            mail_host: Arc::clone(&mail_host),
            secrets: Arc::clone(&options.secrets),
            authorize_timeout: options.authorize_timeout,
            google: Arc::clone(&google_broker),
            gog_root: gog_root.clone(),
        },
    ));
    // The mailbox desk (ADR-0019): the one path that makes, proves,
    // recovers, sleeps and deletes an Agent Mailbox.
    let mailbox_store = stores.agent_mailboxes.clone();
    // The sent-message table: the mail tools write it, and the
    // collector reads it to land a reply in the Thread that sent the
    // message it answers (ADR-0019).
    let sent_mail = stores.sent_mail.clone();
    // The Standing Mail Rule needs the Trigger module, which reads the
    // declarations the broker holds and is therefore built later
    // (ADR-0019).
    let standing_mail_rule = Arc::new(crate::mail_events::DeferredStandingRule::default());
    // The same pair for calls (ADR-0020): the number desk writes the
    // Standing Call Rule when a line becomes an Agent's, and a settled
    // call hands its envelope to the Trigger module. Both wait for the
    // Trigger module, which is built further down.
    let standing_call_rule = Arc::new(crate::call_events::DeferredStandingCallRule::default());
    let call_events = Arc::new(crate::call_events::DeferredCallEvents::default());
    let mailbox_desk = Arc::new(pagis_mail::MailboxDesk::new(pagis_mail::MailboxDeskDeps {
        mailboxes: Arc::clone(&mailbox_store) as _,
        connections: Arc::clone(&connections) as _,
        org_workspace_id: org_workspace_id.clone(),
        agents: Arc::clone(&agent_store) as _,
        host: mail_host,
        transport: mail_transport,
        secrets: Arc::clone(&options.secrets),
        bus: Arc::clone(&bus),
        standing_rule: Arc::clone(&standing_mail_rule) as _,
        proof: options.mailbox_proof,
    }));
    // The `mail` block of an inbound mail that woke the Agent and of a
    // mail it sent (ADR-0019).
    let mail_blocks = Arc::new(pagis_mail::MailBlocks::new(pagis_mail::MailBlockDeps {
        grants: Arc::clone(&grants) as _,
        messages: Arc::clone(&messages) as _,
        bus: Arc::clone(&bus),
        triggers: Arc::clone(&trigger_store) as _,
        desk: Arc::clone(&mailbox_desk),
    }));
    let phone_numbers = stores.phone_numbers.clone();
    // The carrier's call half: one line for each carrier Connection
    // keeps the SIP credential registered and carries every number.
    let call_transport: Arc<dyn pagis_telephony::CallTransport> = match options.call_transport {
        Some(transport) => transport,
        None => Arc::new(
            pagis_telephony::SipCallTransport::new()
                .map_err(|error| anyhow::anyhow!("the SIP transport failed to start: {error}"))?,
        ),
    };
    let endpoints = Arc::new(pagis_telephony::Endpoints::new(
        pagis_telephony::EndpointsDeps {
            transport: Arc::clone(&call_transport),
            numbers: Arc::clone(&phone_numbers) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: org_workspace_id.clone(),
            secrets: Arc::clone(&options.secrets),
            bus: Arc::clone(&bus),
        },
    ));
    // The calls that run right now: the desk refuses to move a
    // line that is on one, and the session tools exist only on one.
    let live_calls = options.live_calls.unwrap_or_default();
    let live_tiers = options.live_tiers.unwrap_or_default();
    let calls = stores.calls.clone();
    // The Text Records (ADR-0020): every text a number sent or
    // received. There is no conversation table and no sent-texts
    // table; the outbound record's Thread field is the reply lookup.
    let texts = stores.text_records.clone();
    let trust_list = stores.trust_list.clone();
    let numbers = Arc::new(pagis_telephony::NumberDesk::new(
        pagis_telephony::NumberDeskDeps {
            numbers: Arc::clone(&phone_numbers) as _,
            connections: Arc::clone(&connections) as _,
            org_workspace_id: org_workspace_id.clone(),
            agents: Arc::clone(&agent_store) as _,
            catalogs: number_catalogs,
            text_transports: Arc::clone(&text_transports),
            secrets: Arc::clone(&options.secrets),
            calls: Arc::clone(&live_calls) as _,
            bus: Arc::clone(&bus),
            endpoints: Arc::clone(&endpoints),
            standing_rule: Arc::clone(&standing_call_rule) as _,
        },
    ));
    // A purchase that outlived the daemon is settled against the
    // carrier's own list, never by buying a second time (ADR-0018).
    let pending_purchases = phone_numbers.list_pending_intents().await?;
    spawn_purchase_reconciliation(
        Arc::clone(&numbers),
        pending_purchases,
        options.cancel.clone(),
    );
    spawn_carrier_start(
        Arc::clone(&numbers),
        Arc::clone(&workspaces) as _,
        options.cancel.clone(),
    );
    // Voice: keys resolve on every call, so a key the wizard
    // stores takes effect without a restart.
    let voice: Arc<dyn pagis_voice::VoiceProvider> = match options.voice {
        Some(voice) => voice,
        None => Arc::new(pagis_voice::RouterVoice::new(
            Arc::clone(&keys),
            Arc::clone(&model_aliases) as _,
        )),
    };
    // The vault is built once and serves both the broker (records and
    // requests) and settings. It shares the holder of the Tenant Data
    // Keys with the Google broker.
    let vault = Arc::new(pagis_vault::Vault::new(
        Arc::clone(&credentials) as _,
        Arc::clone(&computers),
        Arc::clone(&tenant_keys),
    ));
    let subscription_tools = Arc::new(crate::tools::DeferredExecutor::default());
    // The Software tools reach the broker to install the
    // Capability Manifest of a published Version, so they join after
    // the broker exists, the way the automation tools do.
    let software_tools = Arc::new(crate::tools::DeferredExecutor::default());
    // The call tool reads the Run's free tools from the broker, so it
    // is joined after the broker exists, the way the automation tools
    // are.
    let phone_tools = Arc::new(crate::tools::DeferredExecutor::default());
    // The MCP host of the Plugins. It is built once the broker
    // exists, because it installs Capability Manifests into it.
    let plugin_tools = Arc::new(crate::tools::DeferredExecutor::default());
    // The mail tool route for runs that pass the broker's authority and
    // effect reservation checks.
    let mail_tools = Arc::new(pagis_mail::MailToolRuntime::new(pagis_mail::MailToolDeps {
        desk: Arc::clone(&mailbox_desk),
        runs: Arc::clone(&runs) as _,
        sent: Arc::clone(&sent_mail) as _,
        blocks: Arc::clone(&mail_blocks),
    }));
    let broker = Arc::new(pagis_broker::Broker::new(pagis_broker::BrokerDeps {
        forget: stores.forget.clone(),
        grants: Arc::clone(&grants) as _,
        connections: Arc::clone(&connections) as _,
        credentials: Arc::clone(&vault) as _,
        requests: Arc::clone(&requests) as _,
        snapshots: stores.capability_snapshots.clone(),
        runs: Arc::clone(&runs) as _,
        conversation_evidence: Arc::clone(&conversation_evidence) as _,
        bus: Arc::clone(&bus),
        executor: Arc::new(crate::tools::RoutingExecutor::new(
            Arc::clone(&tool_runtime) as _,
            Arc::clone(&vault) as _,
            connection_runtime as _,
            Arc::clone(&subscription_tools) as _,
            Arc::clone(&phone_tools) as _,
            Arc::clone(&software_tools) as _,
            Arc::clone(&plugin_tools) as _,
            Arc::clone(&mail_tools) as _,
            Arc::new(pagis_coding::CodingToolRuntime::new(
                Arc::clone(&coding_sessions),
                Arc::clone(&session_starts) as _,
            )) as _,
        )),
        phone_numbers: Arc::clone(&phone_numbers) as _,
        mailboxes: Arc::clone(&mailbox_store) as _,
        hosts: stores.hosts.clone(),
        presence: Arc::clone(&host_presence),
        session_starts: Arc::clone(&session_starts) as _,
    }));
    // The realtime bridge: the model session, the SIP leg
    // and the recording of one call. A test injects a scripted bridge
    // instead; the daemon builds this one.
    let call_bridge: Arc<dyn pagis_telephony::CallBridge> = match options.call_bridge {
        Some(bridge) => bridge,
        None => {
            let recordings = booted.home.join("recordings");
            std::fs::create_dir_all(&recordings)?;
            let bridge = Arc::new(pagis_telephony::RealtimeBridge::new(
                pagis_telephony::RealtimeBridgeDeps {
                    endpoints: Arc::clone(&endpoints),
                    sessions: Arc::new(pagis_telephony::KeyedModelSessions::new(
                        Arc::clone(&keys),
                        Arc::clone(&model_aliases) as _,
                    )),
                    tools: Arc::new(pagis_telephony::BrokerCallTools::new(Arc::clone(&broker))),
                    calls: Arc::clone(&calls) as _,
                    artifacts: Arc::clone(&artifacts) as _,
                    blobs: Arc::clone(&blobs),
                    recordings,
                    live: Arc::clone(&live_calls),
                    tiers: Arc::clone(&trust_list) as _,
                    keypad: pagis_telephony::Keypad {
                        code: Arc::new(crate::keypad::VaultCodeCheck::new(Arc::clone(
                            &options.secrets,
                        ))),
                        failures: stores.keypad_failures.clone(),
                        clock: Arc::clone(&options.clock),
                    },
                    live_tiers: Arc::clone(&live_tiers),
                    cancel: options.cancel.clone(),
                },
            ));
            // A call does not survive a restart (ADR-0020): each Call
            // record the stopped daemon left unsettled ends now.
            let settled = bridge.recover().await?;
            if settled > 0 {
                tracing::info!(settled, "settled the calls a stopped daemon left behind");
            }
            bridge
        }
    };
    phone_tools.set(Arc::new(pagis_telephony::PhoneToolRuntime::new(
        pagis_telephony::PhoneToolDeps {
            desk: Arc::clone(&numbers),
            agents: Arc::clone(&agent_store) as _,
            runs: Arc::new(crate::tools::BrokerRunTools(Arc::clone(&broker))),
            run_records: Arc::clone(&runs) as _,
            messages: Arc::clone(&messages) as _,
            calls: Arc::clone(&calls) as _,
            tiers: Arc::clone(&trust_list) as _,
            transport: Arc::clone(&call_transport),
            bridge: Arc::clone(&call_bridge),
            live: Arc::clone(&live_calls),
            artifacts: Arc::clone(&artifacts) as _,
            blobs: Arc::clone(&blobs),
            bus: Arc::clone(&bus),
        },
    )));
    // The calls the carrier's line answers: each runs under the
    // Agent that holds the dialed number, in a Run opened in its Thread
    // with the user (ADR-0022). The receiver is taken once, here. The
    // stored record of the dialed number names the Workspace, so one
    // runner serves the numbers of every person.
    let inbound_calls = Arc::new(pagis_telephony::InboundCalls::new(
        pagis_telephony::InboundCallsDeps {
            numbers: Arc::clone(&phone_numbers) as _,
            agents: Arc::clone(&agent_store) as _,
            runs: Arc::new(crate::tools::BrokerInboundRuns {
                runs: Arc::clone(&runs) as _,
                channels: Arc::clone(&channels) as _,
                broker: Arc::clone(&broker),
                bus: Arc::clone(&bus),
                clock: Arc::clone(&options.clock),
            }),
            run_tools: Arc::new(crate::tools::BrokerRunTools(Arc::clone(&broker))),
            run_records: Arc::clone(&runs) as _,
            messages: Arc::clone(&messages) as _,
            transport: call_transport,
            bridge: call_bridge,
            live: Arc::clone(&live_calls),
            artifacts: Arc::clone(&artifacts) as _,
            blobs: Arc::clone(&blobs),
            bus: Arc::clone(&bus),
            events: Arc::clone(&call_events) as _,
        },
    ));
    let answered = endpoints
        .take_answered_calls()
        .expect("the answered calls are taken once, by the daemon");
    inbound_calls.spawn(answered, options.cancel.clone());
    // The Software List (ADR-0016): the records, the bare
    // repositories under the data directory, and the search index.
    let software_packages = stores.software.clone();
    let software_git = Arc::new(pagis_software::SoftwareGitStore::new(
        booted.home.join("software"),
    ));
    // A publish, a fork, a contribution and its close write one line
    // into the acting Agent's private memory.
    let software_notes = Arc::new(pagis_software::MemoryNotes::new(
        Arc::clone(&memory) as _,
        Arc::clone(&bus),
    ));
    let software = Arc::new(pagis_software::SoftwareList::new(
        pagis_software::SoftwareListDeps {
            packages: Arc::clone(&software_packages) as _,
            agents: Arc::clone(&agent_store) as _,
            git: Arc::clone(&software_git),
            computers: Arc::clone(&computers),
            manifests: Arc::new(crate::software_tools::BrokerManifests(Arc::clone(&broker))),
            bus: Arc::clone(&bus),
            notes: Arc::clone(&software_notes) as _,
        },
    ));
    // Fork and contribute back (ADR-0016). A Contribution and
    // its outcome go into the two agents' direct channel, over the
    // same path `send_message` takes.
    let contribution_store = stores.contributions.clone();
    let contributions = Arc::new(pagis_software::Contributions::new(
        pagis_software::ContributionsDeps {
            packages: Arc::clone(&software_packages) as _,
            contributions: Arc::clone(&contribution_store) as _,
            agents: Arc::clone(&agent_store) as _,
            git: Arc::clone(&software_git),
            messenger: Arc::new(crate::software_tools::DmMessenger {
                runtime: Arc::clone(&tool_runtime),
                dm: pagis_agent::AgentDm {
                    channels: Arc::clone(&channels) as _,
                    participants: Arc::clone(&participants) as _,
                    messages: Arc::clone(&messages) as _,
                    bus: Arc::clone(&bus),
                    clock: Arc::clone(&options.clock),
                },
                agents: Arc::clone(&agent_store) as _,
            }),
            bus: Arc::clone(&bus),
            notes: Arc::clone(&software_notes) as _,
        },
    ));
    // The installed Plugins (ADR-0017): the records, the bare
    // repository and checkout of each one under the data directory,
    // and the secret store every `secret` Binding writes to.
    let plugin_store = stores.plugins.clone();
    let plugin_git = Arc::new(pagis_plugin::PluginGitStore::new(
        booted.home.join("plugins"),
    ));
    // A Computer reads each checkout as a uid that does not own it, and
    // a restore gives every file the owner's bits alone. A checkout
    // that stays closed fails one Plugin, not the start.
    for plugin in plugin_store.list(&org_workspace_id).await? {
        if let Err(error) = plugin_git
            .make_checkout_readable(&org_workspace_id, plugin.id.as_str())
            .await
        {
            tracing::warn!(plugin = %plugin.id, %error, "the Plugin checkout is not readable by its Computer");
        }
    }
    // The MCP hosts (ADR-0017): one per tenant, each with its
    // own supervised clients, its own Plugin Computer and its own broker
    // registry. A Plugin is the Org's install, so the rows, the Bindings
    // and the checkout live in the Org's Workspace and every tenant runs
    // them.
    let plugin_hosts = pagis_plugins::PluginHosts::new(pagis_plugins::PluginHostsDeps {
        org_workspace_id: org_workspace_id.clone(),
        workspaces: Arc::clone(&workspaces) as _,
        plugins: Arc::clone(&plugin_store) as _,
        catalogs: stores.plugin_tools.clone(),
        connections: Arc::clone(&connections) as _,
        grants: Arc::clone(&grants) as _,
        secrets: Arc::clone(&options.secrets),
        tokens: Arc::new(crate::plugin_tools::WorkspaceConnectionTokens {
            secrets: Arc::clone(&options.secrets),
        }),
        git: Arc::clone(&plugin_git),
        // A Plugin's server process runs inside the tenant's Plugin
        // Computer and never on the daemon host (ADR-0017).
        processes: Arc::new(crate::plugin_tools::ComputerServerProcesses::new(
            Arc::clone(&computers),
            Arc::clone(&plugin_store) as _,
            Arc::clone(&plugin_git),
            org_workspace_id.clone(),
        )),
        manifests: Arc::clone(&broker) as _,
        bus: Arc::clone(&bus),
        logs: Arc::new(pagis_plugins::PluginLogs::new(
            crate::boot::plugin_logs_directory(&booted.home),
        )),
    });
    plugin_tools.set(Arc::new(crate::plugin_tools::PluginToolRuntime::new(
        Arc::clone(&plugin_hosts),
    )));
    let plugins = Arc::new(pagis_plugin::Plugins::new(pagis_plugin::PluginsDeps {
        plugins: Arc::clone(&plugin_store) as _,
        connections: Arc::clone(&connections) as _,
        grants: Arc::clone(&grants) as _,
        secrets: Arc::clone(&options.secrets),
        git: Arc::clone(&plugin_git),
        namespaces: Arc::clone(&broker) as _,
        servers: Arc::clone(&plugin_hosts) as _,
        bus: Arc::clone(&bus),
    }));
    // Every installed Plugin offers the tools its state froze, in every
    // tenant's registry. No server starts for it: the catalog is a record
    // (ADR-0005, ADR-0017).
    plugin_hosts.load_every_tenant().await;
    pagis_plugins::spawn_idle_reaper(
        Arc::clone(&plugin_hosts),
        IDLE_REAP_EVERY,
        options.cancel.clone(),
    );
    let materializer = Arc::new(pagis_software::Materializer::new(
        Arc::clone(&computers),
        Arc::clone(&software) as _,
    ));
    software_tools.set(Arc::new(crate::software_tools::SoftwareToolRuntime::new(
        Arc::clone(&software),
        Arc::new(pagis_software::SoftwareRunner::new(
            Arc::clone(&computers),
            materializer,
            Arc::clone(&software) as _,
        )),
        contributions,
    )));
    // Every package of every tenant resolves to its latest Version now.
    // A run that starts later takes that Version and keeps it to the end.
    software
        .refresh_all(&tenants)
        .await
        .map_err(|error| anyhow::anyhow!("the Software List failed to load: {error}"))?;
    broker
        .install_installation_manifest(pagis_vault::manifest())
        .map_err(|error| anyhow::anyhow!("the vault manifest is invalid: {error}"))?;
    broker
        .install_installation_manifest(pagis_google::capability_manifest())
        .map_err(|error| anyhow::anyhow!("the Google manifest is invalid: {error}"))?;
    // The Trigger module reads the installed declarations, so it is
    // built once the manifests are in place.
    let trigger = Arc::new(pagis_trigger::Trigger::new(pagis_trigger::TriggerDeps {
        schedules: Arc::clone(&schedule_store) as _,
        store: Arc::clone(&trigger_store) as _,
        subscriptions: Arc::clone(&subscription_store) as _,
        connections: Arc::clone(&connections) as _,
        agents: Arc::clone(&agent_store) as _,
        channels: Arc::clone(&channels) as _,
        messages: Arc::clone(&messages) as _,
        org_workspace_id: org_workspace_id.clone(),
        grants: Arc::clone(&grants) as _,
        catalog: Arc::new(pagis_broker::WorkspaceEventCatalog::new(Arc::clone(
            &broker,
        ))),
        matchers: std::collections::HashMap::from([
            (
                pagis_mail::MAIL_MATCHER.to_string(),
                Arc::new(pagis_mail::MailMessageMatcher) as Arc<dyn pagis_core::EventMatcher>,
            ),
            (
                pagis_telephony::CALL_MATCHER.to_string(),
                Arc::new(pagis_telephony::CallEndedMatcher) as Arc<dyn pagis_core::EventMatcher>,
            ),
        ]),
        events: Arc::clone(&bus),
        forget_keys: Arc::clone(&tenant_keys) as _,
    }));
    subscription_tools.set(Arc::new(crate::tools::AutomationExecutor::new(
        Arc::new(crate::schedule_tools::ScheduleToolRuntime::new(
            Arc::clone(&trigger),
            Arc::clone(&channels) as _,
            Arc::clone(&options.clock),
            Arc::clone(&tool_runtime),
        )),
        Arc::new(crate::subscription_tools::SubscriptionToolRuntime::new(
            Arc::clone(&trigger),
            Arc::clone(&connections) as _,
            Arc::clone(&channels) as _,
        )),
    )));
    let agents = AgentSystem::start(AgentDeps {
        forget: stores.forget.clone(),
        workspaces: Arc::clone(&workspaces) as _,
        agents: Arc::clone(&agent_store) as _,
        channels: Arc::clone(&channels) as _,
        messages: Arc::clone(&messages) as _,
        runs: Arc::clone(&runs) as _,
        model_aliases: Arc::clone(&model_aliases) as _,
        users: stores.users.clone(),
        usage: stores.usage.clone(),
        capture: Arc::clone(&capture),
        model_request_captures: stores.model_request_captures.clone(),
        participants: Arc::clone(&participants) as _,
        requests: Arc::clone(&requests) as _,
        grants: Arc::clone(&grants) as _,
        connections: Arc::clone(&connections) as _,
        artifacts: Arc::clone(&artifacts) as _,
        blobs: Arc::clone(&blobs),
        memory: Arc::clone(&memory),
        pending_evidence: Arc::clone(&pending_evidence) as _,
        continuations: Arc::clone(&continuations) as _,
        briefs: stores.briefs.clone(),
        schedules: Arc::clone(&schedule_store) as _,
        skills: Arc::clone(&skills) as _,
        computers: Arc::clone(&computers),
        bus: Arc::clone(&bus),
        brain: Arc::clone(&brain),
        models: Arc::clone(&models),
        broker: Arc::clone(&broker),
        tool_runtime,
        hub: Arc::clone(&hub),
        progress: Arc::clone(&progress),
        triggers: Arc::clone(&trigger_store) as _,
        clock: Arc::clone(&options.clock),
        config: options.agents,
    })
    .await;
    spawn_scheduler(
        Arc::clone(&trigger),
        Arc::clone(&pending_evidence) as _,
        Arc::clone(&agents),
        Arc::clone(&mail_blocks),
        Arc::clone(&bus),
        Arc::clone(&options.clock),
        options.cancel.clone(),
    );
    let needs_you = Arc::new(pagis_server::NeedsYou::new(&stores));
    // Each item that enters the Needs-You Queue sends a Notification,
    // and waits while the Person is active in a client (ADR-0030). The
    // task subscribes before the queue task can publish.
    let person_activity = Arc::new(pagis_server::PersonActivity::new());
    let notifications = Arc::new(pagis_server::Notifications::new(
        options.secrets.as_ref(),
        options.public_origin.clone(),
        options.push_policy,
        stores.push_subscriptions.clone(),
        stores.agents.clone(),
        Arc::clone(&needs_you),
        Arc::clone(&options.clock),
    )?);
    pagis_server::spawn_notifications(
        Arc::clone(&notifications),
        Arc::clone(&person_activity),
        Arc::clone(&bus),
        options.cancel.clone(),
    )
    .await;
    // The Needs-You Queue publishes each item that enters or leaves it
    // (ADR-0030). The task reads the baseline of each Workspace before
    // the daemon serves.
    pagis_server::spawn_needs_you(
        Arc::clone(&needs_you),
        Arc::clone(&bus),
        Arc::clone(&options.clock),
        options.cancel.clone(),
    )
    .await?;
    // The Trust List answers the mail collector and the `sender_trust`
    // Page Signal alike (ADR-0011).
    let sender_trust = Arc::new(pagis_mail::ListedSenders::new(
        Arc::clone(&trust_list) as _,
        Arc::clone(&connections) as _,
    ));
    let knowledge_worker = Arc::new(crate::knowledge::Worker {
        bus: Arc::clone(&bus),
        workspaces: Arc::clone(&workspaces) as _,
        store: Arc::clone(&knowledge) as _,
        forget_keys: Arc::clone(&tenant_keys) as _,
        connections: Arc::clone(&connections) as _,
        connection_state: Arc::clone(&connection_state),
        grants: Arc::clone(&grants) as _,
        agents: Arc::clone(&agent_store) as _,
        google: Arc::clone(&google_binder),
        trigger: Arc::clone(&trigger),
        memory: Arc::clone(&memory),
        clock: Arc::clone(&options.clock),
        trust: Arc::clone(&sender_trust) as _,
    });
    let catalogues =
        Arc::clone(&knowledge_worker) as Arc<dyn pagis_core::knowledge::CatalogueProvider>;
    crate::collectors::spawn_collectors(
        Arc::clone(&trigger),
        Arc::clone(&connections) as _,
        Arc::clone(&google_binder),
        knowledge_worker,
        options.collector_interval,
        options.cancel.clone(),
    );
    crate::collectors::spawn_reauth_catchup(
        Arc::clone(&trigger),
        Arc::clone(&connections) as _,
        Arc::clone(&workspaces) as _,
        options.collector_interval,
        options.cancel.clone(),
    );
    // The mailbox desk can now make a Standing Mail Rule, and one task
    // per `active` mailbox watches its INBOX (ADR-0019).
    standing_mail_rule.set(Arc::new(crate::mail_events::StandingMailRules::new(
        Arc::clone(&trigger),
        Arc::clone(&channels) as _,
    )));
    // The number desk can now write a Standing Call Rule, and a call
    // the Agent answers reaches the rule when it settles (ADR-0020).
    standing_call_rule.set(Arc::new(crate::call_events::StandingCallRules::new(
        Arc::clone(&trigger),
        Arc::clone(&channels) as _,
    )));
    call_events.set(Arc::new(crate::call_events::TriggerCallIngest(Arc::clone(
        &trigger,
    ))));
    let mail_collector = Arc::new(pagis_mail::MailCollector::new(
        pagis_mail::MailCollectorDeps {
            workspaces: Arc::clone(&workspaces) as _,
            desk: Arc::clone(&mailbox_desk),
            sent: Arc::clone(&sent_mail) as _,
            trust: Arc::clone(&sender_trust) as _,
            ingest: Arc::new(crate::mail_events::TriggerIngest(Arc::clone(&trigger))),
            idle_turn: pagis_mail::MAX_IDLE_TURN,
            poll_interval: pagis_mail::POLL_INTERVAL,
        },
    ));
    mail_collector.spawn(
        options.collector_interval.min(pagis_mail::SWEEP_INTERVAL),
        options.cancel.clone(),
    );

    let analytics_blocked = spawn_analytics(
        booted,
        options.analytics,
        crate::analytics::InstallationSource {
            system: Arc::clone(&options.system),
            stores: stores.clone(),
            org_id: org.id.clone(),
            installation: match booted.installation() {
                crate::Installation::Local => pagis_analytics::InstallationKind::Local,
                crate::Installation::Server => pagis_analytics::InstallationKind::Server,
            },
            storage: match booted.config.database.url() {
                Some(_) => pagis_analytics::StorageBackend::Postgres,
                None => pagis_analytics::StorageBackend::Sqlite,
            },
            remote_access: options.remote_access,
            docker_discovery: Arc::clone(&options.docker_discovery),
        },
        options.cancel.clone(),
    );

    let remote_access_turn = match (options.remote_access, options.remote_access_turn_listener) {
        (false, _) => None,
        (true, None) => anyhow::bail!(
            "Remote Access is on, and nothing listens on the TURN port {} of the live screen",
            options.remote_access_turn_port
        ),
        (true, Some(listener)) => Some(Arc::new(start_remote_access_turn(
            listener,
            &options.screen,
            options.cancel.child_token(),
        )?)),
    };

    let state = AppState {
        bus,
        channels,
        participants,
        messages,
        runs,
        pending_evidence,
        trigger,
        model_aliases,
        schedules: stores.schedules.clone(),
        usage: stores.usage.clone(),
        capture: Arc::clone(&capture),
        model_request_captures: stores.model_request_captures.clone(),
        onboarding,
        retention_policies,
        connections,
        connector,
        credentials,
        vault,
        requests,
        grants,
        hosts: stores.hosts.clone(),
        host_presence,
        home_exits: Arc::clone(&home_exits),
        host_sessions: Arc::clone(&host_sessions),
        coding_sessions: Arc::clone(&coding_sessions),
        coding_session_store: stores.coding_sessions.clone(),
        broker,
        agent_store,
        mailbox_desk,
        numbers,
        trust_list: Arc::clone(&trust_list) as _,
        live_tiers,
        secrets: Arc::clone(&options.secrets),
        keypad_failures: stores.keypad_failures.clone(),
        needs_you,
        live_calls,
        calls: calls as _,
        texts: texts as _,
        text_transports,
        voice,
        artifacts,
        blobs,
        artifact_max_bytes: options.artifact_max_bytes,
        events,
        memory,
        knowledge,
        catalogues,
        forget: stores.forget.clone(),
        forget_keys: Arc::clone(&tenant_keys) as _,
        computers: Arc::clone(&computers),
        workspaces,
        software: Arc::clone(&software_packages) as _,
        versions: Arc::clone(&software) as _,
        plugins: Arc::clone(&plugins),
        plugin_hosts: Arc::clone(&plugin_hosts),
        org_workspace_id,
        contributions: Arc::clone(&contribution_store) as _,
        orgs: stores.orgs.clone(),
        users: stores.users.clone(),
        sessions: stores.sessions.clone(),
        push_subscriptions: stores.push_subscriptions.clone(),
        notifications,
        person_activity,
        live_connections: pagis_server::LiveConnections::new(Arc::clone(&options.clock)),
        sign_in_links: stores.sign_in_links.clone(),
        sign_in_limits: Arc::new(pagis_server::SignInLimits::default()),
        password_verifier: options.password_verifier,
        // The record the boot made: `Some` on a local installation and
        // `None` on a server (ADR-0025).
        client_credential: booted.client_credential.clone(),
        runtime_port: options.runtime_port,
        port_override: options.port_override,
        supervised: options.supervised,
        started_at: pagis_core::now_ms(),
        analytics_blocked,
        public_origin: options.public_origin,
        // The owner's Client App and the Sign-In Link open a local
        // installation at this origin, whatever the Public Origin names
        // (ADR-0024). A server has no such origin.
        local_origin: booted
            .client_credential
            .is_some()
            .then(|| booted.config.local_origin(options.runtime_port)),
        remote_access: options.remote_access,
        remote_access_switch: pagis_server::RemoteAccessSwitch::new(
            options.tailscale,
            options.remote_access_turn_port,
        ),
        remote_access_turn,
        administration: options.administration,
        proxy: options.proxy,
        screen: options.screen,
        ring,
        hub,
        progress,
        agents,
        keys,
        models,
        docker_discovery: options.docker_discovery,
        // Which backend holds the records. The Administration
        // Interface reports it; nothing branches on it.
        database_backend: match booted.config.database.url() {
            Some(_) => "postgres".to_string(),
            None => "sqlite".to_string(),
        },
        system: options.system,
        restart: options.restart,
        clock: options.clock,
    };
    // A local installation with Remote Access off serves this machine
    // alone, on both ports and for the pages too, whatever a Funnel or a
    // proxy on this machine forwards.
    let this_machine_only = pagis_server::serves_this_machine_only(&state);
    pagis_server::forget::resume(&state).await?;
    // A key of the environment or of `config.toml` comes with no key
    // route, so the voice and call aliases take the providers it serves.
    pagis_server::model_lists::route_unrouted_plumbing(&state)
        .await
        .map_err(|error| anyhow::anyhow!("routing the voice aliases failed: {}", error.message))?;
    let routers = pagis_server::routers(state);
    let mut interfaces = Interfaces {
        product: routers.product.fallback(crate::spa::serve),
        // The administration page is the other entry of the same UI
        // build, served at every address of this port.
        administration: routers
            .administration
            .fallback(crate::spa::serve_administration),
        computers,
        home_exits,
        host_sessions,
        coding_sessions,
    };
    if this_machine_only {
        let guard = || axum::middleware::from_fn(pagis_server::forwarded::refuse_other_machines);
        interfaces.product = interfaces.product.layer(guard());
        interfaces.administration = interfaces.administration.layer(guard());
    }
    Ok(interfaces)
}

/// Start the TURN server of Remote Access on `listener` (ADR-0028). It
/// relays to the Media Relay of `screen` alone: its advertised address,
/// on a port of its range. The server stops with the daemon.
fn start_remote_access_turn(
    listener: tokio::net::TcpListener,
    screen: &pagis_server::ScreenRelay,
    stop: CancellationToken,
) -> anyhow::Result<pagis_computer::RemoteAccessTurn> {
    let address: std::net::IpAddr = screen.advertise_ip.parse().map_err(|_| {
        anyhow::anyhow!(
            "screen.advertise_ip is {:?}; the TURN server of Remote Access relays to the Media \
             Relay at that address, so it is an IP address of this machine",
            screen.advertise_ip
        )
    })?;
    let peers = pagis_computer::MediaRelayPeers::new(address, screen.media_ports.clone());
    let turn = pagis_computer::RemoteAccessTurn::start(listener, peers, stop)?;
    tracing::info!(
        address = %turn.address(),
        "the TURN server of Remote Access carries the live screen to other machines"
    );
    Ok(turn)
}

/// Start the analytics task of a daemon that may send (ADR-0026), and
/// answer why the daemon sends nothing otherwise. The task runs apart
/// from everything else, and nothing waits on it.
fn spawn_analytics(
    booted: &Booted,
    analytics: crate::analytics::AnalyticsOptions,
    source: crate::analytics::InstallationSource,
    cancel: CancellationToken,
) -> Option<pagis_analytics::Blocked> {
    let project = match analytics.destination {
        Ok(project) => project,
        Err(blocked) => {
            tracing::debug!(?blocked, "this daemon sends no analytics");
            return Some(blocked);
        }
    };
    match pagis_analytics::Analytics::new(
        project,
        Arc::new(source),
        booted.home.clone(),
        pagis_server::VERSION,
    ) {
        Ok(task) => {
            tracing::info!(
                "this release build sends anonymous analytics once a day while the Analytics \
                 System Setting is on; DO_NOT_TRACK=1 stops them"
            );
            task.with_timing(analytics.first_after, analytics.check_every)
                .spawn(cancel);
        }
        Err(error) => {
            tracing::warn!(
                error = format!("{error:#}"),
                "the analytics task did not start"
            );
        }
    }
    None
}

/// Settle the purchase intents that outlived the last run. It happens
/// off the boot path: an unreachable carrier must not stop the daemon.
/// The intents are captured before serving requests, so recovery never
/// races a purchase that this daemon is still completing.
fn spawn_purchase_reconciliation(
    numbers: Arc<pagis_telephony::NumberDesk>,
    pending_at_boot: Vec<pagis_core::PurchaseIntent>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        if let Some(Err(error)) = cancel
            .run_until_cancelled(numbers.reconcile_purchases(pending_at_boot))
            .await
        {
            tracing::error!(%error, "reconciling phone number purchases failed");
        }
    });
}

/// Start the carrier's line, which registers the SIP credential once
/// for every number, and prepare the texting of every held number of
/// every Workspace. It happens off the boot path: a registrar that does
/// not answer must not stop the daemon, and each number's page shows
/// the state as it changes.
fn spawn_carrier_start(
    numbers: Arc<pagis_telephony::NumberDesk>,
    workspaces: Arc<dyn WorkspaceStore>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        let started = cancel.run_until_cancelled(async move {
            numbers.start_carrier().await?;
            for workspace in workspaces.list().await? {
                numbers.prepare_assigned_texting(&workspace.id).await?;
            }
            Ok::<(), anyhow::Error>(())
        });
        if let Some(Err(error)) = started.await {
            tracing::error!(%error, "starting the carrier's line failed");
        }
    });
}

/// How often the scheduler looks again for a model key while there is
/// none. Storing a key publishes no event, so the scheduler asks.
const NO_KEY_RECHECK: std::time::Duration = std::time::Duration::from_secs(5);

/// How long the scheduler waits after a pass over the due Schedules
/// failed, before it tries the same pass again.
const SCHEDULER_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// Reconcile the SQLite lease with Git after boot recovery has stopped
/// every old Run. A Git commit is the durable outcome for a changed
/// review; an empty review completes atomically in SQLite and therefore
/// never remains leased here.
async fn recover_pending_reviews(
    pending: &dyn pagis_core::PendingEvidenceStore,
    memory: &dyn pagis_core::MemoryStore,
    now: i64,
) -> anyhow::Result<()> {
    for record in pending.leased().await? {
        let Some(run_id) = record.lease_run_id.as_ref() else {
            continue;
        };
        let revision = record.revision.to_string();
        let after = record
            .after_exclusive
            .as_ref()
            .map_or_else(|| "none".to_string(), ToString::to_string);
        let through = record.through_inclusive.to_string();
        let commit = memory
            .find_commit_with_trailers(
                &record.workspace_id,
                &[
                    ("Run-Id", run_id.as_str()),
                    ("Pagis-Memory-Phase", "reflection"),
                    ("Pending-Review-Id", record.id.as_str()),
                    ("Pending-Review-Revision", revision.as_str()),
                    ("Source-After-Exclusive", after.as_str()),
                    ("Source-Through-Inclusive", through.as_str()),
                ],
            )
            .await?;
        let settled = match commit {
            Some(sha) => {
                pending
                    .complete(
                        &record.workspace_id,
                        run_id,
                        record.revision,
                        Some(&sha),
                        now,
                    )
                    .await?
            }
            None => {
                pending
                    .fail(
                        &record.workspace_id,
                        run_id,
                        record.revision,
                        "daemon restarted",
                        now,
                    )
                    .await?
            }
        };
        anyhow::ensure!(
            settled,
            "pending review {} changed during restart recovery",
            record.id
        );
    }
    Ok(())
}

fn spawn_scheduler(
    trigger: Arc<pagis_trigger::Trigger>,
    pending_evidence: Arc<dyn pagis_core::PendingEvidenceStore>,
    agents: Arc<AgentSystem>,
    mail_blocks: Arc<pagis_mail::MailBlocks>,
    bus: Arc<dyn EventBus>,
    clock: Arc<dyn pagis_core::Clock>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        let mut changes = bus.subscribe(EventScope::Installation, None).await;
        // When both queues are due in one pass, give a waiting Wake-up the
        // next claim. Do not hold either execution pool until the other Run
        // ends: a long Schedule must not block the separate Review slot.
        let mut review_won_last = std::collections::HashSet::new();
        while !cancel.is_cancelled() {
            let now = clock.now_ms();
            // With no model key nothing can think. A due Schedule still
            // becomes a Wake-up, and the Wake-up and every pending
            // review wait until a key is stored: a Run started now would
            // fail and show as work that needs the person.
            if !agents.brain_ready() {
                if let Err(error) = trigger.process_due(now).await {
                    tracing::error!(%error, "processing due Schedules failed");
                }
                tokio::select! {
                    () = tokio::time::sleep(NO_KEY_RECHECK) => {}
                    _ = changes.next() => {}
                    () = cancel.cancelled() => {}
                }
                continue;
            }
            let due_failed = if let Err(error) = trigger.process_due(now).await {
                tracing::error!(%error, "processing due Schedules failed");
                true
            } else {
                false
            };
            // Every Agent with pending work, each with the Workspace
            // that owns it: a claim names the tenant it claims for.
            let wake_agents = match trigger.pending_agents().await {
                Ok(pairs) => pairs,
                Err(error) => {
                    tracing::error!(%error, "loading pending Wake-ups failed");
                    Vec::new()
                }
            };
            let mut pending_agents = wake_agents.clone();
            match pending_evidence.pending_agents(clock.now_ms()).await {
                Ok(review_agents) => {
                    for pair in review_agents {
                        if !pending_agents.contains(&pair) {
                            pending_agents.push(pair);
                        }
                    }
                }
                Err(error) => tracing::error!(%error, "loading pending reviews failed"),
            }
            for (workspace_id, agent_id) in &pending_agents {
                let has_wakeup = wake_agents.iter().any(|(_, held)| held == agent_id);
                let yield_review = has_wakeup && review_won_last.contains(agent_id);
                if !yield_review {
                    let available = agents.available_slots(agent_id).await;
                    // Admit one review per scheduler pass. This gives a Wake-up
                    // that becomes due between passes a chance to claim the
                    // shared Arrival slot before another review batch starts.
                    let review_slots = available.arrival.min(1);
                    match pending_evidence
                        .claim(workspace_id, agent_id, review_slots, clock.now_ms())
                        .await
                    {
                        Ok(claims) => {
                            if !claims.is_empty() {
                                review_won_last.insert(agent_id.clone());
                            }
                            for claim in claims {
                                let _ = bus.publish(pagis_core::NewEvent {
                                workspace_id: claim.run.workspace_id.clone(),
                                event_type: "run.created".to_string(),
                                agent_id: Some(claim.run.agent_id.clone()),
                                run_id: Some(claim.run.id.clone()),
                                channel_id: claim.run.channel_id.clone(),
                                payload: serde_json::json!({
                                    "trigger_kind": claim.run.trigger_kind,
                                    "trigger_ref": claim.run.trigger_ref,
                                    "root_message_id": claim.run.root_message_id.as_ref().map(pagis_core::MessageId::as_str),
                                }),
                            }).await;
                                agents.enqueue_proactive(claim.run);
                            }
                        }
                        Err(error) => {
                            tracing::error!(%error, %agent_id, "claiming pending reviews failed")
                        }
                    }
                }
                let available = agents.available_slots(agent_id).await;
                match trigger
                    .claim_wakeups(workspace_id, agent_id, available, clock.now_ms())
                    .await
                {
                    Ok(claims) => {
                        if !claims.is_empty() {
                            review_won_last.remove(agent_id);
                        }
                        for claim in claims {
                            // The mail that woke the Agent reads as a
                            // line in the Thread before the Run
                            // answers it (ADR-0019).
                            mail_blocks.on_wake(&claim.run).await;
                            agents.enqueue_proactive(claim.run);
                        }
                    }
                    Err(error) => tracing::error!(%error, %agent_id, "claiming Wake-ups failed"),
                }
            }
            let delay = if pending_agents.is_empty() {
                let trigger_due = match trigger.next_due_at().await {
                    Ok(due) => due,
                    Err(error) => {
                        tracing::error!(%error, "loading next Schedule time failed");
                        Some(clock.now_ms() + 1_000)
                    }
                };
                let review_due = match pending_evidence.next_due_at().await {
                    Ok(due) => due,
                    Err(error) => {
                        tracing::error!(%error, "loading next pending review time failed");
                        Some(clock.now_ms() + 1_000)
                    }
                };
                match trigger_due.into_iter().chain(review_due).min() {
                    Some(due_at) => std::time::Duration::from_millis(
                        due_at.saturating_sub(clock.now_ms()).max(1) as u64,
                    ),
                    None => std::time::Duration::from_secs(3600),
                }
            } else {
                std::time::Duration::from_millis(100)
            };
            // A Schedule that stays due keeps the next delay at one
            // millisecond, so a pass that fails would run again at once
            // and log again at once. Back off instead: one line every
            // half second tells the same story without the storm.
            let delay = if due_failed {
                delay.max(SCHEDULER_RETRY_DELAY)
            } else {
                delay
            };
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = trigger.changed() => {}
                _ = changes.next() => {}
                () = clock.changed() => {}
                () = cancel.cancelled() => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The TURN server of Remote Access relays to the address that the
    /// Media Relay advertises, so a name there stops it, and the error
    /// names the setting.
    #[tokio::test]
    async fn an_advertised_name_that_is_not_an_address_stops_the_turn_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let screen = pagis_server::ScreenRelay {
            relay: pagis_server::MediaRelayKind::Daemon,
            advertise_ip: "pagis.example.net".to_string(),
            media_ports: 50000..=50099,
        };

        let error = start_remote_access_turn(listener, &screen, CancellationToken::new())
            .map(|_| ())
            .expect_err("a name is not an address");

        assert!(error.to_string().contains("screen.advertise_ip"), "{error}");
    }

    /// The TURN server starts on the listener it gets, and stops with the
    /// daemon.
    #[tokio::test]
    async fn the_turn_server_starts_on_its_listener_and_stops_with_the_daemon() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();

        let turn = start_remote_access_turn(
            listener,
            &pagis_server::ScreenRelay {
                relay: pagis_server::MediaRelayKind::Daemon,
                advertise_ip: "127.0.0.1".to_string(),
                media_ports: 50000..=50099,
            },
            stop.clone(),
        )
        .expect("the TURN server starts");

        assert_eq!(turn.address(), address);
        assert!(tokio::net::TcpStream::connect(address).await.is_ok());
        stop.cancel();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::net::TcpStream::connect(address).await.is_ok() {
            assert!(tokio::time::Instant::now() < deadline, "it still listens");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}
