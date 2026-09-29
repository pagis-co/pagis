//! The Agent Mailbox surface (ADR-0019): provision,
//! read, reset the password and delete.
//!
//! The mailbox belongs to one Agent, so every route hangs off that
//! Agent. Nothing here returns the mailbox password, and nothing here
//! deletes a mailbox without the address typed back: the host's mail
//! goes with the mailbox and Pagis keeps no copy.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use pagis_core::{AgentId, AgentMailbox, ConnectionId, day_start, now_ms};
use pagis_mail::{MailboxError, MailboxOffer, NewMailbox};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// One Agent Mailbox as the desk shows it. The password is never part
/// of it (ADR-0013).
#[derive(Debug, Serialize, ToSchema)]
pub struct MailboxDto {
    pub id: String,
    pub agent_id: String,
    /// The address itself. It never changes.
    pub address: String,
    /// `provisioning`, `active`, `unavailable`, `dormant` or `deleted`.
    pub state: String,
    /// Why the mailbox is `unavailable`, in the words the user reads.
    pub reason: Option<String>,
    /// The Mailbox Provider Connection that carries it.
    pub connection_id: String,
    /// The most messages a day this mailbox may send.
    pub outgoing_cap: u32,
    /// How many it has sent today.
    pub sends_today: u32,
    /// The Mail Recipient Domain allow rules the user wrote from a
    /// send card (ADR-0019). A send whose recipients all sit on these
    /// registrable domains needs no card.
    pub allow_rules: Vec<String>,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

pub fn mailbox_dto(mailbox: AgentMailbox) -> MailboxDto {
    let sends_today = mailbox.sends_on(day_start(now_ms()));
    MailboxDto {
        id: mailbox.id.to_string(),
        agent_id: mailbox.agent_id.to_string(),
        address: mailbox.address,
        state: mailbox.state.as_str().to_string(),
        reason: mailbox.reason,
        connection_id: mailbox.connection_id.to_string(),
        outgoing_cap: mailbox.outgoing_cap,
        sends_today,
        allow_rules: mailbox.allow_rules,
        created_at: mailbox.created_at,
        deleted_at: mailbox.deleted_at,
    }
}

/// One Mailbox Provider the form can offer, and the name this Agent
/// would take on it (ADR-0019).
#[derive(Debug, Serialize, ToSchema)]
pub struct MailboxOfferDto {
    pub connection_id: String,
    pub display_name: String,
    pub domain: String,
    /// The local part the form fills in, with the ledger collision
    /// suffix on it when the plain name is taken. Empty when the
    /// Agent's name carries nothing an address can hold.
    pub suggested_local_part: String,
    /// The messages a day a new mailbox gets unless the user says
    /// otherwise.
    pub default_outgoing_cap: u32,
    /// Whether the host mints the mailbox password. Where it does not,
    /// the user types the password of an inbox made at the host.
    pub mints_password: bool,
    /// Whether the host deletes the mailbox. Where it does not, the
    /// user deletes it at the host afterwards.
    pub deletes_mailbox: bool,
}

/// The Agent's mailbox, and what it could have instead when it has
/// none. One read serves the creation form and the mailbox card.
#[derive(Debug, Serialize, ToSchema)]
pub struct MailboxPage {
    pub mailbox: Option<MailboxDto>,
    /// The Mailbox Providers that are `connected` now.
    pub offers: Vec<MailboxOfferDto>,
}

/// The mailbox section of the Agent creation form, and the body of a
/// later provision (ADR-0019).
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct NewMailboxRequest {
    /// The Mailbox Provider Connection to make it on.
    pub connection_id: String,
    /// The name before the `@`, on that Connection's domain.
    pub local_part: String,
    /// The messages a day; absent takes the default.
    #[serde(default)]
    pub outgoing_cap: Option<u32>,
    /// The password of an inbox the user made at a manual host. A host
    /// with an API mints its own and refuses this.
    #[serde(default)]
    pub password: Option<String>,
}

impl From<NewMailboxRequest> for NewMailbox {
    fn from(request: NewMailboxRequest) -> Self {
        NewMailbox {
            connection_id: pagis_core::ConnectionId::from(request.connection_id),
            local_part: request.local_part,
            outgoing_cap: request.outgoing_cap,
            password: request.password,
        }
    }
}

/// The new password, when the host does not mint one itself.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ResetMailboxPasswordRequest {
    #[serde(default)]
    pub password: Option<String>,
}

