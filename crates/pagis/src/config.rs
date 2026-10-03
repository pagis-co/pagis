use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use pagis_core::Provider;
use pagis_server::{MediaRelayKind, origin_host_is_loopback};
use serde::{Deserialize, Serialize};

use crate::boot::Installation;

pub const DEFAULT_PORT: u16 = 4400;
/// The interface the listener binds. A local installation is
/// reached from its own machine alone, so the default is loopback and an
/// installation that serves a network names its address on purpose.
pub const DEFAULT_BIND: &str = "127.0.0.1";
/// The port the Administration Interface binds. It sits next to
/// the product port so a deployment reads the pair at a glance.
pub const DEFAULT_ADMINISTRATION_PORT: u16 = 4401;
/// The interface the Administration Interface binds. It is
/// loopback whatever the product port binds, because the point of the
/// second port is that it need not be reachable from the network at all:
/// an administrator reaches it over an SSH tunnel, or names an address
/// here on purpose.
pub const DEFAULT_ADMINISTRATION_BIND: &str = "127.0.0.1";
pub const DEFAULT_ARTIFACT_MAX_BYTES: u64 = 50 * 1024 * 1024;
pub const DEFAULT_COMPUTER_IDLE_STOP_MINUTES: u64 = 30;
/// The level of the daemon's own crates (ADR-0024); every other crate
/// logs warnings and errors alone (`logging::filter`). `PAGIS_LOG`
/// wins over it, for one run under a finer filter.
pub const DEFAULT_LOG_LEVEL: &str = "info";
/// Where a non-macOS installation reads the Installation Key from.
/// A Docker secret and a Kubernetes secret both mount at
/// `/run/secrets/<name>`, and a systemd unit can put a file there too.
pub const DEFAULT_SECRETS_KEY_FILE: &str = "/run/secrets/pagis-secrets-key";
/// The Media Relay's UDP port range. One port serves one viewer
/// session, so a hundred ports are a hundred people watching a screen
/// at once. The range sits below the ephemeral ports every operating
/// system hands out, so nothing else takes a port of it first.
pub const DEFAULT_MEDIA_PORT_FIRST: u16 = 50000;
pub const DEFAULT_MEDIA_PORT_LAST: u16 = 50099;
/// How long a minted TURN credential lives.
pub const DEFAULT_TURN_TTL_SECONDS: u64 = 3600;
/// The loopback port of the TURN server of Remote Access (ADR-0028). It
/// sits next to the product port and the administration port. Funnel
/// publishes it on port 8443, and the turn-on names it in the Funnel
/// configuration, so it is fixed and never random.
pub const DEFAULT_REMOTE_ACCESS_TURN_PORT: u16 = 4402;
/// The TCP port of the exit listener of a Server (ADR-0029). It sits
/// next to the other ports of the daemon, and the egress rules of the
/// deployment open it to the Computers, so it is fixed and never random.
pub const DEFAULT_COMPUTER_EXIT_PORT: u16 = 4403;
/// The Media Relay implementations `screen.relay` names.
pub const RELAY_DAEMON: &str = "daemon";
pub const RELAY_TURN: &str = "turn";
/// The variable the headless image sets. The daemon then refuses to
/// start as a local installation, or without a Public Origin that people
/// on other machines can open.
pub const REQUIRE_PUBLIC_ORIGIN: &str = "PAGIS_REQUIRE_PUBLIC_ORIGIN";
/// The variable that turns Remote Access on or off for one run, over
/// `[remote_access] enabled` of `config.toml` (ADR-0028). A Local
/// Installation and a Server both read it.
pub const REMOTE_ACCESS: &str = "PAGIS_REMOTE_ACCESS";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub port: u16,
    /// The interface the listener binds: loopback by default,
    /// this machine's LAN or private address to serve a network, and
    /// `0.0.0.0` to serve every interface.
    pub bind: String,
    /// The origin a browser reaches this installation at, such as
    /// `https://pagis.example.net`. The CORS answer names it, and
    /// the one-time sign-in link starts at it. Empty derives it from the
    /// bind address and the port, which is what a local installation and
    /// a plain LAN installation want.
    pub public_origin: String,
    /// The address of the reverse proxy in front of the daemon.
    /// The daemon believes that address's `X-Forwarded-For` and
    /// `X-Forwarded-Proto` and nobody else's. Empty believes no
    /// forwarded header, which is the local installation.
    pub trusted_proxy: String,
    /// Remote Access (ADR-0028). `PAGIS_REMOTE_ACCESS` overrides it.
    pub remote_access: RemoteAccess,
    /// The Administration Interface's own listener.
    pub administration: Administration,
    /// The artifact upload size cap, 50 MB by default.
    pub artifact_max_bytes: u64,
    /// What one agent computer may take, and how many may be awake.
    pub computer: Computer,
    /// The Docker endpoint the user typed in System settings
    /// (ADR-0024). Empty gives discovery the choice.
    pub docker_endpoint: String,
    /// The tracing filter (ADR-0024): `trace`, `debug`, `info`, `warn`
    /// or `error`.
    pub log_level: String,
    /// Whether a release build sends anonymous analytics (ADR-0026). On
    /// by default; an Administrator turns it off in System Settings.
    pub analytics: bool,
    pub screen: Screen,
    pub providers: Providers,
    pub database: Database,
    pub secrets: Secrets,
}

/// Remote Access (ADR-0028): the installation serves other machines at
/// the public name of the owner's Tailscale Funnel, where another machine
/// signs in with a Sign-In Link and never with a password. The switch of
/// a Local Installation writes it with the Public Origin and the Trusted
/// Proxy. The deployment of a Server sets `PAGIS_REMOTE_ACCESS`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteAccess {
    pub enabled: bool,
}

/// The Administration Interface's own listener.
///
/// The product port can face the team while this one stays private, so
/// the two are configured apart: this section names the port and the
/// interface of the administration listener alone. The default binds
/// loopback on a server as on a local installation, which is the
/// setting that makes the surface private by default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Administration {
    pub port: u16,
    /// The interface this listener binds: loopback by default, a private
    /// address of this machine to reach it from an administrator's own
    /// network.
    pub bind: String,
}

impl Default for Administration {
    fn default() -> Self {
        Self {
            port: DEFAULT_ADMINISTRATION_PORT,
            bind: DEFAULT_ADMINISTRATION_BIND.to_string(),
        }
    }
}

impl Administration {
    /// The interface the administration listener binds. An empty
    /// entry is loopback; anything that is not an IP address stops the
    /// daemon at boot rather than at the first request.
    pub fn bind_address(&self) -> anyhow::Result<IpAddr> {
        parse_bind(
            &self.bind,
            DEFAULT_ADMINISTRATION_BIND,
            "administration.bind",
        )
    }
}

/// One bind setting as an address. An empty entry is the default.
fn parse_bind(bind: &str, default: &str, key: &str) -> anyhow::Result<IpAddr> {
    let bind = bind.trim();
    if bind.is_empty() {
        return Ok(default.parse().expect("the default bind is an address"));
    }
    bind.parse().map_err(|_| {
        anyhow::anyhow!(
            "{key} is {bind:?}; it is an IP address of this machine, \
             such as {default:?} or \"0.0.0.0\""
        )
    })
}

