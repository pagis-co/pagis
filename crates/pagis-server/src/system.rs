//! System settings and the restart switch (ADR-0024). A System
//! Setting belongs to the installation, not to the Workspace: the
//! port, the Docker endpoint, the log level and the data directory. An
//! administrator changes them here and nobody edits a file.
//!
//! The same form switches the multi-user mode of a local installation:
//! the Public Origin and the Trusted Proxy that let People on other
//! machines reach it through the owner's proxy or tunnel. The mode is
//! derived from the Public Origin, so no flag of its own is stored.
//!
//! The same view switches the anonymous analytics of the installation
//! (ADR-0026) on and off. The change takes effect at the next check of
//! the analytics task, with no restart.
//!
//! Every route here answers on the Administration Port alone and takes
//! the [`Administrator`] extractor: a Member reads nothing of the
//! installation and changes nothing of it.

use std::net::IpAddr;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Json;
use axum::extract::State;
use pagis_computer::DockerReport;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Administrator;
use crate::error::ApiError;

/// The exit code that means "the user asked for a restart"
/// (ADR-0024). The desktop shell starts the daemon again on it, and
/// the CLI prints "run `pagis` again".
pub const RESTART_EXIT_CODE: i32 = 75;

/// The log levels the daemon accepts, lowest first.
const LOG_LEVELS: [&str; 5] = ["trace", "debug", "info", "warn", "error"];

/// The lowest port the daemon binds. Ports under 1024 need root.
const MIN_PORT: u16 = 1024;

/// The System Settings that live in the config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemConfig {
    pub port: u16,
    /// The Docker endpoint the user typed, which wins over discovery.
    pub docker_endpoint: Option<String>,
    pub log_level: String,
    /// Whether a release build sends anonymous analytics (ADR-0026).
    pub analytics: bool,
}

/// The network settings of an installation in the multi-user mode
/// (ADR-0024): the Public Origin people open, whose host is not
/// loopback, and the Trusted Proxy that forwards their requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiUserMode {
    pub public_origin: String,
    pub trusted_proxy: Option<IpAddr>,
}

/// Which Media Relay carries the live screen (ADR-0014): `daemon`
/// forwards media in the daemon, and `turn` puts an external TURN server
/// in front of the browser leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaRelayKind {
    Daemon,
    Turn,
}

/// The Media Relay the running daemon serves the live screen through
/// (ADR-0014): the `[screen]` section of the config file, with the
/// environment overrides of this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenRelay {
    pub relay: MediaRelayKind,
    /// The address the relay gives to browsers, or to the TURN server of
    /// a `turn` relay.
    pub advertise_ip: String,
    /// The UDP ports the relay binds, one for each viewer session.
    pub media_ports: RangeInclusive<u16>,
}

/// The config-file seam. The daemon owns the file, so the API reads
/// and writes it through this one path.
pub trait SystemConfigFile: Send + Sync {
    fn read(&self) -> Result<SystemConfig, String>;
    fn write(&self, config: &SystemConfig) -> Result<(), String>;
    /// The multi-user mode the file configures, derived from its Public
    /// Origin. `None` is an installation that serves its own machine.
    fn multi_user(&self) -> Result<Option<MultiUserMode>, String>;
    /// Switch the multi-user mode. Both directions bind loopback: the
    /// owner's proxy or tunnel on the same machine reaches the daemon
    /// there. `None` also clears the Public Origin and the Trusted Proxy.
    fn set_multi_user(&self, mode: Option<&MultiUserMode>) -> Result<(), String>;
    /// The data directory. The user reads it and never sets it.
    fn data_directory(&self) -> PathBuf;
}

/// The daemon's restart request (ADR-0024). The API asks; the process
/// stops serving and exits with [`RESTART_EXIT_CODE`].
#[derive(Debug, Default)]
pub struct RestartSwitch {
    asked: AtomicBool,
    notify: Notify,
}

impl RestartSwitch {
    /// Ask the daemon to stop and start again.
    pub fn ask(&self) {
        self.asked.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Wait until somebody asks. The graceful shutdown waits on this.
    pub async fn asked(&self) {
        let waiter = self.notify.notified();
        if self.is_asked() {
            return;
        }
        waiter.await;
    }

    pub fn is_asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }
}

