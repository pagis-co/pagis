//! Full-daemon tests of the Notifications (ADR-0030): an item that
//! enters the Needs-You Queue sends a Web Push to each Push Subscription
//! of its Workspace, and the daemon reads what the push service answers.
//!
//! Each push service is a `wiremock` server on loopback, which the test
//! sender reaches through `Policy::AllowLoopback`. The route takes no
//! loopback endpoint, so each test writes its Push Subscription through
//! the stores. Only the client decrypts a Web Push, so each test holds
//! the client keys and decrypts the body.
//!
//! A new item waits while the Person is active in a client: the tests
//! here show that an `activity` frame holds a push and that nothing else
//! does. The release of a held item after 120 s is a unit test of the
//! task, on paused time.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::SecretKey;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use pagis_core::{
    AgentId, ChannelId, ClientKind, PushSubscription, PushSubscriptionId, RunState,
    SESSION_LIFETIME_MS, Session, SessionId, UserId, WorkspaceId, now_ms,
};
use pagis_testkit::fixture::{pending_request, queued_run};
use pagis_testkit::{Socket, TestDaemon, TestDaemonOptions, TwoTenants};
use tokio_tungstenite::tungstenite::Message;
use web_push_native::Auth;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A daemon whose sender reaches a push service on loopback.
async fn daemon() -> TestDaemon {
    TestDaemon::start_with(options()).await
}

fn options() -> TestDaemonOptions {
    TestDaemonOptions {
        push_policy: pagis_push::Policy::AllowLoopback,
        ..TestDaemonOptions::default()
    }
}

/// A client of the Product App and the push service that holds its
/// endpoint. Only the client decrypts a Web Push.
struct PushService {
    server: MockServer,
    secret: SecretKey,
    auth: [u8; 16],
}

impl PushService {
    /// A push service that answers each Web Push with `answer`.
    async fn answering(answer: ResponseTemplate) -> Self {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(answer)
            .mount(&server)
            .await;
        Self::on(server)
    }

    fn on(server: MockServer) -> Self {
        let secret = loop {
            if let Ok(key) = SecretKey::from_slice(&rand::random::<[u8; 32]>()) {
                break key;
            }
        };
        Self {
            server,
            secret,
            auth: rand::random(),
        }
    }

    /// Write the Push Subscription of this client in `workspace_id`, on
    /// a new Session of `user_id`, as the route would.
    async fn subscribe(
        &self,
        daemon: &TestDaemon,
        user_id: &UserId,
        workspace_id: &WorkspaceId,
    ) -> PushSubscription {
        let stores = daemon.stores();
        let now = now_ms();
        let session = Session {
            id: SessionId::generate(),
            user_id: user_id.clone(),
            token_hash: format!("the hash of a phone of {user_id}"),
            client_kind: ClientKind::Browser,
            client_name: Some("Safari on iPhone".to_string()),
            created_at: now,
            last_used_at: now,
            expires_at: now + SESSION_LIFETIME_MS,
        };
        stores
            .sessions
            .create(&session)
            .await
            .expect("write the Session of the phone");
        stores
            .push_subscriptions
            .upsert(&PushSubscription {
                id: PushSubscriptionId::generate(),
                workspace_id: workspace_id.clone(),
                session_id: session.id,
                endpoint: format!("{}/push/{}", self.server.uri(), SessionId::generate()),
                p256dh: URL_SAFE_NO_PAD
                    .encode(self.secret.public_key().to_encoded_point(false).as_bytes()),
                auth: URL_SAFE_NO_PAD.encode(self.auth),
                created_at: now,
                last_sent_at: None,
            })
            .await
            .expect("write the Push Subscription")
    }

    async fn received(&self) -> Vec<Request> {
        self.server
            .received_requests()
            .await
            .expect("the push service records requests")
    }

    /// Wait until the push service received `count` Web Pushes, and
    /// answer them.
    async fn wait_for(&self, count: usize) -> Vec<Request> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let received = self.received().await;
                if received.len() >= count {
                    return received;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{count} Web Pushes before the timeout"))
    }

    /// The plaintext of a Web Push, as the client decrypts it.
    fn decrypt(&self, request: &Request) -> serde_json::Value {
        let plaintext = web_push_native::decrypt(
            request.body.clone(),
            &self.secret,
            &Auth::clone_from_slice(&self.auth),
        )
        .expect("the client decrypts the body");
        serde_json::from_slice(&plaintext).expect("the plaintext is JSON")
    }
}

fn header<'a>(request: &'a Request, name: &str) -> &'a str {
    request
        .headers
        .get(name)
        .unwrap_or_else(|| panic!("the Web Push has no {name} header"))
        .to_str()
        .expect("a text header")
}