/// One `PAGIS_*` switch: `true` or `1` turns it on, and `false`, `0` or
/// an empty value turns it off. Anything else stops the daemon and names
/// the variable.
fn parse_flag(key: &str, value: &str) -> anyhow::Result<bool> {
    match value.trim() {
        "" | "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        other => anyhow::bail!("{key} is {other:?}; it is \"true\" or \"false\""),
    }
}

/// One `PAGIS_*` port override, named in the failure so the deployment
/// reads which variable it mistyped.
fn parse_env(key: &str, value: &str) -> anyhow::Result<u16> {
    value
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("{key} is {value:?}; it is a port number"))
}

/// Where a server on Linux reads the Installation Key (ADR-0013).
///
/// The key seals `secrets.enc`, so it never sits beside that file and it
/// never sits in this one. The daemon reads the named file once at
/// start, and it refuses a file any group or any other user can read.
/// A server on macOS ignores the setting, because the key is one
/// keychain item there. A local installation ignores it too: it keeps
/// the key in the keychain on macOS or in the Secret Service on Linux,
/// or in a Key File it generates in the state directory.
///
/// A file is what every service manager, Docker and Kubernetes deliver
/// the same way. An environment variable leaks into `ps` output and
/// into crash reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Secrets {
    /// The path of the 64-hexadecimal-character key file.
    pub key_file: String,
}

impl Default for Secrets {
    fn default() -> Self {
        Self {
            key_file: DEFAULT_SECRETS_KEY_FILE.to_string(),
        }
    }
}

impl Secrets {
    /// The path the daemon reads the Installation Key from.
    pub fn key_file(&self) -> &Path {
        let path = self.key_file.trim();
        Path::new(if path.is_empty() {
            DEFAULT_SECRETS_KEY_FILE
        } else {
            path
        })
    }
}

/// Where the daemon keeps its records.
///
/// A local installation sets nothing: one SQLite file in the state
/// directory, and no service to run, which is what makes the local
/// product one signed binary. A server sets `url` to a Postgres
/// database, which gives a team of people one writer each instead of
/// one writer for the installation, and the backup and replication
/// tooling a team already knows. [`Config::check_database`] holds each
/// kind of installation to its backend (ADR-0024).
///
/// The files stay on the server's disk either way: the memory
/// repositories, the artifacts, the recordings, the sealed secrets, the
/// Plugins and the Software. Postgres holds the records, not the files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Database {
    /// The Postgres URL of a server, such as
    /// `postgres://pagis:secret@db.internal/pagis`. A local
    /// installation leaves it empty.
    pub url: String,
}

impl Database {
    /// The Postgres URL the user set, if any. An empty entry in the
    /// file means SQLite.
    pub fn url(&self) -> Option<&str> {
        let url = self.url.trim();
        (!url.is_empty()).then_some(url)
    }
}

/// What one agent computer may take of the server, and how many of
/// them may be awake at once (ADR-0014).
///
/// The container is a boundary between tenants, so these are the
/// figures that keep one tenant from taking the machine every other
/// tenant runs on. The defaults are what one desktop with a browser
/// needs; a server with more people or less memory moves them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Computer {
    /// Minutes an awake agent computer sits idle before it stops.
    pub idle_stop_minutes: u64,
    /// How many Computers one tenant may hold awake at once.
    pub awake_per_tenant: u32,
    /// How many Computers the whole server may hold awake at once.
    pub awake_per_server: u32,
    /// The memory ceiling of one container, in mebibytes.
    pub memory_mb: u64,
    /// The CPU share of one container, in CPUs.
    pub cpus: f64,
    /// How many processes and threads one container may hold.
    pub pids: u32,
    /// The `nofile` ulimit of every process in a container.
    pub open_files: u32,
    /// `/dev/shm` of one container, in mebibytes. Chromium starves
    /// under Docker's 64 MB default.
    pub shm_mb: u64,
    /// The quota of one Agent volume, in gibibytes. Zero asks for no
    /// quota, and a Docker daemon whose filesystem carries no project
    /// quotas gives none either way.
    pub volume_gb: u64,
    /// The size of the writable container layer of one Computer, in
    /// gibibytes: every write outside `/data`. Zero asks for no size,
    /// and a Docker storage driver that holds no layer to a size gives
    /// none either way.
    pub layer_gb: u64,
    /// The TCP port of the exit listener of a Server (ADR-0029). The
    /// Exit Proxy of each Agent's Computer in Home mode sends each
    /// connection there, and the egress rules open it to the Computers.
    /// A Local Installation opens no exit listener.
    pub exit_port: u16,
    /// The Home Exit System Setting of a Server (ADR-0029): whether its
    /// People may send their Computers' connections through a Home Exit.
    /// On by default; an Administrator turns it off for the installation
    /// in System Settings. A Local Installation has no Home Exit.
    pub home_exit: bool,
}

impl Default for Computer {
    fn default() -> Self {
        Self {
            idle_stop_minutes: DEFAULT_COMPUTER_IDLE_STOP_MINUTES,
            awake_per_tenant: 3,
            awake_per_server: 24,
            memory_mb: 4096,
            cpus: 2.0,
            pids: 512,
            open_files: 8192,
            shm_mb: 512,
            volume_gb: 10,
            layer_gb: 10,
            exit_port: DEFAULT_COMPUTER_EXIT_PORT,
            home_exit: true,
        }
    }
}

impl Computer {
    /// What one container may take, as the Computer runtime asks for it.
    pub fn limits(&self) -> pagis_computer::ComputerLimits {
        pagis_computer::ComputerLimits {
            memory_bytes: (self.memory_mb * 1024 * 1024) as i64,
            nano_cpus: (self.cpus * 1_000_000_000.0) as i64,
            pids: self.pids as i64,
            open_files: self.open_files as i64,
            shm_bytes: (self.shm_mb * 1024 * 1024) as i64,
            volume_bytes: (self.volume_gb > 0).then(|| self.volume_gb * 1024 * 1024 * 1024),
            layer_bytes: (self.layer_gb > 0).then(|| self.layer_gb * 1024 * 1024 * 1024),
        }
    }

    /// How many Computers may be awake at once.
    pub fn caps(&self) -> pagis_computer::AwakeCaps {
        pagis_computer::AwakeCaps {
            per_tenant: self.awake_per_tenant,
            per_server: self.awake_per_server,
        }
    }

    pub fn idle_stop(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.idle_stop_minutes * 60)
    }

    /// The port of the exit listener. Zero would be a random port, which
    /// the egress rules cannot name.
    pub fn exit_port(&self) -> anyhow::Result<u16> {
        anyhow::ensure!(
            self.exit_port > 0,
            "computer.exit_port is 0; it is the fixed port of the exit listener that the egress \
             rules open to the Computers, such as {DEFAULT_COMPUTER_EXIT_PORT}"
        );
        Ok(self.exit_port)
    }
}

