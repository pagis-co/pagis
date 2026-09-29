//! What a Forget removes after its memory purge.
//!
//! The memory purge rebuilds the history without the forgotten content
//! and brings the Page Index to the rebuilt history. Two records still
//! name a purged page then: the Learning Feed entries in the event log
//! and the cursors of the Briefs. The step that records the end of the
//! memory purge clears both, and the last step erases the purged rows
//! from the database files (ADR-0008).
use crate::{begin_write, db_err, forget_store::encode};
use pagis_core::*;
use sqlx::{Row, SqliteConnection, SqlitePool};
use std::collections::{HashMap, HashSet};

/// The keys of a Learning Feed entry that hold a field of a record: the
/// files and the titles of a change, the sentence of a commit, and the
/// name, the instruction and the Subject Page of a Schedule.
const RECORD_KEYS: [&str; 6] = [
    "files",
    "titles",
    "message",
    "name",
    "instruction",
    "subject_page_path",
];

/// Record the end of the memory purge. The same transaction clears the
/// records that name what the purge removed, so the phase and the
/// records change together.
pub(crate) async fn memory_purged(
    pool: &SqlitePool,
    workspace: &WorkspaceId,
    id: &str,
) -> Result<(), StoreError> {
    let mut tx = begin_write(pool).await.map_err(db_err)?;
    let phase: Option<String> = sqlx::query_scalar(
        "SELECT phase FROM forget_operations WHERE workspace_id=? AND id=? \
         AND error IS NULL AND structured_cleaned=1",
    )
    .bind(workspace.as_str())
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?;
    match phase.as_deref() {
        Some("structured_purged") => {}
        Some("memory_purged" | "complete") => return Ok(()),
        _ => {
            return Err(StoreError::Conflict(
                "previous forget phase has not completed".into(),
            ));
        }
    }
    forget_feed_entries(&mut tx, workspace).await?;
    forget_brief_paths(&mut tx, workspace).await?;
    sqlx::query("UPDATE forget_operations SET phase='memory_purged' WHERE workspace_id=? AND id=?")
        .bind(workspace.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)
}

/// Erase from the database files what the memory purge and
/// [`memory_purged`] removed, then complete the operation. The memory
/// purge deleted the Memory Search rows of the purged pages after the
/// structured compaction ran, so the WAL can hold their old pages. The
/// checkpoint writes the pages without them over the database file and
/// empties the WAL. A failed checkpoint leaves the phase, and a retry
/// runs this step again.
pub(crate) async fn complete(
    pool: &SqlitePool,
    workspace: &WorkspaceId,
    id: &str,
) -> Result<(), StoreError> {
    let phase: Option<String> = sqlx::query_scalar(
        "SELECT phase FROM forget_operations WHERE workspace_id=? AND id=? AND error IS NULL",
    )
    .bind(workspace.as_str())
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    if phase.as_deref() == Some("memory_purged") {
        let mut connection = pool.acquire().await.map_err(db_err)?;
        crate::forget_purge::truncate_wal(&mut connection).await?;
    }
    crate::forget_journal::advance(pool, workspace, id, "memory_purged", "complete").await
}

