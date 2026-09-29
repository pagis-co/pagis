//! Owner controls for blocking, resumable local deletion, and explicit re-opt-in.
use crate::auth::Tenant;
use crate::{AppState, error::ApiError};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use pagis_core::{
    ForgetOperation, ForgetPhase, ForgetPreview, ForgetTarget, MemoryError, StoreError, WorkspaceId,
};
use serde::Deserialize;
use std::sync::Arc;
use utoipa::ToSchema;
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewForget {
    pub target: ForgetTarget,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfirmForget {
    pub target: ForgetTarget,
    pub preview: ForgetPreview,
}
fn storage(error: StoreError) -> ApiError {
    match error {
        StoreError::Conflict(message) => ApiError::conflict(message),
        other => other.into(),
    }
}
fn memory(error: MemoryError) -> ApiError {
    match error {
        MemoryError::Conflict => ApiError::conflict("forget preview changed; review it again"),
        _ => ApiError::internal(),
    }
}
#[utoipa::path(post,path="/api/v1/knowledge/forget/preview",request_body=PreviewForget,responses((status=200,body=ForgetPreview),(status=409,body=crate::error::ErrorBody)))]
pub async fn preview(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<PreviewForget>,
) -> Result<Json<ForgetPreview>, ApiError> {
    Ok(Json(
        state
            .memory
            .preview_forget(&tenant.workspace_id, &request.target)
            .await
            .map_err(memory)?,
    ))
}
#[utoipa::path(post,path="/api/v1/knowledge/forget",request_body=ConfirmForget,responses((status=202,body=ForgetOperation),(status=409,body=crate::error::ErrorBody)))]
pub async fn confirm(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<ConfirmForget>,
) -> Result<(StatusCode, Json<ForgetOperation>), ApiError> {
    let operation = state
        .memory
        .begin_forget(
            &tenant.workspace_id,
            &request.target,
            &request.preview,
            pagis_core::now_ms(),
            state.forget_keys.as_ref(),
        )
        .await
        .map_err(memory)?;
    spawn(&state, &tenant.workspace_id, operation.clone());
    Ok((StatusCode::ACCEPTED, Json(operation)))
}
#[utoipa::path(get,path="/api/v1/knowledge/forget",responses((status=200,body=Vec<ForgetOperation>),(status=500,body=crate::error::ErrorBody)))]
pub async fn list(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<Vec<ForgetOperation>>, ApiError> {
    Ok(Json(
        state
            .forget
            .operations(&tenant.workspace_id)
            .await
            .map_err(storage)?,
    ))
}
#[utoipa::path(get,path="/api/v1/knowledge/forget/{id}",params(("id"=String,Path)),responses((status=200,body=ForgetOperation),(status=404,body=crate::error::ErrorBody)))]
pub async fn get(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<Json<ForgetOperation>, ApiError> {
    Ok(Json(operation(&state, &tenant.workspace_id, &id).await?))
}
async fn operation(
    state: &AppState,
    workspace_id: &WorkspaceId,
    id: &str,
) -> Result<ForgetOperation, ApiError> {
    state
        .forget
        .operation(workspace_id, id)
        .await
        .map_err(storage)?
        .ok_or_else(|| ApiError::not_found("forget operation"))
}
#[utoipa::path(post,path="/api/v1/knowledge/forget/{id}/retry",params(("id"=String,Path)),responses((status=202,body=ForgetOperation),(status=404,body=crate::error::ErrorBody),(status=409,body=crate::error::ErrorBody)))]
pub async fn retry(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<ForgetOperation>), ApiError> {
    state
        .forget
        .retry(&tenant.workspace_id, &id)
        .await
        .map_err(storage)?;
    let operation = operation(&state, &tenant.workspace_id, &id).await?;
    spawn(&state, &tenant.workspace_id, operation.clone());
    Ok((StatusCode::ACCEPTED, Json(operation)))
}
#[utoipa::path(post,path="/api/v1/knowledge/forget/{id}/reopt",params(("id"=String,Path)),responses((status=200,body=ForgetOperation),(status=404,body=crate::error::ErrorBody),(status=409,body=crate::error::ErrorBody)))]
pub async fn reopt(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<Json<ForgetOperation>, ApiError> {
    state
        .forget
        .reopt_in(&tenant.workspace_id, &id, pagis_core::now_ms())
        .await
        .map_err(storage)?;
    Ok(Json(operation(&state, &tenant.workspace_id, &id).await?))
}
struct PurgeState {
    workspace_id: pagis_core::WorkspaceId,
    forget: Arc<dyn pagis_core::ForgetStore>,
    memory: Arc<dyn pagis_core::MemoryStore>,
}
pub(crate) fn spawn(state: &AppState, workspace_id: &WorkspaceId, operation: ForgetOperation) {
    let state = PurgeState {
        workspace_id: workspace_id.clone(),
        forget: Arc::clone(&state.forget),
        memory: Arc::clone(&state.memory),
    };
    tokio::spawn(async move {
        if let Err(error) = process(&state, &operation).await {
            tracing::error!(operation_id=%operation.id,%error,"forget purge failed");
            // Error details may contain paths or source text; persist only the safe action.
            let _ = state
                .forget
                .record_failure(
                    &state.workspace_id,
                    &operation.id,
                    "Local deletion could not finish. Retry when storage is available.",
                )
                .await;
        }
    });
}
async fn process(state: &PurgeState, operation: &ForgetOperation) -> Result<(), StoreError> {
    let workspace = &state.workspace_id;
    let id = &operation.id;
    if matches!(
        operation.phase,
        ForgetPhase::Blocked | ForgetPhase::StructuredPurged
    ) {
        state.forget.purge_structured(workspace, id).await?;
    }
    if matches!(
        operation.phase,
        ForgetPhase::Blocked | ForgetPhase::StructuredPurged
    ) {
        state
            .memory
            .purge_forgotten(workspace, id)
            .await
            .map_err(|_| StoreError::Conflict("memory purge failed".into()))?;
        state.forget.memory_purged(workspace, id).await?;
    }
    state.forget.complete(workspace, id).await
}
/// Take up the unfinished deletions of every Workspace after a restart.
/// The tenant comes off each row, so one pass covers every person.
pub async fn resume(state: &AppState) -> Result<(), StoreError> {
    for workspace in state.workspaces.list().await? {
        for operation in state.forget.unfinished(&workspace.id).await? {
            spawn(state, &workspace.id, operation);
        }
    }
    Ok(())
}