/// The live screen view and the Media Relay behind it
/// (ADR-0014).
///
/// A Computer publishes media on the daemon's loopback and on no host
/// interface, so a browser reaches a screen through the relay and never
/// through a container. One relay serves every Computer of the
/// installation, so a deployment opens one address and one UDP range in
/// its firewall and names them here once. A local installation sets
/// nothing: the relay runs on loopback and the browser is on the same
/// machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Screen {
    /// The address browsers reach the relay at. Set it to this
    /// machine's LAN or public IP for remote viewers; a `turn`
    /// deployment sets the address its TURN server reaches the daemon
    /// at.
    pub advertise_ip: String,
    /// The first UDP port of the relay's range.
    pub media_port_first: u16,
    /// The last UDP port of the relay's range. One port serves one
    /// viewer session, so the range bounds how many people watch a
    /// screen at once.
    pub media_port_last: u16,
    /// Which Media Relay runs: `daemon` forwards media in the daemon
    /// itself and needs no other service; `turn` puts an external TURN
    /// server in front of the browser leg.
    pub relay: String,
    /// The loopback TCP port of the TURN server that carries the live
    /// screen to another machine in Remote Access (ADR-0028). The daemon
    /// listens on it while Remote Access is on, and Funnel publishes it on
    /// port 8443.
    pub remote_access_turn_port: u16,
    /// The TURN server the `turn` relay uses.
    pub turn: Turn,
}

/// The external TURN server of the `turn` relay.
///
/// The daemon mints one credential for each viewer session under the
/// TURN REST API long-term credential scheme, which is coturn's
/// `use-auth-secret` mode: no account exists on the TURN server, and a
/// credential that leaks expires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Turn {
    /// The URLs a browser tries, such as
    /// `turn:relay.example.net:3478?transport=udp`.
    pub urls: Vec<String>,
    /// The shared secret of the TURN server's `static-auth-secret`.
    pub secret: String,
    /// How long a minted credential lives.
    pub ttl_seconds: u64,
}

impl Default for Turn {
    fn default() -> Self {
        Self {
            urls: Vec::new(),
            secret: String::new(),
            ttl_seconds: DEFAULT_TURN_TTL_SECONDS,
        }
    }
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            advertise_ip: "127.0.0.1".to_string(),
            media_port_first: DEFAULT_MEDIA_PORT_FIRST,
            media_port_last: DEFAULT_MEDIA_PORT_LAST,
            relay: RELAY_DAEMON.to_string(),
            remote_access_turn_port: DEFAULT_REMOTE_ACCESS_TURN_PORT,
            turn: Turn::default(),
        }
    }
}

impl Screen {
    /// The Media Relay this installation runs. The screen path
    /// holds the answer and knows nothing else about the media path.
    pub fn media_relay(&self) -> anyhow::Result<Arc<dyn pagis_computer::MediaRelay>> {
        anyhow::ensure!(
            self.media_port_first > 0 && self.media_port_first <= self.media_port_last,
            "screen.media_port_first..media_port_last is not a UDP range: {}-{}",
            self.media_port_first,
            self.media_port_last
        );
        let forwarder = pagis_computer::MediaForwarder::new(
            self.advertise_ip.trim().to_string(),
            self.media_port_first..=self.media_port_last,
        );
        match self.relay_kind()? {
            MediaRelayKind::Daemon => Ok(Arc::new(pagis_computer::DaemonRelay::new(forwarder))),
            MediaRelayKind::Turn => {
                let urls: Vec<String> = self
                    .turn
                    .urls
                    .iter()
                    .map(|url| url.trim().to_string())
                    .filter(|url| !url.is_empty())
                    .collect();
                anyhow::ensure!(
                    !urls.is_empty(),
                    "screen.relay is \"turn\" and screen.turn.urls is empty"
                );
                anyhow::ensure!(
                    !self.turn.secret.trim().is_empty(),
                    "screen.relay is \"turn\" and screen.turn.secret is empty"
                );
                Ok(Arc::new(pagis_computer::TurnRelay::new(
                    forwarder,
                    pagis_computer::TurnServer {
                        urls,
                        secret: self.turn.secret.trim().to_string(),
                        ttl: std::time::Duration::from_secs(self.turn.ttl_seconds),
                    },
                )))
            }
        }
    }

    /// The loopback port of the TURN server of Remote Access. Zero would
    /// be a random port, which the Funnel configuration cannot name.
    pub fn remote_access_turn_port(&self) -> anyhow::Result<u16> {
        anyhow::ensure!(
            self.remote_access_turn_port > 0,
            "screen.remote_access_turn_port is 0; it is the fixed loopback port that Funnel \
             publishes the TURN server of Remote Access on, such as \
             {DEFAULT_REMOTE_ACCESS_TURN_PORT}"
        );
        Ok(self.remote_access_turn_port)
    }

    /// The Media Relay this installation runs, as the System Settings
    /// name it.
    pub fn screen_relay(&self) -> anyhow::Result<pagis_server::ScreenRelay> {
        Ok(pagis_server::ScreenRelay {
            relay: self.relay_kind()?,
            advertise_ip: self.advertise_ip.trim().to_string(),
            media_ports: self.media_port_first..=self.media_port_last,
        })
    }

    fn relay_kind(&self) -> anyhow::Result<MediaRelayKind> {
        match self.relay.trim() {
            RELAY_DAEMON => Ok(MediaRelayKind::Daemon),
            RELAY_TURN => Ok(MediaRelayKind::Turn),
            other => {
                anyhow::bail!("screen.relay is {other:?}; it is {RELAY_DAEMON:?} or {RELAY_TURN:?}")
            }
        }
    }
}

/// Plaintext provider key overrides for headless boxes. The
/// environment variables win over these; both win over `secrets.enc`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Providers {
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub deepgram_api_key: Option<String>,
    pub elevenlabs_api_key: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind: DEFAULT_BIND.to_string(),
            public_origin: String::new(),
            trusted_proxy: String::new(),
            remote_access: RemoteAccess::default(),
            administration: Administration::default(),
            artifact_max_bytes: DEFAULT_ARTIFACT_MAX_BYTES,
            computer: Computer::default(),
            docker_endpoint: String::new(),
            log_level: DEFAULT_LOG_LEVEL.to_string(),
            analytics: true,
            screen: Screen::default(),
            providers: Providers::default(),
            database: Database::default(),
            secrets: Secrets::default(),
        }
    }
}

impl Config {
    /// Read the config file, writing the defaults first when absent,
    /// then apply `PAGIS_*` environment overrides.
    pub fn load_or_init(path: &Path) -> anyhow::Result<Config> {
        let mut config = Config::read_file(path)?;
        config.apply_environment(&|key: &str| std::env::var(key).ok())?;
        Ok(config)
    }

    /// Refuse a start that the headless image does not allow.
    ///
    /// The headless image sets `PAGIS_REQUIRE_PUBLIC_ORIGIN`. A container
    /// then never holds a Client Credential, whatever else its
    /// configuration says: the daemon stops at boot when it is asked to
    /// run a local installation, and when the Public Origin is empty or
    /// has a loopback host, and the error names what to change. The
    /// check adds no behaviour of its own; a binary that passes it runs
    /// exactly as it would without it.
    pub fn check_server_origin(
        &self,
        installation: Installation,
        var: &dyn Fn(&str) -> Option<String>,
    ) -> anyhow::Result<()> {
        let required = match var(REQUIRE_PUBLIC_ORIGIN) {
            Some(value) => parse_flag(REQUIRE_PUBLIC_ORIGIN, &value)?,
            None => false,
        };
        if !required {
            return Ok(());
        }
        if installation == Installation::Local {
            anyhow::bail!(
                "{REQUIRE_PUBLIC_ORIGIN} is set, so this Pagis runs as a server and \
                 holds no Client Credential; start it without --local"
            );
        }
        let origin = self.public_origin.trim();
        if origin.is_empty() {
            anyhow::bail!(
                "this Pagis server has no public origin; set PAGIS_PUBLIC_ORIGIN to the \
                 address people open it at, such as https://pagis.example.net"
            );
        }
        if origin_host_is_loopback(origin) {
            anyhow::bail!(
                "PAGIS_PUBLIC_ORIGIN is {origin:?}, which is not an address people on \
                 other machines can open; set PAGIS_PUBLIC_ORIGIN to the address of \
                 this server, such as https://pagis.example.net"
            );
        }
        Ok(())
    }

