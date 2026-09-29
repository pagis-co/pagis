//! The Agent Phone Number surface (ADR-0018): search, buy,
//! assign, unassign and release.
//!
//! The user works on the Agent's own Settings page, so `agent_id`
//! travels with a purchase and an assignment. Nothing here returns the
//! carrier handle or the API key: the API says `e164` and no more.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use pagis_core::{AgentId, PhoneNumberId};
use pagis_telephony::NumberError;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// One number the Workspace owns. `provider_number_id` is not here and
/// never will be: it is the carrier's handle, not the user's.
#[derive(Debug, Serialize, ToSchema)]
pub struct PhoneNumberDto {
    pub id: String,
    /// The number itself, in E.164.
    pub e164: String,
    /// `assigned`, `unassigned` or `released`.
    pub status: String,
    /// The Agent that holds the line, when one does.
    pub agent_id: Option<String>,
    /// The carrier Connection that carries the number.
    pub connection_id: String,
    /// Where the line of the number's carrier stands with the
    /// registrar: `unregistered`, `registering`, `registered` or
    /// `failed`. One line carries every number of the carrier. Absent
    /// when no Agent holds the number, because a call to it is refused
    /// whatever the line does. An unregistered line drops inbound calls
    /// in silence, so the Agent's page shows this.
    pub registration: Option<String>,
    /// Why the registration failed: `no_credential`, `unauthorized`,
    /// `unreachable` or `refused`.
    pub registration_failure: Option<String>,
    pub created_at: i64,
    pub assigned_at: Option<i64>,
}

/// The record plus the live state of its carrier's line, which the
/// desk holds and the record does not (ADR-0018).
fn phone_number_dto(state: &AppState, number: pagis_core::PhoneNumber) -> PhoneNumberDto {
    let registration = state.numbers.registration(&number);
    PhoneNumberDto {
        id: number.id.to_string(),
        e164: number.e164,
        status: number.status.as_str().to_string(),
        agent_id: number.agent_id.map(|agent_id| agent_id.to_string()),
        connection_id: number.connection_id.to_string(),
        registration: registration.map(|state| state.as_str().to_string()),
        registration_failure: registration
            .and_then(|state| state.failure())
            .map(|failure| failure.as_str().to_string()),
        created_at: number.created_at,
        assigned_at: number.assigned_at,
    }
}

/// The numbers of the Workspace, and the carrier they came from. The
/// carrier's status decides whether Settings shows the search or a
/// reconnect action in its place.
#[derive(Debug, Serialize, ToSchema)]
pub struct PhoneNumberPage {
    pub items: Vec<PhoneNumberDto>,
    pub carrier: Option<CarrierDto>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CarrierDto {
    pub connection_id: String,
    pub display_name: String,
    /// `disconnected`, `connecting`, `connected` or `reauth_required`.
    pub status: String,
}

/// One number the carrier offers now, with today's price. Pagis stores
/// no price: this is read at search time and shown, never kept.
#[derive(Debug, Serialize, ToSchema)]
pub struct AvailableNumberDto {
    pub e164: String,
    pub region: Option<String>,
    pub monthly_cost: Option<String>,
    pub currency: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AvailableNumberPage {
    pub items: Vec<AvailableNumberDto>,
}

/// A country, and an area code or a locality (ADR-0018).
#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchNumbersQuery {
    /// The two-letter country, e.g. `US`.
    pub country: String,
    pub area_code: Option<String>,
    pub locality: Option<String>,
}

/// Buy one number, and give it to an Agent when the user bought it from
/// that Agent's page.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BuyNumberRequest {
    pub e164: String,
    pub agent_id: Option<String>,
}

/// Adopt a number the carrier account already holds, and give
/// it to an Agent when the user added it from that Agent's page.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AdoptNumberRequest {
    pub e164: String,
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AssignNumberRequest {
    pub agent_id: String,
}

