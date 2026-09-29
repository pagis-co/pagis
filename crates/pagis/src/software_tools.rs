//! The Software Package tools (ADR-0016).
//!
//! One runtime serves every half the daemon needs: the
//! `software_publish` core tool, which mints the next Version of a
//! package from the author's own Computer; `software_fork` and the
//! three contribute-back tools; and every `ToolRoute::Software` call,
//! which runs one tool of a published Version inside the caller's
//! Computer.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{
    AuthorizedCall, Broker, CapabilityManifest, CoreTool, LOADED_PACKAGES, ToolExecutor,
    ToolResult, ToolRoute,
};
use pagis_software::{Contributions, ManifestSink, SoftwareList, SoftwareRunner, tool_search};

/// The broker as the Software List writes manifests into it. A
/// Software Package belongs to one tenant, and the Workspace comes from
/// the call, so one sink serves every tenant.
pub struct BrokerManifests(pub Arc<Broker>);

impl ManifestSink for BrokerManifests {
    fn install(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        manifest: CapabilityManifest,
    ) -> Result<(), String> {
        self.0
            .install_manifest(workspace_id, manifest)
            .map_err(|error| error.to_string())
    }
}

/// The direct channel path, as the Contribution tools post through it
/// (ADR-0016). It is the same [`AgentDm`] `send_message` uses,
/// so a Contribution reaches the author as a message from the forker
/// and wakes it the same way.
pub struct DmMessenger {
    pub dm: pagis_agent::AgentDm,
    pub runtime: Arc<pagis_agent::CoreToolRuntime>,
    pub agents: Arc<dyn pagis_core::AgentStore>,
}

#[async_trait]
impl pagis_software::AgentMessenger for DmMessenger {
    async fn post(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        from: &pagis_core::AgentId,
        to: &pagis_core::AgentId,
        run_id: &pagis_core::RunId,
        text: &str,
    ) -> Result<(), String> {
        let sender = self.agent(workspace_id, from).await?;
        let target = self.agent(workspace_id, to).await?;
        let channel = self
            .dm
            .between(workspace_id, &sender, &target, run_id)
            .await?;
        let exposures = self
            .runtime
            .retained_exposures(workspace_id, run_id, from)
            .await?;
        self.dm
            .post(workspace_id, &channel, &sender.id, run_id, text, &exposures)
            .await?;
        Ok(())
    }
}

impl DmMessenger {
    async fn agent(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        id: &pagis_core::AgentId,
    ) -> Result<pagis_core::Agent, String> {
        self.agents
            .get(workspace_id, id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("there is no agent {id}"))
    }
}

pub struct SoftwareToolRuntime {
    list: Arc<SoftwareList>,
    runner: Arc<SoftwareRunner>,
    contributions: Arc<Contributions>,
}

impl SoftwareToolRuntime {
    pub fn new(
        list: Arc<SoftwareList>,
        runner: Arc<SoftwareRunner>,
        contributions: Arc<Contributions>,
    ) -> Self {
        Self {
            list,
            runner,
            contributions,
        }
    }

    /// The routes this runtime answers.
    pub fn owns(route: &ToolRoute) -> bool {
        matches!(
            route,
            ToolRoute::Software { .. }
                | ToolRoute::Core {
                    tool: CoreTool::SoftwarePublish
                        | CoreTool::ToolSearch
                        | CoreTool::SoftwareFork
                        | CoreTool::SoftwareContribute
                        | CoreTool::ContributionView
                        | CoreTool::ContributionClose
                }
        )
    }

    /// Search the Software List. The packages the call loads
    /// come back as metadata: the run loop, not the broker, holds the
    /// loaded set of the run.
    fn search(&self, call: &AuthorizedCall) -> ToolResult {
        let query = call.arguments["query"].as_str().unwrap_or_default();
        let outcome = tool_search(&self.list.index(&call.workspace_id), query);
        ToolResult::success(outcome.text)
            .with_metadata(serde_json::json!({ LOADED_PACKAGES: outcome.packages }))
    }

