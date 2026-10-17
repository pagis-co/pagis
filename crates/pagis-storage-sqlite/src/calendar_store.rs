//! Events of one Workspace Calendar.
use crate::db_err;
use async_trait::async_trait;
use pagis_core::{
    Author, CalendarEvent, CalendarEventData, CalendarEventId, CalendarOccurrence, CalendarRange,
    CalendarStore, StoreError, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

#[derive(Clone)]
pub struct SqliteCalendarStore {
    pool: SqlitePool,
}
impl SqliteCalendarStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}
fn decode(row: sqlx::sqlite::SqliteRow) -> Result<CalendarEvent, StoreError> {
    Ok(CalendarEvent {
        id: CalendarEventId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        uid: row.get("uid"),
        created_by: Author::from_columns(
            &row.get::<String, _>("author_kind"),
            row.get("author_person_id"),
            row.get("author_agent_id"),
        )
        .map_err(StoreError::Corrupt)?,
        data: serde_json::from_str(row.get::<&str, _>("data"))
            .map_err(|e| StoreError::Corrupt(e.to_string()))?,
    })
}
fn encode(value: &CalendarEventData) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Corrupt(e.to_string()))
}
#[async_trait]
impl CalendarStore for SqliteCalendarStore {
    async fn attendee_events(
        &self,
        workspace: &WorkspaceId,
        email: &str,
        from: i64,
        to: i64,
    ) -> Result<Vec<CalendarEvent>, StoreError> {
        let range = CalendarRange::new(from, to)?;
        sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = ? AND starts_at < ? AND (ends_by IS NULL OR ends_by > ?) AND EXISTS (SELECT 1 FROM json_each(data, '$.attendees') WHERE lower(json_extract(value, '$.email')) = lower(?)) ORDER BY starts_at, id")
            .bind(workspace.as_str())
            .bind(range.to())
            .bind(range.from())
            .bind(email)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?
            .into_iter()
            .map(decode)
            .collect()
    }

    async fn linked_occurrences(
        &self,
        workspace: &WorkspaceId,
        uid: &str,
    ) -> Result<Vec<CalendarEvent>, StoreError> {
        sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = ? AND json_extract(data, '$.invitation.occurrence.series_uid') = ?")
            .bind(workspace.as_str()).bind(uid).fetch_all(&self.pool).await.map_err(db_err)?
            .into_iter().map(decode).collect()
    }
    async fn get_by_uid(
        &self,
        workspace: &WorkspaceId,
        uid: &str,
    ) -> Result<Option<CalendarEvent>, StoreError> {
        sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = ? AND uid = ?")
            .bind(workspace.as_str())
            .bind(uid)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
            .map(decode)
            .transpose()
    }

    async fn search(
        &self,
        workspace: &WorkspaceId,
        query: &str,
        limit: u32,
    ) -> Result<Vec<CalendarEvent>, StoreError> {
        sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = ? AND (instr(lower(json_extract(data, '$.title')), lower(?)) > 0 OR instr(lower(json_extract(data, '$.description')), lower(?)) > 0 OR instr(lower(json_extract(data, '$.location')), lower(?)) > 0) ORDER BY starts_at DESC, id LIMIT ?").bind(workspace.as_str()).bind(query).bind(query).bind(query).bind(i64::from(limit))
            .fetch_all(&self.pool).await.map_err(db_err)?.into_iter().map(decode).collect()
    }

