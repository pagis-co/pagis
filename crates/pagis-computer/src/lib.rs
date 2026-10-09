//! The agent computer lifecycle: the version-pinned image and its one
//! pull for the installation, the Docker runtime seam, and the manager
//! that wakes, idle-stops, and serves screen previews. One container and
//! one named volume per agent; `/data` survives stops.

mod bollard_runtime;
pub mod browser;
pub mod docker;
pub mod exec;
pub mod exit_listener;
pub mod fake;
pub mod home_exit;
mod image;
mod manager;
mod pull_progress;
pub mod relay;
pub mod remote_access_turn;
mod tenants;
pub mod test_docker;

pub use bollard_runtime::{BollardRuntime, RuntimeOptions};
pub use browser::{FieldKind, FillField};
pub use docker::{
    DockerCandidate, DockerCandidateResult, DockerDiscovery, DockerReport, DockerSearch,
    DockerSource,
};
pub use exit_listener::{ComputerTokens, ExitListener};
pub use home_exit::{ExitBytes, ExitError, ExitInUse, ExitStream, HomeExits};
pub use image::{ComputerImage, ImagePullError};
pub use manager::{
    ComputerManager, ComputerManagerDeps, DaemonHold, ExecPin, ExitSwitchFailure, HarnessExec,
    Preview, SHELL_HOME, SHELL_USER, ShellCommand, TakeoverTiming,
};
pub use relay::{
    DaemonRelay, IceCredentials, IceServer, MediaForwarder, MediaPath, MediaRelay, OpenPath,
    REGISTRATION_PREFIX, TurnRelay, TurnServer,
};
pub use remote_access_turn::{MediaRelayPeers, RemoteAccessTurn};
pub use tenants::{AwakeCeiling, ComputerKind, ComputerManagers, ComputerManagersDeps};

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use pagis_core::{AgentId, SkillMount, WorkspaceId};
use serde::Serialize;

/// The pinned computer image. The tag and the image's
/// `co.pagis.computer.version` label move together; see
/// `computer/Dockerfile`.
pub use pagis_versions::{COMPUTER_IMAGE as IMAGE, COMPUTER_IMAGE_VERSION as IMAGE_VERSION};
/// The image label the daemon verifies before booting a container.
pub const VERSION_LABEL: &str = "co.pagis.computer.version";

/// The repository of an image reference: the reference without its tag
/// and without its digest. `ghcr.io/pagis-co/pagis-computer@sha256:…`
/// and `ghcr.io/pagis-co/pagis-computer:0.17.1` both give
/// `ghcr.io/pagis-co/pagis-computer`. The port of a registry is not a
/// tag, because a tag comes after the last `/`.
pub fn image_repository(reference: &str) -> &str {
    let name = reference
        .split_once('@')
        .map_or(reference, |(name, _digest)| name);
    match name.rfind(':') {
        Some(colon) if !name[colon..].contains('/') => &name[..colon],
        _ => name,
    }
}

/// One local image of the Computer Image repository other than the
/// pinned one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherImage {
    pub id: String,
    /// Its [`VERSION_LABEL`], or `None` where it has none.
    pub version: Option<String>,
}

/// What Docker did with an image that the daemon asked it to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageRemoval {
    Removed,
    /// Docker answered 409 Conflict and kept the image: a container
    /// uses it, or another image or repository refers to it. This is not
    /// an error.
    InUse,
}
/// The seccomp profile every computer runs under. It is
/// Docker's default profile with the user-namespace calls permitted,
/// which is what Chromium's renderer sandbox needs to start; under the
/// default profile the browser has no way in but `--no-sandbox`.
/// `seccomp/NOTICE` names the upstream file and the one change.
pub const SECCOMP_PROFILE: &str = include_str!("../seccomp/chromium.json");
/// The container label that names the mount set a container booted
/// with. A container whose set does not match the wanted set is
/// replaced at the next wake, so a Grant, an update or an uninstall
/// reaches the agent's next Run.
pub const MOUNTS_LABEL: &str = "co.pagis.computer.mounts";
/// Where a granted Plugin's `skills/` directory appears inside the
/// container (ADR-0017). The image ships its own Skills at
/// `/opt/pagis/skills/`.
pub const PLUGIN_MOUNT_ROOT: &str = "/opt/plugins";
/// The locale every computer runs in. The image generates this
/// one locale, and the container carries it as `LANG`.
pub const CONTAINER_LANG: &str = "en_US.UTF-8";
/// The timezone a computer takes when the Workspace has none to give.
pub const DEFAULT_TIMEZONE: &str = "UTC";
/// The largest tar a download reads out of a computer. The tar comes
/// from the agent's own files, so the daemon stops the download when
/// the raw byte count passes this limit. A limit that does not depend
/// on a volume quota bounds the disk that one download takes.
pub const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;

/// The Exit Proxy inside every Computer (ADR-0029). screend listens
/// here, and `computer/browser.sh` gives Chromium the same address.
pub const EXIT_PROXY: &str = "http://127.0.0.1:3128";

