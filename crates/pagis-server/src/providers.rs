//! The installation's setup of each provider, on the administration
//! port (ADR-0024).
//!
//! The rule is the same for every provider. What the installation sets
//! up one time for everybody is the Administration Interface's: a model
//! provider's key, the Installation OAuth Client, a carrier account
//! with its SIP sign-in, and a mail domain. What a person does with the
//! provider stays on the product port: a person connects their own
//! Google account, buys an Agent Phone Number or makes an Agent
//! Mailbox.
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
use axum::extract::{Path, State};
use pagis_connect::{InstallationSetup, SetupKind, SetupPart};
use pagis_core::{Connection, Provider, now_ms};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Administrator;
use crate::error::ApiError;
use crate::settings::{ProviderFieldDto, connect_error, secret_error};

/// Where the connect flow of an Installation Connection says its request
/// came from. An Installation Connection is a carrier account or a mail
/// domain, and neither runs a consent on the daemon host, so the answer
/// is the one that starts no such consent.
const INSTALLATION_SETUP: pagis_connect::RequestSource = pagis_connect::RequestSource::Elsewhere;

/// One value a part states about what the installation holds, or what
/// the administrator copies into the provider's console. It is never a
/// secret.
#[derive(Debug, Serialize, ToSchema)]
pub struct SetupFactDto {
    pub label: String,
    pub value: String,
}

/// One part of a provider that the installation sets up, and where it
/// stands.
#[derive(Debug, Serialize, ToSchema)]
pub struct SetupPartDto {
    /// The path segment the routes name the part by.
    pub id: String,
    /// `connection`, `sip_credential`, `oauth_client` or `model_key`.
    pub kind: String,
    pub label: String,
    /// What the administrator reads before they type.
    pub blurb: String,
    /// What the form asks for. A secret field is masked and never read
    /// back.
    pub fields: Vec<ProviderFieldDto>,
    pub configured: bool,
    /// The Connection's status, for a Connection part that exists:
    /// `connected`, `unavailable` or `reauth_required`.
    pub status: Option<String>,
    /// The Installation Connection this part is, once it exists.
    pub connection_id: Option<String>,
    /// Whether a test proves the kept credential at the provider.
    pub testable: bool,
    pub facts: Vec<SetupFactDto>,
}

/// One provider as the installation sets it up.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderSetupDto {
    /// The catalog id or the model provider id.
    pub provider: String,
    pub label: String,
    /// `models`, `accounts`, `telephony` or `mailboxes`.
    pub group: String,
    pub parts: Vec<SetupPartDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderSetupPage {
    pub items: Vec<ProviderSetupDto>,
}

/// The values of one part's fields, by field key. A secret field passes
/// to the provider or the secret store and is never written to the
/// database. The type carries no `Debug`, so none of them reaches a log
/// line.
#[derive(Deserialize, ToSchema)]
pub struct ConfigureSetupPartRequest {
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
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
        SetupKind::ModelKey => {
            let provider = model_provider(setup.provider)?;
            let source = state
                .keys
                .status()
                .map_err(secret_error)?
                .into_iter()
                .find(|key| key.provider == provider)
                .and_then(|key| key.source);
            if let Some(source) = source {
                configured = true;
                facts.push(fact("Source", source.label(provider)));
            }
        }
        SetupKind::OauthClient => {
            let broker = state.connector.google();
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

fn model_provider(id: &str) -> Result<Provider, ApiError> {
    Provider::from_id(id).ok_or_else(|| ApiError::not_found("provider setup"))
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

#[utoipa::path(get, path = "/api/v1/administration/providers", responses(
    (status = 200, body = ProviderSetupPage),
    (status = 401, body = crate::error::ErrorBody),
    (status = 403, body = crate::error::ErrorBody),
))]
/// Every provider the installation sets up, with where each part
/// stands.
pub async fn list_provider_setups(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<ProviderSetupPage>, ApiError> {
    let mut items = Vec::new();
    for setup in pagis_connect::installation_setups() {
        items.push(describe(&state, &setup).await?);
    }
    Ok(Json(ProviderSetupPage { items }))
}

#[utoipa::path(put, path = "/api/v1/administration/providers/{provider}/{part}",
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
        SetupKind::ModelKey => {
            let key = required(&fields, "api_key")?;
            let provider = model_provider(setup.provider)?;
            state.keys.set(provider, &key).map_err(secret_error)?;
            crate::model_lists::key_changed(&state.models, provider);
            crate::model_lists::route_unrouted_workspaces(&state).await?;
        }
        SetupKind::OauthClient => {
            state
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
                    .connector
                    .authorize(
                        &state.org_workspace_id,
                        &connection.id,
                        &[],
                        secret.as_deref(),
                        INSTALLATION_SETUP,
                        &crate::settings::initiator(&administrator),
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
                    .connector
                    .create(pagis_connect::NewConnection {
                        workspace_id: state.org_workspace_id.clone(),
                        alias: entry.default_alias.to_string(),
                        display_name: entry.default_display_name.to_string(),
                        credentials,
                        source: INSTALLATION_SETUP,
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
                    && let Err(error) = state.numbers.start_carrier().await
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
            state.numbers.sip_credential_changed(&connection.id).await;
        }
    }
    describe(&state, &setup).await.map(Json)
}

#[utoipa::path(post, path = "/api/v1/administration/providers/{provider}/{part}/test",
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
        .connector
        .authorize(
            &state.org_workspace_id,
            &connection.id,
            &[],
            None,
            INSTALLATION_SETUP,
            &crate::settings::initiator(&administrator),
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

#[utoipa::path(delete, path = "/api/v1/administration/providers/{provider}/{part}",
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
/// Removing the OAuth client leaves every brokered Google connection
/// unable to refresh, so each person consents again against the next
/// client (ADR-0012).
pub async fn remove_provider_part(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Path((provider, part)): Path<(String, String)>,
) -> Result<Json<ProviderSetupDto>, ApiError> {
    let (setup, part) = setup_part(&provider, &part)?;
    match part.kind {
        SetupKind::ModelKey => {
            let provider = model_provider(setup.provider)?;
            state.keys.remove(provider).map_err(secret_error)?;
            state.models.forget(provider);
            crate::model_lists::route_unrouted_workspaces(&state).await?;
        }
        SetupKind::OauthClient => {
            state
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
                .connector
                .clear_sip_credential(&state.org_workspace_id, &connection.id)
                .await
                .map_err(connect_error)?;
            state.numbers.sip_credential_changed(&connection.id).await;
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
        numbers |= state
            .numbers
            .connection_in_use(&workspace.id, &connection.id)
            .await
            .map_err(crate::phone_numbers::number_error)?;
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
            "this mail domain still holds {mailboxes} agent {mailbox}. Delete them first."
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
        state.numbers.sip_credential_changed(&connection.id).await;
    }
    publish(
        state,
        "connection.deleted",
        serde_json::json!({ "connection_id": connection.id.as_str() }),
    )
    .await;
    Ok(())
}
