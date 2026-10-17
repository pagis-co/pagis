//! The Notifications of the daemon (ADR-0030): a Web Push to each Push
//! Subscription of the Person when an item enters the Needs-You Queue.
//!
//! [`payload`] writes the plaintext: the Declarative Web Push JSON of
//! WebKit, with the Pagis fields in `notification.data`. Safari shows it
//! with no service worker code, and the service worker and the Mobile
//! App read the same JSON. The plaintext holds the line and the place of
//! the item, never the content of a message, a tool input or a
//! Credential.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use pagis_core::{
    AgentId, AgentStore, Clock, PushSubscription, PushSubscriptionStore, Request, WorkspaceId,
};
use pagis_push::{MAX_PLAINTEXT, Options, Outcome, Subscription, Topic, Urgency};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;

use crate::NotificationSender;
#[cfg(test)]
use crate::needs_you::{NEEDS_YOU_ADDED, NEEDS_YOU_REMOVED};
use crate::needs_you::{NeedsYou, NeedsYouItem};
#[cfg(test)]
use crate::person_activity::PersonActivity;

/// The version of the payload format. A change that breaks the format
/// raises it (ADR-0030).
const PAYLOAD_VERSION: u32 = 1;

/// The magic number of a Declarative Web Push message: the RFC of Web
/// Push.
const DECLARATIVE_WEB_PUSH: u32 = 8030;

/// The title of a Notification that belongs to no Agent.
const PAGIS: &str = "Pagis";

/// How long a push service keeps a Notification for a client that is not
/// reachable.
const TTL: Duration = Duration::from_secs(86_400);

/// The length of the `Topic` of a Notification: the longest that
/// RFC 8030 allows.
const TOPIC_CHARS: usize = 32;

/// What ends a body that was cut to fit.
const ELLIPSIS: char = '\u{2026}';

/// The longest wait before the second send to a push service that
/// answered `429`.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(300);

/// The answers that a Notification offers for a Request. It never offers
/// `always`, so an answer from a Notification writes no Allow Rule.
const REQUEST_ACTIONS: [&str; 2] = ["approve_once", "deny"];

/// The plaintext of the Notification of one queue item: `agent_name` is
/// the name of the Agent of the item, `None` for an item of no Agent,
/// and `count` is the count of the queue. The body is cut at a word
/// boundary when the plaintext would be over [`MAX_PLAINTEXT`].
pub fn payload(
    item: &NeedsYouItem,
    agent_name: Option<&str>,
    count: usize,
    public_origin: &str,
) -> Vec<u8> {
    let mut data = json!({
        "v": PAYLOAD_VERSION,
        "item": item.id(),
        "kind": item.kind(),
    });
    let body = match item {
        NeedsYouItem::Approval(approval) => {
            if answerable(&approval.request_kind) {
                data["request"] = json!({
                    "id": approval.request_id,
                    "actions": REQUEST_ACTIONS,
                });
            }
            format!("{}\n{}", approval.line, approval.title)
        }
        _ => item.line().to_string(),
    };
    let message = Message {
        title: agent_name.unwrap_or(PAGIS),
        navigate: navigate(public_origin, item.url()),
        data,
        app_badge: Some(count),
    };
    message.fitted(&body)
}

/// The plaintext of the test Notification that the Person sends from
/// Settings. It sets no badge.
pub fn test_payload(public_origin: &str) -> Vec<u8> {
    let message = Message {
        title: PAGIS,
        navigate: navigate(public_origin, "/settings/notifications"),
        data: json!({"v": PAYLOAD_VERSION, "item": "test", "kind": "test"}),
        app_badge: None,
    };
    message.fitted("Notifications work here.")
}

/// The delivery options of the Notification of one queue item.
pub fn options(item: &NeedsYouItem) -> Options {
    let urgency = match item {
        NeedsYouItem::Assignment(_)
        | NeedsYouItem::Approval(_)
        | NeedsYouItem::Waiting(_)
        | NeedsYouItem::Keypad(_) => Urgency::High,
        NeedsYouItem::Call(_) | NeedsYouItem::Failed(_) | NeedsYouItem::Mail(_) => Urgency::Normal,
    };
    Options {
        ttl: TTL,
        urgency,
        topic: Some(topic(item.id())),
    }
}

/// The delivery options of the test Notification. The Person waits for
/// it, so it is urgent, and it has no topic.
pub fn test_options() -> Options {
    Options {
        ttl: TTL,
        urgency: Urgency::High,
        topic: None,
    }
}

