//! The API error shape: `{error: {code, message}}` with a stable
//! string code.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorDetail {
    /// Stable machine-readable code: `unauthorized`, `forbidden`,
    /// `not_found`, `validation`, `conflict`, `payload_too_large`,
    /// `internal`.
    pub code: String,
    pub message: String,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn unauthorized() -> Self {
        ApiError {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "missing or invalid session".to_string(),
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: message.into(),
        }
    }

    /// The answer for a route of an absent Native Part.
    pub fn not_available(part: &str) -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            code: "not_available",
            message: format!("This server does not offer {part}."),
        }
    }

    pub fn not_found(what: &str) -> Self {
        ApiError {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: format!("{what} not found"),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "validation",
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        ApiError {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
        }
    }

    pub fn payload_too_large(max_bytes: u64) -> Self {
        ApiError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "payload_too_large",
            message: format!("upload exceeds the {max_bytes}-byte limit"),
        }
    }

    pub fn internal() -> Self {
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: "internal error".to_string(),
        }
    }
}

impl From<pagis_core::CalendarError> for ApiError {
    fn from(error: pagis_core::CalendarError) -> Self {
        Self::validation(error.to_string())
    }
}

impl From<pagis_core::StoreError> for ApiError {
    fn from(err: pagis_core::StoreError) -> Self {
        match err {
            pagis_core::StoreError::LegalHold => {
                return Self::conflict(pagis_core::LegalHold::MESSAGE);
            }
            pagis_core::StoreError::Conflict(message) => return Self::conflict(message),
            _ => {}
        }
        tracing::error!(error = %err, "store error");
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: "internal error".to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code.to_string(),
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod calendar_tests {
    use super::*;

    #[test]
    fn calendar_input_errors_are_validation_errors() {
        let error = ApiError::from(pagis_core::CalendarError::Invalid("Invalid range.".into()));
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.code, "validation");
        assert_eq!(error.message, "Invalid range.");
    }
}
