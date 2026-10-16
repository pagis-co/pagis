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
        if data.calendar.get(workspace, id).is_some_and(|old| {
            value
                .exdates
                .iter()
                .any(|date| !old.data.exdates.contains(date))
        }) {
            super::audit::refuse_delete(&data, workspace)?;
        }
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
        let mut data = self.0.lock().unwrap();
        super::audit::refuse_delete(&data, workspace)?;
        Ok(data.calendar.remove(workspace, id).is_some())
    }
    async fn free_busy(
        &self,
        workspace: &WorkspaceId,
        from: i64,
        to: i64,
    ) -> Result<Vec<crate::BusyInterval>, StoreError> {
        crate::calendar::free_busy(
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
