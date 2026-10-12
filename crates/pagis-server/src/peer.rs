//! The address of the connection that carries a request.

use std::net::SocketAddr;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::error::ApiError;

/// The process that serves the router sets this extension from the
/// address of each accepted connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer(pub SocketAddr);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Self>().copied().ok_or_else(|| {
            tracing::error!(
                "request has no Peer extension; the process that serves the router must set it"
            );
            ApiError::internal()
        })
    }
}
