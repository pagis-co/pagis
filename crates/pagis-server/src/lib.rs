//! REST + WebSocket surface: session-authed channels and
//! messages REST, the `/api/v1/ws` domain-event firehose, and the
//! OpenAPI contract.

mod administration;
mod agents;
mod auth;
#[cfg(feature = "native")]
mod axum_socket;
mod calendar;
mod calendar_mail;
pub mod calls;
mod channels;
mod client_name;
mod coding_session_feed;
mod coding_sessions;
#[cfg(feature = "native")]
mod connections;
#[cfg(all(test, not(feature = "native")))]
mod connections_portable_tests;
mod conversations;
mod cors;
mod cross_origin;
mod disclosure;
pub mod error;
pub mod fake;
mod files;
pub mod forget;
pub mod forwarded;
mod grants;
pub mod harness_model;
mod harnesses;
mod hosts;
mod knowledge;
mod live_connections;
mod mail;
mod mailboxes;
mod memory;
mod message_socket;
pub mod model_lists;
pub mod model_preference;
#[cfg(feature = "native")]
pub mod native;
mod needs_you;
#[cfg(feature = "native")]
mod openapi;
mod peer;
#[cfg(feature = "native")]
mod phone_numbers;
mod plugins;
mod providers;
pub mod provisioning;
mod push_subscriptions;
pub mod remote_access;
mod remote_access_native;
mod search;
#[cfg(not(feature = "native"))]
mod unavailable;
mod workspace_mail;
#[cfg(feature = "native")]
pub use native::NativeState;
#[cfg(feature = "native")]
use native::{
    administration_routes as native_administration_routes, product_routes as native_product_routes,
    public_routes as native_public_routes,
};
#[cfg(not(feature = "native"))]
use unavailable::{
    administration_routes as native_administration_routes, product_routes as native_product_routes,
    public_routes as native_public_routes,
};
mod agent_versions;
mod requests;
mod ring;
pub mod routes;
mod rt;
mod run_steps;
mod runs;
mod schedules;
mod served_file;
#[cfg(feature = "native")]
pub mod session_socket;
mod sessions;
mod settings;
pub mod setup;
mod shared_workspace;
mod sign_in_links;
mod software;
mod subscriptions;
pub mod system;
mod trust_list;
mod user;
pub mod voice;
pub mod voice_list;
mod widgets;
mod workspace;
mod workspace_limits;
mod workspace_members;
pub mod ws;

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Query, State};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router, middleware};
use object_store::ObjectStore;
use pagis_agent::{AgentSystem, ProgressHub, StreamHub};
use pagis_core::{
    AgentStore, ChannelStore, Clock, ConnectionStore, CredentialStore, EventBus, EventLog,
    FileStore, GrantStore, MemoryStore, MessageStore, ModelAliasStore, OnboardingStore, OrgStore,
    ParticipantStore, ProviderKeys, RequestStore, RetentionPolicyStore, RunStore, SessionStore,
    SignInLinkStore, SoftwareStore, UserStore, WorkspaceStore,
};
use pagis_needs_you::{NeedsYou, Notifications, PersonActivity, PersonActivityReader};

pub use administration::{AdministrationSection, AdministrationSections};
pub use auth::resolve as resolve_session;
pub use auth::{SESSION_COOKIE, Tenant, hash_secret};
pub use coding_session_feed::CodingSessionFeed;
pub use forwarded::TrustedProxy;
pub use harness_model::{HarnessModelDeps, HarnessModelTokens};
pub use live_connections::{LiveConnections, LiveTenant};
pub use message_socket::{CloseFrame, Message, MessageSocket};
#[cfg(feature = "native")]
pub use openapi::ApiDoc;
pub use peer::Peer;
pub use remote_access::serves_this_machine_only;
#[cfg(feature = "native")]
pub use remote_access_native::{
    FUNNEL_PROXY, FUNNEL_TURN_PORT, FunnelTargets, RemoteAccessSwitch, Tailscale,
};
pub use remote_access_native::{FunnelPort, TailscaleState};
pub use ring::{EventRing, RingConfig, RingedBus};
pub use routes::{
    ADMINISTRATION_PUBLIC_ROUTES, ADMINISTRATION_ROUTES, PUBLIC_ROUTES, ROUTES, Route,
    SHARED_ROUTES,
};
pub use sessions::{PasswordVerifier, SignInLimits, hash_password, random_secret};
pub use sign_in_links::{
    MintedLink, SIGN_IN_PAGE, mint_public_origin_link, mint_start_link, qr_svg, qr_text,
};
pub use system::{
    MediaRelayKind, RESTART_EXIT_CODE, RestartSwitch, ScreenRelay, SystemConfig, SystemConfigFile,
    origin_host_is_loopback,
};
pub use user::{
    AdministrationAddress, ConfiguredLink, NoOrgMenu, NoPersonNotice, NoticeText, NoticeTone,
    OrgDto, OrgMenu, OrgMenuEntry, PersonNotice, PersonNoticeDto,
};