/// Where the Exit Proxy of an Agent's Computer on a Server starts
/// (ADR-0029): the exit listener of the daemon, which `Home` mode sends
/// each connection to, and the first mode. A Computer of a Local
/// Installation and the Plugin Computer have none, and run in `Direct`
/// mode alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitStart {
    /// The exit listener as a Computer reaches it, such as
    /// `host.docker.internal:4403` (see [`exit_daemon`]).
    pub daemon: String,
    pub mode: ExitMode,
}

/// The address at which a Computer reaches the exit listener of the
/// daemon on `port`: the Docker host, as the Media Relay is reached.
pub fn exit_daemon(port: u16) -> String {
    format!("{RELAY_HOST}:{port}")
}

/// The base URL at which the harness of a Coding Session in a Computer
/// reaches the Harness Model Endpoint of the daemon on `port`: the Docker
/// host. `NO_PROXY` holds that name, so the requests do not pass the Exit
/// Proxy.
pub fn model_endpoint(port: u16) -> String {
    format!("http://{RELAY_HOST}:{port}")
}

/// The exit of the Computers of a Server (ADR-0029): the exit listener
/// that each Agent's Computer names at its start, and the Home Exits of
/// the People, which say which mode is in effect for each Person. A Local
/// Installation has none, and its Computers run in `Direct` mode alone.
#[derive(Clone)]
pub struct ComputerExit {
    /// The exit listener as a Computer reaches it ([`exit_daemon`]).
    pub daemon: String,
    pub home_exits: Arc<HomeExits>,
}

/// The environment one container boots with: the Workspace timezone,
/// the locale of the image, the Exit Proxy, and where the Exit Proxy
/// starts. The compositor, screend and Chromium are children of the
/// entrypoint, so they inherit it, and so does every `docker exec`:
/// `computer_shell` and the servers of the Plugin Computer. The image's
/// sudo keeps the proxy entries for the shell of the terminal.
///
/// A clock and a language that disagree with the egress IP make the
/// browser look automated, and pages then render times the agent has
/// to convert.
///
/// The proxy entries come in both cases, because a tool reads one case or
/// the other: curl reads `http_proxy` in lower case alone. `NO_PROXY` holds
/// loopback, which Chromium also sends to no proxy, and the Docker host
/// ([`RELAY_HOST`]), so a connection to the daemon's machine never
/// leaves through the Exit Proxy. Every container that the runtime
/// starts is a Computer Image container that runs screend, the Plugin
/// Computer too, so every container with these entries runs the proxy
/// that they name.
///
/// `exit` names the exit listener of the daemon and the first mode in
/// `PAGIS_EXIT_DAEMON` and `PAGIS_EXIT_MODE`, which screend reads at
/// start. The address is not a secret: the listener asks for the
/// Computer's token, which the agent's shell cannot read.
pub fn container_env(timezone: &str, exit: Option<&ExitStart>) -> Vec<String> {
    let no_proxy = format!("localhost,127.0.0.1,::1,{RELAY_HOST}");
    let mut env = vec![
        format!("TZ={timezone}"),
        format!("LANG={CONTAINER_LANG}"),
        format!("HTTP_PROXY={EXIT_PROXY}"),
        format!("HTTPS_PROXY={EXIT_PROXY}"),
        format!("http_proxy={EXIT_PROXY}"),
        format!("https_proxy={EXIT_PROXY}"),
        format!("NO_PROXY={no_proxy}"),
        format!("no_proxy={no_proxy}"),
    ];
    if let Some(exit) = exit {
        env.push(format!("PAGIS_EXIT_DAEMON={}", exit.daemon));
        env.push(format!("PAGIS_EXIT_MODE={}", exit.mode.as_str()));
    }
    env
}

/// The in-container screend control port.
pub const CONTROL_PORT: u16 = 7900;
/// The in-container WebRTC media UDP port. No host port is
/// published for it: the pipeline registers outbound with the
/// Media Relay for each viewer session, so nothing reaches it from the
/// host.
pub const MEDIA_PORT: u16 = 7901;

/// The name a container reaches the daemon's Media Relay at.
/// Docker Desktop resolves it to the host; the runtime maps it to the
/// Docker host's gateway on every other Docker host.
pub const RELAY_HOST: &str = "host.docker.internal";