/// Clear each Learning Feed entry whose change a Forget took out of the
/// history. The memory purge takes out each commit whose exposures a
/// Forget does not permit: its files and its sentence. A revert of such
/// a commit goes with it, and so does the entry of a Schedule that a
/// Forget archived. An entry keeps its place in the log, so the
/// sequence that the event bus reads does not change, and the feed does
/// not show an entry that names no file.
async fn forget_feed_entries(
    tx: &mut SqliteConnection,
    workspace: &WorkspaceId,
) -> Result<(), StoreError> {
    let schedules: HashSet<String> =
        sqlx::query_scalar("SELECT id FROM schedules WHERE workspace_id=? AND forgotten=1")
            .bind(workspace.as_str())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?
            .into_iter()
            .collect();
    let entries = sqlx::query(
        "SELECT id,event_type,payload FROM events WHERE workspace_id=? \
         AND event_type IN ('memory.committed','memory.reverted','schedule.created') ORDER BY rowid",
    )
    .bind(workspace.as_str())
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    // The commits that the history no longer holds. A revert comes
    // after the commit it undoes, so the log order fills this first.
    let mut commits = HashSet::new();
    // Many commits share one set of exposures, so each set asks once.
    let mut permitted = HashMap::new();
    for entry in entries {
        let mut payload: serde_json::Value = serde_json::from_str(entry.get("payload"))
            .map_err(|_| StoreError::Corrupt("invalid event payload".into()))?;
        let forgotten = match entry.get::<&str, _>("event_type") {
            "memory.committed" => {
                // The memory purge keeps a commit whose exposures it
                // cannot read, so this step keeps its entry too.
                let exposures: Vec<MemoryExposure> =
                    serde_json::from_value(payload["exposures"].clone()).unwrap_or_default();
                let known = encode(&exposures)?;
                let allowed = match permitted.get(&known) {
                    Some(allowed) => *allowed,
                    None => {
                        let allowed = crate::forget_store::permits_provenance(
                            &mut *tx, workspace, &exposures,
                        )
                        .await?;
                        permitted.insert(known, allowed);
                        allowed
                    }
                };
                !allowed
            }
            "memory.reverted" => payload["reverted_sha"]
                .as_str()
                .is_some_and(|sha| commits.contains(sha)),
            _ => payload["schedule_id"]
                .as_str()
                .is_some_and(|schedule| schedules.contains(schedule)),
        };
        if !forgotten {
            continue;
        }
        if let Some(sha) = payload["sha"].as_str() {
            commits.insert(sha.to_string());
        }
        let Some(fields) = payload.as_object_mut() else {
            continue;
        };
        let held = fields.len();
        for key in RECORD_KEYS {
            fields.remove(key);
        }
        if fields.len() == held {
            continue;
        }
        sqlx::query("UPDATE events SET payload=? WHERE id=?")
            .bind(encode(&payload)?)
            .bind(entry.get::<&str, _>("id"))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
    }
    Ok(())
}

/// Keep in each Brief cursor of the Workspace only the pages that the
/// Page Index holds. The memory purge brought the index to the rebuilt
/// history, so no purged page is in it. A page that memory does not
/// hold cannot be shown, so a cursor that forgets it loses nothing.
async fn forget_brief_paths(
    tx: &mut SqliteConnection,
    workspace: &WorkspaceId,
) -> Result<(), StoreError> {
    let held: HashSet<String> =
        sqlx::query_scalar("SELECT path FROM memory_page_index WHERE workspace_id=?")
            .bind(workspace.as_str())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?
            .into_iter()
            .collect();
    let cursors = sqlx::query(
        "SELECT agent_id,channel_id,root_message_id,shown_paths FROM memory_brief_cursors \
         WHERE workspace_id=?",
    )
    .bind(workspace.as_str())
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    for cursor in cursors {
        let agent = AgentId::from(cursor.get::<String, _>("agent_id"));
        let shown: Vec<String> = serde_json::from_str(cursor.get("shown_paths"))
            .map_err(|_| StoreError::Corrupt("invalid Brief cursor".into()))?;
        let kept: Vec<&String> = shown
            .iter()
            .filter(|path| {
                ScopedPath::parse(path).is_ok_and(|path| held.contains(&path.repo_path(&agent)))
            })
            .collect();
        if kept.len() == shown.len() {
            continue;
        }
        sqlx::query(
            "UPDATE memory_brief_cursors SET shown_paths=? WHERE workspace_id=? AND agent_id=? \
             AND channel_id=? AND root_message_id=?",
        )
        .bind(encode(&kept)?)
        .bind(workspace.as_str())
        .bind(agent.as_str())
        .bind(cursor.get::<&str, _>("channel_id"))
        .bind(cursor.get::<&str, _>("root_message_id"))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}
