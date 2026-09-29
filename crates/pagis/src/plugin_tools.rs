//! The Plugin tools (ADR-0017).
//!
//! The MCP host answers every `ToolRoute::Plugin` call, and this file
//! is the two seams the daemon fills in for it: the executor the
//! broker dispatches through, and the Connection tokens a bound
//! Connection hands to a server.

use std::sync::Arc;

use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, ToolExecutor, ToolResult, ToolRoute};
use pagis_core::{Connection, SecretStore};
use pagis_plugins::{ConnectionTokens, PluginHosts};

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

/// The token a bound Connection gives a plugin server (ADR-0017).
///
/// The carrier Connection holds an API key in the daemon's secret
/// store, so its token can be read. Google keeps its tokens in the OS
/// keychain behind `gog`, which has no seam that hands one out, so a
/// Plugin bound to a Google Connection does not start. That is the
/// honest answer: a server started without its credential fails in a
/// way nobody can read.
pub struct WorkspaceConnectionTokens {
    pub secrets: Arc<dyn SecretStore>,
}

#[async_trait]
impl ConnectionTokens for WorkspaceConnectionTokens {
    async fn token(&self, connection: &Connection) -> Option<String> {
        if !pagis_telephony::is_carrier(&connection.provider) {
            return None;
        }
        self.secrets
            .get(&pagis_telephony::carrier_key_secret_name(
                &connection.provider,
                &connection.alias,
            ))
            .ok()
            .flatten()
    }
}

/// The tenant's Plugin Computer, as the MCP host reaches it
/// (ADR-0017).
///
/// A Plugin's server process runs inside the container of the tenant that
/// calls the Plugin, and never on the daemon host. The container carries
/// one read-only mount for each of the Org's Plugin checkouts, so
/// `${PLUGIN_ROOT}` is a real directory inside it;
/// `${PLUGIN_DATA}` is a directory of the container's own volume, which
/// survives a stop and needs no writable host mount.
pub struct ComputerServerProcesses {
    computers: Arc<pagis_computer::ComputerManagers>,
    plugins: Arc<dyn pagis_core::PluginStore>,
    git: Arc<pagis_plugin::PluginGitStore>,
    /// The Org's Workspace, where the installed Plugins and their
    /// checkouts live (ADR-0017). The mount set comes from here; the
    /// container the mounts go into is the running tenant's.
    org_workspace_id: pagis_core::WorkspaceId,
}

impl ComputerServerProcesses {
    pub fn new(
        computers: Arc<pagis_computer::ComputerManagers>,
        plugins: Arc<dyn pagis_core::PluginStore>,
        git: Arc<pagis_plugin::PluginGitStore>,
        org_workspace_id: pagis_core::WorkspaceId,
    ) -> Self {
        Self {
            computers,
            plugins,
            git,
            org_workspace_id,
        }
    }

    /// The mount set of a tenant's Plugin Computer: the checkout of every
    /// Plugin the Org installed, read-only. Every tenant mounts the same
    /// set, because the Org holds one version of each Plugin. It holds
    /// every Plugin and not only the one that starts now, so one server
    /// starting does not replace the container under another server. An
    /// install or an uninstall changes the set, and the container is
    /// replaced at the next start, the way a Grant replaces an Agent's
    /// Computer (ADR-0017).
    async fn mounts(&self) -> Result<Vec<pagis_computer::BindMount>, String> {
        let installed = self
            .plugins
            .list(&self.org_workspace_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut mounts: Vec<pagis_computer::BindMount> = installed
            .iter()
            .map(|plugin| {
                let root = self
                    .git
                    .paths(&self.org_workspace_id, plugin.id.as_str())
                    .root;
                pagis_computer::BindMount {
                    host: root,
                    container: format!("{}/{}", pagis_plugins::CONTAINER_PLUGIN_ROOT, plugin.id),
                    read_only: true,
                }
            })
            .filter(|mount| mount.host.is_dir())
            .collect();
        mounts.sort_by(|left, right| left.container.cmp(&right.container));
        Ok(mounts)
    }
}

#[async_trait]
impl pagis_plugins::ServerProcesses for ComputerServerProcesses {
    async fn start(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
        spawn: &pagis_plugins::Spawn,
    ) -> Result<pagis_plugins::ServerIo, String> {
        let manager = self.computers.get(workspace_id);
        let mounts = self.mounts().await?;
        let computer = manager
            .ensure_plugin_computer(mounts)
            .await
            .map_err(|error| error.to_string())?;
        // The plugin's writable directory belongs to the uid that runs
        // the server, and it lives in the container's own volume.
        if let Some(data) = spawn.env.get(pagis_plugins::substitute::PLUGIN_DATA) {
            let outcome = manager
                .shell(
                    &pagis_computer::plugin_agent(),
                    pagis_computer::ShellCommand {
                        command: format!("mkdir -p {}", shell_quote(data)),
                        timeout: std::time::Duration::from_secs(30),
                        cwd: None,
                        stdin: None,
                        output_cap: None,
                    },
                )
                .await
                .map_err(|error| error.to_string())?;
            if outcome.exit_code != 0 {
                return Err(format!(
                    "the plugin data directory could not be made: {}",
                    outcome.stderr.trim()
                ));
            }
        }
        let mut argv = vec![spawn.program.clone()];
        argv.extend(spawn.args.iter().cloned());
        let stream = manager
            .plugin_server(
                &computer,
                pagis_computer::ExecRequest {
                    argv,
                    // Not root: a plugin server is third-party code, and
                    // it runs as the unprivileged uid of the container.
                    user: SERVER_USER.to_string(),
                    cwd: spawn.cwd.display().to_string(),
                    env: spawn
                        .env
                        .iter()
                        .map(|(name, value)| format!("{name}={value}"))
                        .collect(),
                    stdin: None,
                    // A stream has no head and tail cap. The supervisor
                    // reads it to the end of the process, with a cap on
                    // each stdout message and a log of a fixed size.
                    output_cap: pagis_computer::OutputCap { head: 0, tail: 0 },
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(pagis_plugins::ServerIo {
            stdin: stream.stdin,
            stdout: stream.stdout,
            stderr: stream.stderr,
        })
    }
}

/// The uid one plugin server runs as inside the Plugin Computer.
const SERVER_USER: &str = "agent";

/// One path as a single shell word.
fn shell_quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', r"'\''"))
}
