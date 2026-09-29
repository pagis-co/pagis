//! The `tool_search` core tool.
//!
//! Every Software List tool is in the run's Capability Snapshot from
//! run start, and none of them is in the model's tool array. This
//! search is the one gate: a query loads every tool of each matching
//! package, and an empty query lists the whole list.
//!
//! The text is Pagis's own, not the author's program's, so it carries
//! no untrusted envelope (ADR-0005).

use crate::index::ToolIndex;

/// How many packages one query loads. A package's tools are designed
/// together, so a match loads the package, not one tool.
const LOAD_LIMIT: usize = 5;

/// What a query with no match answers.
pub const NO_MATCH: &str = "no package matches; call with an empty query to browse";

/// What a browse of an empty Software List answers.
pub const EMPTY_LIST: &str = "the Software List is empty; publish a package to add tools";

/// One search: the text the model reads, and the packages the run
/// loads because of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOutcome {
    pub text: String,
    /// The packages this call loads, best match first. A package the
    /// run already holds is in the list again: the load is a no-op,
    /// and the text stays the same.
    pub packages: Vec<String>,
}

/// Search the Software List. An empty query browses.
pub fn tool_search(index: &ToolIndex, query: &str) -> SearchOutcome {
    if query.trim().is_empty() {
        let lines = index.browse();
        let text = if lines.is_empty() {
            EMPTY_LIST.to_string()
        } else {
            lines.join("\n")
        };
        return SearchOutcome {
            text,
            packages: Vec::new(),
        };
    }
    let hits = match index.search(query) {
        Ok(hits) => hits,
        Err(problem) => {
            return SearchOutcome {
                text: problem,
                packages: Vec::new(),
            };
        }
    };
    let hits: Vec<_> = hits.into_iter().take(LOAD_LIMIT).collect();
    if hits.is_empty() {
        return SearchOutcome {
            text: NO_MATCH.to_string(),
            packages: Vec::new(),
        };
    }
    let mut lines = Vec::new();
    for hit in &hits {
        for tool in &hit.tools {
            lines.push(format!("{}__{}: {}", hit.name, tool.name, tool.description));
        }
    }
    lines.push("These tools are now callable.".to_string());
    SearchOutcome {
        text: lines.join("\n"),
        packages: hits.into_iter().map(|hit| hit.name).collect(),
    }
}
