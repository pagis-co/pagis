use async_trait::async_trait;
use pagis_core::StoreError;
use pagis_core::knowledge::*;
use sqlx::{Row, SqlitePool};

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
pub struct SqliteKnowledgeStore {
    pool: SqlitePool,
}

impl SqliteKnowledgeStore {
    pub fn new(pool: SqlitePool) -> Self {
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
impl KnowledgeStore for SqliteKnowledgeStore {
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
        sqlx::query("UPDATE source_versions SET arrival_ack_at = COALESCE(arrival_ack_at, ?), arrival_outcome = COALESCE(arrival_outcome, 'appended') WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND sequence = ?")
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
        // inside ARRIVAL_RETRY_CAP_MS.
        sqlx::query(&format!(
            "UPDATE source_versions SET arrival_attempts = arrival_attempts + 1, arrival_failure = ?, \
             arrival_retry_at = ? + MIN({ARRIVAL_RETRY_STEP_MS} * (1 << MIN(arrival_attempts + 1, 20)), {ARRIVAL_RETRY_CAP_MS}), \
             arrival_ack_at = CASE WHEN ? OR arrival_attempts + 1 >= {ARRIVAL_ATTEMPT_LIMIT} THEN ? ELSE arrival_ack_at END, \
             arrival_outcome = CASE WHEN ? OR arrival_attempts + 1 >= {ARRIVAL_ATTEMPT_LIMIT} THEN 'skipped' ELSE arrival_outcome END \
             WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND sequence = ? AND arrival_ack_at IS NULL"
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
            "UPDATE sync_resources SET arrival_error = ? WHERE workspace_id = ? AND connection_id = ? AND resource = ?"
        } else {
            "UPDATE sync_resources SET acquisition_error = ? WHERE workspace_id = ? AND connection_id = ? AND resource = ?"
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
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?) \
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
        let rows = sqlx::query(
            "SELECT COALESCE(json_extract(arrival, '$.metadata.thread_id'), json_extract(arrival, '$.provider_event_id')) AS subject, \
             MAX(COALESCE(json_extract(arrival, '$.occurred_at'), observed_at)) AS newest_at \
             FROM source_versions WHERE workspace_id = ? AND connection_id = ? AND resource = ? \
             AND historical = 1 AND arrival IS NOT NULL \
             GROUP BY subject HAVING subject IS NOT NULL \
             AND MAX(json_extract(record, '$.metadata') IS NOT NULL) \
             AND subject NOT IN \
             (SELECT page_subject FROM page_reflections WHERE workspace_id = ? AND connection_id = ? \
              AND resource = ? AND filter_revision = ?) \
             ORDER BY newest_at DESC LIMIT ?",
        )
        .bind(key.workspace_id.as_str())
        .bind(key.connection_id.as_str())
        .bind(&key.resource)
        .bind(key.workspace_id.as_str())
        .bind(key.connection_id.as_str())
        .bind(&key.resource)
        .bind(filter_revision)
        .bind(limit.min(500))
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
             WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND decided_by = 'backfill' \
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
            "SELECT page_path FROM page_reflections WHERE workspace_id = ? AND connection_id = ? \
             AND resource = ? AND filter_revision = ? AND decided_by = 'backfill' \
             AND verdict = 'reflect' AND wakeup_id IS NULL AND attempts < {BACKFILL_ATTEMPTS_PER_PAGE} \
             ORDER BY decided_at, page_path LIMIT ?"
        ))
        .bind(key.workspace_id.as_str())
        .bind(key.connection_id.as_str())
        .bind(&key.resource)
        .bind(filter_revision)
        .bind(limit.min(500))
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
                "UPDATE page_reflections SET wakeup_id = ?, reflected_at = ? \
                 WHERE workspace_id = ? AND connection_id = ? AND resource = ? \
                 AND page_path = ? AND filter_revision = ? AND wakeup_id IS NULL",
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
            WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND operation = 'upsert' \
            AND json_extract(record, '$.metadata') IS NULL ORDER BY sequence LIMIT ?",
        )
        .bind(key.workspace_id.as_str())
        .bind(key.connection_id.as_str())
        .bind(&key.resource)
        .bind(limit.min(500))
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
        let row = sqlx::query("UPDATE source_versions SET record = json_set(record, '$.metadata', json(?)) \
            WHERE sequence = ? AND workspace_id = ? AND connection_id = ? AND resource = ? \
            RETURNING id, version, COALESCE(json_extract(arrival, '$.metadata.thread_id'), json_extract(arrival, '$.provider_event_id')) AS subject")
            .bind(&metadata).bind(sequence).bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
            .fetch_optional(&mut *tx).await.map_err(db_err)?;
        let Some(row) = row else {
            return Err(StoreError::Corrupt("the version does not exist".into()));
        };
        // The current record of the item keeps the same facts.
        sqlx::query("UPDATE source_items SET record = json_set(record, '$.metadata', json(?)) \
            WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND id = ? AND version = ?")
            .bind(&metadata).bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
            .bind(row.get::<String, _>("id")).bind(row.get::<String, _>("version"))
            .execute(&mut *tx).await.map_err(db_err)?;
        if let Some(subject) = row.get::<Option<String>, _>("subject") {
            sqlx::query("DELETE FROM page_reflections WHERE workspace_id = ? AND connection_id = ? AND resource = ? \
                AND page_subject = ? AND decided_by = 'backfill' AND wakeup_id IS NULL")
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
            "SELECT json_extract(v.record, '$.metadata') FROM source_versions v \
            WHERE v.workspace_id = ? AND v.connection_id = ? AND v.resource = ? AND json_extract(v.record, '$.parent') = ? \
            AND json_extract(v.record, '$.metadata') IS NOT NULL AND {NEWEST_VERSION} ORDER BY v.sequence"
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
            "SELECT json_extract(v.record, '$.parent'), json_extract(v.record, '$.metadata') \
            FROM source_versions v WHERE v.workspace_id = ? AND v.connection_id = ? AND v.resource = ? \
            AND json_extract(v.record, '$.parent') IS NOT NULL AND json_extract(v.record, '$.metadata') IS NOT NULL \
            AND {NEWEST_VERSION} ORDER BY json_extract(v.record, '$.parent'), v.sequence"
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
            "SELECT COUNT(*) FROM (SELECT json_extract(v.record, '$.parent') AS parent FROM source_versions v \
             WHERE v.workspace_id = ? AND v.connection_id = ? AND v.resource = ? \
             AND json_extract(v.record, '$.metadata.{field}') = ? AND json_extract(v.record, '$.parent') IS NOT NULL \
             AND {NEWEST_VERSION} \
             GROUP BY parent HAVING MAX(COALESCE(json_extract(v.record, '$.source_at'), 0)) >= ?)"
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
        let changed = sqlx::query("UPDATE sync_resources SET checkpoint = 'null', caught_up = 0, revision = revision + 1, cursor_revision = cursor_revision + 1, updated_at = ? WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND cursor_revision = ?")
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
        let old: Option<String> = sqlx::query_scalar("SELECT config FROM sync_resources WHERE workspace_id = ? AND connection_id = ? AND resource = ?")
            .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).fetch_optional(&mut *tx).await.map_err(db_err)?;
        let old = old.map(decode::<SyncConfig>).transpose()?;
        let changed_scope = old.as_ref().is_some_and(|old| old.since != config.since);
        // A new filter starts its own reflection records, so the counts of
        // the old revision never mix with the new ones (ADR-0011).
        let changed_filter = old.as_ref().is_some_and(|old| old.filter != config.filter);
        let resumed = old
            .as_ref()
            .is_some_and(|old| !old.enabled && config.enabled);
        sqlx::query("INSERT INTO sync_resources(workspace_id, connection_id, resource, config, updated_at) VALUES (?, ?, ?, ?, ?) \
            ON CONFLICT(workspace_id, connection_id, resource) DO UPDATE SET config = excluded.config, revision = revision + 1, cursor_revision = cursor_revision + 1, updated_at = excluded.updated_at")
            .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
            .bind(encode(&config)?).bind(now).execute(&mut *tx).await.map_err(db_err)?;
        if changed_filter {
            sqlx::query("UPDATE sync_resources SET filter_revision = filter_revision + 1 WHERE workspace_id = ? AND connection_id = ? AND resource = ?")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).execute(&mut *tx).await.map_err(db_err)?;
        }
        if resumed {
            sqlx::query("UPDATE sync_resources SET caught_up = 0 WHERE workspace_id = ? AND connection_id = ? AND resource = ?")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).execute(&mut *tx).await.map_err(db_err)?;
        }
        if changed_scope {
            sqlx::query("UPDATE sync_resources SET checkpoint = 'null', caught_up = 0 WHERE workspace_id = ? AND connection_id = ? AND resource = ?")
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
            COALESCE((SELECT rebuilding FROM knowledge_generations k WHERE k.workspace_id=r.workspace_id AND k.connection_id=r.connection_id AND k.resource=r.resource),0) AS rebuilding, acquisition_error, arrival_error, \
            (SELECT COUNT(*) FROM source_versions i WHERE i.workspace_id = r.workspace_id AND i.connection_id = r.connection_id AND i.resource = r.resource AND arrival IS NOT NULL AND arrival_ack_at IS NULL) AS arrival_pending, \
            (SELECT COUNT(*) FROM source_versions i WHERE i.workspace_id = r.workspace_id AND i.connection_id = r.connection_id AND i.resource = r.resource AND arrival IS NOT NULL AND arrival_outcome = 'appended') AS arrival_processed, \
            (SELECT COUNT(*) FROM source_versions i WHERE i.workspace_id = r.workspace_id AND i.connection_id = r.connection_id AND i.resource = r.resource AND arrival IS NOT NULL AND arrival_outcome = 'skipped') AS arrival_skipped, \
            (SELECT COUNT(*) FROM source_items i WHERE i.workspace_id = r.workspace_id AND i.connection_id = r.connection_id AND i.resource = r.resource) AS processed, \
            (SELECT COUNT(*) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.filter_revision = r.filter_revision AND p.verdict = 'reflect') AS reflected_pages, \
            (SELECT COUNT(*) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.filter_revision = r.filter_revision AND p.verdict = 'skip') AS skipped_pages, \
            (SELECT COUNT(*) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.filter_revision = r.filter_revision AND p.decided_by = 'backfill' AND p.verdict = 'reflect' AND p.wakeup_id IS NULL AND p.attempts < {BACKFILL_ATTEMPTS_PER_PAGE}) AS backfill_pending, \
            (SELECT COUNT(*) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.filter_revision = r.filter_revision AND p.decided_by = 'backfill' AND p.verdict = 'reflect' AND p.wakeup_id IS NULL AND p.attempts >= {BACKFILL_ATTEMPTS_PER_PAGE}) AS backfill_failed, \
            (SELECT COUNT(*) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.filter_revision = r.filter_revision AND p.decided_by = 'backfill' AND p.wakeup_id IS NOT NULL) AS backfill_reflected, \
            (SELECT COUNT(DISTINCT wakeup_id) FROM page_reflections p WHERE p.workspace_id = r.workspace_id AND p.connection_id = r.connection_id AND p.resource = r.resource AND p.decided_by = 'backfill' AND p.reflected_at > ?) AS backfill_runs, \
            (SELECT COUNT(*) FROM source_versions i WHERE i.workspace_id = r.workspace_id AND i.connection_id = r.connection_id AND i.resource = r.resource AND i.operation = 'upsert' AND json_extract(i.record, '$.metadata') IS NULL) AS incomplete_versions \
            FROM sync_resources r WHERE workspace_id = ? AND connection_id = ? AND resource = ?"))
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
        let changed = sqlx::query("UPDATE sync_resources SET checkpoint = ?, caught_up = ?, revision = revision + ?, cursor_revision = cursor_revision + 1, updated_at = ? \
            WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND cursor_revision = ? AND json_extract(config, '$.enabled') = 1")
            .bind(encode(&batch.checkpoint)?).bind(batch.caught_up).bind(!batch.changes.is_empty()).bind(now)
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
        sqlx::query("UPDATE knowledge_generations SET rebuilding=0 WHERE workspace_id=? AND connection_id=? AND resource=?")
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
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    key: &SourceKey,
) -> Result<(), StoreError> {
    sqlx::query("INSERT INTO knowledge_generations(workspace_id,connection_id,resource,generation,rebuilding) VALUES(?,?,?,1,1) ON CONFLICT(workspace_id,connection_id,resource) DO UPDATE SET generation=generation+1,rebuilding=1")
        .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
        .execute(&mut **tx).await.map_err(db_err)?;
    sqlx::query("INSERT INTO knowledge_invalidations(workspace_id,connection_id,resource,reason) VALUES(?,?,?,'source_reset')")
        .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
        .execute(&mut **tx).await.map_err(db_err)?;
    sqlx::query("UPDATE source_items SET record = json_set(record, '$.operation', 'access_lost') WHERE workspace_id = ? AND connection_id = ? AND resource = ?")
        .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
        .execute(&mut **tx).await.map_err(db_err)?;
    Ok(())
}

