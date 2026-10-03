//! Shared test machinery: typed fixture builders, the scripted brain,
//! and the in-process daemon harness. Dev-dependency only; never
//! ships in the binary.

pub mod brain;
pub mod browser;
pub mod connection;
pub mod contribution;
pub mod evaluation;
pub mod exit_client;
pub mod fixture;
pub mod grant;
pub mod host_client;
pub mod plugin;
pub mod postgres;
pub mod software;
pub mod sql;
pub mod store_suite;
pub mod tailscale;
pub mod tenancy;

use std::net::SocketAddr;
use std::sync::Arc;

use pagis_agent::{AgentLoopConfig, Brain};
use pagis_core::{
    ClientKind, MemorySecretStore, ProviderKeys, SESSION_LIFETIME_MS, Session, SessionId, UserId,
    WorkspaceId, now_ms,
};
use pagis_server::RingConfig;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

pub use brain::{Script, ScriptedBrain};
pub use connection::MemoryConnectionStore;
pub use contribution::MemoryContributionStore;
pub use exit_client::ExitClient;
pub use grant::MemoryGrantStore;
pub use host_client::{HostAnswer, HostClient};
pub use plugin::{MemoryPluginStore, MemoryPluginToolStore};
pub use software::MemorySoftwareStore;
pub use tailscale::FakeTailscale;
pub use tenancy::{A_PLUGIN_STDERR, Person, Report, TwoTenants};

/// A Docker search that looks at an empty directory, so no test ever
/// finds the machine's own Docker.
pub fn empty_docker_search() -> pagis_computer::DockerSearch {
    let nowhere = std::path::PathBuf::from("/nonexistent/pagis-test");
    pagis_computer::DockerSearch {
        docker_host: None,
        docker_context: None,
        docker_config_dir: nowhere.join(".docker"),
        home: nowhere.join("home"),
        runtime_dir: None,
        temp_dir: nowhere.join("tmp"),
        root: nowhere,
    }
}

/// A ping that refuses every endpoint, so a test that saves a Docker
/// override sees the daemon refuse it.
pub struct RefusingDockerPing;

#[async_trait::async_trait]
impl pagis_computer::docker::DockerPing for RefusingDockerPing {
    async fn ping(&self, endpoint: &str) -> Result<(), String> {
        Err(format!("no Docker at {endpoint} in a test"))
    }
}

/// A ping that answers for the endpoints it was given, so a test can
/// save a Docker override and read the candidate table.
pub struct ScriptedDockerPing(pub Vec<String>);

#[async_trait::async_trait]
impl pagis_computer::docker::DockerPing for ScriptedDockerPing {
    async fn ping(&self, endpoint: &str) -> Result<(), String> {
        if self.0.iter().any(|answering| answering == endpoint) {
            Ok(())
        } else {
            Err("connection refused".to_string())
        }
    }
}

/// A key resolver detached from the real environment: an in-memory
/// secret store and an injected (default empty) environment.
pub fn test_provider_keys(env: Vec<(&'static str, &'static str)>) -> Arc<ProviderKeys> {
    Arc::new(ProviderKeys::with_env(
        move |name| {
            env.iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| value.to_string())
        },
        std::collections::HashMap::new(),
        Arc::new(MemorySecretStore::default()),
    ))
}

