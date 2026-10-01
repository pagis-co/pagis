//! A person's own settings: onboarding, the model aliases, the
//! retention windows, their own Connections and their saved logins.
//!
//! The onboarding wizard reads one status document, stores the
//! installation's first provider key and Docker endpoint, and records
//! completion. Those two first-run writes are the only installation
//! settings the product port takes, and only until onboarding
//! completes; the provider routes of the Administration Port change
//! them afterwards. The product port refuses every other installation
//! part of every provider ([`crate::providers`]).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use pagis_core::{OnboardingModelVerification, Provider, ProviderKeys, now_ms};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::{Administrator, Tenant};
use crate::error::ApiError;

/// One provider's key status. `source` is `env`, `config`, or
/// `secret_file` when a key is configured.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderKeyDto {
    pub provider: String,
    pub configured: bool,
    pub source: Option<String>,
    /// What the key does in Pagis: `thinking`, `spoken_replies`,
    /// `dictation` and `calls`.
    pub uses: Vec<String>,
}

/// Everything the onboarding wizard needs in one read.
#[derive(Debug, Serialize, ToSchema)]
pub struct OnboardingDto {
    pub completed: bool,
    pub providers: Vec<ProviderKeyDto>,
    /// The server-owned key check of each provider that passed one. A
    /// check whose credential has changed since is left out.
    pub checks: Vec<ModelCheckDto>,
    /// Docker discovery. A Member reads the endpoint in use and no
    /// candidate: the candidates name the sockets of the installation's
    /// own host.
    pub docker: crate::system::DockerReportDto,
    /// The Docker endpoint an Administrator typed, or `null` for
    /// discovery. It is `null` for a Member.
    pub docker_endpoint: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetOnboardingDockerEndpointRequest {
    /// A socket path, a `unix://` or a `tcp://` endpoint. Null gives
    /// discovery the choice back. Pagis pings the endpoint before it
    /// saves.
    pub docker_endpoint: Option<String>,
}

/// What one key check proved: the provider listed its models for the
/// key.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ModelCheckDto {
    pub provider: String,
    /// How many models the provider lists for the key.
    pub available: i64,
}

