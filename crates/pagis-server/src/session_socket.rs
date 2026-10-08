//! The session socket of a Host (ADR-0033): a further WebSocket of a
//! Client App that carries one stream for each Coding Session on the
//! machine.
//!
//! The Client App opens it after its Host socket registered the
//! machine, with the id that the registration answered and the same
//! Session cookie. The Host must be of the Session's Workspace and must
//! declare at least one `harness:<id>` capability. [`crate::byte_socket`]
//! carries the bytes, and [`pagis_broker::HostSessions`] runs yamux over
//! them: the daemon opens one stream for each Coding Session.
//!
//! At the end of the Session the socket closes with 1008, and every
//! Coding Session that it carried loses its place.

use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::response::Response;
use pagis_core::HostId;

use crate::AppState;
use crate::byte_socket;
use crate::error::ApiError;
use crate::live_connections::LiveTenant;

/// Open the session socket of one Host of the signed-in Person.
pub async fn upgrade(
    State(state): State<Arc<AppState>>,
    live: LiveTenant,
    Path(host_id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let workspace_id = live.tenant.workspace_id.clone();
    // The read names the Workspace, so the Host of another Person is
    // absent here.
    let host = state
        .hosts
        .get(&workspace_id, &HostId::from(host_id))
        .await?
        .ok_or_else(|| ApiError::not_found("host"))?;
    if host.harnesses().is_empty() {
        return Err(ApiError::conflict(
            "this Host declared no `harness:<id>` capability, so it starts no Coding Session",
        ));
    }
    let session_ended = live.session_ended;
    Ok(ws.on_upgrade(move |socket| {
        byte_socket::carry(socket, session_ended, move |daemon_end| async move {
            state
                .host_sessions
                .serve(workspace_id, host.id, daemon_end)
                .await
        })
    }))
}
