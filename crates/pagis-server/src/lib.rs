//! REST + WebSocket surface: session-authed channels and
//! messages REST, the `/api/v1/ws` domain-event firehose, and the
//! OpenAPI contract.

mod administration;
mod agents;
mod artifacts;
mod auth;
mod calls;
mod channels;
mod cors;
mod cross_origin;
pub mod error;
pub mod forget;
pub mod forwarded;
mod grants;
mod hosts;
mod knowledge;
mod live_connections;
mod mail;
mod mailboxes;
mod memory;
pub mod model_lists;
pub mod model_preference;
mod openapi;
mod phone_numbers;
mod plugins;
mod providers;
pub mod provisioning;
mod requests;
mod ring;
pub mod routes;
mod run_steps;
mod runs;
mod schedules;
mod served_file;
mod sessions;
mod settings;
pub mod setup;
mod software;
mod subscriptions;
pub mod system;
mod trust_list;
mod user;
mod voice;
pub mod voice_list;
mod widgets;
mod workspace;
mod ws;

use std::sync::Arc;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Query, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router, middleware};
use object_store::ObjectStore;
use pagis_agent::{AgentSystem, ProgressHub, StreamHub};
use pagis_core::{
    AgentStore, ArtifactStore, ChannelStore, Clock, ConnectionStore, CredentialStore, EventBus,
    EventLog, GrantStore, MemoryStore, MessageStore, ModelAliasStore, OnboardingStore, OrgStore,
    ParticipantStore, ProviderKeys, RequestStore, RetentionPolicyStore, RunStore, SessionStore,
    SignInLinkStore, SoftwareStore, UserStore, WorkspaceStore,
};

pub use auth::{SESSION_COOKIE, Tenant, hash_secret};
pub use forwarded::TrustedProxy;
pub use live_connections::LiveConnections;
pub use openapi::ApiDoc;
pub use ring::{EventRing, RingConfig, RingedBus};
pub use routes::{
    ADMINISTRATION_PUBLIC_ROUTES, ADMINISTRATION_ROUTES, PUBLIC_ROUTES, ROUTES, Route,
    SHARED_ROUTES,
};
pub use sessions::{
    PasswordVerifier, SignInLimits, hash_password, mint_sign_in_link, random_secret,
};
pub use system::{
    MediaRelayKind, MultiUserMode, RESTART_EXIT_CODE, RestartSwitch, ScreenRelay, SystemConfig,
    SystemConfigFile, origin_host_is_loopback, serves_this_machine_only,
};
pub use user::AdministrationAddress;

