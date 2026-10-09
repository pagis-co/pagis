//! The checks of a Coding Session start (ADR-0033).
//!
//! The broker asks [`CodingSessionStarts`] once the machine of a start is
//! settled and before its card, so a start that cannot happen is an
//! answer and writes no Request. The tool runtime asks again when the
//! approved start runs, because the Agent can start another session
//! while the Person reads the card.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{SessionStartAction, SessionStarts, ToolResult, session_allowance};
use pagis_core::{
    AgentId, CodingSessionStore, GrantStore, Host, SessionApprovalMode, StoreError, WorkspaceId,
    harness,
};
use serde_json::Value;

/// The most Coding Sessions that one Agent holds open: not `closed` and
/// not `failed`.
pub const MAX_OPEN_SESSIONS: u32 = 4;

/// The prefix of the branch of each worktree that a session makes.
const BRANCH_PREFIX: &str = "pagis/";

/// The longest slug of a title in a branch name.
const MAX_SLUG_CHARS: usize = 48;

/// The checks of a Coding Session start, on the Coding Session and Grant
/// stores.
pub struct CodingSessionStarts {
    sessions: Arc<dyn CodingSessionStore>,
    grants: Arc<dyn GrantStore>,
}

impl CodingSessionStarts {
    pub fn new(sessions: Arc<dyn CodingSessionStore>, grants: Arc<dyn GrantStore>) -> Self {
        Self { sessions, grants }
    }
}

#[async_trait]
impl SessionStarts for CodingSessionStarts {
    /// Refuses, in this order: a harness that is not in the Harness
    /// Catalog, a directory that is not absolute, a fifth open session of
    /// the Agent, a mode wider than the host Grant allows, and a harness
    /// that never asks where the host Grant does not allow Unattended
    /// Modes.
    async fn describe(
        &self,
        workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        host: &Host,
        arguments: &Value,
    ) -> Result<SessionStartAction, ToolResult> {
        let harness_id = arguments["harness"].as_str().unwrap_or_default();
        let Some(entry) = harness::entry(harness_id) else {
            let known = harness::catalog()
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ToolResult::error(
                "unknown_harness",
                format!(
                    "{harness_id:?} is not a Coding Harness that Pagis starts. Use one of: {known}."
                ),
            ));
        };
        let directory = arguments["directory"].as_str().unwrap_or_default();
        if !is_absolute(directory, &host.platform) {
            return Err(ToolResult::error(
                "bad_directory",
                format!(
                    "{directory:?} is not an absolute path. Name the directory from the root of \
                     the file system of the user's {}.",
                    host.name
                ),
            ));
        }
        let open = self
            .sessions
            .count_open(workspace_id, agent_id)
            .await
            .map_err(unavailable)?;
        if open >= MAX_OPEN_SESSIONS {
            return Err(ToolResult::error(
                "session_limit",
                format!(
                    "You have {open} open coding sessions, and {MAX_OPEN_SESSIONS} is the most. \
                     Close one of your coding sessions first."
                ),
            ));
        }
        let mode = match arguments["mode"].as_str() {
            None => SessionApprovalMode::Person,
            Some(mode) => mode
                .parse::<SessionApprovalMode>()
                .map_err(|error| ToolResult::error("invalid_request", error))?,
        };
        let allowance = session_allowance(self.grants.as_ref(), workspace_id, agent_id, &host.id)
            .await
            .map_err(unavailable)?;
        let widest = allowance.widest_mode;
        if !widest.permits(mode) {
            return Err(ToolResult::error(
                "mode_not_allowed",
                format!(
                    "On the user's {}, the widest mode that you can use is {}. Start the session \
                     in that mode or a narrower one, or ask the user to allow a wider mode.",
                    host.name,
                    widest.as_str()
                ),
            ));
        }
        // Pagis policy sees no action of a harness that never asks.
        if !entry.asks_permission && !allowance.unattended_modes {
            return Err(ToolResult::error(
                "unattended_mode_not_allowed",
                format!(
                    "{} does not ask before it acts, so it runs only where the user allows modes \
                     that act without asking. Ask the user to allow them for you on {}.",
                    entry.label, host.name
                ),
            ));
        }
        let worktree = arguments["worktree"].as_bool().unwrap_or(true);
        Ok(SessionStartAction {
            harness_id: entry.id.to_string(),
            harness_name: entry.label.to_string(),
            directory: directory.to_string(),
            branch: worktree
                .then(|| worktree_branch(arguments["title"].as_str().unwrap_or_default())),
            mode,
            asks_permission: entry.asks_permission,
        })
    }
}

/// Whether a directory is an absolute path on a Host of `platform`. The
/// daemon can run on another operating system than the Host, so the
/// check reads the platform that the Client App reports.
fn is_absolute(directory: &str, platform: &str) -> bool {
    if platform == "windows" {
        let bytes = directory.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/');
        drive || directory.starts_with("\\\\")
    } else {
        directory.starts_with('/')
    }
}

/// The branch of the worktree of a session: `pagis/<slug of the title>`.
/// The slug keeps the ASCII letters and digits, in lower case, and puts
/// one `-` for each run of other characters, so the name is a valid git
/// branch and a valid directory name.
pub fn worktree_branch(title: &str) -> String {
    let mut slug = String::new();
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            if slug.len() == MAX_SLUG_CHARS {
                break;
            }
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        format!("{BRANCH_PREFIX}session")
    } else {
        format!("{BRANCH_PREFIX}{slug}")
    }
}

fn unavailable(error: StoreError) -> ToolResult {
    ToolResult::error("temporarily_unavailable", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_branch_is_a_slug_of_the_title() {
        assert_eq!(worktree_branch("Fix the login"), "pagis/fix-the-login");
        assert_eq!(
            worktree_branch("  Fix: the *login*!  "),
            "pagis/fix-the-login"
        );
        assert_eq!(worktree_branch("修复登录"), "pagis/session");
        let long = worktree_branch(&"a".repeat(100));
        assert_eq!(long, format!("pagis/{}", "a".repeat(MAX_SLUG_CHARS)));
    }

    #[test]
    fn an_absolute_directory_follows_the_platform_of_the_host() {
        assert!(is_absolute("/Users/bo/app", "macos"));
        assert!(!is_absolute("~/app", "linux"));
        assert!(is_absolute("C:\\Users\\bo\\app", "windows"));
        assert!(is_absolute("\\\\server\\share", "windows"));
        assert!(!is_absolute("/Users/bo/app", "windows"));
        assert!(!is_absolute("C:\\Users\\bo\\app", "macos"));
    }
}
