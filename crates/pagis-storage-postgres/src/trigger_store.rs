//! The durable half of the Trigger module (ADR-0006).
//!
//! Two write paths land here, and both are one transaction each:
//! `process_due` turns Schedules into Wake-ups, and `ingest` commits a
//! collection pass — its cursor, its deduplicated Incoming Events, its
//! matches, and its Wake-ups — together. A batch that fails part way
//! leaves the cursor where it was, so the next pass reads the same
//! window again and deduplication absorbs the overlap.

use std::str::FromStr;

use async_trait::async_trait;
use pagis_core::{
    ArrivalRun, CodingSessionId, ConnectionId, DueBatch, EventMatcher, EventSource,
    EventSubscription, EventSubscriptionId, EventWakeupContext, IncomingEvent, IncomingEventId,
    IngestBatch, IngestOutcome, MessageId, Run, RunId, RunState, ScheduleId, ScheduleOccurrence,
    ScheduleOccurrenceId, SourceBatch, SourceBatchId, SourceBatchOutcome, TriggerStore, Wakeup,
    WakeupClaim, WakeupId, WakeupLanding, WakeupRule, WorkspaceId,
};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::db_err;
use crate::schedule_store::{SCHEDULE_COLUMNS, WAKEUP_COLUMNS, row_to_schedule, row_to_wakeup};

/// Columns of one stored Incoming Event.
pub(crate) const EVENT_COLUMNS: &str = "id, workspace_id, connection_id, coding_session_id, \
    event_kind, provider_event_id, metadata, occurred_at, received_at, batch_id";
pub(crate) const BATCH_COLUMNS: &str = "id, workspace_id, connection_id, coding_session_id, \
    event_kind, collected_at, collected_count, stored_count, wakeup_count, outcome, detail";

#[derive(Clone)]
pub struct PostgresTriggerStore {
    pool: PgPool,
}

impl PostgresTriggerStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// The source that the `connection_id` and `coding_session_id`
/// columns of one row hold.
pub(crate) fn row_to_source(
    row: &sqlx::postgres::PgRow,
) -> Result<EventSource, pagis_core::StoreError> {
    EventSource::from_columns(row.get("connection_id"), row.get("coding_session_id"))
        .map_err(pagis_core::StoreError::Corrupt)
}

/// The column that names one source, and the id it holds.
pub(crate) fn source_column(source: &EventSource) -> (&'static str, &str) {
    match source {
        EventSource::Connection { connection_id } => ("connection_id", connection_id.as_str()),
        EventSource::CodingSession { coding_session_id } => {
            ("coding_session_id", coding_session_id.as_str())
        }
    }
}

pub(crate) fn row_to_event(
    row: &sqlx::postgres::PgRow,
) -> Result<IncomingEvent, pagis_core::StoreError> {
    Ok(IncomingEvent {
        id: IncomingEventId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        source: row_to_source(row)?,
        event_kind: row.get("event_kind"),
        provider_event_id: row.get("provider_event_id"),
        metadata: serde_json::from_str(&row.get::<String, _>("metadata"))
            .map_err(|error| pagis_core::StoreError::Corrupt(format!("metadata: {error}")))?,
        occurred_at: row.get("occurred_at"),
        received_at: row.get("received_at"),
        batch_id: SourceBatchId::from(row.get::<String, _>("batch_id")),
    })
}

pub(crate) fn row_to_batch(
    row: &sqlx::postgres::PgRow,
) -> Result<SourceBatch, pagis_core::StoreError> {
    Ok(SourceBatch {
        id: SourceBatchId::from(row.get::<String, _>("id")),
        workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
        source: row_to_source(row)?,
        event_kind: row.get("event_kind"),
        collected_at: row.get("collected_at"),
        collected_count: count(row, "collected_count")?,
        stored_count: count(row, "stored_count")?,
        wakeup_count: count(row, "wakeup_count")?,
        outcome: row
            .get::<String, _>("outcome")
            .parse()
            .map_err(pagis_core::StoreError::Corrupt)?,
        detail: row.get("detail"),
    })
}

fn count(row: &sqlx::postgres::PgRow, name: &str) -> Result<u32, pagis_core::StoreError> {
    u32::try_from(row.get::<i64, _>(name))
        .map_err(|error| pagis_core::StoreError::Corrupt(format!("{name}: {error}")))
}