/// The daemon's version. `/api/v1/health` reports it so the desktop
/// shell knows whether it may attach to a daemon it did not start
/// (ADR-0025).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything the handlers need. Stores and the bus are trait objects
/// behind the storage seams. The bus must already be ring-wrapped
/// (`RingedBus`) so every published event is resumable over `last_seq`.
pub struct AppState {
    pub bus: Arc<dyn EventBus>,
    pub channels: Arc<dyn ChannelStore>,
    pub participants: Arc<dyn ParticipantStore>,
    pub messages: Arc<dyn MessageStore>,
    pub runs: Arc<dyn RunStore>,
    pub pending_evidence: Arc<dyn pagis_core::PendingEvidenceStore>,
    pub trigger: Arc<pagis_trigger::Trigger>,
    pub model_aliases: Arc<dyn ModelAliasStore>,
    /// The Schedules of every Workspace. The Report Schedule of a new
    /// account is written through it.
    pub schedules: Arc<dyn pagis_core::ScheduleStore>,
    pub onboarding: Arc<dyn OnboardingStore>,
    /// The Artifact retention windows the user set.
    pub retention_policies: Arc<dyn RetentionPolicyStore>,
    pub connections: Arc<dyn ConnectionStore>,
    /// The connect flow: the one path that writes a Connection
    /// row and takes it through the OAuth exchange.
    pub connector: Arc<pagis_connect::Connector>,
    pub credentials: Arc<dyn CredentialStore>,
    /// The Workspace vault: the only holder of the data key.
    pub vault: Arc<pagis_vault::Vault>,
    pub requests: Arc<dyn RequestStore>,
    pub grants: Arc<dyn GrantStore>,
    /// The machines the people's clients run on.
    pub hosts: Arc<dyn pagis_core::HostStore>,
    /// Which of those machines are connected now. The WebSocket
    /// registers one while it holds the client's socket, and the reads
    /// that show presence ask it.
    pub host_presence: Arc<pagis_broker::HostPresence>,
    /// The installed Capability Manifests are the authority for
    /// Connection capability names.
    pub broker: Arc<pagis_broker::Broker>,
    /// The agent profiles, for names on the grants page.
    pub agent_store: Arc<dyn AgentStore>,
    /// The calls that run right now. Listen-Live and hang up find the
    /// media hub of a live call here, under the Person's own Workspace.
    pub live_calls: Arc<pagis_telephony::LiveCalls>,
    /// The Call records. The strip, the call inspector and the
    /// settled block read them: the record is the one source of truth.
    pub calls: Arc<dyn pagis_core::CallStore>,
    /// The Text Records (ADR-0020). The text strip, the
    /// Conversation Inspector and the text tools read them: the
    /// record is the one source of truth for a text, and a Text
    /// Conversation is grouped from these rows.
    pub texts: Arc<dyn pagis_core::TextRecordStore>,
    /// The text half of every carrier (ADR-0020), one per provider.
    /// The number surfaces and the text tools read it to learn whether
    /// the carrier that carries the number carries a text.
    pub text_transports: Arc<pagis_telephony::TextTransports>,
    /// The mailbox desk (ADR-0019): the one path that makes,
    /// proves, recovers, sleeps and deletes an Agent Mailbox. Removing
    /// a Mailbox Provider Connection also reads the count of the
    /// mailboxes that still point at it here.
    pub mailbox_desk: Arc<pagis_mail::MailboxDesk>,
    /// The number desk: the one path that buys, assigns,
    /// unassigns and releases an Agent Phone Number.
    pub numbers: Arc<pagis_telephony::NumberDesk>,
    /// The Trust List (ADR-0021, ADR-0019). Only this
    /// surface writes it.
    pub trust_list: Arc<dyn pagis_core::TrustListStore>,
    /// The tier gate of every call that runs now, so the user can drop
    /// a live call of their own Workspace to Unknown.
    pub live_tiers: Arc<pagis_telephony::LiveTiers>,
    /// The daemon's secret store. It holds the hash of the Keypad Code.
    pub secrets: Arc<dyn pagis_core::SecretStore>,
    /// The failed-attempt count of each Workspace's Keypad Code
    /// (ADR-0021). The person reads it and clears it here.
    pub keypad_failures: Arc<dyn pagis_core::KeypadFailureStore>,
    /// The voice seam (ADR-0020): dictation in, spoken blocks
    /// out. Nothing it handles is stored.
    pub voice: Arc<dyn pagis_voice::VoiceProvider>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub blobs: Arc<dyn ObjectStore>,
    /// The upload size cap; larger uploads get 413.
    pub artifact_max_bytes: u64,
    /// The events table, read directly for the learning feed.
    pub events: Arc<dyn EventLog>,
    pub memory: Arc<dyn MemoryStore>,
    pub knowledge: Arc<dyn pagis_core::knowledge::KnowledgeStore>,
    /// The Signal Catalogue of one resource, for the filter editor (ADR-0011).
    pub catalogues: Arc<dyn pagis_core::knowledge::CatalogueProvider>,
    pub forget: Arc<dyn pagis_core::ForgetStore>,
    /// The source of the suppression key of each Workspace: the one
    /// shared holder of the Tenant Data Keys. The database holds no
    /// Forget key (ADR-0008).
    pub forget_keys: Arc<dyn pagis_core::ForgetKeys>,
    /// The agent computer lifecycle, one manager per tenant.
    /// A handler resolves the manager of the signed-in person's
    /// Workspace, so no handler reaches another tenant's containers.
    pub computers: Arc<pagis_computer::ComputerManagers>,
    /// The Software List (ADR-0016). The desk reads it; only the
    /// Agent tools write it.
    pub software: Arc<dyn SoftwareStore>,
    /// The Version tree behind the records: the manifest of one
    /// Version and the bytes of one file of it. A Widget page reads
    /// through it.
    pub versions: Arc<dyn pagis_software::VersionSource>,
    /// The installed Plugins (ADR-0017). Only the desk writes
    /// them: installing one decides which code runs on this computer.
    pub plugins: Arc<pagis_plugin::Plugins>,
    /// The MCP hosts of those Plugins (ADR-0017), one per
    /// tenant. A handler resolves the host of the signed-in person's
    /// Workspace: the desk starts a failed Plugin again in that person's
    /// own Plugin Computer and reads that Computer's server log.
    pub plugin_hosts: Arc<pagis_plugins::PluginHosts>,
    /// The Org's Workspace ([`pagis_core::Org::workspace_id`]), which
    /// no person owns. It holds the installed Plugins with their
    /// Bindings and checkouts (ADR-0017) and the Installation
    /// Connections. They are read here whoever asks, and every tenant
    /// runs a Plugin in its own Plugin Computer.
    pub org_workspace_id: pagis_core::WorkspaceId,
    pub contributions: Arc<dyn pagis_core::ContributionStore>,
    pub workspaces: Arc<dyn WorkspaceStore>,
    /// The installation. Exactly one Org row exists.
    pub orgs: Arc<dyn OrgStore>,
    /// The people of the Org.
    pub users: Arc<dyn UserStore>,
    /// What each model call spent. The Administrator reads spend
    /// per person, and each person reads their own.
    pub usage: Arc<dyn pagis_core::UsageStore>,
    /// What every signed-in client holds.
    pub sessions: Arc<dyn SessionStore>,
    /// The signal of each Session that holds a live connection. A
    /// sign-out and an Administrator who ends every Session of a Person
    /// fire it, and so does the expiry of the Session, and every socket
    /// and Media Relay path of that Session closes.
    pub live_connections: Arc<LiveConnections>,
    /// The one-time sign-in links the `pagis` binary prints.
    pub sign_in_links: Arc<dyn SignInLinkStore>,
    /// The sign-in attempts and the sign-ins, per account and per source
    /// address.
    pub sign_in_limits: Arc<SignInLimits>,
    /// The password checks of a password sign-in: argon2id on the
    /// blocking pool, with one verification permit for each core.
    pub password_verifier: Arc<PasswordVerifier>,
    /// The Client Credential of a local installation. `Some` on a
    /// local installation, where the Client App trades the file
    /// for a Session from this machine; `None` on a server, which
    /// refuses the exchange and the sign-in link, so a credential file
    /// is never a way into one.
    pub client_credential: Option<String>,
    /// The actual listener port. Runtime identity binds its proof to it so a
    /// process on another port cannot relay the challenge.
    pub runtime_port: u16,
    /// What sets the port of this run over `config.toml`: `--port` or
    /// `PAGIS_PORT`. `None` when the file sets it.
    pub port_override: Option<&'static str>,
    /// Whether a supervisor starts the daemon again after it exits for
    /// a restart.
    pub supervised: bool,
    /// When this daemon process started, in Unix milliseconds. A
    /// restart changes it, so a page that asked for one knows when the
    /// new process answers.
    pub started_at: i64,
    /// Why this daemon sends no analytics whatever the System Setting
    /// says (ADR-0026), or `None` for a release build that can send.
    pub analytics_blocked: Option<pagis_analytics::Blocked>,
    /// The origin a browser reaches this installation at: the
    /// reverse proxy's on a server, loopback on a local installation. The
    /// CORS answer names it and no other, and the product port refuses a
    /// browser request from any other origin but [`AppState::local_origin`].
    pub public_origin: String,
    /// The loopback origin at which a program on this machine reaches the
    /// product port of a local installation: the origin of the owner's
    /// Client App window and of the Sign-In Link (ADR-0024). It is the
    /// Public Origin while the multi-user mode is off. `None` on a server,
    /// which no Client App opens at loopback.
    pub local_origin: Option<String>,
    /// Where a browser reaches the Administration Interface. The
    /// product answers it to an Administrator, so the page can link to
    /// the port it draws no installation setting of its own for. The
    /// Administration Port refuses a browser request from any other
    /// origin.
    pub administration: user::AdministrationAddress,
    /// The one address whose `X-Forwarded-*` headers the daemon believes.
    /// A local installation trusts none.
    pub proxy: TrustedProxy,
    /// The Media Relay the live screen goes through. The System Settings
    /// name it, because it does not go through the proxy.
    pub screen: ScreenRelay,
    pub ring: Arc<EventRing>,
    pub hub: Arc<StreamHub>,
    /// The derived run progress: the live line per run.
    pub progress: Arc<ProgressHub>,
    pub agents: Arc<AgentSystem>,
    pub keys: Arc<ProviderKeys>,
    /// The Provider Model Lists of the installation's keys.
    pub models: Arc<pagis_agent::ModelCatalog>,
    /// Docker discovery (ADR-0024). One instance serves the runtime
    /// and the System tab, so they name the same endpoint.
    pub docker_discovery: Arc<pagis_computer::DockerDiscovery>,
    /// Which backend holds the records: `sqlite` or `postgres`.
    /// The Administration Interface reports it; nothing branches
    /// on it.
    pub database_backend: String,
    /// The config file the System tab reads and writes.
    pub system: Arc<dyn SystemConfigFile>,
    /// The restart the System tab asks for.
    pub restart: Arc<RestartSwitch>,
    /// The daemon's logical time for owner messages and scheduled work.
    pub clock: Arc<dyn Clock>,
}