/// Harness knobs. The default brain is an empty [`ScriptedBrain`]: a
/// triggered run fails fast instead of calling a provider. Keys start
/// unconfigured and Docker reads as absent.
pub struct TestDaemonOptions {
    /// Whether a supervisor starts the daemon again after a restart.
    pub supervised: bool,
    /// Which kind of installation the daemon boots as. The default is
    /// a local installation, which holds a Client Credential, as the
    /// daemon the Client App starts does. A server runs on Postgres
    /// alone, so a server test starts through
    /// [`TestDaemon::start_on_postgres_with`].
    pub installation: pagis::Installation,
    /// The interface the daemon binds. The default is loopback,
    /// as a local installation's is; the network test names one of this
    /// machine's own addresses.
    pub bind: std::net::IpAddr,
    /// The origin a browser reaches the daemon at. Empty derives
    /// it from the bind address and the served port, which is what every
    /// test but the reverse-proxy one wants.
    pub public_origin: String,
    /// The address whose forwarded headers the daemon believes.
    /// `None` trusts none, as a local installation does.
    pub trusted_proxy: Option<std::net::IpAddr>,
    /// Whether the daemon runs in Remote Access (ADR-0028). The harness
    /// writes it to the configuration file, as the switch does; a test
    /// names the Public Origin and the Trusted Proxy beside it.
    pub remote_access: bool,
    /// The Tailscale that the Remote Access switch drives. The default
    /// has no `tailscale` command; keep an `Arc` to approve a turn-on and
    /// read the changes it was asked for.
    pub tailscale: Arc<dyn pagis_server::Tailscale>,
    /// The Media Relay the System Settings name. The daemon serves
    /// the screen through the fake loopback relay whatever this says.
    pub screen: pagis_server::ScreenRelay,
    /// The Postgres URL a server runs against, or `None` for SQLite in
    /// the state directory of a local installation. The boot reads it
    /// from the configuration file, so this writes `[database] url`
    /// before it starts.
    pub database: Option<String>,
    pub ring: RingConfig,
    pub brain: Arc<dyn Brain>,
    pub agents: AgentLoopConfig,
    pub keys: Arc<ProviderKeys>,
    /// The base URL every provider's model list is fetched from. `None`
    /// names a closed local port, because no test may reach a real
    /// provider; a test that needs a list names a fake provider here.
    pub model_list_base_url: Option<String>,
    /// Docker discovery (ADR-0024). The default finds nothing, because
    /// no test may reach the machine's Docker. Keep an `Arc` to script
    /// the ping.
    pub docker_discovery: Arc<pagis_computer::DockerDiscovery>,
    /// The artifact upload cap; tests shrink it for the 413 scenario.
    pub artifact_max_bytes: u64,
    /// The computer runtime; a fresh fake without an image by
    /// default. Keep an `Arc` to script it from the test.
    pub computer: Arc<dyn pagis_computer::ComputerRuntime>,
    pub computer_idle_stop: std::time::Duration,
    /// How many Computers may be awake at once. The default is
    /// wide, so only a test about the cap meets it.
    pub computer_awake_caps: pagis_computer::AwakeCaps,
    /// The secret store the daemon runs on. Keep an `Arc` to open a
    /// sealed vault record the way the daemon does.
    pub secrets: Arc<dyn pagis_core::SecretStore>,
    /// The provider factory behind Connections; `None` builds
    /// the Google provider over `gog`, which no test should start.
    pub connection_providers: Option<Arc<dyn pagis::connections::ConnectionProviderFactory>>,
    pub connection_call_timeout: std::time::Duration,
    /// How the connect flow reaches `gog`; `None` starts the
    /// distributed binary, which no test should do.
    pub gog: Option<Arc<dyn pagis_google::GogRunner>>,
    /// Google's OAuth endpoints for the brokered flow. `None`
    /// keeps Google's own, which no test may reach; a test that drives
    /// the callback names a fake token endpoint here.
    pub google_oauth: Option<Arc<pagis_google::GoogleOAuth>>,
    pub authorize_timeout: std::time::Duration,
    /// How often a Connection with a live Event Subscription is polled.
    /// The default is short so a test does not wait a minute.
    pub collector_interval: std::time::Duration,
    /// The number half of the one carrier the test daemon knows, filed
    /// under the Telnyx provider. The default carrier
    /// offers a few San Francisco numbers; keep an `Arc` to script it.
    pub number_catalog: Arc<dyn pagis_telephony::NumberCatalog>,
    /// The text half of that carrier (ADR-0020): a transport that
    /// carries texts in memory, so no network is used. Keep an `Arc`
    /// to push an inbound text, move a delivery state or take texting
    /// away.
    pub text_transport: Arc<pagis_telephony::fake::FakeTextTransport>,
    /// The mailbox half of a Mailbox Provider (ADR-0019). The default
    /// host holds a domain in memory; keep an `Arc` to script it.
    pub mail_host: Arc<dyn pagis_mail::MailboxHost>,
    /// The mail half of a Mailbox Provider (ADR-0019): a transport
    /// that answers from memory, so no socket opens. Keep an `Arc` to
    /// refuse a login and drive a mailbox to `unavailable`.
    pub mail_transport: Arc<dyn pagis_mail::MailTransport>,
    /// How the mailbox login proof waits (ADR-0019). The default gives
    /// up at once, so a test never waits three minutes.
    pub mailbox_proof: pagis_mail::ProofTiming,
    /// The voice seam. The default hears one canned line and has
    /// no realtime socket; keep an `Arc` to script it.
    pub voice: Arc<dyn pagis_voice::VoiceProvider>,
    /// The carrier's call half: a registrar that answers from
    /// memory, so no socket opens. Keep an `Arc` to script it.
    pub call_transport: Arc<dyn pagis_telephony::CallTransport>,
    /// What executes one call: a bridge that answers from a
    /// script. The default answers every call and hangs up. Keep an
    /// `Arc` to read the brief it was handed. `None` lets the daemon
    /// build its realtime bridge.
    pub call_bridge: Option<Arc<dyn pagis_telephony::CallBridge>>,
    /// The password checks of a password sign-in. The default is the
    /// daemon's own argon2id verifier; a test injects a check that counts
    /// or holds its calls.
    pub password_verifier: Arc<pagis_server::PasswordVerifier>,
    /// The logical now of the knowledge worker's background passes.
    /// The default is the machine's wall clock; the evaluation
    /// driver injects the fixture clock of the chronology it replays.
    pub clock: Arc<dyn pagis_core::Clock>,
    /// Where the daemon sends analytics. The default sends nothing, as a
    /// build from source does; the analytics test points it at a fake
    /// PostHog.
    pub analytics: pagis::AnalyticsOptions,
}

