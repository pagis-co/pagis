//! The Workspace Number in Settings (ADR-0041).
use crate::{AppState, auth::Tenant, error::ApiError};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use pagis_core::PhoneNumberStatus;
use pagis_telephony::NumberError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceNumberDto {
    pub id: String,
    pub e164: String,
    pub status: String,
    pub connection_id: String,
    pub registration: Option<String>,
    pub registration_failure: Option<String>,
    pub created_at: i64,
}
fn number_dto(state: &AppState, number: pagis_core::PhoneNumber) -> WorkspaceNumberDto {
    let registration = state.native.numbers.registration(&number);
    WorkspaceNumberDto {
        id: number.id.to_string(),
        e164: number.e164,
        status: number.status.as_str().into(),
        connection_id: number.connection_id.to_string(),
        registration: registration.map(|state| state.as_str().into()),
        registration_failure: registration
            .and_then(|state| state.failure())
            .map(|failure| failure.as_str().into()),
        created_at: number.created_at,
    }
}
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceNumberPage {
    pub number: Option<WorkspaceNumberDto>,
    pub carrier: Option<CarrierDto>,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct CarrierDto {
    pub connection_id: String,
    pub display_name: String,
    pub status: String,
}
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
#[derive(Debug, Deserialize, IntoParams)]
pub struct SearchNumbersQuery {
    pub country: String,
    pub area_code: Option<String>,
    pub locality: Option<String>,
}
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct BuyNumberRequest {
    pub e164: String,
}

#[utoipa::path(get, path = "/api/v1/workspace/number", responses((status = 200, body = WorkspaceNumberPage)))]
pub async fn get_workspace_number(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<WorkspaceNumberPage>, ApiError> {
    let number = state
        .native
        .numbers
        .list(&tenant.workspace_id)
        .await
        .map_err(number_error)?
        .into_iter()
        .find(|number| number.status == PhoneNumberStatus::Active)
        .map(|number| number_dto(&state, number));
    let carrier = state
        .native
        .numbers
        .carrier()
        .await
        .map_err(number_error)?
        .map(|connection| CarrierDto {
            connection_id: connection.id.to_string(),
            display_name: connection.display_name,
            status: connection.status,
        });
    Ok(Json(WorkspaceNumberPage { number, carrier }))
}
#[utoipa::path(post, path = "/api/v1/workspace/number", request_body = BuyNumberRequest, responses((status = 201, body = WorkspaceNumberDto), (status = 409, body = crate::error::ErrorBody)))]
pub async fn buy_workspace_number(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<BuyNumberRequest>,
) -> Result<(StatusCode, Json<WorkspaceNumberDto>), ApiError> {
    let number = state
        .native
        .numbers
        .buy(&tenant.workspace_id, &request.e164)
        .await
        .map_err(number_error)?;
    Ok((StatusCode::CREATED, Json(number_dto(&state, number))))
}
#[utoipa::path(delete, path = "/api/v1/workspace/number", responses((status = 204), (status = 409, body = crate::error::ErrorBody)))]
pub async fn release_workspace_number(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<StatusCode, ApiError> {
    let number = state
        .native
        .numbers
        .list(&tenant.workspace_id)
        .await
        .map_err(number_error)?
        .into_iter()
        .find(|number| number.status == PhoneNumberStatus::Active)
        .ok_or_else(|| ApiError::not_found("Workspace Number"))?;
    state
        .native
        .numbers
        .release(&tenant.workspace_id, &number.id)
        .await
        .map_err(number_error)?;
    Ok(StatusCode::NO_CONTENT)
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
        .native
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
    }
}
