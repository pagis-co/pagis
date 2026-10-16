//! Administrator access to Legal Holds and the Org Audit Log.
use crate::{AppState, auth::Administrator, error::ApiError};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use pagis_core::{AuditEntry, AuditEntryId, Author, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, ToSchema)]
pub struct StartLegalHold {
    pub reason: String,
}
#[derive(Default, Deserialize, IntoParams)]
pub struct AuditQuery {
    pub offset: Option<u32>,
    pub limit: Option<u32>,
}
#[derive(Serialize, ToSchema)]
pub struct AuditPage {
    pub items: Vec<AuditEntry>,
    pub next_offset: Option<u32>,
}

async fn change(
    state: &AppState,
    administrator: Administrator,
    id: String,
    reason: Option<String>,
    start: bool,
) -> Result<Json<crate::administration::AdministrationWorkspaceDto>, ApiError> {
    let id = WorkspaceId::from(id);
    state
        .workspaces
        .get(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace"))?;
    let entry = AuditEntry {
        id: AuditEntryId::generate(),
        org_workspace_id: state.org_workspace_id.clone(),
        actor: Author::Person(administrator.0.user_id),
        action: if start {
            "legal_hold.started"
        } else {
            "legal_hold.ended"
        }
        .into(),
        subject_workspace_id: id.clone(),
        reason,
        at: state.clock.now_ms(),
    };
    if start {
        state.workspaces.start_legal_hold(&entry).await?;
    } else {
        state.workspaces.end_legal_hold(&entry).await?;
    }
    let workspace = state
        .workspaces
        .get(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace"))?;
    Ok(Json(
        crate::administration::workspace_dto(state, &workspace).await?,
    ))
}
#[utoipa::path(put, operation_id = "start_legal_hold", path = "/api/v1/admin/workspaces/{workspace_id}/legal-hold", params(("workspace_id" = String, Path)), request_body = StartLegalHold, responses((status = 200, body = crate::administration::AdministrationWorkspaceDto), (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn start(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Path(id): Path<String>,
    Json(body): Json<StartLegalHold>,
) -> Result<Json<crate::administration::AdministrationWorkspaceDto>, ApiError> {
    let reason = body.reason.trim();
    if reason.is_empty() {
        return Err(ApiError::validation("Enter a reason for the Legal Hold."));
    }
    change(&state, administrator, id, Some(reason.into()), true).await
}
#[utoipa::path(delete, operation_id = "end_legal_hold", path = "/api/v1/admin/workspaces/{workspace_id}/legal-hold", params(("workspace_id" = String, Path)), responses((status = 200, body = crate::administration::AdministrationWorkspaceDto), (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)))]
pub async fn end(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Path(id): Path<String>,
) -> Result<Json<crate::administration::AdministrationWorkspaceDto>, ApiError> {
    change(&state, administrator, id, None, false).await
}
#[utoipa::path(get, operation_id = "list_audit_entries", path = "/api/v1/admin/audit", params(AuditQuery), responses((status = 200, body = AuditPage), (status = 403, body = crate::error::ErrorBody)))]
pub async fn list(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Query(query): Query<AuditQuery>,
) -> Result<Json<AuditPage>, ApiError> {
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let mut items = state
        .audit
        .list(&state.org_workspace_id, offset, limit + 1)
        .await?;
    let next_offset = (items.len() > limit as usize).then(|| offset.saturating_add(limit));
    items.truncate(limit as usize);
    Ok(Json(AuditPage { items, next_offset }))
}