/// A Request that a Notification can answer: a tool action or a
/// credential action Approval, or a Harness Permission.
fn answerable(request_kind: &str) -> bool {
    [
        Request::TOOL_ACTION_KIND,
        Request::CREDENTIAL_ACTION_KIND,
        Request::HARNESS_PERMISSION_KIND,
    ]
    .contains(&request_kind)
}

/// The place `path` of the Product App on the Public Origin.
fn navigate(public_origin: &str, path: &str) -> String {
    format!("{}{path}", public_origin.trim_end_matches('/'))
}

/// The `Topic` of an item: the first characters of the unpadded
/// base64url SHA-256 of its id. A newer push of the same item replaces a
/// push that the push service did not deliver.
fn topic(item_id: &str) -> Topic {
    let hash = URL_SAFE_NO_PAD.encode(Sha256::digest(item_id.as_bytes()));
    Topic::new(&hash[..TOPIC_CHARS]).expect("base64url is the alphabet of a topic")
}

/// A Declarative Web Push message with no body yet.
struct Message<'a> {
    title: &'a str,
    navigate: String,
    data: Value,
    app_badge: Option<usize>,
}

impl Message<'_> {
    fn render(&self, body: &str) -> Vec<u8> {
        let mut message = json!({
            "web_push": DECLARATIVE_WEB_PUSH,
            "notification": {
                "title": self.title,
                "body": body,
                "navigate": self.navigate,
                "data": self.data,
            },
            "mutable": true,
        });
        // WebKit reads `app_badge` from the top level of the message.
        if let Some(count) = self.app_badge {
            message["app_badge"] = json!(count);
        }
        serde_json::to_vec(&message).expect("a JSON value serializes")
    }

    /// The message with `body`, or with the longest start of `body` that
    /// fits in [`MAX_PLAINTEXT`] bytes. The cut falls before a
    /// whitespace, or at a character boundary when no word fits, and an
    /// ellipsis ends it.
    fn fitted(&self, body: &str) -> Vec<u8> {
        let whole = self.render(body);
        if whole.len() <= MAX_PLAINTEXT {
            return whole;
        }
        let cut = |end: usize| format!("{}{ELLIPSIS}", body[..end].trim_end());
        let fits = |end: &usize| self.render(&cut(*end)).len() <= MAX_PLAINTEXT;
        let words: Vec<usize> = body
            .char_indices()
            .filter(|(_, character)| character.is_whitespace())
            .map(|(index, _)| index)
            .filter(|index| !body[..*index].trim_end().is_empty())
            .collect();
        let characters: Vec<usize> = body
            .char_indices()
            .map(|(index, _)| index)
            .skip(1)
            .collect();
        // A longer start makes a longer message, so the ends that fit
        // come first in each list.
        let end = [words, characters].into_iter().find_map(|ends| {
            let fitting = ends.partition_point(fits);
            fitting.checked_sub(1).map(|last| ends[last])
        });
        match end {
            Some(end) => self.render(&cut(end)),
            None => self.render(&ELLIPSIS.to_string()),
        }
    }
}

/// The Notification sender of the installation: the Web Push sender
/// with the VAPID Key, and the records that a Notification reads and
/// changes.
pub struct Notifications {
    sender: Arc<dyn NotificationSender>,
    public_origin: String,
    push_subscriptions: Arc<dyn PushSubscriptionStore>,
    agents: Arc<dyn AgentStore>,
    /// The Needs-You Queue, which a held item reads again before it goes.
    needs_you: Arc<NeedsYou>,
    pub(crate) clock: Arc<dyn Clock>,
}

