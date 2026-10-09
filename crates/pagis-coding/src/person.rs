//! The Person answers a Harness Permission that Pagis policy does not
//! allow (ADR-0033).
//!
//! The daemon makes a Request of the kind `harness_permission` with no
//! Run, posts its approval card in the session's Thread as a System
//! message, and waits on the bus for `request.decided`, as a parked Run
//! does. The decision route is the one route of every kind. The Request
//! expires when the harness stops waiting for it.

use std::sync::Arc;

use futures::StreamExt;
use futures::future::BoxFuture;
use pagis_broker::{Decider, HarnessToolKind, derive_rules};
use pagis_core::{
    AuthorKind, Block, CodingSession, CodingSessionPlace, DecideOutcome, EventBus, EventScope,
    EventSource, HostStore, Message, MessageId, MessageStatus, MessageStore, NewEvent, Request,
    RequestId, RequestState, RequestStore, harness, now_ms, wrap_untrusted,
};
use serde_json::{Value, json};

use crate::policy::{Fact, Outcome};
use crate::{PermissionAnswer, PermissionOptionKind, Waited};

/// What the card of a session in the Agent's own Computer names as the
/// place where the harness runs.
const COMPUTER_CARD_NAME: &str = "the sprite's computer";

/// The longest body of a card, in characters.
const BODY_CHARS: usize = 1_000;

/// One Harness Permission, as Pagis policy read it.
#[derive(Debug, Clone)]
pub(crate) struct Permission {
    pub tool_call_id: String,
    /// The title of the tool call, as the harness wrote it.
    pub title: Option<String>,
    pub kind: HarnessToolKind,
    pub command: Option<String>,
    pub locations: Vec<String>,
}

/// Asks the Person on an approval card.
#[derive(Clone)]
pub(crate) struct PersonAsks {
    pub requests: Arc<dyn RequestStore>,
    pub messages: Arc<dyn MessageStore>,
    pub hosts: Arc<dyn HostStore>,
    pub bus: Arc<dyn EventBus>,
}

