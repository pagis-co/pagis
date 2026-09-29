use async_trait::async_trait;
use pagis_core::StoreError;
use pagis_core::knowledge::*;
use sqlx::{PgPool, Row};

/// Page Signals read the messages of a thread, not their history: one
/// version of each id, the newest one acquired. A version the mailbox
/// replaced leaves no signal behind, and neither does a message the
/// mailbox no longer holds, whose newest version carries no metadata.
const NEWEST_VERSION: &str = "v.sequence = (SELECT MAX(n.sequence) FROM source_versions n \
    WHERE n.workspace_id = v.workspace_id AND n.connection_id = v.connection_id \
    AND n.resource = v.resource AND n.id = v.id)";

use crate::{begin_write, db_err};

/// The first failure holds an arrival for two minutes, and each later
/// failure doubles the wait up to one hour.
const ARRIVAL_RETRY_STEP_MS: i64 = 60_000;
const ARRIVAL_RETRY_CAP_MS: i64 = 3_600_000;
/// The fifth failure skips the arrival instead of holding it again.
const ARRIVAL_ATTEMPT_LIMIT: i64 = 5;

#[derive(Clone)]
pub struct PostgresKnowledgeStore {
    pool: PgPool,
}

impl PostgresKnowledgeStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn decode<T: serde::de::DeserializeOwned>(text: String) -> Result<T, StoreError> {
    serde_json::from_str(&text).map_err(|_| StoreError::Corrupt("invalid knowledge record".into()))
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|_| StoreError::Corrupt("invalid knowledge record".into()))
}

#[async_trait]
impl KnowledgeStore for PostgresKnowledgeStore {
    async fn pending_invalidations(
        &self,
        workspace: &pagis_core::WorkspaceId,
        limit: u32,
    ) -> Result<Vec<KnowledgeInvalidation>, StoreError> {
        crate::knowledge_events::pending(&self.pool, workspace, limit).await
    }

    async fn acknowledge_invalidation(
        &self,
        workspace: &pagis_core::WorkspaceId,
        id: i64,
    ) -> Result<(), StoreError> {
        crate::knowledge_events::acknowledge(&self.pool, workspace, id).await
    }
    async fn versions(
        &self,
        key: &SourceKey,
        after_sequence: i64,
        limit: u32,
    ) -> Result<Vec<SourceVersion>, StoreError> {
        read_versions(&self.pool, key, after_sequence, limit, None).await
    }

    async fn arrivals(
        &self,
        key: &SourceKey,
        limit: u32,
        now: i64,
    ) -> Result<Vec<SourceVersion>, StoreError> {
        read_versions(&self.pool, key, 0, limit, Some(now)).await
    }

    async fn acknowledge_arrival(
        &self,
        key: &SourceKey,
        sequence: i64,
        now: i64,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE source_versions SET arrival_ack_at = COALESCE(arrival_ack_at, $1), arrival_outcome = COALESCE(arrival_outcome, 'appended') WHERE workspace_id = $2 AND connection_id = $3 AND resource = $4 AND sequence = $5")
            .bind(now).bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(sequence).execute(&self.pool).await.map_err(db_err)?;
        Ok(())
    }

