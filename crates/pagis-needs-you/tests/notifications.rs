//! An Embedder runs the Notification rule with its own clock and timer.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{
    Agent, AgentId, AgentStatus, Clock, Event, EventId, PushSubscription, PushSubscriptionId,
    Request, RequestId, RequestState, SessionId, Stores, UnixMillis, WorkspaceId, fake,
};
use pagis_needs_you::{
    Action, HeldItem, NEEDS_YOU_ADDED, NeedsYou, NotificationRule, NotificationSender,
    Notifications, PersonActivity,
};
use pagis_push::{Options, Outcome, Subscription, Urgency};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct FakeClock(Mutex<UnixMillis>);

impl Clock for FakeClock {
    fn now_ms(&self) -> UnixMillis {
        *self.0.lock().unwrap()
    }
}

#[derive(Default)]
struct FakeSender {
    sent: Mutex<Vec<(Subscription, Value, Options)>>,
    outcomes: Mutex<HashMap<String, VecDeque<Outcome>>>,
}

#[async_trait]
impl NotificationSender for FakeSender {
    async fn send(
        &self,
        subscription: &Subscription,
        plaintext: &[u8],
        options: Options,
    ) -> Outcome {
        self.sent.lock().unwrap().push((
            subscription.clone(),
            serde_json::from_slice(plaintext).unwrap(),
            options,
        ));
        self.outcomes
            .lock()
            .unwrap()
            .get_mut(&subscription.endpoint)
            .and_then(VecDeque::pop_front)
            .unwrap_or(Outcome::Delivered)
    }
}

struct Embedder {
    stores: Stores,
    workspace: WorkspaceId,
    clock: Arc<FakeClock>,
    sender: Arc<FakeSender>,
    notifications: Arc<Notifications>,
}

impl Embedder {
    async fn new() -> Self {
        let stores = fake::stores();
        let workspace = WorkspaceId::from("workspace".to_string());
        stores
            .workspaces
            .create(&pagis_core::Workspace {
                kind: pagis_core::WorkspaceKind::Private,
                monthly_spend_cap_usd: None,
                id: workspace.clone(),
                user_id: Some(pagis_core::UserId::from(workspace.as_str().to_string())),
                name: "Private".into(),
                timezone: "UTC".into(),
                created_at: 0,
                onboarded_at: None,
                chief_of_staff_agent_id: None,
                report_schedule_id: None,
                home_exit_host_id: None,
            })
            .await
            .unwrap();
        let clock = Arc::new(FakeClock::default());
        let sender = Arc::new(FakeSender::default());
        stores
            .agents
            .create(&Agent {
                id: AgentId::from("agent".to_string()),
                workspace_id: workspace.clone(),
                name: "Robin".into(),
                job: String::new(),
                description: String::new(),
                personality: String::new(),
                model_alias: String::new(),
                avatar: Default::default(),
                voice: None,
                standing_brief: None,
                status: AgentStatus::Active,
                created_at: 0,
                updated_at: 0,
            })
            .await
            .unwrap();
        let notifications = Arc::new(Notifications::new(
            sender.clone(),
            "https://pagis.example.com".into(),
            stores.push_subscriptions.clone(),
            stores.agents.clone(),
            stores.workspace_members.clone(),
            Arc::new(NeedsYou::new(&stores)),
            clock.clone(),
        ));
        Self {
            stores,
            workspace,
            clock,
            sender,
            notifications,
        }
    }

    async fn request(&self, id: &str) -> Request {
        let request = Request {
            decided_by: None,
            id: RequestId::from(id.to_string()),
            workspace_id: self.workspace.clone(),
            agent_id: AgentId::from("agent".to_string()),
            run_id: None,
            kind: Request::TOOL_ACTION_KIND.into(),
            payload: json!({"action_title": "Open a file", "body": "private tool input"}),
            state: RequestState::Pending,
            values: None,
            decided_at: None,
            created_at: self.clock.now_ms(),
        };
        self.stores.requests.create(&request).await.unwrap();
        request
    }

    async fn subscribe(&self, workspace: &WorkspaceId, id: &str) -> PushSubscription {
        self.stores
            .push_subscriptions
            .upsert(&PushSubscription {
                id: PushSubscriptionId::from(id.to_string()),
                user_id: pagis_core::UserId::from(workspace.as_str().to_string()),
                session_id: SessionId::from(format!("session-{id}")),
                endpoint: format!("https://push.example.com/{id}"),
                p256dh: format!("key-{id}"),
                auth: format!("auth-{id}"),
                created_at: 0,
                last_sent_at: None,
            })
            .await
            .unwrap()
    }
}

