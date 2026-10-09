use async_trait::async_trait;
use pagis_core::{
    AgentId, ChannelId, GrantId, MemoryExposure, MessageId, PendingEvidence, PendingEvidenceId,
    PendingEvidenceRecord, PendingEvidenceStore, PendingUrgency, RunId, RunState, StoreError,
    TriggerKind, WorkspaceId,
};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::db_err;

const NORMAL_DELAY_MS: i64 = 5 * 60 * 1_000;
const NORMAL_MAX_AGE_MS: i64 = 30 * 60 * 1_000;
const URGENT_MAX_AGE_MS: i64 = 60 * 1_000;
const LEASE_MS: i64 = 10 * 60 * 1_000;

#[derive(Clone)]
pub struct PostgresPendingEvidenceStore {
    pool: PgPool,
}

impl PostgresPendingEvidenceStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn lower_cursor(a: Option<String>, b: Option<&MessageId>) -> Option<String> {
    match (a, b) {
        (None, _) | (_, None) => None,
        (Some(a), Some(b)) if a.as_str() <= b.as_str() => Some(a),
        (_, Some(b)) => Some(b.to_string()),
    }
}

async fn insert_sources(
    transaction: &mut Transaction<'_, Postgres>,
    id: &PendingEvidenceId,
    evidence: &PendingEvidence,
) -> Result<(), StoreError> {
    for message_id in &evidence.source_message_ids {
        sqlx::query(
            "INSERT INTO pending_evidence_messages(pending_id,message_id,channel_id,root_message_id,root_key) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
        )
        .bind(id.as_str())
        .bind(message_id.as_str())
        .bind(evidence.channel_id.as_str())
        .bind(evidence.root_message_id.as_ref().map(MessageId::as_str))
        .bind(
            evidence
                .root_message_id
                .as_ref()
                .map_or("", MessageId::as_str),
        )
        .execute(&mut **transaction)
        .await
        .map_err(db_err)?;
    }
    for exposure in &evidence.exposures {
        sqlx::query(
            "INSERT INTO pending_evidence_exposures(pending_id,grant_id,grant_revision) VALUES($1,$2,$3) ON CONFLICT DO NOTHING",
        )
        .bind(id.as_str())
        .bind(exposure.grant_id.as_str())
        .bind(exposure.revision)
        .execute(&mut **transaction)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

async fn upsert_review_cursor(
    transaction: &mut Transaction<'_, Postgres>,
    evidence: &PendingEvidence,
    subject: &str,
    through: &MessageId,
    memory_revision: Option<&str>,
    now: i64,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO pending_review_cursors(workspace_id,agent_id,channel_id,root_message_id,root_key,subject,through_inclusive,memory_revision,reviewed_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) \
         ON CONFLICT(workspace_id,agent_id,channel_id,root_key,subject) DO UPDATE SET through_inclusive=GREATEST(pending_review_cursors.through_inclusive,excluded.through_inclusive),memory_revision=excluded.memory_revision,reviewed_at=excluded.reviewed_at",
    )
    .bind(evidence.workspace_id.as_str())
    .bind(evidence.agent_id.as_str())
    .bind(evidence.channel_id.as_str())
    .bind(evidence.root_message_id.as_ref().map(MessageId::as_str))
    .bind(
        evidence
            .root_message_id
            .as_ref()
            .map_or("", MessageId::as_str),
    )
    .bind(subject)
    .bind(through.as_str())
    .bind(memory_revision)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(db_err)?;
    Ok(())
}

async fn load_record(
    transaction: &mut Transaction<'_, Postgres>,
    id: &PendingEvidenceId,
) -> Result<PendingEvidenceRecord, StoreError> {
    let row = sqlx::query("SELECT * FROM pending_evidence WHERE id=$1")
        .bind(id.as_str())
        .fetch_one(&mut **transaction)
        .await
        .map_err(db_err)?;
    let message_ids = sqlx::query_scalar::<_, String>(
        "SELECT message_id FROM pending_evidence_messages WHERE pending_id=$1 ORDER BY message_id",
    )
    .bind(id.as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(db_err)?
    .into_iter()
    .map(MessageId::from)
    .collect();
    let exposures = sqlx::query(
        "SELECT grant_id,grant_revision FROM pending_evidence_exposures WHERE pending_id=$1 ORDER BY grant_id,grant_revision",
    )
    .bind(id.as_str())
    .fetch_all(&mut **transaction)
    .await
    .map_err(db_err)?
    .into_iter()
    .map(|row| MemoryExposure {
        grant_id: GrantId::from(row.get::<String, _>("grant_id")),
        revision: row.get("grant_revision"),
    })
    .collect();
    let urgency = row
        .get::<String, _>("urgency")
        .parse()
        .map_err(StoreError::Corrupt)?;
    let state = row
        .get::<String, _>("state")
        .parse()
        .map_err(StoreError::Corrupt)?;
    Ok(PendingEvidenceRecord {
        id: id.clone(),
        evidence: PendingEvidence {
            workspace_id: WorkspaceId::from(row.get::<String, _>("workspace_id")),
            agent_id: AgentId::from(row.get::<String, _>("agent_id")),
            channel_id: ChannelId::from(row.get::<String, _>("channel_id")),
            root_message_id: row
                .get::<Option<String>, _>("root_message_id")
                .map(MessageId::from),
            subject: row.get("subject"),
            after_exclusive: row
                .get::<Option<String>, _>("after_exclusive")
                .map(MessageId::from),
            through_inclusive: MessageId::from(row.get::<String, _>("through_inclusive")),
            source_message_ids: message_ids,
            exposures,
            reason: row.get("reason"),
            urgency,
            created_at: row.get("created_at"),
        },
        eligible_at: row.get("eligible_at"),
        maximum_due_at: row.get("maximum_due_at"),
        attempt_count: u32::try_from(row.get::<i64, _>("attempt_count"))
            .map_err(|error| StoreError::Corrupt(format!("pending attempt count: {error}")))?,
        state,
        revision: u32::try_from(row.get::<i64, _>("revision"))
            .map_err(|error| StoreError::Corrupt(format!("pending revision: {error}")))?,
        lease_run_id: row
            .get::<Option<String>, _>("lease_run_id")
            .map(RunId::from),
        error: row.get("error"),
        memory_revision: row.get("memory_revision"),
        completed_at: row.get("completed_at"),
        failed_overlap_id: row
            .get::<Option<String>, _>("failed_overlap_id")
            .map(PendingEvidenceId::from),
    })
}

#[async_trait]
impl PendingEvidenceStore for PostgresPendingEvidenceStore {
    async fn record(
        &self,
        mut evidence: PendingEvidence,
    ) -> Result<Option<PendingEvidenceRecord>, StoreError> {
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        let cursor = sqlx::query_scalar::<_, String>(
            "SELECT through_inclusive FROM pending_review_cursors WHERE workspace_id=$1 AND agent_id=$2 AND channel_id=$3 AND root_key=$4 AND subject=$5",
        )
        .bind(evidence.workspace_id.as_str())
        .bind(evidence.agent_id.as_str())
        .bind(evidence.channel_id.as_str())
        .bind(
            evidence
                .root_message_id
                .as_ref()
                .map_or("", MessageId::as_str),
        )
        .bind(&evidence.subject)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_err)?
        .map(MessageId::from);
        if let Some(cursor) = cursor {
            evidence
                .source_message_ids
                .retain(|message_id| message_id.as_str() > cursor.as_str());
            let Some(through) = evidence
                .source_message_ids
                .iter()
                .max_by_key(|message_id| message_id.as_str())
                .cloned()
            else {
                return Ok(None);
            };
            evidence.after_exclusive = Some(cursor);
            evidence.through_inclusive = through;
        }
        let existing = sqlx::query(
            "SELECT id,after_exclusive,through_inclusive,reason,urgency,maximum_due_at FROM pending_evidence \
             WHERE workspace_id=$1 AND agent_id=$2 AND subject=$3 AND state='pending'",
        )
        .bind(evidence.workspace_id.as_str())
        .bind(evidence.agent_id.as_str())
        .bind(&evidence.subject)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_err)?;
        let id = if let Some(row) = existing {
            let id = PendingEvidenceId::from(row.get::<String, _>("id"));
            let urgency = if evidence.urgency == PendingUrgency::Urgent
                || row.get::<String, _>("urgency") == "urgent"
            {
                PendingUrgency::Urgent
            } else {
                PendingUrgency::Normal
            };
            let eligible_at = match urgency {
                PendingUrgency::Urgent => evidence.created_at,
                PendingUrgency::Normal => evidence.created_at + NORMAL_DELAY_MS,
            };
            let newest = row
                .get::<String, _>("through_inclusive")
                .max(evidence.through_inclusive.to_string());
            let old_reason: String = row.get("reason");
            let reason = if old_reason == evidence.reason {
                old_reason
            } else {
                format!("{old_reason}\n{}", evidence.reason)
            };
            sqlx::query(
                "UPDATE pending_evidence SET after_exclusive=$1,through_inclusive=$2,reason=$3,urgency=$4,eligible_at=$5,maximum_due_at=LEAST(maximum_due_at,$6),revision=revision+1 WHERE id=$7 AND state='pending'",
            )
            .bind(lower_cursor(
                row.get::<Option<String>, _>("after_exclusive"),
                evidence.after_exclusive.as_ref(),
            ))
            .bind(newest)
            .bind(reason)
            .bind(urgency.as_str())
            .bind(eligible_at)
            .bind(match urgency {
                PendingUrgency::Normal => evidence.created_at + NORMAL_MAX_AGE_MS,
                PendingUrgency::Urgent => evidence.created_at + URGENT_MAX_AGE_MS,
            })
            .bind(id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            id
        } else {
            let id = PendingEvidenceId::generate();
            let failed_overlap = sqlx::query_scalar::<_, String>(
                "SELECT id FROM pending_evidence WHERE workspace_id=$1 AND agent_id=$2 AND subject=$3 AND state='failed' ORDER BY created_at DESC,id DESC LIMIT 1",
            )
            .bind(evidence.workspace_id.as_str())
            .bind(evidence.agent_id.as_str())
            .bind(&evidence.subject)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(db_err)?;
            let (eligible_at, maximum_due_at) = match evidence.urgency {
                PendingUrgency::Normal => (
                    evidence.created_at + NORMAL_DELAY_MS,
                    evidence.created_at + NORMAL_MAX_AGE_MS,
                ),
                PendingUrgency::Urgent => {
                    (evidence.created_at, evidence.created_at + URGENT_MAX_AGE_MS)
                }
            };
            sqlx::query(
                "INSERT INTO pending_evidence(id,workspace_id,agent_id,channel_id,root_message_id,subject,after_exclusive,through_inclusive,reason,urgency,eligible_at,maximum_due_at,state,failed_overlap_id,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12, 'pending',$13,$14)",
            )
            .bind(id.as_str())
            .bind(evidence.workspace_id.as_str())
            .bind(evidence.agent_id.as_str())
            .bind(evidence.channel_id.as_str())
            .bind(evidence.root_message_id.as_ref().map(MessageId::as_str))
            .bind(&evidence.subject)
            .bind(evidence.after_exclusive.as_ref().map(MessageId::as_str))
            .bind(evidence.through_inclusive.as_str())
            .bind(&evidence.reason)
            .bind(evidence.urgency.as_str())
            .bind(eligible_at)
            .bind(maximum_due_at)
            .bind(failed_overlap)
            .bind(evidence.created_at)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            id
        };
        insert_sources(&mut transaction, &id, &evidence).await?;
        let record = load_record(&mut transaction, &id).await?;
        transaction.commit().await.map_err(db_err)?;
        Ok(Some(record))
    }

    async fn review_cursor(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        channel_id: &ChannelId,
        root_message_id: Option<&MessageId>,
        subject: &str,
    ) -> Result<Option<MessageId>, StoreError> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT through_inclusive FROM pending_review_cursors WHERE workspace_id=$1 AND agent_id=$2 AND channel_id=$3 AND root_key=$4 AND subject=$5",
        )
        .bind(workspace_id.as_str())
        .bind(agent_id.as_str())
        .bind(channel_id.as_str())
        .bind(root_message_id.map_or("", MessageId::as_str))
        .bind(subject)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
            .map(MessageId::from))
    }

    async fn settle_compaction(
        &self,
        evidence: &PendingEvidence,
        memory_revision: Option<&str>,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        upsert_review_cursor(
            &mut transaction,
            evidence,
            &evidence.subject,
            &evidence.through_inclusive,
            memory_revision,
            now,
        )
        .await?;

        let covered = sqlx::query(
            "SELECT p.id,p.subject,MAX(m.message_id) AS through_inclusive \
             FROM pending_evidence p JOIN pending_evidence_messages m ON m.pending_id=p.id \
             WHERE p.workspace_id=$1 AND p.agent_id=$2 AND p.state='pending' \
             GROUP BY p.id,p.subject \
             HAVING SUM(CASE WHEN m.channel_id=$3 AND m.root_key=$4 AND m.message_id<=$5 THEN 0 ELSE 1 END)=0",
        )
        .bind(evidence.workspace_id.as_str())
        .bind(evidence.agent_id.as_str())
        .bind(evidence.channel_id.as_str())
        .bind(
            evidence
                .root_message_id
                .as_ref()
                .map_or("", MessageId::as_str),
        )
        .bind(evidence.through_inclusive.as_str())
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_err)?;
        for row in covered {
            let id: String = row.get("id");
            let subject: String = row.get("subject");
            let through = MessageId::from(row.get::<String, _>("through_inclusive"));
            upsert_review_cursor(
                &mut transaction,
                evidence,
                &subject,
                &through,
                memory_revision,
                now,
            )
            .await?;
            sqlx::query(
                "UPDATE pending_evidence SET state='completed',memory_revision=$1,completed_at=$2,error=NULL WHERE id=$3 AND state='pending'",
            )
            .bind(memory_revision)
            .bind(now)
            .bind(id)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        }
        transaction.commit().await.map_err(db_err)
    }

    async fn next_due_at(&self) -> Result<Option<i64>, StoreError> {
        sqlx::query_scalar(
            "SELECT MIN(due_at) FROM (\
               SELECT LEAST(eligible_at,maximum_due_at) AS due_at FROM pending_evidence WHERE state='pending'\
               UNION ALL SELECT lease_expires_at FROM pending_evidence WHERE state='leased' AND lease_expires_at IS NOT NULL\
             ) due",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)
    }

    async fn pending_agents(&self, now: i64) -> Result<Vec<(WorkspaceId, AgentId)>, StoreError> {
        Ok(sqlx::query_as::<_, (String, String)>(
            "SELECT DISTINCT p.workspace_id, p.agent_id FROM pending_evidence p \
             LEFT JOIN runs r ON r.id=p.lease_run_id \
             WHERE (p.state='pending' AND LEAST(p.eligible_at,p.maximum_due_at)<=$1) OR \
             (p.state='leased' AND p.lease_expires_at<=$2 AND r.state IN ('completed','failed','canceled')) \
             ORDER BY p.workspace_id, p.agent_id",
        )
        .bind(now)
        .bind(now)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(|(workspace, agent)| (WorkspaceId::from(workspace), AgentId::from(agent)))
        .collect())
    }

    async fn claim(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        arrival_slots: u32,
        now: i64,
    ) -> Result<Vec<pagis_core::PendingReviewClaim>, StoreError> {
        if arrival_slots == 0 {
            return Ok(Vec::new());
        }
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        sqlx::query(
            "UPDATE pending_evidence SET state=CASE WHEN attempt_count>=3 THEN 'failed' ELSE 'pending' END, \
             eligible_at=CASE attempt_count WHEN 1 THEN $1 WHEN 2 THEN $2 ELSE eligible_at END, \
             lease_expires_at=NULL,error='review lease expired',revision=revision+1 \
             WHERE agent_id=$3 AND workspace_id=$4 AND state='leased' AND lease_expires_at<=$5 \
             AND lease_run_id IN (\
               SELECT id FROM runs WHERE state IN ('completed','failed','canceled')\
             )",
        )
        .bind(now + 60_000)
        .bind(now + 5 * 60_000)
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM pending_evidence \
             WHERE agent_id=$1 AND workspace_id=$2 AND state='pending' \
             AND LEAST(eligible_at,maximum_due_at)<=$3 \
             ORDER BY (urgency='urgent') DESC,maximum_due_at,created_at,id LIMIT $4",
        )
        .bind(agent_id.as_str())
        .bind(workspace_id.as_str())
        .bind(now)
        .bind(i64::from(arrival_slots))
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_err)?;
        let mut claims = Vec::with_capacity(ids.len());
        for value in ids {
            let id = PendingEvidenceId::from(value);
            let run_id = RunId::generate();
            let pending = load_record(&mut transaction, &id).await?;
            let run = pagis_core::Run {
                title: pagis_core::run_title(pagis_core::RunTitleSource::Review),
                id: run_id,
                workspace_id: pending.workspace_id.clone(),
                agent_id: pending.agent_id.clone(),
                channel_id: Some(pending.channel_id.clone()),
                root_message_id: pending.root_message_id.clone(),
                trigger_kind: TriggerKind::Review,
                trigger_ref: Some(pending.id.to_string()),
                hop_count: 0,
                origin: None,
                state: RunState::Queued,
                failure_kind: None,
                dismissed_at: None,
                error: None,
                started_at: None,
                ended_at: None,
                created_at: now,
            };
            sqlx::query(
                "INSERT INTO runs(id,workspace_id,agent_id,channel_id,root_message_id,trigger_kind,trigger_ref,hop_count,state,created_at,title) VALUES($1,$2,$3,$4,$5,'review',$6,0,'queued',$7,$8)",
            )
            .bind(run.id.as_str())
            .bind(run.workspace_id.as_str())
            .bind(run.agent_id.as_str())
            .bind(run.channel_id.as_ref().map(ChannelId::as_str))
            .bind(run.root_message_id.as_ref().map(MessageId::as_str))
            .bind(run.trigger_ref.as_deref())
            .bind(now)
            .bind(&run.title)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            let updated = sqlx::query(
                "UPDATE pending_evidence SET state='leased',attempt_count=attempt_count+1,revision=revision+1,lease_run_id=$1,leased_at=$2,lease_expires_at=$3 WHERE id=$4 AND state='pending'",
            )
            .bind(run.id.as_str())
            .bind(now)
            .bind(now + LEASE_MS)
            .bind(id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?
            .rows_affected();
            if updated != 1 {
                sqlx::query("DELETE FROM runs WHERE id=$1")
                    .bind(run.id.as_str())
                    .execute(&mut *transaction)
                    .await
                    .map_err(db_err)?;
                continue;
            }
            let record = load_record(&mut transaction, &id).await?;
            claims.push(pagis_core::PendingReviewClaim {
                lease_revision: record.revision,
                evidence: record,
                run,
            });
        }
        transaction.commit().await.map_err(db_err)?;
        Ok(claims)
    }

    async fn for_run(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
    ) -> Result<Option<PendingEvidenceRecord>, StoreError> {
        let Some(id) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM pending_evidence WHERE lease_run_id=$1 AND workspace_id=$2",
        )
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?
        else {
            return Ok(None);
        };
        let mut transaction = self.pool.begin().await.map_err(db_err)?;
        load_record(&mut transaction, &PendingEvidenceId::from(id))
            .await
            .map(Some)
    }

    async fn leased(&self) -> Result<Vec<PendingEvidenceRecord>, StoreError> {
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM pending_evidence WHERE state='leased' ORDER BY leased_at,id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        let mut records = Vec::with_capacity(ids.len());
        for id in ids {
            let mut transaction = self.pool.begin().await.map_err(db_err)?;
            records.push(load_record(&mut transaction, &PendingEvidenceId::from(id)).await?);
        }
        Ok(records)
    }

    async fn complete(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        lease_revision: u32,
        memory_revision: Option<&str>,
        now: i64,
    ) -> Result<bool, StoreError> {
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        let row = sqlx::query(
            "SELECT id,workspace_id,agent_id,subject FROM pending_evidence \
             WHERE lease_run_id=$1 AND workspace_id=$2 AND revision=$3 AND state='leased'",
        )
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .bind(i64::from(lease_revision))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Ok(false);
        };
        let id: String = row.get("id");
        // `root_key` is `COALESCE(root_message_id,'')`, but Postgres does
        // not know that, so `root_message_id` must also be a group column.
        let origins = sqlx::query(
            "SELECT channel_id,root_message_id,root_key,MAX(message_id) AS through_inclusive FROM pending_evidence_messages WHERE pending_id=$1 GROUP BY channel_id,root_key,root_message_id",
        )
        .bind(&id)
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_err)?;
        for origin in origins {
            sqlx::query(
                "INSERT INTO pending_review_cursors(workspace_id,agent_id,channel_id,root_message_id,root_key,subject,through_inclusive,memory_revision,reviewed_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) \
                 ON CONFLICT(workspace_id,agent_id,channel_id,root_key,subject) DO UPDATE SET through_inclusive=GREATEST(pending_review_cursors.through_inclusive,excluded.through_inclusive),memory_revision=excluded.memory_revision,reviewed_at=excluded.reviewed_at",
            )
            .bind(row.get::<String, _>("workspace_id"))
            .bind(row.get::<String, _>("agent_id"))
            .bind(origin.get::<String, _>("channel_id"))
            .bind(origin.get::<Option<String>, _>("root_message_id"))
            .bind(origin.get::<String, _>("root_key"))
            .bind(row.get::<String, _>("subject"))
            .bind(origin.get::<String, _>("through_inclusive"))
            .bind(memory_revision)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        }
        let changed = sqlx::query(
            "UPDATE pending_evidence SET state='completed',memory_revision=$1,completed_at=$2,lease_expires_at=NULL,error=NULL WHERE lease_run_id=$3 AND revision=$4 AND state='leased'",
        )
        .bind(memory_revision)
        .bind(now)
        .bind(run_id.as_str())
        .bind(i64::from(lease_revision))
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?
        .rows_affected()
            == 1;
        transaction.commit().await.map_err(db_err)?;
        Ok(changed)
    }

    async fn fail(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        lease_revision: u32,
        error: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        let changed = sqlx::query(
            "UPDATE pending_evidence SET state=CASE WHEN attempt_count>=3 THEN 'failed' ELSE 'pending' END, \
             eligible_at=CASE attempt_count WHEN 1 THEN $1 WHEN 2 THEN $2 ELSE eligible_at END, \
             lease_expires_at=NULL,error=$3,revision=revision+1 \
             WHERE lease_run_id=$4 AND workspace_id=$5 AND revision=$6 AND state='leased'",
        )
        .bind(now + 60_000)
        .bind(now + 5 * 60_000)
        .bind(error)
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .bind(i64::from(lease_revision))
        .execute(&self.pool)
        .await
        .map_err(db_err)?
        .rows_affected()
            == 1;
        Ok(changed)
    }

    async fn invalidate(
        &self,
        workspace_id: &WorkspaceId,
        run_id: &RunId,
        lease_revision: u32,
        now: i64,
    ) -> Result<bool, StoreError> {
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        let id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM pending_evidence \
             WHERE lease_run_id=$1 AND workspace_id=$2 AND revision=$3 AND state='leased'",
        )
        .bind(run_id.as_str())
        .bind(workspace_id.as_str())
        .bind(i64::from(lease_revision))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_err)?;
        let Some(id) = id else {
            return Ok(false);
        };
        sqlx::query("DELETE FROM pending_evidence_messages WHERE pending_id=$1")
            .bind(&id)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM pending_evidence_exposures WHERE pending_id=$1")
            .bind(&id)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        let changed = sqlx::query(
            "UPDATE pending_evidence SET state='invalidated',reason='Source access changed.',error='source access changed',completed_at=$1,lease_expires_at=NULL,revision=revision+1 WHERE id=$2 AND state='leased'",
        )
        .bind(now)
        .bind(&id)
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?
        .rows_affected() == 1;
        transaction.commit().await.map_err(db_err)?;
        Ok(changed)
    }

    async fn retry(
        &self,
        workspace_id: &WorkspaceId,
        id: &PendingEvidenceId,
        now: i64,
    ) -> Result<bool, StoreError> {
        let changed = sqlx::query(
            "UPDATE pending_evidence SET state='pending',attempt_count=0,eligible_at=$1,error=NULL,lease_run_id=NULL,leased_at=NULL,lease_expires_at=NULL,revision=revision+1 \
             WHERE id=$2 AND workspace_id=$3 AND state='failed' AND NOT EXISTS (SELECT 1 FROM pending_evidence open WHERE open.workspace_id=pending_evidence.workspace_id AND open.agent_id=pending_evidence.agent_id AND open.subject=pending_evidence.subject AND open.state='pending')",
        )
        .bind(now)
        .bind(id.as_str())
        .bind(workspace_id.as_str())
        .execute(&self.pool)
        .await
        .map_err(db_err)?
        .rows_affected()
            == 1;
        Ok(changed)
    }
}
