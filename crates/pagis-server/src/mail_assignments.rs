//! Mail Assignment lists, Manager settings and the Person's FYI feed.
use crate::{AppState, auth::Tenant, error::ApiError};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use pagis_core::{MailAssignment, MailAssignmentId, MailAssignmentSettings, WorkspaceRole};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

#[derive(Serialize, ToSchema)]
pub struct MailAssignmentPage {
    pub items: Vec<MailAssignment>,
}
#[derive(Deserialize, IntoParams)]
pub struct MailAssignmentQuery {
    pub message_id: Option<String>,
}
fn require_shared(tenant: &Tenant, role: WorkspaceRole) -> Result<(), ApiError> {
    if tenant.workspace_role.is_none_or(|held| held < role) {
        return Err(ApiError::forbidden(
            "This Mail Assignment action needs the Shared Workspace role.",
        ));
    }
    Ok(())
}
#[utoipa::path(get, path = "/api/v1/workspace/mail/assignments", params(MailAssignmentQuery), responses((status = 200, body = MailAssignmentPage), (status = 403, body = crate::error::ErrorBody)))]
pub async fn list_mail_assignments(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<MailAssignmentQuery>,
) -> Result<Json<MailAssignmentPage>, ApiError> {
    require_shared(&tenant, WorkspaceRole::Responder)?;
    let key = if let Some(id) = query.message_id {
        let (mut store, message) =
            crate::workspace_mail::open_message(&state, &tenant, &id, "mail.assignments").await?;
        let key = message.message.summary.assessment_key().to_string();
        store
            .close()
            .await
            .map_err(crate::workspace_mail::read_error)?;
        Some(key)
    } else {
        None
    };
    Ok(Json(MailAssignmentPage {
        items: state
            .assignments
            .records()
            .list(&tenant.workspace_id, key.as_deref(), 500)
            .await?,
    }))
}
#[utoipa::path(get, path = "/api/v1/workspace/mailbox/assignment-settings", responses((status = 200, body = MailAssignmentSettings), (status = 403, body = crate::error::ErrorBody)))]
pub async fn get_assignment_settings(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<MailAssignmentSettings>, ApiError> {
    require_shared(&tenant, WorkspaceRole::Manager)?;
    Ok(Json(
        state
            .assignments
            .records()
            .settings(&tenant.workspace_id)
            .await?,
    ))
}
#[utoipa::path(put, path = "/api/v1/workspace/mailbox/assignment-settings", request_body = MailAssignmentSettings, responses((status = 200, body = MailAssignmentSettings), (status = 403, body = crate::error::ErrorBody)))]
pub async fn set_assignment_settings(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(settings): Json<MailAssignmentSettings>,
) -> Result<Json<MailAssignmentSettings>, ApiError> {
    require_shared(&tenant, WorkspaceRole::Manager)?;
    state
        .assignments
        .records()
        .set_settings(&tenant.workspace_id, &settings)
        .await?;
    Ok(Json(settings))
}
#[utoipa::path(get, path = "/api/v1/mail-assignments/feed", responses((status = 200, body = MailAssignmentPage), (status = 403, body = crate::error::ErrorBody)))]
pub async fn assignment_feed(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<MailAssignmentPage>, ApiError> {
    tenant.require_private("FYI feed")?;
    let workspace = state
        .workspaces
        .get(&tenant.workspace_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Workspace"))?;
    Ok(Json(MailAssignmentPage {
        items: state
            .assignments
            .feed(&tenant.user_id, state.clock.now_ms(), &workspace.timezone)
            .await?,
    }))
}
#[utoipa::path(post, path = "/api/v1/mail-assignments/{assignment_id}/done", params(("assignment_id" = String, Path)), responses((status = 200), (status = 404, body = crate::error::ErrorBody)))]
pub async fn finish_assignment(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tenant.require_private("Mail Assignment")?;
    let id = MailAssignmentId::from(id);
    let row = state
        .assignments
        .records()
        .find(&id)
        .await?
        .filter(|a| a.assignee == tenant.user_id)
        .ok_or_else(|| ApiError::not_found("Mail Assignment"))?;
    if state
        .workspace_members
        .get(&row.workspace_id, &tenant.user_id)
        .await?
        .is_none()
    {
        return Err(ApiError::not_found("Mail Assignment"));
    }
    state
        .assignments
        .finish_by_person(&tenant.user_id, &id)
        .await?;
    Ok(Json(serde_json::json!({
        "state": "handled",
    })))
}

#[utoipa::path(get, path = "/api/v1/conversations/{conversation_id}/assignment", params(("conversation_id" = String, Path)), responses((status = 200, body = Option<MailAssignment>), (status = 404, body = crate::error::ErrorBody)))]
pub async fn conversation_assignment(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<Json<Option<MailAssignment>>, ApiError> {
    let channel = pagis_core::ChannelId::from(id);
    crate::conversations::entrance(&state, &tenant, &channel).await?;
    Ok(Json(
        state.assignments.records().by_channel(&channel).await?,
    ))
}