/// What one container may take of the host. Without these one
/// tenant exhausts the machine every other tenant runs on, and the
/// container stops being a boundary. The values come from `Config`;
/// the defaults here are what one desktop with a browser needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerLimits {
    /// The memory ceiling of the container, in bytes.
    pub memory_bytes: i64,
    /// The CPU share, in billionths of a CPU: one CPU is 1e9.
    pub nano_cpus: i64,
    /// How many processes and threads the container may hold at once.
    pub pids: i64,
    /// The `nofile` ulimit of every process in the container.
    pub open_files: i64,
    /// `/dev/shm`. Chromium starves under Docker's 64 MB default
    /// and does not need 2 GiB: the figure is per container, and a
    /// shared server multiplies it.
    pub shm_bytes: i64,
    /// The size of the volume at `/data`, in bytes, or `None` for no
    /// size. Docker's `local` volume driver holds a volume to it only
    /// where the Docker data directory is on XFS mounted with `pquota`.
    /// On every other filesystem Docker refuses the option: the daemon
    /// makes the volume without a size and logs one warning, and
    /// nothing bounds what `/data` holds. The disk figure of the Desk
    /// reports what the volumes hold. It bounds nothing.
    pub volume_bytes: Option<u64>,
    /// The size of the writable container layer, in bytes, or `None` for
    /// no size. The layer holds every write outside `/data`: `/tmp`, the
    /// packages that `pagis-apt` installs, and caches. Docker holds the
    /// layer to it under the `overlay2` storage driver on XFS mounted
    /// with `pquota`, and under the `btrfs` and `zfs` storage drivers.
    /// `overlay2` on every other filesystem refuses the option: the
    /// daemon makes the container without a size and logs one warning.
    /// The containerd image store takes the option and holds the layer
    /// to nothing. A stop removes the container and its layer, so the
    /// layer grows without a bound only while its Computer is awake.
    pub layer_bytes: Option<u64>,
}

impl Default for ComputerLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 4 * 1024 * 1024 * 1024,
            nano_cpus: 2 * 1_000_000_000,
            pids: 512,
            open_files: 8192,
            shm_bytes: 512 * 1024 * 1024,
            volume_bytes: Some(10 * 1024 * 1024 * 1024),
            layer_bytes: Some(10 * 1024 * 1024 * 1024),
        }
    }
}

/// How many Computers may be awake at once. Idle eviction bounds
/// how long a Computer stays awake; this bounds how many there are, per
/// tenant and for the whole server, so the load is not people
/// multiplied by sprites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwakeCaps {
    pub per_tenant: u32,
    pub per_server: u32,
}

impl Default for AwakeCaps {
    fn default() -> Self {
        Self {
            per_tenant: 3,
            per_server: 24,
        }
    }
}

/// Where the daemon puts the container's screend token, read-only.
/// The entrypoint copies it to a file only the `screen` user
/// reads, then closes this directory, so the sprite's own shell never
/// reads the token of its container.
pub const TOKEN_MOUNT: &str = "/run/pagis-token/screend";

/// What one tenant's Docker objects add up to.
///
/// The owner label decides whose object each one is, never its name, so
/// the figures are one tenant's and the server's total is nobody's
/// business here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TenantResources {
    /// The containers the tenant holds, awake or not.
    pub containers: u32,
    /// The named volumes of the tenant's Agents.
    pub volumes: u32,
    /// The bytes those volumes take. A volume the runtime has not
    /// measured counts as nothing.
    pub volume_bytes: u64,
}

/// The Docker label that names the tenant a container, a volume or a
/// Tenant Network belongs to. One tenant's Docker objects are
/// listed, measured and reaped by this label alone.
pub const WORKSPACE_LABEL: &str = "co.pagis.workspace";
/// The Docker label that names the Agent inside the tenant.
pub const AGENT_LABEL: &str = "co.pagis.agent";
/// The Docker label that marks a container, a volume or a Tenant
/// Network that a test created (see [`test_docker::TestDocker`]). The
/// daemon never puts it on an object, and a sweep of test objects
/// removes only objects that carry it.
pub const TEST_LABEL: &str = "co.pagis.test";

/// Who owns one container and one volume: the tenant, and the
/// Agent inside it. Every name and every label a Docker object carries
/// comes from here, so no two tenants share an object and one tenant's
/// objects are always separable from another's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ComputerOwner {
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
}

impl ComputerOwner {
    pub fn new(workspace_id: WorkspaceId, agent_id: AgentId) -> Self {
        Self {
            workspace_id,
            agent_id,
        }
    }

    /// The container name: the owner prefix of the tenant, then the
    /// Agent.
    pub fn container_name(&self) -> String {
        format!("{}{}", container_prefix(&self.workspace_id), self.agent_id)
    }

    /// The named volume, mounted at `/data`. It holds both homes of
    /// the uid split: `/data/agent` for the agent's shell and
    /// `/data/screen` for the compositor session and the browser
    /// profile.
    pub fn volume_name(&self) -> String {
        format!("{}{}", volume_prefix(&self.workspace_id), self.agent_id)
    }

    /// The labels every container and every volume of this owner
    /// carries.
    pub fn labels(&self) -> [(String, String); 2] {
        [
            (WORKSPACE_LABEL.to_string(), self.workspace_id.to_string()),
            (AGENT_LABEL.to_string(), self.agent_id.to_string()),
        ]
    }
}

/// Every container of one tenant carries this name prefix.
pub fn container_prefix(workspace_id: &WorkspaceId) -> String {
    format!("pagis-computer-{workspace_id}-")
}

/// Every volume of one tenant carries this name prefix, so the disk
/// figure counts that tenant's volumes and nobody else's.
pub fn volume_prefix(workspace_id: &WorkspaceId) -> String {
    format!("pagis-volume-{workspace_id}-")
}

