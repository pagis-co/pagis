//! The Product API reads the tenant Workspace Mailbox through its store.
mod write;
pub use write::*;

use crate::mailboxes::{MailboxOfferDto, NewMailboxRequest, mailbox_error, offer_dto};
use crate::{AppState, auth::Tenant, error::ApiError};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use pagis_mail::{FlagChange, MessageId, PageRequest, WorkspaceMailStore};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceMailboxDto {
    pub spam_mode: String,
    pub workspace_id: String,
    pub connection_id: String,
    pub address: String,
    pub created_at: i64,
}
impl From<pagis_core::WorkspaceMailbox> for WorkspaceMailboxDto {
    fn from(m: pagis_core::WorkspaceMailbox) -> Self {
        Self {
            spam_mode: match m.spam_mode {
                pagis_core::MailScoringMode::ScoreOnly => "score_only".into(),
            },
            workspace_id: m.workspace_id.to_string(),
            connection_id: m.connection_id.to_string(),
            address: m.address,
            created_at: m.created_at,
        }
    }
}
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceMailboxPage {
    pub mailbox: Option<WorkspaceMailboxDto>,
    pub offers: Vec<MailboxOfferDto>,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceMailSummaryDto {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub snippet: String,
    pub date: i64,
    pub seen: bool,
    pub flagged: bool,
    pub labels: Vec<String>,
    pub spam_verdict: SpamVerdictDto,
    pub trust_tier: String,
}
impl From<pagis_mail::WorkspaceMailSummary> for WorkspaceMailSummaryDto {
    fn from(m: pagis_mail::WorkspaceMailSummary) -> Self {
        let assessment = m
            .summary
            .assessment
            .expect("Workspace Mailbox intake precedes a read");
        Self {
            spam_verdict: assessment.verdict.into(),
            trust_tier: assessment.trust_tier.as_str().into(),
            id: m.summary.id.to_string(),
            thread_id: m.summary.thread_id,
            from: m.summary.from,
            to: m.summary.to,
            subject: m.summary.subject,
            snippet: m.summary.snippet,
            date: m.summary.date,
            seen: m.flags.seen,
            flagged: m.flags.flagged,
            labels: m.labels,
        }
    }
}
#[derive(Debug, Serialize, ToSchema)]
pub struct SpamVerdictDto {
    pub opinion: Option<pagis_core::MailOpinion>,
    pub score: f64,
    pub action: String,
    pub reasons: Vec<String>,
    pub source: String,
}
impl From<pagis_core::SpamVerdict> for SpamVerdictDto {
    fn from(v: pagis_core::SpamVerdict) -> Self {
        Self {
            opinion: v.opinion,
            score: f64::from(v.score) / 1000.0,
            action: v.action.as_str().into(),
            reasons: v.reasons,
            source: v.source,
        }
    }
}
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceMailMessageDto {
    #[serde(flatten)]
    pub summary: WorkspaceMailSummaryDto,
    /// HTML mail is converted to text by the mail transport.
    pub text: String,
    pub attachments: Vec<MailAttachmentDto>,
}
impl From<pagis_mail::WorkspaceMailMessage> for WorkspaceMailMessageDto {
    fn from(m: pagis_mail::WorkspaceMailMessage) -> Self {
        Self {
            summary: pagis_mail::WorkspaceMailSummary {
                summary: m.message.summary,
                flags: m.flags,
                labels: m.labels,
            }
            .into(),
            text: m.message.text,
            attachments: m
                .message
                .attachments
                .into_iter()
                .map(|a| MailAttachmentDto {
                    name: a.name,
                    bytes: a.bytes,
                })
                .collect(),
        }
    }
}
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceMailFolders {
    pub items: Vec<String>,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceMailMessages {
    pub items: Vec<WorkspaceMailSummaryDto>,
    pub next: Option<String>,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceMailThread {
    pub items: Vec<WorkspaceMailMessageDto>,
}
#[derive(Debug, Deserialize, IntoParams)]
pub struct WorkspaceMailQuery {
    pub folder: Option<String>,
    pub q: Option<String>,
    pub cursor: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/workspace/mailbox", responses(
    (status = 200, body = WorkspaceMailboxPage), (status = 401, body = crate::error::ErrorBody)
))]
pub async fn get_workspace_mailbox(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<WorkspaceMailboxPage>, ApiError> {
    let mailbox = state
        .mailbox_desk
        .workspace_mailbox(&tenant.workspace_id)
        .await
        .map_err(mailbox_error)?;
    let offers = if mailbox.is_none() {
        state
            .mailbox_desk
            .offers("")
            .await
            .map_err(mailbox_error)?
            .into_iter()
            .filter(|offer| offer.settings.imap.is_some() && offer.settings.smtp.is_some())
            .map(offer_dto)
            .collect()
    } else {
        Vec::new()
    };
    Ok(Json(WorkspaceMailboxPage {
        mailbox: mailbox.map(Into::into),
        offers,
    }))
}
#[utoipa::path(post, path = "/api/v1/workspace/mailbox", request_body = NewMailboxRequest, responses(
    (status = 201, body = WorkspaceMailboxDto), (status = 401, body = crate::error::ErrorBody),
    (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)
))]
pub async fn create_workspace_mailbox(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<NewMailboxRequest>,
) -> Result<(StatusCode, Json<WorkspaceMailboxDto>), ApiError> {
    let mailbox = state
        .mailbox_desk
        .provision_workspace(&tenant.workspace_id, request.into())
        .await
        .map_err(mailbox_error)?;
    if let Some(agent) = state
        .workspaces
        .get(&tenant.workspace_id)
        .await?
        .and_then(|w| w.chief_of_staff_agent_id)
    {
        crate::agent_versions::record(&state, &tenant, &agent).await?;
    }
    Ok((StatusCode::CREATED, Json(mailbox.into())))
}
async fn open(state: &AppState, tenant: &Tenant) -> Result<Box<dyn WorkspaceMailStore>, ApiError> {
    if state
        .mailbox_desk
        .workspace_mailbox(&tenant.workspace_id)
        .await
        .map_err(mailbox_error)?
        .is_none()
    {
        return Err(ApiError::not_found("Workspace Mailbox"));
    }
    state
        .mailbox_desk
        .open_workspace(&tenant.workspace_id)
        .await
        .map_err(mailbox_error)
}
fn read_error(error: pagis_mail::TransportError) -> ApiError {
    match error.0 {
        pagis_mail::TransportErrorCode::InvalidRequest => {
            ApiError::validation("the mail request is not valid")
        }
        pagis_mail::TransportErrorCode::StaleId => ApiError::not_found("mail message"),
        _ => ApiError::conflict(error.to_string()),
    }
}
#[utoipa::path(get, path = "/api/v1/workspace/mail/folders", responses(
    (status = 200, body = WorkspaceMailFolders), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)
))]
pub async fn list_workspace_mail_folders(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<WorkspaceMailFolders>, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = store.folders().await;
    let _ = store.close().await;
    Ok(Json(WorkspaceMailFolders {
        items: result.map_err(read_error)?,
    }))
}
#[utoipa::path(get, path = "/api/v1/workspace/mail/messages", params(WorkspaceMailQuery), responses(
    (status = 200, body = WorkspaceMailMessages), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)
))]
pub async fn list_workspace_mail_messages(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Query(query): Query<WorkspaceMailQuery>,
) -> Result<Json<WorkspaceMailMessages>, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = store
        .messages(
            query.folder.as_deref().unwrap_or(""),
            query.q.as_deref().unwrap_or(""),
            &PageRequest::new(query.cursor, 50),
        )
        .await;
    let _ = store.close().await;
    let page = result.map_err(read_error)?;
    Ok(Json(WorkspaceMailMessages {
        items: page.items.into_iter().map(Into::into).collect(),
        next: page.next,
    }))
}
#[utoipa::path(get, path = "/api/v1/workspace/mail/messages/{message_id}", params(("message_id" = String, Path)), responses(
    (status = 200, body = WorkspaceMailMessageDto), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)
))]
pub async fn get_workspace_mail_message(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceMailMessageDto>, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = async {
        let id = MessageId::new(id);
        store
            .set_flags(
                &id,
                FlagChange {
                    seen: Some(true),
                    ..Default::default()
                },
            )
            .await?;
        store.message(&id).await
    }
    .await;
    let _ = store.close().await;
    Ok(Json(result.map_err(read_error)?.into()))
}
#[utoipa::path(get, path = "/api/v1/workspace/mail/threads/{thread_id}", params(("thread_id" = String, Path)), responses(
    (status = 200, body = WorkspaceMailThread), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)
))]
pub async fn get_workspace_mail_thread(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceMailThread>, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = async {
        let mut messages = store.thread(&id).await?;
        for m in &mut messages {
            store
                .set_flags(
                    &m.message.summary.id,
                    FlagChange {
                        seen: Some(true),
                        ..Default::default()
                    },
                )
                .await?;
            m.flags.seen = true;
        }
        Ok::<_, pagis_mail::TransportError>(messages)
    }
    .await;
    let _ = store.close().await;
    let items = result
        .map_err(read_error)?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(WorkspaceMailThread { items }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MailAttachmentDto {
    pub name: String,
    pub bytes: u64,
}
