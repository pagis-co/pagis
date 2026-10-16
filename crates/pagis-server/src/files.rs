//! Files REST: multipart upload with content dedup, and byte
//! download through the daemon. Bytes live in the blob store; the row
//! is metadata only. A download shows only a passive type inline and
//! serves every other type as bytes to save (`served_file`).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use object_store::ObjectStoreExt as _;
use object_store::path::Path as BlobPath;
use pagis_core::{File, FileId, FileOutcome, now_ms};
use serde::Serialize;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;
use crate::served_file;

/// Body-limit headroom for the multipart framing around the file.
pub const MULTIPART_OVERHEAD: usize = 64 * 1024;

#[derive(Debug, Serialize, ToSchema)]
pub struct FileDto {
    pub id: String,
    pub filename: Option<String>,
    pub mime: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub created_at: i64,
}

impl From<File> for FileDto {
    fn from(a: File) -> Self {
        FileDto {
            id: a.id.to_string(),
            filename: a.filename,
            mime: a.mime,
            size_bytes: a.size_bytes,
            sha256: a.sha256,
            created_at: a.created_at,
        }
    }
}

/// The multipart upload shape: one `file` part with the bytes.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct UploadForm {
    #[schema(format = Binary, content_media_type = "application/octet-stream")]
    file: String,
}

fn multipart_error(err: axum::extract::multipart::MultipartError, max_bytes: u64) -> ApiError {
    if err.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::payload_too_large(max_bytes);
    }
    ApiError::validation(format!("malformed multipart upload: {}", err.body_text()))
}

fn blob_error(err: object_store::Error) -> ApiError {
    tracing::error!(error = %err, "blob store error");
    ApiError::internal()
}

#[utoipa::path(
    post,
    path = "/api/v1/files",
    request_body(content = UploadForm, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "File stored", body = FileDto),
        (status = 200, description = "Duplicate content; the original file", body = FileDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 413, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn upload_file(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<FileDto>), ApiError> {
    let max_bytes = state.file_max_bytes;
    let field = multipart
        .next_field()
        .await
        .map_err(|e| multipart_error(e, max_bytes))?
        .ok_or_else(|| ApiError::validation("upload needs one file part"))?;

    let filename = field.file_name().map(str::to_owned);
    let mime = field
        .content_type()
        .unwrap_or("application/octet-stream")
        .to_owned();
    let data = field
        .bytes()
        .await
        .map_err(|e| multipart_error(e, max_bytes))?;
    if data.len() as u64 > max_bytes {
        return Err(ApiError::payload_too_large(max_bytes));
    }
    if data.is_empty() {
        return Err(ApiError::validation("uploaded file is empty"));
    }

    let sha256 = format!("{:x}", Sha256::digest(&data));
    let storage_key = format!("{}/{sha256}", tenant.workspace_id);
    let file = File {
        id: FileId::generate(),
        workspace_id: tenant.workspace_id.clone(),
        creator_agent_id: None,
        run_id: None,
        kind: pagis_core::FileKind::Other,
        filename,
        mime,
        size_bytes: data.len() as i64,
        sha256,
        storage_key: storage_key.clone(),
        created_at: now_ms(),
    };

    // Write the bytes first so a stored row always has its blob. A
    // dedup hit rewrites the same content under the same key.
    state
        .blobs
        .put(&BlobPath::from(storage_key), data.into())
        .await
        .map_err(blob_error)?;

    match state.files.insert(&file).await? {
        FileOutcome::Created(file) => Ok((StatusCode::CREATED, Json(file.into()))),
        FileOutcome::Deduplicated(original) => Ok((StatusCode::OK, Json(original.into()))),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/files/{file_id}",
    params(("file_id" = String, Path,)),
    responses(
        (status = 200, description = "The file bytes. A passive image, audio, video or plain-text type shows inline; every other type is an `application/octet-stream` attachment.", content_type = "application/octet-stream"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn download_file(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(file_id): Path<String>,
) -> Result<Response, ApiError> {
    let file = state
        .files
        .get(&tenant.workspace_id, &FileId::from(file_id))
        .await?
        .ok_or_else(|| ApiError::not_found("file"))?;
    let bytes = state
        .blobs
        .get(&BlobPath::from(file.storage_key.clone()))
        .await
        .map_err(blob_error)?
        .bytes()
        .await
        .map_err(blob_error)?;

    Ok((
        served_file::headers(&file.mime, file.filename.as_deref()),
        bytes,
    )
        .into_response())
}