impl Default for TestDaemonOptions {
    fn default() -> Self {
        Self {
            supervised: false,
            installation: pagis::Installation::Local,
            bind: std::net::Ipv4Addr::LOCALHOST.into(),
            public_origin: String::new(),
            trusted_proxy: None,
            remote_access: false,
            tailscale: Arc::new(FakeTailscale::not_installed()),
            screen: pagis::Screen::default()
                .screen_relay()
                .expect("the default screen section"),
            database: None,
            ring: RingConfig::default(),
            brain: Arc::new(ScriptedBrain::default()),
            agents: AgentLoopConfig::default(),
            keys: test_provider_keys(Vec::new()),
            model_list_base_url: None,
            docker_discovery: Arc::new(pagis_computer::DockerDiscovery::new(
                empty_docker_search(),
                Arc::new(RefusingDockerPing),
                None,
            )),
            artifact_max_bytes: 50 * 1024 * 1024,
            computer: Arc::new(pagis_computer::fake::FakeComputerRuntime::default()),
            computer_idle_stop: std::time::Duration::from_secs(1800),
            computer_awake_caps: pagis_computer::AwakeCaps {
                per_tenant: 64,
                per_server: 256,
            },
            secrets: Arc::new(pagis_core::MemorySecretStore::default()),
            connection_providers: None,
            connection_call_timeout: pagis::connections::DEFAULT_CALL_TIMEOUT,
            gog: None,
            google_oauth: None,
            authorize_timeout: pagis_connect::DEFAULT_AUTHORIZE_TIMEOUT,
            collector_interval: std::time::Duration::from_millis(100),
            number_catalog: Arc::new(pagis_telephony::fake::FakeNumberCatalog::offering(&[
                "+14155550123",
                "+14155550124",
            ])),
            text_transport: Arc::new(pagis_telephony::fake::FakeTextTransport::default()),
            mail_host: Arc::new(pagis_mail::fake::FakeMailboxHost::default()),
            mail_transport: Arc::new(pagis_mail::fake::FakeMailTransport::default()),
            mailbox_proof: pagis_mail::ProofTiming {
                first_wait: std::time::Duration::from_millis(1),
                max_wait: std::time::Duration::from_millis(1),
                budget: std::time::Duration::ZERO,
            },
            voice: Arc::new(pagis_voice::fake::FakeVoice::default()),
            call_transport: Arc::new(pagis_telephony::fake::FakeCallTransport::new(Arc::new(
                pagis_telephony::fake::TokioClock,
            ))),
            call_bridge: Some(Arc::new(pagis_telephony::FakeCallBridge::reporting(
                pagis_telephony::FakeCallBridge::answered_report(""),
            ))),
            password_verifier: Arc::new(pagis_server::PasswordVerifier::argon2()),
            clock: Arc::new(pagis_core::SystemClock),
            analytics: pagis::AnalyticsOptions::off(),
        }
    }
}

