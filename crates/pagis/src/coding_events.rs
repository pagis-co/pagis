//! Where the Coding Sessions meet the Trigger module (ADR-0033).
//!
//! The session runtime stops at two seams: it makes the Session Rule of
//! each session when the session starts and ends it when the session
//! closes or fails, and it hands each piece of news of a session to
//! `ingest`. Both are filled here, because the Trigger module reads the
//! declarations that the broker holds and is therefore built after the
//! session runtime. It is the call pair next door, for Coding Sessions.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{CODING_SESSION_ENDED, CODING_SESSION_EVENT_KINDS};
use pagis_coding::{
    SESSION_END_INSTRUCTION, SESSION_RULE_INSTRUCTION, SessionEvents, SessionRuleError,
    SessionRules, session_rule_name,
};
use pagis_core::{CodingSession, CreatorKind, EventSource, IngestBatch, now_ms};
use pagis_trigger::{NewSubscription, Trigger};

/// The Session Rule of ADR-0033: one Event Subscription for each kind of
/// session event, in the session's Thread, with the provenance of the
/// user.
pub struct TriggerSessionRules(pub Arc<Trigger>);

#[async_trait]
impl SessionRules for TriggerSessionRules {
    async fn create(&self, session: &CodingSession) -> Result<(), SessionRuleError> {
        // A rule matches one declared kind (ADR-0006), so the rule of a
        // session is one subscription for each kind. They share the
        // source, so one `end_source` ends all of them.
        for kind in CODING_SESSION_EVENT_KINDS {
            self.0
                .create_subscription(session_rule(session, kind))
                .await
                .map_err(|error| SessionRuleError(error.to_string()))?;
        }
        Ok(())
    }

    async fn end(&self, session: &CodingSession) -> Result<(), SessionRuleError> {
        self.0
            .end_source(
                &session.workspace_id,
                &EventSource::coding_session(session.id.clone()),
                now_ms(),
            )
            .await
            .map(|_| ())
            .map_err(|error| SessionRuleError(error.to_string()))
    }
}

/// The Event Subscription of one kind of event in the Session Rule of
/// `session`, with the provenance of the user. The supervision wakes the
/// Agent in the session's Thread. The end wakes it at the place where the
/// session started, so its report shows where the Person asked.
fn session_rule(session: &CodingSession, kind: &str) -> NewSubscription {
    let (root_message_id, instruction) = if kind == CODING_SESSION_ENDED {
        (session.starting_thread().cloned(), SESSION_END_INSTRUCTION)
    } else {
        (
            Some(session.root_message_id.clone()),
            SESSION_RULE_INSTRUCTION,
        )
    };
    NewSubscription {
        workspace_id: session.workspace_id.clone(),
        agent_id: session.agent_id.clone(),
        source: EventSource::coding_session(session.id.clone()),
        event_kind: kind.to_string(),
        name: session_rule_name(session),
        instruction: instruction.to_string(),
        channel_id: session.channel_id.clone(),
        root_message_id,
        // The source names the one session.
        filter: serde_json::json!({}),
        // The Person approved the start, so the rule is the user's.
        // Nobody edits it: it is daemon housekeeping.
        creator: CreatorKind::User,
        // A rule wakes only for the events after its activation. The rule
        // watches the whole session, so it is active from the instant
        // before the session's first instant: an event in the first
        // millisecond of the session wakes it too.
        now: session.created_at - 1,
    }
}

/// The session news's `ingest` seam over the Trigger module.
pub struct TriggerSessionEvents(pub Arc<Trigger>);

#[async_trait]
impl SessionEvents for TriggerSessionEvents {
    async fn ingest(&self, batch: IngestBatch) -> Result<(), SessionRuleError> {
        self.0
            .ingest(batch)
            .await
            .map(|_| ())
            .map_err(|error| SessionRuleError(error.to_string()))
    }
}

