//! System settings and the restart switch (ADR-0024). A System
//! Setting belongs to the installation, not to the Workspace: the
//! port, the Docker endpoint, the log level and the data directory. An
//! administrator changes them here and nobody edits a file. Remote
//! Access, which the same view switches, has its own module
//! ([`crate::remote_access`]).
//!
//! The same view switches the anonymous analytics of the installation
//! (ADR-0026) on and off. The change takes effect at the next check of
//! the analytics task, with no restart.
//!
//! On a Server the same view turns the Home Exit off and on for every
//! Person (ADR-0029). Off, every Computer runs in Direct mode and the
//! awake ones switch at once; each Person's choice stays and is in effect
//! again when the setting is on. It never turns a Home Exit on for a
//! Person. A Local Installation has no Home Exit.
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
    /// Whether the People of a Server may use a Home Exit (ADR-0029).
    pub home_exit: bool,
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
    /// Remote Access as the file records it (ADR-0028): the Public
    /// Origin that other machines open while it is on, and `None` while
    /// it is off.
    fn remote_access(&self) -> Result<Option<String>, String>;
    /// Switch Remote Access. `Some` turns it on with that Public Origin
    /// and the Funnel of this machine as the Trusted Proxy; `None` turns
    /// it off and clears both. Both directions bind loopback, where the
    /// Funnel and the owner's Client App reach the daemon.
    fn set_remote_access(&self, public_origin: Option<&str>) -> Result<(), String>;
    /// The Model Request Capture setting as the file records it
    /// (ADR-0031): whether it is on, and its retention in days.
    fn model_request_capture(&self) -> Result<(bool, u32), String>;
    /// Write the Model Request Capture setting.
    fn set_model_request_capture(&self, enabled: bool, retention_days: u32) -> Result<(), String>;
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
    /// `override`, `environment`, `context`, `docker_desktop`,
    /// `orbstack`, `colima`, `rancher_desktop`, `lima`,
    /// `rootless_docker`, `system_socket`, or `podman`.
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

/// The Media Relay of the running daemon as the Settings view shows
/// it (ADR-0014). The live screen goes through neither the Trusted Proxy
/// nor the Funnel of Remote Access, so the view names where it goes.
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

/// The Model Request Capture System Setting (ADR-0031).
#[derive(Debug, Serialize, ToSchema)]
pub struct ModelRequestCaptureSettingDto {
    /// Whether the daemon keeps a copy of each model request of a Run.
    /// Off by default.
    pub enabled: bool,
    /// How many days the daemon keeps a capture, from 1 to 30.
    pub retention_days: u32,
}

/// Turn Model Request Capture on or off, and set its retention.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetModelRequestCaptureRequest {
    pub enabled: bool,
    /// From 1 to 30.
    pub retention_days: u32,
}

/// The Home Exit of the People of a Server (ADR-0029).
#[derive(Debug, Serialize, ToSchema)]
pub struct HomeExitSettingDto {
    /// The System Setting: whether the People may send their Computers'
    /// connections through a Home Exit of their own. On by default.
    pub enabled: bool,
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
    /// The Media Relay the running daemon serves the live screen
    /// through. A change to `[screen]` takes effect at the next start.
    pub screen: ScreenDto,
    pub analytics: AnalyticsDto,
    pub model_request_capture: ModelRequestCaptureSettingDto,
    /// The Home Exit of a Server, or null on a Local Installation, which
    /// has no Home Exit.
    pub home_exit: Option<HomeExitSettingDto>,
}

/// The saved settings, and whether they take effect only after a
/// restart. The port and the log level need one; the Docker endpoint
/// does not.
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

/// Turn the Home Exit off or on for every Person of a Server.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetHomeExitSettingRequest {
    pub enabled: bool,
}