/// The daemon's version. `/api/v1/health` reports it so the desktop
/// shell knows whether it may attach to a daemon it did not start
/// (ADR-0025).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything the handlers need. Stores and the bus are trait objects
/// behind the storage seams. The bus must already be ring-wrapped
/// (`RingedBus`) so every published event is resumable over `last_seq`.
pub struct AppState {
    pub disclosure: Arc<dyn pagis_core::DisclosureStore>,
    pub governance: Arc<dyn pagis_core::WorkspaceGovernanceStore>,
    pub audit: Arc<dyn pagis_core::AuditLog>,
    #[cfg(feature = "native")]
    pub native: NativeState,
    /// The installed Plugins (ADR-0017), with the PackageSource and
    /// PluginStates supplied by the Embedder. The Administration Interface
    /// writes them: an install decides which code can run.
    pub plugins: Arc<pagis_plugin::Plugins>,
    /// The MCP hosts of those Plugins (ADR-0017), one per
    /// tenant. A handler resolves the host of the signed-in person's
    /// Workspace: the desk starts a failed Plugin again in that person's
    /// own Plugin Computer and reads that Computer's server log. The
    /// Embedder supplies PluginStates, McpHttp and StdioServers.
    pub plugin_hosts: Arc<pagis_plugins::PluginHosts>,

