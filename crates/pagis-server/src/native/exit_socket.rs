//! The exit socket of a Host (ADR-0029): the second WebSocket of a
//! Client App that carries the connections of its Person's Computers as
//! their Home Exit.
//!
//! The Client App opens it after its Host socket registered the
//! machine, with the id that the registration answered and the same
//! Session cookie. The Host must be of the Session's Workspace and must
//! declare the `exit` capability. [`crate::native::byte_socket`] carries the
//! bytes, and [`pagis_computer::HomeExits`] runs yamux over them: the
//! daemon opens one stream for each connection.
//!
//! The Host is present as a Home Exit while this socket lives. At the end
//! of the Session the socket closes with 1008, and every connection that
//! it carried closes with it.

use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::response::Response;
use pagis_core::{EXIT_CAPABILITY, Host, HostId};

use crate::auth::Tenant;
use crate::axum_socket::AxumSocket;
use crate::error::ApiError;
use crate::live_connections::LiveTenant;
use crate::native::byte_socket;
use crate::{AppState, MessageSocket};

/// Open the exit socket of one Host of the signed-in Person.
pub async fn upgrade(
    State(state): State<Arc<AppState>>,
    live: LiveTenant,
    Path(host_id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let host = check_host(&state, &live.tenant, &HostId::from(host_id)).await?;
    Ok(ws.on_upgrade(move |socket| serve_exit(AxumSocket(socket), state, live, host)))
}

/// Check the Workspace and the capability before a host serves frames.
pub async fn check_host(
    state: &AppState,
    tenant: &Tenant,
    host_id: &HostId,
) -> Result<Host, ApiError> {
    tenant.require_private("Hosts")?;
    let workspace_id = &tenant.workspace_id;
    // The read names the Workspace, so the Host of another Person is
    // absent here.
    let host = state
        .hosts
        .get(workspace_id, host_id)
        .await?
        .ok_or_else(|| ApiError::not_found("host"))?;
    if !host.can(EXIT_CAPABILITY) {
        return Err(ApiError::conflict(
            "this Host declared no `exit` capability, so it carries no exit traffic",
        ));
    }
    Ok(host)
}

/// Serve the binary frames of the Host until its Session ends.
pub async fn serve_exit(
    socket: impl MessageSocket,
    state: Arc<AppState>,
    live: LiveTenant,
    host: Host,
) {
    let workspace_id = live.tenant.workspace_id;
    byte_socket::carry(socket, live.session_ended, move |daemon_end| async move {
        state
            .native
            .home_exits
            .serve(workspace_id, host.id, host.name, daemon_end)
            .await
    })
    .await;
}
