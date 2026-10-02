//! The exit socket of a Host (ADR-0029): the second WebSocket of a
//! Client App that carries the connections of its Person's Computers as
//! their Home Exit.
//!
//! The Client App opens it after its Host socket registered the
//! machine, with the id that the registration answered and the same
//! Session cookie. The Host must be of the Session's Workspace and must
//! declare the `exit` capability. Binary frames carry one byte stream,
//! and [`pagis_computer::HomeExits`] runs yamux over it: the daemon opens
//! one stream for each connection. A text frame is a protocol fault and
//! ends the socket.
//!
//! The Host is present as a Home Exit while this socket lives. The
//! socket lives no longer than its Session: at the end of the Session it
//! closes with 1008, as the Host socket does, and every connection that
//! it carried closes with it.

use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use pagis_core::{EXIT_CAPABILITY, HostId, WorkspaceId};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tokio_util::sync::CancellationToken;

use crate::AppState;
use crate::error::ApiError;
use crate::live_connections::{LiveTenant, close_for_ended_session};

/// How many bytes wait between the socket and the yamux of the daemon,
/// each way.
const PIPE_BYTES: usize = 64 * 1024;

/// Open the exit socket of one Host of the signed-in Person.
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
    if !host.can(EXIT_CAPABILITY) {
        return Err(ApiError::conflict(
            "this Host declared no `exit` capability, so it carries no exit traffic",
        ));
    }
    let session_ended = live.session_ended;
    Ok(ws.on_upgrade(move |socket| carry(state, workspace_id, host.id, session_ended, socket)))
}

/// Serve the socket as the Home Exit of its Host until the client
/// closes it, the yamux of the daemon ends, or the Session ends.
async fn carry(
    state: Arc<AppState>,
    workspace_id: WorkspaceId,
    host_id: HostId,
    session_ended: CancellationToken,
    mut socket: WebSocket,
) {
    let (daemon_end, socket_end) = tokio::io::duplex(PIPE_BYTES);
    let served = state
        .home_exits
        .serve(workspace_id, host_id, daemon_end.compat());
    let ended = tokio::select! {
        biased;
        () = session_ended.cancelled() => true,
        () = served => false,
        () = pump(&mut socket, socket_end) => false,
    };
    // The Host is absent as a Home Exit from here on, and every stream
    // of the socket has ended.
    if ended {
        let _ = close_for_ended_session(&mut socket).await;
    }
}

/// Copy the bytes of binary frames into the pipe, and the bytes of the
/// pipe into binary frames, until either side ends.
async fn pump(socket: &mut WebSocket, mut pipe: DuplexStream) {
    let mut buffer = vec![0; PIPE_BYTES];
    loop {
        tokio::select! {
            frame = socket.recv() => match frame {
                Some(Ok(WsMessage::Binary(bytes))) => {
                    if pipe.write_all(&bytes).await.is_err() {
                        return;
                    }
                }
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => {}
                // A close, a text frame, an error, or the end.
                _ => return,
            },
            read = pipe.read(&mut buffer) => match read {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    let frame = WsMessage::Binary(buffer[..count].to_vec().into());
                    if socket.send(frame).await.is_err() {
                        return;
                    }
                }
            },
        }
    }
}