    pub bus: Arc<dyn EventBus>,
    pub channels: Arc<dyn ChannelStore>,
    pub participants: Arc<dyn ParticipantStore>,
    pub messages: Arc<dyn MessageStore>,
    pub runs: Arc<dyn RunStore>,
    pub conversation_evidence: Arc<dyn pagis_core::ConversationEvidenceStore>,
    pub pending_evidence: Arc<dyn pagis_core::PendingEvidenceStore>,
    pub trigger: Arc<pagis_trigger::Trigger>,
    pub model_aliases: Arc<dyn ModelAliasStore>,
    /// The Schedules of every Workspace. The Report Schedule of a new
    /// account is written through it.
    pub schedules: Arc<dyn pagis_core::ScheduleStore>,
    pub onboarding: Arc<dyn OnboardingStore>,
    /// The File retention windows the user set.
    pub retention_policies: Arc<dyn RetentionPolicyStore>,
    pub connections: Arc<dyn ConnectionStore>,
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
    /// The session sockets of the Hosts, which carry one stream for each
    /// Coding Session (ADR-0033).
    pub host_sessions: Arc<pagis_broker::HostSessions>,
    /// The Coding Session records and their transcripts. Each write
    /// reports itself as an event.
    pub coding_session_store: Arc<dyn pagis_core::CodingSessionStore>,
    /// The installed Capability Manifests are the authority for
    /// Connection capability names.
    pub broker: Arc<pagis_broker::Broker>,
    /// The agent profiles, for names on the grants page.
    pub agent_store: Arc<dyn AgentStore>,
    /// The Call records. The strip, the call inspector and the
    /// settled block read them: the record is the one source of truth.
    pub calls: Arc<dyn pagis_core::CallStore>,
    /// The live Call operations supplied by the Embedder.
    pub live_call_control: Arc<dyn calls::LiveCallControl>,
    /// The Text Records (ADR-0020). The text strip, the
    /// Conversation Inspector and the text tools read them: the
    /// record is the one source of truth for a text, and a Text
    /// Conversation is grouped from these rows.
    pub texts: Arc<dyn pagis_core::TextRecordStore>,
    /// The mailbox desk (ADR-0019): the one path that makes,
    /// proves, recovers, sleeps and deletes an Agent Mailbox. Removing
    /// a Mailbox Provider Connection also reads the count of the
    /// mailboxes that still point at it here.
    pub mailbox_desk: Arc<pagis_mail::MailboxDesk>,
    /// Mail reads through the service supplied by the Embedder.
    pub mailboxes: Arc<dyn pagis_mail::Mailboxes>,
    /// The Trust List (ADR-0021, ADR-0019). Only this
    /// surface writes it.
    pub trust_list: Arc<dyn pagis_core::TrustListStore>,
    /// The daemon's secret store. It holds the hash of the Keypad Code.
    pub secrets: Arc<dyn pagis_core::SecretStore>,
    /// The failed-attempt count of each Workspace's Keypad Code
    /// (ADR-0021). The person reads it and clears it here.
    pub keypad_failures: Arc<dyn pagis_core::KeypadFailureStore>,
    /// The reader of the Needs-You Queue (ADR-0030). It derives the
    /// queue on each read and stores no copy of it.
    pub needs_you: Arc<NeedsYou>,
    /// The voice seam (ADR-0020): dictation in, spoken blocks
    /// out. Nothing it handles is stored.
    #[cfg(not(target_arch = "wasm32"))]
    pub voice: Arc<dyn pagis_voice::VoiceProvider>,
    /// Axum requires shared state to be `Send + Sync`. The voice provider
    /// stays on the WebAssembly host's one thread; the wrapper checks it.
    #[cfg(target_arch = "wasm32")]
    pub voice: send_wrapper::SendWrapper<Arc<dyn pagis_voice::VoiceProvider>>,
    pub files: Arc<dyn FileStore>,
    pub blobs: Arc<dyn ObjectStore>,
    /// The upload size cap; larger uploads get 413.
    pub file_max_bytes: u64,
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
    /// The Software List (ADR-0016). The desk reads it; only the
    /// Agent tools write it.
    pub software: Arc<dyn SoftwareStore>,
    /// The Org's Workspace ([`pagis_core::Org::workspace_id`]), which
    /// no person owns. It holds the installed Plugins with their
    /// Bindings and checkouts (ADR-0017) and the Installation
    /// Connections. They are read here whoever asks, and every tenant
    /// runs a Plugin in its own Plugin Computer.
    pub org_workspace_id: pagis_core::WorkspaceId,
    pub contributions: Arc<dyn pagis_core::ContributionStore>,
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub workspace_members: Arc<dyn pagis_core::WorkspaceMemberStore>,
    /// The installation. Exactly one Org row exists.
    pub orgs: Arc<dyn OrgStore>,
    /// The people of the Org.
    pub users: Arc<dyn UserStore>,
    /// What each model call spent. The Administrator reads spend
    /// per person, and each person reads their own.
    pub usage: Arc<dyn pagis_core::UsageStore>,
    /// The live Model Request Capture setting (ADR-0031), which the
    /// agent loop also reads.
    pub capture: Arc<pagis_core::CaptureSetting>,
    /// The captures an Administrator reads on the page of a Run.
    pub model_request_captures: Arc<dyn pagis_core::ModelRequestCaptureStore>,
    /// What every signed-in client holds.
    pub sessions: Arc<dyn SessionStore>,
    /// The Push Subscriptions of each Session (ADR-0030). Each one ends
    /// with its Session.
    pub calendar: Arc<dyn pagis_core::CalendarStore>,
    pub push_subscriptions: Arc<dyn pagis_core::PushSubscriptionStore>,
    /// The Notification sender of the installation (ADR-0030): one Web
    /// Push sender with the VAPID Key.
    pub notifications: Arc<Notifications>,
    /// The last client activity of each Person (ADR-0030). A new item
    /// waits while it is recent.
    pub person_activity: Arc<PersonActivity>,
    /// Reads the activity of each Person for presence. A host can read
    /// it from where it keeps the rest of the Person's data.
    pub person_activity_reader: Arc<dyn PersonActivityReader>,
    /// The signal of each Session that holds a live connection. A
    /// sign-out and an Administrator who ends every Session of a Person
    /// fire it, and so does the expiry of the Session, and every socket
    /// and Media Relay path of that Session closes.
    pub live_connections: Arc<LiveConnections>,
    /// The Sign-In Links: the start link the `pagis` binary prints, and
    /// the links of the Public Origin.
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
    /// The origin a browser reaches this installation at: the
    /// reverse proxy's on a server, loopback on a local installation. The
    /// CORS answer names it and no other, and the product port refuses a
    /// browser request from any other origin but [`AppState::local_origin`].
    pub public_origin: String,
    /// The loopback origin at which a program on this machine reaches the
    /// product port of a local installation: the origin of the owner's
    /// Client App window and of the Sign-In Link (ADR-0024). It is the
    /// Public Origin while Remote Access is off. `None` on a server,
    /// which no Client App opens at loopback.
    pub local_origin: Option<String>,
    /// Whether this daemon runs in Remote Access (ADR-0028):
    /// `[remote_access] enabled` of `config.toml`, or
    /// `PAGIS_REMOTE_ACCESS`. Another machine then signs in with a
    /// Sign-In Link and never with a password, and a local installation
    /// serves other machines.
    pub remote_access: bool,
    /// Where a browser reaches the Administration Interface. The
    /// product answers it to an Administrator, so the page can link to
    /// the port it draws no installation setting of its own for. The
    /// Administration Port refuses a browser request from any other
    /// origin.
    pub administration: user::AdministrationAddress,
    /// The links the Server configuration names for the Product App.
    pub product_app_links: Vec<ConfiguredLink>,
    /// The provider reads the Org name and the Org menu entries for each
    /// Person on each request.
    pub org_menu: Arc<dyn OrgMenu>,
    /// The notice the server resolves for each Person on each request.
    pub person_notice: Arc<dyn PersonNotice>,
    /// The optional sections this server offers on the Administration Port.
    pub administration_sections: AdministrationSections,
    /// The one address whose `X-Forwarded-*` headers the daemon believes.
    /// A local installation trusts none.
    pub proxy: TrustedProxy,
    pub ring: Arc<EventRing>,
    pub hub: Arc<StreamHub>,
    /// The derived run progress: the live line per run.
    pub progress: Arc<ProgressHub>,
    pub agents: Arc<AgentSystem>,
    pub keys: Arc<ProviderKeys>,
    /// The Provider Model Lists of the installation's keys.
    pub models: Arc<pagis_agent::ModelCatalog>,
    /// Which backend holds the records: `sqlite` or `postgres`.
    /// The Administration Interface reports it; nothing branches
    /// on it.
    pub database_backend: String,
    /// The daemon's logical time for owner messages and scheduled work.
    pub clock: Arc<dyn Clock>,
}

