//! The events that tell a client that a Coding Session changed
//! (ADR-0033).
//!
//! [`CodingSessionFeed`] is the Coding Session store of the daemon: it
//! writes through the inner store, then publishes one durable event for
//! each write that a client shows. `coding_session.changed` reports a
//! write of the record. `coding_session.transcript` reports a new row at
//! once, and a row that grows by merges at most once a second. A client
//! reads the rows again from the highest `seq` that it holds, less one,
//! so the next event also gives it the last words of a message.
//!
//! No event carries the text of a row or the title of a session: the
//! transcript is foreign text, and a client reads it over REST.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_core::{
    AgentId, Clock, CodingSession, CodingSessionEvent, CodingSessionEventKind, CodingSessionId,
    CodingSessionState, CodingSessionStore, EventBus, ModelTokenOwner, NewCodingSessionEvent,
    NewEvent, StoreError, UnixMillis, WorkspaceId,
};
use serde_json::json;

/// The event of each write of a Coding Session record.
pub const CHANGED_EVENT: &str = "coding_session.changed";

/// The event of a new or longer transcript row.
pub const TRANSCRIPT_EVENT: &str = "coding_session.transcript";

/// The shortest time between two `coding_session.transcript` events of
/// one session that report no new row.
const MERGE_INTERVAL_MS: UnixMillis = 1_000;

/// The Coding Session store that reports each write as an event.
pub struct CodingSessionFeed {
    inner: Arc<dyn CodingSessionStore>,
    bus: Arc<dyn EventBus>,
    clock: Arc<dyn Clock>,
    /// The last `coding_session.transcript` of each session that has one
    /// and is not terminal.
    reported: Mutex<HashMap<CodingSessionId, Reported>>,
}

/// One `coding_session.transcript` event.
#[derive(Debug, Clone, Copy)]
struct Reported {
    seq: i64,
    at: UnixMillis,
}

impl CodingSessionFeed {
    pub fn new(
        inner: Arc<dyn CodingSessionStore>,
        bus: Arc<dyn EventBus>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            inner,
            bus,
            clock,
            reported: Mutex::default(),
        }
    }

    /// Publishes one event. The write that it reports is done, so a
    /// failed publish is logged and the write still succeeds.
    async fn publish(&self, event: NewEvent) {
        let event_type = event.event_type.clone();
        if let Err(error) = self.bus.publish(event).await {
            tracing::warn!(%error, event_type, "a Coding Session event was not published");
        }
    }

    async fn changed(&self, session: &CodingSession) {
        self.publish(NewEvent {
            workspace_id: session.workspace_id.clone(),
            event_type: CHANGED_EVENT.to_string(),
            agent_id: Some(session.agent_id.clone()),
            run_id: None,
            channel_id: Some(session.channel_id.clone()),
            payload: json!({
                "coding_session_id": session.id,
                "state": session.state,
                "end_reason": session.end_reason,
            }),
        })
        .await;
    }

    /// Whether a write that reaches `seq` gets an event now. It records
    /// the event when it does.
    fn report_now(&self, session_id: &CodingSessionId, seq: i64) -> bool {
        let now = self.clock.now_ms();
        let mut reported = self.reported.lock().expect("the reported transcripts");
        let due = reported
            .get(session_id)
            .is_none_or(|last| seq > last.seq || now - last.at >= MERGE_INTERVAL_MS);
        if due {
            reported.insert(session_id.clone(), Reported { seq, at: now });
        }
        due
    }
}

#[async_trait]
impl CodingSessionStore for CodingSessionFeed {
    async fn insert(&self, session: &CodingSession) -> Result<(), StoreError> {
        self.inner.insert(session).await?;
        self.changed(session).await;
        Ok(())
    }

