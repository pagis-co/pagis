//! Services and routes of the Native Parts.

use crate::{AppState, RemoteAccessSwitch, RestartSwitch, ScreenRelay, SystemConfigFile};
use crate::{
    calls, coding_sessions, connections, grants, harnesses, hosts, phone_numbers, session_socket,
    software, voice, widgets, ws,
};

pub(crate) mod byte_socket;
pub(crate) mod computers;
pub mod exit_socket;
pub(crate) mod home_exit;
use axum::{
    Router,
    routing::{delete, get, post, put},
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
    /// The connect flow: the one path that writes a Connection
    /// row and takes it through the OAuth exchange.
    pub connector: Arc<pagis_connect::Connector>,
    /// The Coding Sessions that run on those sockets (ADR-0033): the
    /// start, the prompts, the cancel and the close of each session.
    pub coding_sessions: Arc<pagis_coding::CodingSessions>,
    /// The Harness Sign-Ins that the Person starts on their Hosts
    /// (ADR-0033).
    pub sign_ins: Arc<pagis_coding::SignIns>,
    /// The harnesses that need a Harness Sign-In on each Host. The Host
    /// list reads it.
    pub sign_in_reports: Arc<pagis_coding::SignInReports>,
    /// The text half of every carrier (ADR-0020), one per provider.
    /// The number surfaces and the text tools read it to learn whether
    /// the carrier that carries the number carries a text.
    pub text_transports: Arc<pagis_telephony::TextTransports>,
    /// The number desk: the path that buys
    /// and releases a Workspace Number.
    pub numbers: Arc<pagis_telephony::NumberDesk>,
    /// The Version tree behind the records: the manifest of one
    /// Version and the bytes of one file of it. A Widget page reads
    /// through it.
    pub versions: Arc<dyn pagis_software::VersionSource>,
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
        .route(
            "/api/v1/settings/connections/providers",
            get(connections::list_connection_providers),
        )
        .route(
            "/api/v1/settings/connections",
            get(connections::list_connections).post(connections::create_connection),
        )
        .route(
            "/api/v1/settings/connections/{connection_id}",
            delete(connections::delete_connection),
        )
        .route(
            "/api/v1/settings/connections/{connection_id}/authorize",
            post(connections::authorize_connection),
        )
        .route("/api/v1/hosts", get(hosts::list_my_hosts))
        .route(
            "/api/v1/hosts/{host_id}/harnesses/{harness_id}/sign-in",
            post(harnesses::start_sign_in),
        )
        .route(
            "/api/v1/hosts/{host_id}/harnesses/{harness_id}/sign-in-check",
            post(harnesses::check_sign_in),
        )
        .route("/api/v1/harnesses", get(harnesses::list_harnesses))
        .route(
            "/api/v1/agents/{agent_id}/hosts/{host_id}/session-approval-mode",
            put(grants::set_session_approval_mode),
        )
        .route(
            "/api/v1/agents/{agent_id}/hosts/{host_id}/unattended-modes",
            put(grants::set_unattended_modes),
        )
        .route(
            "/api/v1/coding-sessions",
            get(coding_sessions::list_coding_sessions),
        )
        .route(
            "/api/v1/coding-sessions/{coding_session_id}",
            get(coding_sessions::get_coding_session),
        )
        .route(
            "/api/v1/coding-sessions/{coding_session_id}/transcript",
            get(coding_sessions::coding_session_transcript),
        )
        .route(
            "/api/v1/coding-sessions/{coding_session_id}/stop",
            post(coding_sessions::stop_coding_session),
        )
        .route(
            "/api/v1/grants/workspace-number",
            post(grants::create_workspace_number_grant),
        )
        .route(
            "/api/v1/workspace/number",
            get(phone_numbers::get_workspace_number)
                .post(phone_numbers::buy_workspace_number)
                .delete(phone_numbers::release_workspace_number),
        )
        .route(
            "/api/v1/settings/phone-numbers/available",
            get(phone_numbers::search_phone_numbers),
        )
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

/// The native routes inside the Administration Port's guard layers.
pub(crate) fn administration_routes(
    sections: crate::AdministrationSections,
) -> Router<Arc<AppState>> {
    let mut router = Router::new()
        .route(
            "/api/v1/administration/resources",
            get(computers::installation_resources),
        )
        .route(
            "/api/v1/administration/computer-image/pull",
            post(computers::pull_computer_image),
        );
    if sections.contains(crate::AdministrationSection::Providers) {
        router = router
            .route(
                "/api/v1/administration/providers/{provider}/{part}",
                put(connections::configure_provider_part).delete(connections::remove_provider_part),
            )
            .route(
                "/api/v1/administration/providers/{provider}/{part}/test",
                post(connections::test_provider_part),
            );
    }
    router
}

/// The public routes of the Native Parts.
pub(crate) fn public_routes() -> Router<Arc<AppState>> {
    Router::new()
        // The start route of a Google authorization. It reads
        // the Session itself: a browser with no Session goes to sign in
        // and comes back, and only a Session of the Person who started
        // the authorization gets the transaction cookie that binds the
        // `state` to that browser, and the address at Google. A local
        // installation with Remote Access off has one Person, and there
        // any browser of the machine goes on with no Session.
        .route(
            "/api/v1/connections/google/start",
            get(connections::google_start),
        )
        // Where Google redirects a person's browser after they consent.
        // It holds no Session: the cookie is SameSite=Strict, so a
        // browser sends none on a cross-site navigation. It finishes only
        // for the browser that holds the transaction cookie of the
        // `state`, while the Session that started the authorization is
        // live, and only for the Google account of the Connection.
        .route(
            "/api/v1/connections/google/callback",
            get(connections::google_callback),
        )
        // The sandbox proxy: the proxy document holds no data of
        // its own.
        .route(
            "/api/v1/widgets/{workspace_id}/{package}/{version}/{widget}/sandbox",
            get(widgets::widget_sandbox),
        )
}

/// The native live Call operations.
pub struct NativeLiveCallControl {
    pub calls: Arc<pagis_telephony::LiveCalls>,
    pub tiers: Arc<pagis_telephony::LiveTiers>,
}

#[async_trait::async_trait]
impl calls::LiveCallControl for NativeLiveCallControl {
    fn hub(
        &self,
        workspace: &pagis_core::WorkspaceId,
        call: &pagis_core::CallId,
    ) -> Option<Arc<pagis_telephony::hub::MediaHub>> {
        self.calls.hub(workspace, call)
    }

    async fn hang_up(
        &self,
        workspace: &pagis_core::WorkspaceId,
        call: &pagis_core::CallId,
    ) -> bool {
        let Some(hub) = self.calls.hub(workspace, call) else {
            return false;
        };
        hub.hangup().await;
        true
    }

    async fn drop_to_unknown(
        &self,
        workspace: &pagis_core::WorkspaceId,
        call: &pagis_core::CallId,
    ) -> Option<Result<(), pagis_core::StoreError>> {
        self.tiers
            .drop_to_unknown(workspace, call)
            .await
            .map(|result| result.map(|_| ()))
    }
}
