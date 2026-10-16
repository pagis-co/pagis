//! A Person sends and organizes Workspace Mailbox mail.
use super::{open, read_error};
use crate::mailboxes::mailbox_error;
use crate::{AppState, auth::Tenant, error::ApiError};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use object_store::{ObjectStoreExt, path::Path as BlobPath};
use pagis_core::FileId;
use pagis_mail::{MessageId, OutgoingMessage, WorkspaceMailStore, forward_message, reply_message};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct SendMailRequest {
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub files: Vec<String>,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct SentMailDto {
    pub message_id: String,
}
#[derive(Debug, Deserialize, ToSchema)]
pub struct MailNameRequest {
    pub name: String,
}
#[derive(Debug, Deserialize, IntoParams)]
pub struct ComposeQuery {
    pub action: String,
}
#[derive(Debug, Serialize, ToSchema)]
pub struct MailComposerDto {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub body: String,
}

#[utoipa::path(get, path = "/api/v1/workspace/mail/messages/{message_id}/composer", params(("message_id" = String, Path), ComposeQuery), responses((status = 200, body = MailComposerDto)))]
pub async fn prepare_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Query(query): Query<ComposeQuery>,
) -> Result<Json<MailComposerDto>, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = prepare(&state, &tenant, store.as_mut(), &id, &query.action).await;
    let _ = store.close().await;
    let mail = result?;
    Ok(Json(MailComposerDto {
        to: mail.to,
        cc: mail.cc,
        subject: mail.subject,
        body: mail.body,
    }))
}
async fn prepare(
    state: &AppState,
    tenant: &Tenant,
    store: &mut dyn WorkspaceMailStore,
    id: &str,
    action: &str,
) -> Result<OutgoingMessage, ApiError> {
    let original = super::read_message(state, tenant, store, id, "mail.composer.read")
        .await?
        .message;
    let mailbox = state
        .mailbox_desk
        .workspace_mailbox(&tenant.workspace_id)
        .await
        .map_err(mailbox_error)?
        .ok_or_else(|| ApiError::not_found("Workspace Mailbox"))?;
    match action {
        "reply" | "reply-all" => Ok(reply_message(
            &original,
            &mailbox.address,
            action == "reply-all",
        )),
        "forward" => {
            let mut mail = forward_message(&original);
            for index in 0..original.attachments.len() {
                mail.attachments.push(
                    store
                        .attachment(&MessageId::new(id), index)
                        .await
                        .map_err(read_error)?,
                );
            }
            Ok(mail)
        }
        _ => Err(ApiError::validation("the mail action is not valid")),
    }
}
async fn send(
    state: &AppState,
    tenant: &Tenant,
    source: Option<(&str, &str)>,
    request: SendMailRequest,
) -> Result<(StatusCode, Json<SentMailDto>), ApiError> {
    let mut store = open(state, tenant).await?;
    let result = async {
        let mut mail = match source {
            Some((id, action)) => prepare(state, tenant, store.as_mut(), id, action).await?,
            None => OutgoingMessage::default(),
        };
        mail.to = request.to;
        mail.cc = request.cc;
        mail.bcc = request.bcc;
        mail.subject = request.subject;
        mail.body = request.body;
        for id in request.files {
            let file = state
                .files
                .get(&tenant.workspace_id, &FileId::from(id))
                .await?
                .ok_or_else(|| ApiError::not_found("File"))?;
            let bytes = state
                .blobs
                .get(&BlobPath::from(file.storage_key))
                .await
                .map_err(|_| ApiError::internal())?
                .bytes()
                .await
                .map_err(|_| ApiError::internal())?;
            mail.attachments.push(pagis_mail::OutgoingAttachment {
                name: file.filename.unwrap_or_else(|| "attachment".into()),
                content_type: file.mime,
                data: bytes.to_vec(),
            });
        }
        if mail.to.is_empty() && mail.cc.is_empty() && mail.bcc.is_empty() {
            return Err(ApiError::validation("mail needs a recipient"));
        }
        let sent = state
            .mailbox_desk
            .send_workspace(&tenant.workspace_id, store.as_mut(), &mail)
            .await
            .map_err(mailbox_error)?;
        super::audit_read(
            state,
            tenant,
            "mail.sent",
            serde_json::json!({"message_id":sent.message_id}),
        )
        .await?;
        Ok((
            StatusCode::CREATED,
            Json(SentMailDto {
                message_id: sent.message_id,
            }),
        ))
    }
    .await;
    let _ = store.close().await;
    result
}
#[utoipa::path(post, path = "/api/v1/workspace/mail/messages", request_body = SendMailRequest, responses((status = 201, body = SentMailDto), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn send_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<SendMailRequest>,
) -> Result<(StatusCode, Json<SentMailDto>), ApiError> {
    send(&state, &tenant, None, request).await
}

