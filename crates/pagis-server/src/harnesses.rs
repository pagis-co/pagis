//! The Harness Catalog as the Product App reads it, and the Harness
//! Sign-In that the Person starts on one of their Hosts (ADR-0033).
//!
//! Only the Person starts a sign-in or a check of the sign-in state,
//! through these routes. No Agent tool does. The route answers when the Client App has the sign-in, and the
//! Person signs in in the terminal window that it opens. Pagis never
//! reads, copies, stores or relays the credential.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use pagis_coding::SignInFailure;
use pagis_core::harness::{self, SignInMethod};
use pagis_core::{Host, HostId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// One Coding Harness of the Harness Catalog.
#[derive(Debug, Serialize, ToSchema)]
pub struct HarnessDto {
    /// The id of the harness, as a Host declares it in `harness:<id>`.
    pub id: String,
    /// The display name of the harness.
    pub name: String,
    /// The ways the Person can sign in to the harness.
    pub sign_in_methods: Vec<SignInMethodDto>,
    /// True when the harness has a status command, so a Host can check
    /// whether the Person is signed in.
    pub checks_sign_in: bool,
}

/// One way to sign in to a harness.
#[derive(Debug, Serialize, ToSchema)]
pub struct SignInMethodDto {
    pub method: SignInMethod,
    /// The name of the method that a person reads.
    pub label: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HarnessesDto {
    pub items: Vec<HarnessDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StartSignInRequest {
    pub method: SignInMethod,
}

/// The sign-in that the Host received.
#[derive(Debug, Serialize, ToSchema)]
pub struct SignInStartedDto {
    pub id: String,
}

/// The Coding Harnesses that Pagis starts, in the order a list shows
/// them, with their sign-in methods.
#[utoipa::path(
    get,
    path = "/api/v1/harnesses",
    responses(
        (status = 200, body = HarnessesDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_harnesses(_tenant: Tenant) -> Json<HarnessesDto> {
    let items = harness::catalog()
        .iter()
        .map(|entry| HarnessDto {
            id: entry.id.to_string(),
            name: entry.label.to_string(),
            sign_in_methods: entry
                .sign_in
                .iter()
                .map(|sign_in| SignInMethodDto {
                    method: sign_in.method,
                    label: sign_in.method.label().to_string(),
                })
                .collect(),
            checks_sign_in: entry.sign_in_check.is_some(),
        })
        .collect();
    Json(HarnessesDto { items })
}

/// Start a Harness Sign-In on one of the Person's Hosts. The Client App
/// opens a terminal window that runs the vendor's own sign-in.
#[utoipa::path(
    post,
    path = "/api/v1/hosts/{host_id}/harnesses/{harness_id}/sign-in",
    params(
        ("host_id" = String, Path, description = "The Host"),
        ("harness_id" = String, Path, description = "The harness, by its id in the Harness Catalog"),
    ),
    request_body = StartSignInRequest,
    responses(
        (status = 202, body = SignInStartedDto, description = "The Host received the sign-in"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "The Host is not connected"),
        (status = 422, body = crate::error::ErrorBody, description = "The Host does not declare the harness, or the harness has no such sign-in"),
        (status = 502, body = crate::error::ErrorBody, description = "The harness did not give its sign-in methods"),
    )
)]
pub async fn start_sign_in(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((host_id, harness_id)): Path<(String, String)>,
    Json(request): Json<StartSignInRequest>,
) -> Result<(StatusCode, Json<SignInStartedDto>), ApiError> {
    let host = state
        .hosts
        .get(&tenant.workspace_id, &HostId::from(host_id))
        .await?
        .ok_or_else(|| ApiError::not_found("that host"))?;
    let id = state
        .sign_ins
        .start(&host, &harness_id, request.method)
        .await
        .map_err(|failure| sign_in_error(&host, failure))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SignInStartedDto { id: id.to_string() }),
    ))
}

/// Ask one of the Person's Hosts to run the vendor's status command of a
/// harness. The Client App reports the state on its Host socket, and
/// `harness.sign_in_changed` tells the clients when it changed.
#[utoipa::path(
    post,
    path = "/api/v1/hosts/{host_id}/harnesses/{harness_id}/sign-in-check",
    params(
        ("host_id" = String, Path, description = "The Host"),
        ("harness_id" = String, Path, description = "The harness, by its id in the Harness Catalog"),
    ),
    responses(
        (status = 204, description = "The Host received the check"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody, description = "The Host is not connected"),
        (status = 422, body = crate::error::ErrorBody, description = "The Host does not declare the harness, or the harness has no status command"),
    )
)]
pub async fn check_sign_in(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((host_id, harness_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let host = state
        .hosts
        .get(&tenant.workspace_id, &HostId::from(host_id))
        .await?
        .ok_or_else(|| ApiError::not_found("that host"))?;
    state
        .sign_ins
        .check(&host, &harness_id)
        .map_err(|failure| sign_in_error(&host, failure))?;
    Ok(StatusCode::NO_CONTENT)
}

fn sign_in_error(host: &Host, failure: SignInFailure) -> ApiError {
    let (status, message) = match &failure {
        SignInFailure::NotConnected => (
            StatusCode::CONFLICT,
            pagis_broker::not_connected_message(&host.name),
        ),
        SignInFailure::UndeclaredHarness(_)
        | SignInFailure::MethodNotOffered { .. }
        | SignInFailure::NoCheck(_)
        | SignInFailure::Unavailable { .. } => {
            (StatusCode::UNPROCESSABLE_ENTITY, failure.to_string())
        }
        SignInFailure::ProbeFailed(_) => (StatusCode::BAD_GATEWAY, failure.to_string()),
    };
    ApiError {
        status,
        code: failure.code(),
        message,
    }
}
