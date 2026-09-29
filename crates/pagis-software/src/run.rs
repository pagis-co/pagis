//! The run contract of a Software tool.
//!
//! One call is one process: the entry of the tool runs with the
//! materialized package root as its working directory, the shell's own
//! environment plus `PAGIS_TOOL` and `PAGIS_PACKAGE_VERSION`, and the
//! argument object on stdin as one JSON document. Exit 0 with valid
//! JSON on stdout is the result, whatever the JSON says. Everything
//! else is an `is_error` that carries the exit code, the tail of
//! stderr and the tail of stdout. The daemon never retries.

use std::sync::Arc;

use pagis_broker::{ToolResult, WIDGET_RESULT};
use pagis_computer::{ComputerManagers, OutputCap, ShellCommand};
use pagis_core::AgentId;

use crate::materialize::{Materializer, VersionSource, quote};
use crate::widget::split_result;

/// The stdout a result may have. A larger document is refused whole:
/// half of a JSON document is worth nothing.
pub const MAX_STDOUT_BYTES: usize = 256 * 1024;
/// The stderr an error message carries.
pub const STDERR_TAIL_BYTES: usize = 16 * 1024;
/// The stdout an error message carries.
const STDOUT_TAIL_BYTES: usize = 4 * 1024;
/// The exit code of a command the in-container deadline stopped.
const EXIT_TIMED_OUT: i64 = 124;

/// One call of one tool of one Software Package.
pub struct SoftwareRunner {
    /// Every tenant's Computer manager. A tool call is one
    /// tenant's work: the manager is resolved from the Workspace of the
    /// caller on every call, so a person's tool never runs on another
    /// person's Tenant Network, volume or awake cap.
    computers: Arc<ComputerManagers>,
    materializer: Arc<Materializer>,
    source: Arc<dyn VersionSource>,
}

impl SoftwareRunner {
    pub fn new(
        computers: Arc<ComputerManagers>,
        materializer: Arc<Materializer>,
        source: Arc<dyn VersionSource>,
    ) -> Self {
        Self {
            computers,
            materializer,
            source,
        }
    }

    pub async fn run(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        agent_id: &AgentId,
        package: &str,
        version: &str,
        tool: &str,
        arguments: &serde_json::Value,
    ) -> ToolResult {
        let package_version = match self.source.version(workspace_id, package, version).await {
            Ok(found) => found,
            Err(error) => {
                return ToolResult::error(
                    "temporarily_unavailable",
                    format!("{package}@{version}: {error}"),
                );
            }
        };
        let Some(spec) = package_version.manifest.tool(tool) else {
            return ToolResult::error(
                "invalid_request",
                format!("{package}@{version} declares no tool {tool:?}"),
            );
        };
        let root = match self
            .materializer
            .ensure(
                workspace_id,
                agent_id,
                package,
                version,
                package_version.manifest.package.setup.as_deref(),
            )
            .await
        {
            Ok(root) => root,
            Err(error) => {
                return ToolResult::error(
                    "temporarily_unavailable",
                    format!("{package}@{version} is not ready: {error}"),
                );
            }
        };

        // The environment of a Software tool is the shell's own plus
        // two entries, so `bash -c` sets them in front of the process
        // it replaces itself with.
        let command = format!(
            "PAGIS_TOOL={tool} PAGIS_PACKAGE_VERSION={version} exec {entry}",
            tool = quote(tool),
            version = quote(version),
            entry = quote(&format!("{root}/{}", spec.entry)),
        );
        let outcome = match self
            .computers
            .get(workspace_id)
            .shell(
                agent_id,
                ShellCommand {
                    command,
                    timeout: spec.timeout(),
                    cwd: Some(root),
                    stdin: Some(arguments.to_string().into_bytes()),
                    output_cap: Some(OutputCap {
                        head: MAX_STDOUT_BYTES,
                        tail: STDERR_TAIL_BYTES,
                    }),
                },
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                return ToolResult::error(
                    "temporarily_unavailable",
                    format!("{package}__{tool}: {error}"),
                );
            }
        };

        if outcome.exit_code == 0 && !outcome.truncated {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(outcome.stdout.trim()) else {
                return failure(
                    package,
                    tool,
                    &outcome,
                    "the tool printed something that is not one JSON value",
                );
            };
            let mut metadata = serde_json::json!({
                "package": package,
                "version": version,
                "tool": tool,
            });
            // A Widget tool's result splits (ADR-0016): the data half
            // reaches the Widget alone, and the text half the model.
            // A data half that fails its schema renders nothing.
            let Some(name) = spec.widget.as_deref() else {
                return ToolResult::success(outcome.stdout).with_metadata(metadata);
            };
            let schema = package_version
                .widget_schemas
                .get(name)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object"}));
            return match split_result(&value, &schema) {
                Ok(split) => {
                    metadata[WIDGET_RESULT] = serde_json::json!({
                        "package": package,
                        "version": version,
                        "widget": name,
                        "structured_content": split.structured_content,
                    });
                    ToolResult::success(split.content).with_metadata(metadata)
                }
                Err(problem) => ToolResult::error(
                    "invalid_request",
                    format!("{package}__{tool} renders the widget {name:?}: {problem}"),
                ),
            };
        }
        if outcome.truncated {
            return failure(
                package,
                tool,
                &outcome,
                &format!("the tool printed more than {MAX_STDOUT_BYTES} bytes; return less"),
            );
        }
        if outcome.exit_code == EXIT_TIMED_OUT {
            return failure(
                package,
                tool,
                &outcome,
                &format!(
                    "the tool passed its deadline of {} s",
                    spec.timeout().as_secs()
                ),
            );
        }
        failure(package, tool, &outcome, "the tool failed")
    }
}

/// One failed call, as the model reads it.
fn failure(
    package: &str,
    tool: &str,
    outcome: &pagis_computer::ExecOutcome,
    reason: &str,
) -> ToolResult {
    let mut content = format!(
        "{package}__{tool}: {reason}\nexit code: {}",
        outcome.exit_code
    );
    let stderr = tail(&outcome.stderr, STDERR_TAIL_BYTES);
    if !stderr.is_empty() {
        content.push_str("\nstderr:\n");
        content.push_str(stderr);
    }
    let stdout = tail(&outcome.stdout, STDOUT_TAIL_BYTES);
    if !stdout.is_empty() {
        content.push_str("\nstdout:\n");
        content.push_str(stdout);
    }
    ToolResult::error("tool_failed", content)
}

/// The last `bytes` of a text, cut on a character boundary.
fn tail(text: &str, bytes: usize) -> &str {
    if text.len() <= bytes {
        return text;
    }
    let mut start = text.len() - bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}
