//! Current Mail Visibility limits human reads. Agents use their Grants.
use super::*;
use pagis_core::{AuditEntry, AuditEntryId, Author, MailVisibility, WorkspaceRole};

pub(crate) async fn effective_visibility(
    state: &AppState,
    tenant: &Tenant,
) -> Result<MailVisibility, ApiError> {
    let mailbox = state
        .mailbox_desk
        .workspace_mailbox(&tenant.workspace_id)
        .await
        .map_err(mailbox_error)?
        .ok_or_else(|| ApiError::not_found("Workspace Mailbox"))?;
    if tenant.is_private() {
        return Ok(MailVisibility::AllMail);
    }
    Ok(mailbox.visibility.min(
        state
            .governance
            .limits(&tenant.workspace_id)
            .await?
            .max_mail_visibility,
    ))
}
pub(super) async fn mailbox_dto(
    state: &AppState,
    tenant: &Tenant,
    mailbox: pagis_core::WorkspaceMailbox,
) -> Result<WorkspaceMailboxDto, ApiError> {
    let mut dto: WorkspaceMailboxDto = mailbox.into();
    if !tenant.is_private() {
        dto.max_visibility = state
            .governance
            .limits(&tenant.workspace_id)
            .await?
            .max_mail_visibility;
        dto.effective_visibility = dto.visibility.min(dto.max_visibility);
    }
    Ok(dto)
}

/// Assignment links use this same check before they expose a message.
pub async fn can_read_workspace_mail(
    state: &AppState,
    tenant: &Tenant,
    message: &pagis_mail::MessageSummary,
) -> Result<bool, ApiError> {
    let level = effective_visibility(state, tenant).await?;
    let escalated = if level == MailVisibility::Escalations {
        state
            .mailbox_desk
            .workspace_mailbox_records()
            .is_escalated(&tenant.workspace_id, message.assessment_key())
            .await?
    } else {
        false
    };
    Ok(level.permits(escalated))
}
pub(super) async fn audit_read(
    state: &AppState,
    tenant: &Tenant,
    action: &str,
    detail: serde_json::Value,
) -> Result<(), ApiError> {
    state
        .audit
        .append(&AuditEntry {
            id: AuditEntryId::generate(),
            org_workspace_id: state.org_workspace_id.clone(),
            actor: Author::Person(tenant.user_id.clone()),
            action: action.into(),
            subject_workspace_id: tenant.workspace_id.clone(),
            reason: Some(detail.to_string()),
            at: state.clock.now_ms(),
        })
        .await?;
    Ok(())
}
pub(super) async fn read_message(
    state: &AppState,
    tenant: &Tenant,
    store: &mut dyn WorkspaceMailStore,
    id: &str,
    action: &str,
) -> Result<pagis_mail::WorkspaceMailMessage, ApiError> {
    let message = store
        .message(&MessageId::new(id))
        .await
        .map_err(read_error)?;
    if !can_read_workspace_mail(state, tenant, &message.message.summary).await? {
        return Err(ApiError::not_found("mail message"));
    }
    audit_read(state, tenant, action, serde_json::json!({"message_id":id})).await?;
    Ok(message)
}
#[derive(Deserialize, ToSchema)]
pub struct SetMailVisibility {
    pub visibility: MailVisibility,
}
#[utoipa::path(put, path = "/api/v1/workspace/mailbox/visibility", request_body = SetMailVisibility, responses((status = 200, body = WorkspaceMailboxDto), (status = 403, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)))]
pub async fn set_workspace_mail_visibility(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(body): Json<SetMailVisibility>,
) -> Result<Json<WorkspaceMailboxDto>, ApiError> {
    if tenant.workspace_role != Some(WorkspaceRole::Manager) {
        return Err(ApiError::forbidden(
            "Only a Manager changes Mail Visibility.",
        ));
    }
    state
        .governance
        .limits(&tenant.workspace_id)
        .await?
        .check_mail_visibility(body.visibility)?;
    let mut mailbox = state
        .mailbox_desk
        .workspace_mailbox(&tenant.workspace_id)
        .await
        .map_err(mailbox_error)?
        .ok_or_else(|| ApiError::not_found("Workspace Mailbox"))?;
    state
        .mailbox_desk
        .workspace_mailbox_records()
        .set_visibility(&tenant.workspace_id, body.visibility)
        .await?;
    mailbox.visibility = body.visibility;
    Ok(Json(mailbox_dto(&state, &tenant, mailbox).await?))
}
#[utoipa::path(get, path = "/api/v1/workspace/mail/audit", params(crate::legal_hold::AuditQuery), responses((status = 200, body = crate::legal_hold::AuditPage), (status = 403, body = crate::error::ErrorBody)))]
pub async fn list_workspace_mail_audit(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<crate::legal_hold::AuditQuery>,
) -> Result<Json<crate::legal_hold::AuditPage>, ApiError> {
    if tenant.workspace_role != Some(WorkspaceRole::Manager) {
        return Err(ApiError::forbidden(
            "Only a Manager reads mailbox audit entries.",
        ));
    }
    let offset = query.offset.unwrap_or(0) as usize;
    let limit = query.limit.unwrap_or(50).clamp(1, 200) as usize;
    let mut items = Vec::new();
    let mut scan = 0;
    loop {
        let page = state.audit.list(&state.org_workspace_id, scan, 200).await?;
        let end = page.len() < 200;
        items.extend(page.into_iter().filter(|e| {
            e.subject_workspace_id == tenant.workspace_id && e.action.starts_with("mail.")
        }));
        if end || items.len() > offset + limit {
            break;
        }
        scan += 200;
    }
    let next_offset = (items.len() > offset + limit).then_some((offset + limit) as u32);
    Ok(Json(crate::legal_hold::AuditPage {
        items: items.into_iter().skip(offset).take(limit).collect(),
        next_offset,
    }))
}
