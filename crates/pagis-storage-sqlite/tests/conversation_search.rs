//! The FTS5 segments of the two conversation search tables hold no word
//! of a forgotten message.
//!
//! FTS5 keeps the words of a deleted row in its segments until a later
//! merge, unless the table has the `secure-delete` option. The store
//! suite reads every value of the database after a Forget, and this
//! test names the segments of the two tables directly (ADR-0008).

use pagis_core::{Grant, GrantId, MemoryExposure, RetainedToolEvidence};
use pagis_testkit::fixture;
use pagis_testkit::store_suite::{Backend, Rows};

/// A word of the forgotten reply. It is one token for FTS5, the porter
/// stemmer keeps it as it is, and no other word of its index starts
/// with its first letters, so a segment holds it whole.
const REPLY_WORD: &str = "zqxreply3f9a";

/// A word of the tool result of the Run, with the same properties.
const TOOL_WORD: &str = "zqxtool3f9a";

/// Every value of each table whose name starts with `index`: the FTS5
/// table and its shadow tables, the segments included.
async fn index_values(pool: &sqlx::SqlitePool, index: &str) -> Vec<Vec<u8>> {
    let columns: Vec<(String, String)> = sqlx::query_as(
        "SELECT t.name, c.name FROM sqlite_schema t, pragma_table_info(t.name) c \
         WHERE t.type = 'table' AND t.name LIKE ? || '%'",
    )
    .bind(index)
    .fetch_all(pool)
    .await
    .unwrap();
    assert!(
        columns
            .iter()
            .any(|(table, _)| table == &format!("{index}_data")),
        "{index} has no segment table"
    );
    let mut values = Vec::new();
    for (table, column) in columns {
        let found: Vec<Vec<u8>> = sqlx::query_scalar(&format!(
            "SELECT CAST(\"{column}\" AS BLOB) FROM \"{table}\" WHERE \"{column}\" IS NOT NULL"
        ))
        .fetch_all(pool)
        .await
        .unwrap();
        values.extend(found);
    }
    values
}

/// Whether one value of the index holds `word`.
async fn index_holds(pool: &sqlx::SqlitePool, index: &str, word: &str) -> bool {
    index_values(pool, index).await.iter().any(|value| {
        value
            .windows(word.len())
            .any(|window| window == word.as_bytes())
    })
}

#[tokio::test]
async fn the_segments_of_the_conversation_search_hold_no_word_of_a_forgotten_message() {
    let backend = Backend::sqlite().await;
    let Rows::Sqlite(pool) = backend.rows().clone() else {
        unreachable!("a SQLite backend holds a SQLite pool");
    };
    let stores = backend.stores();
    let workspace = backend.seeded_workspace().await;
    let agent = fixture::agent(&workspace.id);
    stores.agents.create(&agent).await.unwrap();
    let channel = fixture::channel(&workspace.id);
    stores.channels.create(&channel).await.unwrap();
    let grant = Grant {
        id: GrantId::from("grant-mail".to_string()),
        workspace_id: workspace.id.clone(),
        agent_id: agent.id.clone(),
        resource_kind: Grant::CONNECTION_KIND.to_string(),
        resource_id: None,
        scope: serde_json::json!({}),
        revision: 1,
        created_at: 1,
        revoked_at: None,
    };
    stores.grants.create(&grant).await.unwrap();
    let exposure = MemoryExposure {
        grant_id: grant.id.clone(),
        revision: 1,
    };
    let run = fixture::queued_run(&workspace.id, &agent.id, &channel.id);
    stores.runs.create(&run).await.unwrap();
    let reply = pagis_core::Message {
        run_id: Some(run.id.clone()),
        ..fixture::agent_message(
            &workspace.id,
            &channel.id,
            &agent.id,
            &format!("The {REPLY_WORD} invoice is due on Friday."),
        )
    };
    stores
        .messages
        .insert_stamped(&reply, std::slice::from_ref(&exposure))
        .await
        .unwrap();
    stores
        .conversation_evidence
        .retain_tool(&RetainedToolEvidence {
            scope: pagis_core::ConversationScope {
                workspace_id: workspace.id.clone(),
                agent_id: agent.id.clone(),
                channel_id: channel.id.clone(),
                root_message_id: None,
            },
            source_message_id: reply.id.clone(),
            run_id: run.id.clone(),
            tool_call_id: "call-1".to_string(),
            tool_name: "mail__get_message".to_string(),
            content: format!("The {TOOL_WORD} invoice arrived."),
            complete: true,
            exposure,
            created_at: 1,
        })
        .await
        .unwrap();
    // A message that stays keeps the segments in use, so the check reads
    // a table that FTS5 still writes to and not an empty one.
    let kept = fixture::user_message(&workspace.id, &channel.id, "The lanternfish note stays.");
    stores.messages.insert(&kept).await.unwrap();
    assert!(
        index_holds(&pool, "conversation_message_search", REPLY_WORD).await,
        "the message index holds the reply before the Forget"
    );
    assert!(
        index_holds(&pool, "conversation_tool_search", TOOL_WORD).await,
        "the tool index holds the tool result before the Forget"
    );

    // A Forget marks the message forgotten. That takes its row out of
    // the message index and deletes its tool result, whose row leaves
    // the tool index.
    sqlx::query("INSERT INTO forgotten_messages(message_id) VALUES (?)")
        .bind(reply.id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    assert!(
        !index_holds(&pool, "conversation_message_search", REPLY_WORD).await,
        "an FTS5 segment of the message index holds a word of the forgotten message"
    );
    assert!(
        !index_holds(&pool, "conversation_tool_search", TOOL_WORD).await,
        "an FTS5 segment of the tool index holds a word of the forgotten tool result"
    );
    assert!(
        index_holds(&pool, "conversation_message_search", "lanternfish").await,
        "the message index lost the message that stays"
    );
}
