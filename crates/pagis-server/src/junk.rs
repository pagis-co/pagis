//! Junk policy, Person corrections and Administrator quarantine access.
use crate::{
    AppState,
    auth::{Administrator, Tenant},
    error::ApiError,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use pagis_core::{
    AuditEntry, AuditEntryId, Author, JunkMessage, MailLists, OrgJunkPolicy, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SpamCorrection {
    pub spam: bool,
}
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceJunkPolicy {
    pub lists: MailLists,
    pub spam_mode: pagis_core::MailScoringMode,
}
#[derive(Serialize, ToSchema)]
pub struct QuarantinePage {
    pub items: Vec<JunkMessage>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct QuarantineMessageRequest {
    pub workspace_id: String,
    pub mailbox: String,
    pub message_key: String,
}

#[utoipa::path(post, path = "/api/v1/workspace/mail/messages/{message_id}/spam", params(("message_id" = String, Path)), request_body = SpamCorrection, responses((status = 204), (status = 404, body = crate::error::ErrorBody)))]
pub async fn correct(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Json(request): Json<SpamCorrection>,
) -> Result<StatusCode, ApiError> {
    let (mut store, _) =
        crate::workspace_mail::open_message(&state, &tenant, &id, "mail.message.spam").await?;
    let result = async {
        store
            .mark_spam(&pagis_mail::MessageId::new(id), request.spam)
            .await
            .map_err(crate::workspace_mail::read_error)
    }
    .await;
    let _ = store.close().await;
    result?;
    Ok(StatusCode::NO_CONTENT)
}
#[utoipa::path(get, path = "/api/v1/workspace/mail/junk-policy", responses((status = 200, body = WorkspaceJunkPolicy), (status = 401, body = crate::error::ErrorBody)))]
pub async fn get_workspace_policy(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<WorkspaceJunkPolicy>, ApiError> {
    let mailbox = state
        .mailbox_desk
        .workspace_mailbox(&tenant.workspace_id)
        .await
        .map_err(crate::mailboxes::mailbox_error)?
        .ok_or_else(|| ApiError::not_found("Workspace Mailbox"))?;
    Ok(Json(WorkspaceJunkPolicy {
        lists: state
            .mailbox_desk
            .junk()
            .workspace_lists(&tenant.workspace_id)
            .await?,
        spam_mode: mailbox.spam_mode,
    }))
}
#[utoipa::path(put, path = "/api/v1/workspace/mail/junk-policy", request_body = WorkspaceJunkPolicy, responses((status = 204), (status = 422, body = crate::error::ErrorBody)))]
pub async fn set_workspace_policy(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    body: Result<Json<WorkspaceJunkPolicy>, axum::extract::rejection::JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(policy) = body.map_err(|error| ApiError::validation(error.body_text()))?;
    policy
        .lists
        .validate()
        .map_err(|e| ApiError::validation(e.to_string()))?;
    state
        .mailbox_desk
        .workspace_mailbox_records()
        .set_spam_mode(&tenant.workspace_id, policy.spam_mode)
        .await?;
    state
        .mailbox_desk
        .junk()
        .set_workspace_lists(&tenant.workspace_id, &policy.lists)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
#[utoipa::path(get, path = "/api/v1/administration/mail/junk-policy", responses((status = 200, body = OrgJunkPolicy), (status = 403, body = crate::error::ErrorBody)))]
pub async fn get_org_policy(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<OrgJunkPolicy>, ApiError> {
    Ok(Json(state.mailbox_desk.junk().org_policy().await?))
}
#[utoipa::path(put, path = "/api/v1/administration/mail/junk-policy", request_body = OrgJunkPolicy, responses((status = 204), (status = 422, body = crate::error::ErrorBody)))]
pub async fn set_org_policy(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Json(policy): Json<OrgJunkPolicy>,
) -> Result<StatusCode, ApiError> {
    policy
        .lists
        .validate()
        .map_err(|e| ApiError::validation(e.to_string()))?;
    state.mailbox_desk.junk().set_org_policy(&policy).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[utoipa::path(get, path = "/api/v1/administration/mail/quarantine", responses((status = 200, body = QuarantinePage), (status = 403, body = crate::error::ErrorBody)))]
pub async fn quarantine(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
) -> Result<Json<QuarantinePage>, ApiError> {
    Ok(Json(QuarantinePage {
        items: state.mailbox_desk.junk().quarantine().await?,
    }))
}
async fn entry(
    state: &AppState,
    request: &QuarantineMessageRequest,
) -> Result<JunkMessage, ApiError> {
    state
        .mailbox_desk
        .junk()
        .message(
            &WorkspaceId::from(request.workspace_id.clone()),
            &request.mailbox,
            &request.message_key,
        )
        .await?
        .filter(|m| m.quarantined && !m.released)
        .ok_or_else(|| ApiError::not_found("quarantined message"))
}
#[utoipa::path(post, path = "/api/v1/administration/mail/quarantine/read", request_body = QuarantineMessageRequest, responses((status = 200, body = crate::workspace_mail::WorkspaceMailMessageDto), (status = 404, body = crate::error::ErrorBody)))]
pub async fn read_quarantine(
    State(state): State<Arc<AppState>>,
    _administrator: Administrator,
    Json(request): Json<QuarantineMessageRequest>,
) -> Result<Json<crate::workspace_mail::WorkspaceMailMessageDto>, ApiError> {
    let entry = entry(&state, &request).await?;
    Ok(Json(
        state
            .mailbox_desk
            .read_quarantine(&entry, state.mailboxes.as_ref())
            .await
            .map_err(crate::mailboxes::mailbox_error)?
            .into(),
    ))
}
#[utoipa::path(post, path = "/api/v1/administration/mail/quarantine/release", request_body = QuarantineMessageRequest, responses((status = 204), (status = 404, body = crate::error::ErrorBody)))]
pub async fn release(
    State(state): State<Arc<AppState>>,
    administrator: Administrator,
    Json(request): Json<QuarantineMessageRequest>,
) -> Result<StatusCode, ApiError> {
    let entry = entry(&state, &request).await?;
    state
        .mailbox_desk
        .junk()
        .release(
            &entry.mailbox,
            &entry.message_key,
            &AuditEntry {
                id: AuditEntryId::generate(),
                org_workspace_id: state.org_workspace_id.clone(),
                actor: Author::Person(administrator.0.user_id),
                action: "mail.quarantine_released".into(),
                subject_workspace_id: entry.workspace_id,
                reason: Some(format!("{} {}", entry.mailbox, entry.message_key)),
                at: state.clock.now_ms(),
            },
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
