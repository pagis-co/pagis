//! Events of one Workspace Calendar.
use crate::db_err;
use async_trait::async_trait;
use pagis_core::{
    Author, CalendarEvent, CalendarEventData, CalendarEventId, CalendarOccurrence, CalendarRange,
    CalendarStore, StoreError, WorkspaceId,
};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PostgresCalendarStore {
    pool: PgPool,
}
impl PostgresCalendarStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
fn decode(row: sqlx::postgres::PgRow) -> Result<CalendarEvent, StoreError> {
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
impl CalendarStore for PostgresCalendarStore {
    async fn search(
        &self,
        workspace: &WorkspaceId,
        query: &str,
        limit: u32,
    ) -> Result<Vec<CalendarEvent>, StoreError> {
        sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = $1 AND (strpos(lower(data::jsonb->>'title'), lower($2)) > 0 OR strpos(lower(data::jsonb->>'description'), lower($2)) > 0 OR strpos(lower(data::jsonb->>'location'), lower($2)) > 0) ORDER BY starts_at DESC, id LIMIT $3").bind(workspace.as_str()).bind(query).bind(i64::from(limit))
            .fetch_all(&self.pool).await.map_err(db_err)?.into_iter().map(decode).collect()
    }

    async fn create(&self, event: &CalendarEvent) -> Result<(), StoreError> {
        let bounds = event.data.bounds()?;
        sqlx::query("INSERT INTO calendar_events (id, workspace_id, uid, author_kind, author_person_id, author_agent_id, data, starts_at, ends_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
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
        sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = $1 AND id = $2")
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
        let bounds = data.bounds()?;
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        if !data.exdates.is_empty() {
            let previous =
                sqlx::query("SELECT * FROM calendar_events WHERE workspace_id=$1 AND id=$2")
                    .bind(workspace.as_str())
                    .bind(id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_err)?
                    .map(decode)
                    .transpose()?;
            if previous.is_some_and(|old| {
                data.exdates
                    .iter()
                    .any(|date| !old.data.exdates.contains(date))
            }) {
                crate::audit_log::refuse_delete(&mut tx, workspace).await?;
            }
        }
        let updated = sqlx::query("UPDATE calendar_events SET data = $1, starts_at = $2, ends_by = $3 WHERE workspace_id = $4 AND id = $5")
            .bind(encode(data)?).bind(bounds.starts_at).bind(bounds.ends_by).bind(workspace.as_str()).bind(id.as_str())
            .execute(&mut *tx).await.map_err(db_err)?.rows_affected() == 1;
        tx.commit().await.map_err(db_err)?;
        Ok(updated)
    }
    async fn delete(
        &self,
        workspace: &WorkspaceId,
        id: &CalendarEventId,
    ) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        crate::audit_log::refuse_delete(&mut tx, workspace).await?;
        let deleted =
            sqlx::query("DELETE FROM calendar_events WHERE workspace_id = $1 AND id = $2")
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
        let events = sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = $1 AND starts_at < $2 AND (ends_by IS NULL OR ends_by > $3) ORDER BY starts_at, id")
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
        let events = sqlx::query("SELECT * FROM calendar_events WHERE workspace_id = $1 AND starts_at < $2 AND (ends_by IS NULL OR ends_by > $3) ORDER BY starts_at, id")
            .bind(workspace.as_str()).bind(range.to()).bind(range.from()).fetch_all(&self.pool).await.map_err(db_err)?
            .into_iter().map(decode).collect::<Result<Vec<_>, _>>()?;
        pagis_core::calendar::occurrences(events, range).map_err(Into::into)
    }
}