async fn read_versions(
    pool: &SqlitePool,
    key: &SourceKey,
    after_sequence: i64,
    limit: u32,
    due_arrivals: Option<i64>,
) -> Result<Vec<SourceVersion>, StoreError> {
    let rows = sqlx::query("SELECT sequence, record, observed_at, historical, arrival FROM source_versions WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND sequence > ? AND (? IS NULL OR (arrival IS NOT NULL AND arrival_ack_at IS NULL AND arrival_retry_at <= ?)) ORDER BY sequence LIMIT ?")
        .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(after_sequence).bind(due_arrivals).bind(due_arrivals).bind(limit.min(500)).fetch_all(pool).await.map_err(db_err)?;
    rows.iter().map(version_row).collect()
}

fn version_row(r: &sqlx::sqlite::SqliteRow) -> Result<SourceVersion, StoreError> {
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
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
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
        let inserted = sqlx::query("INSERT OR IGNORE INTO source_versions(workspace_id, connection_id, resource, id, version, operation, record, observed_at, historical, arrival) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(&change.id).bind(&change.version).bind(operation.as_str()).bind(encode(change)?).bind(now).bind(historical).bind(arrival.as_deref())
                .execute(&mut **tx).await.map_err(db_err)?.rows_affected();
        if inserted == 0 && arrival.is_some() {
            sqlx::query("UPDATE source_versions SET arrival = ? WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND id = ? AND version = ? AND operation = ? AND arrival IS NULL")
                    .bind(arrival).bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(&change.id).bind(&change.version).bind(operation.as_str()).execute(&mut **tx).await.map_err(db_err)?;
        }
        // A version that is already stored writes no new row. It restores
        // the current record only when a reset marked the item
        // `access_lost` or the record has no source time.
        if inserted == 0 {
            let unavailable: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM source_items WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND id = ? AND (json_extract(record, '$.operation') = 'access_lost' OR (json_extract(record, '$.source_at') IS NULL AND ? IS NOT NULL)))")
                    .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(&change.id).bind(change.source_at).fetch_one(&mut **tx).await.map_err(db_err)?;
            if !unavailable {
                continue;
            }
        }
        let first_observed: i64 = sqlx::query_scalar("SELECT observed_at FROM source_versions WHERE workspace_id = ? AND connection_id = ? AND resource = ? AND id = ? AND version = ? AND operation = ?")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource).bind(&change.id).bind(&change.version).bind(operation.as_str()).fetch_one(&mut **tx).await.map_err(db_err)?;
        sqlx::query("INSERT INTO source_items(workspace_id, connection_id, resource, id, version, record, observed_at) VALUES (?, ?, ?, ?, ?, ?, ?) \
                ON CONFLICT(workspace_id, connection_id, resource, id) DO UPDATE SET version = excluded.version, record = excluded.record,
                observed_at = excluded.observed_at \
                WHERE source_items.record != excluded.record")
                .bind(key.workspace_id.as_str()).bind(key.connection_id.as_str()).bind(&key.resource)
                .bind(&change.id).bind(&change.version).bind(encode(change)?).bind(first_observed)
                .execute(&mut **tx).await.map_err(db_err)?;
    }
    Ok(())
}
