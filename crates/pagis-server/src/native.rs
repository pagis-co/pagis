//! Services and routes of the Native Parts.

use crate::{AppState, RemoteAccessSwitch, RestartSwitch, ScreenRelay, SystemConfigFile};
use crate::{calls, session_socket, voice, ws};

pub(crate) mod byte_socket;
pub(crate) mod computers;
pub mod exit_socket;
pub(crate) mod home_exit;
use axum::{
    Router,
    routing::{get, post, put},
};
use std::sync::Arc;

/// The services that need the native feature.
pub struct NativeState {
    /// The exit sockets of the Hosts, which carry the connections of
    /// their Person's Computers as the Home Exit (ADR-0029). The exit
    /// listener opens a stream on them.
    pub home_exits: Arc<pagis_computer::HomeExits>,
    /// The agent computer lifecycle, one manager per tenant.
    /// A handler resolves the manager of the signed-in person's
    /// Workspace, so no handler reaches another tenant's containers.
    pub computers: Arc<pagis_computer::ComputerManagers>,
    /// The Media Relay the live screen goes through. The System Settings
    /// name it, because it does not go through the proxy.
    pub screen: ScreenRelay,
    /// Docker discovery (ADR-0024). One instance serves the runtime
    /// and the System tab, so they name the same endpoint.
    pub docker_discovery: Arc<pagis_computer::DockerDiscovery>,

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
        .route("/api/v1/screen/ice", get(computers::screen_ice))
        .route(
            "/api/v1/agents/{agent_id}/computer",
            get(computers::computer_state),
        )
        .route(
            "/api/v1/agents/{agent_id}/computer/wake",
            post(computers::wake_computer),
        )
        .route(
            "/api/v1/agents/{agent_id}/computer/sleep",
            post(computers::sleep_computer),
        )
        .route("/api/v1/computers/disk", get(computers::computer_disk))
        .route(
            "/api/v1/agents/{agent_id}/screen/preview.png",
            get(computers::screen_preview),
        )
        .route(
            "/api/v1/agents/{agent_id}/screen/offer",
            post(computers::screen_offer),
        )
        .route(
            "/api/v1/agents/{agent_id}/screen/takeover",
            post(computers::screen_takeover),
        )
        .route(
            "/api/v1/agents/{agent_id}/screen/handback",
            post(computers::screen_handback),
        )
        // The Person's own Home Exit (ADR-0029).
        .route(
            "/api/v1/settings/home-exit",
            get(home_exit::get_home_exit)
                .put(home_exit::set_home_exit)
                .delete(home_exit::clear_home_exit),
        )
        .route(
            "/api/v1/settings/onboarding/docker-endpoint",
            put(computers::set_onboarding_docker_endpoint),
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

pub(crate) fn administration_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/administration/resources",
            get(computers::installation_resources),
        )
        .route(
            "/api/v1/administration/computer-image/pull",
            post(computers::pull_computer_image),
        )
}