/// One candidate endpoint and what it answered.
#[derive(Debug, Serialize, ToSchema)]
pub struct DockerCandidateDto {
    /// `override`, `environment`, `context`, `docker_run`, `colima`,
    /// or `system_socket`.
    pub source: String,
    pub endpoint: String,
    pub reachable: bool,
    /// Why the candidate did not answer, when it did not.
    pub error: Option<String>,
}

/// Docker discovery as the System tab shows it.
#[derive(Debug, Serialize, ToSchema)]
pub struct DockerReportDto {
    /// The endpoint in use: the first candidate that answered. Null
    /// means Pagis found no Docker, and agent computers do not run.
    pub endpoint: Option<String>,
    pub candidates: Vec<DockerCandidateDto>,
}

impl From<DockerReport> for DockerReportDto {
    fn from(report: DockerReport) -> Self {
        Self {
            endpoint: report.endpoint,
            candidates: report
                .candidates
                .into_iter()
                .map(|result| DockerCandidateDto {
                    source: result.candidate.source.as_str().to_string(),
                    endpoint: result.candidate.endpoint,
                    reachable: result.error.is_none(),
                    error: result.error,
                })
                .collect(),
        }
    }
}

/// The multi-user mode as the Settings view shows it (ADR-0024).
#[derive(Debug, Serialize, ToSchema)]
pub struct MultiUserDto {
    /// Whether the installation serves People on other machines: its
    /// Public Origin host is not loopback.
    pub enabled: bool,
    /// The Public Origin people open. Null when the mode is off.
    pub public_origin: Option<String>,
    /// The address whose forwarded headers the daemon believes, or null.
    pub trusted_proxy: Option<String>,
    /// Whether an Administrator can switch the mode here. Only a local
    /// installation can: a server always serves a network, and its
    /// deployment names the Public Origin.
    pub switchable: bool,
}

impl MultiUserDto {
    fn of(mode: Option<MultiUserMode>, switchable: bool) -> Self {
        match mode {
            Some(mode) => Self {
                enabled: true,
                public_origin: Some(mode.public_origin),
                trusted_proxy: mode.trusted_proxy.map(|address| address.to_string()),
                switchable,
            },
            None => Self {
                enabled: false,
                public_origin: None,
                trusted_proxy: None,
                switchable,
            },
        }
    }
}

/// The Media Relay of the running daemon as the Settings view shows
/// it (ADR-0014). The live screen does not go through the proxy or
/// tunnel of the multi-user mode, so the view names where it goes.
#[derive(Debug, Serialize, ToSchema)]
pub struct ScreenDto {
    pub relay: MediaRelayKind,
    /// `[screen] advertise_ip`: the address the relay gives to browsers.
    pub advertise_ip: String,
    /// Whether `advertise_ip` is loopback. Then only this machine
    /// reaches the `daemon` relay.
    pub loopback: bool,
    /// The first UDP port of the relay's range.
    pub media_port_first: u16,
    /// The last UDP port of the relay's range.
    pub media_port_last: u16,
}

impl From<&ScreenRelay> for ScreenDto {
    fn from(screen: &ScreenRelay) -> Self {
        let address = screen.advertise_ip.trim();
        Self {
            relay: screen.relay,
            advertise_ip: address.to_string(),
            loopback: address.eq_ignore_ascii_case("localhost")
                || address
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback()),
            media_port_first: *screen.media_ports.start(),
            media_port_last: *screen.media_ports.end(),
        }
    }
}

/// Why a daemon sends no analytics whatever the setting says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticsBlockedDto {
    /// The build holds no PostHog project: a build from source or a
    /// development build.
    Build,
    /// The environment of the daemon sets `DO_NOT_TRACK`.
    DoNotTrack,
}

impl From<pagis_analytics::Blocked> for AnalyticsBlockedDto {
    fn from(blocked: pagis_analytics::Blocked) -> Self {
        match blocked {
            pagis_analytics::Blocked::Build => Self::Build,
            pagis_analytics::Blocked::DoNotTrack => Self::DoNotTrack,
        }
    }
}

