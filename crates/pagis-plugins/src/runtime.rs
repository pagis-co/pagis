//! The Plugin tools as the broker's executor.

use crate::PluginHosts;
use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, ToolExecutor, ToolResult, ToolRoute};
use std::sync::Arc;

/// The MCP hosts as the broker's executor. One host serves one tenant,
/// and the call names the tenant, so the dispatch resolves the
/// host it belongs to and never another person's.
pub struct PluginToolRuntime {
    hosts: Arc<PluginHosts>,
}

impl PluginToolRuntime {
    pub fn new(hosts: Arc<PluginHosts>) -> Self {
        Self { hosts }
    }

    /// The routes this runtime answers.
    pub fn owns(route: &ToolRoute) -> bool {
        matches!(route, ToolRoute::Plugin { .. })
    }
}

#[async_trait]
impl ToolExecutor for PluginToolRuntime {
    async fn execute(&self, call: AuthorizedCall) -> ToolResult {
        self.hosts.get(&call.workspace_id).dispatch(&call).await
    }
}
