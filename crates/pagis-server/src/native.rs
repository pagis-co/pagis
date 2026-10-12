//! Services and routes of the Native Parts.

use crate::{AppState, RemoteAccessSwitch, RestartSwitch, SystemConfigFile};
use crate::{
    agents, calls, exit_socket, plugins, session_socket, settings, software, voice, widgets, ws,
};
use axum::{
    Router,
    routing::{get, post, put},
};
use std::sync::Arc;

/// The services that need the native feature.
pub struct NativeState {
    /// The Version tree behind the records: the manifest of one
    /// Version and the bytes of one file of it. A Widget page reads
    /// through it.
    pub versions: Arc<dyn pagis_software::VersionSource>,
    /// The installed Plugins (ADR-0017). The Administration Interface writes
    /// them: installing one decides which code runs on this computer.
    pub plugins: Arc<pagis_plugin::Plugins>,
    /// The MCP hosts of those Plugins (ADR-0017), one per
    /// tenant. A handler resolves the host of the signed-in person's
    /// Workspace: the desk starts a failed Plugin again in that person's
    /// own Plugin Computer and reads that Computer's server log.
    pub plugin_hosts: Arc<pagis_plugins::PluginHosts>,

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
        .route("/api/v1/plugins", get(plugins::list_plugins))
        .route("/api/v1/plugins/{plugin_id}", get(plugins::get_plugin))
        .route(
            "/api/v1/plugins/{plugin_id}/grants",
            post(plugins::grant_plugin),
        )
        .route("/api/v1/plugins/{plugin_id}/log", get(plugins::plugin_log))
        .route(
            "/api/v1/widgets/{package}/{version}/{widget}",
            get(widgets::widget_page),
        )
        .route(
            "/api/v1/widgets/{tool_call_id}/view",
            get(widgets::widget_view),
        )
        .route(
            "/api/v1/widgets/{tool_call_id}/rpc",
            post(widgets::widget_rpc),
        )
        .route("/api/v1/software", get(software::list_software))
        .route("/api/v1/software/{name}", get(software::get_software))
        .route(
            "/api/v1/software/{name}/contributions/{contribution_id}",
            get(software::get_contribution),
        )
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

/// The Plugin routes inside the Administration Port's guard layers.
pub(crate) fn administration_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Which Plugins the Org holds, and what each one is bound to
        // (ADR-0017). A Person grants an installed Plugin to their own
        // Agent on the product port; installing one is the Org's.
        .route(
            "/api/v1/plugins",
            get(plugins::list_plugins).post(plugins::install_plugin),
        )
        .route(
            "/api/v1/plugins/{plugin_id}",
            get(plugins::get_plugin).delete(plugins::uninstall_plugin),
        )
        .route(
            "/api/v1/plugins/{plugin_id}/update",
            post(plugins::update_plugin),
        )
        .route(
            "/api/v1/plugins/{plugin_id}/bindings/{field}",
            put(plugins::bind_plugin_field),
        )
        .route(
            "/api/v1/plugins/{plugin_id}/start",
            post(plugins::start_plugin),
        )
}

/// The public Sandbox Proxy holds no data of its own.
pub(crate) fn public_routes() -> Router<Arc<AppState>> {
    Router::new()
        // The sandbox proxy: the proxy document holds no data of
        // its own.
        .route(
            "/api/v1/widgets/{workspace_id}/{package}/{version}/{widget}/sandbox",
            get(widgets::widget_sandbox),
        )
}
