//! Services and product routes of the Native Parts.

use crate::{AppState, RemoteAccessSwitch, RestartSwitch, SystemConfigFile};
use crate::{
    agents, calls, coding_sessions, exit_socket, grants, harnesses, hosts, session_socket,
    settings, voice, ws,
};
use axum::{
    Router,
    routing::{get, post, put},
};
use std::sync::Arc;

/// The services that need the native feature.
pub struct NativeState {
    /// The Coding Sessions that run on those sockets (ADR-0033): the
    /// start, the prompts, the cancel and the close of each session.
    pub coding_sessions: Arc<pagis_coding::CodingSessions>,
    /// The Harness Sign-Ins that the Person starts on their Hosts
    /// (ADR-0033).
    pub sign_ins: Arc<pagis_coding::SignIns>,
    /// The harnesses that need a Harness Sign-In on each Host. The Host
    /// list reads it.
    pub sign_in_reports: Arc<pagis_coding::SignInReports>,

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
