//! The HTTP surface of the Needs-You Queue.

use std::sync::Arc;

use axum::{Json, extract::State};
use pagis_needs_you::NeedsYouQueue;

use crate::{AppState, auth::Tenant, error::ApiError};

/// The Needs-You Queue of the Person's Workspace.
#[utoipa::path(
    get,
    path = "/api/v1/needs-you",
    responses(
        (status = 200, body = NeedsYouQueue),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn get_needs_you(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<NeedsYouQueue>, ApiError> {
    let queue = state
        .needs_you
        .derive(&tenant.workspace_id, state.clock.now_ms())
        .await?;
    Ok(Json(queue))
}