impl PersonAsks {
    /// Opens the Request, posts its card, and gives the answer as a
    /// future that waits for the decision. It gives `None` when the card
    /// is not posted, and the permission is then cancelled. The card of
    /// an escalation shows the `note` of the Agent or of the daemon.
    ///
    /// A drop of the future before the decision expires the Request.
    pub(crate) async fn ask(
        &self,
        session: &CodingSession,
        permission: &Permission,
        options: &[PermissionOptionKind],
        fact: Fact,
        note: Option<&str>,
    ) -> Option<BoxFuture<'static, Waited<PermissionAnswer>>> {
        let host_name = match self.host_name(session).await {
            Ok(Some(name)) => name,
            Ok(None) => {
                tracing::warn!(session = %session.id, "the Host of a Coding Session is gone, so its permission is cancelled");
                return None;
            }
            Err(error) => {
                tracing::error!(session = %session.id, %error, "the Host of a Coding Session was not read, so its permission is cancelled");
                return None;
            }
        };
        let card = Card::new(session, &host_name, permission, note);
        // Subscribe before the row exists, so no decision slips past.
        let mut events = self
            .bus
            .subscribe(EventScope::workspace(&session.workspace_id), None)
            .await;
        let request = Request {
            id: RequestId::generate(),
            workspace_id: session.workspace_id.clone(),
            agent_id: session.agent_id.clone(),
            run_id: None,
            kind: Request::HARNESS_PERMISSION_KIND.to_string(),
            payload: card.payload.clone(),
            state: RequestState::Pending,
            values: None,
            decided_at: None,
            // A Request's own lifecycle: the wall clock, as for a Run.
            created_at: now_ms(),
        };
        if let Err(error) = self.requests.create(&request).await {
            tracing::error!(session = %session.id, %error, "the Request of a Harness Permission was not written, so it is cancelled");
            return None;
        }
        let mut expiry = Expiry {
            requests: Arc::clone(&self.requests),
            bus: Arc::clone(&self.bus),
            request: request.clone(),
            fact: Some(fact),
        };
        let mut created = request_event(
            &request,
            "request.created",
            json!({"request_id": request.id.as_str(), "kind": request.kind}),
        );
        created.channel_id = Some(session.channel_id.clone());
        if let Err(error) = self.bus.publish(created).await {
            tracing::warn!(request = %request.id, %error, "request.created was not published");
        }
        if let Err(error) = self.post_card(session, &request.id, &card).await {
            tracing::error!(session = %session.id, %error, "the approval card of a Harness Permission was not posted, so it is cancelled");
            // The policy writes the audit fact of the cancel, so the
            // expiry writes none.
            expiry.fact = None;
            let expired = self
                .requests
                .decide(
                    &request.workspace_id,
                    &request.id,
                    RequestState::Expired,
                    None,
                    now_ms(),
                )
                .await;
            if let Err(error) = expired {
                tracing::warn!(request = %request.id, %error, "a Harness Permission with no card did not expire");
            }
            return None;
        }
        let request_id = request.id.as_str().to_string();
        let options = options.to_vec();
        let bus = Arc::clone(&self.bus);
        Some(Box::pin(async move {
            let mut expiry = expiry;
            while let Some(event) = events.next().await {
                if event.event_type != "request.decided"
                    || event.payload["request_id"] != request_id
                {
                    continue;
                }
                let fact = expiry.fact.take().expect("a fact until the decision");
                let decided = Decided::of(&event.payload, &options);
                let mut fact = fact.event(decided.decider, decided.outcome, decided.option);
                fact.payload["scope"] = event.payload["scope"].clone();
                if let Err(error) = bus.publish(fact).await {
                    tracing::error!(%error, "the audit fact of a Harness Permission was not written, so it is cancelled");
                    return Waited::answered(PermissionAnswer::Cancel, decided.decider);
                }
                return Waited::answered(decided.answer, decided.decider);
            }
            // The bus ended. The drop of the expiry expires the Request.
            Waited::answered(PermissionAnswer::Cancel, None)
        }))
    }

    /// The name of the place of the session that the card names: the
    /// name of its Host, or the Agent's own Computer.
    async fn host_name(
        &self,
        session: &CodingSession,
    ) -> Result<Option<String>, pagis_core::StoreError> {
        if session.place == CodingSessionPlace::Computer {
            return Ok(Some(COMPUTER_CARD_NAME.to_string()));
        }
        let Some(host_id) = &session.host_id else {
            return Ok(None);
        };
        Ok(self
            .hosts
            .get(&session.workspace_id, host_id)
            .await?
            .map(|host| host.name))
    }

    /// Posts the card as a System message in the session's Thread. The
    /// block views a row that the daemon owns, so no Agent can make it
    /// (ADR-0004).
    async fn post_card(
        &self,
        session: &CodingSession,
        request_id: &RequestId,
        card: &Card,
    ) -> Result<(), pagis_core::StoreError> {
        let now = now_ms();
        let message = Message {
            id: MessageId::generate(),
            workspace_id: session.workspace_id.clone(),
            channel_id: session.channel_id.clone(),
            parent_message_id: Some(session.root_message_id.clone()),
            author_kind: AuthorKind::System,
            author_agent_id: None,
            run_id: None,
            status: MessageStatus::Complete,
            blocks: vec![Block::approval_card(
                request_id.as_str(),
                &card.title,
                &card.body,
            )],
            text_content: card.text_content(session),
            pending_id: None,
            created_at: now,
            completed_at: Some(now),
        };
        // The card holds harness text and no memory, so it has no
        // exposure.
        self.messages.insert_stamped(&message, &[]).await?;
        let published = self
            .bus
            .publish(NewEvent {
                workspace_id: message.workspace_id.clone(),
                event_type: "message.completed".to_string(),
                agent_id: Some(session.agent_id.clone()),
                run_id: None,
                channel_id: Some(message.channel_id.clone()),
                payload: json!({
                    "message_id": message.id.as_str(),
                    "parent_message_id": message.parent_message_id.as_ref().map(MessageId::as_str),
                    "author_kind": AuthorKind::System,
                }),
            })
            .await;
        // The message is stored, so a client that reads the Thread
        // again finds the card.
        if let Err(error) = published {
            tracing::warn!(session = %session.id, %error, "the approval card of a Harness Permission did not reach the clients");
        }
        Ok(())
    }
}

/// An event about a Request, with no Run.
fn request_event(request: &Request, event_type: &str, payload: Value) -> NewEvent {
    NewEvent {
        workspace_id: request.workspace_id.clone(),
        event_type: event_type.to_string(),
        agent_id: Some(request.agent_id.clone()),
        run_id: None,
        channel_id: None,
        payload,
    }
}

