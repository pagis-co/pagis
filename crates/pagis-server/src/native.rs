//! Services and routes of the Native Parts.

use crate::{AppState, RemoteAccessSwitch, RestartSwitch, SystemConfigFile};
use crate::{
    agents, calls, exit_socket, phone_numbers, plugins, session_socket, settings, software, voice,
    widgets, ws,
};
use axum::{
    Router,
    routing::{get, post, put},
};
use std::sync::Arc;

/// The services that need the native feature.
pub struct NativeState {
    /// The calls that run right now. Listen-Live and hang up find the
    /// media hub of a live call here, under the Person's own Workspace.
    pub live_calls: Arc<pagis_telephony::LiveCalls>,
    /// The text half of every carrier (ADR-0020), one per provider.
    /// The number surfaces and the text tools read it to learn whether
    /// the carrier that carries the number carries a text.
    pub text_transports: Arc<pagis_telephony::TextTransports>,
    /// The number desk: the one path that buys, assigns,
    /// unassigns and releases an Agent Phone Number.
    pub numbers: Arc<pagis_telephony::NumberDesk>,
    /// The tier gate of every call that runs now, so the user can drop
    /// a live call of their own Workspace to Unknown.
    pub live_tiers: Arc<pagis_telephony::LiveTiers>,
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
        .route("/api/v1/calls/{call_id}/control", post(calls::control))
        .route("/api/v1/calls", get(calls::list_calls))
        .route("/api/v1/calls/{call_id}", get(calls::get_call))
        .route("/api/v1/calls/{call_id}/dismiss", post(calls::dismiss_call))
        .route("/api/v1/calls/{call_id}/hangup", post(calls::hang_up))
        .route("/api/v1/calls/{call_id}/tier", post(calls::drop_call_tier))
        .route(
            "/api/v1/settings/phone-numbers",
            get(phone_numbers::list_phone_numbers).post(phone_numbers::buy_phone_number),
        )
        .route(
            "/api/v1/settings/phone-numbers/adopt",
            post(phone_numbers::adopt_phone_number),
        )
        .route(
            "/api/v1/settings/phone-numbers/available",
            get(phone_numbers::search_phone_numbers),
        )
        .route(
            "/api/v1/settings/phone-numbers/{phone_number_id}/assign",
            post(phone_numbers::assign_phone_number),
        )
        .route(
            "/api/v1/settings/phone-numbers/{phone_number_id}/unassign",
            post(phone_numbers::unassign_phone_number),
        )
        .route(
            "/api/v1/settings/phone-numbers/{phone_number_id}/release",
            post(phone_numbers::release_phone_number),
        )
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