    async fn create(&self, event: &CalendarEvent) -> Result<(), StoreError> {
        let bounds = event.data.bounds()?;
        sqlx::query("INSERT INTO calendar_events (id, workspace_id, uid, author_kind, author_person_id, author_agent_id, data, starts_at, ends_by) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(event.id.as_str())
            .bind(event.workspace_id.as_str())
            .bind(&event.uid)
            .bind(event.created_by.kind())
            .bind(event.created_by.person_id().map(|id| id.as_str()))
            .bind(event.created_by.agent_id().map(|id| id.as_str()))
            .bind(encode(&event.data)?)
            .bind(bounds.starts_at)
            .bind(bounds.ends_by)
            .execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }
    async fn get(
        &self,
        workspace: &WorkspaceId,
        id: &CalendarEventId,
    ) -> Result<Option<CalendarEvent>, StoreError> {
        sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = ? AND id = ?")
            .bind(workspace.as_str())
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?
            .map(decode)
            .transpose()
    }
    async fn update(
        &self,
        workspace: &WorkspaceId,
        id: &CalendarEventId,
        data: &CalendarEventData,
    ) -> Result<bool, StoreError> {
        let Some(mut event) = self.get(workspace, id).await? else {
            return Ok(false);
        };
        event.data = data.clone();
        self.change(&event).await
    }
    async fn change(&self, event: &CalendarEvent) -> Result<bool, StoreError> {
        let workspace = &event.workspace_id;
        let id = &event.id;
        let data = &event.data;
        let bounds = data.bounds()?;
        let mut tx = crate::begin_write(&self.pool).await.map_err(db_err)?;
        let previous = sqlx::query("SELECT * FROM calendar_events WHERE workspace_id=?1 AND id=?2")
            .bind(workspace.as_str())
            .bind(id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?
            .map(decode)
            .transpose()?;
        let Some(previous) = previous else {
            return Ok(false);
        };
        if !data.can_replace(&previous.data) {
            return Ok(false);
        }
        if data
            .exdates
            .iter()
            .any(|date| !previous.data.exdates.contains(date))
        {
            crate::audit_log::refuse_delete(&mut tx, workspace).await?;
        }
        let updated = sqlx::query("UPDATE calendar_events SET data = ?, starts_at = ?, ends_by = ?, author_kind = ?, author_person_id = ?, author_agent_id = ? WHERE workspace_id = ? AND id = ?")
            .bind(encode(data)?).bind(bounds.starts_at).bind(bounds.ends_by).bind(event.created_by.kind()).bind(event.created_by.person_id().map(|id| id.as_str())).bind(event.created_by.agent_id().map(|id| id.as_str())).bind(workspace.as_str()).bind(id.as_str())
            .execute(&mut *tx).await.map_err(db_err)?.rows_affected() == 1;
        tx.commit().await.map_err(db_err)?;
        Ok(updated)
    }
    async fn delete(
        &self,
        workspace: &WorkspaceId,
        id: &CalendarEventId,
    ) -> Result<bool, StoreError> {
        let mut tx = crate::begin_write(&self.pool).await.map_err(db_err)?;
        crate::audit_log::refuse_delete(&mut tx, workspace).await?;
        let deleted = sqlx::query("DELETE FROM calendar_events WHERE workspace_id = ? AND id = ?")
            .bind(workspace.as_str())
            .bind(id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?
            .rows_affected()
            == 1;
        tx.commit().await.map_err(db_err)?;
        Ok(deleted)
    }
    async fn free_busy(
        &self,
        workspace: &WorkspaceId,
        from: i64,
        to: i64,
    ) -> Result<Vec<pagis_core::BusyInterval>, StoreError> {
        let range = CalendarRange::new(from, to)?;
        let events = sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = ? AND starts_at < ? AND (ends_by IS NULL OR ends_by > ?) ORDER BY starts_at, id")
            .bind(workspace.as_str()).bind(range.to()).bind(range.from()).fetch_all(&self.pool).await.map_err(db_err)?
            .into_iter().map(decode).collect::<Result<Vec<_>, _>>()?;
        pagis_core::calendar::free_busy(events, range).map_err(Into::into)
    }
    async fn list(
        &self,
        workspace: &WorkspaceId,
        from: i64,
        to: i64,
    ) -> Result<Vec<CalendarOccurrence>, StoreError> {
        let range = CalendarRange::new(from, to)?;
        let events = sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = ? AND starts_at < ? AND (ends_by IS NULL OR ends_by > ?) ORDER BY starts_at, id")
            .bind(workspace.as_str()).bind(range.to()).bind(range.from()).fetch_all(&self.pool).await.map_err(db_err)?
            .into_iter().map(decode).collect::<Result<Vec<_>, _>>()?;
        pagis_core::calendar::occurrences(events, range).map_err(Into::into)
    }
}
