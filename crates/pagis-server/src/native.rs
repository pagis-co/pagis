//! Services and product routes of the Native Parts.

use crate::{AppState, RemoteAccessSwitch, RestartSwitch, SystemConfigFile};
use crate::{agents, calls, exit_socket, session_socket, settings, voice, ws};
use axum::{
    Router,
    routing::{get, put},
};
use std::sync::Arc;

/// The services that need the native feature.
pub struct NativeState {
    /// Why this daemon sends no analytics whatever the System Setting
    /// says (ADR-0026), or `None` for a release build that can send.
    pub analytics_blocked: Option<pagis_analytics::Blocked>,
    /// The Remote Access switch of a local installation, over the
    /// Tailscale of this machine.
    pub remote_access_switch: RemoteAccessSwitch,
    /// The TURN server of Remote Access, which carries the live screen to
    /// a browser on another machine over the Funnel (ADR-0028). `Some`
    /// while the daemon runs in Remote Access, and `None` otherwise.
    pub remote_access_turn: Option<Arc<pagis_computer::RemoteAccessTurn>>,
    /// The config file the System tab reads and writes.
    pub system: Arc<dyn SystemConfigFile>,
    /// The restart the System tab asks for.
    pub restart: Arc<RestartSwitch>,
}

pub(crate) fn product_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/screen/ice", get(agents::screen_ice))
        .route(
            "/api/v1/settings/onboarding",
            get(settings::onboarding_status),
        )
        .route(
            "/api/v1/settings/onboarding/docker-endpoint",
            put(settings::set_onboarding_docker_endpoint),
        )
        .route("/api/v1/ws", get(ws::upgrade))
        .route("/api/v1/hosts/{host_id}/exit", get(exit_socket::upgrade))
        .route(
            "/api/v1/hosts/{host_id}/sessions",
            get(session_socket::upgrade),
        )
        .route("/api/v1/channels/{channel_id}/dictate", get(voice::dictate))
        .route("/api/v1/calls/{call_id}/listen", get(calls::listen))
}