/// Write a pending Request of the seeded Agent in `workspace_id`, with
/// no event, and answer its id.
async fn plant_pending_request(daemon: &TestDaemon, workspace_id: &WorkspaceId) -> String {
    let agent_id = AgentId::from(daemon.agent_id.clone());
    let run = queued_run(
        workspace_id,
        &agent_id,
        &ChannelId::from(daemon.dm_channel_id.clone()),
    );
    let stores = daemon.stores();
    stores
        .runs
        .create(&run)
        .await
        .expect("write the parked Run");
    let request = pending_request(workspace_id, &agent_id, &run.id, "echo hi");
    stores
        .requests
        .create(&request)
        .await
        .expect("write the pending Request");
    request.id.to_string()
}

/// Write a failed Run of today in `workspace_id`, with no event, and
/// answer its id.
async fn plant_failed_run(daemon: &TestDaemon, workspace_id: &WorkspaceId) -> String {
    let mut run = queued_run(
        workspace_id,
        &AgentId::from(daemon.agent_id.clone()),
        &ChannelId::from(daemon.dm_channel_id.clone()),
    );
    run.state = RunState::Failed;
    daemon
        .stores()
        .runs
        .create(&run)
        .await
        .expect("write the failed Run");
    run.id.to_string()
}

/// Clear the failed keypad codes of the Workspace of `cookie`, as
/// Settings does. The event makes the queue task read the records that
/// a test planted.
async fn clear_keypad_failures(daemon: &TestDaemon, cookie: &str) {
    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/settings/keypad-code/failures",
            daemon.base_url
        ))
        .header("cookie", cookie)
        .send()
        .await
        .expect("clear the failed keypad codes");
    assert_eq!(response.status(), 204);
}

/// Wait for the `needs_you.added` frame of the item `item_id`.
async fn wait_for_added(socket: &mut Socket, item_id: &str) {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), futures::StreamExt::next(socket))
            .await
            .expect("a frame before the timeout")
            .expect("the socket stays open")
            .expect("a readable frame");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).expect("the frame is JSON");
        if frame["type"] == "needs_you.added"
            && frame["payload"]["payload"]["item"]["id"] == item_id
        {
            return;
        }
    }
}

/// The time a queue task and a sender take for an event that sends
/// nothing, before a test says that nothing went.
const SETTLE: Duration = Duration::from_millis(500);

/// Send `frame` and then a `ping`, and wait for the `pong`. The daemon
/// reads the frames of one socket in order, so it read `frame` first.
async fn send_then_ping(socket: &mut Socket, frame: serde_json::Value) {
    use futures::SinkExt as _;

    for frame in [frame, serde_json::json!({"type": "ping"})] {
        socket
            .send(Message::text(frame.to_string()))
            .await
            .expect("the frame goes out");
    }
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), futures::StreamExt::next(socket))
            .await
            .expect("a frame before the timeout")
            .expect("the socket stays open")
            .expect("a readable frame");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).expect("the frame is JSON");
        assert_ne!(
            frame["type"], "error",
            "the daemon refused a frame: {frame}"
        );
        if frame["type"] == "pong" {
            return;
        }
    }
}

/// The Person uses the client of `socket` now.
async fn activity(socket: &mut Socket) {
    send_then_ping(socket, serde_json::json!({"type": "activity"})).await;
}

async fn subscriptions(daemon: &TestDaemon, workspace_id: &WorkspaceId) -> Vec<PushSubscription> {
    daemon
        .stores()
        .push_subscriptions
        .list(workspace_id)
        .await
        .expect("list the Push Subscriptions")
}

/// Wait until the stores hold `subscriptions` that satisfy `done`.
async fn wait_until_subscriptions(
    daemon: &TestDaemon,
    workspace_id: &WorkspaceId,
    done: impl Fn(&[PushSubscription]) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !done(&subscriptions(daemon, workspace_id).await) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the Push Subscriptions change before the timeout");
}

