//! A Manager changes Shared Workspace membership.
use crate::{AppState, auth::Tenant, error::ApiError};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::get,
};
use pagis_core::{Author, UserId, WorkspaceMember, WorkspaceRole};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct MemberDto {
    pub person_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub role: WorkspaceRole,
}
#[derive(Serialize, ToSchema)]
pub struct MembersDto {
    pub items: Vec<MemberDto>,
}
#[derive(Deserialize, ToSchema)]
pub struct AddMember {
    pub person_id: String,
    pub role: WorkspaceRole,
}
#[derive(Deserialize, ToSchema)]
pub struct ChangeMember {
    pub role: WorkspaceRole,
}

pub(crate) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/members",
            get(list).post(add),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members/{person_id}",
            axum::routing::patch(change).delete(remove),
        )
}

#[utoipa::path(get, operation_id = "list_workspace_members", path = "/api/v1/workspaces/{workspace_id}/members", params(("workspace_id" = String, Path)), responses((status = 200, body = MembersDto)))]
pub async fn list(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<MembersDto>, ApiError> {
    let mut items = Vec::new();
    for member in state.workspace_members.list(&tenant.workspace_id).await? {
        let person = state
            .users
            .get(&member.person_id)
            .await?
            .ok_or_else(|| ApiError::not_found("person"))?;
        items.push(MemberDto {
            person_id: person.id.to_string(),
            name: person.name,
            email: person.email,
            role: member.role,
        });
    }
    Ok(Json(MembersDto { items }))
}

#[utoipa::path(post, operation_id = "add_workspace_member", path = "/api/v1/workspaces/{workspace_id}/members", params(("workspace_id" = String, Path)), request_body = AddMember, responses((status = 204), (status = 409, body = crate::error::ErrorBody)))]
pub async fn add(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(body): Json<AddMember>,
) -> Result<StatusCode, ApiError> {
    let person_id = UserId::from(body.person_id);
    let person = state
        .users
        .get(&person_id)
        .await?
        .filter(|u| !u.is_disabled())
        .ok_or_else(|| ApiError::not_found("person"))?;
    let caller = state
        .users
        .get(&tenant.user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("person"))?;
    if person.org_id != caller.org_id {
        return Err(ApiError::not_found("person"));
    }
    state
        .workspace_members
        .add(&WorkspaceMember {
            workspace_id: tenant.workspace_id.clone(),
            person_id: person_id.clone(),
            role: body.role,
            author: Author::Person(tenant.user_id.clone()),
        })
        .await
        .map_err(membership_error)?;
    record_change(
        &state,
        &tenant,
        "workspace.member_added",
        person_id.as_str(),
        Some(body.role),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(patch, operation_id = "change_workspace_member", path = "/api/v1/workspaces/{workspace_id}/members/{person_id}", params(("workspace_id" = String, Path), ("person_id" = String, Path)), request_body = ChangeMember, responses((status = 204), (status = 409, body = crate::error::ErrorBody)))]
pub async fn change(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((_workspace_id, person_id)): Path<(String, String)>,
    Json(body): Json<ChangeMember>,
) -> Result<StatusCode, ApiError> {
    let changed = state
        .workspace_members
        .change(&WorkspaceMember {
            workspace_id: tenant.workspace_id.clone(),
            person_id: UserId::from(person_id.clone()),
            role: body.role,
            author: Author::Person(tenant.user_id.clone()),
        })
        .await
        .map_err(membership_error)?;
    if !changed {
        return Err(ApiError::not_found("member"));
    }
    record_change(
        &state,
        &tenant,
        "workspace.member_changed",
        &person_id,
        Some(body.role),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, operation_id = "remove_workspace_member", path = "/api/v1/workspaces/{workspace_id}/members/{person_id}", params(("workspace_id" = String, Path), ("person_id" = String, Path)), responses((status = 204), (status = 409, body = crate::error::ErrorBody)))]
pub async fn remove(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((_workspace_id, person_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    if !state
        .workspace_members
        .remove(&tenant.workspace_id, &UserId::from(person_id.clone()))
        .await
        .map_err(membership_error)?
    {
        return Err(ApiError::not_found("member"));
    }
    record_change(
        &state,
        &tenant,
        "workspace.member_removed",
        &person_id,
        None,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn record_change(
    state: &AppState,
    tenant: &Tenant,
    event_type: &str,
    person_id: &str,
    role: Option<WorkspaceRole>,
) -> Result<(), ApiError> {
    state
        .bus
        .publish(pagis_core::NewEvent {
            workspace_id: tenant.workspace_id.clone(),
            event_type: event_type.into(),
            agent_id: None,
            run_id: None,
            channel_id: None,
            payload: serde_json::json!({ "person_id": person_id, "role": role, "author": { "kind": "person", "id": tenant.user_id } }),
        })
        .await
        .map_err(membership_error)?;
    Ok(())
}

pub(crate) fn membership_error(error: pagis_core::StoreError) -> ApiError {
    match error {
        pagis_core::StoreError::Conflict(message) => ApiError::conflict(message),
        other => other.into(),
    }
}
