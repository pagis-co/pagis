use async_trait::async_trait;
use pagis_core::{
    AuthorKind, ConversationEvidenceHit, ConversationEvidenceMessage, ConversationEvidenceStore,
    ConversationScope, ConversationToolEvidence, EvidenceArtifactRef, MessageId,
    RetainedToolEvidence, StoreError,
};
use sqlx::{PgPool, Row};

use crate::db_err;

#[derive(Clone)]
pub struct PostgresConversationEvidenceStore {
    pool: PgPool,
}

impl PostgresConversationEvidenceStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// The `tsquery` of the words a caller gave. The words are joined with
/// `|`, so a row with more of the words ranks higher and a word the row
/// does not hold does not hide it. A token holds only alphanumeric
/// characters, so no token can be read as `tsquery` syntax.
fn fts_query(query: &str) -> String {
    query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The thread predicate, and the value it binds. `first` is the number
/// of the first placeholder it may take: a Postgres placeholder carries
/// its position, so a fragment that a caller splices has to know where
/// it lands.
fn root_predicate(root: Option<&MessageId>, first: usize) -> (String, Option<&str>) {
    match root {
        Some(root) => (
            format!("(m.id = ${first} OR m.parent_message_id = ${})", first + 1),
            Some(root.as_str()),
        ),
        None => ("m.parent_message_id IS NULL".to_string(), None),
    }
}

/// How many placeholders [`root_predicate`] takes.
fn root_binds(root: Option<&MessageId>) -> usize {
    if root.is_some() { 2 } else { 0 }
}

fn live_source_predicate() -> &'static str {
    "NOT EXISTS (SELECT 1 FROM forgotten_messages f WHERE f.message_id = m.id) \
     AND (m.author_kind = 'user' OR (\
       EXISTS (SELECT 1 FROM message_exposure_sets s WHERE s.message_id = m.id) \
       AND NOT EXISTS (\
         SELECT 1 FROM message_exposures x \
         WHERE x.message_id = m.id AND NOT EXISTS (\
           SELECT 1 FROM grants g WHERE g.id = x.grant_id \
             AND g.agent_id = m.author_agent_id \
             AND g.revision = x.grant_revision AND g.revoked_at IS NULL\
         )\
       )\
     ))"
}

/// The `ts_headline` options that stand for the FTS5
/// `snippet(t, c, '[', ']', '...', 18)` of the SQLite backend: the same
/// brackets and about the same length.
///
/// `MinWords` is what makes the window hold words beside the match.
/// With a small one `ts_headline` answers the matched word alone, and
/// the reader of a hit learns nothing from it. `MaxFragments` is not
/// set, because fragment mode cuts a short text at the first cover.
const HEADLINE: &str = "StartSel=[, StopSel=], MaxWords=18, MinWords=9, ShortWord=0";