impl Notifications {
    /// A sender and the records that a Notification reads and changes.
    pub fn new(
        sender: Arc<dyn NotificationSender>,
        public_origin: String,
        push_subscriptions: Arc<dyn PushSubscriptionStore>,
        agents: Arc<dyn AgentStore>,
        needs_you: Arc<NeedsYou>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            sender,
            public_origin,
            push_subscriptions,
            agents,
            needs_you,
            clock,
        }
    }

    /// Send the test Notification to one Push Subscription, and answer
    /// what the push service answered. A `429` gets no second send: the
    /// Person sees it and tries again.
    pub async fn send_test(&self, row: &PushSubscription) -> Outcome {
        self.send(row, &test_payload(&self.public_origin), test_options())
            .await
    }

    /// The name of the Agent of an item, or `None` when the store holds
    /// no such Agent.
    async fn agent_name(&self, workspace_id: &WorkspaceId, agent_id: &str) -> Option<String> {
        match self
            .agents
            .get(workspace_id, &AgentId::from(agent_id.to_string()))
            .await
        {
            Ok(agent) => agent.map(|agent| agent.name),
            Err(error) => {
                tracing::error!(%error, %workspace_id, agent_id, "the Agent of a Notification was not read");
                None
            }
        }
    }

    /// Send one Notification, and send it once more after the delay
    /// that a `429` asks for, at most [`MAX_RETRY_DELAY`] later.
    async fn deliver(&self, row: &PushSubscription, plaintext: &[u8], options: Options) {
        let Outcome::RateLimited { retry_after } = self.send(row, plaintext, options.clone()).await
        else {
            return;
        };
        let delay = retry_after.map_or(MAX_RETRY_DELAY, |delay| delay.min(MAX_RETRY_DELAY));
        pagis_rt::sleep(delay).await;
        self.send(row, plaintext, options).await;
    }

    /// Send one Web Push and act on the outcome: a delivery marks the
    /// Push Subscription as sent, and a gone endpoint ends it. Every
    /// other outcome writes a log line with the id and the status, and
    /// never the payload.
    async fn send(&self, row: &PushSubscription, plaintext: &[u8], options: Options) -> Outcome {
        let subscription = Subscription {
            endpoint: row.endpoint.clone(),
            p256dh: row.p256dh.clone(),
            auth: row.auth.clone(),
        };
        let outcome = self.sender.send(&subscription, plaintext, options).await;
        let id = &row.id;
        match &outcome {
            Outcome::Delivered => {
                let now = self.clock.now_ms();
                if let Err(error) = self
                    .push_subscriptions
                    .mark_sent(&row.user_id, id, now)
                    .await
                {
                    tracing::error!(%error, push_subscription_id = %id, "the send time was not kept");
                }
            }
            Outcome::Gone => {
                tracing::info!(push_subscription_id = %id, "the push service ended the Push Subscription");
                if let Err(error) = self
                    .push_subscriptions
                    .delete_by_endpoint(&row.user_id, &row.endpoint)
                    .await
                {
                    tracing::error!(%error, push_subscription_id = %id, "the gone Push Subscription was not deleted");
                }
            }
            Outcome::TooLarge => {
                tracing::warn!(push_subscription_id = %id, status = 413, "the Notification is too large");
            }
            Outcome::RateLimited { retry_after } => {
                tracing::warn!(
                    push_subscription_id = %id,
                    status = 429,
                    retry_after_seconds = retry_after.map(|delay| delay.as_secs()),
                    "the push service asks the daemon to wait"
                );
            }
            Outcome::Failed { status, error } => {
                tracing::warn!(
                    push_subscription_id = %id,
                    status = status.map(|status| status.as_u16()),
                    %error,
                    "the Notification was not sent"
                );
            }
        }
        outcome
    }
}

/// An item as the Needs-You Queue holds it, with the count of the
/// queue: the payload of `needs_you.added`, and the answer of
/// [`Notifications::queued`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QueuedItem {
    pub item: NeedsYouItem,
    pub count: usize,
}

/// What the Notification task reads and does: the item as the Needs-You
/// Queue holds it now, and the send. [`Notifications`] is the one of the
/// daemon.
pub(crate) trait Notifier: Send + Sync + 'static {
    fn recipients(
        &self,
        workspace_id: &WorkspaceId,
    ) -> impl Future<Output = Vec<pagis_core::UserId>> + Send;

    /// The item `item_id` in the Needs-You Queue of `workspace_id` now,
    /// or `None` when the item left the queue.
    fn queued(
        &self,
        workspace_id: &WorkspaceId,
        item_id: &str,
    ) -> impl Future<Output = Option<QueuedItem>> + Send;

    /// Send the Notification of `item` to each Push Subscription of
    /// `user_id`. `count` is the count of the queue, and `cancel`
    /// stops each send.
    fn notify(
        self: &Arc<Self>,
        workspace_id: &WorkspaceId,
        user_id: &pagis_core::UserId,
        item: &NeedsYouItem,
        count: usize,
        cancel: &CancellationToken,
    ) -> impl Future<Output = ()> + Send;
}