/// The Tenant Network of one tenant: the one Docker network
/// every container of that tenant joins. A container on it reaches its
/// own tenant's containers and the internet, and no container of
/// another tenant on any port.
pub fn network_name(workspace_id: &WorkspaceId) -> String {
    format!("pagis-tenant-{workspace_id}")
}

/// The reserved Agent of the tenant's Plugin Computer
/// (ADR-0017): the one container of a tenant that runs that tenant's
/// Plugin MCP servers. Every real Agent id is a ULID, so this name
/// collides with none of them.
pub const PLUGIN_AGENT: &str = "plugins";

/// The Agent id of the tenant's Plugin Computer. The container,
/// the volume and the labels follow from it like any Agent's, so the
/// Plugin Computer is one of that tenant's Docker objects and nobody
/// else's.
pub fn plugin_agent() -> AgentId {
    AgentId::from(PLUGIN_AGENT.to_string())
}

/// One host directory a container mounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindMount {
    pub host: PathBuf,
    pub container: String,
    pub read_only: bool,
}

impl BindMount {
    /// The read-only mount of one Plugin's Skills (ADR-0017). The
    /// Plugin's name, not its id, names the directory the agent sees,
    /// because that is the name the Skills listing uses.
    pub fn skills(mount: &SkillMount) -> Self {
        Self {
            host: mount.skills_dir.clone(),
            container: format!("{PLUGIN_MOUNT_ROOT}/{}/skills", mount.plugin),
            read_only: true,
        }
    }

    /// The `host:container[:ro]` bind specification.
    pub fn spec(&self) -> String {
        let mode = if self.read_only { ":ro" } else { "" };
        format!("{}:{}{mode}", self.host.display(), self.container)
    }
}

/// One name for a whole mount set, short enough for a container label.
/// Two sets with the same fingerprint mount the same directories in
/// the same places.
pub fn mounts_fingerprint(mounts: &[BindMount]) -> String {
    use sha2::{Digest, Sha256};
    let mut specs: Vec<String> = mounts.iter().map(BindMount::spec).collect();
    specs.sort();
    let mut hasher = Sha256::new();
    hasher.update(specs.join("\n").as_bytes());
    hex::encode(hasher.finalize())
}

/// One agent computer's state, as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ComputerState {
    Off,
    Pulling { percent: u8 },
    Starting,
    Awake,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComputerImageState {
    Absent,
    Present,
    Mismatched { found: Option<String> },
    Unavailable { message: String },
}

/// Who drives the computer's input: the daemon-held
/// switch. Exactly one source at a time; the default is the agent.
/// `Daemon` is the vault's held switch (ADR-0013): the daemon fills a
/// secret the model never sees through the browser channel, so
/// model-visible capture is suppressed while it holds. The daemon sends
/// no input batch of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputHolder {
    Agent,
    User,
    Daemon,
}

impl InputHolder {
    pub fn as_str(&self) -> &'static str {
        match self {
            InputHolder::Agent => "agent",
            InputHolder::User => "user",
            InputHolder::Daemon => "daemon",
        }
    }
}

/// Where the Exit Proxy of a Computer opens each connection
/// (ADR-0029).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitMode {
    /// Each connection leaves from the Computer.
    Direct,
    /// Each connection goes to the exit listener of the daemon, which
    /// carries it through the Person's Home Exit, or from the server when
    /// that Host is absent. A literal private address leaves from the
    /// Computer.
    Home,
}

impl ExitMode {
    /// The name on the wire and in `PAGIS_EXIT_MODE`.
    pub fn as_str(self) -> &'static str {
        match self {
            ExitMode::Direct => "direct",
            ExitMode::Home => "home",
        }
    }
}

/// What the Exit Proxy of a Computer reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
pub struct ExitStatus {
    pub mode: ExitMode,
    /// The client connections that the proxy holds now.
    pub connections: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    /// The local image under the pinned tag carries the wrong version
    /// label; the daemon refuses to boot it.
    #[error(
        "the local computer image reports version {found:?}; this daemon needs \
         {IMAGE_VERSION} — remove the stale image and wake again"
    )]
    VersionMismatch { found: Option<String> },
    /// No live frame and no stored screenshot.
    #[error("no screen preview exists for this agent yet")]
    NoPreview,
    /// A live-view offer arrived while the computer is not awake.
    #[error("the computer is not awake")]
    Asleep,
    /// Someone other than the agent holds the input switch:
    /// the agent's screen-lease requests are denied until the
    /// switch comes back.
    #[error("{} has control of this computer", holder.as_str())]
    SwitchHeld { holder: InputHolder },
    /// A sleep asked for while a command runs in the container.
    /// A stop would kill the command, so the user waits for it.
    #[error("this computer is running a command; it stops when the command ends")]
    Busy,
    /// The awake cap is reached. The Run reads this sentence, so
    /// it says what to do and not only what refused.
    #[error(
        "{scope} already has {cap} computers awake, which is the cap; \
         a computer has to sleep before this one can wake"
    )]
    AwakeCapReached { scope: &'static str, cap: u32 },
    /// The Agent is not of this manager's Workspace. A Computer is
    /// the boundary between tenants, so a caller that resolved the wrong
    /// tenant's manager gets nothing: no container starts, and nothing of
    /// this tenant's is named, labelled or counted for a stranger.
    #[error("that sprite is not in this workspace")]
    ForeignAgent,
    /// The Media Relay could not open a path for this viewer,
    /// which on the `daemon` relay means every port of the range is
    /// taken. The machine is full, not the request wrong, so the sentence
    /// says what to do.
    #[error("this screen has no media path now: {reason}")]
    NoMediaPath { reason: String },
    #[error("computer runtime error: {0}")]
    Runtime(String),
}