/// The two interfaces one daemon serves: the product on the port
/// the team reaches, the Administration Interface on a port of its own.
/// Both read one [`AppState`], because they are one process.
pub struct Routers {
    pub product: Router,
    pub administration: Router,
}

/// Build both routers over one state.
///
/// Each listener refuses a browser request from an origin it does not
/// serve, before the Session check and before any upgrade (ADR-0024).
/// The layer wraps the whole router, so a new route cannot skip it. The
/// product port serves the Public Origin and, on a local installation,
/// its loopback origin; the Administration Port serves its own origin.
pub fn routers(state: AppState) -> Routers {
    let state = Arc::new(state);
    let product_origins = std::iter::once(state.public_origin.clone())
        .chain(state.local_origin.clone())
        .collect();
    let product =
        cross_origin::refuse_other_origins(product_router(Arc::clone(&state)), product_origins);
    let administration = cross_origin::refuse_other_origins(
        administration::router(Arc::clone(&state)),
        vec![state.administration.origin.clone()],
    );
    Routers {
        product: product.layer(no_store()),
        administration: administration.layer(no_store()),
    }
}

/// No answer of the API goes into a cache. An answer is one Person's
/// records, or the state of one installation at one moment: a browser
/// that kept one could show another installation at the same address
/// a `410` from its closed setup, or show a signed-out Person's records
/// after a sign-in. A route that sets its own `Cache-Control`, such as
/// an immutable widget file, keeps it.
fn no_store() -> tower_http::set_header::SetResponseHeaderLayer<axum::http::HeaderValue> {
    tower_http::set_header::SetResponseHeaderLayer::if_not_present(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    )
}