/// The settings after the Home Exit changed, and how many awake
/// Computers did not switch. Each of those keeps its mode and takes the
/// setting at its next wake; the log of the daemon names them.
#[derive(Debug, Serialize, ToSchema)]
pub struct SwitchedHomeExitSettingDto {
    pub settings: SystemSettingsDto,
    pub not_switched: u32,
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
/// installation switches Remote Access.
pub(crate) fn is_local(state: &AppState) -> bool {
    state.client_credential.is_some()
}

fn settings_dto(state: &AppState, config: SystemConfig, report: DockerReport) -> SystemSettingsDto {
    SystemSettingsDto {
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
        screen: (&state.screen).into(),
        analytics: AnalyticsDto {
            enabled: config.analytics,
            blocked: state.analytics_blocked.map(Into::into),
        },
        model_request_capture: ModelRequestCaptureSettingDto {
            enabled: state.capture.is_enabled(),
            retention_days: state.capture.retention_days(),
        },
        home_exit: (!is_local(state)).then_some(HomeExitSettingDto {
            enabled: config.home_exit,
        }),
    }
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
    Ok(Json(settings_dto(&state, config, report)))
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
        home_exit: current.home_exit,
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
        settings: settings_dto(&state, config, report),
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
        settings: settings_dto(&state, config, report),
        restart_required: false,
    }))
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/system/model-request-capture",
    request_body = SetModelRequestCaptureRequest,
    responses(
        (status = 200, body = SavedSystemSettingsDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// Turn Model Request Capture on or off, and set its retention
/// (ADR-0031). The agent loop reads the live setting at each model
/// request, so the change needs no restart. Turning it off deletes every
/// capture of the installation.
pub async fn set_model_request_capture(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Json(request): Json<SetModelRequestCaptureRequest>,
) -> Result<Json<SavedSystemSettingsDto>, ApiError> {
    let days = request.retention_days;
    if !(1..=pagis_core::MAX_CAPTURE_RETENTION_DAYS).contains(&days) {
        return Err(ApiError::validation(format!(
            "the retention must be from 1 to {} days",
            pagis_core::MAX_CAPTURE_RETENTION_DAYS
        )));
    }
    state
        .system
        .set_model_request_capture(request.enabled, days)
        .map_err(|error| {
            tracing::error!(%error, "cannot write the config file");
            ApiError::internal()
        })?;
    state.capture.set(request.enabled, days);
    if !request.enabled {
        let deleted = state.model_request_captures.delete_all().await?;
        tracing::info!(deleted, "turning the capture off deleted every capture");
    }
    tracing::info!(
        enabled = request.enabled,
        retention_days = days,
        "the administrator switched the model request capture"
    );
    let config = state.system.read().map_err(|error| {
        tracing::error!(%error, "cannot read the config file");
        ApiError::internal()
    })?;
    let report = state.docker_discovery.probe().await;
    Ok(Json(SavedSystemSettingsDto {
        settings: settings_dto(&state, config, report),
        restart_required: false,
    }))
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/system/home-exit",
    request_body = SetHomeExitSettingRequest,
    responses(
        (status = 200, body = SwitchedHomeExitSettingDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, description = "A Local Installation has no Home Exit", body = crate::error::ErrorBody),
    )
)]
/// Turn the Home Exit off or on for every Person of a Server
/// (ADR-0029). The daemon writes the setting, and each awake Computer
/// whose mode changes switches at once, with no restart.
pub async fn set_home_exit_setting(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Json(request): Json<SetHomeExitSettingRequest>,
) -> Result<Json<SwitchedHomeExitSettingDto>, ApiError> {
    if is_local(&state) {
        return Err(ApiError::conflict(
            "a Local Installation has no Home Exit: its Computers reach the internet from this              machine's own connection",
        ));
    }
    let mut config = state.system.read().map_err(|error| {
        tracing::error!(%error, "cannot read the config file");
        ApiError::internal()
    })?;
    config.home_exit = request.enabled;
    state.system.write(&config).map_err(|error| {
        tracing::error!(%error, "cannot write the config file");
        ApiError::internal()
    })?;
    state.home_exits.set_enabled(request.enabled);
    tracing::info!(
        enabled = request.enabled,
        "the administrator switched the Home Exit"
    );
    let not_switched = state.computers.settle_exits().await.len();
    let report = state.docker_discovery.probe().await;
    Ok(Json(SwitchedHomeExitSettingDto {
        settings: settings_dto(&state, config, report),
        not_switched: u32::try_from(not_switched).unwrap_or(u32::MAX),
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