/// An already-running container plus the image version it runs, so a
/// wake can tell a live computer from a stale one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningComputer {
    pub computer: StartedComputer,
    /// The container's `co.pagis.computer.version` label, inherited
    /// from the image it booted from; `None` when the label is absent.
    pub version: Option<String>,
    /// The container's `co.pagis.computer.mounts` label: the
    /// fingerprint of the mount set it booted with.
    pub mounts: Option<String>,
    /// Whether Docker resolved the running container to the exact
    /// platform image selected by the immutable release reference.
    pub image_matches: bool,
}

/// A booted container's reachable control endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedComputer {
    /// The container the computer runs in, by name or by id. An exec
    /// addresses the container itself, not the control port.
    pub container: String,
    /// `host:port` for the published screend control port.
    pub control_addr: String,
    /// The secret the control endpoint asks for. The daemon
    /// holds one per container and sends it on every control request;
    /// reaching the port is not the same as holding it.
    pub token: String,
}

/// Whether the host holds one part of a Computer's disk to the size the
/// limits name: its volume, or its writable container layer.
///
/// Docker refuses a size where the storage under it has no project
/// quotas, so the answer is a fact about the machine and not a setting.
/// The Health view of the Administration Interface reports both
/// answers. Where one is `unsupported`, nothing bounds that part of any
/// tenant's disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quota {
    /// Docker has not answered, or the limits name no size.
    Unknown,
    Supported,
    Unsupported,
}

impl Quota {
    /// The word the Administration Interface reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
        }
    }
}

/// The Docker seam: bollard in production, a fake in tests.
/// Docker-real coverage lives in `#[ignore]`-tagged tests.
#[async_trait]
pub trait ComputerRuntime: Send + Sync {
    /// The pinned image's version label, or `None` when the image is
    /// not present locally.
    async fn image_version(&self) -> Result<Option<String>, String>;

    /// Pull `image`, reporting whole-pull percentages. The daemon pulls
    /// [`IMAGE`], and an image of its repository by digest that the
    /// Client App asks for.
    async fn pull_image(
        &self,
        image: &str,
        progress: tokio::sync::mpsc::UnboundedSender<u8>,
    ) -> Result<(), String>;

    /// Each local image of the Computer Image repository
    /// ([`image_repository`] of [`IMAGE`]) other than the image that
    /// [`IMAGE`] names. Each name of a listed image is of that
    /// repository, so an image that another repository names is never in
    /// the list. Fails while the pinned image is absent.
    async fn other_images(&self) -> Result<Vec<OtherImage>, String>;

    /// Remove one image by its ID, and never by force. An image that
    /// Docker keeps is [`ImageRemoval::InUse`], not an error.
    async fn remove_image(&self, id: &str) -> Result<ImageRemoval, String>;

    /// The agent's already-running container, when one exists (daemon
    /// restart recovery), with the image version it runs.
    async fn running(&self, owner: &ComputerOwner) -> Result<Option<RunningComputer>, String>;

    /// The Agents of one tenant whose container runs now, found by the
    /// owner labels. A daemon that starts adopts each one, so the list
    /// says nothing about the image or the health of a container.
    async fn running_agents(&self, workspace_id: &WorkspaceId) -> Result<Vec<AgentId>, String>;

    /// Create (Tenant Network, volume, container) as needed, boot, and
    /// wait for the control endpoint to answer. The container joins its
    /// tenant's network alone, so it reaches no container of
    /// another tenant. `mounts` are the host directories
    /// the container carries beside its own volume; the
    /// container records their fingerprint in [`MOUNTS_LABEL`]. `env`
    /// are the entries the container carries, each one `KEY=value`;
    /// every process in it inherits them.
    async fn start(
        &self,
        owner: &ComputerOwner,
        mounts: &[BindMount],
        env: &[String],
    ) -> Result<StartedComputer, String>;

    /// Stop and remove the agent's container. The volume stays.
    async fn stop(&self, owner: &ComputerOwner) -> Result<(), String>;

    /// What one tenant holds on the host: the
    /// containers, the volumes, and the bytes those volumes take. The
    /// Desk Panel reads the disk figure of the one tenant, and the
    /// Administration Interface reads the set per person; neither ever
    /// reads what the server keeps for everybody. One call answers all
    /// three, because the runtime reports them together.
    async fn resources(&self, workspace_id: &WorkspaceId) -> Result<TenantResources, String>;