/// Build the product router.
fn product_router(state: Arc<AppState>) -> Router {
    let public_origin = state.public_origin.clone();

    let authed = Router::new()
        .route(
            "/api/v1/knowledge/forget",
            get(forget::list).post(forget::confirm),
        )
        .route("/api/v1/knowledge/forget/preview", post(forget::preview))
        .route("/api/v1/knowledge/forget/{id}", get(forget::get))
        .route("/api/v1/knowledge/forget/{id}/retry", post(forget::retry))
        .route("/api/v1/knowledge/forget/{id}/reopt", post(forget::reopt))
        .route(
            "/api/v1/connections/{connection_id}/sync",
            get(knowledge::get_sync).put(knowledge::configure_sync),
        )
        .route(
            "/api/v1/connections/{connection_id}/sync/catalogue",
            get(knowledge::get_catalogue),
        )
        .route(
            "/api/v1/connections/{connection_id}/sync/filter/preview",
            post(knowledge::preview_filter),
        )
        .route(
            "/api/v1/agents/{agent_id}/sync-connections",
            get(knowledge::agent_sync_connections),
        )
        .route("/api/v1/hosts", get(hosts::list_hosts))
        .route(
            "/api/v1/agents",
            get(agents::list_agents).post(agents::create_agent),
        )
        .route("/api/v1/agents/{agent_id}", put(agents::update_agent))
        .route(
            "/api/v1/agents/{agent_id}/appearance",
            put(agents::update_appearance),
        )
        .route(
            "/api/v1/agents/{agent_id}/archive",
            post(agents::archive_agent),
        )
        .route(
            "/api/v1/agents/{agent_id}/mailbox",
            get(mailboxes::get_mailbox)
                .post(mailboxes::provision_mailbox)
                .delete(mailboxes::delete_mailbox),
        )
        .route("/api/v1/settings/mailboxes", get(mailboxes::list_mailboxes))
        .route(
            "/api/v1/settings/mailbox-offers",
            get(mailboxes::list_mailbox_offers),
        )
        .route(
            "/api/v1/settings/mailbox-offers/name",
            get(mailboxes::check_mailbox_name),
        )
        .route(
            "/api/v1/agents/{agent_id}/mailbox/reset-password",
            post(mailboxes::reset_mailbox_password),
        )
        .route(
            "/api/v1/mail/{mailbox}/{message_id}",
            get(mail::get_mail_message),
        )
        .route(
            "/api/v1/agents/{agent_id}/computer",
            get(agents::computer_state),
        )
        .route(
            "/api/v1/agents/{agent_id}/computer/wake",
            post(agents::wake_computer),
        )
        .route(
            "/api/v1/agents/{agent_id}/computer/sleep",
            post(agents::sleep_computer),
        )
        .route("/api/v1/computers/disk", get(agents::computer_disk))
        .route("/api/v1/screen/ice", get(agents::screen_ice))
        .route(
            "/api/v1/agents/{agent_id}/screen/preview.png",
            get(agents::screen_preview),
        )
        .route(
            "/api/v1/agents/{agent_id}/screen/offer",
            post(agents::screen_offer),
        )
        .route(
            "/api/v1/agents/{agent_id}/screen/takeover",
            post(agents::screen_takeover),
        )
        .route(
            "/api/v1/agents/{agent_id}/screen/handback",
            post(agents::screen_handback),
        )
        .route(
            "/api/v1/channels",
            get(channels::list_channels).post(channels::create_channel),
        )
        .route(
            "/api/v1/channels/{channel_id}/messages",
            get(channels::list_messages).post(channels::send_message),
        )
        .route(
            "/api/v1/channels/{channel_id}/messages/{message_id}",
            get(channels::get_message),
        )
        .route(
            "/api/v1/channels/{channel_id}/messages/{message_id}/speech",
            get(voice::speak_block),
        )
        .route(
            "/api/v1/channels/{channel_id}/threads/{root_message_id}",
            get(channels::get_thread),
        )
        .route("/api/v1/settings/voices", get(voice::list_voices))
        .route(
            "/api/v1/artifacts",
            post(artifacts::upload_artifact).layer(DefaultBodyLimit::max(
                state.artifact_max_bytes as usize + artifacts::MULTIPART_OVERHEAD,
            )),
        )
        .route(
            "/api/v1/artifacts/{artifact_id}",
            get(artifacts::download_artifact),
        )
        .route("/api/v1/calls/{call_id}/control", post(calls::control))
        .route("/api/v1/runs/{run_id}/cancel", post(runs::cancel_run))
        .route("/api/v1/runs/{run_id}/dismiss", post(runs::dismiss_run))
        .route("/api/v1/runs/{run_id}/retry", post(runs::retry_review))
        .route("/api/v1/runs/{run_id}/events", get(runs::run_events))
        .route("/api/v1/runs/{run_id}/steps", get(runs::run_steps))
        .route("/api/v1/runs", get(runs::list_runs))
        .route(
            "/api/v1/schedules",
            get(schedules::list_schedules).post(schedules::create_schedule),
        )
        .route(
            "/api/v1/schedules/{schedule_id}",
            get(schedules::get_schedule).post(schedules::update_schedule),
        )
        .route(
            "/api/v1/schedules/{schedule_id}/archive",
            post(schedules::archive_schedule),
        )
        .route(
            "/api/v1/schedules/{schedule_id}/run",
            post(schedules::run_schedule_now),
        )
        .route(
            "/api/v1/schedules/{schedule_id}/revisions",
            get(schedules::list_revisions),
        )
        .route(
            "/api/v1/schedules/{schedule_id}/occurrences",
            get(schedules::list_occurrences),
        )
        .route(
            "/api/v1/schedules/{schedule_id}/wakeups",
            get(schedules::list_wakeups),
        )
        .route(
            "/api/v1/event-subscriptions",
            get(subscriptions::list_subscriptions).post(subscriptions::create_subscription),
        )
        .route(
            "/api/v1/event-subscriptions/{subscription_id}",
            get(subscriptions::get_subscription).post(subscriptions::update_subscription),
        )
        .route(
            "/api/v1/event-subscriptions/{subscription_id}/events",
            get(subscriptions::list_events),
        )
        .route(
            "/api/v1/event-subscriptions/{subscription_id}/wakeups",
            get(subscriptions::list_subscription_wakeups),
        )
        .route("/api/v1/requests", get(requests::list_requests))
        .route("/api/v1/requests/{request_id}", get(requests::get_request))
        .route(
            "/api/v1/requests/{request_id}/decision",
            post(requests::decide_request),
        )
        .route("/api/v1/memory/feed", get(memory::feed))
        .route(
            "/api/v1/memory/commits/{sha}/revert",
            post(memory::revert_commit),
        )
        .route("/api/v1/memory/file", get(memory::get_file))
        .route("/api/v1/memory/pages", get(memory::list_pages))
        .route("/api/v1/memory/pages/counts", get(memory::count_pages))
        .route(
            "/api/v1/memory/commits/{sha}/diff",
            get(memory::commit_diff),
        )
        .route(
            "/api/v1/grants",
            get(grants::list_grants).post(grants::create_connection_grant),
        )
        .route(
            "/api/v1/grants/{grant_id}/rules",
            put(grants::set_grant_rules),
        )
        .route(
            "/api/v1/grants/{grant_id}/capabilities",
            put(grants::set_grant_capabilities),
        )
        .route("/api/v1/grants/{grant_id}", delete(grants::revoke_grant))
        .route("/api/v1/user", get(user::get_user))
        .route("/api/v1/workspace", get(workspace::get_workspace))
        .route(
            "/api/v1/workspace/chief-of-staff",
            put(workspace::set_chief_of_staff),
        )
        .route("/api/v1/workspace/timezone", put(workspace::put_timezone))
        .route("/api/v1/workspace/report", get(workspace::get_report))
        .route(
            "/api/v1/settings/onboarding",
            get(settings::onboarding_status),
        )
        .route(
            "/api/v1/settings/onboarding/complete",
            post(settings::complete_onboarding),
        )
        // The first run writes the installation's model key and Docker
        // endpoint from the product port, and nothing after it does.
        .route(
            "/api/v1/settings/onboarding/providers/{provider}/key",
            put(settings::set_onboarding_provider_key),
        )
        .route(
            "/api/v1/settings/onboarding/providers/{provider}/key/check",
            post(settings::check_onboarding_provider_key),
        )
        .route(
            "/api/v1/settings/onboarding/docker-endpoint",
            put(settings::set_onboarding_docker_endpoint),
        )
        .route(
            "/api/v1/settings/onboarding/default-model",
            put(model_lists::set_onboarding_default_model),
        )
        .route(
            "/api/v1/settings/providers/{provider}/check",
            post(settings::check_provider_model),
        )
        .route("/api/v1/settings/models", get(model_lists::list_models))
        .route(
            "/api/v1/settings/model-aliases/{alias}",
            put(settings::update_model_alias).delete(settings::delete_model_alias),
        )
        .route(
            "/api/v1/settings/model-aliases",
            get(settings::list_model_aliases).post(settings::create_model_alias),
        )
        .route(
            "/api/v1/settings/retention",
            get(settings::list_retention_policies),
        )
        .route(
            "/api/v1/settings/retention/{kind}",
            put(settings::set_retention_policy),
        )
        .route(
            "/api/v1/settings/connections/providers",
            get(settings::list_connection_providers),
        )
        .route(
            "/api/v1/settings/connections",
            get(settings::list_connections).post(settings::create_connection),
        )
        .route(
            "/api/v1/settings/connections/{connection_id}",
            delete(settings::delete_connection),
        )
        .route(
            "/api/v1/settings/connections/{connection_id}/authorize",
            post(settings::authorize_connection),
        )
        .route(
            "/api/v1/settings/trust-list",
            get(trust_list::list_trust_entries).post(trust_list::add_trust_entry),
        )
        .route(
            "/api/v1/settings/trust-list/{trust_entry_id}",
            delete(trust_list::delete_trust_entry),
        )
        .route(
            "/api/v1/settings/keypad-code",
            put(trust_list::set_keypad_code).delete(trust_list::delete_keypad_code),
        )
        .route(
            "/api/v1/settings/keypad-code/failures",
            delete(trust_list::clear_keypad_failures),
        )
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
        .route(
            "/api/v1/settings/credentials",
            get(settings::list_credentials).post(settings::add_credential),
        )
        .route(
            "/api/v1/settings/credentials/{credential_id}",
            delete(settings::delete_credential),
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
        // What the signed-in person spent. A Member reads their own and
        // nothing else of the installation's accounting.
        .route("/api/v1/usage", get(administration::my_usage))
        .route("/api/v1/sessions/current", delete(sessions::sign_out))
        // The sockets read the same session cookie as every other
        // route, so they sit behind the same middleware. Their
        // first frame carries no credential. The origin check that
        // `routers` puts around the product router refuses an upgrade
        // that a page at another origin starts.
        .route("/api/v1/ws", get(ws::upgrade))
        .route("/api/v1/channels/{channel_id}/dictate", get(voice::dictate))
        // Listen-Live (ADR-0020).
        .route("/api/v1/calls/{call_id}/listen", get(calls::listen))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::authenticate,
        ));

    Router::new()
        // The sandbox proxy: the proxy document holds no data of
        // its own.
        .route(
            "/api/v1/widgets/{workspace_id}/{package}/{version}/{widget}/sandbox",
            get(widgets::widget_sandbox),
        )
        .route("/api/v1/health", get(health))
        .route("/api/v1/runtime/identity", get(runtime_identity))
        // The start route of a brokered Google authorization. It reads
        // the Session itself: a browser with no Session goes to sign in
        // and comes back, and only a Session of the Person who started
        // the authorization gets the transaction cookie that binds the
        // `state` to that browser, and the address at Google. A local
        // installation with the multi-user mode off has one Person, and
        // there any browser of the machine goes on with no Session.
        .route(
            "/api/v1/connections/google/start",
            get(settings::google_start),
        )
        // Where Google redirects a person's browser after they consent.
        // It holds no Session: the cookie is SameSite=Strict, so a
        // browser sends none on a cross-site navigation. It finishes only
        // for the browser that holds the transaction cookie of the
        // `state`, while the Session that started the authorization is
        // live, and only for the Google account of the Connection.
        .route(
            "/api/v1/connections/google/callback",
            get(settings::google_callback),
        )
        // The read of the server's own first run, from which the
        // sign-in page names the Administration Port. It answers while
        // the installation holds no Client Credential and no
        // Administrator holds a password, and `410 Gone` afterwards.
        // Server Setup posts on the Administration Port alone: this
        // port faces the internet, and a write here would give the
        // first Administrator to the first person who posts.
        .route("/api/v1/setup", get(setup::get_setup))
        .route("/api/v1/openapi.json", get(openapi_json))
        // The ways in. They are the only routes that answer
        // without a session cookie, because they are what hands one out.
        .route("/api/v1/sessions", post(sessions::sign_in_with_password))
        .route(
            "/api/v1/sessions/client",
            post(sessions::sign_in_with_client_credential),
        )
        .route(
            "/api/v1/sessions/link/{code}",
            get(sessions::sign_in_with_link),
        )
        .merge(authed)
        // One origin may make a credentialed cross-origin call: the one
        // this installation is served from. The layer answers the
        // preflight, so no route handles `OPTIONS`.
        .layer(cors::cors_layer(&public_origin))
        .with_state(state)
}