#[tokio::test]
async fn a_pending_request_sends_one_urgent_push_to_the_push_subscription() {
    let daemon = daemon().await;
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    service
        .subscribe(&daemon, &daemon.user_id, &daemon.workspace_id)
        .await;
    let request_id = plant_pending_request(&daemon, &daemon.workspace_id).await;
    let agent = daemon
        .stores()
        .agents
        .get(
            &daemon.workspace_id,
            &AgentId::from(daemon.agent_id.clone()),
        )
        .await
        .expect("read the Agent")
        .expect("the seeded Agent");

    clear_keypad_failures(&daemon, daemon.cookie()).await;

    let received = service.wait_for(1).await;
    let push = &received[0];
    assert_eq!(header(push, "urgency"), "high");
    assert_eq!(header(push, "ttl"), "86400");
    let topic = header(push, "topic");
    assert_eq!(topic.len(), 32, "{topic}");
    let message = service.decrypt(push);
    assert_eq!(message["web_push"], 8030, "{message}");
    assert_eq!(message["app_badge"], 1, "{message}");
    let notification = &message["notification"];
    assert_eq!(notification["title"], agent.name.as_str(), "{message}");
    assert_eq!(
        notification["navigate"],
        format!("{}/c/{}", daemon.public_origin, daemon.dm_channel_id)
    );
    assert_eq!(
        notification["data"],
        serde_json::json!({
            "v": 1,
            "item": format!("request:{request_id}"),
            "kind": "approval",
            "request": {"id": request_id, "actions": ["approve_once", "deny"]},
        })
    );
    // The plaintext names the place of the item, never the tool input.
    assert!(!message.to_string().contains("echo hi"), "{message}");
    wait_until_subscriptions(&daemon, &daemon.workspace_id, |rows| {
        rows.iter().all(|row| row.last_sent_at.is_some())
    })
    .await;
    tokio::time::sleep(SETTLE).await;
    assert_eq!(service.received().await.len(), 1);
}

#[tokio::test]
async fn a_push_service_that_answers_410_ends_the_push_subscription() {
    let daemon = daemon().await;
    let service = PushService::answering(ResponseTemplate::new(410)).await;
    service
        .subscribe(&daemon, &daemon.user_id, &daemon.workspace_id)
        .await;
    plant_failed_run(&daemon, &daemon.workspace_id).await;

    clear_keypad_failures(&daemon, daemon.cookie()).await;

    service.wait_for(1).await;
    wait_until_subscriptions(&daemon, &daemon.workspace_id, <[_]>::is_empty).await;
}

#[tokio::test]
async fn a_push_service_that_answers_429_gets_one_more_send_after_retry_after() {
    let daemon = daemon().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;
    let service = PushService::on(server);
    service
        .subscribe(&daemon, &daemon.user_id, &daemon.workspace_id)
        .await;
    plant_failed_run(&daemon, &daemon.workspace_id).await;

    let started = std::time::Instant::now();
    clear_keypad_failures(&daemon, daemon.cookie()).await;

    let received = service.wait_for(2).await;
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "the second send waits for Retry-After"
    );
    assert_eq!(service.decrypt(&received[0]), service.decrypt(&received[1]));
    wait_until_subscriptions(&daemon, &daemon.workspace_id, |rows| {
        rows.iter().all(|row| row.last_sent_at.is_some())
    })
    .await;
}

#[tokio::test]
async fn a_workspace_with_no_push_subscription_sends_nothing() {
    let daemon = daemon().await;
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;
    let first = plant_failed_run(&daemon, &daemon.workspace_id).await;

    clear_keypad_failures(&daemon, daemon.cookie()).await;
    wait_for_added(&mut socket, &format!("run:{first}")).await;
    tokio::time::sleep(SETTLE).await;

    assert!(service.received().await.is_empty());
    // A Push Subscription made later gets the next item and only it.
    service
        .subscribe(&daemon, &daemon.user_id, &daemon.workspace_id)
        .await;
    let second = plant_failed_run(&daemon, &daemon.workspace_id).await;
    clear_keypad_failures(&daemon, daemon.cookie()).await;
    let received = service.wait_for(1).await;
    assert_eq!(
        service.decrypt(&received[0])["notification"]["data"]["item"],
        format!("run:{second}")
    );
    tokio::time::sleep(SETTLE).await;
    assert_eq!(service.received().await.len(), 1);
}

#[tokio::test]
async fn the_push_subscription_of_another_workspace_gets_nothing() {
    let world = TwoTenants::on(daemon().await).await;
    let daemon = &world.daemon;
    let b_service = PushService::answering(ResponseTemplate::new(201)).await;
    b_service
        .subscribe(daemon, &world.b.user_id, &world.b.workspace_id)
        .await;
    let mut a_socket = daemon.event_socket(&world.a.cookie).await;

    let a_run = plant_failed_run(daemon, &world.a.workspace_id).await;
    clear_keypad_failures(daemon, &world.a.cookie).await;
    wait_for_added(&mut a_socket, &format!("run:{a_run}")).await;
    tokio::time::sleep(SETTLE).await;

    assert!(b_service.received().await.is_empty());
    let b_run = plant_failed_run(daemon, &world.b.workspace_id).await;
    clear_keypad_failures(daemon, &world.b.cookie).await;
    let received = b_service.wait_for(1).await;
    let message = b_service.decrypt(&received[0]);
    assert_eq!(
        message["notification"]["data"]["item"],
        format!("run:{b_run}")
    );
    assert_eq!(message["app_badge"], 1);
}