/// What a decision of the decision route gives the harness.
struct Decided {
    answer: PermissionAnswer,
    decider: Option<Decider>,
    outcome: Outcome,
    option: Option<PermissionOptionKind>,
}

impl Decided {
    /// "Approve" answers the first offered `allow_once` option, and
    /// "Deny" the first offered `reject_once` option. A request that
    /// offers no option of the kind gets `cancelled`.
    fn of(payload: &Value, options: &[PermissionOptionKind]) -> Self {
        let (wanted, answer, outcome) = match payload["decision"].as_str() {
            Some("approved") => (
                PermissionOptionKind::AllowOnce,
                PermissionAnswer::AllowOnce,
                Outcome::Allowed,
            ),
            Some("denied") => (
                PermissionOptionKind::RejectOnce,
                PermissionAnswer::RejectOnce,
                Outcome::Rejected,
            ),
            // Only the expiry of the daemon decides a Request otherwise.
            _ => {
                return Self {
                    answer: PermissionAnswer::Cancel,
                    decider: None,
                    outcome: Outcome::Expired,
                    option: None,
                };
            }
        };
        if options.contains(&wanted) {
            Self {
                answer,
                decider: Some(Decider::Person),
                outcome,
                option: Some(wanted),
            }
        } else {
            Self {
                answer: PermissionAnswer::Cancel,
                decider: Some(Decider::Person),
                outcome: Outcome::Cancelled,
                option: None,
            }
        }
    }
}

/// Expires the Request of a permission when the harness stops waiting
/// for it before the decision: a cancel of the turn, a close, a lost
/// place, or an ACP connection that ends.
struct Expiry {
    requests: Arc<dyn RequestStore>,
    bus: Arc<dyn EventBus>,
    request: Request,
    /// The audit fact of the permission, until the decision takes it.
    fact: Option<Fact>,
}

impl Drop for Expiry {
    fn drop(&mut self) {
        let Some(fact) = self.fact.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(request = %self.request.id, "a Harness Permission did not expire: no runtime");
            return;
        };
        let requests = Arc::clone(&self.requests);
        let bus = Arc::clone(&self.bus);
        let request = self.request.clone();
        runtime.spawn(async move {
            let decided = requests
                .decide(
                    &request.workspace_id,
                    &request.id,
                    RequestState::Expired,
                    None,
                    now_ms(),
                )
                .await;
            let outcome = match decided {
                Ok(DecideOutcome::Decided(_)) => {
                    let expired = request_event(
                        &request,
                        "request.decided",
                        json!({"request_id": request.id.as_str(), "decision": "expired"}),
                    );
                    if let Err(error) = bus.publish(expired).await {
                        tracing::warn!(request = %request.id, %error, "the expiry of a Harness Permission was not published");
                    }
                    Outcome::Expired
                }
                // The Person decided, but the harness no longer waited,
                // so it got `cancelled`.
                Ok(DecideOutcome::NotPending(_)) => Outcome::Cancelled,
                Err(error) => {
                    tracing::warn!(request = %request.id, %error, "a Harness Permission did not expire");
                    Outcome::Cancelled
                }
            };
            if let Err(error) = bus.publish(fact.event(None, outcome, None)).await {
                tracing::warn!(request = %request.id, %error, "the audit fact of an ended Harness Permission was not written");
            }
        });
    }
}

/// The approval card of one permission, and the payload of its Request.
#[derive(Debug)]
struct Card {
    /// A daemon sentence from the harness name and the tool kind.
    title: String,
    /// Harness text: the command, the locations or the title of the
    /// tool call.
    body: String,
    payload: Value,
}