/// The daemon booted in-process on a temp directory and served on an
/// ephemeral port. Dropping it stops the server and deletes the
/// directory.
pub struct TestDaemon {
    pub booted: pagis::Booted,
    pub addr: SocketAddr,
    pub base_url: String,
    /// The Administration Interface's own listener.
    pub administration_addr: SocketAddr,
    /// Where a test reaches the administration port.
    pub administration_base_url: String,
    /// The loopback address of the TURN server of Remote Access
    /// (ADR-0028), on an ephemeral port. It serves while the daemon runs
    /// in Remote Access; otherwise its port is only the one that the
    /// switch names to Tailscale.
    pub remote_access_turn_addr: SocketAddr,
    /// The loopback address of the exit listener of a Server
    /// (ADR-0029), on an ephemeral port, or `None` on a Local
    /// Installation, which opens none.
    pub exit_addr: Option<SocketAddr>,
    /// The exit sockets of the Hosts of this daemon, which carry the
    /// connections of their Person's Computers as the Home Exit.
    pub home_exits: Arc<pagis_computer::HomeExits>,
    /// The origin a browser reaches this daemon at. It is
    /// [`TestDaemon::base_url`] unless the test named one.
    pub public_origin: String,
    /// The seeded DM channel with the default assistant; messages here
    /// trigger the agent loop.
    pub dm_channel_id: String,
    /// The seeded default assistant.
    pub agent_id: String,
    /// The person the seed made: the administrator of the one Org.
    pub user_id: UserId,
    /// The Workspace that person owns. Every by-id store read names it.
    pub workspace_id: WorkspaceId,
    /// The `Cookie` header value of that person's Session.
    session_cookie: String,
    /// The restart the System tab asks for (ADR-0024). A test reads it
    /// instead of watching the process exit.
    pub restart: Arc<pagis_server::RestartSwitch>,
    /// The media hubs of the calls that run now. A test makes a call
    /// live here the way the bridge does.
    pub live_calls: Arc<pagis_telephony::LiveCalls>,
    /// The tier gates of the calls that run now, as the bridge binds
    /// them.
    pub live_tiers: Arc<pagis_telephony::LiveTiers>,
    /// The rows behind the stores, for a test that plants a row no store
    /// method writes.
    rows: crate::sql::Rows,
    /// The SQLite pool of this daemon, or `None` when it runs on
    /// Postgres. A test that names a SQLite store reads it through
    /// [`TestDaemon::pool`].
    sqlite: Option<sqlx::SqlitePool>,
    home: Option<TempDir>,
    server: Option<tokio::task::JoinHandle<()>>,
    administration_server: Option<tokio::task::JoinHandle<()>>,
    /// The stop signal of the daemon's background loops.
    cancel: CancellationToken,
}

impl TestDaemon {
    pub async fn start() -> Self {
        Self::start_with(TestDaemonOptions::default()).await
    }

    /// Start with a custom resume ring, for buffer-overflow scenarios.
    pub async fn start_with_ring(ring: RingConfig) -> Self {
        Self::start_with(TestDaemonOptions {
            ring,
            ..TestDaemonOptions::default()
        })
        .await
    }

    /// Start with a scripted brain and loop config.
    pub async fn start_with(options: TestDaemonOptions) -> Self {
        let home = tempfile::tempdir().expect("create temp home");
        Self::boot_on(home, options).await
    }

    /// Start a server against a fresh Postgres database, or answer
    /// `None` when Docker is not reachable. The reason is already
    /// on stderr, and the caller skips.
    pub async fn start_on_postgres() -> Option<Self> {
        Self::start_on_postgres_with(TestDaemonOptions::default()).await
    }

    /// As [`TestDaemon::start_on_postgres`], with a scripted brain and
    /// loop config. The daemon boots as a server whatever `options`
    /// names, because a server is the installation that runs on
    /// Postgres (ADR-0024).
    pub async fn start_on_postgres_with(options: TestDaemonOptions) -> Option<Self> {
        let database = crate::postgres::database().await?;
        let home = tempfile::tempdir().expect("create temp home");
        Some(
            Self::boot_on(
                home,
                TestDaemonOptions {
                    installation: pagis::Installation::Server,
                    database: Some(database.url),
                    ..options
                },
            )
            .await,
        )
    }

