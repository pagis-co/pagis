use async_trait::async_trait;
use pagis_core::memory_page::PageBrief;
use pagis_core::{
    IndexedPage, MemoryAuthor, MemoryExposure, MemoryPageIndex, PageIndexHead, PageIndexUpdate,
    PageSearchHit, StoreError, WorkspaceId,
};
use sqlx::{Row, SqlitePool};

use crate::db_err;

/// The Page Index in SQLite (ADR-0008).
#[derive(Clone)]
pub struct SqliteMemoryPageIndex {
    pool: SqlitePool,
}

impl SqliteMemoryPageIndex {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MemoryPageIndex for SqliteMemoryPageIndex {
    async fn head(&self, workspace_id: &WorkspaceId) -> Result<Option<PageIndexHead>, StoreError> {
        let row = sqlx::query(
            "SELECT revision, next_position FROM memory_page_index_heads WHERE workspace_id = ?",
        )
        .bind(workspace_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(|row| PageIndexHead {
            revision: row.get("revision"),
            next_position: row.get::<i64, _>("next_position") as u64,
        }))
    }

    async fn apply(
        &self,
        workspace_id: &WorkspaceId,
        update: &PageIndexUpdate,
    ) -> Result<(), StoreError> {
        let mut transaction = crate::begin_write(&self.pool).await.map_err(db_err)?;
        if update.replace {
            sqlx::query(
                "DELETE FROM memory_page_search WHERE rowid IN \
                 (SELECT id FROM memory_page_index WHERE workspace_id = ?)",
            )
            .bind(workspace_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            sqlx::query("DELETE FROM memory_page_index WHERE workspace_id = ?")
                .bind(workspace_id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            sqlx::query("DELETE FROM memory_page_link WHERE workspace_id = ?")
                .bind(workspace_id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
        }
        for path in &update.removed {
            sqlx::query(
                "DELETE FROM memory_page_search WHERE rowid = \
                 (SELECT id FROM memory_page_index WHERE workspace_id = ? AND path = ?)",
            )
            .bind(workspace_id.as_str())
            .bind(path)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            sqlx::query("DELETE FROM memory_page_index WHERE workspace_id = ? AND path = ?")
                .bind(workspace_id.as_str())
                .bind(path)
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            // The links of a removed page go with its row. A link that
            // points at the removed page stays: its target no longer
            // exists, and a reader sees that from the rows.
            sqlx::query("DELETE FROM memory_page_link WHERE workspace_id = ? AND source_path = ?")
                .bind(workspace_id.as_str())
                .bind(path)
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
        }
        for page in &update.changed {
            let id: i64 = sqlx::query(
                "INSERT INTO memory_page_index \
                 (workspace_id, path, position, changed_at, changed_by_name, changed_by_email, \
                 title, kind, source_connection_id, exposures, brief_words) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT (workspace_id, path) DO UPDATE SET \
                 position = excluded.position, changed_at = excluded.changed_at, \
                 changed_by_name = excluded.changed_by_name, \
                 changed_by_email = excluded.changed_by_email, title = excluded.title, \
                 kind = excluded.kind, source_connection_id = excluded.source_connection_id, \
                 exposures = excluded.exposures, \
                 brief_words = excluded.brief_words RETURNING id",
            )
            .bind(workspace_id.as_str())
            .bind(&page.path)
            .bind(page.position as i64)
            .bind(page.changed_at)
            .bind(&page.changed_by.name)
            .bind(&page.changed_by.email)
            .bind(&page.title)
            .bind(&page.kind)
            .bind(&page.source_connection_id)
            .bind(encode(page.exposures.as_deref())?)
            .bind(page.brief.as_ref().map(|brief| brief.words.join(" ")))
            .fetch_one(&mut *transaction)
            .await
            .map_err(db_err)?
            .get("id");
            sqlx::query("DELETE FROM memory_page_search WHERE rowid = ?")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            sqlx::query(
                "INSERT INTO memory_page_search (rowid, workspace_id, path, title, body) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(workspace_id.as_str())
            .bind(&page.path)
            .bind(&page.title)
            .bind(&page.body)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            sqlx::query("DELETE FROM memory_page_link WHERE workspace_id = ? AND source_path = ?")
                .bind(workspace_id.as_str())
                .bind(&page.path)
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            for (position, target) in page.links.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO memory_page_link \
                     (workspace_id, source_path, target_path, position) \
                     VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
                )
                .bind(workspace_id.as_str())
                .bind(&page.path)
                .bind(target)
                .bind(position as i64)
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            }
        }
        for (path, exposures) in &update.restamped {
            sqlx::query(
                "UPDATE memory_page_index SET exposures = ? WHERE workspace_id = ? AND path = ?",
            )
            .bind(encode(exposures.as_deref())?)
            .bind(workspace_id.as_str())
            .bind(path)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
        }
        sqlx::query(
            "INSERT INTO memory_page_index_heads (workspace_id, revision, next_position) \
             VALUES (?, ?, ?) ON CONFLICT DO UPDATE SET \
             revision = excluded.revision, next_position = excluded.next_position",
        )
        .bind(workspace_id.as_str())
        .bind(&update.head.revision)
        .bind(update.head.next_position as i64)
        .execute(&mut *transaction)
        .await
        .map_err(db_err)?;
        transaction.commit().await.map_err(db_err)
    }

    async fn pages(
        &self,
        workspace_id: &WorkspaceId,
        root: &str,
    ) -> Result<Vec<IndexedPage>, StoreError> {
        // A range on the primary key, not LIKE: an agent id can hold
        // `_`, which LIKE reads as a wildcard. U+10FFFF sorts after
        // each character a path can hold.
        let rows = sqlx::query(
            "SELECT i.path, i.position, i.changed_at, i.changed_by_name, i.changed_by_email, \
             i.title, i.kind, i.source_connection_id, i.exposures, i.brief_words, \
             (SELECT group_concat(l.target_path, char(10) ORDER BY l.position) FROM memory_page_link l \
             WHERE l.workspace_id = i.workspace_id AND l.source_path = i.path) AS links \
             FROM memory_page_index i \
             WHERE i.workspace_id = ? AND i.path >= ? AND i.path < ? \
             ORDER BY i.position DESC, i.path",
        )
        .bind(workspace_id.as_str())
        .bind(root)
        .bind(format!("{root}\u{10FFFF}"))
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(indexed_page).collect()
    }

    async fn search(
        &self,
        workspace_id: &WorkspaceId,
        root: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<PageSearchHit>, StoreError> {
        let query = fts_query(query);
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT i.path, i.position, i.changed_at, i.changed_by_name, i.changed_by_email, \
             i.title, i.kind, i.source_connection_id, i.exposures, i.brief_words, \
             (SELECT group_concat(l.target_path, char(10) ORDER BY l.position) FROM memory_page_link l \
             WHERE l.workspace_id = i.workspace_id AND l.source_path = i.path) AS links, \
             snippet(memory_page_search, 3, '[', ']', '...', 16) AS snippet, \
             bm25(memory_page_search, 0.0, 1.0, 5.0, 1.0) AS rank \
             FROM memory_page_search JOIN memory_page_index i \
             ON i.id = memory_page_search.rowid \
             WHERE memory_page_search MATCH ? \
             AND memory_page_search.workspace_id = ? AND i.workspace_id = ? \
             AND i.path >= ? AND i.path < ? ORDER BY rank, i.path LIMIT ?",
        )
        .bind(query)
        .bind(workspace_id.as_str())
        .bind(workspace_id.as_str())
        .bind(root)
        .bind(format!("{root}\u{10FFFF}"))
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        rows.iter()
            .map(|row| {
                Ok(PageSearchHit {
                    page: indexed_page(row)?,
                    snippet: row.get("snippet"),
                    rank: row.get("rank"),
                })
            })
            .collect()
    }
}

/// One Page Index row. The body of a file is in the search table only.
///
/// The links come from one subquery over the link table. A link target
/// never holds a line break, so one joins the targets of a page.
fn indexed_page(row: &sqlx::sqlite::SqliteRow) -> Result<IndexedPage, StoreError> {
    let exposures: Option<String> = row.get("exposures");
    let brief_words: Option<String> = row.get("brief_words");
    let links: Option<String> = row.get("links");
    Ok(IndexedPage {
        brief: brief_words.map(|words| PageBrief {
            words: words
                .split(' ')
                .filter(|word| !word.is_empty())
                .map(str::to_string)
                .collect(),
        }),
        path: row.get("path"),
        position: row.get::<i64, _>("position") as u64,
        changed_at: row.get("changed_at"),
        changed_by: MemoryAuthor {
            name: row.get("changed_by_name"),
            email: row.get("changed_by_email"),
        },
        title: row.get("title"),
        body: String::new(),
        kind: row.get("kind"),
        source_connection_id: row.get("source_connection_id"),
        links: links
            .iter()
            .flat_map(|targets| targets.split('\n'))
            .filter(|target| !target.is_empty())
            .map(str::to_string)
            .collect(),
        exposures: exposures
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| StoreError::Corrupt(format!("page exposures: {error}")))?,
    })
}

/// The FTS5 query of the words a caller gave. Each word is a quoted
/// string, so no word is read as FTS5 syntax. The words are joined
/// with OR: a page with more of the words ranks higher, and a word the
/// page does not hold does not hide it.
fn fts_query(query: &str) -> String {
    query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{token}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn encode(exposures: Option<&[MemoryExposure]>) -> Result<Option<String>, StoreError> {
    exposures
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| StoreError::Corrupt(format!("page exposures: {error}")))
}