/// What the desktop shell reads before it attaches to a daemon it did
/// not start (ADR-0025): the daemon answers, and it says its version.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct HealthDto {
    pub status: String,
    pub version: String,
}

#[utoipa::path(get, path = "/api/v1/health", responses((status = 200, body = HealthDto)))]
async fn health() -> Json<HealthDto> {
    Json(HealthDto {
        status: "ok".to_string(),
        version: VERSION.to_string(),
    })
}

/// A credential-possession proof for the Client App. The client sends
/// only a fresh nonce, so an unrelated process never receives the Client
/// Credential.
#[derive(Debug, serde::Deserialize)]
struct RuntimeIdentityQuery {
    challenge: String,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RuntimeIdentityDto {
    pub status: String,
    pub release: String,
    pub workspace_id: String,
    pub port: u16,
    pub computer_image: String,
    pub proof: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/runtime/identity",
    params(("challenge" = String, Query, description = "Fresh 32-byte lowercase hexadecimal challenge")),
    responses(
        (status = 200, body = RuntimeIdentityDto),
        (status = 403),
        (status = 404),
        (status = 422),
    )
)]
async fn runtime_identity(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    Query(query): Query<RuntimeIdentityQuery>,
) -> Result<Json<RuntimeIdentityDto>, axum::http::StatusCode> {
    // The proof is keyed by the Client Credential, so only a client that
    // holds the file can check it. A server holds none and answers
    // nothing. The Client App asks from this machine, so the handshake
    // answers nobody else, as the credential trade does.
    let credential = state
        .client_credential
        .as_deref()
        .ok_or(axum::http::StatusCode::NOT_FOUND)?;
    if !forwarded::is_from_this_machine(peer, &headers) {
        return Err(axum::http::StatusCode::FORBIDDEN);
    }
    if query.challenge.len() != 64
        || !query
            .challenge
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }
    // The identity names the Workspace of the person the credential
    // belongs to, which is the Workspace the client started, never
    // whichever row the store answers first.
    let person = sessions::seeded_administrator(&state)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let workspace_id = state
        .workspaces
        .for_user(&person.id)
        .await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(axum::http::StatusCode::NOT_FOUND)?
        .id
        .to_string();
    let computer_image = pagis_computer::IMAGE;
    Ok(Json(RuntimeIdentityDto {
        status: "ok".to_string(),
        release: VERSION.to_string(),
        workspace_id: workspace_id.clone(),
        port: state.runtime_port,
        computer_image: computer_image.to_string(),
        proof: runtime_identity_proof(
            credential,
            state.runtime_port,
            VERSION,
            &workspace_id,
            computer_image,
            &query.challenge,
        ),
    }))
}

fn runtime_identity_proof(
    credential: &str,
    port: u16,
    release: &str,
    workspace_id: &str,
    computer_image: &str,
    challenge: &str,
) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let message = format!(
        "pagis-runtime-identity-v1\0{port}\0{release}\0{workspace_id}\0{computer_image}\0{challenge}"
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(credential.as_bytes())
        .expect("HMAC accepts a key of any length");
    mac.update(message.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

async fn openapi_json() -> Json<serde_json::Value> {
    use utoipa::OpenApi;
    Json(serde_json::to_value(ApiDoc::openapi()).expect("spec serializes"))
}

#[cfg(test)]
mod runtime_identity_tests {
    use super::runtime_identity_proof;

    #[test]
    fn identity_proof_matches_the_node_client_vector() {
        assert_eq!(
            runtime_identity_proof(
                "token-123",
                4400,
                "0.1.0",
                "01HQTESTWORKSPACE",
                "ghcr.io/pagis-co/pagis-computer@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                &"a".repeat(64),
            ),
            "b7ddc2ba3d3245faf25d7c71cb11317fa35aaa165b040f9288afabc717012853"
        );
    }
}