#[utoipa::path(get, path = "/api/v1/settings/phone-numbers", responses(
    (status = 200, body = PhoneNumberPage),
    (status = 401, body = crate::error::ErrorBody),
))]
pub async fn list_phone_numbers(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<PhoneNumberPage>, ApiError> {
    let items = state
        .numbers
        .list(&tenant.workspace_id)
        .await
        .map_err(number_error)?
        .into_iter()
        .map(|number| phone_number_dto(&state, number))
        .collect();
    let carrier = state
        .numbers
        .carrier()
        .await
        .map_err(number_error)?
        .map(|connection| CarrierDto {
            connection_id: connection.id.to_string(),
            display_name: connection.display_name,
            status: connection.status,
        });
    Ok(Json(PhoneNumberPage { items, carrier }))
}

/// The carrier's own list, read live. A search needs the carrier
/// Connection at `connected`.
#[utoipa::path(get, path = "/api/v1/settings/phone-numbers/available",
    params(SearchNumbersQuery), responses(
        (status = 200, body = AvailableNumberPage),
        (status = 401, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn search_phone_numbers(
    State(state): State<Arc<AppState>>,
    _tenant: Tenant,
    Query(query): Query<SearchNumbersQuery>,
) -> Result<Json<AvailableNumberPage>, ApiError> {
    let items = state
        .numbers
        .search(
            &query.country,
            query.area_code.as_deref(),
            query.locality.as_deref(),
        )
        .await
        .map_err(number_error)?
        .into_iter()
        .map(|number| AvailableNumberDto {
            e164: number.e164,
            region: number.region,
            monthly_cost: number.monthly_cost,
            currency: number.currency,
        })
        .collect();
    Ok(Json(AvailableNumberPage { items }))
}

/// Buy one number. The daemon writes a purchase intent first, so a
/// restart in the middle of the purchase reconciles instead of buying
/// twice (ADR-0018).
#[utoipa::path(post, path = "/api/v1/settings/phone-numbers",
    request_body = BuyNumberRequest, responses(
        (status = 201, body = PhoneNumberDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn buy_phone_number(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<BuyNumberRequest>,
) -> Result<(StatusCode, Json<PhoneNumberDto>), ApiError> {
    let agent_id = request.agent_id.map(AgentId::from);
    let number = state
        .numbers
        .buy(&tenant.workspace_id, &request.e164, agent_id.as_ref())
        .await
        .map_err(number_error)?;
    Ok((StatusCode::CREATED, Json(phone_number_dto(&state, number))))
}

/// Adopt a number the account already holds. The daemon asks the
/// carrier whether the account has it and records it; nothing is
/// bought. A number the account does not hold is refused.
#[utoipa::path(post, path = "/api/v1/settings/phone-numbers/adopt",
    request_body = AdoptNumberRequest, responses(
        (status = 201, body = PhoneNumberDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn adopt_phone_number(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<AdoptNumberRequest>,
) -> Result<(StatusCode, Json<PhoneNumberDto>), ApiError> {
    let agent_id = request.agent_id.map(AgentId::from);
    let number = state
        .numbers
        .adopt(&tenant.workspace_id, &request.e164, agent_id.as_ref())
        .await
        .map_err(number_error)?;
    Ok((StatusCode::CREATED, Json(phone_number_dto(&state, number))))
}

#[utoipa::path(post, path = "/api/v1/settings/phone-numbers/{phone_number_id}/assign",
    params(("phone_number_id" = String, Path)),
    request_body = AssignNumberRequest, responses(
        (status = 200, body = PhoneNumberDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    )
)]
pub async fn assign_phone_number(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(phone_number_id): Path<String>,
    Json(request): Json<AssignNumberRequest>,
) -> Result<Json<PhoneNumberDto>, ApiError> {
    let number = state
        .numbers
        .assign(
            &tenant.workspace_id,
            &PhoneNumberId::from(phone_number_id),
            &AgentId::from(request.agent_id),
        )
        .await
        .map_err(number_error)?;
    Ok(Json(phone_number_dto(&state, number)))
}

/// Take the line back. The Workspace keeps the number and keeps paying
/// for it, which is what makes this different from a release.
#[utoipa::path(post, path = "/api/v1/settings/phone-numbers/{phone_number_id}/unassign",
    params(("phone_number_id" = String, Path)), responses(
        (status = 200, body = PhoneNumberDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    )
)]
pub async fn unassign_phone_number(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(phone_number_id): Path<String>,
) -> Result<Json<PhoneNumberDto>, ApiError> {
    let number = state
        .numbers
        .unassign(&tenant.workspace_id, &PhoneNumberId::from(phone_number_id))
        .await
        .map_err(number_error)?;
    Ok(Json(phone_number_dto(&state, number)))
}

/// Give the number back to the carrier. The charge stops, the record
/// stays for the Calls that point at it, and the number never comes
/// back. The confirmation that names the Agent is the UI's work.
#[utoipa::path(post, path = "/api/v1/settings/phone-numbers/{phone_number_id}/release",
    params(("phone_number_id" = String, Path)), responses(
        (status = 200, body = PhoneNumberDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    )
)]
pub async fn release_phone_number(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(phone_number_id): Path<String>,
) -> Result<Json<PhoneNumberDto>, ApiError> {
    let number = state
        .numbers
        .release(&tenant.workspace_id, &PhoneNumberId::from(phone_number_id))
        .await
        .map_err(number_error)?;
    Ok(Json(phone_number_dto(&state, number)))
}

/// A refusal the user can act on keeps its words. A carrier failure
/// reports the stable code and nothing from upstream.
pub fn number_error(error: NumberError) -> ApiError {
    match error {
        NumberError::Store(error) => ApiError::from(error),
        NumberError::Validation(message) => ApiError::validation(message),
        NumberError::Conflict(message) => ApiError::conflict(message),
        NumberError::NotFound => ApiError::not_found("phone number"),
        NumberError::CallActive => {
            ApiError::conflict("a call on this number is running. This waits until the call ends.")
        }
        NumberError::NoCarrier => {
            ApiError::conflict("connect a telephony account before you buy a number".to_string())
        }
        NumberError::CarrierUnavailable(status) => {
            ApiError::conflict(format!("the telephony connection is {status}"))
        }
        NumberError::UnknownProvider(provider) => ApiError::conflict(format!(
            "this daemon has no client for the carrier provider {provider}"
        )),
        NumberError::Carrier(error) => ApiError::validation(format!(
            "the carrier did not complete this ({})",
            error.0.as_str()
        )),
        NumberError::EmergencyRefused(refused) => ApiError::conflict(refused.to_string()),
    }
}