/// Start the public send in a task of its own, as an Embedder that
/// keeps its process open until the send ends.
fn start_send(
    notifications: &Arc<Notifications>,
    workspace: &WorkspaceId,
    queued: &pagis_needs_you::QueuedItem,
    cancel: &CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let notifications = Arc::clone(notifications);
    let workspace = workspace.clone();
    let queued = queued.clone();
    let cancel = cancel.clone();
    tokio::spawn(async move {
        notifications
            .notify(
                &workspace,
                &pagis_core::UserId::from(workspace.as_str().to_string()),
                &queued.item,
                queued.count,
                &cancel,
            )
            .await;
    })
}

/// Let a started send run on paused runtime time. The rule uses
/// FakeClock.
async fn let_the_send_run() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

#[tokio::test(start_paused = true)]
async fn an_embedder_sends_a_held_notification_with_the_current_item_and_count() {
    let embedder = Embedder::new().await;
    let delivered = embedder.subscribe(&embedder.workspace, "delivered").await;
    let also_delivered = embedder
        .subscribe(&embedder.workspace, "also-delivered")
        .await;
    let gone = embedder.subscribe(&embedder.workspace, "gone").await;
    let other_workspace = WorkspaceId::from("other".to_string());
    let other = embedder.subscribe(&other_workspace, "other").await;
    embedder
        .sender
        .outcomes
        .lock()
        .unwrap()
        .insert(gone.endpoint.clone(), VecDeque::from([Outcome::Gone]));
    embedder.request("first").await;
    let queued = embedder
        .notifications
        .queued(&embedder.workspace, "request:first")
        .await
        .unwrap();
    let activity = Arc::new(PersonActivity::new());
    activity.record(
        &pagis_core::UserId::from(embedder.workspace.as_str().to_string()),
        embedder.clock.now_ms(),
    );
    let mut rule = NotificationRule::new(
        embedder.workspace.clone(),
        pagis_core::UserId::from(embedder.workspace.as_str().to_string()),
        activity,
    );
    let event = Event {
        id: EventId::from("added".to_string()),
        seq: 1,
        workspace_id: embedder.workspace.clone(),
        event_type: NEEDS_YOU_ADDED.into(),
        agent_id: None,
        run_id: None,
        channel_id: None,
        payload: json!({"item": queued.item, "count": queued.count}),
        created_at: embedder.clock.now_ms(),
    };
    assert_eq!(
        rule.on_event(&event, embedder.clock.now_ms()),
        vec![Action::Hold {
            item_id: "request:first".into(),
            until: 120_000,
        }]
    );
    assert!(embedder.sender.sent.lock().unwrap().is_empty());

    embedder.request("second").await;
    let mut agent = embedder
        .stores
        .agents
        .get(&embedder.workspace, &AgentId::from("agent".to_string()))
        .await
        .unwrap()
        .unwrap();
    agent.name = "Sage".into();
    embedder.stores.agents.update(&agent).await.unwrap();
    *embedder.clock.0.lock().unwrap() = 120_000;
    let current = embedder
        .notifications
        .queued(&embedder.workspace, "request:first")
        .await
        .unwrap();
    assert_eq!(current.count, 2);
    assert_eq!(current.item.line(), "Sage needs your approval");
    let actions = rule.on_hold_end(
        HeldItem {
            item_id: "request:first".into(),
            queued: Some(current.clone()),
        },
        embedder.clock.now_ms(),
    );
    assert_eq!(
        actions,
        vec![
            Action::EndHold {
                item_id: "request:first".into()
            },
            Action::Send(current)
        ]
    );
    for action in actions {
        if let Action::Send(queued) = action {
            embedder
                .notifications
                .notify(
                    &embedder.workspace,
                    &pagis_core::UserId::from(embedder.workspace.as_str().to_string()),
                    &queued.item,
                    queued.count,
                    &CancellationToken::new(),
                )
                .await;
        }
    }

    // The send has ended when `notify` returns.
    let sent = embedder.sender.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 3);
    for row in [&delivered, &also_delivered, &gone] {
        let (subscription, message, options) = sent
            .iter()
            .find(|(subscription, _, _)| subscription.endpoint == row.endpoint)
            .unwrap();
        assert_eq!(subscription.p256dh, row.p256dh);
        assert_eq!(subscription.auth, row.auth);
        assert_eq!(
            message,
            &json!({
                "web_push": 8030,
                "notification": {
                    "title": "Sage",
                    "body": "Sage needs your approval\nOpen a file",
                    "navigate": "https://pagis.example.com/",
                    "data": {"v": 1, "item": "request:first", "kind": "approval", "request": {"id": "first", "actions": ["approve_once", "deny"]}},
                },
                "app_badge": 2,
                "mutable": true,
            })
        );
        assert_eq!(options.ttl, Duration::from_secs(86_400));
        assert_eq!(options.urgency, Urgency::High);
        assert_eq!(options.topic.as_ref().unwrap().as_str().len(), 32);
    }
    let remaining = embedder
        .stores
        .push_subscriptions
        .list(&pagis_core::UserId::from(
            embedder.workspace.as_str().to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(remaining.len(), 2);
    for row in remaining {
        assert!([delivered.id.clone(), also_delivered.id.clone()].contains(&row.id));
        assert_eq!(row.last_sent_at, Some(120_000));
    }
    assert_eq!(
        embedder
            .stores
            .push_subscriptions
            .list(&pagis_core::UserId::from(
                other_workspace.as_str().to_string()
            ))
            .await
            .unwrap(),
        vec![other]
    );
}

#[tokio::test(start_paused = true)]
async fn a_queue_read_returns_none_after_a_request_is_decided_or_for_another_workspace() {
    let embedder = Embedder::new().await;
    let request = embedder.request("first").await;
    assert!(
        embedder
            .notifications
            .queued(&WorkspaceId::from("other".to_string()), "request:first")
            .await
            .is_none()
    );
    embedder
        .stores
        .requests
        .decide(
            &embedder.workspace,
            &request.id,
            RequestState::Approved,
            None,
            pagis_core::Author::System,
            1,
        )
        .await
        .unwrap();
    assert!(
        embedder
            .notifications
            .queued(&embedder.workspace, "request:first")
            .await
            .is_none()
    );
}

#[tokio::test(start_paused = true)]
async fn the_public_send_retries_once_after_at_most_300_seconds() {
    for retry_after in [
        None,
        Some(Duration::from_secs(600)),
        Some(Duration::from_secs(10)),
    ] {
        let embedder = Embedder::new().await;
        let row = embedder.subscribe(&embedder.workspace, "limited").await;
        embedder.sender.outcomes.lock().unwrap().insert(
            row.endpoint.clone(),
            VecDeque::from([Outcome::RateLimited { retry_after }, Outcome::Delivered]),
        );
        embedder.request("first").await;
        let queued = embedder
            .notifications
            .queued(&embedder.workspace, "request:first")
            .await
            .unwrap();
        let send = start_send(
            &embedder.notifications,
            &embedder.workspace,
            &queued,
            &CancellationToken::new(),
        );
        let_the_send_run().await;
        assert_eq!(embedder.sender.sent.lock().unwrap().len(), 1);
        assert_eq!(
            embedder
                .stores
                .push_subscriptions
                .list(&pagis_core::UserId::from(
                    embedder.workspace.as_str().to_string()
                ))
                .await
                .unwrap()[0]
                .last_sent_at,
            None
        );
        let delay = retry_after
            .unwrap_or(Duration::from_secs(300))
            .min(Duration::from_secs(300));
        tokio::time::sleep(delay - Duration::from_millis(2)).await;
        assert_eq!(embedder.sender.sent.lock().unwrap().len(), 1);
        assert!(!send.is_finished(), "the send waits for its retry");
        *embedder.clock.0.lock().unwrap() = 42;
        send.await.unwrap();
        let sent = embedder.sender.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], sent[1]);
        assert_eq!(
            embedder
                .stores
                .push_subscriptions
                .list(&pagis_core::UserId::from(
                    embedder.workspace.as_str().to_string()
                ))
                .await
                .unwrap()[0]
                .last_sent_at,
            Some(42)
        );
        tokio::time::sleep(Duration::from_secs(600)).await;
        assert_eq!(embedder.sender.sent.lock().unwrap().len(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_stops_a_pending_retry() {
    let embedder = Embedder::new().await;
    let row = embedder.subscribe(&embedder.workspace, "limited").await;
    embedder.sender.outcomes.lock().unwrap().insert(
        row.endpoint.clone(),
        VecDeque::from([Outcome::RateLimited { retry_after: None }]),
    );
    embedder.request("first").await;
    let queued = embedder
        .notifications
        .queued(&embedder.workspace, "request:first")
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let send = start_send(
        &embedder.notifications,
        &embedder.workspace,
        &queued,
        &cancel,
    );
    let_the_send_run().await;
    assert_eq!(embedder.sender.sent.lock().unwrap().len(), 1);
    cancel.cancel();
    send.await.unwrap();
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(embedder.sender.sent.lock().unwrap().len(), 1);
    assert_eq!(
        embedder
            .stores
            .push_subscriptions
            .list(&pagis_core::UserId::from(
                embedder.workspace.as_str().to_string()
            ))
            .await
            .unwrap()[0]
            .last_sent_at,
        None
    );
}

#[tokio::test]
async fn shared_workspace_notifications_and_push_subscriptions_are_separate_for_each_member() {
    use pagis_core::{
        Author, EventBus, NewEvent, UserId, WorkspaceKind, WorkspaceMember, WorkspaceRole,
    };
    let embedder = Embedder::new().await;
    let alice = UserId::from("alice".to_string());
    let bob = UserId::from("bob".to_string());
    for person in [&alice, &bob] {
        embedder
            .stores
            .users
            .create(&pagis_core::User {
                id: person.clone(),
                org_id: pagis_core::OrgId::generate(),
                email: None,
                name: Some(person.to_string()),
                password_hash: None,
                role: pagis_core::UserRole::Member,
                disabled_at: None,
                last_signed_in_at: None,
                created_at: 0,
                updated_at: 0,
            })
            .await
            .unwrap();
    }
    let mut shared = embedder
        .stores
        .workspaces
        .get(&embedder.workspace)
        .await
        .unwrap()
        .unwrap();
    shared.id = WorkspaceId::from("team".to_string());
    shared.kind = WorkspaceKind::Shared;
    shared.user_id = None;
    embedder
        .stores
        .workspace_members
        .create_workspace(
            &shared,
            &WorkspaceMember {
                workspace_id: shared.id.clone(),
                person_id: alice.clone(),
                role: WorkspaceRole::Manager,
                author: Author::Person(alice.clone()),
            },
        )
        .await
        .unwrap();
    embedder
        .stores
        .workspace_members
        .add(&WorkspaceMember {
            workspace_id: shared.id.clone(),
            person_id: bob.clone(),
            role: WorkspaceRole::Responder,
            author: Author::Person(alice.clone()),
        })
        .await
        .unwrap();
    embedder
        .subscribe(&WorkspaceId::from("alice".to_string()), "alice-phone")
        .await;
    embedder
        .subscribe(&WorkspaceId::from("bob".to_string()), "bob-phone")
        .await;
    assert_eq!(
        embedder
            .stores
            .push_subscriptions
            .list(&alice)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        embedder
            .stores
            .push_subscriptions
            .list(&bob)
            .await
            .unwrap()
            .len(),
        1
    );
    let activity = Arc::new(PersonActivity::new());
    activity.record(&alice, embedder.clock.now_ms());
    let bus = Arc::new(pagis_core::fake::FakeDatabase::default().bus());
    let cancel = CancellationToken::new();
    pagis_needs_you::run_notifications(
        embedder.notifications.clone(),
        activity,
        bus.clone(),
        cancel.clone(),
    )
    .await;
    bus.publish(NewEvent {
        workspace_id: shared.id,
        event_type: NEEDS_YOU_ADDED.into(),
        agent_id: None,
        run_id: None,
        channel_id: None,
        payload: json!({"item": pagis_needs_you::NeedsYouItem::Approval(
            pagis_needs_you::NeedsYouApproval {
                id: "request:team".into(), agent_id: "team-agent".into(),
                line: "Support needs your approval".into(), url: "/requests/team".into(), at: 0,
                request_id: "team".into(), request_kind: Request::TOOL_ACTION_KIND.into(),
                title: "Send a reply".into(), body: String::new(),
            }
        ), "count":1}),
    })
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if !embedder.sender.sent.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let sent = embedder.sender.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.endpoint, "https://push.example.com/bob-phone");
    cancel.cancel();
}