    /// Start on a state directory the test owns, which is how a test
    /// boots a directory a restore just wrote.
    pub async fn start_on(home: TempDir, options: TestDaemonOptions) -> Self {
        Self::boot_on(home, options).await
    }

    /// Stop this daemon and answer its state directory, for a test that
    /// backs the installation up. The instance lock goes with the
    /// daemon, which is what the backup waits for.
    pub async fn stop(mut self) -> TempDir {
        if let Some(server) = self.server.take() {
            server.abort();
        }
        if let Some(server) = self.administration_server.take() {
            server.abort();
        }
        self.cancel.cancel();
        let home = self.home.take().expect("home present");
        drop(self);
        home
    }

    /// Stop this daemon and boot a fresh one on the same home
    /// directory, for restart-recovery scenarios.
    pub async fn restart(mut self, options: TestDaemonOptions) -> Self {
        if let Some(server) = self.server.take() {
            server.abort();
        }
        if let Some(server) = self.administration_server.take() {
            server.abort();
        }
        // Stop the background loops of the old daemon before the new
        // one opens the same home. Two schedulers on one database file
        // contend for the write lock, and the old one claims Wake-ups
        // that the new brain is scripted to answer.
        self.cancel.cancel();
        let home = self.home.take().expect("home present");
        drop(self);
        Self::boot_on(home, options).await
    }

