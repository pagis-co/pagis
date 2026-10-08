//! The turn's block buffer (ADR-0004).
//!
//! `add_block` appends a content block to the message the current turn
//! finalizes. One turn stays one message: the buffer holds the turn's
//! blocks, the run loop takes them when it settles the reply row, and
//! the model's prose becomes the markdown blocks around them.
//!
//! Every cap is checked here and returned as a tool error, so the
//! model retries smaller. A cap enforced at message finalize would
//! instead kill an otherwise good turn.

use std::collections::HashMap;
use std::sync::Mutex;

use pagis_broker::ToolResult;
use pagis_core::{Block, KnownBlock, RunId};

/// Blocks one message carries.
pub const MAX_BLOCKS: usize = 20;

/// Rows one table carries. Past this the agent writes a CSV artifact
/// and posts a `file` block.
pub const MAX_TABLE_ROWS: usize = 100;

/// Bytes of blocks JSON one message carries.
pub const MAX_BLOCKS_BYTES: usize = 256 * 1024;

/// The per-run buffer of blocks `add_block` has appended to the turn
/// in flight.
#[derive(Default)]
pub struct BlockBuffer {
    runs: Mutex<HashMap<RunId, Vec<Block>>>,
}

impl BlockBuffer {
    /// Append one block, or refuse it with the reason. The argument is
    /// the raw `block` the model wrote.
    pub fn push(&self, run_id: &RunId, block: &serde_json::Value) -> Result<String, ToolResult> {
        let parsed = parse(block)?;
        let mut runs = self.runs.lock().expect("block buffer lock");
        let blocks = runs.entry(run_id.clone()).or_default();
        if blocks.len() >= MAX_BLOCKS {
            return Err(too_big(format!(
                "a message carries at most {MAX_BLOCKS} blocks; this one already has {}",
                blocks.len()
            )));
        }
        blocks.push(parsed.into());
        let bytes = serde_json::to_vec(&*blocks).map_or(0, |json| json.len());
        if bytes > MAX_BLOCKS_BYTES {
            blocks.pop();
            return Err(too_big(format!(
                "a message carries at most {MAX_BLOCKS_BYTES} bytes of blocks; \
                 this one would reach {bytes}"
            )));
        }
        Ok(format!(
            "added; the message now has {} blocks",
            blocks.len()
        ))
    }

    /// Append one block the daemon minted (ADR-0004). It goes
    /// past the `add_block` caps on purpose: an agent did not write
    /// it, and the daemon owns what it mints.
    pub fn mint(&self, run_id: &RunId, block: Block) {
        self.runs
            .lock()
            .expect("block buffer lock")
            .entry(run_id.clone())
            .or_default()
            .push(block);
    }

    /// The run's buffered blocks, and an empty buffer after it. The
    /// caps count against one message, so the turn starts over.
    pub fn take(&self, run_id: &RunId) -> Vec<Block> {
        self.runs
            .lock()
            .expect("block buffer lock")
            .remove(run_id)
            .unwrap_or_default()
    }

    pub fn clear(&self, run_id: &RunId) {
        self.runs.lock().expect("block buffer lock").remove(run_id);
    }
}

/// Read the model's block, and refuse anything an agent may not emit.
fn parse(block: &serde_json::Value) -> Result<KnownBlock, ToolResult> {
    let Ok(Block::Known(known)) = serde_json::from_value::<Block>(block.clone()) else {
        return Err(ToolResult::error(
            "invalid_request",
            "add_block: the block is not one of markdown, table, image or file",
        ));
    };
    if !known.is_content() {
        return Err(ToolResult::error(
            "invalid_request",
            "add_block: this block type is minted by Pagis. Add markdown, table, image or file; \
             use ask_user to ask a question.",
        ));
    }
    if let KnownBlock::Table { rows, .. } = &known
        && rows.len() > MAX_TABLE_ROWS
    {
        return Err(too_big(format!(
            "a table carries at most {MAX_TABLE_ROWS} rows, not {}. \
             Write a CSV artifact and post a file block instead.",
            rows.len()
        )));
    }
    Ok(known)
}