#[utoipa::path(post, path = "/api/v1/workspace/mail/messages/{message_id}/reply", params(("message_id" = String, Path)), request_body = SendMailRequest, responses((status = 201, body = SentMailDto), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn reply_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Json(request): Json<SendMailRequest>,
) -> Result<(StatusCode, Json<SentMailDto>), ApiError> {
    send(&state, &tenant, Some((&id, "reply")), request).await
}

#[utoipa::path(post, path = "/api/v1/workspace/mail/messages/{message_id}/reply-all", params(("message_id" = String, Path)), request_body = SendMailRequest, responses((status = 201, body = SentMailDto), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn reply_all_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Json(request): Json<SendMailRequest>,
) -> Result<(StatusCode, Json<SentMailDto>), ApiError> {
    send(&state, &tenant, Some((&id, "reply-all")), request).await
}

#[utoipa::path(post, path = "/api/v1/workspace/mail/messages/{message_id}/forward", params(("message_id" = String, Path)), request_body = SendMailRequest, responses((status = 201, body = SentMailDto), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn forward_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Json(request): Json<SendMailRequest>,
) -> Result<(StatusCode, Json<SentMailDto>), ApiError> {
    send(&state, &tenant, Some((&id, "forward")), request).await
}

#[utoipa::path(post, path = "/api/v1/workspace/mail/folders", request_body = MailNameRequest, responses((status = 204), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn create_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<MailNameRequest>,
) -> Result<StatusCode, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = store.create_folder(&request.name).await;
    let _ = store.close().await;
    result.map_err(read_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(patch, path = "/api/v1/workspace/mail/folders/{name}", params(("name" = String, Path)), request_body = MailNameRequest, responses((status = 204), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn rename_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(name): Path<String>,
    Json(request): Json<MailNameRequest>,
) -> Result<StatusCode, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = store.rename_folder(&name, &request.name).await;
    let _ = store.close().await;
    result.map_err(read_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/api/v1/workspace/mail/folders/{name}", params(("name" = String, Path)), responses((status = 204), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn delete_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let _hold = pagis_core::LegalHold::lock(&tenant.workspace_id).await;
    state.workspaces.refuse_delete(&tenant.workspace_id).await?;
    let mut store = open(&state, &tenant).await?;
    let result = store.delete_folder(&name).await;
    let _ = store.close().await;
    result.map_err(read_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/api/v1/workspace/mail/messages/{message_id}", params(("message_id" = String, Path)), responses((status = 204), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn delete_workspace_mail_message(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let _hold = pagis_core::LegalHold::lock(&tenant.workspace_id).await;
    state.workspaces.refuse_delete(&tenant.workspace_id).await?;
    let mut store = open(&state, &tenant).await?;
    let result = store.delete_message(&MessageId::new(id)).await;
    let _ = store.close().await;
    result.map_err(read_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/workspace/mail/messages/{message_id}/move", params(("message_id" = String, Path)), request_body = MailNameRequest, responses((status = 204), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn move_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Json(request): Json<MailNameRequest>,
) -> Result<StatusCode, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = store.move_message(&MessageId::new(id), &request.name).await;
    let _ = store.close().await;
    result.map_err(read_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, path = "/api/v1/workspace/mail/messages/{message_id}/labels", params(("message_id" = String, Path)), request_body = MailNameRequest, responses((status = 204), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn label_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(id): Path<String>,
    Json(request): Json<MailNameRequest>,
) -> Result<StatusCode, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = store
        .set_label(&MessageId::new(id), &request.name, true)
        .await;
    let _ = store.close().await;
    result.map_err(read_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete, path = "/api/v1/workspace/mail/messages/{message_id}/labels/{label}", params(("message_id" = String, Path), ("label" = String, Path)), responses((status = 204), (status = 401, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)))]
pub async fn unlabel_workspace_mail(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((id, label)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = store.set_label(&MessageId::new(id), &label, false).await;
    let _ = store.close().await;
    result.map_err(read_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/v1/workspace/mail/messages/{message_id}/attachments/{index}", params(("message_id" = String, Path), ("index" = usize, Path)), responses((status = 200, content_type = "application/octet-stream")))]
pub async fn download_workspace_mail_attachment(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((id, index)): Path<(String, usize)>,
) -> Result<Response, ApiError> {
    let mut store = open(&state, &tenant).await?;
    let result = async {
        super::read_message(&state, &tenant, store.as_mut(), &id, "mail.attachment.read").await?;
        store
            .attachment(&MessageId::new(id), index)
            .await
            .map_err(read_error)
    }
    .await;
    let _ = store.close().await;
    let file = result?;
    Ok((
        crate::served_file::headers(&file.content_type, Some(&file.name)),
        file.data,
    )
        .into_response())
}