    async fn update(&self, session: &CodingSession) -> Result<bool, StoreError> {
        let written = self.inner.update(session).await?;
        if written {
            if session.state.is_terminal() {
                self.reported
                    .lock()
                    .expect("the reported transcripts")
                    .remove(&session.id);
            }
            self.changed(session).await;
        }
        Ok(written)
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &CodingSessionId,
    ) -> Result<Option<CodingSession>, StoreError> {
        self.inner.get(workspace_id, id).await
    }

    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: Option<&AgentId>,
        state: Option<CodingSessionState>,
        before: Option<&CodingSessionId>,
        limit: u32,
    ) -> Result<Vec<CodingSession>, StoreError> {
        self.inner
            .list(workspace_id, agent_id, state, before, limit)
            .await
    }

    async fn count_open(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
    ) -> Result<u32, StoreError> {
        self.inner.count_open(workspace_id, agent_id).await
    }

    async fn list_open(&self) -> Result<Vec<CodingSession>, StoreError> {
        self.inner.list_open().await
    }

    async fn append_event(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        event: NewCodingSessionEvent,
    ) -> Result<Vec<CodingSessionEvent>, StoreError> {
        let rows = self
            .inner
            .append_event(workspace_id, coding_session_id, event)
            .await?;
        let Some(last) = rows.iter().max_by_key(|row| row.seq) else {
            return Ok(rows);
        };
        if self.report_now(coding_session_id, last.seq) {
            self.publish(NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: TRANSCRIPT_EVENT.to_string(),
                agent_id: None,
                run_id: None,
                channel_id: None,
                payload: json!({
                    "coding_session_id": coding_session_id,
                    "seq": last.seq,
                    "kind": last.kind,
                }),
            })
            .await;
        }
        Ok(rows)
    }

    async fn list_events(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        after: Option<i64>,
        limit: u32,
    ) -> Result<Vec<CodingSessionEvent>, StoreError> {
        self.inner
            .list_events(workspace_id, coding_session_id, after, limit)
            .await
    }

    async fn latest_event(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        kinds: &[CodingSessionEventKind],
    ) -> Result<Option<CodingSessionEvent>, StoreError> {
        self.inner
            .latest_event(workspace_id, coding_session_id, kinds)
            .await
    }

    async fn set_model_token(
        &self,
        workspace_id: &WorkspaceId,
        coding_session_id: &CodingSessionId,
        hash: &str,
    ) -> Result<bool, StoreError> {
        self.inner
            .set_model_token(workspace_id, coding_session_id, hash)
            .await
    }

    async fn model_token_owner(&self, hash: &str) -> Result<Option<ModelTokenOwner>, StoreError> {
        self.inner.model_token_owner(hash).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use pagis_core::coding_session::fold;
    use pagis_core::{
        ChannelId, CodingSessionPlace, CodingSessionUsage, Event, EventId, EventScope, EventStream,
        HostId, MessageId, RunId, SessionApprovalMode, TranscriptWrite,
    };
    use serde_json::Value;

    use super::*;

    use CodingSessionEventKind as Kind;

    /// A clock that a test moves by hand.
    #[derive(Default)]
    struct ManualClock(AtomicI64);

    impl ManualClock {
        fn advance(&self, ms: i64) {
            self.0.fetch_add(ms, Ordering::SeqCst);
        }
    }

    impl Clock for ManualClock {
        fn now_ms(&self) -> UnixMillis {
            self.0.load(Ordering::SeqCst)
        }
    }

    /// A bus that keeps each event that it gets.
    #[derive(Default)]
    struct RecordingBus(Mutex<Vec<NewEvent>>);

    impl RecordingBus {
        fn events(&self, event_type: &str) -> Vec<NewEvent> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.event_type == event_type)
                .cloned()
                .collect()
        }
    }

    #[async_trait]
    impl EventBus for RecordingBus {
        async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
            let mut events = self.0.lock().unwrap();
            events.push(event.clone());
            Ok(Event {
                id: EventId::generate(),
                seq: events.len() as i64,
                workspace_id: event.workspace_id,
                event_type: event.event_type,
                agent_id: event.agent_id,
                run_id: event.run_id,
                channel_id: event.channel_id,
                payload: event.payload,
                created_at: 0,
            })
        }

        async fn subscribe(&self, _: EventScope, _: Option<i64>) -> EventStream {
            Box::pin(futures::stream::empty())
        }
    }

    /// The writes of a store, in memory: the records and one transcript,
    /// folded as a store folds it.
    #[derive(Default)]
    struct MemoryStore {
        records: Mutex<HashMap<CodingSessionId, CodingSession>>,
        rows: Mutex<Vec<CodingSessionEvent>>,
    }

    #[async_trait]
    impl CodingSessionStore for MemoryStore {
        async fn insert(&self, session: &CodingSession) -> Result<(), StoreError> {
            self.records
                .lock()
                .unwrap()
                .insert(session.id.clone(), session.clone());
            Ok(())
        }

        async fn update(&self, session: &CodingSession) -> Result<bool, StoreError> {
            let mut records = self.records.lock().unwrap();
            let Some(record) = records.get_mut(&session.id) else {
                return Ok(false);
            };
            *record = session.clone();
            Ok(true)
        }

        async fn append_event(
            &self,
            workspace_id: &WorkspaceId,
            coding_session_id: &CodingSessionId,
            event: NewCodingSessionEvent,
        ) -> Result<Vec<CodingSessionEvent>, StoreError> {
            let mut rows = self.rows.lock().unwrap();
            let mut written = Vec::new();
            for write in fold(rows.last(), event) {
                match write {
                    TranscriptWrite::Replace { payload, .. } => {
                        let row = rows.last_mut().expect("a replace has a last row");
                        row.payload = payload;
                        written.push(row.clone());
                    }
                    TranscriptWrite::Append {
                        seq,
                        at,
                        kind,
                        payload,
                    } => {
                        let row = CodingSessionEvent {
                            workspace_id: workspace_id.clone(),
                            coding_session_id: coding_session_id.clone(),
                            seq,
                            at,
                            kind,
                            payload,
                        };
                        rows.push(row.clone());
                        written.push(row);
                    }
                }
            }
            Ok(written)
        }

        async fn get(
            &self,
            _: &WorkspaceId,
            _: &CodingSessionId,
        ) -> Result<Option<CodingSession>, StoreError> {
            unreachable!("the feed tests only write")
        }
        async fn list(
            &self,
            _: &WorkspaceId,
            _: Option<&AgentId>,
            _: Option<CodingSessionState>,
            _: Option<&CodingSessionId>,
            _: u32,
        ) -> Result<Vec<CodingSession>, StoreError> {
            unreachable!("the feed tests only write")
        }
        async fn count_open(&self, _: &WorkspaceId, _: &AgentId) -> Result<u32, StoreError> {
            unreachable!("the feed tests only write")
        }
        async fn list_open(&self) -> Result<Vec<CodingSession>, StoreError> {
            unreachable!("the feed tests only write")
        }
        async fn list_events(
            &self,
            _: &WorkspaceId,
            _: &CodingSessionId,
            _: Option<i64>,
            _: u32,
        ) -> Result<Vec<CodingSessionEvent>, StoreError> {
            unreachable!("the feed tests only write")
        }
        async fn latest_event(
            &self,
            _: &WorkspaceId,
            _: &CodingSessionId,
            _: &[CodingSessionEventKind],
        ) -> Result<Option<CodingSessionEvent>, StoreError> {
            unreachable!("the feed tests only write")
        }
        async fn set_model_token(
            &self,
            _: &WorkspaceId,
            _: &CodingSessionId,
            _: &str,
        ) -> Result<bool, StoreError> {
            unreachable!("the feed tests only write")
        }
        async fn model_token_owner(&self, _: &str) -> Result<Option<ModelTokenOwner>, StoreError> {
            unreachable!("the feed tests only write")
        }
    }

    struct World {
        feed: CodingSessionFeed,
        bus: Arc<RecordingBus>,
        clock: Arc<ManualClock>,
        session: CodingSession,
    }

    fn world() -> World {
        let bus = Arc::new(RecordingBus::default());
        let clock = Arc::new(ManualClock::default());
        let feed = CodingSessionFeed::new(
            Arc::new(MemoryStore::default()),
            Arc::clone(&bus) as _,
            Arc::clone(&clock) as _,
        );
        let session = CodingSession {
            id: CodingSessionId::generate(),
            workspace_id: WorkspaceId::generate(),
            agent_id: AgentId::generate(),
            harness_id: "claude".to_string(),
            harness_version: "0.87.0".to_string(),
            place: CodingSessionPlace::Host,
            host_id: Some(HostId::generate()),
            directory: "/Users/bo/code/app".to_string(),
            working_directory: None,
            worktree_branch: None,
            approval_mode: SessionApprovalMode::Person,
            harness_mode: None,
            harness_modes: Vec::new(),
            title: "The secret title".to_string(),
            state: CodingSessionState::Starting,
            end_reason: None,
            end_detail: None,
            acp_session_id: None,
            channel_id: ChannelId::generate(),
            root_message_id: MessageId::generate(),
            message_id: MessageId::generate(),
            run_id: RunId::generate(),
            usage: CodingSessionUsage::default(),
            created_at: 0,
            updated_at: 0,
            ended_at: None,
        };
        World {
            feed,
            bus,
            clock,
            session,
        }
    }

    impl World {
        async fn chunk(&self, text: &str, message_id: &str) {
            self.feed
                .append_event(
                    &self.session.workspace_id,
                    &self.session.id,
                    NewCodingSessionEvent {
                        at: self.clock.now_ms(),
                        kind: Kind::AgentMessage,
                        payload: json!({"text": text, "message_id": message_id}),
                    },
                )
                .await
                .unwrap();
        }

        fn transcript_payloads(&self) -> Vec<Value> {
            self.bus
                .events(TRANSCRIPT_EVENT)
                .into_iter()
                .map(|event| event.payload)
                .collect()
        }
    }

    #[tokio::test]
    async fn an_insert_and_an_update_publish_coding_session_changed() {
        let mut world = world();

        world.feed.insert(&world.session).await.unwrap();
        world.session.state = CodingSessionState::Closed;
        world.session.end_reason = Some("stopped".to_string());
        assert!(world.feed.update(&world.session).await.unwrap());

        let changed = world.bus.events(CHANGED_EVENT);
        assert_eq!(changed.len(), 2);
        assert_eq!(
            changed[0].payload,
            json!({
                "coding_session_id": world.session.id.as_str(),
                "state": "starting",
                "end_reason": null,
            })
        );
        assert_eq!(
            changed[1].payload,
            json!({
                "coding_session_id": world.session.id.as_str(),
                "state": "closed",
                "end_reason": "stopped",
            })
        );
        assert_eq!(changed[1].agent_id.as_ref(), Some(&world.session.agent_id));
        assert_eq!(
            changed[1].channel_id.as_ref(),
            Some(&world.session.channel_id)
        );
        assert_eq!(changed[1].workspace_id, world.session.workspace_id);
    }

    #[tokio::test]
    async fn an_update_of_a_session_that_the_store_does_not_hold_publishes_nothing() {
        let world = world();

        assert!(!world.feed.update(&world.session).await.unwrap());

        assert!(world.bus.events(CHANGED_EVENT).is_empty());
    }

    #[tokio::test]
    async fn a_new_row_publishes_coding_session_transcript_at_once() {
        let world = world();

        world.chunk("Hello.", "m1").await;
        world.chunk("Bye.", "m2").await;

        assert_eq!(
            world.transcript_payloads(),
            [
                json!({"coding_session_id": world.session.id.as_str(), "seq": 1, "kind": "agent_message"}),
                json!({"coding_session_id": world.session.id.as_str(), "seq": 2, "kind": "agent_message"}),
            ]
        );
    }

    #[tokio::test]
    async fn ten_merges_into_one_row_in_one_second_publish_one_event() {
        let world = world();

        for _ in 0..10 {
            world.chunk("word ", "m1").await;
            world.clock.advance(90);
        }

        assert_eq!(world.transcript_payloads().len(), 1);
    }

    #[tokio::test]
    async fn a_merge_after_one_second_publishes_again() {
        let world = world();
        world.chunk("Hello, ", "m1").await;
        world.chunk("dear ", "m1").await;

        world.clock.advance(1_000);
        world.chunk("world.", "m1").await;

        assert_eq!(
            world.transcript_payloads(),
            [
                json!({"coding_session_id": world.session.id.as_str(), "seq": 1, "kind": "agent_message"}),
                json!({"coding_session_id": world.session.id.as_str(), "seq": 1, "kind": "agent_message"}),
            ]
        );
    }

    #[tokio::test]
    async fn a_terminal_update_forgets_the_last_transcript_event_of_the_session() {
        let mut world = world();
        world.feed.insert(&world.session).await.unwrap();
        world.chunk("Hello.", "m1").await;

        world.session.state = CodingSessionState::Failed;
        world.session.end_reason = Some("harness_exited".to_string());
        world.feed.update(&world.session).await.unwrap();

        assert!(world.feed.reported.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_event_holds_the_text_of_a_row_or_the_title() {
        let mut world = world();
        world.feed.insert(&world.session).await.unwrap();
        world.chunk("The secret words of the harness.", "m1").await;
        world.session.state = CodingSessionState::Idle;
        world.feed.update(&world.session).await.unwrap();

        let events = world.bus.0.lock().unwrap().clone();
        assert_eq!(events.len(), 3);
        for event in events {
            let payload = event.payload.to_string();
            assert!(!payload.contains("secret"), "{payload}");
        }
    }
}