    /// Refuse a backend that this kind of installation does not run on
    /// (ADR-0024).
    ///
    /// A server keeps its records in Postgres, so a team of people does
    /// not queue behind one writer, and it does not start without a
    /// database URL. A local installation keeps its records in SQLite in
    /// the state directory, whether one person uses it or several, so it
    /// runs from one binary with no database service; it does not start
    /// with a database URL.
    pub fn check_database(&self, installation: Installation) -> anyhow::Result<()> {
        match (installation, self.database.url()) {
            (Installation::Server, None) => anyhow::bail!(
                "this Pagis server has no database; a server keeps its records in \
                 Postgres, so set PAGIS_DATABASE_URL (or [database] url in config.toml) \
                 to a postgres:// URL"
            ),
            (Installation::Local, Some(_)) => anyhow::bail!(
                "a local installation keeps its records in SQLite in the state \
                 directory; remove PAGIS_DATABASE_URL (and [database] url from \
                 config.toml), or start the daemon without --local to run a server"
            ),
            _ => Ok(()),
        }
    }

    /// The `PAGIS_*` overrides, read through `var` so a test reads no
    /// process environment.
    ///
    /// A container deployment states each of these once in its `.env`
    /// file and passes it in, so the state volume carries no file the
    /// administrator has to edit by hand. An override wins for
    /// the run; the System Settings endpoint writes the file and never
    /// the override (ADR-0024).
    pub fn apply_environment(
        &mut self,
        var: &dyn Fn(&str) -> Option<String>,
    ) -> anyhow::Result<()> {
        if let Some(port) = var("PAGIS_PORT") {
            self.port = parse_env("PAGIS_PORT", &port)?;
        }
        if let Some(url) = var("PAGIS_DATABASE_URL") {
            self.database.url = url;
        }
        if let Some(bind) = var("PAGIS_BIND") {
            self.bind = bind;
        }
        if let Some(origin) = var("PAGIS_PUBLIC_ORIGIN") {
            self.public_origin = origin;
        }
        if let Some(proxy) = var("PAGIS_TRUSTED_PROXY") {
            self.trusted_proxy = proxy;
        }
        if let Some(remote_access) = var(REMOTE_ACCESS) {
            self.remote_access.enabled = parse_flag(REMOTE_ACCESS, &remote_access)?;
        }
        if let Some(port) = var("PAGIS_ADMINISTRATION_PORT") {
            self.administration.port = parse_env("PAGIS_ADMINISTRATION_PORT", &port)?;
        }
        if let Some(bind) = var("PAGIS_ADMINISTRATION_BIND") {
            self.administration.bind = bind;
        }
        // The Media Relay's address and range, which a deployment
        // publishes in its firewall and states here once.
        if let Some(address) = var("PAGIS_SCREEN_ADVERTISE_IP") {
            self.screen.advertise_ip = address;
        }
        if let Some(port) = var("PAGIS_SCREEN_MEDIA_PORT_FIRST") {
            self.screen.media_port_first = parse_env("PAGIS_SCREEN_MEDIA_PORT_FIRST", &port)?;
        }
        if let Some(port) = var("PAGIS_SCREEN_MEDIA_PORT_LAST") {
            self.screen.media_port_last = parse_env("PAGIS_SCREEN_MEDIA_PORT_LAST", &port)?;
        }
        // The exit port, which the deployment also gives the egress rules.
        if let Some(port) = var("PAGIS_COMPUTER_EXIT_PORT") {
            self.computer.exit_port = parse_env("PAGIS_COMPUTER_EXIT_PORT", &port)?;
        }
        Ok(())
    }

    /// The file as it is written, with no environment override. The
    /// System Settings endpoint edits this, so a port set for one run
    /// is never written back as the setting (ADR-0024).
    pub fn read_file(path: &Path) -> anyhow::Result<Config> {
        if !path.exists() {
            std::fs::write(path, toml::to_string_pretty(&Config::default())?)?;
        }
        Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
    }

    /// The interface the listener binds. An empty entry is
    /// loopback; anything that is not an IP address stops the daemon at
    /// boot rather than at the first request.
    pub fn bind_address(&self) -> anyhow::Result<IpAddr> {
        parse_bind(&self.bind, DEFAULT_BIND, "bind")
    }

    /// The origin a browser reaches this installation at.
    ///
    /// The setting wins, because only the deployment knows the name its
    /// proxy answers on. With no setting the daemon builds the origin it
    /// is actually reachable at: the bind address and the served port,
    /// over plain HTTP, since the daemon terminates no TLS. A wildcard
    /// bind has no one address, so that derives loopback.
    pub fn public_origin(&self, port: u16) -> String {
        let origin = self.public_origin.trim().trim_end_matches('/');
        if !origin.is_empty() {
            return origin.to_string();
        }
        let host = match self.bind_address() {
            Ok(address) if !address.is_unspecified() => address,
            _ => DEFAULT_BIND
                .parse()
                .expect("the default bind is an address"),
        };
        format!("http://{}", std::net::SocketAddr::new(host, port))
    }

    /// The origin a program on this machine reaches the daemon at: a
    /// loopback address of the listener and the served port.
    ///
    /// The Sign-In Link starts here, not at the Public Origin, because
    /// the daemon accepts the link only from a loopback socket peer and
    /// never through a proxy (ADR-0025). A loopback bind is its own
    /// address, and a wildcard bind answers loopback too.
    pub fn local_origin(&self, port: u16) -> String {
        let host: IpAddr = match self.bind_address() {
            Ok(address) if address.is_loopback() => address,
            Ok(IpAddr::V6(_)) => std::net::Ipv6Addr::LOCALHOST.into(),
            _ => std::net::Ipv4Addr::LOCALHOST.into(),
        };
        format!("http://{}", std::net::SocketAddr::new(host, port))
    }

    /// The reverse proxy whose forwarded headers the daemon believes.
    /// An empty entry believes none.
    pub fn trusted_proxy(&self) -> anyhow::Result<pagis_server::TrustedProxy> {
        let proxy = self.trusted_proxy.trim();
        if proxy.is_empty() {
            return Ok(pagis_server::TrustedProxy::none());
        }
        let address: IpAddr = proxy.parse().map_err(|_| {
            anyhow::anyhow!(
                "trusted_proxy is {proxy:?}; it is the IP address the reverse \
                 proxy reaches the daemon from"
            )
        })?;
        Ok(pagis_server::TrustedProxy::at(address))
    }

