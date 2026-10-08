//! A WebSocket that carries one byte stream in its binary frames: the
//! exit socket (ADR-0029) and the session socket (ADR-0033) of a Host.
//!
//! [`carry`] joins the frames to one end of a pipe and hands the other
//! end to the daemon, which runs yamux over it. A text frame is a
//! protocol fault and ends the socket. The socket lives no longer than
//! its Session: at the end of the Session it closes with 1008, as the
//! Host socket does, and every stream that it carried ends with it.

use std::future::Future;

use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};
use tokio_util::sync::CancellationToken;

use crate::live_connections::close_for_ended_session;

/// How many bytes wait between the socket and the yamux of the daemon,
/// each way.
const PIPE_BYTES: usize = 64 * 1024;

/// Carry the bytes of `socket` to `serve` and back, until the client
/// closes it, `serve` ends, or the Session ends.
///
/// The two directions run apart. The yamux of the daemon reads no frame
/// while it holds the answer to a Ping and the pipe does not take it, so
/// one loop that waits to write into the pipe and only then reads from
/// it would wait for ever.
pub async fn carry<Serve, Served>(socket: WebSocket, session_ended: CancellationToken, serve: Serve)
where
    Serve: FnOnce(Compat<DuplexStream>) -> Served,
    Served: Future<Output = ()>,
{
    let (daemon_end, socket_end) = tokio::io::duplex(PIPE_BYTES);
    let served = serve(daemon_end.compat());
    let (mut sink, mut stream) = socket.split();
    let (pipe_reader, pipe_writer) = tokio::io::split(socket_end);
    let ended = tokio::select! {
        biased;
        () = session_ended.cancelled() => true,
        () = served => false,
        () = into_pipe(&mut stream, pipe_writer) => false,
        () = out_of_pipe(&mut sink, pipe_reader) => false,
    };
    // Every stream of the socket has ended here.
    if ended && let Ok(mut socket) = stream.reunite(sink) {
        let _ = close_for_ended_session(&mut socket).await;
    }
}

/// Copy the bytes of each binary frame into the pipe, until the client
/// closes the socket or sends a frame that is not binary.
async fn into_pipe(stream: &mut SplitStream<WebSocket>, mut pipe: WriteHalf<DuplexStream>) {
    while let Some(frame) = stream.next().await {
        match frame {
            Ok(WsMessage::Binary(bytes)) => {
                if pipe.write_all(&bytes).await.is_err() {
                    return;
                }
            }
            Ok(WsMessage::Ping(_) | WsMessage::Pong(_)) => {}
            // A close, a text frame, or an error.
            _ => return,
        }
    }
}

/// Send the bytes of the pipe in binary frames, until the pipe ends.
async fn out_of_pipe(sink: &mut SplitSink<WebSocket, WsMessage>, mut pipe: ReadHalf<DuplexStream>) {
    let mut buffer = vec![0; PIPE_BYTES];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(count) => {
                let frame = WsMessage::Binary(buffer[..count].to_vec().into());
                if sink.send(frame).await.is_err() {
                    return;
                }
            }
        }
    }
}
