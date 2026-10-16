use super::*;
use async_trait::async_trait;

#[async_trait]
impl CalendarStore for FakeCalendarStore {
    async fn create(&self, event: &CalendarEvent) -> Result<(), StoreError> {
        event.data.bounds()?;
        self.0
            .lock()
            .unwrap()
            .calendar
            .insert(&event.workspace_id, event.id.clone(), event.clone())
    }
    async fn get(
        &self,
        workspace: &WorkspaceId,
        id: &CalendarEventId,
    ) -> Result<Option<CalendarEvent>, StoreError> {
        Ok(self.0.lock().unwrap().calendar.get(workspace, id).cloned())
    }
    async fn update(
        &self,
        workspace: &WorkspaceId,
        id: &CalendarEventId,
        value: &CalendarEventData,
    ) -> Result<bool, StoreError> {
        value.bounds()?;
        let mut data = self.0.lock().unwrap();
        let Some(event) = data.calendar.get_mut(workspace, id) else {
            return Ok(false);
        };
        event.data = value.clone();
        Ok(true)
    }
    async fn delete(
        &self,
        workspace: &WorkspaceId,
        id: &CalendarEventId,
    ) -> Result<bool, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .calendar
            .remove(workspace, id)
            .is_some())
    }
    async fn list(
        &self,
        workspace: &WorkspaceId,
        from: i64,
        to: i64,
    ) -> Result<Vec<CalendarOccurrence>, StoreError> {
        crate::calendar::occurrences(
            self.0
                .lock()
                .unwrap()
                .calendar
                .rows(workspace)
                .cloned()
                .collect(),
            CalendarRange::new(from, to)?,
        )
        .map_err(Into::into)
    }
}
