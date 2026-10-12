//! The connection address that the daemon puts in each request.

use std::convert::Infallible;
use std::future::{Ready, ready};

use axum::serve::IncomingStream;
use axum::{Extension, Router};
use pagis_server::Peer;
use tokio::net::TcpListener;
use tower::{Layer, Service};

/// The router of one connection, which adds that connection's [`Peer`]
/// to each request.
type PeerRouter = <Extension<Peer> as Layer<Router>>::Service;

/// Set [`Peer`] from each accepted connection before the router runs. The
/// router is wrapped once per connection, as axum adds `ConnectInfo`.
pub fn peer_service(
    router: Router,
) -> impl for<'a> Service<
    IncomingStream<'a, TcpListener>,
    Response = PeerRouter,
    Error = Infallible,
    Future = Ready<Result<PeerRouter, Infallible>>,
> + Clone {
    tower::service_fn(move |stream: IncomingStream<'_, TcpListener>| {
        ready(Ok(
            Extension(Peer(*stream.remote_addr())).layer(router.clone())
        ))
    })
}

#[cfg(test)]
mod tests {
    use axum::extract::{ConnectInfo, FromRequestParts, Request};
    use axum::http::StatusCode;
    use axum::{Router, routing::get};
    use pagis_server::Peer;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tracing::instrument::WithSubscriber;

    use super::peer_service;

    #[tokio::test]
    async fn each_connection_sets_its_accepted_address_as_peer() {
        async fn address(Peer(address): Peer, request: Request) -> String {
            assert!(
                request
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .is_none()
            );
            address.to_string()
        }

        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
            let destination = listener.local_addr().unwrap();
            let router = Router::new().route("/", get(address));
            let server = tokio::spawn(async move {
                axum::serve(listener, peer_service(router)).await.unwrap();
            });
            let mut first = tokio::net::TcpStream::connect(destination).await.unwrap();
            let mut second = tokio::net::TcpStream::connect(destination).await.unwrap();
            let first_peer = first.local_addr().unwrap();
            let second_peer = second.local_addr().unwrap();
            assert_ne!(first_peer, second_peer);
            for (stream, expected) in [(&mut first, first_peer), (&mut second, second_peer)] {
                stream
                    .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
                let mut response = String::new();
                stream.read_to_string(&mut response).await.unwrap();
                assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
                assert!(response.ends_with(&expected.to_string()), "{response}");
            }
            server.abort();
            let _ = server.await;
        }
    }

    struct Log(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn missing_peer_returns_an_internal_error_and_logs_the_cause() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&bytes);
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || Log(Arc::clone(&output)))
            .finish();
        let (mut parts, _) = Request::new(axum::body::Body::empty()).into_parts();
        let error = Peer::from_request_parts(&mut parts, &())
            .with_subscriber(subscriber)
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        let log = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert!(log.contains("ERROR"), "{log}");
        assert!(log.contains("request has no Peer extension"), "{log}");
        assert!(
            log.contains("the process that serves the router must set it"),
            "{log}"
        );
    }
}