/// Insert one Wake-up row. The caller has already decided the rule.
async fn insert_wakeup(
    transaction: &mut Transaction<'_, Postgres>,
    wakeup: &Wakeup,
) -> Result<(), pagis_core::StoreError> {
    sqlx::query(
        "INSERT INTO wakeups (id, workspace_id, source_kind, schedule_id, subscription_id, subject_paths, \
         rule_revision, rule_name, agent_id, channel_id, root_message_id, instruction, \
         scheduled_at, state, source_count, created_at, historical) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'pending', $14, $15, $16)",
    )
    .bind(wakeup.id.as_str())
    .bind(wakeup.workspace_id.as_str())
    .bind(wakeup.rule.kind_str())
    .bind(wakeup.rule.schedule_id().map(|id| id.as_str()))
    .bind(wakeup.rule.subscription_id().map(|id| id.as_str()))
    .bind(match &wakeup.rule {
        WakeupRule::Arrival { subject_paths, .. } => Some(
            serde_json::to_string(subject_paths)
                .map_err(|error| pagis_core::StoreError::Corrupt(error.to_string()))?,
        ),
        WakeupRule::Schedule { .. } | WakeupRule::EventSubscription { .. } => None,
    })
    .bind(i64::from(wakeup.rule_revision))
    .bind(&wakeup.rule_name)
    .bind(wakeup.agent_id.as_str())
    .bind(wakeup.channel_id.as_ref().map(|id| id.as_str()))
    .bind(wakeup.root_message_id.as_ref().map(|id| id.as_str()))
    .bind(&wakeup.instruction)
    .bind(wakeup.scheduled_at)
    .bind(i64::from(wakeup.source_count))
    .bind(wakeup.created_at)
    .bind(wakeup.rule.historical())
    .execute(&mut **transaction)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn link_source(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: &WorkspaceId,
    wakeup_id: &WakeupId,
    source_kind: &str,
    source_id: &str,
) -> Result<(), pagis_core::StoreError> {
    sqlx::query(
        "INSERT INTO wakeup_sources (workspace_id, wakeup_id, source_kind, source_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(workspace_id.as_str())
    .bind(wakeup_id.as_str())
    .bind(source_kind)
    .bind(source_id)
    .execute(&mut **transaction)
    .await
    .map_err(db_err)?;
    Ok(())
}

#[async_trait]
impl TriggerStore for PostgresTriggerStore {
    async fn next_due_at(&self) -> Result<Option<i64>, pagis_core::StoreError> {
        let row = sqlx::query(
            "SELECT MIN(next_due_at) AS next_due_at FROM schedules WHERE state = 'active'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.get("next_due_at"))
    }

    async fn pending_agents(
        &self,
    ) -> Result<Vec<(WorkspaceId, pagis_core::AgentId)>, pagis_core::StoreError> {
        let rows = sqlx::query(
            "SELECT DISTINCT workspace_id, agent_id FROM wakeups WHERE state = 'pending' \
             ORDER BY workspace_id, agent_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    WorkspaceId::from(row.get::<String, _>("workspace_id")),
                    pagis_core::AgentId::from(row.get::<String, _>("agent_id")),
                )
            })
            .collect())
    }

    async fn process_due(&self, now: i64) -> Result<DueBatch, pagis_core::StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let blocked_rows = sqlx::query(&format!(
            "SELECT {SCHEDULE_COLUMNS} FROM schedules WHERE state = 'active' \
             AND EXISTS (SELECT 1 FROM agents WHERE agents.id = schedules.agent_id \
                 AND agents.status != 'active')"
        ))
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_err)?;
        let mut blocked_schedules = blocked_rows
            .iter()
            .map(row_to_schedule)
            .collect::<Result<Vec<_>, _>>()?;
        for schedule in &mut blocked_schedules {
            schedule.state = pagis_core::ScheduleState::Blocked;
            schedule.next_due_at = None;
            schedule.updated_at = now;
        }
        let withdrawn_rows = sqlx::query(&format!(
            "SELECT {WAKEUP_COLUMNS} FROM wakeups WHERE state = 'pending' \
             AND schedule_id IN (SELECT id FROM schedules WHERE state = 'active' \
                 AND EXISTS (SELECT 1 FROM agents WHERE agents.id = schedules.agent_id \
                     AND agents.status != 'active'))"
        ))
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_err)?;
        let mut withdrawn_wakeups = withdrawn_rows
            .iter()
            .map(row_to_wakeup)
            .collect::<Result<Vec<_>, _>>()?;
        for wakeup in &mut withdrawn_wakeups {
            wakeup.state = pagis_core::WakeupState::Withdrawn;
        }
        sqlx::query(
            "UPDATE schedules SET state = 'blocked', next_due_at = NULL, updated_at = $1 \
             WHERE state = 'active' AND EXISTS (SELECT 1 FROM agents \
                 WHERE agents.id = schedules.agent_id AND agents.status != 'active')",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "UPDATE wakeups SET state = 'withdrawn' WHERE state = 'pending' \
             AND schedule_id IN (SELECT id FROM schedules WHERE state = 'blocked')",
        )
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        let rows = sqlx::query(&format!(
            "SELECT {SCHEDULE_COLUMNS} FROM schedules \
             WHERE state = 'active' AND next_due_at <= $1 ORDER BY next_due_at, id"
        ))
        .bind(now)
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_err)?;
        let schedules = rows
            .iter()
            .map(row_to_schedule)
            .collect::<Result<Vec<_>, _>>()?;
        let mut occurrences = Vec::new();
        let mut wakeups = Vec::new();
        for schedule in schedules {
            let previous_due_at = schedule.next_due_at.expect("due Schedule has a next time");
            let (scheduled_at, next_due_at) =
                match (schedule.kind, schedule.anchor_at, schedule.interval_ms) {
                    (pagis_core::ScheduleKind::Interval, Some(anchor), Some(interval_ms)) => {
                        let scheduled_at =
                            anchor + (now.saturating_sub(anchor) / interval_ms) * interval_ms;
                        (scheduled_at, Some(scheduled_at + interval_ms))
                    }
                    (pagis_core::ScheduleKind::Cron, _, _) => cron_window(&schedule, now)?,
                    _ => (
                        schedule.next_due_at.expect("due Schedule has a next time"),
                        None,
                    ),
                };
            let occurrence = ScheduleOccurrence {
                id: ScheduleOccurrenceId::generate(),
                workspace_id: schedule.workspace_id.clone(),
                schedule_id: schedule.id.clone(),
                schedule_revision: schedule.revision,
                scheduled_at,
                processed_at: now,
                outcome: "wakeup_created".to_string(),
                wakeup_id: None,
            };
            let inserted = sqlx::query(
                "INSERT INTO schedule_occurrences (id, workspace_id, schedule_id, \
                 schedule_revision, scheduled_at, processed_at, outcome) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING",
            )
            .bind(occurrence.id.as_str())
            .bind(occurrence.workspace_id.as_str())
            .bind(occurrence.schedule_id.as_str())
            .bind(i64::from(occurrence.schedule_revision))
            .bind(occurrence.scheduled_at)
            .bind(occurrence.processed_at)
            .bind(&occurrence.outcome)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?
            .rows_affected();
            if inserted == 0 {
                continue;
            }
            let missed_count = missed_count(&schedule, previous_due_at, scheduled_at)?;
            let pending_row = sqlx::query(&format!(
                "SELECT {WAKEUP_COLUMNS} FROM wakeups WHERE schedule_id = $1 AND state = 'pending' \
                 ORDER BY id LIMIT 1"
            ))
            .bind(schedule.id.as_str())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(db_err)?;
            if let Some(pending_row) = pending_row {
                let mut pending = row_to_wakeup(&pending_row)?;
                let added = missed_count.saturating_add(1);
                sqlx::query(
                    "UPDATE schedule_occurrences SET outcome = 'combined', wakeup_id = $1 WHERE id = $2",
                )
                .bind(pending.id.as_str())
                .bind(occurrence.id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
                link_source(
                    &mut transaction,
                    &pending.workspace_id,
                    &pending.id,
                    "schedule_occurrence",
                    occurrence.id.as_str(),
                )
                .await?;
                sqlx::query(
                    "UPDATE wakeups SET source_count = wakeups.source_count + $1 WHERE id = $2",
                )
                .bind(i64::from(added))
                .bind(pending.id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
                sqlx::query("UPDATE schedules SET next_due_at = $1, updated_at = $2 WHERE id = $3")
                    .bind(next_due_at)
                    .bind(now)
                    .bind(schedule.id.as_str())
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_err)?;
                pending.source_count = pending.source_count.saturating_add(added);
                occurrences.push(ScheduleOccurrence {
                    outcome: "combined".to_string(),
                    wakeup_id: Some(pending.id),
                    ..occurrence
                });
                continue;
            }
            let wakeup = Wakeup {
                id: WakeupId::generate(),
                workspace_id: schedule.workspace_id.clone(),
                rule: WakeupRule::Schedule {
                    schedule_id: schedule.id.clone(),
                },
                rule_revision: schedule.revision,
                rule_name: schedule.name.clone(),
                agent_id: schedule.agent_id.clone(),
                channel_id: Some(schedule.channel_id.clone()),
                root_message_id: schedule.root_message_id.clone(),
                instruction: schedule.instruction.clone(),
                scheduled_at,
                state: pagis_core::WakeupState::Pending,
                run_id: None,
                source_count: missed_count.saturating_add(1),
                created_at: now,
                started_at: None,
            };
            insert_wakeup(&mut transaction, &wakeup).await?;
            sqlx::query("UPDATE schedule_occurrences SET wakeup_id = $1 WHERE id = $2")
                .bind(wakeup.id.as_str())
                .bind(occurrence.id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            link_source(
                &mut transaction,
                &wakeup.workspace_id,
                &wakeup.id,
                "schedule_occurrence",
                occurrence.id.as_str(),
            )
            .await?;
            sqlx::query("UPDATE schedules SET next_due_at = $1, updated_at = $2 WHERE id = $3")
                .bind(next_due_at)
                .bind(now)
                .bind(schedule.id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            occurrences.push(ScheduleOccurrence {
                wakeup_id: Some(wakeup.id.clone()),
                ..occurrence
            });
            wakeups.push(wakeup);
        }
        transaction.commit().await.map_err(db_err)?;
        Ok(DueBatch {
            blocked_schedules,
            withdrawn_wakeups,
            occurrences,
            wakeups,
        })
    }

    /// The user asked for one Schedule's work now (ADR-0022). The
    /// Wake-up is recorded as its own occurrence, stamped `now` so it
    /// never collides with a cadence occurrence, and the Schedule's own
    /// `next_due_at` is left alone.
    async fn wake_now(
        &self,
        workspace_id: &WorkspaceId,
        schedule_id: &ScheduleId,
        now: i64,
    ) -> Result<Option<Wakeup>, pagis_core::StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let row = sqlx::query(&format!(
            "SELECT {SCHEDULE_COLUMNS} FROM schedules \
             WHERE id = $1 AND workspace_id = $2 AND state = 'active'"
        ))
        .bind(schedule_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let schedule = row_to_schedule(&row)?;
        // One Run of a Schedule at a time: a Wake-up that already
        // waits is the answer the user is waiting for.
        let pending_row = sqlx::query(&format!(
            "SELECT {WAKEUP_COLUMNS} FROM wakeups WHERE schedule_id = $1 AND state = \'pending\' \
             ORDER BY id LIMIT 1"
        ))
        .bind(schedule.id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_err)?;
        if let Some(pending_row) = pending_row {
            transaction.commit().await.map_err(db_err)?;
            return Ok(Some(row_to_wakeup(&pending_row)?));
        }
        let occurrence = ScheduleOccurrence {
            id: ScheduleOccurrenceId::generate(),
            workspace_id: schedule.workspace_id.clone(),
            schedule_id: schedule.id.clone(),
            schedule_revision: schedule.revision,
            scheduled_at: now,
            processed_at: now,
            outcome: "wakeup_created".to_string(),
            wakeup_id: None,
        };
        sqlx::query(
            "INSERT INTO schedule_occurrences (id, workspace_id, schedule_id, \
             schedule_revision, scheduled_at, processed_at, outcome) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(occurrence.id.as_str())
        .bind(occurrence.workspace_id.as_str())
        .bind(occurrence.schedule_id.as_str())
        .bind(i64::from(occurrence.schedule_revision))
        .bind(occurrence.scheduled_at)
        .bind(occurrence.processed_at)
        .bind(&occurrence.outcome)
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        let wakeup = Wakeup {
            id: WakeupId::generate(),
            workspace_id: schedule.workspace_id.clone(),
            rule: WakeupRule::Schedule {
                schedule_id: schedule.id.clone(),
            },
            rule_revision: schedule.revision,
            rule_name: schedule.name.clone(),
            agent_id: schedule.agent_id.clone(),
            channel_id: Some(schedule.channel_id.clone()),
            root_message_id: schedule.root_message_id.clone(),
            instruction: schedule.instruction.clone(),
            scheduled_at: now,
            state: pagis_core::WakeupState::Pending,
            run_id: None,
            source_count: 1,
            created_at: now,
            started_at: None,
        };
        insert_wakeup(&mut transaction, &wakeup).await?;
        sqlx::query("UPDATE schedule_occurrences SET wakeup_id = $1 WHERE id = $2")
            .bind(wakeup.id.as_str())
            .bind(occurrence.id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        link_source(
            &mut transaction,
            &wakeup.workspace_id,
            &wakeup.id,
            "schedule_occurrence",
            occurrence.id.as_str(),
        )
        .await?;
        transaction.commit().await.map_err(db_err)?;
        Ok(Some(wakeup))
    }

    async fn claim_wakeups(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &pagis_core::AgentId,
        slots: pagis_core::RunSlots,
        now: i64,
    ) -> Result<Vec<WakeupClaim>, pagis_core::StoreError> {
        if slots == pagis_core::RunSlots::default() {
            return Ok(Vec::new());
        }
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let mut pending = Vec::new();
        for (arrival, limit) in [(false, slots.conversation), (true, slots.arrival)] {
            if limit == 0 {
                continue;
            }
            let rows = sqlx::query(&format!(
                "SELECT {WAKEUP_COLUMNS} FROM wakeups \
                 WHERE agent_id = $1 AND workspace_id = $2 AND state = 'pending' \
                 AND (source_kind = 'arrival') = $3 \
                 AND NOT EXISTS (SELECT 1 FROM wakeups active_wakeup \
                     JOIN runs active_run ON active_run.id = active_wakeup.run_id \
                     WHERE (active_wakeup.schedule_id = wakeups.schedule_id \
                         OR active_wakeup.subscription_id = wakeups.subscription_id) \
                       AND active_wakeup.state = 'started' \
                       AND active_run.state IN ('queued', 'running', 'reflecting', 'waiting_for_user', 'waiting_for_approval')) \
                 ORDER BY scheduled_at, id LIMIT $4"
            ))
            .bind(agent_id.as_str())
            .bind(workspace_id.as_str())
            .bind(arrival)
            .bind(i64::from(limit))
            .fetch_all(&mut *transaction)
            .await
            .map_err(db_err)?;
            pending.extend(
                rows.iter()
                    .map(row_to_wakeup)
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        let mut claims = Vec::new();
        for mut wakeup in pending {
            let trigger_kind = wakeup.rule.trigger_kind();
            let run = Run {
                title: pagis_core::run_title(if trigger_kind == pagis_core::TriggerKind::Arrival {
                    pagis_core::RunTitleSource::Arrival
                } else {
                    pagis_core::RunTitleSource::Rule(&wakeup.rule_name)
                }),
                id: RunId::generate(),
                workspace_id: wakeup.workspace_id.clone(),
                agent_id: wakeup.agent_id.clone(),
                channel_id: wakeup.channel_id.clone(),
                root_message_id: wakeup.root_message_id.clone(),
                trigger_kind,
                trigger_ref: Some(wakeup.id.to_string()),
                hop_count: 0,
                origin: wakeup
                    .channel_id
                    .as_ref()
                    .map(|channel_id| pagis_core::RunOrigin {
                        agent_id: wakeup.agent_id.clone(),
                        channel_id: channel_id.clone(),
                        root_message_id: wakeup.root_message_id.clone(),
                    }),
                state: RunState::Queued,
                failure_kind: None,
                dismissed_at: None,
                error: None,
                started_at: None,
                ended_at: None,
                created_at: now,
            };
            sqlx::query(
                "INSERT INTO runs (id, workspace_id, agent_id, channel_id, root_message_id, \
                 trigger_kind, trigger_ref, hop_count, origin_agent_id, origin_channel_id, \
                 origin_root_message_id, state, created_at, title) VALUES ($1, $2, $3, $4, $5, $6, $7, 0, $8, $9, $10, 'queued', $11, $12)",
            )
            .bind(run.id.as_str())
            .bind(run.workspace_id.as_str())
            .bind(run.agent_id.as_str())
            .bind(run.channel_id.as_ref().map(|id| id.as_str()))
            .bind(run.root_message_id.as_ref().map(|id| id.as_str()))
            .bind(trigger_kind.as_str())
            .bind(run.trigger_ref.as_deref())
            .bind(run.origin.as_ref().map(|origin| origin.agent_id.as_str()))
            .bind(run.origin.as_ref().map(|origin| origin.channel_id.as_str()))
            .bind(run.origin.as_ref().and_then(|origin| origin.root_message_id.as_ref()).map(|id| id.as_str()))
            .bind(now)
            .bind(&run.title)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            sqlx::query("UPDATE wakeups SET state = 'started', run_id = $1, started_at = $2 WHERE id = $3 AND state = 'pending'")
                .bind(run.id.as_str())
                .bind(now)
                .bind(wakeup.id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            // A one-shot Schedule is finished once its Wake-up starts,
            // even if the Run later fails. An Event Subscription keeps
            // waiting for the next matching mail.
            if let Some(schedule_id) = wakeup.rule.schedule_id() {
                sqlx::query(
                    "UPDATE schedules SET state = CASE WHEN kind = 'one_shot' THEN 'completed' \
                     ELSE state END, updated_at = $1 WHERE id = $2",
                )
                .bind(now)
                .bind(schedule_id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            }
            wakeup.state = pagis_core::WakeupState::Started;
            wakeup.run_id = Some(run.id.clone());
            wakeup.started_at = Some(now);
            claims.push(WakeupClaim { wakeup, run });
        }
        transaction.commit().await.map_err(db_err)?;
        Ok(claims)
    }

    async fn get_wakeup(
        &self,
        workspace_id: &WorkspaceId,
        id: &WakeupId,
    ) -> Result<Option<Wakeup>, pagis_core::StoreError> {
        let row = sqlx::query(&format!(
            "SELECT {WAKEUP_COLUMNS} FROM wakeups WHERE id = $1 AND workspace_id = $2"
        ))
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        row.as_ref().map(row_to_wakeup).transpose()
    }

    async fn cursor(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
    ) -> Result<Option<String>, pagis_core::StoreError> {
        // `provider_cursors` carries no workspace of its own, so the
        // read joins the Connection that owns the cursor.
        let row = sqlx::query(
            "SELECT p.cursor FROM provider_cursors p \
             JOIN connections c ON c.id = p.connection_id \
             WHERE p.connection_id = $1 AND p.event_kind = $2 AND c.workspace_id = $3",
        )
        .bind(connection_id.as_str())
        .bind(event_kind)
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(|row| row.get::<String, _>("cursor")))
    }

    async fn record_failure(
        &self,
        workspace_id: &WorkspaceId,
        connection_id: &ConnectionId,
        event_kind: &str,
        code: &str,
        at: i64,
    ) -> Result<SourceBatch, pagis_core::StoreError> {
        let batch = SourceBatch {
            id: SourceBatchId::generate(),
            workspace_id: workspace_id.clone(),
            source: EventSource::connection(connection_id.clone()),
            event_kind: event_kind.to_string(),
            collected_at: at,
            collected_count: 0,
            stored_count: 0,
            wakeup_count: 0,
            outcome: SourceBatchOutcome::Failed,
            detail: Some(code.to_string()),
        };
        sqlx::query(
            "INSERT INTO source_batches (id, workspace_id, connection_id, event_kind, \
             collected_at, collected_count, stored_count, wakeup_count, outcome, detail) \
             VALUES ($1, $2, $3, $4, $5, 0, 0, 0, 'failed', $6)",
        )
        .bind(batch.id.as_str())
        .bind(batch.workspace_id.as_str())
        .bind(connection_id.as_str())
        .bind(&batch.event_kind)
        .bind(batch.collected_at)
        .bind(batch.detail.as_deref())
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(batch)
    }

    async fn event_context(
        &self,
        workspace_id: &WorkspaceId,
        wakeup_id: &WakeupId,
    ) -> Result<Option<EventWakeupContext>, pagis_core::StoreError> {
        let header = sqlx::query(
            "SELECT s.id AS subscription_id, s.event_kind AS event_kind, \
             s.connection_id AS connection_id, s.coding_session_id AS coding_session_id, \
             c.alias AS alias \
             FROM wakeups w JOIN event_subscriptions s ON s.id = w.subscription_id \
             LEFT JOIN connections c ON c.id = s.connection_id \
             WHERE w.id = $1 AND w.workspace_id = $2",
        )
        .bind(wakeup_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        let Some(header) = header else {
            return Ok(None);
        };
        let rows = sqlx::query(&format!(
            "SELECT {} FROM incoming_events e JOIN wakeup_sources s \
             ON s.source_kind = 'incoming_event' AND s.source_id = e.id \
             WHERE s.wakeup_id = $1 AND e.workspace_id = $2 ORDER BY e.occurred_at, e.id",
            EVENT_COLUMNS
                .split(", ")
                .map(|column| format!("e.{column}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .bind(wakeup_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(Some(EventWakeupContext {
            subscription_id: EventSubscriptionId::from(header.get::<String, _>("subscription_id")),
            source: row_to_source(&header)?,
            connection_alias: header.get("alias"),
            event_kind: header.get("event_kind"),
            events: rows
                .iter()
                .map(row_to_event)
                .collect::<Result<Vec<_>, _>>()?,
        }))
    }

    async fn withdraw_pending(
        &self,
        workspace_id: &WorkspaceId,
        rule_id: &str,
        _at: i64,
    ) -> Result<Vec<Wakeup>, pagis_core::StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let rows = sqlx::query(&format!(
            "SELECT {WAKEUP_COLUMNS} FROM wakeups \
             WHERE state = 'pending' AND workspace_id = $1 \
             AND (schedule_id = $2 OR subscription_id = $3)"
        ))
        .bind(workspace_id.as_str())
        .bind(rule_id)
        .bind(rule_id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_err)?;
        let mut withdrawn = rows
            .iter()
            .map(row_to_wakeup)
            .collect::<Result<Vec<_>, _>>()?;
        for wakeup in &mut withdrawn {
            sqlx::query(
                "UPDATE wakeups SET state = 'withdrawn' WHERE id = $1 AND state = 'pending'",
            )
            .bind(wakeup.id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            wakeup.state = pagis_core::WakeupState::Withdrawn;
        }
        transaction.commit().await.map_err(db_err)?;
        Ok(withdrawn)
    }

    async fn ingest(
        &self,
        batch: IngestBatch,
        eligible: &[EventSubscription],
        matcher: &dyn EventMatcher,
        arrival: Option<&ArrivalRun>,
        source_resource: Option<&str>,
        keys: &dyn pagis_core::ForgetKeys,
    ) -> Result<IngestOutcome, pagis_core::StoreError> {
        let mut transaction = crate::pool::begin_write(&self.pool).await.map_err(db_err)?;
        let collected_count = u32::try_from(batch.events.len()).unwrap_or(u32::MAX);
        let batch_id = SourceBatchId::generate();
        let connection_id = batch.source.connection_id();
        let coding_session_id = batch.source.coding_session_id();
        // The batch row lands first: an Incoming Event points at the
        // pass that acquired it.
        sqlx::query(
            "INSERT INTO source_batches (id, workspace_id, connection_id, coding_session_id, \
             event_kind, collected_at, collected_count, stored_count, wakeup_count, outcome, \
             detail) VALUES ($1, $2, $3, $4, $5, $6, $7, 0, 0, $8, NULL)",
        )
        .bind(batch_id.as_str())
        .bind(batch.workspace_id.as_str())
        .bind(connection_id.map(ConnectionId::as_str))
        .bind(coding_session_id.map(CodingSessionId::as_str))
        .bind(&batch.event_kind)
        .bind(batch.received_at)
        .bind(i64::from(collected_count))
        .bind(if batch.baseline {
            SourceBatchOutcome::Baseline.as_str()
        } else {
            SourceBatchOutcome::Collected.as_str()
        })
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        let mut stored = Vec::new();
        // The Source Item of an event has the provider event id as its
        // id, under the synced resource that the declaration of the kind
        // names (ADR-0008).
        // A Coding Session syncs no resource, so a Forget never reaches
        // its events.
        let source = source_resource
            .zip(connection_id)
            .map(
                |(resource, connection_id)| pagis_core::knowledge::SourceKey {
                    workspace_id: batch.workspace_id.clone(),
                    connection_id: connection_id.clone(),
                    resource: resource.to_string(),
                },
            );

        // A baseline pass commits its cursor and emits nothing: mail
        // that arrived before the user subscribed is not news.
        if !batch.baseline {
            for event in &batch.events {
                // A Forget blocks an event of its item like a new
                // retrieval: the pass drops the event, so no row holds
                // its id or its metadata and no rule wakes for it.
                if let Some(source) = &source
                    && crate::forget_store::suppressed(
                        &mut transaction,
                        keys,
                        source,
                        &event.provider_event_id,
                    )
                    .await?
                {
                    continue;
                }
                let id = IncomingEventId::generate();
                let metadata = event.metadata.to_string();
                let inserted = sqlx::query(
                    "INSERT INTO incoming_events (id, workspace_id, connection_id, \
                     coding_session_id, event_kind, provider_event_id, metadata, occurred_at, \
                     received_at, batch_id) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT DO NOTHING",
                )
                .bind(id.as_str())
                .bind(batch.workspace_id.as_str())
                .bind(connection_id.map(ConnectionId::as_str))
                .bind(coding_session_id.map(CodingSessionId::as_str))
                .bind(&batch.event_kind)
                .bind(&event.provider_event_id)
                .bind(&metadata)
                .bind(event.occurred_at)
                .bind(batch.received_at)
                .bind(batch_id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?
                .rows_affected();
                if inserted == 0 {
                    continue;
                }
                stored.push((
                    IncomingEvent {
                        id,
                        workspace_id: batch.workspace_id.clone(),
                        source: batch.source.clone(),
                        event_kind: batch.event_kind.clone(),
                        provider_event_id: event.provider_event_id.clone(),
                        metadata: event.metadata.clone(),
                        occurred_at: event.occurred_at,
                        received_at: batch.received_at,
                        batch_id: batch_id.clone(),
                    },
                    event.landing.clone(),
                ));
            }
        }

        let mut created = Vec::new();
        // A pass that collected occurrences must store one before it
        // reflects. A Backfill Reflection collects none: its Run starts
        // from the selected Subject Pages alone (ADR-0011).
        if let Some(arrival) = arrival.filter(|arrival| {
            !arrival.subject_paths.is_empty() && (!stored.is_empty() || batch.events.is_empty())
        }) {
            let wakeup = Wakeup {
                id: WakeupId::generate(),
                workspace_id: batch.workspace_id.clone(),
                rule: WakeupRule::Arrival {
                    subject_paths: arrival.subject_paths.clone(),
                    historical: arrival.historical,
                },
                rule_revision: 1,
                rule_name: "Synced arrival".to_string(),
                agent_id: arrival.agent_id.clone(),
                channel_id: None,
                root_message_id: None,
                instruction: "Reflect on the synced arrivals.".to_string(),
                scheduled_at: batch.received_at,
                state: pagis_core::WakeupState::Pending,
                run_id: None,
                source_count: u32::try_from(if stored.is_empty() {
                    arrival.subject_paths.len()
                } else {
                    stored.len()
                })
                .unwrap_or(u32::MAX),
                created_at: batch.received_at,
                started_at: None,
            };
            insert_wakeup(&mut transaction, &wakeup).await?;
            for (event, _) in &stored {
                link_source(
                    &mut transaction,
                    &batch.workspace_id,
                    &wakeup.id,
                    "incoming_event",
                    event.id.as_str(),
                )
                .await?;
            }
            created.push(wakeup);
        }
        let mut combined = Vec::new();
        for subscription in eligible {
            // Collection and grant revocation can overlap. Recheck the
            // durable rule inside this transaction so a stale collector
            // snapshot cannot recreate work after the rule was blocked.
            let still_active = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM event_subscriptions WHERE id = $1 \
                 AND state = 'active' AND revision = $2 AND approved_revision = revision",
            )
            .bind(subscription.id.as_str())
            .bind(i64::from(subscription.revision))
            .fetch_one(&mut *transaction)
            .await
            .map_err(db_err)?
                > 0;
            if !still_active {
                continue;
            }
            // The activation watermark keeps a first activation and a
            // resume from replaying mail the rule never saw live.
            let matched: Vec<&(IncomingEvent, Option<WakeupLanding>)> = stored
                .iter()
                .filter(|(event, _)| {
                    subscription
                        .watermark_at
                        .is_none_or(|watermark| event.occurred_at > watermark)
                })
                .filter(|(event, _)| matcher.matches(&subscription.filter, &event.metadata))
                .collect();
            if matched.is_empty() {
                continue;
            }
            // A burst joins one Wake-up, but only where the whole burst
            // lands together: two replies that continue two different
            // Threads are two conversations (ADR-0019). The rule's own
            // conversation is the landing of everything else.
            let rule_landing = WakeupLanding {
                channel_id: subscription.channel_id.clone(),
                root_message_id: subscription.root_message_id.clone(),
            };
            let mut groups: Vec<(WakeupLanding, Vec<&IncomingEvent>)> = Vec::new();
            for (event, landing) in matched {
                let landing = landing.clone().unwrap_or_else(|| rule_landing.clone());
                match groups.iter_mut().find(|(found, _)| *found == landing) {
                    Some((_, events)) => events.push(event),
                    None => groups.push((landing, vec![event])),
                }
            }

            for (landing, events) in groups {
                // One batch creates at most one Wake-up per landing, and
                // more mail joins the pending one rather than queueing a
                // second Run.
                let pending = sqlx::query(&format!(
                    "SELECT {WAKEUP_COLUMNS} FROM wakeups WHERE subscription_id = $1 \
                     AND state = 'pending' AND channel_id = $2 \
                     AND root_message_id IS NOT DISTINCT FROM $3"
                ))
                .bind(subscription.id.as_str())
                .bind(landing.channel_id.as_str())
                .bind(landing.root_message_id.as_ref().map(MessageId::as_str))
                .fetch_optional(&mut *transaction)
                .await
                .map_err(db_err)?;
                let (mut wakeup, is_new) = match pending.as_ref().map(row_to_wakeup).transpose()? {
                    Some(wakeup) => (wakeup, false),
                    None => {
                        let wakeup = Wakeup {
                            id: WakeupId::generate(),
                            workspace_id: batch.workspace_id.clone(),
                            rule: WakeupRule::EventSubscription {
                                subscription_id: subscription.id.clone(),
                            },
                            rule_revision: subscription.revision,
                            rule_name: subscription.name.clone(),
                            agent_id: subscription.agent_id.clone(),
                            channel_id: Some(landing.channel_id.clone()),
                            root_message_id: landing.root_message_id.clone(),
                            instruction: subscription.instruction.clone(),
                            scheduled_at: batch.received_at,
                            state: pagis_core::WakeupState::Pending,
                            run_id: None,
                            source_count: 0,
                            created_at: batch.received_at,
                            started_at: None,
                        };
                        insert_wakeup(&mut transaction, &wakeup).await?;
                        (wakeup, true)
                    }
                };
                let count = events.len();
                for event in events {
                    link_source(
                        &mut transaction,
                        &batch.workspace_id,
                        &wakeup.id,
                        "incoming_event",
                        event.id.as_str(),
                    )
                    .await?;
                }
                wakeup.source_count += u32::try_from(count).unwrap_or(u32::MAX);
                sqlx::query("UPDATE wakeups SET source_count = $1 WHERE id = $2")
                    .bind(i64::from(wakeup.source_count))
                    .bind(wakeup.id.as_str())
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_err)?;
                if is_new {
                    created.push(wakeup);
                } else {
                    combined.push(wakeup);
                }
            }
        }

        // Only a Connection keeps a provider cursor.
        if let (Some(cursor), Some(connection_id)) = (&batch.cursor, connection_id) {
            sqlx::query(
                "INSERT INTO provider_cursors (connection_id, event_kind, cursor, updated_at) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (connection_id, event_kind) \
                 DO UPDATE SET cursor = excluded.cursor, updated_at = excluded.updated_at",
            )
            .bind(connection_id.as_str())
            .bind(&batch.event_kind)
            .bind(cursor)
            .bind(batch.received_at)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        }

        let record = SourceBatch {
            id: batch_id,
            workspace_id: batch.workspace_id.clone(),
            source: batch.source.clone(),
            event_kind: batch.event_kind.clone(),
            collected_at: batch.received_at,
            collected_count,
            stored_count: u32::try_from(stored.len()).unwrap_or(u32::MAX),
            wakeup_count: u32::try_from(created.len() + combined.len()).unwrap_or(u32::MAX),
            outcome: if batch.baseline {
                SourceBatchOutcome::Baseline
            } else {
                SourceBatchOutcome::Collected
            },
            detail: None,
        };
        sqlx::query("UPDATE source_batches SET stored_count = $1, wakeup_count = $2 WHERE id = $3")
            .bind(i64::from(record.stored_count))
            .bind(i64::from(record.wakeup_count))
            .bind(record.id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)?;

        Ok(IngestOutcome {
            batch: record,
            events: stored.into_iter().map(|(event, _)| event).collect(),
            created,
            combined,
        })
    }
}

fn cron_window(
    schedule: &pagis_core::Schedule,
    now: i64,
) -> Result<(i64, Option<i64>), pagis_core::StoreError> {
    let expression = schedule
        .cron_expression
        .as_deref()
        .ok_or_else(|| pagis_core::StoreError::Corrupt("cron expression is missing".to_string()))?;
    let cron = croner::Cron::from_str(expression)
        .map_err(|error| pagis_core::StoreError::Corrupt(format!("cron expression: {error}")))?;
    let timezone = schedule
        .timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|error| pagis_core::StoreError::Corrupt(format!("cron timezone: {error}")))?;
    let now = chrono::DateTime::from_timestamp_millis(now)
        .ok_or_else(|| pagis_core::StoreError::Corrupt("cron time is out of range".to_string()))?
        .with_timezone(&timezone);
    let latest = cron
        .find_previous_occurrence(&now, true)
        .map_err(|error| pagis_core::StoreError::Corrupt(format!("cron occurrence: {error}")))?;
    let next = cron
        .find_next_occurrence(&latest, false)
        .map_err(|error| pagis_core::StoreError::Corrupt(format!("cron occurrence: {error}")))?;
    Ok((latest.timestamp_millis(), Some(next.timestamp_millis())))
}

fn missed_count(
    schedule: &pagis_core::Schedule,
    previous_due_at: i64,
    latest_due_at: i64,
) -> Result<u32, pagis_core::StoreError> {
    if latest_due_at <= previous_due_at {
        return Ok(0);
    }
    match (schedule.kind, schedule.interval_ms) {
        (pagis_core::ScheduleKind::Interval, Some(interval_ms)) => {
            Ok(u32::try_from((latest_due_at - previous_due_at) / interval_ms).unwrap_or(u32::MAX))
        }
        (pagis_core::ScheduleKind::Cron, _) => {
            let expression = schedule.cron_expression.as_deref().ok_or_else(|| {
                pagis_core::StoreError::Corrupt("cron expression is missing".to_string())
            })?;
            let cron = croner::Cron::from_str(expression).map_err(|error| {
                pagis_core::StoreError::Corrupt(format!("cron expression: {error}"))
            })?;
            let timezone = schedule
                .timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|error| {
                    pagis_core::StoreError::Corrupt(format!("cron timezone: {error}"))
                })?;
            let mut occurrence = chrono::DateTime::from_timestamp_millis(previous_due_at)
                .ok_or_else(|| {
                    pagis_core::StoreError::Corrupt("cron time is out of range".to_string())
                })?
                .with_timezone(&timezone);
            let mut count = 0_u32;
            loop {
                let next = cron
                    .find_next_occurrence(&occurrence, false)
                    .map_err(|error| {
                        pagis_core::StoreError::Corrupt(format!("cron occurrence: {error}"))
                    })?;
                if next.timestamp_millis() >= latest_due_at {
                    return Ok(count);
                }
                count = count.saturating_add(1);
                occurrence = next;
            }
        }
        _ => Ok(0),
    }
}
