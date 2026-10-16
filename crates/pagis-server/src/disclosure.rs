//! The Person writes a profile and manages Disclosure Rules.
use crate::{AppState, auth::Tenant, error::ApiError};
use axum::{Json, extract::State};
use pagis_core::DisclosureRule;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct DisclosureDto {
    pub profile: String,
    pub rules: Vec<DisclosureRule>,
}
#[derive(Deserialize, ToSchema)]
pub struct ProfileBody {
    pub profile: String,
}

async fn read(state: &AppState, person: &Tenant) -> Result<Json<DisclosureDto>, ApiError> {
    person.require_private("Disclosure Rules")?;
    Ok(Json(DisclosureDto {
        profile: state.disclosure.profile(&person.workspace_id).await?,
        rules: state.disclosure.list(&person.workspace_id).await?,
    }))
}
#[utoipa::path(get, path = "/api/v1/disclosure", responses((status = 200, body = DisclosureDto), (status = 401, body = crate::error::ErrorBody)))]
pub async fn get_disclosure(
    State(state): State<Arc<AppState>>,
    person: Tenant,
) -> Result<Json<DisclosureDto>, ApiError> {
    read(&state, &person).await
}

#[utoipa::path(put, path = "/api/v1/disclosure/profile", request_body = ProfileBody, responses((status = 200, body = DisclosureDto), (status = 422, body = crate::error::ErrorBody)))]
pub async fn set_profile(
    State(state): State<Arc<AppState>>,
    person: Tenant,
    Json(body): Json<ProfileBody>,
) -> Result<Json<DisclosureDto>, ApiError> {
    person.require_private("Disclosure Rules")?;
    if body.profile.len() > 16000 {
        return Err(ApiError::validation("The profile exceeds 16000 bytes."));
    }
    state
        .disclosure
        .set_profile(&person.workspace_id, &body.profile)
        .await?;
    read(&state, &person).await
}
async fn validate_asker(
    state: &AppState,
    person: &Tenant,
    rule: &DisclosureRule,
) -> Result<(), ApiError> {
    person.require_private("Disclosure Rules")?;
    if let Some(id) = &rule.asker_workspace_id {
        let workspace = state
            .workspaces
            .get(id)
            .await?
            .ok_or_else(|| ApiError::not_found("Workspace"))?;
        let owner = workspace
            .user_id
            .ok_or_else(|| ApiError::validation("Select a colleague's Private Workspace."))?;
        let asker = state
            .users
            .get(&owner)
            .await?
            .ok_or_else(|| ApiError::not_found("Person"))?;
        let author = state
            .users
            .get(&person.user_id)
            .await?
            .ok_or_else(|| ApiError::not_found("Person"))?;
        if asker.org_id != author.org_id || asker.is_disabled() || id == &person.workspace_id {
            return Err(ApiError::validation(
                "Select an enabled colleague of the Org.",
            ));
        }
    }
    Ok(())
}
#[utoipa::path(post, path = "/api/v1/disclosure/rules", request_body = DisclosureRule, responses((status = 200, body = DisclosureDto), (status = 422, body = crate::error::ErrorBody)))]
pub async fn add_rule(
    State(state): State<Arc<AppState>>,
    person: Tenant,
    Json(rule): Json<DisclosureRule>,
) -> Result<Json<DisclosureDto>, ApiError> {
    validate_asker(&state, &person, &rule).await?;
    state.disclosure.add(&person.workspace_id, &rule).await?;
    read(&state, &person).await
}
#[utoipa::path(delete, path = "/api/v1/disclosure/rules", request_body = DisclosureRule, responses((status = 200, body = DisclosureDto), (status = 422, body = crate::error::ErrorBody)))]
pub async fn remove_rule(
    State(state): State<Arc<AppState>>,
    person: Tenant,
    Json(rule): Json<DisclosureRule>,
) -> Result<Json<DisclosureDto>, ApiError> {
    person.require_private("Disclosure Rules")?;
    state.disclosure.remove(&person.workspace_id, &rule).await?;
    read(&state, &person).await
}