    async fn boot_on(home: TempDir, options: TestDaemonOptions) -> Self {
        // The backend, the bind address, the Public Origin, the Trusted
        // Proxy and Remote Access are settings, so the harness writes them
        // before the boot reads them. None of them decides whether the
        // boot writes a Client Credential: the kind of installation does
        // (ADR-0025).
        let path = home.path().join("config.toml");
        let writes_config = options.database.is_some()
            || !options.bind.is_loopback()
            || !options.public_origin.is_empty()
            || options.trusted_proxy.is_some()
            || options.remote_access;
        if writes_config {
            let mut config = pagis::Config::read_file(&path).expect("read the config");
            if let Some(url) = &options.database {
                config.database.url = url.clone();
            }
            if !options.bind.is_loopback() {
                config.bind = options.bind.to_string();
            }
            config.public_origin = options.public_origin.clone();
            if let Some(proxy) = options.trusted_proxy {
                config.trusted_proxy = proxy.to_string();
            }
            config.remote_access.enabled = options.remote_access;
            config.save(&path).expect("write the config");
        }
        let booted = pagis::boot(home.path(), options.installation)
            .await
            .expect("boot daemon");
        let restart = Arc::new(pagis_server::RestartSwitch::default());
        let cancel = CancellationToken::new();
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(options.bind, 0))
            .await
            .unwrap_or_else(|error| panic!("bind an ephemeral port on {}: {error}", options.bind));
        let addr = listener.local_addr().expect("local addr");
        let public_origin = if options.public_origin.is_empty() {
            format!("http://{addr}")
        } else {
            options.public_origin.clone()
        };
        // The Administration Interface answers on its own listener,
        // on an ephemeral port of the same interface, so a test
        // drives both ports of one daemon.
        let administration_listener =
            tokio::net::TcpListener::bind(SocketAddr::new(options.bind, 0))
                .await
                .unwrap_or_else(|error| {
                    panic!(
                        "bind an ephemeral administration port on {}: {error}",
                        options.bind
                    )
                });
        let administration_addr = administration_listener.local_addr().expect("local addr");
        // The TURN server of Remote Access binds loopback whatever the
        // daemon binds, as the production one does.
        let remote_access_turn_listener =
            tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .expect("bind an ephemeral TURN port");
        let remote_access_turn_addr = remote_access_turn_listener
            .local_addr()
            .expect("local addr");
        // The exit listener of a Server (ADR-0029), on loopback. A Local
        // Installation opens none.
        let exit_listener = match options.installation {
            pagis::Installation::Server => Some(
                tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                    .await
                    .expect("bind an ephemeral exit port"),
            ),
            pagis::Installation::Local => None,
        };
        let exit_addr = exit_listener
            .as_ref()
            .map(|listener| listener.local_addr().expect("local addr"));
        let live_calls = Arc::new(pagis_telephony::LiveCalls::default());
        let live_tiers = Arc::new(pagis_telephony::LiveTiers::default());
        let interfaces = pagis::app(
            &booted,
            pagis::AppOptions {
                runtime_port: addr.port(),
                port_override: None,
                supervised: options.supervised,
                public_origin: public_origin.clone(),
                administration: pagis_server::AdministrationAddress::of(administration_addr),
                proxy: match options.trusted_proxy {
                    Some(address) => pagis_server::TrustedProxy::at(address),
                    None => pagis_server::TrustedProxy::none(),
                },
                remote_access: options.remote_access,
                tailscale: options.tailscale,
                remote_access_turn_port: remote_access_turn_addr.port(),
                remote_access_turn_listener: options
                    .remote_access
                    .then_some(remote_access_turn_listener),
                exit_listener,
                ring: options.ring,
                brain: Some(options.brain),
                agents: options.agents,
                keys: options.keys,
                model_list_base_urls: pagis_core::PROVIDERS
                    .into_iter()
                    .map(|provider| {
                        (
                            provider,
                            options
                                .model_list_base_url
                                .clone()
                                .unwrap_or_else(|| "http://127.0.0.1:1".to_string()),
                        )
                    })
                    .collect(),
                docker_discovery: Arc::clone(&options.docker_discovery),
                system: Arc::new(pagis::FileSystemConfig::new(home.path())),
                restart: Arc::clone(&restart),
                artifact_max_bytes: options.artifact_max_bytes,
                computer: Some(options.computer),
                computer_idle_stop: options.computer_idle_stop,
                computer_limits: pagis_computer::ComputerLimits::default(),
                computer_awake_caps: options.computer_awake_caps,
                media_relay: pagis_computer::fake::loopback_relay(),
                screen: options.screen,
                secrets: Arc::clone(&options.secrets),
                connection_providers: options.connection_providers,
                connection_call_timeout: options.connection_call_timeout,
                google_oauth: options.google_oauth,
                gog: options.gog,
                authorize_timeout: options.authorize_timeout,
                collector_interval: options.collector_interval,
                number_catalogs: Some(Arc::new(pagis_telephony::NumberCatalogs::single(
                    pagis_telephony::TELNYX_PROVIDER,
                    options.number_catalog,
                ))),
                text_transports: Some(Arc::new(pagis_telephony::TextTransports::single(
                    pagis_telephony::TELNYX_PROVIDER,
                    options.text_transport,
                ))),
                mail_host: Some(options.mail_host),
                mail_transport: Some(options.mail_transport),
                mailbox_proof: options.mailbox_proof,
                voice: Some(options.voice),
                call_transport: Some(options.call_transport),
                call_bridge: options.call_bridge,
                live_calls: Some(Arc::clone(&live_calls)),
                live_tiers: Some(Arc::clone(&live_tiers)),
                password_verifier: options.password_verifier,
                clock: options.clock,
                cancel: cancel.clone(),
                analytics: options.analytics,
            },
        )
        .await
        .expect("assemble app");

        let workspace = booted
            .stores
            .workspaces
            .list()
            .await
            .expect("list workspaces")
            .into_iter()
            .next()
            .expect("seeded workspace");
        let dm_channel_id = booted
            .stores
            .channels
            .list_by_workspace(&workspace.id)
            .await
            .expect("list channels")
            .into_iter()
            .next()
            .expect("seeded DM channel")
            .id
            .to_string();
        let agent_id = booted
            .stores
            .agents
            .list_by_workspace(&workspace.id)
            .await
            .expect("list agents")
            .into_iter()
            .next()
            .expect("seeded assistant")
            .id
            .to_string();

