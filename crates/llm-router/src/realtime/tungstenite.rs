//! The native WebSocket transport.

use std::time::Duration;

use futures::{SinkExt, StreamExt, future};
use reqwest::header::HeaderMap;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

use super::{
    RealtimeConnectFuture, RealtimeError, RealtimeMessage, RealtimeSocket, RealtimeTransport,
};

/// Opens a native WebSocket through Tokio and Tungstenite.
pub struct TungsteniteTransport;

impl RealtimeTransport for TungsteniteTransport {
    fn connect<'a>(
        &'a self,
        url: &'a str,
        headers: &'a HeaderMap,
        timeout: Duration,
    ) -> RealtimeConnectFuture<'a> {
        Box::pin(async move {
            let mut request = url.into_client_request()?;
            request.headers_mut().extend(headers.clone());
            let (socket, _) =
                crate::rt::timeout(timeout, tokio_tungstenite::connect_async(request))
                    .await
                    .map_err(|_| "timed out opening the WebSocket")??;
            let socket = socket
                .sink_map_err(|error| -> RealtimeError { Box::new(error) })
                .with(|frame| {
                    future::ready(Ok::<_, RealtimeError>(match frame {
                        RealtimeMessage::Text(text) => Message::Text(text.into()),
                        RealtimeMessage::Binary(bytes) => Message::Binary(bytes.into()),
                        RealtimeMessage::Close => Message::Close(None),
                    }))
                })
                .filter_map(|frame| {
                    future::ready(match frame {
                        Ok(Message::Text(text)) => {
                            Some(Ok(RealtimeMessage::Text(text.to_string())))
                        }
                        Ok(Message::Binary(bytes)) => {
                            Some(Ok(RealtimeMessage::Binary(bytes.to_vec())))
                        }
                        Ok(Message::Close(_)) => Some(Ok(RealtimeMessage::Close)),
                        // Tungstenite answers ping and close frames itself.
                        Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => None,
                        Err(error) => Some(Err(Box::new(error) as RealtimeError)),
                    })
                });
            Ok(Box::pin(socket) as RealtimeSocket)
        })
    }
}