    /// The Docker endpoint the user set, if any. An empty entry in the
    /// file means discovery chooses.
    pub fn docker_endpoint(&self) -> Option<String> {
        let endpoint = self.docker_endpoint.trim();
        (!endpoint.is_empty()).then(|| endpoint.to_string())
    }

    /// Write the file. Only the System Settings endpoint calls this;
    /// the daemon is the one writer of the config file (ADR-0024).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Provider keys set in the config file, for the key resolver.
    pub fn provider_keys(&self) -> HashMap<Provider, String> {
        let mut keys = HashMap::new();
        if let Some(key) = &self.providers.anthropic_api_key {
            keys.insert(Provider::Anthropic, key.clone());
        }
        if let Some(key) = &self.providers.openai_api_key {
            keys.insert(Provider::OpenAi, key.clone());
        }
        if let Some(key) = &self.providers.elevenlabs_api_key {
            keys.insert(Provider::ElevenLabs, key.clone());
        }
        if let Some(key) = &self.providers.deepgram_api_key {
            keys.insert(Provider::Deepgram, key.clone());
        }
        if let Some(key) = &self.providers.openrouter_api_key {
            keys.insert(Provider::OpenRouter, key.clone());
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Analytics are on until an Administrator turns them off, in a new
    /// file and in a file that does not name the setting.
    #[test]
    fn analytics_are_on_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        assert!(Config::load_or_init(&path).unwrap().analytics);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("analytics = true")
        );

        std::fs::write(&path, "port = 4400\n").unwrap();
        assert!(Config::read_file(&path).unwrap().analytics);

        std::fs::write(&path, "analytics = false\n").unwrap();
        assert!(!Config::read_file(&path).unwrap().analytics);
    }

