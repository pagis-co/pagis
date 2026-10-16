//! Workspace Connections, Installation Connections and Google authorization.
//!
//! The rule is the same for every provider. What the installation sets
//! up one time for everybody is the Administration Interface's: the
//! Installation OAuth Client, a carrier account with its SIP sign-in,
//! and a mail domain (ADR-0024). What a person does with the provider
//! stays on the product port: a person connects their own Google
//! account, buys a Workspace Number or makes an Agent Mailbox.
//!
//! Each provider declares its installation parts in the Provider
//! Catalog ([`pagis_connect::installation_setups`]), and one set of
//! routes serves all of them: read every setup, configure one part,
//! test it, and remove it. The part's [`SetupKind`] says what each of
//! the three does, so a provider added to the catalog needs no route of
//! its own.
//!
//! An Installation Connection is a record of the Org's Workspace,
//! [`AppState::org_workspace_id`], as the installed Plugins are. No
//! person owns that Workspace, and every person reads the carrier and
//! the mail domain from it. The key behind a Connection is an
//! installation secret whose name carries no Workspace.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use pagis_connect::{InstallationSetup, SetupKind, SetupPart};
use pagis_core::{Connection, now_ms};

use crate::AppState;
use crate::auth::{Administrator, Tenant};
use crate::error::ApiError;
use crate::providers::{ConfigureSetupPartRequest, ProviderSetupDto, SetupFactDto, SetupPartDto};
use crate::settings::{
    AuthorizeConnectionRequest, AuthorizeConnectionResponse, ConnectionDto, ConnectionPage,
    CreateConnectionRequest, GoogleCallbackQuery, GoogleStartQuery, ProviderEntryDto,
    ProviderFieldDto, ProviderPage, publish_settings_event,
};

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

