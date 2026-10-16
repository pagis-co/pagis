//! Administrator control of Shared Workspace Limits.
use crate::{AppState, auth::Administrator, error::ApiError};
use axum::{
    Json,
    extract::{Path, State},
};
use pagis_core::{
    AuditEntry, AuditEntryId, Author, Grant, SharedWorkspaceLimits, WorkspaceId, WorkspaceKind,
};
use serde::Serialize;
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct LimitsDto {
    #[serde(flatten)]
    pub limits: SharedWorkspaceLimits,
    pub outside_plugins: usize,
    pub outside_connections: usize,
}
async fn read(state: &AppState, id: WorkspaceId) -> Result<Json<LimitsDto>, ApiError> {
    state
        .workspaces
        .get(&id)
        .await?
        .filter(|w| w.kind == WorkspaceKind::Shared)
        .ok_or_else(|| ApiError::not_found("Shared Workspace"))?;
    let limits = state.governance.limits(&id).await?;
    let mut result = LimitsDto {
        limits,
        outside_plugins: 0,
        outside_connections: 0,
    };
    for grant in state.grants.list_live(&id).await? {
        if result
            .limits
            .check_resource(
                &grant.resource_kind,
                grant.resource_id.as_deref().unwrap_or(""),
            )
            .is_err()
        {
            match grant.resource_kind.as_str() {
                Grant::PLUGIN_KIND => result.outside_plugins += 1,
                Grant::CONNECTION_KIND => result.outside_connections += 1,
                _ => {}
            }
        }
    }
    Ok(Json(result))
}
#[utoipa::path(get, operation_id = "get_workspace_limits", path = "/api/v1/administration/workspaces/{workspace_id}/limits", params(("workspace_id" = String, Path)), responses((status = 200, body = LimitsDto), (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)))]
pub async fn get(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Path(id): Path<String>,
) -> Result<Json<LimitsDto>, ApiError> {
    read(&state, WorkspaceId::from(id)).await
}
#[utoipa::path(put, operation_id = "set_workspace_limits", path = "/api/v1/administration/workspaces/{workspace_id}/limits", params(("workspace_id" = String, Path)), request_body = SharedWorkspaceLimits, responses((status = 200, body = LimitsDto), (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)))]
pub async fn put(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Path(id): Path<String>,
    Json(limits): Json<SharedWorkspaceLimits>,
) -> Result<Json<LimitsDto>, ApiError> {
    let id = WorkspaceId::from(id);
    let _ = read(&state, id.clone()).await?;
    state
        .governance
        .set_limits(
            &limits,
            &AuditEntry {
                id: AuditEntryId::generate(),
                org_workspace_id: state.org_workspace_id.clone(),
                actor: Author::Person(administrator.0.user_id),
                action: "workspace.limits_changed".into(),
                subject_workspace_id: id.clone(),
                reason: None,
                at: state.clock.now_ms(),
            },
        )
        .await?;
    read(&state, id).await
}
