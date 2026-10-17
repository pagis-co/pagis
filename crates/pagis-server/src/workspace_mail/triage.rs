//! Mail Triage Rules belong to the Workspace that owns the mailbox.
use super::*;
use pagis_core::{MailTriageRule, WorkspaceRole};

fn require_manager(tenant: &Tenant) -> Result<(), ApiError> {
    if !tenant.is_private() && tenant.workspace_role != Some(WorkspaceRole::Manager) {
        return Err(ApiError::forbidden(
            "Only an Owner or Manager changes Mail Triage Rules.",
        ));
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/workspace/mailbox/triage-rules",
    responses(
        (status = 200, body = Vec<MailTriageRule>),
        (status = 403, body = crate::error::ErrorBody)
    )
)]
pub async fn list_mail_triage_rules(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<Vec<MailTriageRule>>, ApiError> {
    require_manager(&tenant)?;
    Ok(Json(
        state
            .mailbox_desk
            .workspace_mailbox_records()
            .triage_rules(&tenant.workspace_id)
            .await?,
    ))
}

#[utoipa::path(
    put,
    path = "/api/v1/workspace/mailbox/triage-rules",
    request_body = Vec<MailTriageRule>,
    responses(
        (status = 200, body = Vec<MailTriageRule>),
        (status = 403, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn replace_mail_triage_rules(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(rules): Json<Vec<MailTriageRule>>,
) -> Result<Json<Vec<MailTriageRule>>, ApiError> {
    require_manager(&tenant)?;
    pagis_core::validate_triage_rules(&rules).map_err(|e| ApiError::validation(e.to_string()))?;
    state
        .mailbox_desk
        .workspace_mailbox_records()
        .replace_triage_rules(&tenant.workspace_id, &rules)
        .await?;
    Ok(Json(rules))
}

#[utoipa::path(
    post,
    path = "/api/v1/workspace/mail/messages/{message_id}/done",
    params(("message_id" = String, Path)),
    responses(
        (status = 204),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn done_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (mut store, message) = open_message(&state, &tenant, &id, "mail.message.done").await?;
    let _ = store.close().await;
    state
        .mailbox_desk
        .resolve_triage(
            &tenant.workspace_id,
            message.message.summary.assessment_key(),
        )
        .await
        .map_err(mailbox_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn open_needs_you_messages(
    state: &AppState,
    tenant: &Tenant,
    cursor: Option<&str>,
    limit: usize,
) -> Result<WorkspaceMailMessages, ApiError> {
    let before: Option<(pagis_core::UnixMillis, String)> = cursor
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| ApiError::validation("the Mail Triage cursor is not valid"))?;
    let page = state
        .mailbox_desk
        .workspace_mailbox_records()
        .open_needs_you(
            &tenant.workspace_id,
            before.as_ref().map(|(at, key)| (*at, key.as_str())),
            limit as u32,
        )
        .await?;
    let next = if page.len() == limit {
        page.last().map(|(key, triage)| {
            serde_json::to_string(&(triage.recorded_at, key)).expect("Mail Triage cursor")
        })
    } else {
        None
    };
    let mut items = Vec::new();
    for (_, triage) in page {
        let (mut store, message) =
            match open_message(state, tenant, &triage.message_id, "mail.message.read").await {
                Ok(message) => message,
                Err(error) if error.status == StatusCode::NOT_FOUND => continue,
                Err(error) => return Err(error),
            };
        let _ = store.close().await;
        let mut dto = WorkspaceMailSummaryDto::from(pagis_mail::WorkspaceMailSummary {
            summary: message.message.summary,
            flags: message.flags,
            labels: message.labels,
        });
        dto.triage = Some(triage);
        items.push(dto);
    }
    Ok(WorkspaceMailMessages { items, next })
}