impl Card {
    fn new(
        session: &CodingSession,
        host_name: &str,
        permission: &Permission,
        note: Option<&str>,
    ) -> Self {
        let harness_name = harness::entry(&session.harness_id).map_or_else(
            || session.harness_id.clone(),
            |entry| entry.label.to_string(),
        );
        let title = format!("{harness_name} wants to {}", action(permission.kind));
        let body = cut(&body(permission));
        // A Host Allow Rule applies on a Host alone, so a card of a
        // session in a Computer offers no "Always allow".
        let proposed_rules = match (session.place, permission.kind, &permission.command) {
            (CodingSessionPlace::Host, HarnessToolKind::Execute, Some(command)) => {
                derive_rules(command)
            }
            _ => Vec::new(),
        };
        // The directory that the harness runs in.
        let directory = session
            .working_directory
            .as_deref()
            .unwrap_or(&session.directory);
        let payload = json!({
            "session_id": session.id,
            "harness_id": session.harness_id,
            "harness_name": harness_name,
            "host_id": session.host_id,
            "host_name": host_name,
            "directory": directory,
            "tool_call_id": permission.tool_call_id,
            "tool_kind": permission.kind.as_str(),
            "command": permission.command,
            "locations": permission.locations,
            "channel_id": session.channel_id,
            "root_message_id": session.root_message_id,
            "action_title": title,
            "body": body,
            "proposed_rules": proposed_rules,
            // What the Agent asks the Person on an escalation. It is the
            // Agent's text, or the daemon's.
            "note": note,
        });
        Self {
            title,
            body,
            payload,
        }
    }

    /// The text of the card that an Agent reads from the Thread. The
    /// body is harness text, so it is inside the envelope of the
    /// session (ADR-0005).
    fn text_content(&self, session: &CodingSession) -> String {
        let source = EventSource::coding_session(session.id.clone()).to_string();
        format!("{}\n{}", self.title, wrap_untrusted(&source, &self.body))
    }
}

/// What the harness wants to do, after "wants to".
fn action(kind: HarnessToolKind) -> &'static str {
    match kind {
        HarnessToolKind::Execute => "run a command",
        HarnessToolKind::Edit => "edit files",
        HarnessToolKind::Delete => "delete files",
        HarnessToolKind::Move => "move files",
        HarnessToolKind::Read => "read files",
        HarnessToolKind::Search => "search files",
        HarnessToolKind::Fetch => "fetch from the web",
        HarnessToolKind::SwitchMode => "change its mode",
        HarnessToolKind::Think | HarnessToolKind::Other => "use a tool",
    }
}

/// The command of an `execute`, else the locations, else the title of
/// the tool call.
fn body(permission: &Permission) -> String {
    if permission.kind == HarnessToolKind::Execute
        && let Some(command) = &permission.command
    {
        return command.clone();
    }
    if !permission.locations.is_empty() {
        return permission.locations.join("\n");
    }
    permission.title.clone().unwrap_or_default()
}