/// The two interfaces one daemon serves: the product on the port
/// the team reaches, the Administration Interface on a port of its own.
/// Both read one [`AppState`], because they are one process.
pub struct Routers {
    /// The state a host gives to the frame protocols.
    pub state: Arc<AppState>,
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
pub fn routers(state: AppState, product_fallback: Router) -> Routers {
    let state = Arc::new(state);
    let product_origins = std::iter::once(state.public_origin.clone())
        .chain(state.local_origin.clone())
        .collect();
    let product = cross_origin::refuse_other_origins(
        shared_workspace::router(product_router(Arc::clone(&state)).merge(product_fallback)),
        product_origins,
    );
    let administration = cross_origin::refuse_other_origins(
        administration::router(Arc::clone(&state)),
        vec![state.administration.origin.clone()],
    );
    Routers {
        state,
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
        .route("/api/v1/plugins", get(plugins::list_plugins))
        .route("/api/v1/plugins/{plugin_id}", get(plugins::get_plugin))
        .route(
            "/api/v1/plugins/{plugin_id}/grants",
            post(plugins::grant_plugin),
        )
        .route("/api/v1/plugins/{plugin_id}/log", get(plugins::plugin_log))
        .route(
            "/api/v1/knowledge/forget",
            get(forget::list).post(forget::confirm),
        )
        .route("/api/v1/calls", get(calls::list_calls))
        .route("/api/v1/calls/{call_id}", get(calls::get_call))
        .route("/api/v1/calls/{call_id}/dismiss", post(calls::dismiss_call))
        .route("/api/v1/calls/{call_id}/hangup", post(calls::hang_up))
        .route("/api/v1/calls/{call_id}/tier", post(calls::drop_call_tier))
        .route("/api/v1/calls/{call_id}/control", post(calls::control))
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
        .route(
            "/api/v1/agents",
            get(agents::list_agents).post(agents::create_agent),
        )
        .route("/api/v1/agents/{agent_id}", put(agents::update_agent))
        .route(
            "/api/v1/agents/{agent_id}/versions",
            get(agent_versions::list),
        )
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
        .route(
            "/api/v1/calendar/events",
            get(calendar::list_calendar_events).post(calendar::create_calendar_event),
        )
        .route(
            "/api/v1/calendar/events/{event_id}",
            get(calendar::get_calendar_event)
                .put(calendar::update_calendar_event)
                .delete(calendar::delete_calendar_event),
        )
        .route(
            "/api/v1/calendar/events/{event_id}/response",
            post(calendar_mail::answer_invitation),
        )
        .route(
            "/api/v1/workspace/mailbox/triage-rules",
            get(workspace_mail::list_mail_triage_rules)
                .put(workspace_mail::replace_mail_triage_rules),
        )
        .route(
            "/api/v1/workspace/mailbox/visibility",
            put(workspace_mail::set_workspace_mail_visibility),
        )
        .route(
            "/api/v1/workspace/mail/audit",
            get(workspace_mail::list_workspace_mail_audit),
        )
        .route(
            "/api/v1/workspace/mail/junk-policy",
            get(junk::get_workspace_policy).put(junk::set_workspace_policy),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/spam",
            post(junk::correct),
        )
        .route(
            "/api/v1/workspace/mailbox",
            get(workspace_mail::get_workspace_mailbox)
                .post(workspace_mail::create_workspace_mailbox),
        )
        .route(
            "/api/v1/workspace/mail/folders",
            get(workspace_mail::list_workspace_mail_folders)
                .post(workspace_mail::create_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages",
            get(workspace_mail::list_workspace_mail_messages)
                .post(workspace_mail::send_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/response",
            post(workspace_mail::answer_mail_invitation),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}",
            get(workspace_mail::get_workspace_mail_message)
                .delete(workspace_mail::delete_workspace_mail_message),
        )
        .route(
            "/api/v1/workspace/mail/threads/{thread_id}",
            get(workspace_mail::get_workspace_mail_thread),
        )
        .route(
            "/api/v1/workspace/mail/folders/{name}",
            patch(workspace_mail::rename_workspace_mail)
                .delete(workspace_mail::delete_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/reply",
            post(workspace_mail::reply_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/reply-all",
            post(workspace_mail::reply_all_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/forward",
            post(workspace_mail::forward_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/composer",
            get(workspace_mail::prepare_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/done",
            post(workspace_mail::done_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/move",
            post(workspace_mail::move_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/labels",
            post(workspace_mail::label_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/labels/{label}",
            delete(workspace_mail::unlabel_workspace_mail),
        )
        .route(
            "/api/v1/workspace/mail/messages/{message_id}/attachments/{index}",
            get(workspace_mail::download_workspace_mail_attachment),
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
        .route("/api/v1/free-busy", post(calendar::query_free_busy))
        .route("/api/v1/search", get(search::search))
        .route("/api/v1/colleagues", get(conversations::list_colleagues))
        .route(
            "/api/v1/conversations",
            get(conversations::list_conversations).post(conversations::start_conversation),
        )
        .route(
            "/api/v1/conversations/{conversation_id}/messages",
            get(conversations::list_conversation_messages)
                .post(conversations::send_conversation_message),
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
            "/api/v1/files",
            post(files::upload_file).layer(DefaultBodyLimit::max(
                state.file_max_bytes as usize + files::MULTIPART_OVERHEAD,
            )),
        )
        .route("/api/v1/files/{file_id}", get(files::download_file))
        .route("/api/v1/runs/{run_id}/cancel", post(runs::cancel_run))
        .route("/api/v1/runs/{run_id}/dismiss", post(runs::dismiss_run))
        .route("/api/v1/runs/{run_id}/retry", post(runs::retry_review))
        .route("/api/v1/runs/{run_id}/events", get(runs::run_events))
        .route("/api/v1/runs/{run_id}/steps", get(runs::run_steps))
        .route(
            "/api/v1/runs/{run_id}/model-requests",
            get(runs::model_requests),
        )
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
        .route("/api/v1/needs-you", get(needs_you::get_needs_you))
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
            "/api/v1/grants/workspace-mailbox",
            post(grants::create_workspace_mailbox_grant),
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
        .route("/api/v1/disclosure", get(disclosure::get_disclosure))
        .route("/api/v1/disclosure/profile", put(disclosure::set_profile))
        .route(
            "/api/v1/disclosure/rules",
            post(disclosure::add_rule).delete(disclosure::remove_rule),
        )
        .route(
            "/api/v1/workspace/delegate-everything",
            put(workspace::put_delegate_everything),
        )
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
        .route(
            "/api/v1/settings/credentials",
            get(settings::list_credentials).post(settings::add_credential),
        )
        .route(
            "/api/v1/settings/credentials/{credential_id}",
            delete(settings::delete_credential),
        )
        // What the signed-in person spent. A Member reads their own and
        // nothing else of the installation's accounting.
        .route("/api/v1/usage", get(administration::my_usage))
        .route("/api/v1/sessions/current", delete(sessions::sign_out))
        // The Person's own Sessions, and a Sign-In Link for one more
        // client of their own (ADR-0028).
        .route("/api/v1/settings/sessions", get(sessions::list_my_sessions))
        .route(
            "/api/v1/settings/sessions/{session_id}",
            delete(sessions::end_my_session),
        )
        .route(
            "/api/v1/settings/sign-in-links",
            post(sign_in_links::make_client_link),
        )
        // The VAPID Key of the installation, and the Push Subscriptions
        // of the Person's Sessions (ADR-0030).
        .route("/api/v1/push/key", get(push_subscriptions::get_vapid_key))
        .route(
            "/api/v1/push-subscriptions",
            get(push_subscriptions::list_push_subscriptions).post(push_subscriptions::subscribe),
        )
        .route(
            "/api/v1/push-subscriptions/{push_subscription_id}",
            delete(push_subscriptions::remove_push_subscription),
        )
        .route(
            "/api/v1/push-subscriptions/{push_subscription_id}/test",
            post(push_subscriptions::send_test_notification),
        )
        // The sockets read the same session cookie as every other
        // route, so they sit behind the same middleware. Their
        // first frame carries no credential. The origin check that
        // `routers` puts around the product router refuses an upgrade
        // that a page at another origin starts.
        .merge(native_product_routes())
        .merge(workspace_members::router())
        .route(
            "/api/v1/workspaces",
            get(workspace::list_accessible_workspaces),
        )
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::authenticate,
        ));

    Router::new()
        .merge(native_public_routes())
        .route("/api/v1/health", get(health))
        .route("/api/v1/runtime/identity", get(runtime_identity))
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
        .route("/api/v1/sessions/link", post(sessions::sign_in_with_link))
        .route(
            "/api/v1/sessions/link/{code}",
            get(sessions::sign_in_with_start_link),
        )
        .layer(middleware::from_fn(shared_workspace::unprefixed_only))
        .merge(authed)
        // One origin may make a credentialed cross-origin call: the one
        // this installation is served from. The layer answers the
        // preflight, so no route handles `OPTIONS`.
        .layer(cors::cors_layer(&public_origin))
        .with_state(state)
}

/// What the desktop shell reads before it attaches to a daemon it did
/// not start (ADR-0025): the daemon answers, and it says its version.
/// The sign-in page of the Product App reads how its browser signs in
/// (ADR-0028).
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct HealthDto {
    pub status: String,
    pub version: String,
    /// How the client that asks signs in: `link` from another machine in
    /// Remote Access, else `password`.
    pub sign_in: remote_access::SignInMethod,
}

#[utoipa::path(get, path = "/api/v1/health", responses((status = 200, body = HealthDto)))]
async fn health(
    State(state): State<Arc<AppState>>,
    Peer(peer): Peer,
    headers: axum::http::HeaderMap,
) -> Json<HealthDto> {
    Json(HealthDto {
        status: "ok".to_string(),
        version: VERSION.to_string(),
        sign_in: remote_access::sign_in_method(state.remote_access, peer, &headers),
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
    Peer(peer): Peer,
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
    let computer_image = pagis_versions::COMPUTER_IMAGE;
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

#[cfg(feature = "native")]
async fn openapi_json() -> Json<serde_json::Value> {
    Json(ApiDoc::openapi())
}

/// The contract of the full build, which a build without `native` serves
/// too (ADR-0038). The `contract-drift` step keeps `ui/openapi.json`
/// equal to what the full build generates.
#[cfg(not(feature = "native"))]
const FULL_CONTRACT: &str = include_str!("../../../ui/openapi.json");

#[cfg(not(feature = "native"))]
async fn openapi_json() -> Json<serde_json::Value> {
    Json(serde_json::from_str(FULL_CONTRACT).expect("the full contract is JSON"))
}

#[cfg(all(test, not(feature = "native")))]
mod full_contract_tests {
    #[tokio::test]
    async fn a_build_without_native_serves_the_full_contract() {
        let served = super::openapi_json().await.0;
        let expected: serde_json::Value = serde_json::from_str(super::FULL_CONTRACT).unwrap();
        assert_eq!(served, expected);
        assert!(served["paths"]["/api/v1/calls/{call_id}/hangup"].is_object());
    }
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

#[cfg(all(test, not(feature = "native")))]
mod calls_portable_tests;

mod legal_hold;
pub mod mail_opinion;

mod junk;
mod person_defaults;