    /// One current frame from the control endpoint, PNG bytes.
    async fn fetch_frame(&self, computer: &StartedComputer) -> Result<Vec<u8>, String>;

    /// Execute one input batch on the control endpoint, in order.
    /// `holder` declares who the batch speaks for; screend
    /// refuses a batch whose declared holder is not the one that holds
    /// the switch, so the switch means the same at both ends.
    async fn send_input(
        &self,
        computer: &StartedComputer,
        holder: InputHolder,
        ops: &[crate::exec::InputOp],
    ) -> Result<(), String>;

    /// Relay one WebRTC SDP offer to the control endpoint, with
    /// the Media Relay path of this session. screend puts the path's
    /// candidate in its ICE host candidate, never the container's
    /// own address, and registers with the path's port and token.
    /// Returns the SDP answer.
    async fn relay_offer(
        &self,
        computer: &StartedComputer,
        offer: &str,
        path: &crate::MediaPath,
    ) -> Result<String, String>;

    /// Open `url` in the daemon's own tab of the browser and wait for
    /// the load event of the page. The answer is the top-level address
    /// the tab shows after every redirect. screend refuses it unless the
    /// daemon holds the switch.
    async fn browser_open(&self, computer: &StartedComputer, url: &str) -> Result<String, String>;

    /// The top-level address the daemon's own tab shows now, with no
    /// navigation. screend refuses it unless the daemon holds the
    /// switch, and fails when the daemon has no tab open.
    async fn browser_page(&self, computer: &StartedComputer) -> Result<String, String>;

    /// Write `fields` into the page of the daemon's own tab through the
    /// browser channel ([`crate::browser`]). screend writes each value
    /// only while the top-level origin is `origin` and the field it
    /// verified has the focus, and it checks both in the same step as
    /// the write. A failed check writes nothing more and answers with
    /// the reason.
    async fn browser_fill(
        &self,
        computer: &StartedComputer,
        origin: &str,
        fields: &[crate::FillField],
    ) -> Result<(), String>;

    /// Flip the pipeline's input switch: screend drops viewer
    /// data-channel input unless the user holds.
    async fn set_holder(
        &self,
        computer: &StartedComputer,
        holder: InputHolder,
    ) -> Result<(), String>;

    /// Milliseconds since the last user input the pipeline applied;
    /// drives the inactivity auto-handback.
    async fn user_input_idle_ms(&self, computer: &StartedComputer) -> Result<u64, String>;

    /// The mode of the Computer's Exit Proxy, and the client connections
    /// it holds now (ADR-0029).
    async fn exit_status(&self, computer: &StartedComputer) -> Result<ExitStatus, String>;

    /// Set the mode of the Computer's Exit Proxy. The proxy closes every
    /// connection that it holds, also when the mode stays the same, so
    /// each client opens a new connection on the path of the mode. The
    /// answer is how many connections the proxy closed.
    async fn set_exit_mode(
        &self,
        computer: &StartedComputer,
        mode: ExitMode,
    ) -> Result<u64, String>;

    /// Run one command in the agent's container and wait for it.
    /// The runtime runs `argv` as given: the caller owns the
    /// in-container timeout, because the Engine API cannot stop a
    /// running exec.
    async fn exec(
        &self,
        computer: &StartedComputer,
        request: ExecRequest,
    ) -> Result<ExecOutcome, String>;

    /// One long-lived process in the container, with its streams
    /// attached. `request.output_cap` does not apply: the caller
    /// reads the streams to their end, and each stream holds at most
    /// [`EXEC_STREAM_CAPACITY`] chunks for it. The Plugin host starts one
    /// MCP server this way, so no server process runs on the daemon host.
    async fn exec_stream(
        &self,
        computer: &StartedComputer,
        request: ExecRequest,
    ) -> Result<ExecStream, String>;

    /// Extract one uncompressed tar archive into a directory that
    /// already exists in the container. The archive entries
    /// carry their own uid and gid, so a materialized package belongs
    /// to the `agent` uid.
    async fn upload_archive(
        &self,
        computer: &StartedComputer,
        path: &str,
        tar: Vec<u8>,
    ) -> Result<(), String>;

    /// The uncompressed tar of one path in the container, as the
    /// container streams it. The archive entries are relative to the
    /// parent of `path`, so the caller strips the first path element.
    async fn download_archive(
        &self,
        computer: &StartedComputer,
        path: &str,
    ) -> Result<ArchiveStream, String>;

    /// Whether this machine holds a volume to its size. The daemon learns
    /// it when it makes a volume. A runtime that has no answer yet asks
    /// Docker with a probe volume. The answer is [`Quota::Unknown`] only
    /// while Docker does not answer, or when the limits name no volume
    /// size.
    async fn volume_quota(&self) -> Quota;