#[async_trait]
impl ConversationEvidenceStore for PostgresConversationEvidenceStore {
    async fn search(
        &self,
        scope: &ConversationScope,
        query: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ConversationEvidenceHit>, StoreError> {
        let query = fts_query(query);
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let (root_sql, root) = root_predicate(scope.root_message_id.as_ref(), 5);
        let (after_id, after_reference) = search_cursor(after);
        // The cursor and the limit sit behind the thread predicate, so
        // their numbers move with it.
        let next = 5 + root_binds(scope.root_message_id.as_ref());
        let sql = format!(
            "SELECT m.id, m.author_kind, m.author_agent_id, m.created_at, \
             ts_headline('english', s.text_content, to_tsquery('english', $1), '{HEADLINE}') \
             AS snippet \
             FROM conversation_message_search s JOIN messages m ON m.id = s.message_id \
             WHERE s.document @@ to_tsquery('english', $1) AND s.workspace_id = $2 \
             AND m.workspace_id = $3 AND m.channel_id = $4 AND {root_sql} \
             AND (m.id > ${next} OR (m.id = ${} AND ('message:' || m.id) > ${})) AND {} \
             ORDER BY m.id LIMIT ${}",
            next + 1,
            next + 2,
            live_source_predicate(),
            next + 3
        );
        let mut statement = sqlx::query(&sql)
            .bind(&query)
            .bind(scope.workspace_id.as_str())
            .bind(scope.workspace_id.as_str())
            .bind(scope.channel_id.as_str());
        if let Some(root) = root {
            statement = statement.bind(root).bind(root);
        }
        let rows = statement
            .bind(after_id)
            .bind(after_id)
            .bind(after_reference)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let mut hits: Vec<ConversationEvidenceHit> = rows
            .iter()
            .map(|row| {
                let id = MessageId::from(row.get::<String, _>("id"));
                Ok(ConversationEvidenceHit {
                    kind: "message".to_string(),
                    reference: format!("message:{}", id.as_str()),
                    message_id: id,
                    speaker: speaker(row)?,
                    created_at: row.get("created_at"),
                    snippet: row.get("snippet"),
                    complete: true,
                })
            })
            .collect::<Result<_, StoreError>>()?;

        let tool_root_sql = if scope.root_message_id.is_some() {
            "t.root_message_id = $6"
        } else {
            "t.root_message_id IS NULL"
        };
        let tool_next = 6 + usize::from(scope.root_message_id.is_some());
        let tool_sql = format!(
            "SELECT t.reference, t.source_message_id, t.tool_name, t.created_at, t.complete, \
             ts_headline('english', s.content, to_tsquery('english', $1), '{HEADLINE}') \
             AS snippet \
             FROM conversation_tool_search s JOIN conversation_tool_evidence t \
             ON t.reference = s.reference \
             WHERE s.document @@ to_tsquery('english', $1) AND s.workspace_id = $2 \
             AND t.workspace_id = $3 AND t.agent_id = $4 AND t.channel_id = $5 \
             AND {tool_root_sql} \
             AND (t.source_message_id > ${tool_next} OR (t.source_message_id = ${} \
               AND t.reference > ${})) \
             AND NOT EXISTS (SELECT 1 FROM forgotten_messages f \
               WHERE f.message_id = t.source_message_id) \
             AND EXISTS (SELECT 1 FROM grants g WHERE g.id = t.grant_id \
               AND g.agent_id = t.agent_id AND g.revision = t.grant_revision \
               AND g.revoked_at IS NULL) \
             ORDER BY t.source_message_id, t.reference LIMIT ${}",
            tool_next + 1,
            tool_next + 2,
            tool_next + 3
        );
        let mut tools = sqlx::query(&tool_sql)
            .bind(query.clone())
            .bind(scope.workspace_id.as_str())
            .bind(scope.workspace_id.as_str())
            .bind(scope.agent_id.as_str())
            .bind(scope.channel_id.as_str());
        if let Some(root) = scope.root_message_id.as_ref() {
            tools = tools.bind(root.as_str());
        }
        let tool_rows = tools
            .bind(after_id)
            .bind(after_id)
            .bind(after_reference)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        for row in tool_rows {
            hits.push(ConversationEvidenceHit {
                kind: "tool".to_string(),
                reference: row.get("reference"),
                message_id: MessageId::from(row.get::<String, _>("source_message_id")),
                speaker: format!("tool:{}", row.get::<String, _>("tool_name")),
                created_at: row.get("created_at"),
                snippet: row.get("snippet"),
                complete: row.get("complete"),
            });
        }
        hits.sort_by(|left, right| {
            left.message_id
                .as_str()
                .cmp(right.message_id.as_str())
                .then_with(|| left.reference.cmp(&right.reference))
        });
        hits.truncate(limit as usize);
        Ok(hits)
    }

    async fn read_range(
        &self,
        scope: &ConversationScope,
        after_exclusive: Option<&MessageId>,
        through_inclusive: &MessageId,
        limit: u32,
    ) -> Result<Vec<ConversationEvidenceMessage>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let (root_sql, root) = root_predicate(scope.root_message_id.as_ref(), 3);
        let next = 3 + root_binds(scope.root_message_id.as_ref());
        let sql = format!(
            "SELECT m.id, m.author_kind, m.author_agent_id, m.created_at, \
             m.text_content, m.blocks FROM messages m \
             WHERE m.workspace_id = $1 AND m.channel_id = $2 AND {root_sql} \
             AND m.id > ${next} AND m.id <= ${} AND m.status = 'complete' \
             AND ((m.blocks::jsonb -> 0) ->> 'type') IS DISTINCT FROM 'progress' AND {} \
             ORDER BY m.id LIMIT ${}",
            next + 1,
            live_source_predicate(),
            next + 2
        );
        let mut statement = sqlx::query(&sql)
            .bind(scope.workspace_id.as_str())
            .bind(scope.channel_id.as_str());
        if let Some(root) = root {
            statement = statement.bind(root).bind(root);
        }
        let rows = statement
            .bind(after_exclusive.map(MessageId::as_str).unwrap_or(""))
            .bind(through_inclusive.as_str())
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            let id = MessageId::from(row.get::<String, _>("id"));
            let blocks: Vec<pagis_core::Block> = serde_json::from_str(row.get("blocks"))
                .map_err(|error| StoreError::Corrupt(format!("message blocks: {error}")))?;
            messages.push(ConversationEvidenceMessage {
                reference: format!("message:{}", id.as_str()),
                message_id: id,
                speaker: speaker(&row)?,
                created_at: row.get("created_at"),
                text: row.get("text_content"),
                artifacts: artifacts(&self.pool, &scope.workspace_id, &blocks).await?,
            });
        }
        Ok(messages)
    }

    async fn read_message(
        &self,
        scope: &ConversationScope,
        message_id: &MessageId,
    ) -> Result<Option<ConversationEvidenceMessage>, StoreError> {
        let (root_sql, root) = root_predicate(scope.root_message_id.as_ref(), 4);
        let sql = format!(
            "SELECT m.id, m.author_kind, m.author_agent_id, m.created_at, \
             m.text_content, m.blocks FROM messages m \
             WHERE m.id = $1 AND m.workspace_id = $2 AND m.channel_id = $3 AND {root_sql} \
             AND m.status = 'complete' \
             AND ((m.blocks::jsonb -> 0) ->> 'type') IS DISTINCT FROM 'progress' AND {}",
            live_source_predicate()
        );
        let mut statement = sqlx::query(&sql)
            .bind(message_id.as_str())
            .bind(scope.workspace_id.as_str())
            .bind(scope.channel_id.as_str());
        if let Some(root) = root {
            statement = statement.bind(root).bind(root);
        }
        let Some(row) = statement.fetch_optional(&self.pool).await.map_err(db_err)? else {
            return Ok(None);
        };
        let blocks: Vec<pagis_core::Block> = serde_json::from_str(row.get("blocks"))
            .map_err(|error| StoreError::Corrupt(format!("message blocks: {error}")))?;
        Ok(Some(ConversationEvidenceMessage {
            reference: format!("message:{}", message_id.as_str()),
            message_id: message_id.clone(),
            speaker: speaker(&row)?,
            created_at: row.get("created_at"),
            text: row.get("text_content"),
            artifacts: artifacts(&self.pool, &scope.workspace_id, &blocks).await?,
        }))
    }

    async fn retain_tool(&self, evidence: &RetainedToolEvidence) -> Result<String, StoreError> {
        let reference = format!(
            "tool:{}:{}",
            evidence.run_id.as_str(),
            evidence.tool_call_id
        );
        sqlx::query(
            "INSERT INTO conversation_tool_evidence \
             (reference, workspace_id, agent_id, channel_id, root_message_id, source_message_id, \
              run_id, tool_call_id, tool_name, content, complete, grant_id, grant_revision, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
             ON CONFLICT (run_id, tool_call_id) DO NOTHING",
        )
        .bind(&reference)
        .bind(evidence.scope.workspace_id.as_str())
        .bind(evidence.scope.agent_id.as_str())
        .bind(evidence.scope.channel_id.as_str())
        .bind(evidence.scope.root_message_id.as_ref().map(MessageId::as_str))
        .bind(evidence.source_message_id.as_str())
        .bind(evidence.run_id.as_str())
        .bind(&evidence.tool_call_id)
        .bind(&evidence.tool_name)
        .bind(&evidence.content)
        .bind(evidence.complete)
        .bind(evidence.exposure.grant_id.as_str())
        .bind(evidence.exposure.revision)
        .bind(evidence.created_at)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(reference)
    }

    async fn read_tool(
        &self,
        scope: &ConversationScope,
        reference: &str,
    ) -> Result<Option<ConversationToolEvidence>, StoreError> {
        let root_sql = if scope.root_message_id.is_some() {
            "t.root_message_id = $5"
        } else {
            "t.root_message_id IS NULL"
        };
        let sql = format!(
            "SELECT t.reference, t.source_message_id, t.tool_name, t.created_at, \
             t.content, t.complete FROM conversation_tool_evidence t \
             WHERE t.reference = $1 AND t.workspace_id = $2 AND t.agent_id = $3 \
             AND t.channel_id = $4 AND {root_sql} \
             AND NOT EXISTS (SELECT 1 FROM forgotten_messages f \
               WHERE f.message_id = t.source_message_id) \
             AND EXISTS (SELECT 1 FROM grants g WHERE g.id = t.grant_id \
               AND g.agent_id = t.agent_id AND g.revision = t.grant_revision \
               AND g.revoked_at IS NULL)"
        );
        let mut statement = sqlx::query(&sql)
            .bind(reference)
            .bind(scope.workspace_id.as_str())
            .bind(scope.agent_id.as_str())
            .bind(scope.channel_id.as_str());
        if let Some(root) = scope.root_message_id.as_ref() {
            statement = statement.bind(root.as_str());
        }
        let row = statement.fetch_optional(&self.pool).await.map_err(db_err)?;
        Ok(row.map(|row| ConversationToolEvidence {
            kind: "tool".to_string(),
            reference: row.get("reference"),
            source_message_id: MessageId::from(row.get::<String, _>("source_message_id")),
            tool_name: row.get("tool_name"),
            created_at: row.get("created_at"),
            content: row.get("content"),
            complete: row.get("complete"),
        }))
    }
}

