use async_trait::async_trait;
use pagis_core::memory_page::PageBrief;
use pagis_core::{
    IndexedPage, MemoryAuthor, MemoryExposure, MemoryPageIndex, PageIndexHead, PageIndexUpdate,
    PageSearchHit, StoreError, WorkspaceId,
};
use sqlx::{PgPool, Row};

use crate::db_err;

/// The Page Index in Postgres (ADR-0008).
#[derive(Clone)]
pub struct PostgresMemoryPageIndex {
    pool: PgPool,
}

impl PostgresMemoryPageIndex {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// The `tsvector` of one page, with the weights that stand for the
/// FTS5 column weights of the SQLite backend (path 1.0, title 5.0,
/// body 1.0). `A` carries the title; `B` the path and `D` the body,
/// both at the same weight in [`RANK_WEIGHTS`].
const DOCUMENT: &str = "setweight(to_tsvector('english', $4), 'A') || \
                        setweight(to_tsvector('english', $3), 'B') || \
                        setweight(to_tsvector('english', $5), 'D')";

/// The weight of each label, in the order Postgres takes them:
/// `{D, C, B, A}`. Title (`A`) at 1.0 against path (`B`) and body
/// (`D`) at 0.2 is the 5:1:1 of the bm25 weights on the other backend.
/// `C` is never set by [`DOCUMENT`], so its weight stands for nothing
/// and Postgres needs the entry only to fill the array.
const RANK_WEIGHTS: &str = "{0.2, 0.4, 0.2, 1.0}";

/// The `ts_headline` options that stand for the FTS5
/// `snippet(memory_page_search, 3, '[', ']', '...', 16)`.
///
/// `MinWords` is what makes the window hold words beside the match.
/// With a small one `ts_headline` answers the matched word alone, and
/// the reader of a hit learns nothing from it. `MaxFragments` is not
/// set, because fragment mode cuts a short body at the first cover.
const HEADLINE: &str = "StartSel=[, StopSel=], MaxWords=16, MinWords=8, ShortWord=0";

#[async_trait]
impl MemoryPageIndex for PostgresMemoryPageIndex {
    async fn head(&self, workspace_id: &WorkspaceId) -> Result<Option<PageIndexHead>, StoreError> {
        let row = sqlx::query(
            "SELECT revision, next_position FROM memory_page_index_heads WHERE workspace_id = $1",
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
                "DELETE FROM memory_page_search WHERE id IN \
                 (SELECT id FROM memory_page_index WHERE workspace_id = $1)",
            )
            .bind(workspace_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            sqlx::query("DELETE FROM memory_page_index WHERE workspace_id = $1")
                .bind(workspace_id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            sqlx::query("DELETE FROM memory_page_link WHERE workspace_id = $1")
                .bind(workspace_id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
        }
        for path in &update.removed {
            sqlx::query(
                "DELETE FROM memory_page_search WHERE id = \
                 (SELECT id FROM memory_page_index WHERE workspace_id = $1 AND path = $2)",
            )
            .bind(workspace_id.as_str())
            .bind(path)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            sqlx::query("DELETE FROM memory_page_index WHERE workspace_id = $1 AND path = $2")
                .bind(workspace_id.as_str())
                .bind(path)
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            // The links of a removed page go with its row. A link that
            // points at the removed page stays: its target no longer
            // exists, and a reader sees that from the rows.
            sqlx::query(
                "DELETE FROM memory_page_link WHERE workspace_id = $1 AND source_path = $2",
            )
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
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
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
            sqlx::query("DELETE FROM memory_page_search WHERE id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(db_err)?;
            sqlx::query(&format!(
                "INSERT INTO memory_page_search (id, workspace_id, path, title, body, document) \
                 VALUES ($1, $2, $3, $4, $5, {DOCUMENT})"
            ))
            .bind(id)
            .bind(workspace_id.as_str())
            .bind(&page.path)
            .bind(&page.title)
            .bind(&page.body)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            sqlx::query(
                "DELETE FROM memory_page_link WHERE workspace_id = $1 AND source_path = $2",
            )
            .bind(workspace_id.as_str())
            .bind(&page.path)
            .execute(&mut *transaction)
            .await
            .map_err(db_err)?;
            for (position, target) in page.links.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO memory_page_link \
                     (workspace_id, source_path, target_path, position) \
                     VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
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
                "UPDATE memory_page_index SET exposures = $1 WHERE workspace_id = $2 AND path = $3",
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
             VALUES ($1, $2, $3) ON CONFLICT (workspace_id) DO UPDATE SET \
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
        // each character a path can hold. Every text column carries the
        // `C` collation, so the range compares bytes here as it does on
        // the other backend.
        let rows = sqlx::query(
            "SELECT i.path, i.position, i.changed_at, i.changed_by_name, i.changed_by_email, \
             i.title, i.kind, i.source_connection_id, i.exposures, i.brief_words, \
             (SELECT string_agg(l.target_path, chr(10) ORDER BY l.position) FROM memory_page_link l \
             WHERE l.workspace_id = i.workspace_id AND l.source_path = i.path) AS links \
             FROM memory_page_index i \
             WHERE i.workspace_id = $1 AND i.path >= $2 AND i.path < $3 \
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
        // The rank is negated, so a smaller rank is a better hit as it
        // is under bm25 on the other backend. `pagis_agent` orders the
        // hits of the two scopes on it, ascending.
        let rows = sqlx::query(&format!(
            "SELECT i.path, i.position, i.changed_at, i.changed_by_name, i.changed_by_email, \
             i.title, i.kind, i.source_connection_id, i.exposures, i.brief_words, \
             (SELECT string_agg(l.target_path, chr(10) ORDER BY l.position) FROM memory_page_link l \
             WHERE l.workspace_id = i.workspace_id AND l.source_path = i.path) AS links, \
             ts_headline('english', s.body, to_tsquery('english', $1), '{HEADLINE}') AS snippet, \
             -ts_rank_cd('{RANK_WEIGHTS}', s.document, to_tsquery('english', $1)) AS rank \
             FROM memory_page_search s JOIN memory_page_index i ON i.id = s.id \
             WHERE s.document @@ to_tsquery('english', $1) \
             AND s.workspace_id = $2 AND i.workspace_id = $3 \
             AND i.path >= $4 AND i.path < $5 ORDER BY rank, i.path LIMIT $6"
        ))
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
                    rank: row.get::<f32, _>("rank") as f64,
                })
            })
            .collect()
    }
}

/// One Page Index row. The body of a file is in the search table only.
///
/// The links come from one subquery over the link table. A link target
/// never holds a line break, so one joins the targets of a page.
fn indexed_page(row: &sqlx::postgres::PgRow) -> Result<IndexedPage, StoreError> {
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

/// The `tsquery` of the words a caller gave. The words are joined with
/// `|`: a page with more of the words ranks higher, and a word the page
/// does not hold does not hide it. A token holds only alphanumeric
/// characters, so no token can be read as `tsquery` syntax.
fn fts_query(query: &str) -> String {
    query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn encode(exposures: Option<&[MemoryExposure]>) -> Result<Option<String>, StoreError> {
    exposures
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| StoreError::Corrupt(format!("page exposures: {error}")))
}