    /// One text argument, trimmed. An absent one reads as empty: the
    /// broker already refused a call whose required text is empty.
    fn text<'a>(call: &'a AuthorizedCall, name: &str) -> &'a str {
        call.arguments[name].as_str().unwrap_or_default().trim()
    }

    async fn publish(&self, call: &AuthorizedCall) -> ToolResult {
        let name = call.arguments["name"].as_str().unwrap_or_default().trim();
        let notes = call.arguments["notes"].as_str().unwrap_or_default().trim();
        match self
            .list
            .publish(
                &call.workspace_id,
                &call.agent_id,
                &call.run_id,
                name,
                notes,
            )
            .await
        {
            Ok(text) => ToolResult::success(text),
            Err(problem) => ToolResult::error("invalid_request", problem),
        }
    }

    async fn fork(&self, call: &AuthorizedCall) -> ToolResult {
        match self
            .list
            .fork(
                &call.workspace_id,
                &call.agent_id,
                &call.run_id,
                Self::text(call, "source"),
                Self::text(call, "new_name"),
            )
            .await
        {
            Ok(text) => ToolResult::success(text),
            Err(problem) => ToolResult::error("invalid_request", problem),
        }
    }

    async fn contribute(&self, call: &AuthorizedCall) -> ToolResult {
        match self
            .contributions
            .contribute(
                &call.workspace_id,
                &call.agent_id,
                &call.run_id,
                Self::text(call, "package"),
                Self::text(call, "summary"),
            )
            .await
        {
            Ok(text) => ToolResult::success(text),
            Err(problem) => ToolResult::error("invalid_request", problem),
        }
    }

    async fn view(&self, call: &AuthorizedCall) -> ToolResult {
        let path = call.arguments["path"].as_str().map(str::trim);
        match self
            .contributions
            .view(
                &call.workspace_id,
                &call.agent_id,
                Self::text(call, "id"),
                path,
            )
            .await
        {
            Ok(text) => ToolResult::success(text),
            Err(problem) => ToolResult::error("invalid_request", problem),
        }
    }

    async fn close(&self, call: &AuthorizedCall) -> ToolResult {
        match self
            .contributions
            .close(
                &call.workspace_id,
                &call.agent_id,
                &call.run_id,
                Self::text(call, "id"),
                Self::text(call, "status"),
                Self::text(call, "reason"),
            )
            .await
        {
            Ok(text) => ToolResult::success(text),
            Err(problem) => ToolResult::error("invalid_request", problem),
        }
    }
}

#[async_trait]
impl ToolExecutor for SoftwareToolRuntime {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        match call.route.clone() {
            ToolRoute::Core {
                tool: CoreTool::SoftwarePublish,
            } => self.publish(&call).await,
            ToolRoute::Core {
                tool: CoreTool::ToolSearch,
            } => self.search(&call),
            ToolRoute::Core {
                tool: CoreTool::SoftwareFork,
            } => self.fork(&call).await,
            ToolRoute::Core {
                tool: CoreTool::SoftwareContribute,
            } => self.contribute(&call).await,
            ToolRoute::Core {
                tool: CoreTool::ContributionView,
            } => self.view(&call).await,
            ToolRoute::Core {
                tool: CoreTool::ContributionClose,
            } => self.close(&call).await,
            // The Version the Run may call is the one its snapshot
            // froze at run start, not the newest one (ADR-0005). The
            // snapshot carries it as the manifest's source version.
            ToolRoute::Software { package, tool } => {
                self.runner
                    .run(
                        &call.workspace_id,
                        &call.agent_id,
                        &package,
                        &call.source_version,
                        &tool,
                        &call.arguments,
                    )
                    .await
            }
            _ => ToolResult::error(
                "invalid_request",
                format!("{} is not a Software tool", call.tool_name),
            ),
        }
    }
}