fn search_cursor(after: Option<&str>) -> (&str, &str) {
    after
        .and_then(|cursor| cursor.split_once('|'))
        .unwrap_or(("", ""))
}

fn speaker(row: &sqlx::postgres::PgRow) -> Result<String, StoreError> {
    let kind: AuthorKind = row
        .get::<String, _>("author_kind")
        .parse()
        .map_err(StoreError::Corrupt)?;
    Ok(match kind {
        AuthorKind::Agent => row
            .get::<Option<String>, _>("author_agent_id")
            .map(|id| format!("agent:{id}"))
            .unwrap_or_else(|| "agent".to_string()),
        _ => kind.as_str().to_string(),
    })
}

async fn artifacts(
    pool: &PgPool,
    workspace_id: &pagis_core::WorkspaceId,
    blocks: &[pagis_core::Block],
) -> Result<Vec<EvidenceArtifactRef>, StoreError> {
    let mut references = Vec::new();
    for block in blocks {
        let id = match block {
            pagis_core::Block::Known(pagis_core::KnownBlock::Image { artifact_id, .. })
            | pagis_core::Block::Known(pagis_core::KnownBlock::File { artifact_id, .. }) => {
                artifact_id
            }
            _ => continue,
        };
        let row = sqlx::query(
            "SELECT filename, mime, size_bytes FROM artifacts WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace_id.as_str())
        .fetch_optional(pool)
        .await
        .map_err(db_err)?;
        references.push(match row {
            Some(row) => EvidenceArtifactRef {
                reference: format!("artifact:{id}"),
                available: true,
                filename: row.get("filename"),
                mime: Some(row.get("mime")),
                size_bytes: Some(row.get("size_bytes")),
            },
            None => EvidenceArtifactRef {
                reference: format!("artifact:{id}"),
                available: false,
                filename: None,
                mime: None,
                size_bytes: None,
            },
        });
    }
    Ok(references)
}