fn model_proof(provider: Provider, source: pagis_core::KeySource, key: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let message = format!(
        "pagis-onboarding-key-v1\0{}\0{}",
        provider.id(),
        source.as_str(),
    );
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// The key checks whose credential still resolves: a check proves the
/// key it read the list with, and no other.
async fn current_checks(state: &AppState, tenant: &Tenant) -> Result<Vec<ModelCheckDto>, ApiError> {
    let mut checks = Vec::new();
    for saved in state
        .onboarding
        .model_verifications(&tenant.workspace_id)
        .await?
    {
        let Some(provider) = Provider::from_id(&saved.provider) else {
            continue;
        };
        let Some((key, source)) = state.keys.resolve(provider).map_err(secret_error)? else {
            continue;
        };
        if saved.proof == model_proof(provider, source, &key) {
            checks.push(ModelCheckDto {
                provider: saved.provider,
                available: saved.available,
            });
        }
    }
    Ok(checks)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetProviderKeyRequest {
    pub key: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateModelAliasRequest {
    pub candidates: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelAliasSettingDto {
    pub alias: String,
    pub label: String,
    pub description: String,
    pub when_candidates: Vec<String>,
    pub candidates: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelAliasDto {
    pub alias: String,
    pub candidates: Vec<String>,
    /// Whether a candidate names a provider that holds a key and serves
    /// the alias's use. An alias that is not reachable fails each call.
    pub reachable: bool,
    pub settings: Vec<ModelAliasSettingDto>,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelAliasPage {
    pub items: Vec<ModelAliasDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateModelAliasRequest {
    pub alias: String,
    pub candidates: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectionDto {
    pub id: String,
    pub provider: String,
    /// What this Connection gives an Agent, from the provider's catalog
    /// entry: `mail`, `calendar`, `mailboxes` or `telephony`.
    /// The card follows this, not the provider name.
    pub capabilities: Vec<String>,
    /// What this Connection's provider does not do, and a sibling
    /// provider does: a carrier that carries no text lists `texting`
    /// (ADR-0020). The card states each one.
    pub absent_capabilities: Vec<String>,
    /// The name a tool call uses to pick this account.
    pub alias: String,
    pub display_name: String,
    /// `disconnected`, `connecting`, `connected`, `reauth_required`, or
    /// `unavailable` once the provider refuses the key it holds.
    pub status: String,
    /// `byo` when the user supplies the OAuth client, `brokered` when
    /// the installation supplies it through its Installation OAuth Client.
    pub auth_mode: String,
    pub authorized_capabilities: Vec<String>,
    /// The external account this Connection binds to. It is not a
    /// secret, and the card names the account the user connected.
    pub account: Option<String>,
    /// The SIP username of a carrier Connection, once the user
    /// entered the credential. The password is never here.
    pub sip_username: Option<String>,
    /// The registrar the carrier's lines register at.
    pub sip_domain: Option<String>,
    /// The mail settings of a Mailbox Provider (ADR-0019), for a
    /// `migadu` or `manual` Connection.
    pub mail: Option<MailboxProviderDto>,
    /// True for an Installation Connection: the carrier account or the
    /// mail domain. The Administration Interface sets it up, and the
    /// product shows it without a way to change it.
    pub installation: bool,
    pub created_at: i64,
}

/// What one Mailbox Provider holds and what it can do
/// (ADR-0019). A capability that is absent is shown as absent, so the
/// page says what the user must do by hand.
#[derive(Debug, Serialize, ToSchema)]
pub struct MailboxProviderDto {
    /// The domain the mailboxes live on, e.g. `example.com`.
    pub domain: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    /// The transport waits for new mail instead of polling.
    pub idle: bool,
    /// The host caps the mailbox's own sends. The daemon enforces the
    /// Outgoing Cap for every host either way.
    pub outgoing_cap: bool,
    /// The host deletes the mailbox. Without it the user deletes the
    /// mailbox at the host.
    pub delete_mailbox: bool,
    /// The daemon mints a new mailbox password. Without it the user
    /// pastes one.
    pub reset_password: bool,
}

impl From<pagis_mail::MailboxProvider> for MailboxProviderDto {
    fn from(provider: pagis_mail::MailboxProvider) -> Self {
        Self {
            domain: provider.domain,
            imap_host: provider.imap.host,
            imap_port: provider.imap.port,
            smtp_host: provider.smtp.host,
            smtp_port: provider.smtp.port,
            idle: provider.capabilities.idle,
            outgoing_cap: provider.capabilities.outgoing_cap,
            delete_mailbox: provider.capabilities.delete_mailbox,
            reset_password: provider.capabilities.reset_password,
        }
    }
}

impl From<pagis_core::Connection> for ConnectionDto {
    fn from(connection: pagis_core::Connection) -> Self {
        let (sip_username, sip_domain) = pagis_telephony::sip_identity(&connection).unzip();
        let mail = pagis_mail::mailbox_provider(&connection).map(MailboxProviderDto::from);
        Self {
            id: connection.id.to_string(),
            capabilities: pagis_connect::capabilities(&connection.provider),
            absent_capabilities: pagis_connect::absent_capabilities(&connection.provider),
            installation: pagis_connect::is_installation_provider(&connection.provider),
            provider: connection.provider,
            alias: connection.alias,
            display_name: connection.display_name,
            status: connection.status,
            auth_mode: connection.auth_mode,
            authorized_capabilities: connection.authorized_capabilities,
            account: connection.config["account"].as_str().map(str::to_string),
            sip_username,
            sip_domain,
            mail,
            created_at: connection.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectionPage {
    pub items: Vec<ConnectionDto>,
}

/// The connect flow's second step (ADR-0012): the provider, the
/// names this Connection carries, and the fields the provider's
/// catalog entry declares.
///
/// A secret field passes through to the provider or the secret store
/// once and is never written to the database. The type carries no
/// `Debug` so none of them reaches a log line.
#[derive(Deserialize, ToSchema)]
pub struct CreateConnectionRequest {
    /// The `id` of a catalog entry.
    pub provider: String,
    /// The name a tool call uses to pick this account. It is unique per
    /// workspace and uses lowercase letters, numbers, hyphens, or
    /// underscores.
    pub alias: String,
    pub display_name: String,
    /// The values of the entry's fields, by field key. A number field
    /// is sent as its decimal text.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}

/// One value a provider's setup form asks for.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderFieldDto {
    /// The key the create request carries the value under.
    pub key: String,
    pub label: String,
    /// The placeholder, with an example value.
    pub hint: String,
    /// `text` or `number`.
    pub kind: String,
    /// The form masks it, and the daemon never writes it to the
    /// database.
    pub secret: bool,
    pub default: Option<String>,
}

/// One provider Pagis connects, as the picker draws it. The
/// client holds no provider list of its own.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderEntryDto {
    /// The `provider` a Connection records.
    pub id: String,
    pub label: String,
    /// What the user reads before they type.
    pub blurb: String,
    /// `oauth` when the user finishes at the provider in the browser,
    /// `fields` when the form below is the whole setup.
    pub kind: String,
    pub fields: Vec<ProviderFieldDto>,
    /// What a Connection of this provider gives an Agent: `mail`,
    /// `calendar`, `mailboxes`, `telephony` or `texting`.
    pub capabilities: Vec<String>,
    /// What this provider does not do, and a sibling provider does:
    /// a carrier that carries no text lists `texting` here (ADR-0020).
    pub absent_capabilities: Vec<String>,
    /// How many Connections of this provider one Workspace holds, or
    /// null for as many as the user makes.
    pub max_instances: Option<u32>,
    pub default_display_name: String,
    pub default_alias: String,
    /// Where the user finds the values, e.g. `the Telnyx portal`.
    pub portal: Option<String>,
    /// True for the `oauth` entry of a brokered flow in which the browser
    /// that goes to the provider can first ask the Person to sign in to
    /// Pagis. False on a local installation with the multi-user mode
    /// off, whose start route asks for no Session, and for every other
    /// entry.
    pub browser_sign_in: bool,
}

impl From<&pagis_connect::ProviderField> for ProviderFieldDto {
    fn from(field: &pagis_connect::ProviderField) -> Self {
        Self {
            key: field.key.to_string(),
            label: field.label.to_string(),
            hint: field.hint.to_string(),
            kind: field.kind.as_str().to_string(),
            secret: field.secret,
            default: field.default.map(str::to_string),
        }
    }
}

impl ProviderEntryDto {
    /// One entry as this installation serves it. `browser_sign_in` says
    /// whether the browser step of a brokered flow can ask for a sign-in
    /// here; only an `oauth` entry has a browser step.
    fn new(entry: &pagis_connect::ProviderEntry, browser_sign_in: bool) -> Self {
        Self {
            id: entry.id.to_string(),
            label: entry.label.to_string(),
            blurb: entry.blurb.to_string(),
            kind: entry.kind.as_str().to_string(),
            fields: entry.fields.iter().map(ProviderFieldDto::from).collect(),
            capabilities: entry
                .capabilities
                .iter()
                .map(|capability| capability.to_string())
                .collect(),
            absent_capabilities: entry
                .absent_capabilities
                .iter()
                .map(|capability| capability.to_string())
                .collect(),
            max_instances: entry.max_instances,
            default_display_name: entry.default_display_name.to_string(),
            default_alias: entry.default_alias.to_string(),
            portal: entry.portal.map(str::to_string),
            browser_sign_in: browser_sign_in && entry.kind == pagis_connect::ProviderKind::Oauth,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderPage {
    pub items: Vec<ProviderEntryDto>,
}

/// The capabilities the user grants this Connection. An empty
/// list authorizes the read-only profile a new Connection starts at.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct AuthorizeConnectionRequest {
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// A replacement carrier or mail host API key. The provider proves
    /// it before Pagis keeps it, and a Google connection ignores it.
    pub api_key: Option<String>,
}

/// One saved login as settings shows it. There is no secret
/// here, and no endpoint that returns one.
#[derive(Debug, Serialize, ToSchema)]
pub struct CredentialDto {
    pub id: String,
    /// The site's registrable domain.
    pub domain: String,
    pub username: String,
    /// The one address a fill opens.
    pub login_url: String,
    /// `user_supplied` or `agent_minted`.
    pub provenance: String,
    /// True when the record carries a one-time code seed.
    pub has_totp: bool,
    /// The owning agent, or null for the user. Archiving an agent
    /// clears it and leaves the record usable.
    pub owner_agent_id: Option<String>,
    pub created_at: i64,
}

impl From<pagis_vault::CredentialView> for CredentialDto {
    fn from(view: pagis_vault::CredentialView) -> Self {
        Self {
            id: view.id,
            domain: view.domain,
            username: view.username,
            login_url: view.login_url,
            provenance: view.provenance,
            has_totp: view.has_totp,
            owner_agent_id: view.owner_agent_id,
            created_at: view.created_at,
        }
    }
}

/// The user's own login, typed into settings. This is the primary
/// way an existing account reaches the vault; takeover login stays as
/// the escape hatch.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AddCredentialRequest {
    /// The site's registrable domain, e.g. `example.com`.
    pub domain: String,
    pub username: String,
    /// The sign-in address; its registrable domain must equal `domain`.
    pub login_url: String,
    /// The password. It is sealed on arrival and never read back.
    pub secret: String,
    /// An optional base32 one-time code seed.
    pub totp_seed: Option<String>,
}

/// What one authorization step answers.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuthorizeConnectionResponse {
    pub connection: ConnectionDto,
    /// The address to open for a `brokered` Google Connection: the
    /// start route on the Public Origin. A browser with a Session of the
    /// Person who asked goes on from there to consent at Google, and the
    /// connection reaches `connected` when Google redirects that browser
    /// back to this installation. A local installation with the
    /// multi-user mode off has one Person and asks the browser for no
    /// Session. Absent for every Connection this request already
    /// finished.
    pub authorization_url: Option<String>,
}

/// The address of the start route.
#[derive(Debug, Deserialize)]
pub struct GoogleStartQuery {
    /// The value the daemon minted for one authorization.
    #[serde(default)]
    pub state: String,
}

/// What Google puts on the redirect it sends the browser.
#[derive(Debug, Deserialize)]
pub struct GoogleCallbackQuery {
    /// The value the daemon minted, which names the Connection, and the
    /// Person and the Session that started the authorization.
    #[serde(default)]
    pub state: String,
    pub code: Option<String>,
    /// `access_denied` when the person declined.
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CredentialPage {
    pub items: Vec<CredentialDto>,
}

/// Refuse the product port a provider the installation sets up. The
/// carrier account and the mail domain are Installation Connections:
/// an Administrator sets up either in the Administration Interface,
/// and no person creates one here.
fn refuse_installation_provider(provider: &str) -> Result<(), ApiError> {
    if !pagis_connect::is_installation_provider(provider) {
        return Ok(());
    }
    let label = pagis_connect::entry(provider)
        .map(|entry| entry.label)
        .unwrap_or(provider);
    Err(ApiError::forbidden(format!(
        "{label} belongs to the installation; an administrator sets it up in the \
         Administration Interface"
    )))
}

/// Refuse the product port a change to an Installation Connection by
/// id. Every person reads the Org's Connections, so each one hears
/// that an administrator changes them. Any other Connection that is not
/// this Workspace's is a `404` from the handler that follows, which
/// says nothing about whose it is.
async fn refuse_installation_connection(
    state: &AppState,
    id: &pagis_core::ConnectionId,
) -> Result<(), ApiError> {
    match state.connections.get(&state.org_workspace_id, id).await? {
        Some(connection) => refuse_installation_provider(&connection.provider),
        None => Ok(()),
    }
}

#[utoipa::path(get, path = "/api/v1/settings/connections", responses(
    (status = 200, body = ConnectionPage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_connections(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<ConnectionPage>, ApiError> {
    // The person's own Connections, then the Org's Installation
    // Connections. Each of those says so: the page shows the carrier and
    // the mail domain with the numbers and the mailboxes on them, and no
    // way to change either.
    let mut connections = state.connections.list(&tenant.workspace_id).await?;
    connections.extend(state.connections.list(&state.org_workspace_id).await?);
    let items = connections.into_iter().map(ConnectionDto::from).collect();
    Ok(Json(ConnectionPage { items }))
}

/// The Provider Catalog (ADR-0012) a person picks from: every provider
/// a person connects on their own, with the form each one needs. An
/// Installation Connection is not in it.
///
/// The Google entry follows the installation. Where the Org holds
/// a Web OAuth client the form asks for the account alone, because the
/// client is the installation's; where it holds none the form asks for
/// the person's own Desktop client. The entry also says whether the
/// browser step can ask for a sign-in, which a local installation with
/// the multi-user mode off never does.
#[utoipa::path(get, path = "/api/v1/settings/connections/providers", responses(
    (status = 200, body = ProviderPage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_connection_providers(
    State(state): State<Arc<AppState>>,
    _tenant: Tenant,
) -> Result<Json<ProviderPage>, ApiError> {
    let brokered = state
        .connector
        .google()
        .web_client()
        .await
        .map_err(connect_error)?
        .is_some();
    let browser_sign_in = brokered && !crate::system::serves_this_machine_only(&state);
    Ok(Json(ProviderPage {
        items: pagis_connect::person_catalog(brokered)
            .iter()
            .map(|entry| ProviderEntryDto::new(entry, browser_sign_in))
            .collect(),
    }))
}

/// Connect an account (ADR-0012). The record lands at
/// `disconnected` with its binding; `authorize_connection` is the step
/// that sends the user to Google.
#[utoipa::path(post, path = "/api/v1/settings/connections",
    request_body = CreateConnectionRequest, responses(
        (status = 201, body = ConnectionDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn create_connection(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    tenant: Tenant,
    Json(request): Json<CreateConnectionRequest>,
) -> Result<(StatusCode, Json<ConnectionDto>), ApiError> {
    refuse_installation_provider(&request.provider)?;
    let credentials =
        pagis_connect::NewCredentials::from_fields(&request.provider, &request.fields)
            .map_err(connect_error)?;
    let connection = state
        .connector
        .create(pagis_connect::NewConnection {
            workspace_id: tenant.workspace_id.clone(),
            alias: request.alias,
            display_name: request.display_name,
            credentials,
            source: crate::forwarded::request_source(peer, &headers),
        })
        .await
        .map_err(connect_error)?;
    publish_settings_event(
        &state,
        &tenant,
        "connection.created",
        serde_json::json!({
            "connection_id": connection.id.as_str(),
            "alias": connection.alias.as_str(),
            "provider": connection.provider.as_str(),
        }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(ConnectionDto::from(connection))))
}

/// Authorize one Connection.
///
/// A `brokered` Google Connection answers an `authorization_url` and
/// returns at once. The URL is the start route on the Public Origin,
/// which belongs to the Person who asks: the client opens it, the person
/// consents at Google in that browser, and the public callback route is
/// what makes the Connection `connected`. Every other Connection is
/// finished when this returns, and `authorization_url` is absent.
#[utoipa::path(post, path = "/api/v1/settings/connections/{connection_id}/authorize",
    params(("connection_id" = String, Path)),
    request_body = AuthorizeConnectionRequest, responses(
        (status = 200, body = AuthorizeConnectionResponse),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn authorize_connection(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    tenant: Tenant,
    Path(connection_id): Path<String>,
    body: Option<Json<AuthorizeConnectionRequest>>,
) -> Result<Json<AuthorizeConnectionResponse>, ApiError> {
    let request = body.map(|Json(request)| request).unwrap_or_default();
    let id = pagis_core::ConnectionId::from(connection_id);
    refuse_installation_connection(&state, &id).await?;
    let answer = state
        .connector
        .authorize(
            &tenant.workspace_id,
            &id,
            &request.capabilities,
            request.api_key.as_deref(),
            crate::forwarded::request_source(peer, &headers),
            &initiator(&tenant),
        )
        .await
        .map_err(connect_error)?;
    let authorization_url = answer.url().map(str::to_string);
    let connection = answer.into_connection();
    publish_settings_event(
        &state,
        &tenant,
        "connection.changed",
        serde_json::json!({
            "connection_id": connection.id.as_str(),
            "status": connection.status.as_str(),
        }),
    )
    .await?;
    Ok(Json(AuthorizeConnectionResponse {
        connection: ConnectionDto::from(connection),
        authorization_url,
    }))
}

/// The Person and the Session of a request, as an authorization names
/// them.
pub(crate) fn initiator(tenant: &Tenant) -> pagis_connect::Initiator {
    pagis_connect::Initiator {
        user_id: tenant.user_id.clone(),
        session_id: tenant.session_id.clone(),
    }
}

/// The cookie that binds a pending Google authorization to the browser
/// that goes to Google (RFC 9700, section 2.1.1). It holds the hash of
/// the `state`, not the `state`.
///
/// It is `SameSite=Lax`, not `Strict`: the redirect from Google is a
/// cross-site top-level navigation, and a `Strict` cookie does not come
/// with it. Its other attributes follow the scheme of the Public Origin,
/// as the `Secure` attribute of the Session cookie follows TLS.
struct TransactionCookie {
    name: &'static str,
    secure: bool,
}

impl TransactionCookie {
    /// The transaction cookie of an installation at `public_origin`.
    ///
    /// Over `https:` it is `__Host-` and `Secure`: the prefix makes a
    /// browser keep it for this host alone, with `Path=/` and no
    /// `Domain`. A Server and a Local Installation in the multi-user mode
    /// are there, because Google takes a plain `http:` redirect URI only
    /// on loopback. A Local Installation on a loopback `http:` origin
    /// gets neither: Safari refuses a `Secure` cookie from a plain `http:`
    /// origin and Chrome refuses the prefix there, so the flow would
    /// never finish. The cookie is still host-only, and on that origin
    /// only programs of the same machine reach the host.
    fn at(public_origin: &str) -> Self {
        let tls = url::Url::parse(public_origin).is_ok_and(|url| url.scheme() == "https");
        match tls {
            true => Self {
                name: "__Host-pagis_google_authorization",
                secure: true,
            },
            false => Self {
                name: "pagis_google_authorization",
                secure: false,
            },
        }
    }

    /// The `Set-Cookie` value that binds `state` to the browser for as
    /// long as the authorization waits.
    fn set(&self, state: &str) -> String {
        self.with_value(
            &crate::auth::hash_secret(state),
            pagis_connect::AUTHORIZE_WINDOW_MS / 1_000,
        )
    }

    /// The `Set-Cookie` value that takes the cookie away again. It
    /// carries the same attributes, so a browser replaces the cookie it
    /// holds.
    fn clear(&self) -> String {
        self.with_value("", 0)
    }

    /// Whether the browser holds the cookie of `state`.
    fn binds(&self, headers: &HeaderMap, state: &str) -> bool {
        crate::auth::cookie(headers, self.name)
            .is_some_and(|value| value == crate::auth::hash_secret(state))
    }

    fn with_value(&self, value: &str, max_age: i64) -> String {
        format!(
            "{}={value}; Path=/; HttpOnly{}; SameSite=Lax; Max-Age={max_age}",
            self.name,
            if self.secure { "; Secure" } else { "" }
        )
    }
}

/// The address of the Product App that a browser with no Session goes
/// to from the start route. The Product App shows its sign-in page at
/// every address it opens with no Session and keeps the address, and at
/// this one it goes back to the start route.
const SIGN_IN_AND_START: &str = "/connections/google/start";

/// Send one browser to Google for a brokered authorization.
///
/// The authorize request answers this address, and it belongs to the
/// Person who asked. It sits outside the session middleware because a
/// browser with no Session must sign in and come back, not get `401`: it
/// goes to the same address in the Product App, which shows the sign-in
/// page and then comes back here. A Session of another Person gets a
/// refusal. For the initiating Person, the route sets the transaction
/// cookie, which binds the `state` to this browser, and redirects to
/// Google.
///
/// A local installation with the multi-user mode off has one Person and
/// answers only programs of its own machine. There the route asks the
/// browser for no Session: the Client App opens it in the system
/// browser, and the owner does not sign in a second time. The callback
/// keeps every check.
#[utoipa::path(get, path = "/api/v1/connections/google/start",
    params(("state" = String, Query)),
    responses(
        (status = 303, description = "To Google with the transaction cookie, or to the sign-in page of the Product App when the browser holds no Session"),
        (status = 403, description = "A page that says the Session belongs to another Person"),
        (status = 410, description = "A page that says no authorization waits under this state"),
    ))]
pub async fn google_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<GoogleStartQuery>,
) -> Result<axum::response::Response, ApiError> {
    use axum::http::header;
    use axum::response::IntoResponse;

    let tenant = match crate::system::serves_this_machine_only(&state) {
        true => None,
        false => match crate::auth::resolve(&state, &headers).await? {
            Some(tenant) => Some(tenant),
            None => {
                let back = url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("state", &query.state)
                    .finish();
                return Ok(
                    axum::response::Redirect::to(&format!("{SIGN_IN_AND_START}?{back}"))
                        .into_response(),
                );
            }
        },
    };
    let opener = match &tenant {
        Some(tenant) => pagis_connect::Opener::SignedIn(&tenant.user_id),
        None => pagis_connect::Opener::ThisMachine,
    };
    let answer = match state.connector.google().start(&query.state, opener) {
        Ok(google) => (
            StatusCode::SEE_OTHER,
            [
                (
                    header::SET_COOKIE,
                    TransactionCookie::at(&state.public_origin).set(&query.state),
                ),
                (header::LOCATION, google),
            ],
        )
            .into_response(),
        Err(pagis_connect::StartRefusal::AnotherPerson) => (
            StatusCode::FORBIDDEN,
            google_page(
                "This Google sign-in belongs to another person. Sign in to Pagis as the \
                 person who started it.",
            ),
        )
            .into_response(),
        Err(pagis_connect::StartRefusal::NotWaiting) => (
            StatusCode::GONE,
            google_page(
                "This Google sign-in is no longer waiting. Go back to Pagis and try again.",
            ),
        )
            .into_response(),
    };
    Ok(answer)
}

/// Finish one brokered Google authorization.
///
/// Google redirects the person's browser here on the installation's own
/// public origin. The request carries no Session: the Session cookie is
/// `SameSite=Strict`, so a browser sends none on a cross-site
/// navigation. The route finishes only for the browser that holds the
/// transaction cookie of the `state`, which the start route set. The
/// `state` names the Connection, and the Person and the Session that
/// started the authorization: that Session must still be live, and the
/// Google account that consented must be the account of the Connection.
/// A redirect without the cookie spends the `state` and connects
/// nothing. Each answer clears the cookie.
#[utoipa::path(get, path = "/api/v1/connections/google/callback",
    params(
        ("state" = String, Query),
        ("code" = Option<String>, Query),
        ("error" = Option<String>, Query),
    ),
    responses((status = 200, description = "A page that tells the person to go back to Pagis")))]
pub async fn google_callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<GoogleCallbackQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    const REFUSED: &str = "Google did not complete this. Go back to Pagis and try again.";
    let google = state.connector.google();
    let cookie = TransactionCookie::at(&state.public_origin);
    let bound = cookie.binds(&headers, &query.state);
    let declined = query.error.as_deref().filter(|reason| !reason.is_empty());
    let message = if !bound || declined.is_some() {
        match declined {
            // Google sends `error=access_denied` when the person declines.
            Some(reason) if bound => tracing::info!(%reason, "a person declined at Google"),
            _ => {
                tracing::warn!("a Google callback came without the transaction cookie of its state")
            }
        }
        if let Err(error) = google.abandon(&query.state).await {
            tracing::error!(%error, "abandoning a Google authorization failed");
        }
        REFUSED
    } else {
        match google
            .finish(&query.state, query.code.as_deref().unwrap_or_default())
            .await
        {
            Ok(connection) => {
                // The client is waiting on the connection list, so the
                // event is what closes its loop.
                publish_connection_changed(&state, &connection).await;
                "Google is connected. You can close this tab and go back to Pagis."
            }
            Err(error) => {
                tracing::warn!(%error, "a Google callback did not finish");
                REFUSED
            }
        }
    };
    (
        [(axum::http::header::SET_COOKIE, cookie.clear())],
        google_page(message),
    )
        .into_response()
}

/// The one page the start route and the callback answer. It names no
/// account and no Workspace: a browser that arrives with a `state`
/// somebody else minted must learn nothing from it.
fn google_page(message: &str) -> axum::response::Html<String> {
    axum::response::Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>Pagis</title></head><body style=\"font:16px system-ui;padding:3rem\">\
         <p>{message}</p></body></html>"
    ))
}

/// A connect failure the user can act on keeps its words; a provider
/// refusal reports the stable code and nothing from upstream.
pub(crate) fn connect_error(error: pagis_connect::ConnectError) -> ApiError {
    use pagis_connect::ConnectError;
    match error {
        ConnectError::Store(error) => ApiError::from(error),
        ConnectError::Validation(message) => ApiError::validation(message),
        ConnectError::Conflict(message) => ApiError::conflict(message),
        ConnectError::NotFound => ApiError::not_found("connection"),
        ConnectError::Provider(code) => ApiError::validation(format!(
            "the provider did not complete the connection ({})",
            code.as_str()
        )),
    }
}

#[utoipa::path(delete, path = "/api/v1/settings/connections/{connection_id}",
    params(("connection_id" = String, Path)), responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn delete_connection(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(connection_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = pagis_core::ConnectionId::from(connection_id);
    refuse_installation_connection(&state, &id).await?;
    if !state
        .connections
        .delete_and_revoke(&tenant.workspace_id, &id, now_ms())
        .await?
    {
        return Err(ApiError::not_found("connection"));
    }
    publish_settings_event(
        &state,
        &tenant,
        "connection.deleted",
        serde_json::json!({
            "connection_id": id.as_str()
        }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/settings/credentials", responses(
    (status = 200, body = CredentialPage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_credentials(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<CredentialPage>, ApiError> {
    let items = state
        .vault
        .list(&tenant.workspace_id, None)
        .await
        .map_err(vault_error)?
        .into_iter()
        .map(CredentialDto::from)
        .collect();
    Ok(Json(CredentialPage { items }))
}

/// Save a login the user already has (ADR-0013). The secret is
/// sealed here and never leaves the daemon again.
#[utoipa::path(post, path = "/api/v1/settings/credentials",
    request_body = AddCredentialRequest, responses(
        (status = 201, body = CredentialDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn add_credential(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<AddCredentialRequest>,
) -> Result<(StatusCode, Json<CredentialDto>), ApiError> {
    let view = state
        .vault
        .add_user_credential(pagis_vault::NewCredential {
            workspace_id: tenant.workspace_id.clone(),
            domain: request.domain,
            username: request.username,
            login_url: request.login_url,
            secret: request.secret,
            totp_seed: request.totp_seed,
        })
        .await
        .map_err(vault_error)?;
    publish_settings_event(
        &state,
        &tenant,
        "credential.created",
        serde_json::json!({ "credential_id": view.id, "domain": view.domain }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(CredentialDto::from(view))))
}

/// A vault failure the user can fix reads as a 422; the rest are 500s
/// with the detail in the log, never in the body.
fn vault_error(error: pagis_vault::VaultError) -> ApiError {
    match error {
        pagis_vault::VaultError::Store(error) => ApiError::from(error),
        pagis_vault::VaultError::Key(message) => {
            tracing::error!(error = %message, "vault key error");
            ApiError::internal()
        }
        other => ApiError::validation(other.to_string()),
    }
}

#[utoipa::path(delete, path = "/api/v1/settings/credentials/{credential_id}",
    params(("credential_id" = String, Path)), responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn delete_credential(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(credential_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = pagis_core::CredentialId::from(credential_id);
    if !state
        .credentials
        .delete_and_revoke(&tenant.workspace_id, &id, now_ms())
        .await?
    {
        return Err(ApiError::not_found("credential"));
    }
    publish_settings_event(
        &state,
        &tenant,
        "credential.deleted",
        serde_json::json!({
            "credential_id": id.as_str()
        }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The connection event the callback publishes. There is no Session on
/// that request, so the Workspace comes from the record the `state`
/// named and from nothing the caller sent.
async fn publish_connection_changed(state: &AppState, connection: &pagis_core::Connection) {
    if let Err(error) = state
        .bus
        .publish(pagis_core::NewEvent {
            workspace_id: connection.workspace_id.clone(),
            event_type: "connection.changed".to_string(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({
                "connection_id": connection.id.as_str(),
                "status": connection.status.as_str(),
            }),
        })
        .await
    {
        tracing::error!(%error, "publishing the Google callback event failed");
    }
}

async fn publish_settings_event(
    state: &AppState,
    tenant: &Tenant,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), ApiError> {
    state
        .bus
        .publish(pagis_core::NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: event_type.to_string(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload,
        })
        .await?;
    Ok(())
}

fn validate_alias(alias: &str, candidates: &[String]) -> Result<(), ApiError> {
    if alias.is_empty()
        || alias.chars().any(|character| {
            !character.is_ascii_lowercase()
                && !character.is_ascii_digit()
                && character != '-'
                && character != '_'
        })
    {
        return Err(ApiError::validation(
            "alias must use lowercase letters, numbers, hyphens, or underscores",
        ));
    }
    if candidates.is_empty() {
        return Err(ApiError::validation(
            "an alias needs at least one candidate",
        ));
    }
    if candidates.iter().any(|candidate| {
        candidate.trim() != candidate
            || candidate
                .split_once('/')
                .is_none_or(|(provider, model)| provider.is_empty() || model.is_empty())
    }) {
        return Err(ApiError::validation(
            "each candidate must use provider/model format",
        ));
    }
    Ok(())
}

fn alias_dto(
    keys: &ProviderKeys,
    model_alias: pagis_core::ModelAlias,
) -> Result<ModelAliasDto, ApiError> {
    let reachable = !crate::model_lists::reachable_candidates(
        keys,
        crate::provisioning::alias_use(&model_alias.alias),
        model_alias.candidates.clone(),
    )?
    .is_empty();
    Ok(ModelAliasDto {
        alias: model_alias.alias,
        candidates: model_alias.candidates,
        reachable,
        settings: Vec::new(),
        updated_at: model_alias.updated_at,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/settings/model-aliases",
    responses(
        (status = 200, body = ModelAliasPage),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_model_aliases(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<ModelAliasPage>, ApiError> {
    let aliases = state.model_aliases.list(&tenant.workspace_id).await?;
    let candidates_by_alias: BTreeMap<_, _> = aliases
        .iter()
        .map(|model_alias| (model_alias.alias.clone(), model_alias.candidates.clone()))
        .collect();
    let items = aliases
        .into_iter()
        .map(|model_alias| {
            let settings = if model_alias.alias == pagis_telephony::PHONE_ALIAS {
                pagis_telephony::PHONE_MODEL_SETTINGS
                    .iter()
                    .map(|setting| ModelAliasSettingDto {
                        alias: setting.alias.to_string(),
                        label: setting.label.to_string(),
                        description: setting.description.to_string(),
                        when_candidates: setting
                            .when_candidates
                            .iter()
                            .map(|candidate| candidate.to_string())
                            .collect(),
                        candidates: candidates_by_alias
                            .get(setting.alias)
                            .cloned()
                            .unwrap_or_else(|| {
                                setting
                                    .default_candidates
                                    .iter()
                                    .map(|candidate| candidate.to_string())
                                    .collect()
                            }),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let mut dto = alias_dto(&state.keys, model_alias)?;
            dto.settings = settings;
            Ok(dto)
        })
        .collect::<Result<_, ApiError>>()?;
    Ok(Json(ModelAliasPage { items }))
}

#[utoipa::path(
    post,
    path = "/api/v1/settings/model-aliases",
    request_body = CreateModelAliasRequest,
    responses(
        (status = 201, body = ModelAliasDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn create_model_alias(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<CreateModelAliasRequest>,
) -> Result<(StatusCode, Json<ModelAliasDto>), ApiError> {
    validate_alias(&request.alias, &request.candidates)?;
    if state
        .model_aliases
        .get_by_alias(&tenant.workspace_id, &request.alias)
        .await?
        .is_some()
    {
        return Err(ApiError::conflict("model alias already exists"));
    }
    let now = now_ms();
    let model_alias = pagis_core::ModelAlias {
        id: pagis_core::ModelAliasId::generate(),
        workspace_id: tenant.workspace_id.clone(),
        alias: request.alias,
        candidates: request.candidates,
        created_at: now,
        updated_at: now,
    };
    state.model_aliases.create(&model_alias).await?;
    publish_settings_event(
        &state,
        &tenant,
        "model_alias.changed",
        serde_json::json!({ "alias": model_alias.alias.as_str(), "action": "created" }),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(alias_dto(&state.keys, model_alias)?),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/settings/model-aliases/{alias}",
    params(("alias" = String, Path)),
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    )
)]
pub async fn delete_model_alias(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(alias): Path<String>,
) -> Result<StatusCode, ApiError> {
    let in_use = state
        .agent_store
        .list_by_workspace(&tenant.workspace_id)
        .await?
        .iter()
        .any(|agent| agent.model_alias == alias);
    if in_use {
        return Err(ApiError::conflict("an agent uses this model alias"));
    }
    if !state
        .model_aliases
        .delete(&tenant.workspace_id, &alias)
        .await?
    {
        return Err(ApiError::not_found("model alias"));
    }
    publish_settings_event(
        &state,
        &tenant,
        "model_alias.changed",
        serde_json::json!({ "alias": alias, "action": "deleted" }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/model-aliases/{alias}",
    params(("alias" = String, Path)),
    request_body = UpdateModelAliasRequest,
    responses(
        (status = 200, body = ModelAliasDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn update_model_alias(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(alias): Path<String>,
    Json(request): Json<UpdateModelAliasRequest>,
) -> Result<Json<ModelAliasDto>, ApiError> {
    validate_alias(&alias, &request.candidates)?;
    let updated_at = now_ms();
    if !state
        .model_aliases
        .update_candidates(
            &tenant.workspace_id,
            &alias,
            &request.candidates,
            updated_at,
        )
        .await?
    {
        return Err(ApiError::not_found("model alias"));
    }
    state
        .bus
        .publish(pagis_core::NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "model_alias.changed".to_string(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({ "alias": alias }),
        })
        .await?;
    let reachable = !crate::model_lists::reachable_candidates(
        &state.keys,
        crate::provisioning::alias_use(&alias),
        request.candidates.clone(),
    )?
    .is_empty();
    Ok(Json(ModelAliasDto {
        alias,
        candidates: request.candidates,
        reachable,
        settings: Vec::new(),
        updated_at,
    }))
}

/// The optional onboarding extras: the user's name seeds shared
/// memory so every agent knows what to call them.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct CompleteOnboardingRequest {
    pub user_name: Option<String>,
}

fn provider_statuses(keys: &ProviderKeys) -> Result<Vec<ProviderKeyDto>, ApiError> {
    let statuses = keys.status().map_err(secret_error)?;
    Ok(statuses
        .into_iter()
        .map(|status| ProviderKeyDto {
            provider: status.provider.id().to_string(),
            configured: status.source.is_some(),
            source: status.source.map(|source| source.as_str().to_string()),
            uses: status
                .provider
                .uses()
                .iter()
                .map(|provider_use| provider_use.id().to_string())
                .collect(),
        })
        .collect())
}

pub(crate) fn secret_error(err: pagis_core::SecretError) -> ApiError {
    tracing::error!(error = %err, "secret store error");
    ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: "internal",
        message: format!("cannot reach the platform secret store: {err}"),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/settings/onboarding",
    responses(
        (status = 200, body = OnboardingDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn onboarding_status(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<OnboardingDto>, ApiError> {
    let workspace = state
        .workspaces
        .get(&tenant.workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace"))?;
    let docker = state.docker_discovery.probe().await;
    Ok(Json(OnboardingDto {
        completed: workspace.onboarded_at.is_some(),
        // The provider keys are the installation's. A Member
        // reads none of them: on a server the Administrator supplies
        // them and a person connecting to one takes no key at all.
        // A local installation's one person is the
        // administrator, so their wizard is unchanged.
        providers: match tenant.role {
            pagis_core::UserRole::Administrator => provider_statuses(&state.keys)?,
            pagis_core::UserRole::Member => Vec::new(),
        },
        checks: current_checks(&state, &tenant).await?,
        docker: match tenant.role {
            pagis_core::UserRole::Administrator => docker.into(),
            pagis_core::UserRole::Member => crate::system::DockerReportDto {
                endpoint: docker.endpoint,
                candidates: Vec::new(),
            },
        },
        docker_endpoint: match tenant.role {
            pagis_core::UserRole::Administrator => {
                state
                    .system
                    .read()
                    .map_err(|error| {
                        tracing::error!(%error, "cannot read the config file");
                        ApiError::internal()
                    })?
                    .docker_endpoint
            }
            pagis_core::UserRole::Member => None,
        },
    }))
}

/// Refuse a first-run write once the Workspace finished onboarding.
///
/// The onboarding wizard is the one place the product port writes an
/// installation setting: a local installation's one person gives the
/// installation its model key and its Docker endpoint before anything
/// else. From the end of onboarding on, the installation settings
/// answer on the Administration Port alone (ADR-0024).
pub(crate) async fn refuse_after_onboarding(
    state: &AppState,
    tenant: &Tenant,
) -> Result<(), ApiError> {
    let workspace = state
        .workspaces
        .get(&tenant.workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace"))?;
    if workspace.onboarded_at.is_some() {
        return Err(ApiError::conflict(
            "setup is complete; the Administration Interface changes the installation settings",
        ));
    }
    Ok(())
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/onboarding/providers/{provider}/key",
    params(("provider" = String, Path, description = "`anthropic`, `openai`, or `openrouter`")),
    request_body = SetProviderKeyRequest,
    responses(
        (status = 200, body = ProviderKeyDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// The model step's key. It answers an Administrator while their
/// Workspace has not finished onboarding, and `409` afterwards.
pub async fn set_onboarding_provider_key(
    State(state): State<Arc<AppState>>,
    Administrator(tenant): Administrator,
    Path(provider): Path<String>,
    Json(request): Json<SetProviderKeyRequest>,
) -> Result<Json<ProviderKeyDto>, ApiError> {
    refuse_after_onboarding(&state, &tenant).await?;
    store_provider_key(&state, &provider, &request.key).map(Json)
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/onboarding/docker-endpoint",
    request_body = SetOnboardingDockerEndpointRequest,
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// The computer step's Docker endpoint. It answers an Administrator
/// while their Workspace has not finished onboarding, and `409`
/// afterwards.
pub async fn set_onboarding_docker_endpoint(
    State(state): State<Arc<AppState>>,
    Administrator(tenant): Administrator,
    Json(request): Json<SetOnboardingDockerEndpointRequest>,
) -> Result<StatusCode, ApiError> {
    refuse_after_onboarding(&state, &tenant).await?;
    let endpoint =
        crate::system::checked_docker_endpoint(&state, request.docker_endpoint.as_deref()).await?;
    crate::system::save_docker_endpoint(&state, endpoint)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/settings/onboarding/complete",
    request_body = CompleteOnboardingRequest,
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn complete_onboarding(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    body: Option<Json<CompleteOnboardingRequest>>,
) -> Result<StatusCode, ApiError> {
    // A stored key is enough to finish. The key check is the
    // person's choice, and an unchecked key never reads as working.
    let mut holds_a_key = false;
    for provider in pagis_core::PROVIDERS {
        if state
            .keys
            .resolve(provider)
            .map_err(secret_error)?
            .is_some()
        {
            holds_a_key = true;
            break;
        }
    }
    if !holds_a_key {
        return Err(ApiError::validation(
            "add a model key before you finish setup",
        ));
    }
    let request = body.map(|Json(request)| request).unwrap_or_default();
    if let Some(name) = request
        .user_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        seed_user_name(&state, &tenant, name).await?;
    }
    state
        .workspaces
        .set_onboarded(&tenant.workspace_id, now_ms())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/settings/providers/{provider}/check",
    params(("provider" = String, Path, description = "`anthropic`, `openai`, or `openrouter`")),
    responses(
        (status = 200, body = ModelCheckDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// Prove the provider's key with one call to the provider's model list.
/// The list call generates nothing and costs nothing. The check passes
/// when the provider answers the list, whatever the list holds, and it
/// fails with the provider's own words when the provider refuses the
/// key. A passed check also refreshes the installation's Provider Model
/// List of that provider.
pub async fn check_provider_model(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(provider): Path<String>,
) -> Result<Json<ModelCheckDto>, ApiError> {
    let provider = Provider::from_id(&provider).ok_or_else(|| ApiError::not_found("provider"))?;
    let Some((key, source)) = state.keys.resolve(provider).map_err(secret_error)? else {
        return Err(ApiError::validation(format!(
            "no key is configured for {}",
            provider.id()
        )));
    };
    let proof = model_proof(provider, source, &key);
    let listed = state
        .models
        .refresh(provider)
        .await
        .map_err(model_list_error)?;
    record_model_check(&state, &tenant, provider, &proof, listed.len())
        .await
        .map(Json)
}

#[utoipa::path(
    post,
    path = "/api/v1/settings/onboarding/providers/{provider}/key/check",
    params(("provider" = String, Path, description = "`anthropic`, `openai`, or `openrouter`")),
    request_body = SetProviderKeyRequest,
    responses(
        (status = 200, body = ModelCheckDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// The model step's check of a typed key. The daemon asks the provider
/// for its model list with the key, and stores the key only when the
/// provider answers. A key that the provider refuses is not stored, and
/// the refusal carries the provider's own words. It answers an
/// Administrator while their Workspace has not finished onboarding, and
/// `409` afterwards.
pub async fn check_onboarding_provider_key(
    State(state): State<Arc<AppState>>,
    Administrator(tenant): Administrator,
    Path(provider): Path<String>,
    Json(request): Json<SetProviderKeyRequest>,
) -> Result<Json<ModelCheckDto>, ApiError> {
    refuse_after_onboarding(&state, &tenant).await?;
    let provider = Provider::from_id(&provider).ok_or_else(|| ApiError::not_found("provider"))?;
    let key = request.key.trim();
    if key.is_empty() {
        return Err(ApiError::validation("key must not be empty"));
    }
    let listed = state
        .models
        .check(provider, key)
        .await
        .map_err(model_list_error)?;
    // The list is cached for this key, so the key goes in with no
    // second list call.
    state.keys.set(provider, key).map_err(secret_error)?;
    let proof = model_proof(provider, pagis_core::KeySource::SecretFile, key);
    record_model_check(&state, &tenant, provider, &proof, listed.len())
        .await
        .map(Json)
}

fn model_list_error(error: pagis_agent::ModelListError) -> ApiError {
    match error {
        pagis_agent::ModelListError::Secret(message) => {
            tracing::error!(%message, "secret store error");
            ApiError::internal()
        }
        other => ApiError::validation(other.to_string()),
    }
}

/// Record that the key `proof` names listed `listed` models. The proof
/// names the key the list call used, so a check whose key is not the
/// one that resolves now proves nothing: the key changed during the
/// call, or a key from the environment or the config file wins over it.
async fn record_model_check(
    state: &AppState,
    tenant: &Tenant,
    provider: Provider,
    proof: &str,
    listed: usize,
) -> Result<ModelCheckDto, ApiError> {
    let Some((key_after, source_after)) = state.keys.resolve(provider).map_err(secret_error)?
    else {
        return Err(ApiError::validation(
            "the provider key changed during the key check",
        ));
    };
    if model_proof(provider, source_after, &key_after) != proof {
        return Err(ApiError::validation(match source_after {
            pagis_core::KeySource::SecretFile => {
                "the provider key changed during the key check".to_string()
            }
            pagis_core::KeySource::Env => format!(
                "{} sets the key of {}, so Pagis does not use a typed key",
                provider.env_var(),
                provider.id()
            ),
            pagis_core::KeySource::Config => format!(
                "config.toml sets the key of {}, so Pagis does not use a typed key",
                provider.id()
            ),
        }));
    }
    let available = i64::try_from(listed).unwrap_or(i64::MAX);
    state
        .onboarding
        .set_model_verification(
            &tenant.workspace_id,
            &OnboardingModelVerification {
                provider: provider.id().to_string(),
                available,
                proof: proof.to_string(),
            },
        )
        .await?;
    Ok(ModelCheckDto {
        provider: provider.id().to_string(),
        available,
    })
}

/// Record the person's name. The record holds it, and
/// shared memory gets a copy the agents read: a fact file plus its index
/// line, committed as the user with no run, and published as
/// `memory.committed` so it is the feed's first entry.
async fn seed_user_name(state: &AppState, tenant: &Tenant, name: &str) -> Result<(), ApiError> {
    use pagis_core::{MemoryChangeset, ScopedPath};

    state
        .users
        .set_name(&tenant.user_id, name, now_ms())
        .await?;

    let mut changeset = MemoryChangeset::default();
    changeset.writes.insert(
        ScopedPath::parse(crate::user::USER_FILE).expect("literal path is valid"),
        crate::user::render_user_file(name),
    );
    changeset.writes.insert(
        ScopedPath::parse("shared/MEMORY.md").expect("literal path is valid"),
        format!(
            "# Shared memory\n\n\
             This index has one line per fact file: `- [Title](path.md) — hook`.\n\n\
             - [User](user.md) — the user's name: {name}\n"
        ),
    );
    let message = "Onboarding: recorded the user's name";
    let memory_agent = pagis_core::AgentId::from(String::new());
    let access = pagis_core::MemoryAccess::Owner {
        agent_id: memory_agent.clone(),
    };
    let revision = state
        .memory
        .load_indexes(&tenant.workspace_id, &access)
        .await
        .map_err(crate::memory::memory_error)?
        .revision;
    let sha = state
        .memory
        .commit(
            &tenant.workspace_id,
            // The changeset touches only `shared/`; no agent scope.
            &memory_agent,
            &access,
            &crate::memory::user_author(),
            &changeset,
            revision.as_deref(),
            message,
            None,
        )
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "onboarding memory seed failed");
            ApiError::internal()
        })?;
    state
        .bus
        .publish(pagis_core::NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: "memory.committed".to_string(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({
                "sha": sha,
                "scopes": changeset.scopes(),
                "files": changeset.files(),
                "titles": changeset.titles(),
                "message": message,
                "source_scoped": false,
                "run_id": null,
            }),
        })
        .await?;
    Ok(())
}

/// Keep one provider's key and answer its status.
fn store_provider_key(
    state: &AppState,
    provider: &str,
    key: &str,
) -> Result<ProviderKeyDto, ApiError> {
    let provider = Provider::from_id(provider).ok_or_else(|| ApiError::not_found("provider"))?;
    if key.trim().is_empty() {
        return Err(ApiError::validation("key must not be empty"));
    }
    state.keys.set(provider, key.trim()).map_err(secret_error)?;
    crate::model_lists::key_changed(&state.models, provider);

    Ok(provider_statuses(&state.keys)?
        .into_iter()
        .find(|dto| dto.provider == provider.id())
        .expect("known provider has a status"))
}

// ---- Artifact retention ----

/// One class's retention window. `retain_days` of `null` keeps the
/// class for ever, which is the default (ADR-0020).
#[derive(Debug, Serialize, ToSchema)]
pub struct RetentionPolicyDto {
    /// `screenshot`, `call_recording`, `call_transcript`, or `file`.
    pub kind: String,
    pub retain_days: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RetentionPolicyPage {
    pub items: Vec<RetentionPolicyDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetRetentionPolicyRequest {
    /// Whole days to keep the class, or `null` to keep it for ever.
    pub retain_days: Option<i64>,
}

fn retention_dto(policy: pagis_core::RetentionPolicy) -> RetentionPolicyDto {
    RetentionPolicyDto {
        kind: policy.kind.to_string(),
        retain_days: policy.retain_days,
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/settings/retention",
    responses(
        (status = 200, body = RetentionPolicyPage),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_retention_policies(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<RetentionPolicyPage>, ApiError> {
    let items = state
        .retention_policies
        .list(&tenant.workspace_id)
        .await?
        .into_iter()
        .map(retention_dto)
        .collect();
    Ok(Json(RetentionPolicyPage { items }))
}

#[utoipa::path(
    put,
    path = "/api/v1/settings/retention/{kind}",
    params(("kind" = String, Path, description = "The artifact class")),
    request_body = SetRetentionPolicyRequest,
    responses(
        (status = 200, body = RetentionPolicyDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn set_retention_policy(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(kind): Path<String>,
    Json(request): Json<SetRetentionPolicyRequest>,
) -> Result<Json<RetentionPolicyDto>, ApiError> {
    let kind = pagis_core::ArtifactKind::parse(&kind)
        .ok_or_else(|| ApiError::not_found("artifact class"))?;
    if let Some(days) = request.retain_days
        && days < 1
    {
        return Err(ApiError::validation("retain_days must be at least 1"));
    }
    state
        .retention_policies
        .set(&tenant.workspace_id, kind, request.retain_days, now_ms())
        .await?;
    Ok(Json(retention_dto(pagis_core::RetentionPolicy {
        kind,
        retain_days: request.retain_days,
    })))
}