        let home_exits = Arc::clone(&interfaces.home_exits);
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                interfaces
                    .product
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("serve");
        });
        let administration_server = tokio::spawn(async move {
            axum::serve(
                administration_listener,
                interfaces
                    .administration
                    .into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("serve the administration interface");
        });

        let session_cookie = open_session(&booted.stores, &workspace.user_id).await;

        // A test that plants a row directly needs the rows of whichever
        // backend the daemon opened. On SQLite the harness opens its own
        // pool on the same file: WAL carries a second reader and writer.
        let (rows, sqlite) = match &options.database {
            Some(url) => (
                crate::sql::Rows::Postgres(
                    pagis_storage_postgres::connect(url)
                        .await
                        .expect("open the test Postgres"),
                ),
                None,
            ),
            None => {
                let pool = pagis_storage_sqlite::connect(&home.path().join("pagis.db"))
                    .await
                    .expect("open the test database");
                (crate::sql::Rows::Sqlite(pool.clone()), Some(pool))
            }
        };

        Self {
            booted,
            addr,
            base_url: format!("http://{addr}"),
            administration_addr,
            administration_base_url: format!("http://{administration_addr}"),
            remote_access_turn_addr,
            exit_addr,
            home_exits,
            public_origin,
            dm_channel_id,
            agent_id,
            user_id: workspace.user_id.clone(),
            workspace_id: workspace.id.clone(),
            session_cookie,
            restart,
            live_calls,
            live_tiers,
            rows,
            sqlite,
            home: Some(home),
            server: Some(server),
            administration_server: Some(administration_server),
            cancel,
        }
    }

    /// Every repository of this daemon, behind its trait. A test reads
    /// records through these and names no backend.
    pub fn stores(&self) -> &pagis_core::Stores {
        &self.booted.stores
    }

    /// The rows behind those stores, for a test that plants a row no
    /// store method writes.
    pub fn rows(&self) -> &crate::sql::Rows {
        &self.rows
    }

    /// The SQLite pool of this daemon.
    ///
    /// A test that builds a `Sqlite*` store or writes SQLite SQL calls
    /// this. It panics on a daemon that runs on Postgres, which is what
    /// a test that must run on both backends wants to hear: it reads
    /// [`TestDaemon::stores`] or [`TestDaemon::rows`] instead.
    pub fn pool(&self) -> &sqlx::SqlitePool {
        self.sqlite
            .as_ref()
            .expect("this daemon runs on Postgres; read `stores()` or `rows()` instead")
    }

    /// The Client Credential of this local installation.
    pub fn client_credential(&self) -> &str {
        self.booted
            .client_credential
            .as_deref()
            .expect("this daemon is a server and holds no Client Credential")
    }

    /// The `Cookie` header of the seeded person's Session. Every
    /// authenticated request carries it.
    pub fn cookie(&self) -> &str {
        &self.session_cookie
    }

    /// Set up one installation part of a provider the way an
    /// Administrator does, on the administration port, and answer the
    /// status and the provider's setup. A carrier account or a mail
    /// domain is an Installation Connection, and a person never creates
    /// one on the product port.
    pub async fn set_up_provider(
        &self,
        provider: &str,
        part: &str,
        fields: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let response = reqwest::Client::new()
            .put(format!(
                "{}/api/v1/administration/providers/{provider}/{part}",
                self.administration_base_url
            ))
            .header("cookie", self.cookie())
            .json(&serde_json::json!({ "fields": fields }))
            .send()
            .await
            .expect("the administration port answers");
        let status = response.status().as_u16();
        (
            status,
            response.json().await.unwrap_or(serde_json::Value::Null),
        )
    }

    /// Connect one Installation Connection and answer its id. The test
    /// fails when the provider refuses the fields.
    pub async fn connect_installation(&self, provider: &str, fields: serde_json::Value) -> String {
        let (status, setup) = self.set_up_provider(provider, "connection", fields).await;
        assert_eq!(status, 200, "{setup}");
        setup["parts"]
            .as_array()
            .and_then(|parts| parts.iter().find(|part| part["id"] == "connection"))
            .and_then(|part| part["connection_id"].as_str())
            .unwrap_or_else(|| panic!("{provider} names no connection: {setup}"))
            .to_string()
    }

    /// Open a Session for another person and answer its `Cookie` header,
    /// so a test can act as a second person in the Org.
    pub async fn cookie_for(&self, user_id: &UserId) -> String {
        open_session(&self.booted.stores, user_id).await
    }

    /// Sign the Session of `cookie` out, as the Product App does. The
    /// test fails when the daemon refuses.
    pub async fn sign_out(&self, cookie: &str) {
        let response = reqwest::Client::new()
            .delete(format!("{}/api/v1/sessions/current", self.base_url))
            .header("cookie", cookie)
            .send()
            .await
            .expect("the daemon answers the sign-out");
        assert_eq!(response.status(), 204, "the sign-out is refused");
    }

    /// An event socket of one Session, authenticated and past its
    /// `ready` frame.
    pub async fn event_socket(&self, cookie: &str) -> Socket {
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let (mut socket, _) =
            tokio_tungstenite::connect_async(self.ws_request_as(&self.ws_url(), cookie))
                .await
                .expect("the event socket opens");
        socket
            .send(Message::text(
                serde_json::json!({ "type": "auth" }).to_string(),
            ))
            .await
            .expect("the auth frame goes out");
        let ready = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
            .await
            .expect("a frame before the timeout")
            .expect("the socket stays open")
            .expect("a readable frame");
        assert_eq!(
            ready.into_text().expect("a text frame").as_str(),
            r#"{"type":"ready"}"#,
            "the socket is not ready"
        );
        socket
    }

    /// Stop the daemon's background loops. The server stays up,
    /// so a test can still write through the API and then prove that
    /// no loop acts on what it wrote.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The WebSocket endpoint URL.
    pub fn ws_url(&self) -> String {
        format!("ws://{}/api/v1/ws", self.addr)
    }

    /// A WebSocket handshake that carries the session cookie. A
    /// socket authenticates exactly as a REST call does.
    pub fn ws_request(&self, url: &str) -> tokio_tungstenite::tungstenite::http::Request<()> {
        self.ws_request_as(url, self.cookie())
    }

    /// The same handshake for another person's Session, so a test can
    /// hold two sockets of two tenants at once.
    pub fn ws_request_as(
        &self,
        url: &str,
        cookie: &str,
    ) -> tokio_tungstenite::tungstenite::http::Request<()> {
        use tokio_tungstenite::tungstenite::handshake::client::generate_key;
        tokio_tungstenite::tungstenite::http::Request::builder()
            .method("GET")
            .uri(url)
            .header("host", self.addr.to_string())
            .header("connection", "Upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", generate_key())
            .header("cookie", cookie)
            .body(())
            .expect("a websocket handshake request")
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
        if let Some(server) = &self.administration_server {
            server.abort();
        }
        self.cancel.cancel();
    }
}