/// The anonymous analytics of the installation (ADR-0026).
#[derive(Debug, Serialize, ToSchema)]
pub struct AnalyticsDto {
    /// The System Setting: whether the Administrator lets the daemon
    /// send. On by default.
    pub enabled: bool,
    /// Why the daemon sends nothing whatever `enabled` says, or null
    /// when it sends while `enabled` is true.
    pub blocked: Option<AnalyticsBlockedDto>,
}

/// Every System Setting in one read.
#[derive(Debug, Serialize, ToSchema)]
pub struct SystemSettingsDto {
    /// The product port that `config.toml` names.
    pub port: u16,
    /// The product port the daemon listens on.
    pub listening_port: u16,
    /// What sets `listening_port` over the file for this run: `--port`
    /// or `PAGIS_PORT`. Null when the file sets it. A port that differs
    /// with no override is a change that the next start puts in effect.
    pub port_override: Option<String>,
    /// Whether a supervisor (the Client App, or the restart policy of
    /// the compose deployment) starts the daemon again after it exits
    /// for a restart. Without one, a person runs `pagis` again.
    pub supervised: bool,
    /// When this daemon process started, in Unix milliseconds. A
    /// restart changes it.
    pub started_at: i64,
    /// The Docker endpoint the user typed, or null for discovery.
    pub docker_endpoint: Option<String>,
    pub log_level: String,
    /// Read-only: where the daemon keeps its data.
    pub data_directory: String,
    /// The running daemon's version. The Client App reads it to know
    /// whether it may attach to a daemon it did not start.
    pub version: String,
    pub docker: DockerReportDto,
    pub multi_user: MultiUserDto,
    /// The Media Relay the running daemon serves the live screen
    /// through. A change to `[screen]` takes effect at the next start.
    pub screen: ScreenDto,
    pub analytics: AnalyticsDto,
}

/// The saved settings, and whether they take effect only after a
/// restart. The port, the log level and the multi-user mode need one;
/// the Docker endpoint does not.
#[derive(Debug, Serialize, ToSchema)]
pub struct SavedSystemSettingsDto {
    pub settings: SystemSettingsDto,
    pub restart_required: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateSystemSettingsRequest {
    pub port: u16,
    /// A socket path, a `unix://` or a `tcp://` endpoint. Null clears
    /// the override and gives discovery the choice back. Pagis pings
    /// the endpoint before it saves.
    pub docker_endpoint: Option<String>,
    pub log_level: String,
}

/// Turn the anonymous analytics on or off.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetAnalyticsRequest {
    pub enabled: bool,
}

/// Turn the multi-user mode on.
#[derive(Debug, Deserialize, ToSchema)]
pub struct EnableMultiUserRequest {
    /// The absolute `https://` or `http://` origin that the owner's proxy
    /// or tunnel answers on, such as `https://pagis.example.net`. Its host
    /// is not loopback.
    pub public_origin: String,
    /// The IP address the proxy or tunnel reaches the daemon from:
    /// `127.0.0.1` for one on this machine. Null or empty believes no
    /// forwarded header.
    pub trusted_proxy: Option<String>,
}

/// The daemon's answer to a restart request.
#[derive(Debug, Serialize, ToSchema)]
pub struct RestartDto {
    /// The code the daemon exits with. The shell starts it again on
    /// this code.
    pub exit_code: i32,
}

/// Whether this installation is local: one started with `--local`,
/// which holds a Client Credential (ADR-0025). Only a local
/// installation switches the multi-user mode.
fn is_local(state: &AppState) -> bool {
    state.client_credential.is_some()
}

/// Whether the running daemon serves this machine alone: a local
/// installation (ADR-0025) that runs with the multi-user mode off
/// (ADR-0024). It has one Person, and it answers only programs of its
/// own machine.
///
/// A switch of the mode writes `config.toml` and takes effect at the
/// next start, so the answer holds for the life of the process. The
/// guard that refuses other machines and the start route of a Google
/// authorization both read it here.
pub fn serves_this_machine_only(state: &AppState) -> bool {
    is_local(state) && running_multi_user(state).is_none()
}

