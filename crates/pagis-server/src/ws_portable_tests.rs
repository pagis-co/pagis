//! The live events of a server without Hosts use an in-memory socket pair.

use super::*;
use futures::channel::mpsc;
use futures::{Sink, Stream};
use pagis_agent::{ProgressHub, StreamHub};
use pagis_core::{
    AgentId, Channel, ChannelKind, MessageId, NewEvent, RunId, UserRole, WorkspaceId,
};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use tracing::instrument::WithSubscriber;

struct Socket {
    incoming: mpsc::Receiver<Message>,
    outgoing: mpsc::Sender<Message>,
}

fn pair() -> (Socket, Socket) {
    let (a_send, b_receive) = mpsc::channel(32);
    let (b_send, a_receive) = mpsc::channel(32);
    (
        Socket {
            incoming: a_receive,
            outgoing: a_send,
        },
        Socket {
            incoming: b_receive,
            outgoing: b_send,
        },
    )
}

impl Stream for Socket {
    type Item = Result<Message, mpsc::SendError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.incoming)
            .poll_next(cx)
            .map(|frame| frame.map(Ok))
    }
}

impl Sink<Message> for Socket {
    type Error = mpsc::SendError;
    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.outgoing).poll_ready(cx)
    }
    fn start_send(mut self: Pin<&mut Self>, frame: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.outgoing).start_send(frame)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.outgoing).poll_flush(cx)
    }
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.outgoing).poll_close(cx)
    }
}

impl MessageSocket for Socket {
    type SocketError = mpsc::SendError;
}

#[derive(Clone, Default)]
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

async fn receive(socket: &mut Socket) -> serde_json::Value {
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Text(text) = frame else {
        panic!("the frame is not text")
    };
    serde_json::from_str(&text).unwrap()
}

async fn send(socket: &mut Socket, frame: serde_json::Value) {
    socket.send(Message::Text(frame.to_string())).await.unwrap();
}

#[tokio::test]
async fn host_frames_are_logged_once_per_connection_and_channel_events_still_arrive() {
    let database = pagis_core::fake::FakeDatabase::default();
    let stores = database.stores();
    let workspace_id = WorkspaceId::generate();
    let channel_id = ChannelId::generate();
    stores
        .channels
        .create(&Channel {
            id: channel_id.clone(),
            workspace_id: workspace_id.clone(),
            kind: ChannelKind::Group,
            title: Some("general".into()),
            created_at: 0,
            updated_at: 0,
        })
        .await
        .unwrap();
    let hub = Arc::new(StreamHub::default());
    let progress = Arc::new(ProgressHub::default());
    let activity = Arc::new(pagis_needs_you::PersonActivity::new());
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let ring = Arc::new(crate::EventRing::new(crate::RingConfig::default()));
    let bus: Arc<dyn pagis_core::EventBus> = Arc::new(crate::RingedBus::new(
        Arc::new(database.bus()),
        ring.clone(),
    ));
    for _ in 0..2 {
        let services = EventServices {
            bus: bus.clone(),
            events: stores.events.clone(),
            channels: stores.channels.clone(),
            ring: ring.clone(),
            hub: hub.clone(),
            progress: progress.clone(),
            person_activity: activity.clone(),
            clock: Arc::new(pagis_core::SystemClock),
        };
        let live = LiveTenant {
            tenant: Tenant {
                workspace_id: workspace_id.clone(),
                user_id: pagis_core::UserId::generate(),
                role: UserRole::Administrator,
                session_id: pagis_core::SessionId::generate(),
                session_expires_at: i64::MAX,
            },
            session_ended: tokio_util::sync::CancellationToken::new(),
        };
        let (mut client, server) = pair();
        let served = tokio::spawn(serve(server, services, live).with_subscriber(dispatch.clone()));
        send(&mut client, serde_json::json!({"type":"auth"})).await;
        assert_eq!(receive(&mut client).await["type"], "ready");
        for frame in [
            serde_json::json!({"type":"register_host","name":"Air","platform":"macos","capabilities":["harness:claude"]}),
            serde_json::json!({"type":"register_host","name":"","platform":""}),
            serde_json::json!({"type":"result","id":"command","exit_code":0}),
            serde_json::json!({"type":"harness_sign_in_result","id":"sign-in","exit_code":0}),
            serde_json::json!({"type":"harness_sign_in_state","harness":"claude","state":"signed_in"}),
            serde_json::json!({"type":"session_exit","session_id":"session","exit_code":0}),
        ] {
            send(&mut client, frame).await;
            send(&mut client, serde_json::json!({"type":"ping"})).await;
            assert_eq!(receive(&mut client).await["type"], "pong");
        }
        assert!(stores.hosts.list(&workspace_id).await.unwrap().is_empty());
        send(
            &mut client,
            serde_json::json!({"type":"subscribe","channel_id":channel_id}),
        )
        .await;
        send(&mut client, serde_json::json!({"type":"activity"})).await;
        send(&mut client, serde_json::json!({"type":"ping"})).await;
        assert_eq!(receive(&mut client).await["type"], "pong");
        assert!(
            activity
                .hold_until(&workspace_id, pagis_core::now_ms())
                .is_some()
        );
        let event = bus
            .publish(NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: "channel.changed".into(),
                channel_id: Some(channel_id.clone()),
                agent_id: None,
                run_id: None,
                payload: serde_json::json!({"title":"general"}),
            })
            .await
            .unwrap();
        let frame = receive(&mut client).await;
        assert_eq!(frame["type"], "channel.changed");
        assert_eq!(frame["seq"], event.id.as_str());
        assert_eq!(frame["payload"]["channel_id"], channel_id.as_str());
        let message_id = MessageId::generate();
        hub.begin(
            channel_id.clone(),
            message_id.clone(),
            RunId::generate(),
            AgentId::generate(),
            None,
        );
        hub.push(&message_id, "Channel text");
        let frame = receive(&mut client).await;
        assert_eq!(frame["type"], "message.delta");
        assert_eq!(frame["payload"]["text"], "Channel text");
        let run_id = RunId::generate();
        progress.begin(
            run_id.clone(),
            channel_id.clone(),
            message_id.clone(),
            AgentId::generate(),
            None,
            pagis_core::RunState::Running,
        );
        let frame = receive(&mut client).await;
        assert_eq!(frame["type"], "progress.state");
        assert_eq!(frame["payload"]["channel_id"], channel_id.as_str());
        progress.end(&run_id, pagis_core::RunState::Completed);
        assert_eq!(receive(&mut client).await["type"], "progress.state");
        hub.end(&message_id);
        send(
            &mut client,
            serde_json::json!({"type":"unsubscribe","channel_id":channel_id}),
        )
        .await;
        send(&mut client, serde_json::json!({"type":"ping"})).await;
        assert_eq!(receive(&mut client).await["type"], "pong");
        hub.begin(
            channel_id.clone(),
            message_id.clone(),
            RunId::generate(),
            AgentId::generate(),
            None,
        );
        hub.push(&message_id, "Not subscribed");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), client.next())
                .await
                .is_err()
        );
        hub.end(&message_id);
        client.send(Message::Close(None)).await.unwrap();
        served.await.unwrap().unwrap();
    }
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert_eq!(text.matches("Host frame dropped").count(), 2, "{text}");
}
