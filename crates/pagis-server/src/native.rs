//! Services and product routes of the Native Parts.

use crate::{AppState, RemoteAccessSwitch, RestartSwitch, SystemConfigFile};
use crate::{agents, exit_socket, session_socket, settings, voice, ws};

#[path = "calls.rs"]
pub mod calls;
#[path = "phone_numbers.rs"]
pub(crate) mod phone_numbers;
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
