//! Shared Workspace requests use the existing Workspace handlers.
use crate::error::ApiError;
use axum::{Router, body::Body, extract::Request, http::Uri, middleware::Next, response::Response};
use pagis_core::WorkspaceId;
use tower::ServiceExt;

#[derive(Clone)]
pub(crate) struct SharedWorkspace(pub WorkspaceId);

/// Public installation routes do not serve a Shared Workspace.
pub(crate) async fn unprefixed_only(request: Request, next: Next) -> Result<Response, ApiError> {
    if request.extensions().get::<SharedWorkspace>().is_some() {
        return Err(ApiError::not_found("route"));
    }
    Ok(next.run(request).await)
}

pub(crate) fn router(inner: Router) -> Router {
    Router::new().fallback_service(tower::service_fn(move |mut request: Request<Body>| {
        let inner = inner.clone();
        async move {
            if let Some(tail) = request.uri().path().strip_prefix("/api/v1/workspaces/")
                && let Some((id, rest)) = tail.split_once('/')
            {
                let workspace = WorkspaceId::from(id.to_string());
                let path = format!("/api/v1/{rest}");
                let query = request
                    .uri()
                    .query()
                    .map(|q| format!("?{q}"))
                    .unwrap_or_default();
                // Membership routes name their Workspace in the contract.
                if !rest.starts_with("members") {
                    *request.uri_mut() = format!("{path}{query}")
                        .parse::<Uri>()
                        .expect("a request path is a URI");
                }
                request.extensions_mut().insert(SharedWorkspace(workspace));
            }
            inner.oneshot(request).await
        }
    }))
}
