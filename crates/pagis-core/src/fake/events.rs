use super::*;

#[async_trait::async_trait]
impl EventLog for FakeEventLog {
    async fn append(&self, event: NewEvent) -> Result<Event, StoreError> {
        let mut data = self.0.lock().unwrap();
        data.event_sequence += 1;
        let event = Event {
            id: EventId::generate(),
            seq: data.event_sequence,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: now_ms(),
        };
        data.events.push(event.clone());
        data.subscribers.retain(|(scope, after_seq, sender)| {
            !sender.is_closed()
                && (event.seq <= *after_seq
                    || !scope.covers(&event)
                    || sender.unbounded_send(event.clone()).is_ok())
        });
        Ok(event)
    }
    async fn list_after(
        &self,
        workspace_id: Option<&WorkspaceId>,
        after_seq: i64,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|row| {
                row.seq > after_seq && workspace_id.is_none_or(|w| &row.workspace_id == w)
            })
            .take(limit as usize)
            .cloned()
            .collect())
    }
    async fn latest_seq(&self) -> Result<i64, StoreError> {
        Ok(self.0.lock().unwrap().event_sequence)
    }
    async fn list_by_types(
        &self,
        workspace_id: &WorkspaceId,
        event_types: &[&str],
        before: Option<&EventId>,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        Ok(page(
            self.0
                .lock()
                .unwrap()
                .events
                .iter()
                .filter(|row| {
                    &row.workspace_id == workspace_id
                        && event_types.contains(&row.event_type.as_str())
                })
                .map(|row| (row.id.clone(), row.clone()))
                .collect(),
            before,
            limit,
        ))
    }
    async fn list_for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Vec<Event>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .events
            .iter()
            .filter(|row| &row.workspace_id == workspace_id && row.run_id.as_ref() == Some(run_id))
            .cloned()
            .collect())
    }
}

use futures::StreamExt;
/// An EventBus over an in-memory EventLog. Each subscription keeps all
/// matching events in sequence order until its stream is dropped.
#[derive(Clone, Default)]
pub struct FakeEventBus {
    log: FakeEventLog,
}
impl FakeEventBus {
    pub fn new(log: FakeEventLog) -> Self {
        Self { log }
    }
    pub fn log(&self) -> FakeEventLog {
        self.log.clone()
    }
}
#[async_trait::async_trait]
impl EventBus for FakeEventBus {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        self.log.append(event).await
    }
    async fn subscribe(&self, scope: EventScope, after_seq: Option<i64>) -> EventStream {
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        let mut data = self.log.0.lock().unwrap();
        if let Some(after) = after_seq {
            for event in data
                .events
                .iter()
                .filter(|event| event.seq > after && scope.covers(event))
            {
                sender
                    .unbounded_send(event.clone())
                    .expect("new stream is open");
            }
        }
        let after_seq = after_seq.unwrap_or(data.event_sequence);
        data.subscribers.push((scope, after_seq, sender));
        receiver.boxed()
    }
}