/// The address, typed back. A delete takes the host's mail with it, so
/// the user says which mailbox in full.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DeleteMailboxRequest {
    pub confirm_address: String,
}

/// What the delete did. `notice` carries the words a manual host asks
/// the desk to show, because that mailbox is still at the host.
#[derive(Debug, Serialize, ToSchema)]
pub struct DeletedMailboxDto {
    pub mailbox: MailboxDto,
    pub notice: Option<String>,
}

/// Every mailbox of the Workspace that is not a tombstone. The
/// Connections page reads it to say which mailboxes sit on a Mailbox
/// Provider, and which Agent holds each one (ADR-0019).
#[derive(Debug, Serialize, ToSchema)]
pub struct MailboxListPage {
    pub items: Vec<MailboxDto>,
}

/// What the mailbox section of the Agent creation form offers. The
/// Agent does not exist yet, so the name it would take comes from the
/// name in the form.
#[derive(Debug, Deserialize, IntoParams)]
pub struct MailboxOffersQuery {
    /// The name typed in the creation form. An empty name suggests no
    /// local part.
    #[serde(default)]
    pub agent_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MailboxOffersPage {
    pub offers: Vec<MailboxOfferDto>,
}

/// One local part, read against one Mailbox Provider.
#[derive(Debug, Deserialize, IntoParams)]
pub struct MailboxNameQuery {
    pub connection_id: String,
    pub local_part: String,
}

/// What the form says under the name field as the user types
/// (ADR-0019). It is a read: it reserves nothing, and the Address
/// Ledger stays the one place that refuses a taken address.
#[derive(Debug, Serialize, ToSchema)]
pub struct MailboxNameDto {
    /// The whole address the name makes on that provider's domain.
    pub address: String,
    /// Whether the address is free and the name is allowed.
    pub available: bool,
    /// Why it is not, in the words the user reads.
    pub reason: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/settings/mailboxes", responses(
    (status = 200, body = MailboxListPage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_mailboxes(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<MailboxListPage>, ApiError> {
    let items = state
        .mailbox_desk
        .list(&tenant.workspace_id)
        .await
        .map_err(mailbox_error)?
        .into_iter()
        .map(mailbox_dto)
        .collect();
    Ok(Json(MailboxListPage { items }))
}

/// The Mailbox Providers a new Agent could take a mailbox on. The
/// Agent's own route answers the same for an Agent that exists.
#[utoipa::path(get, path = "/api/v1/settings/mailbox-offers",
    params(MailboxOffersQuery), responses(
        (status = 200, body = MailboxOffersPage),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_mailbox_offers(
    State(state): State<Arc<AppState>>,
    _tenant: Tenant,
    Query(query): Query<MailboxOffersQuery>,
) -> Result<Json<MailboxOffersPage>, ApiError> {
    let offers = mailbox_offers(&state, &query.agent_name).await?;
    Ok(Json(MailboxOffersPage { offers }))
}

/// Read one local part against the Address Ledger as the user types
/// (ADR-0019). The answer is advice: the ledger still refuses a taken
/// address when the mailbox is made.
#[utoipa::path(get, path = "/api/v1/settings/mailbox-offers/name",
    params(MailboxNameQuery), responses(
        (status = 200, body = MailboxNameDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn check_mailbox_name(
    State(state): State<Arc<AppState>>,
    _tenant: Tenant,
    Query(query): Query<MailboxNameQuery>,
) -> Result<Json<MailboxNameDto>, ApiError> {
    let connection_id = ConnectionId::from(query.connection_id);
    let offer = mailbox_offer(&state, &connection_id).await?;
    let domain = offer.settings.domain;
    let local_part = match pagis_mail::validate_local_part(&query.local_part) {
        Ok(local_part) => local_part,
        Err(error) => {
            return Ok(Json(MailboxNameDto {
                address: pagis_mail::mailbox_address(&query.local_part, &domain),
                available: false,
                reason: Some(error.to_string()),
            }));
        }
    };
    let address = pagis_mail::mailbox_address(&local_part, &domain);
    let taken = state
        .mailbox_desk
        .address_taken(&address)
        .await
        .map_err(mailbox_error)?;
    Ok(Json(MailboxNameDto {
        available: !taken,
        reason: taken.then(|| format!("{address} is in the address ledger.")),
        address,
    }))
}

#[utoipa::path(get, path = "/api/v1/agents/{agent_id}/mailbox",
    params(("agent_id" = String, Path)), responses(
        (status = 200, body = MailboxPage),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn get_mailbox(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
) -> Result<Json<MailboxPage>, ApiError> {
    let agent = agent_of(&state, &tenant, &AgentId::from(agent_id)).await?;
    let mailbox = state
        .mailbox_desk
        .for_agent(&tenant.workspace_id, &agent.id)
        .await
        .map_err(mailbox_error)?;
    let offers = match mailbox.is_some() {
        // A mailbox is held: there is nothing to offer.
        true => Vec::new(),
        false => mailbox_offers(&state, &agent.name).await?,
    };
    Ok(Json(MailboxPage {
        mailbox: mailbox.map(mailbox_dto),
        offers,
    }))
}

/// Make the Agent's mailbox. The answer arrives once the host has made
/// it; the login is proven afterwards, so the mailbox lands
/// `provisioning` and reaches `active` or `unavailable` on its own
/// (ADR-0019).
#[utoipa::path(post, path = "/api/v1/agents/{agent_id}/mailbox",
    params(("agent_id" = String, Path)),
    request_body = NewMailboxRequest, responses(
        (status = 201, body = MailboxDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn provision_mailbox(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
    Json(request): Json<NewMailboxRequest>,
) -> Result<(StatusCode, Json<MailboxDto>), ApiError> {
    let agent = agent_of(&state, &tenant, &AgentId::from(agent_id)).await?;
    let mailbox = state
        .mailbox_desk
        .provision(&tenant.workspace_id, &agent.id, request.into())
        .await
        .map_err(mailbox_error)?;
    Ok((StatusCode::CREATED, Json(mailbox_dto(mailbox))))
}

/// Give the mailbox a new password and prove the login again. The
/// cursor does not move, so nothing the mailbox received while it was
/// unavailable is lost.
#[utoipa::path(post, path = "/api/v1/agents/{agent_id}/mailbox/reset-password",
    params(("agent_id" = String, Path)),
    request_body = ResetMailboxPasswordRequest, responses(
        (status = 200, body = MailboxDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn reset_mailbox_password(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
    Json(request): Json<ResetMailboxPasswordRequest>,
) -> Result<Json<MailboxDto>, ApiError> {
    let agent = agent_of(&state, &tenant, &AgentId::from(agent_id)).await?;
    let mailbox = held_mailbox(&state, &tenant, &agent.id).await?;
    let reset = state
        .mailbox_desk
        .reset_password(&tenant.workspace_id, &mailbox.id, request.password)
        .await
        .map_err(mailbox_error)?;
    Ok(Json(mailbox_dto(reset)))
}

/// Delete the mailbox. The user types the address to confirm it,
/// because the host's mail goes with the mailbox and Pagis keeps no
/// copy of a message body (ADR-0019).
#[utoipa::path(delete, path = "/api/v1/agents/{agent_id}/mailbox",
    params(("agent_id" = String, Path)),
    request_body = DeleteMailboxRequest, responses(
        (status = 200, body = DeletedMailboxDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn delete_mailbox(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(agent_id): Path<String>,
    Json(request): Json<DeleteMailboxRequest>,
) -> Result<Json<DeletedMailboxDto>, ApiError> {
    let agent = agent_of(&state, &tenant, &AgentId::from(agent_id)).await?;
    let mailbox = held_mailbox(&state, &tenant, &agent.id).await?;
    let deleted = state
        .mailbox_desk
        .delete(&tenant.workspace_id, &mailbox.id, &request.confirm_address)
        .await
        .map_err(mailbox_error)?;
    Ok(Json(DeletedMailboxDto {
        mailbox: mailbox_dto(deleted.mailbox),
        notice: deleted.notice,
    }))
}

/// The Mailbox Providers that are `connected`, each with the free name
/// the given Agent name would take. The creation form and the mailbox
/// card read the same list.
async fn mailbox_offers(
    state: &AppState,
    agent_name: &str,
) -> Result<Vec<MailboxOfferDto>, ApiError> {
    Ok(state
        .mailbox_desk
        .offers(agent_name)
        .await
        .map_err(mailbox_error)?
        .into_iter()
        .map(offer_dto)
        .collect())
}

/// One of those offers, by its Connection.
async fn mailbox_offer(
    state: &AppState,
    connection_id: &ConnectionId,
) -> Result<MailboxOffer, ApiError> {
    state
        .mailbox_desk
        .offers("")
        .await
        .map_err(mailbox_error)?
        .into_iter()
        .find(|offer| &offer.connection.id == connection_id)
        .ok_or_else(|| ApiError::not_found("mailbox provider"))
}

fn offer_dto(offer: MailboxOffer) -> MailboxOfferDto {
    MailboxOfferDto {
        connection_id: offer.connection.id.to_string(),
        display_name: offer.connection.display_name,
        domain: offer.settings.domain,
        suggested_local_part: offer.suggested_local_part,
        default_outgoing_cap: pagis_mail::DEFAULT_OUTGOING_CAP,
        mints_password: offer.settings.capabilities.reset_password,
        deletes_mailbox: offer.settings.capabilities.delete_mailbox,
    }
}

/// One Agent of this Workspace.
async fn agent_of(
    state: &AppState,
    tenant: &Tenant,
    agent_id: &AgentId,
) -> Result<pagis_core::Agent, ApiError> {
    state
        .agent_store
        .get(&tenant.workspace_id, agent_id)
        .await?
        .ok_or_else(|| ApiError::not_found("agent"))
}

/// The mailbox the Agent holds now. An Agent that holds none has
/// nothing to reset or delete.
async fn held_mailbox(
    state: &AppState,
    tenant: &Tenant,
    agent_id: &AgentId,
) -> Result<AgentMailbox, ApiError> {
    state
        .mailbox_desk
        .for_agent(&tenant.workspace_id, agent_id)
        .await
        .map_err(mailbox_error)?
        .ok_or_else(|| ApiError::not_found("agent mailbox"))
}

/// A refusal the user can act on keeps its words. A host failure
/// reports the stable code and nothing from upstream.
pub fn mailbox_error(error: MailboxError) -> ApiError {
    match error {
        MailboxError::Store(error) => ApiError::from(error),
        MailboxError::Validation(message) => ApiError::validation(message),
        MailboxError::Conflict(message) => ApiError::conflict(message),
        MailboxError::NotFound => ApiError::not_found("agent mailbox"),
        MailboxError::NoProvider => {
            ApiError::conflict("connect a mailbox provider before you make a mailbox".to_string())
        }
        MailboxError::ProviderUnavailable(status) => {
            ApiError::conflict(format!("the mailbox provider is {status}"))
        }
        MailboxError::Host(error) => ApiError::validation(pagis_mail::host_message(&error)),
        MailboxError::Transport(error) => ApiError::conflict(error.to_string()),
        MailboxError::StandingRule(error) => ApiError::conflict(error.to_string()),
    }
}
