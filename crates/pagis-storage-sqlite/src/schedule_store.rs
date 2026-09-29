use async_trait::async_trait;
use pagis_core::{
    AgentId, ChannelId, EventSubscriptionId, MessageId, RunId, Schedule, ScheduleId,
    ScheduleOccurrence, ScheduleOccurrenceId, ScheduleRevision, ScheduleStore, StoreError,
    UnixMillis, Wakeup, WakeupId, WakeupRule, WorkspaceId, subject_page::OpenSchedule,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

pub(crate) const SCHEDULE_COLUMNS: &str = "id, workspace_id, agent_id, name, instruction, \
    (SELECT page.path FROM schedule_subject_pages page WHERE page.schedule_id = schedules.id) AS subject_page_path, channel_id, \
    root_message_id, kind, cron_expression, interval_ms, anchor_at, timezone, scheduled_at, \
    next_due_at, state, revision, approved_revision, creator, creating_run_id, created_at, \
    updated_at, archived_at, (SELECT r.state FROM wakeups w JOIN runs r ON r.id = w.run_id \
    WHERE w.schedule_id = schedules.id ORDER BY w.started_at DESC, w.id DESC LIMIT 1) AS last_result";
const REVISION_COLUMNS: &str = "schedule_id, revision, agent_id, name, instruction, \
    (SELECT page.path FROM schedule_subject_pages page WHERE page.schedule_id = schedule_revisions.schedule_id) AS subject_page_path, channel_id, \
    root_message_id, kind, cron_expression, interval_ms, anchor_at, timezone, scheduled_at, \
    created_at, creating_run_id";
const OCCURRENCE_COLUMNS: &str = "id, workspace_id, schedule_id, schedule_revision, scheduled_at, \
    processed_at, outcome, wakeup_id";
pub(crate) const WAKEUP_COLUMNS: &str = "id, workspace_id, source_kind, schedule_id, \
    subscription_id, subject_paths, rule_revision, rule_name, agent_id, channel_id, root_message_id, instruction, \
    scheduled_at, state, run_id, source_count, created_at, started_at, historical";

#[derive(Clone)]
pub struct SqliteScheduleStore {
    pool: SqlitePool,
}

impl SqliteScheduleStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

pub(crate) fn row_to_schedule(row: &sqlx::sqlite::SqliteRow) -> Result<Schedule, StoreError> {
    Ok(Schedule {
        id: ScheduleId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        name: row.get("name"),
        instruction: row.get("instruction"),
        subject_page_path: row.get("subject_page_path"),
        channel_id: ChannelId::from(row.get::<String, _>("channel_id")),
        root_message_id: row
            .get::<Option<String>, _>("root_message_id")
            .map(MessageId::from),
        kind: row
            .get::<String, _>("kind")
            .parse()
            .map_err(StoreError::Corrupt)?,
        cron_expression: row.get("cron_expression"),
        interval_ms: row.get("interval_ms"),
        anchor_at: row.get("anchor_at"),
        timezone: row.get("timezone"),
        scheduled_at: row.get("scheduled_at"),
        next_due_at: row.get("next_due_at"),
        last_result: row.get("last_result"),
        state: row
            .get::<String, _>("state")
            .parse()
            .map_err(StoreError::Corrupt)?,
        revision: u32::try_from(row.get::<i64, _>("revision"))
            .map_err(|error| StoreError::Corrupt(format!("revision: {error}")))?,
        approved_revision: row
            .get::<Option<i64>, _>("approved_revision")
            .map(u32::try_from)
            .transpose()
            .map_err(|error| StoreError::Corrupt(format!("approved_revision: {error}")))?,
        creator: row
            .get::<String, _>("creator")
            .parse()
            .map_err(StoreError::Corrupt)?,
        creating_run_id: row
            .get::<Option<String>, _>("creating_run_id")
            .map(RunId::from),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        archived_at: row.get("archived_at"),
    })
}

fn row_to_revision(row: &sqlx::sqlite::SqliteRow) -> Result<ScheduleRevision, StoreError> {
    Ok(ScheduleRevision {
        schedule_id: ScheduleId::from(row.get::<String, _>("schedule_id")),
        revision: u32::try_from(row.get::<i64, _>("revision"))
            .map_err(|error| StoreError::Corrupt(format!("revision: {error}")))?,
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        name: row.get("name"),
        instruction: row.get("instruction"),
        subject_page_path: row.get("subject_page_path"),
        channel_id: ChannelId::from(row.get::<String, _>("channel_id")),
        root_message_id: row
            .get::<Option<String>, _>("root_message_id")
            .map(MessageId::from),
        kind: row
            .get::<String, _>("kind")
            .parse()
            .map_err(StoreError::Corrupt)?,
        cron_expression: row.get("cron_expression"),
        interval_ms: row.get("interval_ms"),
        anchor_at: row.get("anchor_at"),
        timezone: row.get("timezone"),
        scheduled_at: row.get("scheduled_at"),
        created_at: row.get("created_at"),
        creating_run_id: row
            .get::<Option<String>, _>("creating_run_id")
            .map(RunId::from),
    })
}

async fn insert_revision(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    schedule: &Schedule,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO schedule_revisions (schedule_id, revision, agent_id, name, instruction, \
         channel_id, root_message_id, kind, cron_expression, interval_ms, anchor_at, timezone, \
         scheduled_at, created_at, creating_run_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(schedule.id.as_str())
    .bind(i64::from(schedule.revision))
    .bind(schedule.agent_id.as_str())
    .bind(&schedule.name)
    .bind(&schedule.instruction)
    .bind(schedule.channel_id.as_str())
    .bind(schedule.root_message_id.as_ref().map(MessageId::as_str))
    .bind(schedule.kind.as_str())
    .bind(&schedule.cron_expression)
    .bind(schedule.interval_ms)
    .bind(schedule.anchor_at)
    .bind(&schedule.timezone)
    .bind(schedule.scheduled_at)
    .bind(schedule.updated_at)
    .bind(schedule.creating_run_id.as_ref().map(RunId::as_str))
    .execute(&mut **transaction)
    .await
    .map_err(db_err)?;
    Ok(())
}

pub(crate) fn row_to_occurrence(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ScheduleOccurrence, StoreError> {
    Ok(ScheduleOccurrence {
        id: ScheduleOccurrenceId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        schedule_id: ScheduleId::from(row.get::<String, _>("schedule_id")),
        schedule_revision: u32::try_from(row.get::<i64, _>("schedule_revision"))
            .map_err(|error| StoreError::Corrupt(format!("schedule_revision: {error}")))?,
        scheduled_at: row.get("scheduled_at"),
        processed_at: row.get("processed_at"),
        outcome: row.get("outcome"),
        wakeup_id: row
            .get::<Option<String>, _>("wakeup_id")
            .map(WakeupId::from),
    })
}

pub(crate) fn row_to_wakeup(row: &sqlx::sqlite::SqliteRow) -> Result<Wakeup, StoreError> {
    let source_kind: String = row.get("source_kind");
    let rule = match source_kind.as_str() {
        "schedule" => WakeupRule::Schedule {
            schedule_id: ScheduleId::from(
                row.get::<Option<String>, _>("schedule_id")
                    .ok_or_else(|| StoreError::Corrupt("wake-up has no schedule".to_string()))?,
            ),
        },
        "event_subscription" => WakeupRule::EventSubscription {
            subscription_id: EventSubscriptionId::from(
                row.get::<Option<String>, _>("subscription_id")
                    .ok_or_else(|| {
                        StoreError::Corrupt("wake-up has no subscription".to_string())
                    })?,
            ),
        },
        "arrival" => WakeupRule::Arrival {
            subject_paths: serde_json::from_str(&row.get::<String, _>("subject_paths"))
                .map_err(|error| StoreError::Corrupt(format!("subject paths: {error}")))?,
            historical: row.get("historical"),
        },
        other => return Err(StoreError::Corrupt(format!("wake-up source kind: {other}"))),
    };
    Ok(Wakeup {
        id: WakeupId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        rule,
        rule_revision: u32::try_from(row.get::<i64, _>("rule_revision"))
            .map_err(|error| StoreError::Corrupt(format!("rule_revision: {error}")))?,
        rule_name: row.get("rule_name"),
        agent_id: AgentId::from(row.get::<String, _>("agent_id")),
        channel_id: row
            .get::<Option<String>, _>("channel_id")
            .map(ChannelId::from),
        root_message_id: row
            .get::<Option<String>, _>("root_message_id")
            .map(MessageId::from),
        instruction: row.get("instruction"),
        scheduled_at: row.get("scheduled_at"),
        state: row
            .get::<String, _>("state")
            .parse()
            .map_err(StoreError::Corrupt)?,
        run_id: row
            .get::<Option<String>, _>("run_id")
            .map(pagis_core::RunId::from),
        source_count: u32::try_from(row.get::<i64, _>("source_count"))
            .map_err(|error| StoreError::Corrupt(format!("source_count: {error}")))?,
        created_at: row.get("created_at"),
        started_at: row.get("started_at"),
    })
}

#[async_trait]
impl ScheduleStore for SqliteScheduleStore {
    async fn create(&self, schedule: &Schedule) -> Result<(), StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query(
            "INSERT INTO schedules (id, workspace_id, agent_id, name, instruction, channel_id, \
             root_message_id, kind, cron_expression, interval_ms, anchor_at, timezone, scheduled_at, \
             next_due_at, state, revision, approved_revision, creator, creating_run_id, created_at, \
             updated_at, archived_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
             ?, ?, ?, ?, ?)",
        )
        .bind(schedule.id.as_str())
        .bind(schedule.workspace_id.as_str())
        .bind(schedule.agent_id.as_str())
        .bind(&schedule.name)
        .bind(&schedule.instruction)
        .bind(schedule.channel_id.as_str())
        .bind(schedule.root_message_id.as_ref().map(|id| id.as_str()))
        .bind(schedule.kind.as_str())
        .bind(&schedule.cron_expression)
        .bind(schedule.interval_ms)
        .bind(schedule.anchor_at)
        .bind(&schedule.timezone)
        .bind(schedule.scheduled_at)
        .bind(schedule.next_due_at)
        .bind(schedule.state.as_str())
        .bind(i64::from(schedule.revision))
        .bind(schedule.approved_revision.map(i64::from))
        .bind(schedule.creator.as_str())
        .bind(schedule.creating_run_id.as_ref().map(RunId::as_str))
        .bind(schedule.created_at)
        .bind(schedule.updated_at)
        .bind(schedule.archived_at)
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        if let Some(path) = schedule.subject_page_path.as_deref() {
            sqlx::query("INSERT INTO schedule_subject_pages (schedule_id, path) VALUES (?, ?)")
                .bind(schedule.id.as_str())
                .bind(path)
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
        }
        insert_revision(&mut transaction, schedule).await?;
        transaction.commit().await.map_err(db_err)?;
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
    ) -> Result<Option<Schedule>, StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {SCHEDULE_COLUMNS} FROM schedules WHERE id = ? AND workspace_id = ?"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_schedule).transpose()
    }

    async fn edit(
        &self,
        previous: &Schedule,
        replacement: &Schedule,
    ) -> Result<Option<Schedule>, StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let changed = sqlx::query(
            "UPDATE schedules SET agent_id = ?, name = ?, instruction = ?, channel_id = ?, \
             root_message_id = ?, kind = ?, cron_expression = ?, interval_ms = ?, anchor_at = ?, \
             timezone = ?, scheduled_at = ?, next_due_at = ?, state = ?, revision = ?, \
             approved_revision = ?, updated_at = ? \
             WHERE id = ? AND workspace_id = ? AND revision = ?",
        )
        .bind(replacement.agent_id.as_str())
        .bind(&replacement.name)
        .bind(&replacement.instruction)
        .bind(replacement.channel_id.as_str())
        .bind(replacement.root_message_id.as_ref().map(MessageId::as_str))
        .bind(replacement.kind.as_str())
        .bind(&replacement.cron_expression)
        .bind(replacement.interval_ms)
        .bind(replacement.anchor_at)
        .bind(&replacement.timezone)
        .bind(replacement.scheduled_at)
        .bind(replacement.next_due_at)
        .bind(replacement.state.as_str())
        .bind(i64::from(replacement.revision))
        .bind(replacement.approved_revision.map(i64::from))
        .bind(replacement.updated_at)
        .bind(previous.id.as_str())
        .bind(previous.workspace_id.as_str())
        .bind(i64::from(previous.revision))
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?
        .rows_affected();
        if changed == 0 {
            return Err(StoreError::Conflict(
                "Schedule revision changed".to_string(),
            ));
        }
        sqlx::query(
            "UPDATE wakeups SET state = 'withdrawn' WHERE schedule_id = ? \
             AND rule_revision = ? AND state = 'pending'",
        )
        .bind(previous.id.as_str())
        .bind(i64::from(previous.revision))
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        insert_revision(&mut transaction, replacement).await?;
        transaction.commit().await.map_err(db_err)?;
        self.get(&previous.workspace_id, &previous.id).await
    }

    async fn list_revisions(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
    ) -> Result<Vec<ScheduleRevision>, StoreError> {
        // `schedule_revisions` carries no workspace of its own, so the
        // read asks the Schedule that owns the history.
        let rows = sqlx::query(&format!(
            "SELECT {REVISION_COLUMNS} FROM schedule_revisions \
             WHERE schedule_revisions.schedule_id = ? \
             AND EXISTS (SELECT 1 FROM schedules s \
                 WHERE s.id = schedule_revisions.schedule_id AND s.workspace_id = ?) \
             ORDER BY revision"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_revision).collect()
    }

    async fn list(
        &self,
        workspace_id: &WorkspaceId,
        before: Option<&ScheduleId>,
        limit: u32,
    ) -> Result<Vec<Schedule>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {SCHEDULE_COLUMNS} FROM schedules WHERE workspace_id = ? \
             AND (? IS NULL OR id < ?) ORDER BY id DESC LIMIT ?"
        ))
        .bind(workspace_id.as_str())
        .bind(before.map(ScheduleId::as_str))
        .bind(before.map(ScheduleId::as_str))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_schedule).collect()
    }

    async fn list_open_for_subject_page(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        path: &str,
    ) -> Result<Vec<OpenSchedule>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {SCHEDULE_COLUMNS} FROM schedules JOIN schedule_subject_pages metadata \
             ON metadata.schedule_id = schedules.id WHERE schedules.workspace_id = ? \
             AND schedules.agent_id = ? AND metadata.path = ? \
             AND schedules.state IN ('active', 'paused', 'blocked') ORDER BY schedules.id"
        ))
        .bind(workspace_id.as_str())
        .bind(agent_id.as_str())
        .bind(path)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter()
            .map(row_to_schedule)
            .map(|schedule| {
                schedule.map(|schedule| OpenSchedule {
                    id: schedule.id.to_string(),
                    next_due_at: schedule.next_due_at.unwrap_or(schedule.scheduled_at),
                    purpose: format!("{}: {}", schedule.name, schedule.instruction),
                })
            })
            .collect()
    }

    async fn archive(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let changed = sqlx::query(
            "UPDATE schedules SET state = 'archived', next_due_at = NULL, archived_at = ?, \
             updated_at = ? WHERE id = ? AND workspace_id = ? AND state != 'archived'",
        )
        .bind(at)
        .bind(at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?
        .rows_affected();
        if changed > 0 {
            sqlx::query(
                "UPDATE wakeups SET state = 'withdrawn' \
                 WHERE schedule_id = ? AND workspace_id = ? AND state = 'pending'",
            )
            .bind(id.as_str())
            .bind(workspace_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        }
        transaction.commit().await.map_err(db_err)?;
        self.get(workspace_id, id).await
    }

    async fn pause(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let changed = sqlx::query(
            "UPDATE schedules SET state = 'paused', next_due_at = NULL, updated_at = ? \
             WHERE id = ? AND workspace_id = ? AND state = 'active'",
        )
        .bind(at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?
        .rows_affected();
        if changed > 0 {
            sqlx::query(
                "UPDATE wakeups SET state = 'withdrawn' \
                 WHERE schedule_id = ? AND workspace_id = ? AND state = 'pending'",
            )
            .bind(id.as_str())
            .bind(workspace_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        }
        transaction.commit().await.map_err(db_err)?;
        self.get(workspace_id, id).await
    }

    async fn resume(
        &self,
        workspace_id: &WorkspaceId,
        id: &ScheduleId,
        next_due_at: UnixMillis,
        at: UnixMillis,
    ) -> Result<Option<Schedule>, StoreError> {
        sqlx::query(
            "UPDATE schedules SET state = 'active', next_due_at = ?, updated_at = ? \
             WHERE id = ? AND workspace_id = ? AND state = 'paused'",
        )
        .bind(next_due_at)
        .bind(at)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        self.get(workspace_id, id).await
    }

    async fn skip_next(
        &self,
        schedule: &Schedule,
        expected_due_at: UnixMillis,
        next_due_at: Option<UnixMillis>,
        at: UnixMillis,
    ) -> Result<ScheduleOccurrence, StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let next_state = if schedule.kind == pagis_core::ScheduleKind::OneShot {
            "completed"
        } else {
            "active"
        };
        let changed = sqlx::query(
            "UPDATE schedules SET state = ?, next_due_at = ?, updated_at = ? \
             WHERE id = ? AND revision = ? AND state = 'active' AND next_due_at = ?",
        )
        .bind(next_state)
        .bind(next_due_at)
        .bind(at)
        .bind(schedule.id.as_str())
        .bind(i64::from(schedule.revision))
        .bind(expected_due_at)
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?
        .rows_affected();
        if changed == 0 {
            return Err(StoreError::Conflict(
                "the Schedule became due before skip-next completed".to_string(),
            ));
        }
        let occurrence = ScheduleOccurrence {
            id: ScheduleOccurrenceId::generate(),
            workspace_id: schedule.workspace_id.clone(),
            schedule_id: schedule.id.clone(),
            schedule_revision: schedule.revision,
            scheduled_at: expected_due_at,
            processed_at: at,
            outcome: "skipped".to_string(),
            wakeup_id: None,
        };
        sqlx::query(
            "INSERT INTO schedule_occurrences (id, workspace_id, schedule_id, schedule_revision, \
             scheduled_at, processed_at, outcome) VALUES (?, ?, ?, ?, ?, ?, 'skipped')",
        )
        .bind(occurrence.id.as_str())
        .bind(occurrence.workspace_id.as_str())
        .bind(occurrence.schedule_id.as_str())
        .bind(i64::from(occurrence.schedule_revision))
        .bind(occurrence.scheduled_at)
        .bind(occurrence.processed_at)
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;
        Ok(occurrence)
    }

    async fn list_occurrences(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        before: Option<&ScheduleOccurrenceId>,
        limit: u32,
    ) -> Result<Vec<ScheduleOccurrence>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {OCCURRENCE_COLUMNS} FROM schedule_occurrences \
             WHERE schedule_id = ? AND workspace_id = ? \
             AND (? IS NULL OR id < ?) ORDER BY id DESC LIMIT ?"
        ))
        .bind(schedule_id.as_str())
        .bind(workspace_id.as_str())
        .bind(before.map(ScheduleOccurrenceId::as_str))
        .bind(before.map(ScheduleOccurrenceId::as_str))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_occurrence).collect()
    }

    async fn list_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        before: Option<&WakeupId>,
        limit: u32,
    ) -> Result<Vec<Wakeup>, StoreError> {
        let rows = sqlx::query(&format!(
            "SELECT {WAKEUP_COLUMNS} FROM wakeups WHERE schedule_id = ? AND workspace_id = ? \
             AND (? IS NULL OR id < ?) ORDER BY id DESC LIMIT ?"
        ))
        .bind(schedule_id.as_str())
        .bind(workspace_id.as_str())
        .bind(before.map(WakeupId::as_str))
        .bind(before.map(WakeupId::as_str))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(row_to_wakeup).collect()
    }
}
