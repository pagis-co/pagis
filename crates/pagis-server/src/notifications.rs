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
use futures::StreamExt;
use pagis_core::{
    AgentId, AgentStore, Clock, EventBus, EventScope, PushSubscription, PushSubscriptionStore,
    Request, SecretStore, WorkspaceId,
};
use pagis_push::{MAX_PLAINTEXT, Options, Outcome, Policy, Subscription, Topic, Urgency, WebPush};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;

use crate::needs_you::{NEEDS_YOU_ADDED, NeedsYouItem};

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
        NeedsYouItem::Approval(_) | NeedsYouItem::Waiting(_) | NeedsYouItem::Keypad(_) => {
            Urgency::High
        }
        NeedsYouItem::Call(_) | NeedsYouItem::Failed(_) => Urgency::Normal,
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
/// credential action Approval.
fn answerable(request_kind: &str) -> bool {
    request_kind == Request::TOOL_ACTION_KIND || request_kind == Request::CREDENTIAL_ACTION_KIND
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
    web_push: WebPush,
    public_origin: String,
    push_subscriptions: Arc<dyn PushSubscriptionStore>,
    agents: Arc<dyn AgentStore>,
    clock: Arc<dyn Clock>,
}

impl Notifications {
    /// The sender that signs with the VAPID Key of `secrets`, which it
    /// makes at this first need, and that sends to the endpoints that
    /// `policy` allows.
    pub fn new(
        secrets: &dyn SecretStore,
        public_origin: String,
        policy: Policy,
        push_subscriptions: Arc<dyn PushSubscriptionStore>,
        agents: Arc<dyn AgentStore>,
        clock: Arc<dyn Clock>,
    ) -> anyhow::Result<Self> {
        let vapid_key = crate::push_subscriptions::vapid_key(secrets)?;
        let web_push = WebPush::new(&vapid_key, &public_origin, policy)?;
        Ok(Self {
            web_push,
            public_origin,
            push_subscriptions,
            agents,
            clock,
        })
    }

    /// Send the test Notification to one Push Subscription, and answer
    /// what the push service answered. A `429` gets no second send: the
    /// Person sees it and tries again.
    pub(crate) async fn send_test(&self, row: &PushSubscription) -> Outcome {
        self.send(row, &test_payload(&self.public_origin), test_options())
            .await
    }

    /// Send the Notification of `item` to each Push Subscription of its
    /// Workspace, each in its own task.
    async fn notify(
        self: &Arc<Self>,
        workspace_id: &WorkspaceId,
        item: &NeedsYouItem,
        count: usize,
        cancel: &CancellationToken,
    ) {
        let rows = match self.push_subscriptions.list(workspace_id).await {
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
        let plaintext: Arc<[u8]> =
            payload(item, agent_name.as_deref(), count, &self.public_origin).into();
        let options = options(item);
        for row in rows {
            let notifications = Arc::clone(self);
            let plaintext = Arc::clone(&plaintext);
            let options = options.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                cancel
                    .run_until_cancelled(notifications.deliver(&row, &plaintext, options))
                    .await;
            });
        }
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
        tokio::time::sleep(delay).await;
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
        let outcome = self.web_push.send(&subscription, plaintext, options).await;
        let id = &row.id;
        match &outcome {
            Outcome::Delivered => {
                let now = self.clock.now_ms();
                if let Err(error) = self
                    .push_subscriptions
                    .mark_sent(&row.workspace_id, id, now)
                    .await
                {
                    tracing::error!(%error, push_subscription_id = %id, "the send time was not kept");
                }
            }
            Outcome::Gone => {
                tracing::info!(push_subscription_id = %id, "the push service ended the Push Subscription");
                if let Err(error) = self
                    .push_subscriptions
                    .delete_by_endpoint(&row.workspace_id, &row.endpoint)
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

/// The payload of `needs_you.added`.
#[derive(Deserialize)]
struct Added {
    item: NeedsYouItem,
    count: usize,
}

/// Start the task that sends a Notification for each item that enters
/// the Needs-You Queue: on each `needs_you.added`, to each Push
/// Subscription of the Workspace of the item. No push goes when an item
/// leaves the queue. The task subscribes before it answers, so no event
/// after the start is lost, and it stops on `cancel`.
pub async fn spawn_notifications(
    notifications: Arc<Notifications>,
    bus: Arc<dyn EventBus>,
    cancel: CancellationToken,
) {
    let mut events = bus.subscribe(EventScope::Installation, None).await;
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                () = cancel.cancelled() => return,
                event = events.next() => match event {
                    Some(event) => event,
                    None => return,
                },
            };
            if event.event_type != NEEDS_YOU_ADDED {
                continue;
            }
            let workspace_id = event.workspace_id;
            match serde_json::from_value::<Added>(event.payload) {
                Ok(added) => {
                    notifications
                        .notify(&workspace_id, &added.item, added.count, &cancel)
                        .await;
                }
                Err(error) => {
                    tracing::error!(%error, %workspace_id, "a needs_you.added event was not read");
                }
            }
        }
    });
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
}