impl Notifier for Notifications {
    async fn recipients(&self, workspace_id: &WorkspaceId) -> Vec<pagis_core::UserId> {
        match self.needs_you.workspaces.get(workspace_id).await {
            Ok(Some(workspace)) if workspace.kind == pagis_core::WorkspaceKind::Private => {
                workspace.user_id.into_iter().collect()
            }
            _ => Vec::new(),
        }
    }

    async fn queued(&self, workspace_id: &WorkspaceId, item_id: &str) -> Option<QueuedItem> {
        Notifications::queued(self, workspace_id, item_id).await
    }

    /// The send runs in a task of its own, so a held send waits for
    /// its retry while the loop reads the next event.
    async fn notify(
        self: &Arc<Self>,
        workspace_id: &WorkspaceId,
        user_id: &pagis_core::UserId,
        item: &NeedsYouItem,
        count: usize,
        cancel: &CancellationToken,
    ) {
        let notifications = Arc::clone(self);
        let workspace_id = workspace_id.clone();
        let user_id = user_id.clone();
        let item = item.clone();
        let cancel = cancel.clone();
        pagis_rt::spawn(async move {
            Notifications::notify(
                &notifications,
                &workspace_id,
                &user_id,
                &item,
                count,
                &cancel,
            )
            .await;
        });
    }
}

impl Notifications {
    /// Read `item_id` as the Needs-You Queue of `workspace_id` holds it
    /// now, with the current count. Returns `None` when the item left
    /// the queue or the queue cannot be read. An Embedder calls this
    /// at the end of a hold before it calls [`crate::NotificationRule::on_hold_end`].
    pub async fn queued(&self, workspace_id: &WorkspaceId, item_id: &str) -> Option<QueuedItem> {
        if !self.needs_you.is_private(workspace_id).await {
            return None;
        }
        match self
            .needs_you
            .derive(workspace_id, self.clock.now_ms())
            .await
        {
            Ok(queue) => {
                let count = queue.count;
                queue
                    .items
                    .into_iter()
                    .find(|item| item.id() == item_id)
                    .map(|item| QueuedItem { item, count })
            }
            Err(error) => {
                tracing::error!(%error, %workspace_id, item_id, "the Needs-You Queue was not derived, so a held Notification does not go");
                None
            }
        }
    }