async fn send_test(daemon: &TestDaemon, cookie: &str, id: &str) -> (u16, serde_json::Value) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/push-subscriptions/{id}/test",
            daemon.base_url
        ))
        .header("cookie", cookie)
        .send()
        .await
        .expect("send the test Notification");
    let status = response.status().as_u16();
    (
        status,
        response.json().await.unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn the_test_route_sends_the_test_notification_and_answers_the_outcome() {
    let daemon = daemon().await;
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    let row = service
        .subscribe(&daemon, &daemon.user_id, &daemon.workspace_id)
        .await;

    let (status, body) = send_test(&daemon, daemon.cookie(), row.id.as_str()).await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body, serde_json::json!({"outcome": "delivered"}));
    let received = service.received().await;
    assert_eq!(received.len(), 1);
    assert_eq!(
        service.decrypt(&received[0]),
        serde_json::json!({
            "web_push": 8030,
            "notification": {
                "title": "Pagis",
                "body": "Notifications work here.",
                "navigate": format!("{}/settings/notifications", daemon.public_origin),
                "data": {"v": 1, "item": "test", "kind": "test"},
            },
            "mutable": true,
        })
    );
    assert!(
        subscriptions(&daemon, &daemon.workspace_id).await[0]
            .last_sent_at
            .is_some()
    );
}

#[tokio::test]
async fn the_test_route_answers_a_gone_push_subscription_and_ends_it() {
    let daemon = daemon().await;
    let service = PushService::answering(ResponseTemplate::new(410)).await;
    let row = service
        .subscribe(&daemon, &daemon.user_id, &daemon.workspace_id)
        .await;

    let (status, body) = send_test(&daemon, daemon.cookie(), row.id.as_str()).await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body, serde_json::json!({"outcome": "gone"}));
    assert!(
        subscriptions(&daemon, &daemon.workspace_id)
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn the_test_route_reads_a_push_subscription_of_another_person_as_absent() {
    let world = TwoTenants::start().await;

    let (status, _) = send_test(
        &world.daemon,
        &world.b.cookie,
        world.a_id("push_subscription_id"),
    )
    .await;

    assert_eq!(status, 404);
}

#[tokio::test]
async fn an_activity_frame_holds_the_push_of_a_new_request() {
    let daemon = daemon().await;
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    service
        .subscribe(&daemon, &daemon.user_id, &daemon.workspace_id)
        .await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;
    activity(&mut socket).await;

    let request_id = plant_pending_request(&daemon, &daemon.workspace_id).await;
    clear_keypad_failures(&daemon, daemon.cookie()).await;
    wait_for_added(&mut socket, &format!("request:{request_id}")).await;
    tokio::time::sleep(SETTLE).await;

    assert!(service.received().await.is_empty());
}

#[tokio::test]
async fn a_client_that_only_pings_holds_no_push() {
    let daemon = daemon().await;
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    service
        .subscribe(&daemon, &daemon.user_id, &daemon.workspace_id)
        .await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;
    send_then_ping(&mut socket, serde_json::json!({"type": "ping"})).await;

    let request_id = plant_pending_request(&daemon, &daemon.workspace_id).await;
    clear_keypad_failures(&daemon, daemon.cookie()).await;

    let received = service.wait_for(1).await;
    assert_eq!(
        service.decrypt(&received[0])["notification"]["data"]["item"],
        format!("request:{request_id}")
    );
}

#[tokio::test]
async fn an_activity_frame_of_one_workspace_holds_nothing_of_another() {
    let world = TwoTenants::on(daemon().await).await;
    let daemon = &world.daemon;
    let b_service = PushService::answering(ResponseTemplate::new(201)).await;
    b_service
        .subscribe(daemon, &world.b.user_id, &world.b.workspace_id)
        .await;
    let mut a_socket = daemon.event_socket(&world.a.cookie).await;
    activity(&mut a_socket).await;

    let b_run = plant_failed_run(daemon, &world.b.workspace_id).await;
    clear_keypad_failures(daemon, &world.b.cookie).await;

    let received = b_service.wait_for(1).await;
    assert_eq!(
        b_service.decrypt(&received[0])["notification"]["data"]["item"],
        format!("run:{b_run}")
    );
}

#[tokio::test]
async fn an_activity_frame_does_not_hold_the_test_notification() {
    let daemon = daemon().await;
    let service = PushService::answering(ResponseTemplate::new(201)).await;
    let row = service
        .subscribe(&daemon, &daemon.user_id, &daemon.workspace_id)
        .await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;
    activity(&mut socket).await;

    let (status, body) = send_test(&daemon, daemon.cookie(), row.id.as_str()).await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(body, serde_json::json!({"outcome": "delivered"}));
    assert_eq!(service.received().await.len(), 1);
}