/// A WebSocket of the test daemon, as a client holds it.
pub type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// How the daemon ended one socket.
#[derive(Debug)]
pub struct Closed {
    /// The code of the daemon's Close frame.
    pub code: u16,
    /// Each text frame the daemon sent before the Close, in order.
    pub frames: Vec<String>,
}

/// Read `socket` until the daemon closes it, and answer the Close with
/// the text frames that came before it. Binary frames, which carry
/// audio, are not kept. The test fails when the socket stays open for
/// five seconds, or when the connection ends with no Close frame.
pub async fn read_until_closed(socket: &mut Socket) -> Closed {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let next = tokio::time::timeout_at(deadline, socket.next())
            .await
            .unwrap_or_else(|_| {
                panic!("the socket is still open after five seconds; it sent {frames:?}")
            });
        match next {
            Some(Ok(Message::Text(text))) => frames.push(text.to_string()),
            Some(Ok(Message::Close(Some(close)))) => {
                return Closed {
                    code: close.code.into(),
                    frames,
                };
            }
            Some(Ok(Message::Close(None))) => panic!("the daemon closed the socket with no code"),
            Some(Ok(_)) => {}
            Some(Err(error)) => panic!("the socket ended with no Close frame: {error}"),
            None => panic!("the socket ended with no Close frame; it sent {frames:?}"),
        }
    }
}

/// Write a Session for one person and answer the `Cookie` header that
/// carries it. The record holds the hash alone, as the daemon writes it.
async fn open_session(stores: &pagis_core::Stores, user_id: &UserId) -> String {
    let secret = pagis_server::random_secret();
    let now = now_ms();
    stores
        .sessions
        .create(&Session {
            id: SessionId::generate(),
            user_id: user_id.clone(),
            token_hash: pagis_server::hash_secret(&secret),
            client_kind: ClientKind::Browser,
            client_name: None,
            created_at: now,
            last_used_at: now,
            expires_at: now + SESSION_LIFETIME_MS,
        })
        .await
        .expect("open a test session");
    format!("{}={secret}", pagis_server::SESSION_COOKIE)
}