#[utoipa::path(get, tag = "settings", path = "/api/v1/settings/connections", responses(
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
/// The Google entry asks for nothing: the person signs in at Google.
/// It says whether the Org holds the Installation OAuth Client that the
/// sign-in needs, and whether the browser step can ask for a sign-in to
/// Pagis, which a local installation with Remote Access off never does.
#[utoipa::path(get, tag = "settings", path = "/api/v1/settings/connections/providers", responses(
    (status = 200, body = ProviderPage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_connection_providers(
    State(state): State<Arc<AppState>>,
    _tenant: Tenant,
) -> Result<Json<ProviderPage>, ApiError> {
    let google_client = state
        .native
        .connector
        .google()
        .web_client()
        .await
        .map_err(connect_error)?
        .is_some();
    let browser_sign_in = !crate::remote_access::serves_this_machine_only(&state);
    Ok(Json(ProviderPage {
        items: pagis_connect::person_catalog(google_client)
            .iter()
            .map(|provider| ProviderEntryDto::new(provider, browser_sign_in))
            .collect(),
    }))
}

/// Connect an account (ADR-0012). A Google record lands at
/// `disconnected` with no account; `authorize_connection` is the step
/// that sends the person to Google, where they pick the account.
#[utoipa::path(post, tag = "settings", path = "/api/v1/settings/connections",
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
    tenant: Tenant,
    Json(request): Json<CreateConnectionRequest>,
) -> Result<(StatusCode, Json<ConnectionDto>), ApiError> {
    if request.provider == "google" && !request.shared_account {
        tenant.require_private("personal Google Connections")?;
    }
    refuse_installation_provider(&request.provider)?;
    let mut credentials =
        pagis_connect::NewCredentials::from_fields(&request.provider, &request.fields)
            .map_err(connect_error)?;
    if !tenant.is_private() && request.provider == "google" {
        credentials = pagis_connect::NewCredentials::SharedGoogle;
    }
    let connection = state
        .native
        .connector
        .create(pagis_connect::NewConnection {
            workspace_id: tenant.workspace_id.clone(),
            alias: request.alias,
            display_name: request.display_name,
            credentials,
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
/// A Google Connection answers an `authorization_url` and returns at
/// once. The URL is the start route on the Public Origin,
/// which belongs to the Person who asks: the client opens it, the person
/// consents at Google in that browser, and the public callback route is
/// what makes the Connection `connected`. Every other Connection is
/// finished when this returns, and `authorization_url` is absent.
#[utoipa::path(post, tag = "settings", path = "/api/v1/settings/connections/{connection_id}/authorize",
    params(("connection_id" = String, Path)),
    request_body = AuthorizeConnectionRequest, responses(
        (status = 200, body = AuthorizeConnectionResponse),
        (status = 409, body = crate::error::ErrorBody),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn authorize_connection(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(connection_id): Path<String>,
    body: Option<Json<AuthorizeConnectionRequest>>,
) -> Result<Json<AuthorizeConnectionResponse>, ApiError> {
    let request = body.map(|Json(request)| request).unwrap_or_default();
    let id = pagis_core::ConnectionId::from(connection_id);
    refuse_installation_connection(&state, &id).await?;
    let connection = state
        .connections
        .get(&tenant.workspace_id, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("connection"))?;
    if connection.provider == "google" && connection.config["shared_account"] != true {
        tenant.require_private("personal Google Connections")?;
    }
    let answer = state
        .native
        .connector
        .authorize(
            &tenant.workspace_id,
            &id,
            &request.capabilities,
            request.api_key.as_deref(),
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
    /// `Domain`. A Server and a Local Installation in Remote Access are
    /// there, because Google takes a plain `http:` redirect URI only
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

/// Send one browser to Google for an authorization.
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
/// A local installation with Remote Access off has one Person and
/// answers only programs of its own machine. There the route asks the
/// browser for no Session: the Client App opens it in the system
/// browser, and the owner does not sign in a second time. The callback
/// keeps every check.
#[utoipa::path(get, tag = "settings", path = "/api/v1/connections/google/start",
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

    let tenant = match crate::remote_access::serves_this_machine_only(&state) {
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
    let answer = match state
        .native
        .connector
        .google()
        .start(&query.state, opener)
        .await
    {
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
        Err(pagis_connect::StartRefusal::NotManager) => (
            StatusCode::FORBIDDEN,
            google_page("A Manager authorizes a Shared Workspace account. Go back to Pagis."),
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

/// Finish one Google authorization.
///
/// Google redirects the person's browser here on the installation's own
/// public origin. The request carries no Session: the Session cookie is
/// `SameSite=Strict`, so a browser sends none on a cross-site
/// navigation. The route finishes only for the browser that holds the
/// transaction cookie of the `state`, which the start route set. The
/// `state` names the Connection, and the Person and the Session that
/// started the authorization: that Session must still be live. The first
/// consent records the Google account that consented, and a later
/// consent must come from that account. A redirect without the cookie spends the `state` and connects
/// nothing. Each answer clears the cookie.
#[utoipa::path(get, tag = "settings", path = "/api/v1/connections/google/callback",
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
    let google = state.native.connector.google();
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

#[utoipa::path(delete, tag = "settings", path = "/api/v1/settings/connections/{connection_id}",
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

/// The installation setup and the part one path names.
fn setup_part(provider: &str, part: &str) -> Result<(InstallationSetup, SetupPart), ApiError> {
    let setup = pagis_connect::installation_setup(provider)
        .ok_or_else(|| ApiError::not_found("provider setup"))?;
    let part = *setup
        .parts
        .iter()
        .find(|candidate| candidate.id == part)
        .ok_or_else(|| ApiError::not_found("provider setup"))?;
    Ok((setup, part))
}

/// The Installation Connection of one provider, whatever its status.
async fn installation_connection(
    state: &AppState,
    provider: &str,
) -> Result<Option<Connection>, ApiError> {
    Ok(state
        .connections
        .list(&state.org_workspace_id)
        .await?
        .into_iter()
        .find(|connection| connection.provider == provider))
}

/// The Installation Connection a part needs, refused when the
/// installation has not set it up.
async fn required_connection(state: &AppState, provider: &str) -> Result<Connection, ApiError> {
    installation_connection(state, provider)
        .await?
        .ok_or_else(|| ApiError::not_found("installation connection"))
}

fn fact(label: &str, value: impl Into<String>) -> SetupFactDto {
    SetupFactDto {
        label: label.to_string(),
        value: value.into(),
    }
}

/// Where one part stands now: whether it is set up, and the facts it
/// states.
async fn describe_part(
    state: &AppState,
    setup: &InstallationSetup,
    part: &SetupPart,
) -> Result<SetupPartDto, ApiError> {
    let mut configured = false;
    let mut status = None;
    let mut connection_id = None;
    let mut facts = Vec::new();
    match part.kind {
        SetupKind::ModelKey => return Err(ApiError::not_found("connection setup")),
        SetupKind::OauthClient => {
            let broker = state.native.connector.google();
            if let Some(client_id) = broker.registered_client_id().await.map_err(connect_error)? {
                configured = true;
                facts.push(fact("Client ID", client_id));
            }
            // Google matches the redirect URI character for character,
            // so the part states it before a client exists.
            facts.push(fact("Redirect URI", broker.redirect_uri()));
            facts.push(fact(
                "Scopes",
                pagis_google::oauth_scopes(pagis_google::GoogleCapability::ALL.iter().copied())
                    .join(" "),
            ));
        }
        SetupKind::Connection => {
            if let Some(connection) = installation_connection(state, setup.provider).await? {
                configured = true;
                connection_id = Some(connection.id.to_string());
                if let Some(account) = connection.config["account"]
                    .as_str()
                    .filter(|account| !account.is_empty())
                {
                    facts.push(fact("Account", account));
                }
                if let Some(mail) = pagis_mail::mailbox_provider(&connection) {
                    facts.push(fact("Domain", mail.domain));
                }
                status = Some(connection.status);
            }
        }
        SetupKind::SipCredential => {
            if let Some((username, domain)) = installation_connection(state, setup.provider)
                .await?
                .as_ref()
                .and_then(pagis_telephony::sip_identity)
            {
                configured = true;
                facts.push(fact("Sign-in", format!("{username}@{domain}")));
            }
        }
    }
    Ok(SetupPartDto {
        id: part.id.to_string(),
        kind: part.kind.as_str().to_string(),
        label: part.label.to_string(),
        blurb: part.blurb.to_string(),
        fields: part.fields.iter().map(ProviderFieldDto::from).collect(),
        configured,
        status,
        connection_id,
        testable: part.kind.testable(),
        facts,
    })
}

async fn describe(
    state: &AppState,
    setup: &InstallationSetup,
) -> Result<ProviderSetupDto, ApiError> {
    let mut parts = Vec::with_capacity(setup.parts.len());
    for part in setup.parts {
        parts.push(describe_part(state, setup, part).await?);
    }
    Ok(ProviderSetupDto {
        provider: setup.provider.to_string(),
        label: setup.label.to_string(),
        group: setup.group.to_string(),
        parts,
    })
}

/// One field's trimmed value, refused by name when it is empty.
fn required(fields: &BTreeMap<String, String>, key: &str) -> Result<String, ApiError> {
    fields
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ApiError::validation(format!("{key} is required")))
}

/// Tell every person the installation's Connections changed. Each
/// person's page shows the Org's carrier and mail domain, and it hears
/// only the events of its own Workspace.
async fn publish(state: &AppState, event_type: &str, payload: serde_json::Value) {
    let workspaces = match state.workspaces.list().await {
        Ok(workspaces) => workspaces,
        Err(error) => {
            tracing::error!(%error, "reading the workspaces for an installation connection event failed");
            return;
        }
    };
    for workspace in workspaces {
        if let Err(error) = state
            .bus
            .publish(pagis_core::NewEvent {
                workspace_id: workspace.id,
                event_type: event_type.to_string(),
                agent_id: None,
                run_id: None,
                channel_id: None,
                payload: payload.clone(),
            })
            .await
        {
            tracing::error!(%error, "publishing an installation connection event failed");
        }
    }
}

/// The installation's Connection setups. Provider Keys use the portable routes.
pub(crate) async fn installation_setups(
    state: &AppState,
) -> Result<Vec<ProviderSetupDto>, ApiError> {
    let mut items = Vec::new();
    for setup in pagis_connect::installation_setups()
        .into_iter()
        .filter(|setup| setup.group != "models")
    {
        items.push(describe(state, &setup).await?);
    }
    Ok(items)
}

#[utoipa::path(put, tag = "providers", path = "/api/v1/administration/providers/{provider}/{part}",
    params(
        ("provider" = String, Path, description = "A provider id, e.g. `anthropic` or `telnyx`"),
        ("part" = String, Path, description = "A part id, e.g. `key`, `oauth-client`, `connection` or `sip`"),
    ),
    request_body = ConfigureSetupPartRequest,
    responses(
        (status = 200, body = ProviderSetupDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// Set up one part. A Connection part that exists takes a new secret
/// and proves it again; its other fields stay, and removing the part is
/// how they change.
pub async fn configure_provider_part(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Path((provider, part)): Path<(String, String)>,
    Json(request): Json<ConfigureSetupPartRequest>,
) -> Result<Json<ProviderSetupDto>, ApiError> {
    let (setup, part) = setup_part(&provider, &part)?;
    let fields = request.fields;
    match part.kind {
        SetupKind::ModelKey => return Err(ApiError::not_found("connection setup")),
        SetupKind::OauthClient => {
            state
                .native
                .connector
                .google()
                .register(
                    &required(&fields, "client_id")?,
                    &required(&fields, "client_secret")?,
                )
                .await
                .map_err(connect_error)?;
        }
        SetupKind::Connection => match installation_connection(&state, setup.provider).await? {
            Some(connection) => {
                let secret = part
                    .fields
                    .iter()
                    .find(|field| field.secret)
                    .map(|field| required(&fields, field.key))
                    .transpose()?;
                let connection = state
                    .native
                    .connector
                    .authorize(
                        &state.org_workspace_id,
                        &connection.id,
                        &[],
                        secret.as_deref(),
                        &initiator(&administrator),
                    )
                    .await
                    .map_err(connect_error)?
                    .into_connection();
                publish(
                    &state,
                    "connection.changed",
                    serde_json::json!({
                        "connection_id": connection.id.as_str(),
                        "status": connection.status.as_str(),
                    }),
                )
                .await;
            }
            None => {
                let entry = pagis_connect::entry(setup.provider)
                    .ok_or_else(|| ApiError::not_found("provider setup"))?;
                let credentials = pagis_connect::NewCredentials::from_fields(entry.id, &fields)
                    .map_err(connect_error)?;
                let connection = state
                    .native
                    .connector
                    .create(pagis_connect::NewConnection {
                        workspace_id: state.org_workspace_id.clone(),
                        alias: entry.default_alias.to_string(),
                        display_name: entry.default_display_name.to_string(),
                        credentials,
                    })
                    .await
                    .map_err(connect_error)?;
                publish(
                    &state,
                    "connection.created",
                    serde_json::json!({
                        "connection_id": connection.id.as_str(),
                        "alias": connection.alias.as_str(),
                        "provider": connection.provider.as_str(),
                    }),
                )
                .await;
                // A new carrier gets its line at once. With no SIP
                // credential the line asks the carrier nothing, and each
                // held number says that the sign-in is missing.
                if pagis_telephony::is_carrier(&connection.provider)
                    && let Err(error) = state.native.numbers.start_carrier().await
                {
                    tracing::error!(%error, "starting the carrier's line failed");
                }
            }
        },
        SetupKind::SipCredential => {
            let connection = installation_connection(&state, setup.provider)
                .await?
                .ok_or_else(|| {
                    ApiError::conflict("set up the carrier account before its SIP sign-in")
                })?;
            state
                .native
                .connector
                .set_sip_credential(
                    &state.org_workspace_id,
                    &connection.id,
                    fields
                        .get("username")
                        .map(String::as_str)
                        .unwrap_or_default(),
                    fields
                        .get("password")
                        .map(String::as_str)
                        .unwrap_or_default(),
                    fields.get("domain").map(String::as_str).unwrap_or_default(),
                )
                .await
                .map_err(connect_error)?;
            state
                .native
                .numbers
                .sip_credential_changed(&connection.id)
                .await;
        }
    }
    describe(&state, &setup).await.map(Json)
}

#[utoipa::path(post, tag = "providers", path = "/api/v1/administration/providers/{provider}/{part}/test",
    params(
        ("provider" = String, Path, description = "A provider id, e.g. `telnyx`"),
        ("part" = String, Path, description = "A part id, e.g. `connection`"),
    ),
    responses(
        (status = 200, body = ProviderSetupDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
/// Prove one part's kept credential at the provider again. A key the
/// provider refuses leaves the Connection `reauth_required` or
/// `unavailable`, and the answer says why.
pub async fn test_provider_part(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Path((provider, part)): Path<(String, String)>,
) -> Result<Json<ProviderSetupDto>, ApiError> {
    let (setup, part) = setup_part(&provider, &part)?;
    if !part.kind.testable() {
        return Err(ApiError::validation(format!(
            "{} proves itself when it is used, so there is nothing to test here",
            part.label
        )));
    }
    let connection = required_connection(&state, setup.provider).await?;
    let connection = state
        .native
        .connector
        .authorize(
            &state.org_workspace_id,
            &connection.id,
            &[],
            None,
            &initiator(&administrator),
        )
        .await
        .map_err(connect_error)?
        .into_connection();
    publish(
        &state,
        "connection.changed",
        serde_json::json!({
            "connection_id": connection.id.as_str(),
            "status": connection.status.as_str(),
        }),
    )
    .await;
    describe(&state, &setup).await.map(Json)
}

#[utoipa::path(delete, tag = "providers", path = "/api/v1/administration/providers/{provider}/{part}",
    params(
        ("provider" = String, Path, description = "A provider id, e.g. `anthropic` or `telnyx`"),
        ("part" = String, Path, description = "A part id, e.g. `key` or `connection`"),
    ),
    responses(
        (status = 200, body = ProviderSetupDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    )
)]
/// Remove one part. A carrier that carries a number and a mail domain
/// that holds a mailbox stay, and the refusal says what to do first.
/// Removing the OAuth client leaves every Google connection
/// unable to refresh, so each person consents again against the next
/// client (ADR-0012).
pub async fn remove_provider_part(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Path((provider, part)): Path<(String, String)>,
) -> Result<Json<ProviderSetupDto>, ApiError> {
    let (setup, part) = setup_part(&provider, &part)?;
    match part.kind {
        SetupKind::ModelKey => return Err(ApiError::not_found("connection setup")),
        SetupKind::OauthClient => {
            state
                .native
                .connector
                .google()
                .forget()
                .await
                .map_err(connect_error)?;
        }
        SetupKind::Connection => {
            let connection = required_connection(&state, setup.provider).await?;
            remove_connection(&state, &connection).await?;
        }
        SetupKind::SipCredential => {
            let connection = required_connection(&state, setup.provider).await?;
            state
                .native
                .connector
                .clear_sip_credential(&state.org_workspace_id, &connection.id)
                .await
                .map_err(connect_error)?;
            state
                .native
                .numbers
                .sip_credential_changed(&connection.id)
                .await;
        }
    }
    describe(&state, &setup).await.map(Json)
}

/// Delete one Installation Connection, refused while a number or a
/// mailbox of any person still points at it.
async fn remove_connection(state: &AppState, connection: &Connection) -> Result<(), ApiError> {
    let mut numbers = false;
    let mut mailboxes = 0;
    for workspace in state.workspaces.list().await? {
        {
            numbers |= state
                .native
                .numbers
                .connection_in_use(&workspace.id, &connection.id)
                .await
                .map_err(crate::phone_numbers::number_error)?;
        }
        mailboxes += state
            .mailbox_desk
            .connection_in_use(&workspace.id, &connection.id)
            .await
            .map_err(crate::mailboxes::mailbox_error)?;
    }
    // A carrier that still carries a number stays (ADR-0018).
    if numbers {
        return Err(ApiError::conflict(
            "this carrier still carries a phone number. Release the numbers first.",
        ));
    }
    // A mail domain that a mailbox still points at stays (ADR-0019).
    // The count tells the administrator how many are left.
    if mailboxes > 0 {
        let mailbox = if mailboxes == 1 {
            "mailbox"
        } else {
            "mailboxes"
        };
        return Err(ApiError::conflict(format!(
            "this mail domain still holds {mailboxes} {mailbox}. Remove them first."
        )));
    }
    if !state
        .connections
        .delete_and_revoke(&state.org_workspace_id, &connection.id, now_ms())
        .await?
    {
        return Err(ApiError::not_found("installation connection"));
    }
    // The SIP credential goes with the carrier, so the carrier's line
    // stops.
    if pagis_telephony::is_carrier(&connection.provider) {
        state
            .native
            .numbers
            .sip_credential_changed(&connection.id)
            .await;
    }
    publish(
        state,
        "connection.deleted",
        serde_json::json!({ "connection_id": connection.id.as_str() }),
    )
    .await;
    Ok(())
}
