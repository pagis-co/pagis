//! The mail inspector's read (ADR-0019).
//!
//! One route answers the headers and the text body of one message, read
//! live over the transport of its mailbox. The daemon stores none of
//! it: a `mail` block carries the envelope, and the words stay at the
//! mail host until the user opens them.
//!
//! This is the user's own view of their Agent's mail and not the
//! model's, so the untrusted envelope of the tools does not apply here.
//! A message the host no longer holds says so.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use pagis_core::AgentMailbox;
use pagis_mail::{MessageId, TransportError, TransportErrorCode};
use serde::Serialize;
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;
use crate::mailboxes::mailbox_error;

/// What the inspector says when the message is not at the host any
/// more: the daemon keeps no copy of a body (ADR-0019).
const MESSAGE_GONE: &str = "this message is no longer at the host";

/// One header of a message, in the order the message carries them.
#[derive(Debug, Serialize, ToSchema)]
pub struct MailHeaderDto {
    pub name: String,
    pub value: String,
}

/// One attachment: the name and the size, never the bytes (ADR-0019).
#[derive(Debug, Serialize, ToSchema)]
pub struct MailAttachmentDto {
    pub name: String,
    pub bytes: u64,
}

/// One message as the inspector shows it. It is read live and stored
/// nowhere.
#[derive(Debug, Serialize, ToSchema)]
pub struct MailMessageDto {
    /// The mailbox it was read from, as the block names it.
    pub mailbox: String,
    /// The `folder:uid` id.
    pub message_id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    /// When the host received it.
    pub date: i64,
    pub headers: Vec<MailHeaderDto>,
    /// The text body. HTML became text at the transport.
    pub text: String,
    pub attachments: Vec<MailAttachmentDto>,
}

#[utoipa::path(get, path = "/api/v1/mail/{mailbox}/{message_id}",
    params(
        ("mailbox" = String, Path, description = "The mailbox address, as the mail block names it"),
        ("message_id" = String, Path, description = "The folder:uid id of the message"),
    ),
    responses(
        (status = 200, body = MailMessageDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
    )
)]
pub async fn get_mail_message(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path((mailbox, message_id)): Path<(String, String)>,
) -> Result<Json<MailMessageDto>, ApiError> {
    let record = mailbox_named(&state, &tenant, &mailbox).await?;
    // An id the daemon cannot read names no message in this mailbox,
    // which is what a sent message's id is: Pagis sent it and keeps no
    // copy of it.
    let id = message_id
        .parse::<MessageId>()
        .map_err(|_| gone(MESSAGE_GONE))?;
    let mut session = state
        .mailbox_desk
        .open_session(&record)
        .await
        .map_err(mailbox_error)?;
    let folder = session.select(&id.folder).await.map_err(transport_error)?;
    // A host that renumbers a folder gives every id of it another
    // message, so an id from the old numbering names nothing (ADR-0019).
    let renumbered = record.cursor.as_ref().is_some_and(|cursor| {
        cursor.folder == id.folder && cursor.uid_validity != folder.uid_validity
    });
    if renumbered {
        return Err(gone(MESSAGE_GONE));
    }
    let message = session.fetch(&id).await.map_err(transport_error)?;
    Ok(Json(MailMessageDto {
        mailbox: record.address,
        message_id: message.summary.id.to_string(),
        from: message.summary.from,
        to: message.summary.to,
        subject: message.summary.subject,
        date: message.summary.date,
        headers: message
            .headers
            .into_iter()
            .map(|(name, value)| MailHeaderDto { name, value })
            .collect(),
        text: message.text,
        attachments: message
            .attachments
            .into_iter()
            .map(|attachment| MailAttachmentDto {
                name: attachment.name,
                bytes: attachment.bytes,
            })
            .collect(),
    }))
}

/// The mailbox one `mail` block names: an Agent Mailbox of this
/// Workspace, by its address (ADR-0019).
async fn mailbox_named(
    state: &AppState,
    tenant: &Tenant,
    address: &str,
) -> Result<AgentMailbox, ApiError> {
    state
        .mailbox_desk
        .list(&tenant.workspace_id)
        .await
        .map_err(mailbox_error)?
        .into_iter()
        .find(|mailbox| mailbox.address.eq_ignore_ascii_case(address))
        .ok_or_else(|| ApiError::not_found("mailbox"))
}

/// A host that no longer holds the message says so; every other
/// failure keeps the words the mailbox card uses.
fn transport_error(error: TransportError) -> ApiError {
    match error.0 {
        TransportErrorCode::StaleId | TransportErrorCode::MailboxGone => gone(MESSAGE_GONE),
        TransportErrorCode::Unauthorized => {
            ApiError::conflict("the mail host refused the mailbox. Reset its password.".to_string())
        }
        TransportErrorCode::Unreachable | TransportErrorCode::Unreadable => {
            ApiError::conflict("the mail host did not answer. Try again later.".to_string())
        }
        TransportErrorCode::Rejected => {
            ApiError::conflict("the mail host refused the read.".to_string())
        }
    }
}

fn gone(message: &str) -> ApiError {
    ApiError {
        status: axum::http::StatusCode::NOT_FOUND,
        code: "not_found",
        message: message.to_string(),
    }
}