    async fn fail_arrival(
        &self,
        key: &SourceKey,
        sequence: i64,
        permanent: bool,
        reason: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        // The shift stays inside the exponent cap, and the delay stays
        // inside ARRIVAL_RETRY_CAP_MS. The shift count is an `integer`
        // and the shifted value a `bigint`, so the product of the step
        // and the widest shift still fits.
        sqlx::query(&format!(
            "UPDATE source_versions SET arrival_attempts = arrival_attempts + 1, arrival_failure = $1, \
             arrival_retry_at = $2 + LEAST({ARRIVAL_RETRY_STEP_MS} * (1::bigint << LEAST(arrival_attempts + 1, 20)::int), {ARRIVAL_RETRY_CAP_MS}), \
             arrival_ack_at = CASE WHEN $3 OR arrival_attempts + 1 >= {ARRIVAL_ATTEMPT_LIMIT} THEN $4 ELSE arrival_ack_at END, \
             arrival_outcome = CASE WHEN $5 OR arrival_attempts + 1 >= {ARRIVAL_ATTEMPT_LIMIT} THEN 'skipped' ELSE arrival_outcome END \
             WHERE workspace_id = $6 AND connection_id = $7 AND resource = $8 AND sequence = $9 AND arrival_ack_at IS NULL"
        ))
            .bind(reason)
            .bind(now)
            .bind(permanent)
            .bind(now)
            .bind(permanent)
            .bind(key.workspace_id.as_str())
            .bind(key.connection_id.as_str())
            .bind(&key.resource)
            .bind(sequence)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn collection_error(
        &self,
        key: &SourceKey,
        arrival: bool,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        let query = if arrival {
            "UPDATE sync_resources SET arrival_error = $1 WHERE workspace_id = $2 AND connection_id = $3 AND resource = $4"
        } else {
            "UPDATE sync_resources SET acquisition_error = $1 WHERE workspace_id = $2 AND connection_id = $3 AND resource = $4"
        };
        sqlx::query(query)
            .bind(error)
            .bind(key.workspace_id.as_str())
            .bind(key.connection_id.as_str())
            .bind(&key.resource)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn record_reflection(
        &self,
        key: &SourceKey,
        reflection: &PageReflection,
    ) -> Result<(), StoreError> {
        // A new decision drops the Wake-up of the old one: the pages of
        // that Run are the pages the old decision selected.
        sqlx::query("INSERT INTO page_reflections(workspace_id, connection_id, resource, page_path, page_subject, filter_revision, decided_by, verdict, rule_index, reason, wakeup_id, reflected_at, decided_at) \
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULL, NULL, $11) \
            ON CONFLICT(workspace_id, connection_id, resource, page_path, filter_revision) DO UPDATE SET \
            page_subject = excluded.page_subject, decided_by = excluded.decided_by, verdict = excluded.verdict, \
            rule_index = excluded.rule_index, reason = excluded.reason, wakeup_id = NULL, reflected_at = NULL, \
            decided_at = excluded.decided_at")
            .bind(key.workspace_id.as_str())
            .bind(key.connection_id.as_str())
            .bind(&key.resource)
            .bind(&reflection.page_path)
            .bind(&reflection.page_subject)
            .bind(reflection.filter_revision)
            .bind(reflection.decided_by.as_str())
            .bind(reflection.selection.verdict.as_str())
            .bind(reflection.selection.rule.map(|index| index as i64))
            .bind(&reflection.selection.reason)
            .bind(reflection.decided_at)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    async fn backfill_candidates(
        &self,
        key: &SourceKey,
        filter_revision: i64,
        limit: u32,
    ) -> Result<Vec<BackfillPage>, StoreError> {
        // The subject is read the way the arrival appends it: the thread
        // of the event, else the event itself. A subject whose versions
        // all lack metadata waits for its metadata (ADR-0011).
        // A Postgres `HAVING` clause cannot name an output column, so
        // the subject expression stands again in each of its conditions.
        let rows = sqlx::query(
            "SELECT COALESCE(arrival::jsonb #>> '{metadata,thread_id}', arrival::jsonb ->> 'provider_event_id') AS subject, \
             MAX(COALESCE((arrival::jsonb ->> 'occurred_at')::bigint, observed_at)) AS newest_at \
             FROM source_versions WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3 \
             AND historical = true AND arrival IS NOT NULL \
             GROUP BY subject \
             HAVING COALESCE(arrival::jsonb #>> '{metadata,thread_id}', arrival::jsonb ->> 'provider_event_id') IS NOT NULL \
             AND bool_or((record::jsonb ->> 'metadata') IS NOT NULL) \
             AND COALESCE(arrival::jsonb #>> '{metadata,thread_id}', arrival::jsonb ->> 'provider_event_id') NOT IN \
             (SELECT page_subject FROM page_reflections WHERE workspace_id = $4 AND connection_id = $5 \
              AND resource = $6 AND filter_revision = $7) \
             ORDER BY newest_at DESC LIMIT $8",
        )
        .bind(key.workspace_id.as_str())
        .bind(key.connection_id.as_str())
        .bind(&key.resource)
        .bind(key.workspace_id.as_str())
        .bind(key.connection_id.as_str())
        .bind(&key.resource)
        .bind(filter_revision)
        .bind(i64::from(limit.min(500)))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .iter()
            .map(|row| BackfillPage {
                subject: row.get("subject"),
                newest_at: row.get("newest_at"),
            })
            .collect())
    }

    async fn release_failed_backfill_runs(&self, key: &SourceKey) -> Result<u64, StoreError> {
        // A Run that failed or was canceled reflected nothing. Its pages
        // wait again, and the Wake-up leaves the budget window.
        let released = sqlx::query(
            "UPDATE page_reflections SET wakeup_id = NULL, reflected_at = NULL, attempts = attempts + 1 \
             WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3 AND decided_by = 'backfill' \
             AND wakeup_id IN (SELECT w.id FROM wakeups w JOIN runs r ON r.id = w.run_id \
                               WHERE r.state IN ('failed', 'canceled'))",
        )
        .bind(key.workspace_id.as_str())
        .bind(key.connection_id.as_str())
        .bind(&key.resource)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(released.rows_affected())
    }

    async fn backfill_batch(
        &self,
        key: &SourceKey,
        filter_revision: i64,
        limit: u32,
    ) -> Result<Vec<String>, StoreError> {
        sqlx::query_scalar(&format!(
            "SELECT page_path FROM page_reflections WHERE workspace_id = $1 AND connection_id = $2 \
             AND resource = $3 AND filter_revision = $4 AND decided_by = 'backfill' \
             AND verdict = 'reflect' AND wakeup_id IS NULL AND attempts < {BACKFILL_ATTEMPTS_PER_PAGE} \
             ORDER BY decided_at, page_path LIMIT $5"
        ))
        .bind(key.workspace_id.as_str())
        .bind(key.connection_id.as_str())
        .bind(&key.resource)
        .bind(filter_revision)
        .bind(i64::from(limit.min(500)))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)
    }

    async fn record_backfill_run(
        &self,
        key: &SourceKey,
        filter_revision: i64,
        page_paths: &[String],
        wakeup_id: &pagis_core::WakeupId,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut tx = begin_write(&self.pool).await.map_err(db_err)?;
        for path in page_paths {
            sqlx::query(
                "UPDATE page_reflections SET wakeup_id = $1, reflected_at = $2 \
                 WHERE workspace_id = $3 AND connection_id = $4 AND resource = $5 \
                 AND page_path = $6 AND filter_revision = $7 AND wakeup_id IS NULL",
            )
            .bind(wakeup_id.as_str())
            .bind(now)
            .bind(key.workspace_id.as_str())
            .bind(key.connection_id.as_str())
            .bind(&key.resource)
            .bind(path)
            .bind(filter_revision)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)
    }

    async fn versions_without_metadata(
        &self,
        key: &SourceKey,
        limit: u32,
    ) -> Result<Vec<SourceVersion>, StoreError> {
        let rows = sqlx::query(
            "SELECT sequence, record, observed_at, historical, arrival FROM source_versions \
            WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3 AND operation = 'upsert' \
            AND (record::jsonb ->> 'metadata') IS NULL ORDER BY sequence LIMIT $4",
        )
        .bind(key.workspace_id.as_str())
        .bind(key.connection_id.as_str())
        .bind(&key.resource)
        .bind(i64::from(limit.min(500)))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(version_row).collect()
    }

    async fn complete_metadata(
        &self,
        key: &SourceKey,
        sequence: i64,
        metadata: &serde_json::Value,
    ) -> Result<(), StoreError> {
        let metadata = serde_json::to_string(metadata)
            .map_err(|_| StoreError::Corrupt("invalid metadata".into()))?;
        let mut tx = begin_write(&self.pool).await.map_err(db_err)?;
        let row = sqlx::query("UPDATE source_versions SET record = jsonb_set(record::jsonb, '{metadata}', ($1)::jsonb)::text \
            WHERE sequence = $2 AND workspace_id = $3 AND connection_id = $4 AND resource = $5 \
            RETURNING id, version, COALESCE(arrival::jsonb #>> '{metadata,thread_id}', arrival::jsonb ->> 'provider_event_id') AS subject")
            .bind(&metadata).bind(sequence).bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
            .fetch_optional(&mut *tx).await.map_err(db_err)?;
        let Some(row) = row else {
            return Err(StoreError::Corrupt("the version does not exist".into()));
        };
        // The current record of the item keeps the same facts.
        sqlx::query("UPDATE source_items SET record = jsonb_set(record::jsonb, '{metadata}', ($1)::jsonb)::text \
            WHERE workspace_id = $2 AND connection_id = $3 AND resource = $4 AND id = $5 AND version = $6")
            .bind(&metadata).bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
            .bind(row.get::<String, _>("id")).bind(row.get::<String, _>("version"))
            .execute(&mut *tx).await.map_err(db_err)?;
        if let Some(subject) = row.get::<Option<String>, _>("subject") {
            sqlx::query("DELETE FROM page_reflections WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3 \
                AND page_subject = $4 AND decided_by = 'backfill' AND wakeup_id IS NULL")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(subject)
                .execute(&mut *tx).await.map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)
    }

    async fn parent_metadata(
        &self,
        key: &SourceKey,
        parent: &str,
    ) -> Result<Vec<serde_json::Value>, StoreError> {
        let rows: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT v.record::jsonb ->> 'metadata' FROM source_versions v \
            WHERE v.workspace_id = $1 AND v.connection_id = $2 AND v.resource = $3 AND v.record::jsonb ->> 'parent' = $4 \
            AND (v.record::jsonb ->> 'metadata') IS NOT NULL AND {NEWEST_VERSION} ORDER BY v.sequence"
        ))
            .bind(key.workspace_id.as_str())
            .bind(key.connection_id.as_str())
            .bind(&key.resource)
            .bind(parent)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        rows.into_iter().map(decode).collect()
    }

    async fn parents_metadata(
        &self,
        key: &SourceKey,
    ) -> Result<Vec<(String, Vec<serde_json::Value>)>, StoreError> {
        let rows: Vec<(String, String)> = sqlx::query_as(&format!(
            "SELECT v.record::jsonb ->> 'parent', v.record::jsonb ->> 'metadata' \
            FROM source_versions v WHERE v.workspace_id = $1 AND v.connection_id = $2 AND v.resource = $3 \
            AND (v.record::jsonb ->> 'parent') IS NOT NULL AND (v.record::jsonb ->> 'metadata') IS NOT NULL \
            AND {NEWEST_VERSION} ORDER BY v.record::jsonb ->> 'parent', v.sequence"
        ))
            .bind(key.workspace_id.as_str())
            .bind(key.connection_id.as_str())
            .bind(&key.resource)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let mut parents: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
        for (parent, metadata) in rows {
            let metadata = decode(metadata)?;
            match parents.last_mut() {
                Some((last, versions)) if *last == parent => versions.push(metadata),
                _ => parents.push((parent, vec![metadata])),
            }
        }
        Ok(parents)
    }

    async fn parents_with_metadata(
        &self,
        key: &SourceKey,
        field: &str,
        value: &str,
        since: i64,
    ) -> Result<i64, StoreError> {
        // The field is a JSON member name, so a name with a quote or a
        // path separator in it cannot reach the path expression.
        if field.is_empty()
            || !field
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(StoreError::Corrupt("invalid signal field".into()));
        }
        sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM (SELECT v.record::jsonb ->> 'parent' AS parent FROM source_versions v \
             WHERE v.workspace_id = $1 AND v.connection_id = $2 AND v.resource = $3 \
             AND v.record::jsonb #>> '{{metadata,{field}}}' = $4 AND (v.record::jsonb ->> 'parent') IS NOT NULL \
             AND {NEWEST_VERSION} \
             GROUP BY parent HAVING MAX(COALESCE((v.record::jsonb ->> 'source_at')::bigint, 0)) >= $5) parents"
        ))
            .bind(key.workspace_id.as_str())
            .bind(key.connection_id.as_str())
            .bind(&key.resource)
            .bind(value)
            .bind(since)
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)
    }

    async fn reset(
        &self,
        key: &SourceKey,
        expected_revision: i64,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut tx = begin_write(&self.pool).await.map_err(db_err)?;
        let changed = sqlx::query("UPDATE sync_resources SET checkpoint = 'null', caught_up = false, revision = revision + 1, cursor_revision = cursor_revision + 1, updated_at = $1 WHERE workspace_id = $2 AND connection_id = $3 AND resource = $4 AND cursor_revision = $5")
            .bind(now).bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(expected_revision)
            .execute(&mut *tx).await.map_err(db_err)?.rows_affected();
        if changed != 1 {
            return Err(StoreError::Conflict("sync changed during reset".into()));
        }
        invalidate(&mut tx, key).await?;
        tx.commit().await.map_err(db_err)
    }

    async fn configure(&self, config: SyncConfig, now: i64) -> Result<SyncStatus, StoreError> {
        if config.resource.trim().is_empty() || config.since < 0 || config.since > now {
            return Err(StoreError::Conflict("invalid sync scope".into()));
        }
        let key = SourceKey {
            workspace_id: config.workspace_id.clone(),
            connection_id: config.connection_id.clone(),
            resource: config.resource.clone(),
        };
        let mut tx = begin_write(&self.pool).await.map_err(db_err)?;
        let old: Option<String> = sqlx::query_scalar("SELECT config FROM sync_resources WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3")
            .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).fetch_optional(&mut *tx).await.map_err(db_err)?;
        let old = old.map(decode::<SyncConfig>).transpose()?;
        let changed_scope = old.as_ref().is_some_and(|old| old.since != config.since);
        // A new filter starts its own reflection records, so the counts of
        // the old revision never mix with the new ones (ADR-0011).
        let changed_filter = old.as_ref().is_some_and(|old| old.filter != config.filter);
        let resumed = old
            .as_ref()
            .is_some_and(|old| !old.enabled && config.enabled);
        sqlx::query("INSERT INTO sync_resources(workspace_id, connection_id, resource, config, updated_at) VALUES ($1, $2, $3, $4, $5) \
            ON CONFLICT(workspace_id, connection_id, resource) DO UPDATE SET config = excluded.config, revision = sync_resources.revision + 1, cursor_revision = sync_resources.cursor_revision + 1, updated_at = excluded.updated_at")
            .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
            .bind(encode(&config)?).bind(now).execute(&mut *tx).await.map_err(db_err)?;
        if changed_filter {
            sqlx::query("UPDATE sync_resources SET filter_revision = filter_revision + 1 WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).execute(&mut *tx).await.map_err(db_err)?;
        }
        if resumed {
            sqlx::query("UPDATE sync_resources SET caught_up = false WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).execute(&mut *tx).await.map_err(db_err)?;
        }
        if changed_scope {
            sqlx::query("UPDATE sync_resources SET checkpoint = 'null', caught_up = false WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).execute(&mut *tx).await.map_err(db_err)?;
            invalidate(&mut tx, &key).await?;
        }
        tx.commit().await.map_err(db_err)?;
        self.status(&key, now)
            .await?
            .ok_or_else(|| StoreError::Corrupt("sync disappeared".into()))
    }

    async fn status(&self, key: &SourceKey, now: i64) -> Result<Option<SyncStatus>, StoreError> {
        let row = sqlx::query(&format!("SELECT config, revision, cursor_revision, checkpoint, caught_up, updated_at, filter_revision,
            COALESCE((SELECT rebuilding FROM knowledge_generations k WHERE k.workspace_id=r.workspace_id AND k.connection_id=r.connection_id AND k.resource=r.resource),false) AS rebuilding, acquisition_error, arrival_error, \
            (SELECT COUNT(*) FROM source_versions i WHERE i.workspace_id = r.workspace_id AND i.connection_id = r.connection_id AND i.resource = r.resource AND arrival IS NOT NULL AND arrival_ack_at IS NULL) AS arrival_pending, \
            (SELECT COUNT(*) FROM source_versions i WHERE i.workspace_id = r.workspace_id AND i.connection_id = r.connection_id AND i.resource = r.resource AND arrival IS NOT NULL AND arrival_outcome = 'appended') AS arrival_processed, \
            (SELECT COUNT(*) FROM source_versions i WHERE i.workspace_id = r.workspace_id AND i.connection_id = r.connection_id AND i.resource = r.resource AND arrival IS NOT NULL AND arrival_outcome = 'skipped') AS arrival_skipped, \
            (SELECT COUNT(*) FROM source_items i WHERE i.workspace_id = r.workspace_id AND i.connection_id = r.connection_id AND i.resource = r.resource) AS processed, \
            (SELECT COUNT(*) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.filter_revision = r.filter_revision AND p.verdict = 'reflect') AS reflected_pages, \
            (SELECT COUNT(*) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.filter_revision = r.filter_revision AND p.verdict = 'skip') AS skipped_pages, \
            (SELECT COUNT(*) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.filter_revision = r.filter_revision AND p.decided_by = 'backfill' AND p.verdict = 'reflect' AND p.wakeup_id IS NULL AND p.attempts < {BACKFILL_ATTEMPTS_PER_PAGE}) AS backfill_pending, \
            (SELECT COUNT(*) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.filter_revision = r.filter_revision AND p.decided_by = 'backfill' AND p.verdict = 'reflect' AND p.wakeup_id IS NULL AND p.attempts >= {BACKFILL_ATTEMPTS_PER_PAGE}) AS backfill_failed, \
            (SELECT COUNT(*) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.filter_revision = r.filter_revision AND p.decided_by = 'backfill' AND p.wakeup_id IS NOT NULL) AS backfill_reflected, \
            (SELECT COUNT(DISTINCT wakeup_id) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.decided_by = 'backfill' AND p.reflected_at > $1) AS backfill_runs, \
            (SELECT COUNT(*) FROM source_versions i WHERE i.workspace_id = r.workspace_id AND i.connection_id = r.connection_id AND i.resource = r.resource AND i.operation = 'upsert' AND (i.record::jsonb ->> 'metadata') IS NULL) AS incomplete_versions \
            FROM sync_resources r WHERE workspace_id = $2 AND connection_id = $3 AND resource = $4"))
            .bind(now.saturating_sub(BACKFILL_WINDOW_MS))
            .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
            .fetch_optional(&self.pool).await.map_err(db_err)?;
        row.map(|r| {
            Ok(SyncStatus {
                config: decode(r.get("config"))?,
                revision: r.get("revision"),
                cursor_revision: r.get("cursor_revision"),
                checkpoint: decode(r.get("checkpoint"))?,
                caught_up: r.get("caught_up"),
                rebuilding: r.get("rebuilding"),
                processed: r.get("processed"),
                arrival_pending: r.get("arrival_pending"),
                arrival_processed: r.get("arrival_processed"),
                arrival_skipped: r.get("arrival_skipped"),
                filter_revision: r.get("filter_revision"),
                reflected_pages: r.get("reflected_pages"),
                skipped_pages: r.get("skipped_pages"),
                backfill_pending: r.get("backfill_pending"),
                backfill_reflected: r.get("backfill_reflected"),
                backfill_failed: r.get("backfill_failed"),
                backfill_capped: r.get::<i64, _>("backfill_runs") >= BACKFILL_RUNS_PER_WINDOW,
                incomplete_versions: r.get("incomplete_versions"),
                acquisition_error: r.get("acquisition_error"),
                arrival_error: r.get("arrival_error"),
                updated_at: r.get("updated_at"),
            })
        })
        .transpose()
    }

    async fn acquire(
        &self,
        key: &SourceKey,
        batch: &SourceBatch,
        now: i64,
        keys: &dyn pagis_core::ForgetKeys,
    ) -> Result<(), StoreError> {
        if batch
            .changes
            .iter()
            .any(|c| c.id.is_empty() || c.version.is_empty())
        {
            return Err(StoreError::Conflict("invalid source batch".into()));
        }
        let mut tx = begin_write(&self.pool).await.map_err(db_err)?;
        let changed = sqlx::query("UPDATE sync_resources SET checkpoint = $1, caught_up = $2, revision = revision + $3, cursor_revision = cursor_revision + 1, updated_at = $4 \
            WHERE workspace_id = $5 AND connection_id = $6 AND resource = $7 AND cursor_revision = $8 AND config::jsonb -> 'enabled' = 'true'::jsonb")
            .bind(encode(&batch.checkpoint)?).bind(batch.caught_up).bind(i64::from(!batch.changes.is_empty())).bind(now)
            .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(batch.expected_revision)
            .execute(&mut *tx).await.map_err(db_err)?.rows_affected();
        if changed != 1 {
            return Err(StoreError::Conflict("sync changed or paused".into()));
        }
        write_sources(
            &mut tx,
            keys,
            key,
            &batch.changes,
            &batch.arrivals,
            batch.historical,
            now,
        )
        .await?;
        // No worker runs after acquisition, so the batch that writes the
        // sources of a reset also ends its rebuild.
        sqlx::query("UPDATE knowledge_generations SET rebuilding=false WHERE workspace_id=$1 AND connection_id=$2 AND resource=$3")
            .bind(key.workspace_id.as_str())
            .bind(key.connection_id.as_str())
            .bind(&key.resource)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        tx.commit().await.map_err(db_err)
    }
}

/// A reset starts a rebuild of the resource and marks each Source Item
/// `access_lost` until a retrieval finds it again.
async fn invalidate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    key: &SourceKey,
) -> Result<(), StoreError> {
    sqlx::query("INSERT INTO knowledge_generations(workspace_id,connection_id,resource,generation,rebuilding) VALUES($1,$2,$3,1,true) ON CONFLICT(workspace_id,connection_id,resource) DO UPDATE SET generation=knowledge_generations.generation+1,rebuilding=true")
        .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
        .execute(&mut **tx).await.map_err(db_err)?;
    sqlx::query("INSERT INTO knowledge_invalidations(workspace_id,connection_id,resource,reason) VALUES($1,$2,$3,'source_reset')")
        .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
        .execute(&mut **tx).await.map_err(db_err)?;
    sqlx::query("UPDATE source_items SET record = jsonb_set(record::jsonb, '{operation}', '\"access_lost\"'::jsonb)::text WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3")
        .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
        .execute(&mut **tx).await.map_err(db_err)?;
    Ok(())
}

async fn read_versions(
    pool: &PgPool,
    key: &SourceKey,
    after_sequence: i64,
    limit: u32,
    due_arrivals: Option<i64>,
) -> Result<Vec<SourceVersion>, StoreError> {
    let rows = sqlx::query("SELECT sequence, record, observed_at, historical, arrival FROM source_versions WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3 AND sequence > $4 AND ($5 IS NULL OR (arrival IS NOT NULL AND arrival_ack_at IS NULL AND arrival_retry_at <= $6)) ORDER BY sequence LIMIT $7")
        .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(after_sequence).bind(due_arrivals).bind(due_arrivals).bind(i64::from(limit.min(500))).fetch_all(pool).await.map_err(db_err)?;
    rows.iter().map(version_row).collect()
}

fn version_row(r: &sqlx::postgres::PgRow) -> Result<SourceVersion, StoreError> {
    Ok(SourceVersion {
        sequence: r.get("sequence"),
        source: decode(r.get("record"))?,
        observed_at: r.get("observed_at"),
        historical: r.get("historical"),
        arrival: r
            .get::<Option<String>, _>("arrival")
            .map(decode)
            .transpose()?,
    })
}

async fn write_sources(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    keys: &dyn pagis_core::ForgetKeys,
    key: &SourceKey,
    changes: &[SourceChange],
    arrivals: &[pagis_core::NormalizedEvent],
    historical: bool,
    now: i64,
) -> Result<(), StoreError> {
    for change in changes {
        // A Forget blocks a new retrieval of the item. A blocked change
        // writes no row, so no row holds its id or its record, and no
        // pipeline reads the item again (ADR-0008).
        if crate::forget_store::suppressed(tx, keys, key, &change.id).await? {
            continue;
        }
        let arrival = (change.operation == SourceOperation::Upsert)
            .then(|| {
                arrivals
                    .iter()
                    .find(|event| event.provider_event_id == change.id)
            })
            .flatten()
            .map(encode)
            .transpose()?;
        let operation = serde_json::to_value(change.operation)
            .map_err(|_| StoreError::Corrupt("invalid operation".into()))?;
        let inserted = sqlx::query("INSERT INTO source_versions(workspace_id, connection_id, resource, id, version, operation, record, observed_at, historical, arrival) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT DO NOTHING")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(&change.id).bind(&change.version).bind(operation.as_str()).bind(encode(change)?).bind(now).bind(historical).bind(arrival.as_deref())
                .execute(&mut **tx).await.map_err(db_err)?.rows_affected();
        if inserted == 0 && arrival.is_some() {
            sqlx::query("UPDATE source_versions SET arrival = $1 WHERE workspace_id = $2 AND connection_id = $3 AND resource = $4 AND id = $5 AND version = $6 AND operation = $7 AND arrival IS NULL")
                    .bind(arrival).bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(&change.id).bind(&change.version).bind(operation.as_str()).execute(&mut **tx).await.map_err(db_err)?;
        }
        // A version that is already stored writes no new row. It restores
        // the current record only when a reset marked the item
        // `access_lost` or the record has no source time.
        if inserted == 0 {
            let unavailable: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM source_items WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3 AND id = $4 AND ((record::jsonb ->> 'operation') = 'access_lost' OR ((record::jsonb ->> 'source_at') IS NULL AND $5 IS NOT NULL)))")
                    .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(&change.id).bind(change.source_at).fetch_one(&mut **tx).await.map_err(db_err)?;
            if !unavailable {
                continue;
            }
        }
        let first_observed: i64 = sqlx::query_scalar("SELECT observed_at FROM source_versions WHERE workspace_id = $1 AND connection_id = $2 AND resource = $3 AND id = $4 AND version = $5 AND operation = $6")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(&change.id).bind(&change.version).bind(operation.as_str()).fetch_one(&mut **tx).await.map_err(db_err)?;
        sqlx::query("INSERT INTO source_items(workspace_id, connection_id, resource, id, version, record, observed_at) VALUES ($1, $2, $3, $4, $5, $6, $7) \
                ON CONFLICT(workspace_id, connection_id, resource, id) DO UPDATE SET version = excluded.version, record = excluded.record,
                observed_at = excluded.observed_at \
                WHERE source_items.record != excluded.record")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
                .bind(&change.id).bind(&change.version).bind(encode(change)?).bind(first_observed)
                .execute(&mut **tx).await.map_err(db_err)?;
    }
    Ok(())
}