    /// Send the Notification of `item` to each Push Subscription of
    /// `user_id`. `count` is the current count of the Needs-You Queue.
    /// The sends run together, and the method returns when each send and
    /// its retry end, so an Embedder whose process can stop keeps it open
    /// until then. A delivery keeps the send time, a gone endpoint is
    /// deleted, and a `429` gets one retry after at most 300 seconds.
    /// `cancel` stops each send and its retry. An Embedder calls this for
    /// [`crate::Action::Send`].
    pub async fn notify(
        &self,
        workspace_id: &WorkspaceId,
        user_id: &pagis_core::UserId,
        item: &NeedsYouItem,
        count: usize,
        cancel: &CancellationToken,
    ) {
        if !self.recipients(workspace_id).await.contains(user_id) {
            return;
        }
        let rows = match self.push_subscriptions.list(user_id).await {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(%error, %workspace_id, "the Push Subscriptions were not read");
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        let agent_name = match item.agent_id() {
            Some(agent_id) => self.agent_name(workspace_id, agent_id).await,
            None => None,
        };
        let plaintext = payload(item, agent_name.as_deref(), count, &self.public_origin);
        let options = options(item);
        futures::future::join_all(
            rows.iter().map(|row| {
                cancel.run_until_cancelled(self.deliver(row, &plaintext, options.clone()))
            }),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::needs_you::{
        NeedsYouApproval, NeedsYouCall, NeedsYouFailed, NeedsYouKeypad, NeedsYouWaiting,
    };
    use pagis_push::{MAX_PLAINTEXT, Urgency};
    use serde_json::{Value, json};

    const ORIGIN: &str = "https://pagis.example.com";

    fn approval(request_kind: &str) -> NeedsYouItem {
        NeedsYouItem::Approval(NeedsYouApproval {
            workspace_id: "workspace".into(),
            id: "request:r-1".to_string(),
            agent_id: "a-1".to_string(),
            line: "Robin needs your approval".to_string(),
            url: "/c/ch-1".to_string(),
            at: 1,
            request_id: "r-1".to_string(),
            request_kind: request_kind.to_string(),
            title: "host_shell".to_string(),
            body: "rm -rf the secret tool input".to_string(),
        })
    }

    fn waiting() -> NeedsYouItem {
        NeedsYouItem::Waiting(NeedsYouWaiting {
            id: "run:run-1".to_string(),
            agent_id: "a-1".to_string(),
            line: "Robin waits for your answer".to_string(),
            url: "/c/ch-1".to_string(),
            at: 1,
            run_id: "run-1".to_string(),
            channel_id: Some("ch-1".to_string()),
        })
    }

    fn keypad() -> NeedsYouItem {
        NeedsYouItem::Keypad(NeedsYouKeypad {
            id: "keypad".to_string(),
            line: "Callers entered a wrong keypad code 6 times".to_string(),
            url: "/".to_string(),
            at: 1,
            failed_attempts: 6,
            suspended_until: 1,
        })
    }

    fn call() -> NeedsYouItem {
        NeedsYouItem::Call(NeedsYouCall {
            id: "call:c-1".to_string(),
            agent_id: "a-1".to_string(),
            line: "Robin missed a call from +16505550100".to_string(),
            url: "/".to_string(),
            at: 1,
            call_id: "c-1".to_string(),
            remote_e164: "+16505550100".to_string(),
            left_message: false,
        })
    }

    fn failed_with_line(line: &str) -> NeedsYouItem {
        NeedsYouItem::Failed(NeedsYouFailed {
            id: "run:run-2".to_string(),
            agent_id: "a-1".to_string(),
            line: line.to_string(),
            url: "/runs/run-2".to_string(),
            at: 1,
            run_id: "run-2".to_string(),
            channel_id: None,
            failure_kind: None,
        })
    }

    fn failed() -> NeedsYouItem {
        failed_with_line("Robin could not finish the work")
    }

    fn json_of(plaintext: &[u8]) -> Value {
        serde_json::from_slice(plaintext).expect("the payload is JSON")
    }

    #[test]
    fn an_approval_of_a_tool_action_is_a_declarative_web_push_that_can_answer_it() {
        let plaintext = payload(&approval("tool_action"), Some("Robin"), 3, ORIGIN);

        assert_eq!(
            json_of(&plaintext),
            json!({
                "web_push": 8030,
                "notification": {
                    "title": "Robin",
                    "body": "Robin needs your approval\nhost_shell",
                    "navigate": "https://pagis.example.com/c/ch-1",
                    "data": {
                        "v": 1,
                        "item": "request:r-1",
                        "kind": "approval",
                        "request": {"id": "r-1", "actions": ["approve_once", "deny"]},
                    },
                },
                "app_badge": 3,
                "mutable": true,
            })
        );
    }

    #[test]
    fn an_approval_of_a_credential_action_can_be_answered() {
        let message = json_of(&payload(
            &approval("credential_action"),
            Some("Robin"),
            1,
            ORIGIN,
        ));

        assert_eq!(
            message["notification"]["data"]["request"],
            json!({"id": "r-1", "actions": ["approve_once", "deny"]})
        );
    }

    #[test]
    fn an_approval_of_a_harness_permission_can_be_answered_once() {
        let message = json_of(&payload(
            &approval("harness_permission"),
            Some("Robin"),
            1,
            ORIGIN,
        ));

        assert_eq!(
            message["notification"]["data"]["request"],
            json!({"id": "r-1", "actions": ["approve_once", "deny"]})
        );
    }

    #[test]
    fn a_request_that_a_notification_cannot_answer_has_no_request_block() {
        for request_kind in ["form", "choice", "widget"] {
            let message = json_of(&payload(&approval(request_kind), Some("Robin"), 1, ORIGIN));
            let data = &message["notification"]["data"];
            assert_eq!(data["kind"], "approval", "{request_kind}");
            assert!(data.get("request").is_none(), "{request_kind}: {data}");
        }
    }

    #[test]
    fn each_kind_names_its_item_and_its_line_and_carries_no_request_block() {
        for (item, kind, id) in [
            (waiting(), "waiting", "run:run-1"),
            (keypad(), "keypad", "keypad"),
            (call(), "call", "call:c-1"),
            (failed(), "failed", "run:run-2"),
        ] {
            let message = json_of(&payload(&item, Some("Robin"), 2, ORIGIN));
            let notification = &message["notification"];
            assert_eq!(notification["body"], item.line(), "{kind}");
            assert_eq!(
                notification["data"],
                json!({"v": 1, "item": id, "kind": kind}),
                "{kind}"
            );
            assert_eq!(
                notification["navigate"],
                format!("{ORIGIN}{}", item.url()),
                "{kind}"
            );
            assert_eq!(message["app_badge"], 2, "{kind}");
            assert_eq!(message["mutable"], true, "{kind}");
            assert_eq!(message["web_push"], 8030, "{kind}");
        }
    }

    #[test]
    fn the_title_is_the_name_of_the_agent_or_pagis_for_an_item_with_no_agent() {
        let named = json_of(&payload(&waiting(), Some("Robin"), 1, ORIGIN));
        let unnamed = json_of(&payload(&keypad(), None, 1, ORIGIN));

        assert_eq!(named["notification"]["title"], "Robin");
        assert_eq!(unnamed["notification"]["title"], "Pagis");
    }

    #[test]
    fn navigate_joins_the_public_origin_and_the_place_of_the_item() {
        let message = json_of(&payload(
            &failed(),
            Some("Robin"),
            1,
            "https://pagis.example.com/",
        ));

        assert_eq!(
            message["notification"]["navigate"],
            "https://pagis.example.com/runs/run-2"
        );
    }

    #[test]
    fn app_badge_is_the_count_of_the_queue() {
        let message = json_of(&payload(&failed(), Some("Robin"), 42, ORIGIN));

        assert_eq!(message["app_badge"], 42);
    }

    #[test]
    fn a_long_line_is_cut_at_a_word_boundary_so_the_payload_fits() {
        let words: Vec<String> = (0..1000).map(|n| format!("w{n:03}")).collect();
        let line = words.join(" ");
        assert_eq!(line.chars().count(), 4999);

        let plaintext = payload(&failed_with_line(&line), Some("Robin"), 1, ORIGIN);

        assert!(
            plaintext.len() <= MAX_PLAINTEXT,
            "{} bytes",
            plaintext.len()
        );
        assert!(
            plaintext.len() > MAX_PLAINTEXT - 16,
            "{} bytes",
            plaintext.len()
        );
        let body = json_of(&plaintext)["notification"]["body"]
            .as_str()
            .expect("the body")
            .to_string();
        let kept = body
            .strip_suffix('…')
            .expect("a cut body ends with an ellipsis");
        assert!(line.starts_with(kept), "{kept}");
        assert_eq!(line[kept.len()..].chars().next(), Some(' '), "{kept}");
    }

    #[test]
    fn a_long_line_with_no_space_is_cut_at_a_character_boundary() {
        let line = "é".repeat(5000);

        let plaintext = payload(&failed_with_line(&line), Some("Robin"), 1, ORIGIN);

        assert!(
            plaintext.len() <= MAX_PLAINTEXT,
            "{} bytes",
            plaintext.len()
        );
        let body = json_of(&plaintext)["notification"]["body"]
            .as_str()
            .expect("the body")
            .to_string();
        let kept = body
            .strip_suffix('…')
            .expect("a cut body ends with an ellipsis");
        assert!(!kept.is_empty());
        assert!(line.starts_with(kept));
    }

    #[test]
    fn a_line_with_characters_that_json_escapes_still_fits() {
        let line = "\"quoted\" \\ and\ttab ".repeat(300);

        let plaintext = payload(&failed_with_line(&line), Some("Robin"), 1, ORIGIN);

        assert!(
            plaintext.len() <= MAX_PLAINTEXT,
            "{} bytes",
            plaintext.len()
        );
        json_of(&plaintext);
    }

    #[test]
    fn approval_waiting_and_keypad_are_urgent_and_call_and_failed_are_normal() {
        for (item, urgency) in [
            (approval("tool_action"), Urgency::High),
            (waiting(), Urgency::High),
            (keypad(), Urgency::High),
            (call(), Urgency::Normal),
            (failed(), Urgency::Normal),
        ] {
            let options = options(&item);
            assert_eq!(options.urgency, urgency, "{}", item.kind());
            assert_eq!(options.ttl.as_secs(), 86_400, "{}", item.kind());
        }
    }

    #[test]
    fn the_topic_is_32_characters_of_the_hash_of_the_item_id() {
        let topic = options(&waiting()).topic.expect("a topic");
        let again = options(&waiting()).topic.expect("a topic");
        let other = options(&failed()).topic.expect("a topic");

        assert_eq!(topic.as_str().len(), 32);
        assert_eq!(topic, again);
        assert_ne!(topic, other);
        // The unpadded base64url SHA-256 of "run:run-1".
        use base64::Engine as _;
        use sha2::Digest as _;
        let hash = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(b"run:run-1"));
        assert_eq!(topic.as_str(), &hash[..32]);
    }

    #[test]
    fn the_test_notification_opens_the_notifications_settings() {
        assert_eq!(
            json_of(&test_payload(ORIGIN)),
            json!({
                "web_push": 8030,
                "notification": {
                    "title": "Pagis",
                    "body": "Notifications work here.",
                    "navigate": "https://pagis.example.com/settings/notifications",
                    "data": {"v": 1, "item": "test", "kind": "test"},
                },
                "mutable": true,
            })
        );
    }

    /// The Notification task on paused tokio time: the hold of ADR-0030.
    #[cfg(not(target_arch = "wasm32"))]
    mod task {
        use std::sync::Mutex;

        use futures::{StreamExt, channel::mpsc};
        use pagis_core::{Event, EventId, UnixMillis};
        use tokio::time::Instant;

        use super::*;
        use crate::loops::serve;

        /// 2026-09-25 12:00 UTC.
        const NOW: UnixMillis = 1_790_337_600_000;

        fn workspace() -> WorkspaceId {
            WorkspaceId::from("w-1".to_string())
        }

        fn secs(seconds: u64) -> Duration {
            Duration::from_secs(seconds)
        }

        /// A clock on tokio's time, which `start_paused` controls.
        struct TokioClock {
            start: Instant,
        }

        impl Clock for TokioClock {
            fn now_ms(&self) -> UnixMillis {
                NOW + UnixMillis::try_from(self.start.elapsed().as_millis()).expect("a short test")
            }
        }

        /// A Needs-You Queue that the test sets, and the sends.
        #[derive(Default)]
        struct FakeNotifier {
            queue: Mutex<Vec<NeedsYouItem>>,
            queue_reads: Mutex<usize>,
            sent: Mutex<Vec<(String, usize)>>,
        }

        impl Notifier for FakeNotifier {
            async fn recipients(&self, _: &WorkspaceId) -> Vec<pagis_core::UserId> {
                vec![pagis_core::UserId::from("person".to_string())]
            }

            async fn queued(&self, _: &WorkspaceId, item_id: &str) -> Option<QueuedItem> {
                *self.queue_reads.lock().expect("lock") += 1;
                let queue = self.queue.lock().expect("lock");
                queue
                    .iter()
                    .find(|item| item.id() == item_id)
                    .map(|item| QueuedItem {
                        item: item.clone(),
                        count: queue.len(),
                    })
            }

            async fn notify(
                self: &Arc<Self>,
                _: &WorkspaceId,
                _: &pagis_core::UserId,
                item: &NeedsYouItem,
                count: usize,
                _: &CancellationToken,
            ) {
                self.sent
                    .lock()
                    .expect("lock")
                    .push((item.id().to_string(), count));
            }
        }

        /// The task over a stream of events that the test writes.
        struct Harness {
            start: Instant,
            clock: Arc<TokioClock>,
            notifier: Arc<FakeNotifier>,
            activity: Arc<PersonActivity>,
            events: mpsc::UnboundedSender<Event>,
        }

        impl Harness {
            fn start() -> Self {
                let start = Instant::now();
                let clock = Arc::new(TokioClock { start });
                let notifier = Arc::new(FakeNotifier::default());
                let activity = Arc::new(PersonActivity::new());
                let (events, stream) = mpsc::unbounded();
                pagis_rt::spawn(serve(
                    Arc::clone(&notifier),
                    Arc::clone(&activity),
                    Arc::clone(&clock) as Arc<dyn Clock>,
                    stream.boxed(),
                    CancellationToken::new(),
                ));
                Self {
                    start,
                    clock,
                    notifier,
                    activity,
                    events,
                }
            }

            /// The Person uses a client now.
            fn active(&self) {
                self.activity.record(
                    &pagis_core::UserId::from("person".to_string()),
                    self.clock.now_ms(),
                );
            }

            /// `item` enters the queue, and the task reads its event.
            async fn added(&self, item: NeedsYouItem) {
                let count = {
                    let mut queue = self.notifier.queue.lock().expect("lock");
                    queue.push(item.clone());
                    queue.len()
                };
                self.publish(NEEDS_YOU_ADDED, json!({"item": item, "count": count}))
                    .await;
            }

            /// `item_id` leaves the queue, and the task reads its event.
            async fn removed(&self, item_id: &str) {
                let count = self.leave(item_id);
                self.publish(
                    NEEDS_YOU_REMOVED,
                    json!({"item_id": item_id, "count": count}),
                )
                .await;
            }

            /// `item_id` leaves the queue, and the task reads no event.
            fn leave(&self, item_id: &str) -> usize {
                let mut queue = self.notifier.queue.lock().expect("lock");
                queue.retain(|item| item.id() != item_id);
                queue.len()
            }

            async fn publish(&self, event_type: &str, payload: Value) {
                self.events
                    .unbounded_send(Event {
                        id: EventId::generate(),
                        seq: 0,
                        workspace_id: workspace(),
                        event_type: event_type.to_string(),
                        agent_id: None,
                        run_id: None,
                        channel_id: None,
                        payload,
                        created_at: self.clock.now_ms(),
                    })
                    .expect("the task reads events");
                // The task reads the event before the paused clock moves.
                tokio::time::sleep(Duration::from_millis(1)).await;
            }

            /// Wait until `after` passed since the start.
            async fn at(&self, after: Duration) {
                tokio::time::sleep_until(self.start + after).await;
            }

            fn sent(&self) -> Vec<(String, usize)> {
                self.notifier.sent.lock().expect("lock").clone()
            }

            fn queue_reads(&self) -> usize {
                *self.notifier.queue_reads.lock().expect("lock")
            }
        }

        #[tokio::test(start_paused = true)]
        async fn an_item_goes_at_once_when_the_person_is_not_active() {
            let task = Harness::start();

            task.added(failed()).await;

            assert_eq!(task.sent(), [("run:run-2".to_string(), 1)]);
        }

        #[tokio::test(start_paused = true)]
        async fn an_item_waits_while_the_person_is_active_and_goes_120_s_after_the_activity() {
            let task = Harness::start();
            task.active();

            task.added(approval("tool_action")).await;
            task.at(secs(60)).await;
            assert_eq!(task.sent(), []);
            // The send reads the queue again, so it carries the count of
            // the queue at the send.
            task.notifier.queue.lock().expect("lock").push(waiting());
            task.at(secs(120) - Duration::from_millis(1)).await;
            assert_eq!(task.sent(), []);
            task.at(secs(120) + Duration::from_millis(1)).await;

            assert_eq!(task.sent(), [("request:r-1".to_string(), 2)]);
        }

        #[tokio::test(start_paused = true)]
        async fn new_activity_during_the_hold_holds_the_item_again() {
            let task = Harness::start();
            task.active();
            task.added(failed()).await;

            task.at(secs(100)).await;
            task.active();
            task.at(secs(121)).await;
            assert_eq!(task.sent(), []);
            task.at(secs(220) - Duration::from_millis(1)).await;
            assert_eq!(task.sent(), []);
            task.at(secs(220) + Duration::from_millis(1)).await;

            assert_eq!(task.sent(), [("run:run-2".to_string(), 1)]);
        }

        #[tokio::test(start_paused = true)]
        async fn needs_you_removed_drops_a_held_item_at_once() {
            let task = Harness::start();
            task.active();
            task.added(failed()).await;

            task.at(secs(60)).await;
            task.removed("run:run-2").await;
            task.at(secs(600)).await;

            assert_eq!(task.sent(), []);
            // The hold ended on the event, so it never read the queue.
            assert_eq!(task.queue_reads(), 0);
        }

        #[tokio::test(start_paused = true)]
        async fn an_item_that_left_the_queue_during_the_hold_is_dropped() {
            let task = Harness::start();
            task.active();
            task.added(failed()).await;

            task.at(secs(60)).await;
            task.leave("run:run-2");
            task.at(secs(600)).await;

            assert_eq!(task.sent(), []);
            assert_eq!(task.queue_reads(), 1);
        }

        #[tokio::test(start_paused = true)]
        async fn an_item_after_the_hold_ended_goes_at_once() {
            let task = Harness::start();
            task.active();

            task.at(secs(120)).await;
            task.added(failed()).await;

            assert_eq!(task.sent(), [("run:run-2".to_string(), 1)]);
        }
    }
}
