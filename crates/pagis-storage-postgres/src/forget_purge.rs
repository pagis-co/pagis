//! Transactional blocking precedes destructive work; each completed phase is durable.
use crate::{begin_write, db_err, forget_plan::Plan, forget_store::encode};
use pagis_core::*;
use sqlx::{PgConnection, PgPool, Row};

pub(crate) async fn begin(
    pool: &PgPool,
    workspace: &WorkspaceId,
    target: &ForgetTarget,
    expected: &str,
    now: i64,
    keys: &dyn ForgetKeys,
) -> Result<ForgetOperation, StoreError> {
    // The key comes before the write transaction. The first use of a
    // Tenant Data Key reads `secrets.enc`, and no transaction waits on
    // that read.
    let suppression = keys.suppression_key(workspace)?;
    let mut tx = begin_write(pool).await.map_err(db_err)?;
    let plan = crate::forget_plan::load(&mut tx, workspace, target).await?;
    if plan.preview()?.revision != expected {
        return Err(StoreError::Conflict(
            "forget preview changed; review it again".into(),
        ));
    }
    let operation = ForgetOperation {
        connection_id: plan.connection.clone(),
        id: RunId::generate().to_string(),
        phase: ForgetPhase::Blocked,
        created_at: now,
        error: None,
        reopted_at: None,
    };
    sqlx::query("INSERT INTO forget_operations(id,workspace_id,target,phase,created_at,purge_plan,connection_id) VALUES($1,$2,$3,'blocked',$4,$5,$6)")
        .bind(&operation.id).bind(workspace.as_str()).bind(encode(target)?).bind(now).bind(encode(&plan)?).bind(plan.connection.as_str()).execute(&mut *tx).await.map_err(db_err)?;
    if plan.account {
        sqlx::query("INSERT INTO forget_suppressions(operation_id,workspace_id,connection_id,resource,identity,scope) VALUES($1,$2,$3,'','account','account')")
            .bind(&operation.id).bind(workspace.as_str()).bind(plan.connection.as_str()).execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query(
            "INSERT INTO forgotten_messages \
             SELECT DISTINCT e.message_id FROM message_exposures e \
             JOIN messages m ON m.id=e.message_id \
             LEFT JOIN grants g ON g.workspace_id=m.workspace_id AND g.id=e.grant_id \
             WHERE m.workspace_id=$1 AND (g.id IS NULL OR \
                 (g.resource_kind='connection' AND g.resource_id=$2)) \
             ON CONFLICT DO NOTHING",
        )
        .bind(workspace.as_str())
        .bind(plan.connection.as_str())
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    for (source, id) in &plan.sources {
        sqlx::query("INSERT INTO forget_suppressions(operation_id,workspace_id,connection_id,resource,identity) VALUES($1,$2,$3,$4,$5)")
            .bind(&operation.id).bind(workspace.as_str()).bind(source.connection_id.as_str()).bind(&source.resource).bind(suppression.identity(source,id)).execute(&mut *tx).await.map_err(db_err)?;
    }
    for schedule in &plan.schedules {
        sqlx::query("UPDATE schedules SET state='archived',forgotten=true,archived_at=$1,updated_at=$2 WHERE workspace_id=$3 AND id=$4")
            .bind(now).bind(now).bind(workspace.as_str()).bind(schedule).execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("UPDATE wakeups SET state='withdrawn' WHERE workspace_id=$1 AND schedule_id=$2 AND state='pending'")
            .bind(workspace.as_str()).bind(schedule).execute(&mut *tx).await.map_err(db_err)?;
    }
    forget_run_output(&mut tx, workspace, &plan).await?;
    // A pending review is derived from its exact source messages. Once
    // Forget blocks any source, erase the derived reason and dependency
    // rows in the same transaction so no worker can claim stale prose.
    sqlx::query(
        "UPDATE pending_evidence SET state='invalidated',reason='Forgotten source.',error='source forgotten',completed_at=$1,lease_expires_at=NULL,revision=revision+1  \
         WHERE workspace_id=$2 AND state IN ('pending','leased','failed') AND id IN (\
           SELECT pem.pending_id FROM pending_evidence_messages pem JOIN forgotten_messages fm ON fm.message_id=pem.message_id\
         )",
    )
    .bind(now)
    .bind(workspace.as_str())
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    sqlx::query(
        "DELETE FROM pending_evidence_messages WHERE pending_id IN (SELECT id FROM pending_evidence WHERE workspace_id=$1 AND state='invalidated')",
    )
    .bind(workspace.as_str())
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    sqlx::query(
        "DELETE FROM pending_evidence_exposures WHERE pending_id IN (SELECT id FROM pending_evidence WHERE workspace_id=$1 AND state='invalidated')",
    )
    .bind(workspace.as_str())
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    sqlx::query("INSERT INTO knowledge_invalidations(workspace_id,connection_id,resource,reason) VALUES($1,$2,'','forgotten')")
        .bind(workspace.as_str()).bind(plan.connection.as_str()).execute(&mut *tx).await.map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(operation)
}

pub(crate) async fn purge(
    pool: &PgPool,
    workspace: &WorkspaceId,
    id: &str,
) -> Result<(), StoreError> {
    let mut tx = begin_write(pool).await.map_err(db_err)?;
    let row = sqlx::query(
        "SELECT phase,error,purge_plan FROM forget_operations WHERE workspace_id=$1 AND id=$2",
    )
    .bind(workspace.as_str())
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?
    .ok_or_else(|| StoreError::Conflict("forget operation unavailable".into()))?;
    if row.get::<&str, _>("phase") != "blocked" {
        let compact_needed = row.get::<&str, _>("phase") == "structured_purged";
        tx.rollback().await.map_err(db_err)?;
        return if compact_needed {
            compact(pool, workspace, id).await
        } else {
            Ok(())
        };
    }
    if row.get::<Option<String>, _>("error").is_some() {
        return Err(StoreError::Conflict("retry the failed purge first".into()));
    }
    let plan: Plan = serde_json::from_str(row.get("purge_plan"))
        .map_err(|_| StoreError::Corrupt("invalid purge plan".into()))?;
    // After the purge the suppression row is the only structured record
    // of a forgotten source (ADR-0008). The purge deletes each row that
    // holds its id or its record: the current record, every version with
    // its arrival, and the Incoming Event that a delivery stored.
    for (source, source_id) in &plan.sources {
        for table in ["source_items", "source_versions"] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE workspace_id=$1 AND connection_id=$2 AND resource=$3 AND id=$4"
            ))
            .bind(workspace.as_str())
            .bind(source.connection_id.as_str())
            .bind(&source.resource)
            .bind(source_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        sqlx::query(
            "DELETE FROM wakeup_sources WHERE workspace_id=$1 AND source_kind='incoming_event' \
             AND source_id IN (SELECT id FROM incoming_events \
             WHERE workspace_id=$1 AND connection_id=$2 AND provider_event_id=$3)",
        )
        .bind(workspace.as_str())
        .bind(source.connection_id.as_str())
        .bind(source_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "DELETE FROM incoming_events WHERE workspace_id=$1 AND connection_id=$2 AND provider_event_id=$3",
        )
        .bind(workspace.as_str())
        .bind(source.connection_id.as_str())
        .bind(source_id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    // The name of the Subject Page of a forgotten source holds its
    // thread, which is a field of its record. The purge deletes the
    // decisions of the filter about the page, and takes the page out of
    // each arrival Wake-up.
    for page in &plan.pages {
        sqlx::query(
            "DELETE FROM page_reflections WHERE workspace_id=$1 AND connection_id=$2 AND page_path=$3",
        )
        .bind(workspace.as_str())
        .bind(plan.connection.as_str())
        .bind(page)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    forget_wakeup_pages(&mut tx, workspace, &plan.pages).await?;
    for schedule in &plan.schedules {
        sqlx::query("UPDATE schedules SET name='Forgotten schedule',instruction='' WHERE workspace_id=$1 AND id=$2")
            .bind(workspace.as_str()).bind(schedule).execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("UPDATE schedule_revisions SET name='Forgotten schedule',instruction='' WHERE schedule_id=$1")
            .bind(schedule).execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("UPDATE wakeups SET rule_name='Forgotten schedule',instruction='' WHERE workspace_id=$1 AND schedule_id=$2")
            .bind(workspace.as_str()).bind(schedule).execute(&mut *tx).await.map_err(db_err)?;
        sqlx::query("DELETE FROM schedule_subject_pages WHERE schedule_id=$1")
            .bind(schedule)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
    }
    erase_forgotten_messages(&mut tx, workspace).await?;
    sqlx::query("UPDATE forget_operations SET phase='structured_purged',purge_plan=NULL,target='{}' WHERE workspace_id=$1 AND id=$2")
        .bind(workspace.as_str()).bind(id).execute(&mut *tx).await.map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    compact(pool, workspace, id).await
}

/// Forget what each Run that read a forgotten source item wrote: each
/// message of the Run that can hold what the Run read, each tool result
/// that the Run kept, and each Model Request Capture of the Run. A message is forgotten as a whole
/// (ADR-0008). An account Forget also deletes each tool result that a
/// Grant of its Connection read.
async fn forget_run_output(
    tx: &mut PgConnection,
    workspace: &WorkspaceId,
    plan: &Plan,
) -> Result<(), StoreError> {
    // A read that a Run records takes this row for share and checks for
    // a suppression row. The lock orders that check against this
    // transaction, so a read that lands first is found below, and a read
    // that comes later sees the suppression and is refused.
    sqlx::query("SELECT 1 FROM connections WHERE workspace_id=$1 AND id=$2 FOR UPDATE")
        .bind(workspace.as_str())
        .bind(plan.connection.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
    if plan.account {
        sqlx::query(
            "DELETE FROM conversation_tool_evidence WHERE workspace_id=$1 AND grant_id IN (\
             SELECT id FROM grants WHERE workspace_id=$1 AND resource_kind='connection' AND resource_id=$2)",
        )
        .bind(workspace.as_str())
        .bind(plan.connection.as_str())
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    let runs = runs_that_read(tx, workspace, plan).await?;
    for run in &runs {
        // A stamp that names no Grant is prose of no source: the words
        // of the Person, the progress line of the Run and a notice of
        // the daemon (ADR-0004). Each other message of the Run can hold
        // what the Run read.
        sqlx::query(
            "INSERT INTO forgotten_messages \
             SELECT m.id FROM messages m WHERE m.workspace_id=$1 AND m.run_id=$2 \
             AND (EXISTS(SELECT 1 FROM message_exposures e WHERE e.message_id=m.id) \
                  OR NOT EXISTS(SELECT 1 FROM message_exposure_sets s WHERE s.message_id=m.id)) \
             ON CONFLICT DO NOTHING",
        )
        .bind(workspace.as_str())
        .bind(run)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        sqlx::query("DELETE FROM conversation_tool_evidence WHERE workspace_id=$1 AND run_id=$2")
            .bind(workspace.as_str())
            .bind(run)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        // A Model Request Capture of the Run holds what the Run read
        // (ADR-0031).
        sqlx::query("DELETE FROM model_request_captures WHERE workspace_id=$1 AND run_id=$2")
            .bind(workspace.as_str())
            .bind(run)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
    }
    Ok(())
}

/// The Runs that read a forgotten source item. A Run read an item when
/// a call of the Run named the item or its parent, or when the Incoming
/// Event of the item is a source of the Wake-up of the Run. An account
/// Forget reaches each Run that read through its Connection.
///
/// The record of a read names the source id, so this also deletes each
/// record that names a forgotten item or its parent.
async fn runs_that_read(
    tx: &mut PgConnection,
    workspace: &WorkspaceId,
    plan: &Plan,
) -> Result<std::collections::BTreeSet<String>, StoreError> {
    let mut runs = std::collections::BTreeSet::<String>::new();
    if plan.account {
        runs.extend(
            sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT run_id FROM run_source_reads WHERE workspace_id=$1 AND connection_id=$2",
            )
            .bind(workspace.as_str())
            .bind(plan.connection.as_str())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?,
        );
        runs.extend(
            sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT w.run_id FROM incoming_events e \
                 JOIN wakeup_sources s ON s.source_kind='incoming_event' AND s.source_id=e.id \
                 JOIN wakeups w ON w.id=s.wakeup_id \
                 WHERE e.workspace_id=$1 AND e.connection_id=$2 AND w.run_id IS NOT NULL",
            )
            .bind(workspace.as_str())
            .bind(plan.connection.as_str())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?,
        );
        sqlx::query("DELETE FROM run_source_reads WHERE workspace_id=$1 AND connection_id=$2")
            .bind(workspace.as_str())
            .bind(plan.connection.as_str())
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        return Ok(runs);
    }
    for (source, source_id) in &plan.sources {
        let record: Option<String> = sqlx::query_scalar(
            "SELECT record FROM source_items WHERE workspace_id=$1 AND connection_id=$2 AND resource=$3 AND id=$4",
        )
        .bind(workspace.as_str())
        .bind(source.connection_id.as_str())
        .bind(&source.resource)
        .bind(source_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        let parent = record
            .and_then(|record| serde_json::from_str::<serde_json::Value>(&record).ok())
            .and_then(|record| record["parent"].as_str().map(str::to_string));
        runs.extend(
            sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT run_id FROM run_source_reads \
                 WHERE workspace_id=$1 AND connection_id=$2 AND resource=$3 \
                 AND ((kind='item' AND source_id=$4) OR (kind='parent' AND source_id=$5))",
            )
            .bind(workspace.as_str())
            .bind(source.connection_id.as_str())
            .bind(&source.resource)
            .bind(source_id)
            .bind(&parent)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?,
        );
        runs.extend(
            sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT w.run_id FROM incoming_events e \
                 JOIN wakeup_sources s ON s.source_kind='incoming_event' AND s.source_id=e.id \
                 JOIN wakeups w ON w.id=s.wakeup_id \
                 WHERE e.workspace_id=$1 AND e.connection_id=$2 AND e.provider_event_id=$3 \
                 AND w.run_id IS NOT NULL",
            )
            .bind(workspace.as_str())
            .bind(source.connection_id.as_str())
            .bind(source_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?,
        );
        sqlx::query(
            "DELETE FROM run_source_reads WHERE workspace_id=$1 AND connection_id=$2 AND resource=$3 \
             AND ((kind='item' AND source_id=$4) OR (kind='parent' AND source_id=$5))",
        )
        .bind(workspace.as_str())
        .bind(source.connection_id.as_str())
        .bind(&source.resource)
        .bind(source_id)
        .bind(&parent)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    Ok(runs)
}

/// Erase the words of each forgotten message of the Workspace. The row
/// stays, so the conversation keeps its order, and the Person reads
/// that the message is unavailable (ADR-0004, ADR-0008).
async fn erase_forgotten_messages(
    tx: &mut PgConnection,
    workspace: &WorkspaceId,
) -> Result<(), StoreError> {
    sqlx::query(
        "UPDATE messages SET text_content='', blocks='[]' WHERE workspace_id=$1 \
         AND id IN (SELECT message_id FROM forgotten_messages) \
         AND (text_content<>'' OR blocks<>'[]')",
    )
    .bind(workspace.as_str())
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Take the forgotten Subject Pages out of each arrival Wake-up that
/// names one. The purge withdraws a pending Wake-up that keeps no page,
/// so no Run starts for it. `pages` is in order.
async fn forget_wakeup_pages(
    tx: &mut PgConnection,
    workspace: &WorkspaceId,
    pages: &[String],
) -> Result<(), StoreError> {
    if pages.is_empty() {
        return Ok(());
    }
    let wakeups = sqlx::query(
        "SELECT id,subject_paths FROM wakeups WHERE workspace_id=$1 AND source_kind='arrival'",
    )
    .bind(workspace.as_str())
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    for wakeup in wakeups {
        let paths: Vec<String> = serde_json::from_str(wakeup.get("subject_paths"))
            .map_err(|_| StoreError::Corrupt("invalid wakeup subject paths".into()))?;
        let kept: Vec<&String> = paths
            .iter()
            .filter(|path| pages.binary_search(path).is_err())
            .collect();
        if kept.len() == paths.len() {
            continue;
        }
        sqlx::query(
            "UPDATE wakeups SET subject_paths=$1, \
             state=CASE WHEN $2 AND state='pending' THEN 'withdrawn' ELSE state END WHERE id=$3",
        )
        .bind(encode(&kept)?)
        .bind(kept.is_empty())
        .bind(wakeup.get::<&str, _>("id"))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

/// The tables the purge deletes prose from. `VACUUM FULL` takes each
/// one to itself, so the list is the smallest that erases what the
/// purge removed: a whole-database sweep would lock out every other
/// person of a server for as long as it runs.
const PURGED_TABLES: &str = "source_items, source_versions, incoming_events, wakeup_sources, \
                             page_reflections, schedules, schedule_revisions, \
                             schedule_subject_pages, wakeups, forget_operations, pending_evidence, \
                             pending_evidence_messages, pending_evidence_exposures, messages, \
                             conversation_message_search, conversation_tool_evidence, \
                             conversation_tool_search, continuation_checkpoints, run_source_reads";

async fn compact(pool: &PgPool, workspace: &WorkspaceId, id: &str) -> Result<(), StoreError> {
    vacuum_full(pool, PURGED_TABLES).await?;
    sqlx::query("UPDATE forget_operations SET structured_cleaned=true WHERE workspace_id=$1 AND id=$2 AND phase='structured_purged'")
        .bind(workspace.as_str()).bind(id).execute(pool).await.map_err(db_err)?;
    Ok(())
}

/// Write each table of `tables`, a comma-separated list, to a new file.
pub(crate) async fn vacuum_full(pool: &PgPool, tables: &str) -> Result<(), StoreError> {
    // Postgres has no `secure_delete`: a plain `VACUUM` marks the space
    // of a deleted row reusable and leaves the prose in the table file.
    // `VACUUM FULL` writes the table to a new file and unlinks the old
    // one, so it both overwrites and returns the space.
    //
    // `VACUUM` is a utility statement, so it goes through the simple
    // query protocol. It takes each table it names to itself, so a
    // reader that outlasts the pool's `lock_timeout` makes it fail.
    // That is a failed phase the owner retries, never a completed
    // erasure, which is what a busy `VACUUM` is on the other backend.
    let sweep = format!("VACUUM FULL {tables}");
    if let Err(error) = sqlx::raw_sql(&sweep).execute(pool).await {
        return Err(if lock_timeout(&error) {
            StoreError::Conflict("database readers delayed local erasure".into())
        } else {
            db_err(error)
        });
    }
    Ok(())
}

/// Whether a statement gave up the wait for a lock another transaction
/// holds: SQLSTATE 55P03, which `lock_timeout` raises.
fn lock_timeout(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(error) if error.code().as_deref() == Some("55P03"))
}
