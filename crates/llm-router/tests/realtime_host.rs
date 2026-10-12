//! The host supplies a realtime transport. The same contract runs under Node.

use futures::{SinkExt, StreamExt};
use llm_router::{Candidate, ProviderConfig, RealtimeMessage, Router, RouterConfig, TimeoutConfig};
use std::time::Duration;

#[path = "support/realtime.rs"]
mod realtime;
use realtime::MemoryTransport;

fn config() -> RouterConfig {
    let mut provider = ProviderConfig::new(
        llm_router::ProtocolKind::OpenAiChat,
        "https://provider.test/v1",
        "secret",
    );
    provider.headers.insert("x-host".into(), "test".into());
    RouterConfig::new()
        .provider("p", provider)
        .model("voice", [Candidate::new("p", "gpt-realtime-2.1")])
        .timeouts(TimeoutConfig {
            connect: Duration::from_secs(7),
            ..Default::default()
        })
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_host_transport_opens_a_session_and_passes_text_and_binary_both_ways() {
    let (transport, mut peer) = MemoryTransport::pair();
    let router = Router::new(config())
        .unwrap()
        .with_realtime_transport(transport.clone());
    let mut connection = router.realtime_connect("voice").await.unwrap();
    let requests = transport.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let (url, headers, timeout) = &requests[0];
    assert_eq!(
        url,
        "wss://provider.test/v1/realtime?model=gpt-realtime-2.1"
    );
    assert!(!url.contains("secret"));
    assert_eq!(headers["authorization"], "Bearer secret");
    assert!(headers["authorization"].is_sensitive());
    assert_eq!(headers["x-host"], "test");
    assert_eq!(*timeout, Duration::from_secs(7));

    for message in [
        RealtimeMessage::text("session"),
        RealtimeMessage::binary(vec![0, 255, 1]),
    ] {
        connection.send(message.clone()).await.unwrap();
        assert_eq!(peer.next().await.unwrap().unwrap(), message);
        peer.send(message.clone()).await.unwrap();
        assert_eq!(connection.next().await.unwrap().unwrap(), message);
    }
    peer.send(RealtimeMessage::Close).await.unwrap();
    assert_eq!(
        connection.next().await.unwrap().unwrap(),
        RealtimeMessage::Close
    );
    connection.close().await.unwrap();
    assert!(peer.next().await.is_none());
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen_test::wasm_bindgen_test]
async fn a_host_without_a_transport_gets_a_clear_error() {
    let router = Router::new(config()).unwrap();
    for error in [
        router.realtime_connect("voice").await.unwrap_err(),
        router
            .realtime_transcription_connect("voice", 24_000)
            .await
            .unwrap_err(),
    ] {
        assert!(
            error.to_string().contains("no realtime transport"),
            "{error}"
        );
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen_test::wasm_bindgen_test]
async fn a_host_transport_and_its_socket_accept_local_values() {
    use llm_router::{RealtimeConnectFuture, RealtimeSocket, RealtimeTransport};
    use reqwest::header::HeaderMap;
    use std::rc::Rc;
    use std::sync::Arc;

    struct LocalTransport {
        memory: Arc<MemoryTransport>,
        value: Rc<String>,
    }

    impl RealtimeTransport for LocalTransport {
        fn connect<'a>(
            &'a self,
            url: &'a str,
            headers: &'a HeaderMap,
            timeout: Duration,
        ) -> RealtimeConnectFuture<'a> {
            let value = self.value.clone();
            Box::pin(async move {
                let socket = self.memory.connect(url, headers, timeout).await?;
                let socket = socket.map(move |frame| {
                    assert_eq!(value.as_str(), "local");
                    frame
                });
                Ok(Box::pin(socket) as RealtimeSocket)
            })
        }
    }

    let (memory, mut peer) = MemoryTransport::pair();
    let transport = Arc::new(LocalTransport {
        memory,
        value: Rc::new("local".into()),
    });
    let router = Router::new(config())
        .unwrap()
        .with_realtime_transport(transport);
    let mut connection = router.realtime_connect("voice").await.unwrap();
    connection
        .send(RealtimeMessage::text("outbound"))
        .await
        .unwrap();
    assert_eq!(
        peer.next().await.unwrap().unwrap(),
        RealtimeMessage::text("outbound")
    );
    peer.send(RealtimeMessage::text("inbound")).await.unwrap();
    assert_eq!(
        connection.next().await.unwrap().unwrap(),
        RealtimeMessage::text("inbound")
    );
}