    /// The Home Exit of a Server is on until an Administrator turns it
    /// off, in a new file and in a file that does not name the setting.
    #[test]
    fn the_home_exit_is_on_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        assert!(Config::load_or_init(&path).unwrap().computer.home_exit);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("home_exit = true")
        );

        std::fs::write(&path, "[computer]\nexit_port = 4403\n").unwrap();
        assert!(Config::read_file(&path).unwrap().computer.home_exit);

        std::fs::write(&path, "[computer]\nhome_exit = false\n").unwrap();
        assert!(!Config::read_file(&path).unwrap().computer.home_exit);
    }

    /// Remote Access is off until the switch or the deployment turns it
    /// on. `PAGIS_REMOTE_ACCESS` wins over the file both ways, and a value
    /// that is not a switch stops the daemon with the name of the
    /// variable.
    #[test]
    fn remote_access_is_off_until_the_file_or_the_variable_turns_it_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        assert!(!Config::load_or_init(&path).unwrap().remote_access.enabled);
        std::fs::write(&path, "[remote_access]\nenabled = true\n").unwrap();
        assert!(Config::read_file(&path).unwrap().remote_access.enabled);

        for (value, enabled) in [("1", true), ("true", true), ("0", false), ("false", false)] {
            let mut config = Config::read_file(&path).unwrap();
            config
                .apply_environment(&|key| (key == REMOTE_ACCESS).then(|| value.to_string()))
                .unwrap();
            assert_eq!(config.remote_access.enabled, enabled, "{value}");
        }
        let error = Config::default()
            .apply_environment(&|key| (key == REMOTE_ACCESS).then(|| "yes".to_string()))
            .expect_err("a value that is not a switch");
        assert!(error.to_string().contains(REMOTE_ACCESS), "{error}");
    }

    #[test]
    fn init_writes_the_defaults_and_names_the_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::load_or_init(&path).unwrap();

        assert_eq!(config.port, DEFAULT_PORT);
        assert_eq!(
            config.computer.idle_stop_minutes,
            DEFAULT_COMPUTER_IDLE_STOP_MINUTES
        );
        // The file names the key path and holds no key: the key itself
        // never sits in the config file.
        assert_eq!(
            config.secrets.key_file(),
            Path::new(DEFAULT_SECRETS_KEY_FILE)
        );
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("key_file"));
        assert!(!written.contains("secrets_key ="));
        assert_eq!(
            Config::load_or_init(&path).unwrap().secrets.key_file(),
            config.secrets.key_file()
        );
    }

    /// A container deployment states its settings once in its `.env`
    /// file, so every one of them has a `PAGIS_*` override.
    #[test]
    fn the_environment_overrides_every_setting_a_deployment_states() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::load_or_init(&path).unwrap();
        let environment = |key: &str| match key {
            "PAGIS_PORT" => Some("4500".to_string()),
            "PAGIS_BIND" => Some("0.0.0.0".to_string()),
            "PAGIS_PUBLIC_ORIGIN" => Some("https://pagis.example.net".to_string()),
            "PAGIS_TRUSTED_PROXY" => Some("172.28.0.10".to_string()),
            "PAGIS_REMOTE_ACCESS" => Some("true".to_string()),
            "PAGIS_DATABASE_URL" => Some("postgres://pagis@db/pagis".to_string()),
            "PAGIS_ADMINISTRATION_PORT" => Some("4402".to_string()),
            "PAGIS_ADMINISTRATION_BIND" => Some("127.0.0.1".to_string()),
            "PAGIS_SCREEN_ADVERTISE_IP" => Some("198.51.100.7".to_string()),
            "PAGIS_SCREEN_MEDIA_PORT_FIRST" => Some("50000".to_string()),
            "PAGIS_SCREEN_MEDIA_PORT_LAST" => Some("50019".to_string()),
            _ => None,
        };

        config.apply_environment(&environment).unwrap();

        assert_eq!(config.port, 4500);
        assert_eq!(
            config.bind_address().unwrap(),
            "0.0.0.0".parse::<IpAddr>().unwrap()
        );
        assert_eq!(config.public_origin(4500), "https://pagis.example.net");
        assert_eq!(
            config.trusted_proxy().unwrap().address(),
            Some("172.28.0.10".parse::<IpAddr>().unwrap())
        );
        assert!(config.remote_access.enabled);
        assert_eq!(config.database.url(), Some("postgres://pagis@db/pagis"));
        assert_eq!(config.administration.port, 4402);
        assert_eq!(config.screen.advertise_ip, "198.51.100.7");
        assert_eq!(config.screen.media_port_first, 50000);
        assert_eq!(config.screen.media_port_last, 50019);
        // The overrides are for the run alone: nothing wrote them back.
        assert_eq!(Config::read_file(&path).unwrap().port, DEFAULT_PORT);
    }

    fn server_image(key: &str) -> Option<String> {
        (key == REQUIRE_PUBLIC_ORIGIN).then(|| "true".to_string())
    }

    /// The headless image sets the requirement, so a container with no
    /// Public Origin stops at boot and says which variable sets one.
    #[test]
    fn a_server_image_with_no_origin_names_the_variable() {
        let config = Config::default();

        let error = config
            .check_server_origin(Installation::Server, &server_image)
            .expect_err("a server with no origin does not start");

        assert!(error.to_string().contains("PAGIS_PUBLIC_ORIGIN"), "{error}");
    }

    /// A loopback origin is an address nobody on another machine opens,
    /// so the image refuses it.
    #[test]
    fn a_server_image_with_a_loopback_origin_does_not_start() {
        for origin in [
            "http://127.0.0.1:4400",
            "http://localhost:4400",
            "http://[::1]:4400",
        ] {
            let config = Config {
                public_origin: origin.to_string(),
                ..Config::default()
            };

            let error = config
                .check_server_origin(Installation::Server, &|key| {
                    (key == REQUIRE_PUBLIC_ORIGIN).then(|| "1".to_string())
                })
                .expect_err("a loopback origin is not a server");

            assert!(error.to_string().contains("PAGIS_PUBLIC_ORIGIN"), "{error}");
        }
    }

    #[test]
    fn a_server_image_with_a_public_origin_starts() {
        let config = Config {
            public_origin: "https://pagis.example.net".to_string(),
            ..Config::default()
        };

        config
            .check_server_origin(Installation::Server, &server_image)
            .unwrap();
    }

    /// The headless image holds no Client Credential in any
    /// configuration: it refuses to run a local installation, whatever
    /// its Public Origin.
    #[test]
    fn a_server_image_refuses_a_local_installation() {
        for origin in ["", "https://pagis.example.net", "http://127.0.0.1:4400"] {
            let config = Config {
                public_origin: origin.to_string(),
                ..Config::default()
            };

            let error = config
                .check_server_origin(Installation::Local, &server_image)
                .expect_err("the image runs no local installation");

            assert!(error.to_string().contains("--local"), "{error}");
        }
    }

    /// Without the variable nothing changes: a local installation with
    /// no origin still starts, and so does a server.
    #[test]
    fn a_binary_outside_the_image_asks_for_no_origin() {
        let config = Config::default();

        for installation in [Installation::Local, Installation::Server] {
            config.check_server_origin(installation, &|_| None).unwrap();
            config
                .check_server_origin(installation, &|key| {
                    (key == REQUIRE_PUBLIC_ORIGIN).then(|| "false".to_string())
                })
                .unwrap();
        }
    }

    #[test]
    fn a_requirement_that_is_not_a_boolean_names_its_variable() {
        let error = Config::default()
            .check_server_origin(Installation::Server, &|key| {
                (key == REQUIRE_PUBLIC_ORIGIN).then(|| "yes".to_string())
            })
            .expect_err("an unreadable requirement stops the daemon");

        assert!(error.to_string().contains(REQUIRE_PUBLIC_ORIGIN), "{error}");
    }

    fn with_database(url: &str) -> Config {
        Config {
            database: Database {
                url: url.to_string(),
            },
            ..Config::default()
        }
    }

    /// A server keeps its records in Postgres. With no URL it does not
    /// start, and the error names the variable and the file entry.
    #[test]
    fn a_server_with_no_database_url_names_the_variable() {
        for url in ["", "   "] {
            let error = with_database(url)
                .check_database(Installation::Server)
                .expect_err("a server does not run on SQLite");

            assert!(error.to_string().contains("PAGIS_DATABASE_URL"), "{error}");
            assert!(error.to_string().contains("[database] url"), "{error}");
        }
    }

    /// A local installation keeps its records in SQLite in the state
    /// directory. A Postgres URL stops it, and the error names what to
    /// remove.
    #[test]
    fn a_local_installation_with_a_database_url_does_not_start() {
        let error = with_database("postgres://pagis@db.internal/pagis")
            .check_database(Installation::Local)
            .expect_err("a local installation does not run on Postgres");

        assert!(error.to_string().contains("PAGIS_DATABASE_URL"), "{error}");
        assert!(error.to_string().contains("[database] url"), "{error}");
        assert!(error.to_string().contains("--local"), "{error}");
    }

    #[test]
    fn each_installation_starts_on_its_own_backend() {
        with_database("")
            .check_database(Installation::Local)
            .unwrap();
        with_database("postgres://pagis@db.internal/pagis")
            .check_database(Installation::Server)
            .unwrap();
    }

    #[test]
    fn a_port_override_that_is_not_a_port_names_its_own_variable() {
        let mut config = Config::default();

        let error = config
            .apply_environment(&|key| (key == "PAGIS_PORT").then(|| "http".to_string()))
            .expect_err("a port that is not a number stops the daemon");

        assert!(error.to_string().contains("PAGIS_PORT"));
    }

    #[test]
    fn a_key_file_the_user_named_wins_and_an_empty_entry_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "port = 4400\n[secrets]\nkey_file = \"/etc/pagis/key\"\n",
        )
        .unwrap();

        let config = Config::load_or_init(&path).unwrap();
        assert_eq!(config.secrets.key_file(), Path::new("/etc/pagis/key"));

        std::fs::write(&path, "port = 4400\n[secrets]\nkey_file = \"  \"\n").unwrap();
        assert_eq!(
            Config::load_or_init(&path).unwrap().secrets.key_file(),
            Path::new(DEFAULT_SECRETS_KEY_FILE)
        );
    }

    #[test]
    fn a_saved_config_reads_back_with_the_system_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::load_or_init(&path).unwrap();
        assert_eq!(config.log_level, DEFAULT_LOG_LEVEL);
        assert_eq!(config.docker_endpoint(), None);

        config.port = 4500;
        config.docker_endpoint = "unix:///tmp/docker.sock".to_string();
        config.log_level = "debug".to_string();
        config.save(&path).unwrap();

        let reloaded = Config::load_or_init(&path).unwrap();
        assert_eq!(reloaded.port, 4500);
        assert_eq!(
            reloaded.docker_endpoint(),
            Some("unix:///tmp/docker.sock".to_string())
        );
        assert_eq!(reloaded.log_level, "debug");
        assert_eq!(reloaded.secrets.key_file(), config.secrets.key_file());
    }

    /// The System Settings name the relay of the run: the section with
    /// the environment overrides of this run.
    #[test]
    fn the_screen_relay_names_the_relay_the_run_serves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[screen]\nrelay = \"turn\"\nadvertise_ip = \" 10.0.1.7 \"\n",
        )
        .unwrap();
        let mut config = Config::read_file(&path).unwrap();
        assert_eq!(
            config.screen.screen_relay().unwrap(),
            pagis_server::ScreenRelay {
                relay: MediaRelayKind::Turn,
                advertise_ip: "10.0.1.7".to_string(),
                media_ports: DEFAULT_MEDIA_PORT_FIRST..=DEFAULT_MEDIA_PORT_LAST,
            }
        );

        config
            .apply_environment(&|key: &str| match key {
                "PAGIS_SCREEN_ADVERTISE_IP" => Some("192.168.86.55".to_string()),
                "PAGIS_SCREEN_MEDIA_PORT_LAST" => Some("50019".to_string()),
                _ => None,
            })
            .unwrap();
        let relay = config.screen.screen_relay().unwrap();
        assert_eq!(relay.advertise_ip, "192.168.86.55");
        assert_eq!(relay.media_ports, DEFAULT_MEDIA_PORT_FIRST..=50019);

        config.screen.relay = "stun".to_string();
        let error = config.screen.screen_relay().unwrap_err().to_string();
        assert!(error.contains("screen.relay"), "{error}");
    }

    /// A local installation configures nothing and still views a screen:
    /// the default `[screen]` section builds the `daemon` relay
    /// on loopback over the default port range.
    #[test]
    fn the_default_screen_section_builds_the_daemon_relay_on_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::load_or_init(&path).unwrap();

        assert_eq!(config.screen.advertise_ip, "127.0.0.1");
        assert_eq!(config.screen.relay, RELAY_DAEMON);
        assert_eq!(config.screen.media_port_first, DEFAULT_MEDIA_PORT_FIRST);
        assert_eq!(config.screen.media_port_last, DEFAULT_MEDIA_PORT_LAST);
        let relay = config.screen.media_relay().expect("the default relay");
        // The daemon relay needs no ICE server, so a local browser
        // reaches the advertised address by itself.
        assert!(relay.ice_servers().is_empty());
        // The file names the range once, for a firewall rule and a
        // compose entry to follow.
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("media_port_first"), "{written}");
        assert!(written.contains("relay = \"daemon\""), "{written}");
    }

    /// The `turn` relay mints one credential per session from the
    /// shared secret, and refuses to run without a server to mint for.
    #[test]
    fn the_turn_relay_needs_a_server_and_a_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "port = 4400\n[screen]\nrelay = \"turn\"\n\
             [screen.turn]\nurls = [\"turn:relay.example.net:3478\"]\n",
        )
        .unwrap();
        let config = Config::load_or_init(&path).unwrap();

        let error = config
            .screen
            .media_relay()
            .map(|_| ())
            .expect_err("a TURN relay with no secret cannot mint a credential");
        assert!(error.to_string().contains("secret"), "{error}");

        std::fs::write(
            &path,
            "port = 4400\n[screen]\nrelay = \"turn\"\n\
             [screen.turn]\nurls = [\"turn:relay.example.net:3478\"]\n\
             secret = \"shared\"\n",
        )
        .unwrap();
        let relay = Config::load_or_init(&path)
            .unwrap()
            .screen
            .media_relay()
            .expect("the turn relay");

        let servers = relay.ice_servers();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].urls, vec!["turn:relay.example.net:3478"]);
        assert!(!servers[0].credential.is_empty());
    }

    /// A relay nobody implements, and a port range that is not one, stop
    /// the daemon at boot instead of at the first viewer.
    #[test]
    fn an_unknown_relay_or_an_empty_range_is_refused() {
        let unknown = Screen {
            relay: "coturn".to_string(),
            ..Screen::default()
        };
        let error = unknown
            .media_relay()
            .map(|_| ())
            .expect_err("no such relay");
        assert!(error.to_string().contains("\"daemon\""), "{error}");

        let backwards = Screen {
            media_port_first: 50100,
            media_port_last: 50000,
            ..Screen::default()
        };
        let error = backwards
            .media_relay()
            .map(|_| ())
            .expect_err("not a range");
        assert!(error.to_string().contains("media_port_first"), "{error}");
    }

    /// The TURN server of Remote Access has a fixed loopback port next to
    /// the other two, which the file names, and a port of zero, a random
    /// port, stops the daemon.
    #[test]
    fn the_turn_server_of_remote_access_has_a_fixed_port() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::load_or_init(&path).unwrap();

        assert_eq!(
            config.screen.remote_access_turn_port().unwrap(),
            DEFAULT_REMOTE_ACCESS_TURN_PORT
        );
        assert_ne!(DEFAULT_REMOTE_ACCESS_TURN_PORT, config.port);
        assert_ne!(DEFAULT_REMOTE_ACCESS_TURN_PORT, config.administration.port);
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("remote_access_turn_port = 4402"),
            "{written}"
        );

        std::fs::write(&path, "[screen]\nremote_access_turn_port = 4500\n").unwrap();
        let moved = Config::load_or_init(&path).unwrap();
        assert_eq!(moved.screen.remote_access_turn_port().unwrap(), 4500);

        let random = Screen {
            remote_access_turn_port: 0,
            ..Screen::default()
        };
        let error = random
            .remote_access_turn_port()
            .expect_err("no random port");
        assert!(
            error.to_string().contains("remote_access_turn_port"),
            "{error}"
        );
    }

    /// A local installation configures nothing and is reached from its
    /// own machine alone: the default binds loopback and the
    /// derived Public Origin is that address and the served port.
    #[test]
    fn the_default_binds_loopback_and_derives_a_loopback_origin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::load_or_init(&path).unwrap();

        assert_eq!(config.bind, DEFAULT_BIND);
        assert!(config.bind_address().unwrap().is_loopback());
        assert_eq!(config.public_origin(4400), "http://127.0.0.1:4400");
        assert_eq!(
            config.trusted_proxy().unwrap(),
            pagis_server::TrustedProxy::none()
        );
        // The file names the keys once, for a deployment to follow.
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("bind = \"127.0.0.1\""), "{written}");
        assert!(written.contains("public_origin"), "{written}");
        assert!(written.contains("trusted_proxy"), "{written}");
    }

    /// The Administration Interface has its own port and binds loopback
    /// by default, whatever the product port binds.
    #[test]
    fn the_administration_interface_has_its_own_loopback_port() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::load_or_init(&path).unwrap();

        assert_eq!(config.administration.port, DEFAULT_ADMINISTRATION_PORT);
        assert_ne!(config.administration.port, config.port);
        assert!(config.administration.bind_address().unwrap().is_loopback());
        // The file names the section, so a deployment reads the pair of
        // ports at a glance.
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("[administration]"), "{written}");
    }

    /// A server that serves the network keeps the administration port on
    /// loopback unless it names an address of its own.
    #[test]
    fn the_administration_port_stays_private_while_the_product_port_faces_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "port = 4400\nbind = \"0.0.0.0\"\n\n[administration]\nport = 9443\n",
        )
        .unwrap();

        let config = Config::load_or_init(&path).unwrap();

        assert!(config.bind_address().unwrap().is_unspecified());
        assert_eq!(config.administration.port, 9443);
        assert!(config.administration.bind_address().unwrap().is_loopback());
    }

    /// An administration bind that is not an address stops the daemon at
    /// boot, and the message names the key.
    #[test]
    fn an_administration_bind_that_is_not_an_address_is_refused() {
        let named = Administration {
            bind: "admin.example.net".to_string(),
            ..Administration::default()
        };

        let error = named.bind_address().unwrap_err().to_string();

        assert!(error.contains("administration.bind"), "{error}");
        assert!(
            Administration {
                bind: "  ".to_string(),
                ..Administration::default()
            }
            .bind_address()
            .unwrap()
            .is_loopback()
        );
    }

    /// A server names an interface, the origin its proxy answers on, and
    /// the proxy's own address.
    #[test]
    fn a_server_binds_a_named_interface_behind_a_named_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "port = 4400\nbind = \"10.0.0.5\"\n\
             public_origin = \"https://pagis.example.net/\"\n\
             trusted_proxy = \"10.0.0.4\"\n",
        )
        .unwrap();

        let config = Config::load_or_init(&path).unwrap();

        let bind = config.bind_address().unwrap();
        assert_eq!(bind, "10.0.0.5".parse::<IpAddr>().unwrap());
        assert!(!bind.is_loopback());
        // The trailing slash of an origin is not part of it.
        assert_eq!(config.public_origin(4400), "https://pagis.example.net");
        assert_eq!(
            config.trusted_proxy().unwrap(),
            pagis_server::TrustedProxy::at("10.0.0.4".parse().unwrap())
        );
    }

    /// With an interface named and no origin set, the daemon is reached
    /// at that interface: the derived origin says so rather than naming
    /// loopback, which no other machine can reach.
    #[test]
    fn a_named_interface_derives_its_own_origin_and_a_wildcard_derives_loopback() {
        let named = Config {
            bind: "10.0.0.5".to_string(),
            ..Config::default()
        };
        assert_eq!(named.public_origin(4400), "http://10.0.0.5:4400");

        let wildcard = Config {
            bind: "0.0.0.0".to_string(),
            ..Config::default()
        };
        assert_eq!(wildcard.public_origin(4400), "http://127.0.0.1:4400");

        let six = Config {
            bind: "::1".to_string(),
            ..Config::default()
        };
        assert_eq!(six.public_origin(4400), "http://[::1]:4400");
    }

    /// The Sign-In Link starts at loopback, whatever the Public Origin
    /// names: the daemon accepts it from this machine alone.
    #[test]
    fn the_local_origin_is_loopback_whatever_the_public_origin_names() {
        let behind_a_proxy = Config {
            public_origin: "https://pagis.example/".to_string(),
            trusted_proxy: "127.0.0.1".to_string(),
            ..Config::default()
        };
        assert_eq!(behind_a_proxy.local_origin(4400), "http://127.0.0.1:4400");

        let wildcard = Config {
            bind: "0.0.0.0".to_string(),
            ..Config::default()
        };
        assert_eq!(wildcard.local_origin(4400), "http://127.0.0.1:4400");

        let six = Config {
            bind: "::1".to_string(),
            ..Config::default()
        };
        assert_eq!(six.local_origin(4400), "http://[::1]:4400");

        let six_wildcard = Config {
            bind: "::".to_string(),
            ..Config::default()
        };
        assert_eq!(six_wildcard.local_origin(4400), "http://[::1]:4400");
    }

    /// A bind or a proxy address that is not an address stops the daemon
    /// at boot, and an empty entry keeps the default.
    #[test]
    fn a_bind_or_a_proxy_that_is_not_an_address_is_refused() {
        let named_host = Config {
            bind: "pagis.example.net".to_string(),
            ..Config::default()
        };
        let error = named_host.bind_address().unwrap_err();
        assert!(error.to_string().contains("IP address"), "{error}");

        let bad_proxy = Config {
            trusted_proxy: "proxy.internal".to_string(),
            ..Config::default()
        };
        let error = bad_proxy.trusted_proxy().map(|_| ()).unwrap_err();
        assert!(error.to_string().contains("trusted_proxy"), "{error}");

        let blank = Config {
            bind: "  ".to_string(),
            trusted_proxy: "  ".to_string(),
            ..Config::default()
        };
        assert!(blank.bind_address().unwrap().is_loopback());
        assert_eq!(
            blank.trusted_proxy().unwrap(),
            pagis_server::TrustedProxy::none()
        );
    }

    #[test]
    fn provider_entries_surface_as_config_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "port = 4400\n[providers]\nanthropic_api_key = \"sk-config\"\n\
             openrouter_api_key = \"sk-openrouter\"\ndeepgram_api_key = \"sk-deepgram\"\n\
             elevenlabs_api_key = \"sk-elevenlabs\"\n",
        )
        .unwrap();

        let config = Config::load_or_init(&path).unwrap();

        assert_eq!(
            config.provider_keys().get(&Provider::Anthropic),
            Some(&"sk-config".to_string())
        );
        assert_eq!(config.provider_keys().get(&Provider::OpenAi), None);
        assert_eq!(
            config.provider_keys().get(&Provider::OpenRouter),
            Some(&"sk-openrouter".to_string())
        );
        assert_eq!(
            config.provider_keys().get(&Provider::Deepgram),
            Some(&"sk-deepgram".to_string())
        );
        assert_eq!(
            config.provider_keys().get(&Provider::ElevenLabs),
            Some(&"sk-elevenlabs".to_string())
        );
    }

    #[test]
    fn no_database_section_means_sqlite_and_a_url_means_postgres() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let local = Config::load_or_init(&path).unwrap();
        assert_eq!(local.database.url(), None);

        std::fs::write(
            &path,
            "port = 4400\n[database]\nurl = \"postgres://pagis@db.internal/pagis\"\n",
        )
        .unwrap();
        let server = Config::load_or_init(&path).unwrap();
        assert_eq!(
            server.database.url(),
            Some("postgres://pagis@db.internal/pagis")
        );

        // Whitespace is not a URL.
        std::fs::write(&path, "port = 4400\n[database]\nurl = \"   \"\n").unwrap();
        assert_eq!(Config::load_or_init(&path).unwrap().database.url(), None);
    }

    /// The limits and the caps come from the file, and the defaults are
    /// the ones the runtime takes.
    #[test]
    fn the_computer_section_carries_the_limits_and_the_caps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "port = 4400\n[computer]\nmemory_mb = 2048\ncpus = 0.5\nvolume_gb = 0\n\
             layer_gb = 0\nawake_per_tenant = 1\n",
        )
        .unwrap();

        let config = Config::load_or_init(&path).unwrap();
        let limits = config.computer.limits();

        assert_eq!(limits.memory_bytes, 2048 * 1024 * 1024);
        assert_eq!(limits.nano_cpus, 500_000_000);
        // Zero gibibytes asks for no size at all.
        assert_eq!(limits.volume_bytes, None);
        assert_eq!(limits.layer_bytes, None);
        // What the file does not say keeps the default.
        assert_eq!(limits.pids, 512);
        assert_eq!(config.computer.caps().per_tenant, 1);
        assert_eq!(config.computer.caps().per_server, 24);
        assert_eq!(
            config.computer.idle_stop(),
            std::time::Duration::from_secs(DEFAULT_COMPUTER_IDLE_STOP_MINUTES * 60)
        );
        assert_eq!(
            Computer::default().limits(),
            pagis_computer::ComputerLimits::default()
        );
    }

    /// The exit port of a Server comes from the file or from the
    /// environment of the deployment, and it is never zero: the egress
    /// rules name it.
    #[test]
    fn the_exit_port_comes_from_the_file_or_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let config = Config::load_or_init(&path).unwrap();
        assert_eq!(config.computer.exit_port().unwrap(), 4403);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("exit_port = 4403")
        );

        std::fs::write(&path, "[computer]\nexit_port = 4500\n").unwrap();
        let mut config = Config::read_file(&path).unwrap();
        assert_eq!(config.computer.exit_port().unwrap(), 4500);
        config
            .apply_environment(&|key: &str| {
                (key == "PAGIS_COMPUTER_EXIT_PORT").then(|| "4600".to_string())
            })
            .unwrap();
        assert_eq!(config.computer.exit_port().unwrap(), 4600);

        let error = config
            .clone()
            .apply_environment(&|key: &str| {
                (key == "PAGIS_COMPUTER_EXIT_PORT").then(|| "exit".to_string())
            })
            .unwrap_err();
        assert!(error.to_string().contains("PAGIS_COMPUTER_EXIT_PORT"));
        config.computer.exit_port = 0;
        let error = config.computer.exit_port().unwrap_err();
        assert!(error.to_string().contains("computer.exit_port"));
    }
}