fn too_big(message: String) -> ToolResult {
    ToolResult::error("invalid_request", format!("add_block: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagis_core::{TableCell, TableColumn};

    fn run() -> RunId {
        RunId::generate()
    }

    fn table(rows: usize) -> serde_json::Value {
        serde_json::json!({
            "type": "table",
            "columns": [{"key": "r", "label": "R"}],
            "rows": (0..rows).map(|i| serde_json::json!([{"kind": "text", "text": format!("row {i}")}])).collect::<Vec<_>>()
        })
    }

    #[test]
    fn a_content_block_lands_in_call_order() {
        let buffer = BlockBuffer::default();
        let run = run();
        buffer
            .push(
                &run,
                &serde_json::json!({"type": "markdown", "text": "one"}),
            )
            .expect("markdown");
        buffer.push(&run, &table(1)).expect("table");
        let blocks = buffer.take(&run);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], Block::markdown("one"));
        assert_eq!(
            blocks[1],
            KnownBlock::Table {
                columns: vec![TableColumn {
                    key: "r".to_string(),
                    label: "R".to_string(),
                    align: None,
                }],
                rows: vec![vec![TableCell::Text {
                    text: "row 0".to_string()
                }]],
            }
            .into()
        );
        assert!(buffer.take(&run).is_empty(), "take empties the buffer");
    }

    #[test]
    fn a_daemon_owned_block_is_refused() {
        let buffer = BlockBuffer::default();
        for block in [
            serde_json::json!({"type": "approval_card", "request_id": "r", "title": "t", "body": "b"}),
            serde_json::json!({"type": "progress", "run_id": "r", "text": "t"}),
            serde_json::json!({"type": "screen", "agent_id": "a"}),
            serde_json::json!({"type": "form", "request_id": "r", "title": "t", "fields": []}),
            serde_json::json!({"type": "choice_card", "request_id": "r", "title": "t", "options": []}),
        ] {
            let error = buffer.push(&run(), &block).expect_err("refused");
            assert_eq!(error.code.as_deref(), Some("invalid_request"));
        }
    }

    #[test]
    fn a_coding_session_block_is_refused_as_minted_by_pagis() {
        let buffer = BlockBuffer::default();
        let block = serde_json::json!({
            "type": "coding_session",
            "coding_session_id": "cs_1",
            "harness": "Claude Code",
            "machine": "Air",
            "directory": "/Users/bo/code/app",
            "title": "Fix the login bug",
        });

        let error = buffer.push(&run(), &block).expect_err("refused");

        assert_eq!(error.code.as_deref(), Some("invalid_request"));
        assert!(
            error.content.contains("minted by Pagis"),
            "{}",
            error.content
        );
    }

    #[test]
    fn an_unreadable_block_is_refused() {
        let buffer = BlockBuffer::default();
        assert!(
            buffer
                .push(&run(), &serde_json::json!({"type": "hologram"}))
                .is_err()
        );
        assert!(
            buffer
                .push(&run(), &serde_json::json!({"type": "markdown", "text": 7}))
                .is_err()
        );
    }

    #[test]
    fn the_block_cap_refuses_the_next_block_and_keeps_the_message() {
        let buffer = BlockBuffer::default();
        let run = run();
        for i in 0..MAX_BLOCKS {
            buffer
                .push(
                    &run,
                    &serde_json::json!({"type": "markdown", "text": format!("{i}")}),
                )
                .expect("under the cap");
        }
        let error = buffer
            .push(
                &run,
                &serde_json::json!({"type": "markdown", "text": "one more"}),
            )
            .expect_err("over the cap");
        assert!(error.content.contains("20"), "{}", error.content);
        assert_eq!(buffer.take(&run).len(), MAX_BLOCKS);
    }

    #[test]
    fn a_table_past_its_row_cap_is_refused() {
        let buffer = BlockBuffer::default();
        let run = run();
        buffer
            .push(&run, &table(MAX_TABLE_ROWS))
            .expect("at the cap");
        let error = buffer
            .push(&run, &table(MAX_TABLE_ROWS + 1))
            .expect_err("over the cap");
        assert!(error.content.contains("file block"), "{}", error.content);
        assert_eq!(buffer.take(&run).len(), 1);
    }

    #[test]
    fn the_byte_cap_refuses_the_block_that_would_pass_it() {
        let buffer = BlockBuffer::default();
        let run = run();
        let big = "x".repeat(MAX_BLOCKS_BYTES / 4 - 100);
        for _ in 0..4 {
            buffer
                .push(&run, &serde_json::json!({"type": "markdown", "text": big}))
                .expect("under the cap");
        }
        let error = buffer
            .push(&run, &serde_json::json!({"type": "markdown", "text": big}))
            .expect_err("over the cap");
        assert!(error.content.contains("bytes"), "{}", error.content);
        assert_eq!(buffer.take(&run).len(), 4);
    }

    #[test]
    fn one_run_never_sees_another_run_blocks() {
        let buffer = BlockBuffer::default();
        let (first, second) = (run(), run());
        buffer
            .push(
                &first,
                &serde_json::json!({"type": "markdown", "text": "mine"}),
            )
            .expect("push");
        assert!(buffer.take(&second).is_empty());
        buffer.clear(&first);
        assert!(buffer.take(&first).is_empty());
    }
}