    /// Whether this machine holds a writable container layer to its
    /// size. The daemon learns it when it creates the container of a
    /// wake, and it makes no probe container. The answer is
    /// [`Quota::Unknown`] until a Computer wakes after the daemon starts,
    /// or when the limits name no layer size.
    async fn container_quota(&self) -> Quota;
}

/// The bytes of one tar as they come out of a container. An error
/// item ends the download.
pub type ArchiveStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = std::io::Result<bytes::Bytes>> + Send>>;

/// How many output chunks of one streaming exec wait for the caller at
/// most: 16 on stdout and 16 on stderr. A chunk is one Docker frame of
/// at most 32 KiB. When a queue is full, the exec stream waits, so a
/// process that writes faster than the caller reads is held back and
/// does not fill the daemon's memory.
pub const EXEC_STREAM_CAPACITY: usize = 16;

/// The attached streams of one long-lived process in a container.
/// A Plugin's MCP server speaks the stdio protocol over them.
///
/// The Engine API cannot kill a running exec, so nothing here stops the
/// process by itself: an MCP server ends when its stdin closes, and the
/// container stop kills whatever is left.
pub struct ExecStream {
    /// The process's stdin. The caller closes it to end the server.
    pub stdin: std::pin::Pin<Box<dyn tokio::io::AsyncWrite + Send>>,
    /// The process's stdout, and only its stdout: the caller reads a
    /// protocol here, so no stderr byte may enter it.
    pub stdout: std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send + Unpin>>,
    /// The process's stderr, chunk by chunk, for the caller's log. The
    /// caller reads it to its end: a full queue holds stdout back too,
    /// because Docker sends both streams in one connection.
    pub stderr: tokio::sync::mpsc::Receiver<Vec<u8>>,
}

/// How much of one command's output the daemon keeps.
/// The first `head` bytes and the last `tail` bytes of a stream
/// survive; what falls between is counted and dropped. The daemon
/// always drains the whole stream, because a command whose output
/// nobody reads blocks on its next write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputCap {
    pub head: usize,
    pub tail: usize,
}

/// One command to run inside an agent's computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    /// The uid the process runs as, by name or by number.
    pub user: String,
    pub cwd: String,
    /// Environment entries, each one `KEY=value`.
    pub env: Vec<String>,
    /// Bytes to write to the process. The daemon shuts the write half
    /// down after them, so the process sees the end of the input.
    pub stdin: Option<Vec<u8>>,
    /// The cap on each of the two output streams. Separate caps keep a
    /// noisy stderr from crowding out the result.
    pub output_cap: OutputCap,
}

/// What one command left behind. The daemon reads `exit_code`
/// only after the exec instance reports that it stopped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecOutcome {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
    /// True when a cap dropped bytes from either stream.
    pub truncated: bool,
}

/// A bounded capture of one output stream: the head, the tail,
/// and a count of the bytes between the two.
#[derive(Debug)]
pub struct CappedOutput {
    cap: OutputCap,
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    dropped: u64,
}

impl CappedOutput {
    pub fn new(cap: OutputCap) -> Self {
        Self {
            cap,
            head: Vec::new(),
            tail: std::collections::VecDeque::new(),
            dropped: 0,
        }
    }

    /// Take the next bytes from the stream. The caller keeps reading
    /// after the cap is full; this drops what does not fit.
    pub fn push(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        let head_room = self.cap.head.saturating_sub(self.head.len());
        if head_room > 0 {
            let take = head_room.min(rest.len());
            self.head.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
        }
        if self.cap.tail == 0 {
            self.dropped += rest.len() as u64;
            return;
        }
        self.tail.extend(rest.iter().copied());
        while self.tail.len() > self.cap.tail {
            self.tail.pop_front();
            self.dropped += 1;
        }
    }

    pub fn truncated(&self) -> bool {
        self.dropped > 0
    }

    /// The kept text, with the truncation marker between the head and
    /// the tail when the cap dropped something.
    pub fn into_text(self) -> String {
        let mut text = String::from_utf8_lossy(&self.head).into_owned();
        if self.dropped > 0 {
            text.push_str(&format!("\n[... {} bytes truncated ...]\n", self.dropped));
        }
        let tail: Vec<u8> = self.tail.into_iter().collect();
        text.push_str(&String::from_utf8_lossy(&tail));
        text
    }
}

#[cfg(test)]
mod tests {
    use super::SECCOMP_PROFILE;

    const USER_NAMESPACE_CALLS: [&str; 4] = ["clone", "clone3", "setns", "unshare"];

    fn profile() -> serde_json::Value {
        serde_json::from_str(SECCOMP_PROFILE).expect("the seccomp profile is JSON")
    }

    fn names(rule: &serde_json::Value) -> Vec<String> {
        rule["names"]
            .as_array()
            .expect("a rule names syscalls")
            .iter()
            .map(|name| name.as_str().expect("a syscall name").to_string())
            .collect()
    }