/// The first [`BODY_CHARS`] characters of `text`.
fn cut(text: &str) -> String {
    text.chars().take(BODY_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use pagis_core::{
        AgentId, ChannelId, CodingSessionId, CodingSessionPlace, CodingSessionState,
        CodingSessionUsage, HostId, RunId, SessionApprovalMode, WorkspaceId,
    };

    use super::*;

    const WORKTREE: &str = "/Users/bo/.pagis-worktrees/app/pagis-fix-the-login";

    fn session() -> CodingSession {
        CodingSession {
            id: CodingSessionId::from("cs-1".to_string()),
            workspace_id: WorkspaceId::from("ws-1".to_string()),
            agent_id: AgentId::from("ag-1".to_string()),
            harness_id: "claude".to_string(),
            harness_version: "1.0.0".to_string(),
            place: CodingSessionPlace::Host,
            host_id: Some(HostId::from("h-1".to_string())),
            directory: "/Users/bo/code/app".to_string(),
            working_directory: Some(WORKTREE.to_string()),
            worktree_branch: Some("pagis/fix-the-login".to_string()),
            approval_mode: SessionApprovalMode::Person,
            harness_mode: None,
            harness_modes: Vec::new(),
            model: None,
            thought_level: None,
            title: "Fix the login".to_string(),
            state: CodingSessionState::Working,
            end_reason: None,
            end_detail: None,
            acp_session_id: Some("acp-1".to_string()),
            channel_id: ChannelId::from("ch-1".to_string()),
            root_message_id: MessageId::from("msg-root".to_string()),
            message_id: MessageId::from("msg-block".to_string()),
            run_id: RunId::from("run-1".to_string()),
            usage: CodingSessionUsage::default(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
        }
    }

    #[test]
    fn an_execute_proposes_its_rule_and_envelopes_its_command() {
        let session = session();
        let card = Card::new(
            &session,
            "Air",
            &Permission {
                tool_call_id: "call-1".to_string(),
                title: Some("Run the tests".to_string()),
                kind: HarnessToolKind::Execute,
                command: Some("cargo test".to_string()),
                locations: Vec::new(),
            },
            None,
        );

        assert_eq!(card.title, "Claude Code wants to run a command");
        assert_eq!(card.body, "cargo test");
        assert_eq!(
            card.payload,
            json!({
                "session_id": "cs-1",
                "harness_id": "claude",
                "harness_name": "Claude Code",
                "host_id": "h-1",
                "host_name": "Air",
                "directory": WORKTREE,
                "tool_call_id": "call-1",
                "tool_kind": "execute",
                "command": "cargo test",
                "locations": [],
                "channel_id": "ch-1",
                "root_message_id": "msg-root",
                "action_title": "Claude Code wants to run a command",
                "body": "cargo test",
                "proposed_rules": ["cargo test"],
                "note": null,
            })
        );
        assert_eq!(
            card.text_content(&session),
            "Claude Code wants to run a command\n\
             [BEGIN UNTRUSTED source=coding_session:cs-1]\n\
             cargo test\n\
             [END UNTRUSTED source=coding_session:cs-1]"
        );
    }

    #[test]
    fn an_edit_outside_the_directory_shows_its_locations_and_proposes_no_rule() {
        let session = session();
        let locations = vec!["/etc/hosts".to_string(), "/etc/passwd".to_string()];
        let card = Card::new(
            &session,
            "Air",
            &Permission {
                tool_call_id: "call-2".to_string(),
                title: Some("Edit the hosts file".to_string()),
                kind: HarnessToolKind::Edit,
                command: None,
                locations: locations.clone(),
            },
            None,
        );

        assert_eq!(card.title, "Claude Code wants to edit files");
        assert_eq!(card.body, "/etc/hosts\n/etc/passwd");
        assert_eq!(card.payload["tool_kind"], "edit");
        assert_eq!(card.payload["command"], Value::Null);
        assert_eq!(card.payload["locations"], json!(locations));
        assert_eq!(card.payload["proposed_rules"], json!([]));
        assert_eq!(card.payload["body"], "/etc/hosts\n/etc/passwd");
        assert_eq!(
            card.text_content(&session),
            "Claude Code wants to edit files\n\
             [BEGIN UNTRUSTED source=coding_session:cs-1]\n\
             /etc/hosts\n/etc/passwd\n\
             [END UNTRUSTED source=coding_session:cs-1]"
        );
    }

    #[test]
    fn a_body_with_no_command_and_no_location_is_the_title_cut_to_its_limit() {
        let title = "x".repeat(BODY_CHARS + 10);
        let card = Card::new(
            &session(),
            "Air",
            &Permission {
                tool_call_id: "call-3".to_string(),
                title: Some(title),
                kind: HarnessToolKind::Fetch,
                command: None,
                locations: Vec::new(),
            },
            None,
        );

        assert_eq!(card.title, "Claude Code wants to fetch from the web");
        assert_eq!(card.body.chars().count(), BODY_CHARS);
    }

    #[test]
    fn the_card_of_an_escalation_holds_the_note() {
        let card = Card::new(
            &session(),
            "Air",
            &Permission {
                tool_call_id: "call-4".to_string(),
                title: None,
                kind: HarnessToolKind::Execute,
                command: Some("rm -rf build".to_string()),
                locations: Vec::new(),
            },
            Some("Delete the build directory?"),
        );

        assert_eq!(card.payload["note"], "Delete the build directory?");
        assert_eq!(card.body, "rm -rf build");
    }

    #[test]
    fn approve_takes_allow_once_and_deny_takes_reject_once_when_offered() {
        let every = [
            PermissionOptionKind::AllowAlways,
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::RejectOnce,
        ];
        let approved = Decided::of(&json!({"decision": "approved"}), &every);
        assert_eq!(approved.answer, PermissionAnswer::AllowOnce);
        assert_eq!(approved.decider, Some(Decider::Person));
        let denied = Decided::of(&json!({"decision": "denied"}), &every);
        assert_eq!(denied.answer, PermissionAnswer::RejectOnce);
        let only_always = [PermissionOptionKind::AllowAlways];
        let cancelled = Decided::of(&json!({"decision": "approved"}), &only_always);
        assert_eq!(cancelled.answer, PermissionAnswer::Cancel);
        assert_eq!(cancelled.option, None);
    }
}