/// The Session Rule slot, filled once the Trigger module exists. No Agent
/// starts a session before the daemon has finished starting, so an empty
/// slot is a fault and reads as one.
#[derive(Default)]
pub struct DeferredSessionRules {
    inner: std::sync::OnceLock<Arc<dyn SessionRules>>,
}

impl DeferredSessionRules {
    /// Fill the slot. A second call keeps the first rule maker.
    pub fn set(&self, rules: Arc<dyn SessionRules>) {
        let _ = self.inner.set(rules);
    }

    fn rules(&self) -> Result<&Arc<dyn SessionRules>, SessionRuleError> {
        self.inner.get().ok_or_else(not_ready)
    }
}

#[async_trait]
impl SessionRules for DeferredSessionRules {
    async fn create(&self, session: &CodingSession) -> Result<(), SessionRuleError> {
        self.rules()?.create(session).await
    }

    async fn end(&self, session: &CodingSession) -> Result<(), SessionRuleError> {
        self.rules()?.end(session).await
    }
}

/// The session news slot, filled once the Trigger module exists.
#[derive(Default)]
pub struct DeferredSessionEvents {
    inner: std::sync::OnceLock<Arc<dyn SessionEvents>>,
}

impl DeferredSessionEvents {
    /// Fill the slot. A second call keeps the first sink.
    pub fn set(&self, events: Arc<dyn SessionEvents>) {
        let _ = self.inner.set(events);
    }
}

#[async_trait]
impl SessionEvents for DeferredSessionEvents {
    async fn ingest(&self, batch: IngestBatch) -> Result<(), SessionRuleError> {
        match self.inner.get() {
            Some(events) => events.ingest(batch).await,
            None => Err(not_ready()),
        }
    }
}

fn not_ready() -> SessionRuleError {
    SessionRuleError("the trigger module is not ready on this daemon".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagis_core::{AgentId, ChannelId, HostId, RunId, WorkspaceId};

    /// A turn that ends in the millisecond that the session starts in
    /// wakes the Agent: the rule is active before that instant.
    #[test]
    fn the_session_rule_is_active_before_the_first_instant_of_the_session() {
        let session = pagis_testkit::fixture::coding_session(
            &WorkspaceId::generate(),
            &AgentId::generate(),
            &RunId::generate(),
            &ChannelId::generate(),
            &HostId::generate(),
        );

        for kind in CODING_SESSION_EVENT_KINDS {
            let rule = session_rule(&session, kind);

            assert!(rule.now < session.created_at, "{kind}");
            assert_eq!(rule.event_kind, kind);
        }
    }

    /// The supervision stays in the session's Thread, and the end wakes
    /// the Agent where the session started: the top level when the block
    /// is the root, else the Thread of the starting Run.
    #[test]
    fn the_end_of_a_session_wakes_the_agent_where_the_session_started() {
        let mut session = pagis_testkit::fixture::coding_session(
            &WorkspaceId::generate(),
            &AgentId::generate(),
            &RunId::generate(),
            &ChannelId::generate(),
            &HostId::generate(),
        );
        session.root_message_id = session.message_id.clone();

        let ended = session_rule(&session, CODING_SESSION_ENDED);
        assert_eq!(ended.root_message_id, None);
        assert_eq!(ended.instruction, SESSION_END_INSTRUCTION);
        for kind in CODING_SESSION_EVENT_KINDS
            .into_iter()
            .filter(|kind| *kind != CODING_SESSION_ENDED)
        {
            let rule = session_rule(&session, kind);
            assert_eq!(rule.root_message_id, Some(session.root_message_id.clone()));
            assert_eq!(rule.instruction, SESSION_RULE_INSTRUCTION);
        }

        session.root_message_id = pagis_core::MessageId::generate();
        let ended = session_rule(&session, CODING_SESSION_ENDED);
        assert_eq!(ended.root_message_id, Some(session.root_message_id.clone()));
    }
}