/// The multi-user mode the running daemon serves.
fn running_multi_user(state: &AppState) -> Option<MultiUserMode> {
    (!origin_host_is_loopback(&state.public_origin)).then(|| MultiUserMode {
        public_origin: state.public_origin.clone(),
        trusted_proxy: state.proxy.address(),
    })
}

/// The multi-user mode as the Settings view shows it. A local
/// installation shows what its file configures, which a restart puts
/// in effect. A server shows what it runs with, because its deployment
/// names the Public Origin in the environment and not in the file.
fn multi_user_dto(state: &AppState) -> Result<MultiUserDto, ApiError> {
    if !is_local(state) {
        return Ok(MultiUserDto::of(running_multi_user(state), false));
    }
    let mode = state.system.multi_user().map_err(|error| {
        tracing::error!(%error, "cannot read the config file");
        ApiError::internal()
    })?;
    Ok(MultiUserDto::of(mode, true))
}

async fn settings_dto(
    state: &Arc<AppState>,
    config: SystemConfig,
    report: DockerReport,
) -> Result<SystemSettingsDto, ApiError> {
    Ok(SystemSettingsDto {
        port: config.port,
        listening_port: state.runtime_port,
        port_override: state.port_override.map(str::to_string),
        supervised: state.supervised,
        started_at: state.started_at,
        docker_endpoint: config.docker_endpoint,
        log_level: config.log_level,
        data_directory: state.system.data_directory().display().to_string(),
        version: crate::VERSION.to_string(),
        docker: report.into(),
        multi_user: multi_user_dto(state)?,
        screen: (&state.screen).into(),
        analytics: AnalyticsDto {
            enabled: config.analytics,
            blocked: state.analytics_blocked.map(Into::into),
        },
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/settings/system",
    responses(
        (status = 200, body = SystemSettingsDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
pub async fn get_system_settings(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<SystemSettingsDto>, ApiError> {
    let config = state.system.read().map_err(|error| {
        tracing::error!(%error, "cannot read the config file");
        ApiError::internal()
    })?;
    let report = state.docker_discovery.probe().await;
    Ok(Json(settings_dto(&state, config, report).await?))
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/system",
    request_body = UpdateSystemSettingsRequest,
    responses(
        (status = 200, body = SavedSystemSettingsDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn set_system_settings(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Json(request): Json<UpdateSystemSettingsRequest>,
) -> Result<Json<SavedSystemSettingsDto>, ApiError> {
    if request.port < MIN_PORT {
        return Err(ApiError::validation(format!(
            "the port must be {MIN_PORT} or higher"
        )));
    }
    let log_level = request.log_level.trim().to_lowercase();
    if !LOG_LEVELS.contains(&log_level.as_str()) {
        return Err(ApiError::validation(format!(
            "the log level must be one of {}",
            LOG_LEVELS.join(", ")
        )));
    }
    let docker_endpoint =
        checked_docker_endpoint(&state, request.docker_endpoint.as_deref()).await?;

    let current = state.system.read().map_err(|error| {
        tracing::error!(%error, "cannot read the config file");
        ApiError::internal()
    })?;
    let config = SystemConfig {
        port: request.port,
        docker_endpoint,
        log_level,
        analytics: current.analytics,
    };
    state.system.write(&config).map_err(|error| {
        tracing::error!(%error, "cannot write the config file");
        ApiError::internal()
    })?;
    // The endpoint takes effect on the runtime's next connect, with no
    // restart (ADR-0024).
    state
        .docker_discovery
        .set_override(config.docker_endpoint.clone());
    let restart_required = config.port != current.port || config.log_level != current.log_level;
    let report = state.docker_discovery.probe().await;
    Ok(Json(SavedSystemSettingsDto {
        settings: settings_dto(&state, config, report).await?,
        restart_required,
    }))
}

/// The Docker endpoint a person typed, parsed and pinged. An override
/// is pinged before it is saved (ADR-0024), so a typing mistake never
/// leaves the computers unreachable. An empty entry gives discovery the
/// choice back.
pub(crate) async fn checked_docker_endpoint(
    state: &AppState,
    raw: Option<&str>,
) -> Result<Option<String>, ApiError> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(raw) => {
            let endpoint = pagis_computer::docker::parse_endpoint(raw).map_err(|error| {
                ApiError::validation(format!("that Docker endpoint is not usable: {error}"))
            })?;
            state
                .docker_discovery
                .ping(&endpoint)
                .await
                .map_err(|error| {
                    ApiError::validation(format!("Docker did not answer at {endpoint}: {error}"))
                })?;
            Ok(Some(endpoint))
        }
    }
}

/// Save the Docker endpoint and keep every other System Setting. The
/// endpoint takes effect on the runtime's next connect, with no restart
/// (ADR-0024).
pub(crate) fn save_docker_endpoint(
    state: &AppState,
    endpoint: Option<String>,
) -> Result<(), ApiError> {
    let mut config = state.system.read().map_err(|error| {
        tracing::error!(%error, "cannot read the config file");
        ApiError::internal()
    })?;
    config.docker_endpoint = endpoint;
    state.system.write(&config).map_err(|error| {
        tracing::error!(%error, "cannot write the config file");
        ApiError::internal()
    })?;
    state.docker_discovery.set_override(config.docker_endpoint);
    Ok(())
}

/// Whether the host of an origin is loopback: `localhost` or a loopback
/// address. An origin that does not parse names no host and is not
/// loopback.
pub fn origin_host_is_loopback(origin: &str) -> bool {
    let Ok(url) = url::Url::parse(origin.trim()) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// The Public Origin an Administrator typed, as the origin it names.
///
/// It is an absolute `https://` or `http://` URL with a scheme, a host
/// and an optional port, and nothing else. Its host is one that people
/// on other machines open: not loopback, and not the unspecified
/// address.
pub(crate) fn checked_public_origin(raw: &str) -> Result<String, ApiError> {
    let raw = raw.trim();
    let example = "such as https://pagis.example.net";
    let url = url::Url::parse(raw).map_err(|_| {
        ApiError::validation(format!(
            "{raw:?} is not an absolute URL; the Public Origin is the address people open, \
             {example}"
        ))
    })?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err(ApiError::validation(format!(
            "the Public Origin starts with https:// or http://, {example}"
        )));
    }
    let unspecified = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_unspecified(),
        Some(url::Host::Ipv6(address)) => address.is_unspecified(),
        Some(url::Host::Domain(_)) => false,
        None => true,
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ApiError::validation(format!(
            "the Public Origin is a scheme, a host and an optional port, with no path, {example}"
        )));
    }
    if unspecified || origin_host_is_loopback(raw) {
        return Err(ApiError::validation(format!(
            "{raw} is not an address people on other machines can open; the Public Origin is \
             the name your proxy or tunnel answers on, {example}"
        )));
    }
    Ok(url.origin().ascii_serialization())
}

/// The Trusted Proxy an Administrator typed. An empty entry believes no
/// forwarded header.
pub(crate) fn checked_trusted_proxy(raw: Option<&str>) -> Result<Option<IpAddr>, ApiError> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(raw) => raw.parse().map(Some).map_err(|_| {
            ApiError::validation(format!(
                "the Trusted Proxy is {raw:?}; it is the IP address your proxy or tunnel \
                 reaches Pagis from, such as 127.0.0.1"
            ))
        }),
    }
}

/// Refuse a switch on a server. A server always serves a network: its
/// deployment names the Public Origin, and the headless image refuses
/// to start without one.
fn require_local(state: &AppState) -> Result<(), ApiError> {
    if is_local(state) {
        return Ok(());
    }
    Err(ApiError::conflict(
        "this Pagis is a server, which always serves several People; its deployment names \
         the Public Origin in PAGIS_PUBLIC_ORIGIN",
    ))
}

/// Write the mode and answer the settings. A restart is required
/// wherever the mode now differs from the one the daemon runs with.
async fn switch_multi_user(
    state: &Arc<AppState>,
    mode: Option<MultiUserMode>,
) -> Result<SavedSystemSettingsDto, ApiError> {
    state
        .system
        .set_multi_user(mode.as_ref())
        .map_err(|error| {
            tracing::error!(%error, "cannot write the config file");
            ApiError::internal()
        })?;
    tracing::info!(
        public_origin = mode.as_ref().map(|mode| mode.public_origin.as_str()),
        "the administrator switched the multi-user mode"
    );
    let restart_required = mode != running_multi_user(state);
    let config = state.system.read().map_err(|error| {
        tracing::error!(%error, "cannot read the config file");
        ApiError::internal()
    })?;
    let report = state.docker_discovery.probe().await;
    Ok(SavedSystemSettingsDto {
        settings: settings_dto(state, config, report).await?,
        restart_required,
    })
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/system/multi-user",
    request_body = EnableMultiUserRequest,
    responses(
        (status = 200, body = SavedSystemSettingsDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "A server, which always serves a network"),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// Turn the multi-user mode on (ADR-0024): write the Public Origin and
/// the Trusted Proxy, and keep the Bind Address on loopback, where the
/// owner's proxy or tunnel on this machine reaches the daemon. The
/// change takes effect on the next start.
pub async fn enable_multi_user(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Json(request): Json<EnableMultiUserRequest>,
) -> Result<Json<SavedSystemSettingsDto>, ApiError> {
    require_local(&state)?;
    let mode = MultiUserMode {
        public_origin: checked_public_origin(&request.public_origin)?,
        trusted_proxy: checked_trusted_proxy(request.trusted_proxy.as_deref())?,
    };
    Ok(Json(switch_multi_user(&state, Some(mode)).await?))
}

#[utoipa::path(
    delete,
    path = "/api/v1/settings/system/multi-user",
    responses(
        (status = 200, body = SavedSystemSettingsDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "A server, which always serves a network"),
    )
)]
/// Turn the multi-user mode off (ADR-0024): clear the Public Origin and
/// the Trusted Proxy, and bind loopback. People who signed in over the
/// network keep their accounts and reach nothing until the mode is on
/// again. The change takes effect on the next start.
pub async fn disable_multi_user(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<SavedSystemSettingsDto>, ApiError> {
    require_local(&state)?;
    Ok(Json(switch_multi_user(&state, None).await?))
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/system/analytics",
    request_body = SetAnalyticsRequest,
    responses(
        (status = 200, body = SavedSystemSettingsDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
/// Turn the anonymous analytics of the installation on or off
/// (ADR-0026). The analytics task reads the setting at each check, so
/// the change needs no restart.
pub async fn set_analytics(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Json(request): Json<SetAnalyticsRequest>,
) -> Result<Json<SavedSystemSettingsDto>, ApiError> {
    let mut config = state.system.read().map_err(|error| {
        tracing::error!(%error, "cannot read the config file");
        ApiError::internal()
    })?;
    config.analytics = request.enabled;
    state.system.write(&config).map_err(|error| {
        tracing::error!(%error, "cannot write the config file");
        ApiError::internal()
    })?;
    tracing::info!(
        enabled = request.enabled,
        "the administrator switched the analytics"
    );
    let report = state.docker_discovery.probe().await;
    Ok(Json(SavedSystemSettingsDto {
        settings: settings_dto(&state, config, report).await?,
        restart_required: false,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/settings/system/docker/probe",
    responses(
        (status = 200, body = DockerReportDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
pub async fn probe_docker(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<DockerReportDto>, ApiError> {
    Ok(Json(state.docker_discovery.probe().await.into()))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/restart",
    responses(
        (status = 200, body = RestartDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
    )
)]
pub async fn restart(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Json<RestartDto> {
    tracing::info!("the user asked for a restart");
    state.restart.ask();
    Json(RestartDto {
        exit_code: RESTART_EXIT_CODE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_switch_wakes_a_waiter() {
        let switch = Arc::new(RestartSwitch::default());
        let waiting = tokio::spawn({
            let switch = Arc::clone(&switch);
            async move { switch.asked().await }
        });

        switch.ask();

        tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
            .await
            .expect("the waiter wakes")
            .expect("the task ends");
        assert!(switch.is_asked());
    }

    /// Only a loopback address keeps the live screen on this machine:
    /// every form of it, and no other address.
    #[test]
    fn the_screen_says_whether_the_advertised_address_is_loopback() {
        for (address, loopback) in [
            ("127.0.0.1", true),
            (" 127.0.0.2 ", true),
            ("::1", true),
            ("localhost", true),
            ("192.168.86.55", false),
            ("100.101.102.103", false),
            ("0.0.0.0", false),
        ] {
            let screen = ScreenDto::from(&ScreenRelay {
                relay: MediaRelayKind::Daemon,
                advertise_ip: address.to_string(),
                media_ports: 50000..=50099,
            });
            assert_eq!(screen.loopback, loopback, "{address}");
            assert_eq!(screen.advertise_ip, address.trim());
            assert_eq!(
                (screen.media_port_first, screen.media_port_last),
                (50000, 50099)
            );
        }
    }

    #[test]
    fn a_public_origin_is_an_absolute_http_url_with_a_host_people_open() {
        for (raw, origin) in [
            ("https://pagis.example.net", "https://pagis.example.net"),
            ("https://pagis.example.net/", "https://pagis.example.net"),
            (
                "  https://Pagis.Example.net:8443  ",
                "https://pagis.example.net:8443",
            ),
            ("http://192.168.1.20:4400", "http://192.168.1.20:4400"),
            (
                "https://owner.tail1234.ts.net",
                "https://owner.tail1234.ts.net",
            ),
        ] {
            assert_eq!(checked_public_origin(raw).unwrap(), origin, "{raw}");
        }
    }

    #[test]
    fn a_public_origin_that_is_not_an_address_people_open_is_refused() {
        for raw in [
            "",
            "pagis.example.net",
            "/pagis",
            "ftp://pagis.example.net",
            "https://pagis.example.net/pagis",
            "https://pagis.example.net/?a=b",
            "https://owner:secret@pagis.example.net",
            "http://localhost:4400",
            "http://127.0.0.1:4400",
            "http://[::1]:4400",
            "http://0.0.0.0:4400",
        ] {
            let refused = checked_public_origin(raw).expect_err(raw);
            assert_eq!(refused.code, "validation", "{raw}");
            assert!(
                refused.message.contains("https://pagis.example.net"),
                "{raw}"
            );
        }
    }

    #[test]
    fn a_trusted_proxy_is_an_ip_address_or_nothing() {
        assert_eq!(checked_trusted_proxy(None).unwrap(), None);
        assert_eq!(checked_trusted_proxy(Some("  ")).unwrap(), None);
        assert_eq!(
            checked_trusted_proxy(Some("127.0.0.1")).unwrap(),
            Some("127.0.0.1".parse().unwrap())
        );
        assert_eq!(
            checked_trusted_proxy(Some("::1")).unwrap(),
            Some("::1".parse().unwrap())
        );
        let refused = checked_trusted_proxy(Some("proxy.local")).expect_err("a name");
        assert!(
            refused.message.contains("IP address"),
            "{}",
            refused.message
        );
    }

    #[test]
    fn an_origin_on_this_machine_is_loopback() {
        assert!(origin_host_is_loopback("http://localhost:4400"));
        assert!(origin_host_is_loopback("http://LOCALHOST"));
        assert!(origin_host_is_loopback("http://127.0.0.1:4400"));
        assert!(origin_host_is_loopback("http://[::1]:4400"));
        assert!(!origin_host_is_loopback("https://pagis.example.net"));
        assert!(!origin_host_is_loopback("http://192.168.1.20:4400"));
        assert!(!origin_host_is_loopback("not an origin"));
    }

    #[tokio::test]
    async fn a_switch_asked_before_the_wait_returns_at_once() {
        let switch = RestartSwitch::default();
        switch.ask();

        tokio::time::timeout(std::time::Duration::from_secs(5), switch.asked())
            .await
            .expect("the wait ends");
    }
}