    /// Chromium's sandbox makes a user namespace. A rule that
    /// carries `args`, `includes` or `excludes` permits the call only
    /// under a condition the container does not meet, so the profile
    /// must permit these four with no condition at all.
    #[test]
    fn the_seccomp_profile_permits_the_user_namespace_calls() {
        let profile = profile();
        let rules = profile["syscalls"].as_array().expect("syscall rules");
        for call in USER_NAMESPACE_CALLS {
            let permitted = rules.iter().any(|rule| {
                rule["action"] == "SCMP_ACT_ALLOW"
                    && rule["args"].as_array().is_none_or(|args| args.is_empty())
                    && rule["includes"]
                        .as_object()
                        .is_none_or(|caps| caps.is_empty())
                    && rule["excludes"]
                        .as_object()
                        .is_none_or(|caps| caps.is_empty())
                    && names(rule).iter().any(|name| name == call)
            });
            assert!(permitted, "the profile does not permit {call}");
        }
    }

    /// libseccomp reads the rules in order, so a later rule that
    /// refuses one of these calls takes the sandbox away again. The
    /// ENOSYS answer Docker's default gives `clone3` is one such rule.
    #[test]
    fn no_seccomp_rule_refuses_a_user_namespace_call() {
        let profile = profile();
        for rule in profile["syscalls"].as_array().expect("syscall rules") {
            if rule["action"] == "SCMP_ACT_ALLOW" {
                continue;
            }
            for name in names(rule) {
                assert!(
                    !USER_NAMESPACE_CALLS.contains(&name.as_str()),
                    "a {} rule refuses {name}",
                    rule["action"]
                );
            }
        }
    }

    /// Everything else stays Docker's default: a syscall no rule names
    /// is still refused.
    #[test]
    fn the_seccomp_profile_refuses_by_default() {
        assert_eq!(profile()["defaultAction"], "SCMP_ACT_ERRNO");
    }

    /// A container boots on the Workspace clock, in the image's locale,
    /// with the Exit Proxy in both cases of each proxy entry, and with
    /// loopback and the Docker host out of the proxy.
    #[test]
    fn the_container_environment_names_the_exit_proxy() {
        assert_eq!(
            super::container_env("Asia/Tokyo", None),
            [
                "TZ=Asia/Tokyo",
                "LANG=en_US.UTF-8",
                "HTTP_PROXY=http://127.0.0.1:3128",
                "HTTPS_PROXY=http://127.0.0.1:3128",
                "http_proxy=http://127.0.0.1:3128",
                "https_proxy=http://127.0.0.1:3128",
                "NO_PROXY=localhost,127.0.0.1,::1,host.docker.internal",
                "no_proxy=localhost,127.0.0.1,::1,host.docker.internal",
            ]
        );
    }

    /// An Agent's Computer on a Server names the exit listener of the
    /// daemon at the Docker host, and the mode its Exit Proxy starts in.
    #[test]
    fn the_container_environment_names_the_exit_listener_and_the_first_mode() {
        for mode in [super::ExitMode::Direct, super::ExitMode::Home] {
            let exit = super::ExitStart {
                daemon: super::exit_daemon(4403),
                mode,
            };
            let env = super::container_env("UTC", Some(&exit));

            assert_eq!(
                env[env.len() - 2..],
                [
                    "PAGIS_EXIT_DAEMON=host.docker.internal:4403".to_string(),
                    format!("PAGIS_EXIT_MODE={}", mode.as_str()),
                ]
            );
        }
        assert_eq!(super::ExitMode::Home.as_str(), "home");
        assert_eq!(
            serde_json::to_value(super::ExitMode::Home).expect("JSON"),
            serde_json::json!("home")
        );
    }

    /// A harness in a Computer reaches the Harness Model Endpoint at the
    /// Docker host, which `NO_PROXY` holds.
    #[test]
    fn the_model_endpoint_is_at_the_docker_host() {
        assert_eq!(
            super::model_endpoint(4404),
            "http://host.docker.internal:4404"
        );
        assert!(
            super::container_env("UTC", None)
                .iter()
                .any(|entry| entry.starts_with("NO_PROXY=") && entry.contains(super::RELAY_HOST))
        );
    }

    /// The repository of a reference has no tag and no digest. The port
    /// of a registry is not a tag.
    #[test]
    fn the_repository_of_a_reference_has_no_tag_and_no_digest() {
        for (reference, repository) in [
            (
                "ghcr.io/pagis-co/pagis-computer@sha256:0123",
                "ghcr.io/pagis-co/pagis-computer",
            ),
            (
                "ghcr.io/pagis-co/pagis-computer:0.17.1",
                "ghcr.io/pagis-co/pagis-computer",
            ),
            (
                "ghcr.io/pagis-co/pagis-computer:0.17.1@sha256:0123",
                "ghcr.io/pagis-co/pagis-computer",
            ),
            (
                "localhost:5000/pagis-computer",
                "localhost:5000/pagis-computer",
            ),
            (
                "localhost:5000/pagis-computer:dev",
                "localhost:5000/pagis-computer",
            ),
        ] {
            assert_eq!(
                super::image_repository(reference),
                repository,
                "{reference}"
            );
        }
    }
}
