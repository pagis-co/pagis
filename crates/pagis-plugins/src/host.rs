//! The MCP host of the installed Plugins (ADR-0017).
//!
//! One host serves the Workspace. It freezes a Plugin's tool list at
//! install, gives the broker the Capability Manifest that follows from
//! it, and answers every `ToolRoute::Plugin` call by starting the
//! right server and sending one `tools/call`.
//!
//! The host is under the broker, never beside it: it is reached with
//! an `AuthorizedCall`, so the Plugin Grant and the effect class are
//! already settled. What the host checks again is what only it can
//! see: that the Plugin is enabled, that the Grants on its bound
//! Connections still carry the capabilities the package asked for
//! (ADR-0017), and that the tool is in the frozen manifest.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, CapabilityManifest, ToolResult, ToolRoute};
use tokio_util::sync::CancellationToken;

use pagis_core::{
    Connection, ConnectionStore, EventBus, GrantStore, NewEvent, Plugin, PluginBinding,
    PluginBindingValue, PluginId, PluginState, PluginStore, PluginTool, PluginToolStore,
    PluginTools, SecretStore, WorkspaceId, now_ms,
};
use pagis_plugin::{McpConfig, McpServer, PluginGitStore, PluginPackage, check_access, validate};

use crate::freeze::{capability_manifest, freeze_tools};
use crate::log::PluginLogs;
use crate::process::{ServerProcesses, container_paths};
use crate::substitute::{Bound, endpoint_of, spawn_of};
use crate::supervisor::{Launch, ServerError, ServerSupervisor, ToolListChanged, fingerprint};

/// Where a Capability Manifest goes. The broker is the only
/// implementation; the seam keeps the host testable without one.
pub trait Manifests: Send + Sync {
    fn install(
        &self,
        workspace_id: &WorkspaceId,
        manifest: CapabilityManifest,
    ) -> Result<(), String>;
}

impl Manifests for pagis_broker::Broker {
    fn install(
        &self,
        workspace_id: &WorkspaceId,
        manifest: CapabilityManifest,
    ) -> Result<(), String> {
        self.install_manifest(workspace_id, manifest)
            .map_err(|error| error.to_string())
    }
}

/// The token one bound Connection gives a plugin server (ADR-0017).
/// A provider whose tokens the daemon cannot read — Google keeps its
/// own in the keychain — answers `None`, and a Plugin bound to it does
/// not start.
#[async_trait]
pub trait ConnectionTokens: Send + Sync {
    async fn token(&self, connection: &Connection) -> Option<String>;
}

/// The tokens of a daemon that hands none out.
pub struct NoConnectionTokens;

#[async_trait]
impl ConnectionTokens for NoConnectionTokens {
    async fn token(&self, _connection: &Connection) -> Option<String> {
        None
    }
}

/// Why the host could not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("no such plugin")]
    NotFound,
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Storage(String),
}

pub struct PluginHostDeps {
    /// The tenant this host runs Plugins for: its Plugin Computer, its
    /// Grants and its broker registry.
    pub workspace_id: WorkspaceId,
    /// The Org's Workspace, which no person owns. It holds the
    /// installed Plugin rows, Bindings and checkouts (ADR-0017) and the
    /// Installation Connections a Binding names. A Plugin is installed
    /// once for the Org and every tenant runs it, so the rows are read
    /// here and the Plugin runs there.
    pub org_workspace_id: WorkspaceId,
    pub plugins: Arc<dyn PluginStore>,
    /// The frozen tool catalogs, one per Capability Manifest version.
    pub catalogs: Arc<dyn PluginToolStore>,
    pub connections: Arc<dyn ConnectionStore>,
    pub grants: Arc<dyn GrantStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub tokens: Arc<dyn ConnectionTokens>,
    pub git: Arc<PluginGitStore>,
    pub manifests: Arc<dyn Manifests>,
    pub bus: Arc<dyn EventBus>,
    /// Where a Plugin's server process runs (ADR-0017): inside the
    /// tenant's Plugin Computer, and never on the daemon host.
    pub processes: Arc<dyn ServerProcesses>,
    /// The Plugin stderr logs, one file for each Workspace and Plugin.
    /// This host writes and reads the files of its own tenant only.
    pub logs: Arc<PluginLogs>,
}

pub struct PluginHost {
    deps: PluginHostDeps,
    /// One supervisor per declared server, by Plugin and server name.
    servers: Mutex<HashMap<(PluginId, String), Arc<ServerSupervisor>>>,
    /// The package of each installed state, read once per commit: a
    /// dispatch must not walk the plugin tree again.
    packages: Mutex<HashMap<PluginId, (String, Arc<PluginPackage>)>>,
}

impl PluginHost {
    pub fn new(deps: PluginHostDeps) -> Self {
        Self {
            deps,
            servers: Mutex::new(HashMap::new()),
            packages: Mutex::new(HashMap::new()),
        }
    }

    /// Give the broker the Capability Manifest of every installed
    /// Plugin, from the frozen catalogs. The daemon calls it once at
    /// boot: no server starts for it (ADR-0005).
    pub async fn load(&self) -> Result<(), HostError> {
        let plugins = self
            .deps
            .plugins
            .list(&self.deps.org_workspace_id)
            .await
            .map_err(|error| HostError::Storage(error.to_string()))?;
        for plugin in plugins {
            let Some(frozen) = self.catalog(&plugin).await? else {
                continue;
            };
            let package = match self.package(&plugin) {
                Ok(package) => package,
                Err(error) => {
                    tracing::warn!(plugin = %plugin.name, %error, "the plugin package is not readable");
                    continue;
                }
            };
            self.install_manifest(&plugin, &package, &frozen)?;
        }
        Ok(())
    }

    /// Freeze the tool list of one installed state (ADR-0017). Each
    /// declared server starts once, answers `tools/list` and stops.
    /// The state that already has a catalog only installs its
    /// manifest, so a second call starts nothing.
    pub async fn freeze(
        &self,
        plugin: &Plugin,
        package: &PluginPackage,
        bindings: &[PluginBinding],
    ) -> Result<PluginTools, HostError> {
        if let Some(frozen) = self.catalog(plugin).await?
            && frozen.installed_commit == plugin.installed_commit
        {
            self.install_manifest(plugin, package, &frozen)?;
            return Ok(frozen);
        }
        let bound = self.bound(plugin, bindings).await?;
        let mut offered: Vec<(String, Vec<PluginTool>)> = Vec::new();
        for (name, server) in servers_of(package) {
            let supervisor = self.new_supervisor(&plugin.id, name);
            let (launch, mark) = self.launch(plugin, server, &bound)?;
            let client = supervisor
                .client(&launch, &mark, Instant::now())
                .await
                .map_err(|error| HostError::Refused(error.to_string()))?;
            let tools = supervisor
                .list_tools(&client.0)
                .await
                .map_err(|error| HostError::Refused(error.to_string()))?;
            supervisor.stop().await;
            offered.push((name.clone(), tools.iter().map(read_tool).collect()));
        }
        let tools = freeze_tools(&plugin.name, &offered)
            .map_err(|error| HostError::Refused(error.to_string()))?;
        let frozen = PluginTools {
            plugin_id: plugin.id.clone(),
            version: plugin.manifest_version.clone(),
            installed_commit: plugin.installed_commit.clone(),
            tools,
            tools_changed: false,
            created_at: now_ms(),
        };
        self.deps
            .catalogs
            .put(&frozen)
            .await
            .map_err(|error| HostError::Storage(error.to_string()))?;
        self.install_manifest(plugin, package, &frozen)?;
        Ok(frozen)
    }

    /// One `ToolRoute::Plugin` call. Every refusal is a tool result
    /// with a stable code, because the Run reads it and continues
    /// (ADR-0005).
    pub async fn dispatch(&self, call: &AuthorizedCall) -> ToolResult {
        let ToolRoute::Plugin {
            plugin,
            server,
            tool,
        } = &call.route
        else {
            return ToolResult::error("invalid_request", "this is not a plugin tool");
        };
        match self.call_tool(call, plugin, server, tool).await {
            Ok(result) => result,
            Err(result) => result,
        }
    }

    /// Start every server of one Plugin again after it failed. Only
    /// the user reaches this (ADR-0017).
    pub async fn start(&self, plugin_id: &PluginId) -> Result<(), HostError> {
        let plugin = self.plugin(plugin_id).await?;
        let package = self.package(&plugin)?;
        let bindings = self.bindings(plugin_id).await?;
        let bound = self.bound(&plugin, &bindings).await?;
        for (name, server) in servers_of(&package) {
            let supervisor = self.supervisor(&plugin.id, name);
            supervisor.clear_failures().await;
            let (launch, mark) = self.launch(&plugin, server, &bound)?;
            supervisor
                .client(&launch, &mark, Instant::now())
                .await
                .map_err(|error| HostError::Refused(error.to_string()))?;
        }
        if plugin.state == PluginState::Failed {
            self.set_state(&plugin, PluginState::Enabled).await?;
        }
        Ok(())
    }

    /// Stop every server of one Plugin.
    pub async fn stop_plugin(&self, plugin_id: &PluginId) {
        let held: Vec<Arc<ServerSupervisor>> = {
            let servers = self.servers.lock().expect("the supervisor map lock");
            servers
                .iter()
                .filter(|((id, _), _)| id == plugin_id)
                .map(|(_, supervisor)| Arc::clone(supervisor))
                .collect()
        };
        for supervisor in held {
            supervisor.stop().await;
        }
        self.servers
            .lock()
            .expect("the supervisor map lock")
            .retain(|(id, _), _| id != plugin_id);
        self.packages
            .lock()
            .expect("the package cache lock")
            .remove(plugin_id);
    }

    /// The servers of one Plugin that hold a process now. A
    /// server starts at the first dispatch and stops when it is idle,
    /// so the desk names what runs at this moment and nothing more.
    pub async fn running_servers(&self, plugin_id: &PluginId) -> BTreeSet<String> {
        let held: Vec<Arc<ServerSupervisor>> = {
            let servers = self.servers.lock().expect("the supervisor map lock");
            servers
                .iter()
                .filter(|((id, _), _)| id == plugin_id)
                .map(|(_, supervisor)| Arc::clone(supervisor))
                .collect()
        };
        let mut running = BTreeSet::new();
        for supervisor in held {
            if supervisor.is_running().await {
                running.insert(supervisor.server().to_string());
            }
        }
        running
    }

    /// Stop every server that has been unused for the idle time, and
    /// answer with how many stopped. The daemon calls it on a timer.
    pub async fn reap_idle(&self, now: Instant) -> usize {
        let held: Vec<Arc<ServerSupervisor>> = self
            .servers
            .lock()
            .expect("the supervisor map lock")
            .values()
            .map(Arc::clone)
            .collect();
        let mut stopped = 0;
        for supervisor in held {
            if supervisor.stop_if_idle(now).await {
                stopped += 1;
            }
        }
        stopped
    }

    /// Where the server output of one Plugin goes in this host's
    /// Workspace.
    pub fn log_path(&self, plugin_id: &PluginId) -> PathBuf {
        self.deps.logs.path(&self.deps.workspace_id, plugin_id)
    }

    /// The end of one Plugin's log in this host's Workspace, at most
    /// `limit` bytes. Only that end is read from the file, and no log of
    /// another Workspace is read (ADR-0023).
    pub async fn read_log(&self, plugin_id: &PluginId, limit: usize) -> String {
        self.deps
            .logs
            .tail(&self.deps.workspace_id, plugin_id, limit)
            .await
    }

    /// The frozen catalog of the state that is installed now.
    pub async fn catalog(&self, plugin: &Plugin) -> Result<Option<PluginTools>, HostError> {
        self.deps
            .catalogs
            .get(&plugin.workspace_id, &plugin.id, &plugin.manifest_version)
            .await
            .map_err(|error| HostError::Storage(error.to_string()))
    }

    async fn call_tool(
        &self,
        call: &AuthorizedCall,
        plugin_id: &str,
        server: &str,
        tool: &str,
    ) -> Result<ToolResult, ToolResult> {
        let plugin_id = PluginId::from(plugin_id.to_string());
        let plugin = self
            .plugin(&plugin_id)
            .await
            .map_err(|error| unavailable(&error.to_string()))?;
        let bindings = self
            .bindings(&plugin_id)
            .await
            .map_err(|error| unavailable(&error.to_string()))?;
        let grants = self
            .deps
            .grants
            .list_live_for_agent(&call.workspace_id, &call.agent_id)
            .await
            .map_err(|error| unavailable(&error.to_string()))?;
        // A `connection` Binding names an Installation Connection, so
        // whether it is connected is read from the Org's Workspace.
        let connections = self
            .deps
            .connections
            .list(&self.deps.org_workspace_id)
            .await
            .map_err(|error| unavailable(&error.to_string()))?;
        if let Err(denied) = check_access(&plugin, &bindings, &grants, &connections) {
            return Err(ToolResult::error("permission_revoked", denied.to_string()));
        }
        let frozen = self
            .catalog(&plugin)
            .await
            .map_err(|error| unavailable(&error.to_string()))?
            .ok_or_else(|| unavailable("the plugin has no frozen tool list"))?;
        if frozen.tool(tool).is_none() {
            return Err(ToolResult::error(
                "tool_not_in_manifest",
                format!(
                    "the plugin {:?} no longer offers {tool:?}; accept the change to install it",
                    plugin.name
                ),
            ));
        }
        let package = self
            .package(&plugin)
            .map_err(|error| unavailable(&error.to_string()))?;
        let declared = servers_of(&package)
            .into_iter()
            .find(|(name, _)| name.as_str() == server)
            .map(|(_, declared)| declared.clone())
            .ok_or_else(|| unavailable("the plugin no longer declares this server"))?;
        let bound = self
            .bound(&plugin, &bindings)
            .await
            .map_err(|error| unavailable(&error.to_string()))?;
        let (launch, mark) = self
            .launch(&plugin, &declared, &bound)
            .map_err(|error| unavailable(&error.to_string()))?;

        let supervisor = self.supervisor(&plugin.id, server);
        let (client, started) = match supervisor.client(&launch, &mark, Instant::now()).await {
            Ok(client) => client,
            Err(ServerError::Failed(name)) => {
                self.fail(&plugin, &name).await;
                return Err(unavailable(&format!(
                    "the plugin {:?} failed three times in a row; start it again from the desk",
                    plugin.name
                )));
            }
            Err(error) => return Err(unavailable(&error.to_string())),
        };
        if started {
            self.compare_lists(&plugin, &frozen, &supervisor, &client)
                .await;
        }
        let timeout = call.call_timeout.unwrap_or(crate::DEFAULT_CALL_TIMEOUT);
        let answer = supervisor
            .call(&client, tool, call.arguments.clone(), timeout)
            .await;
        match answer {
            Ok(result) => Ok(tool_result(result)),
            Err(error @ ServerError::Unknown(_)) => {
                Err(ToolResult::error("outcome_unknown", error.to_string()))
            }
            Err(error @ ServerError::Overrun { failed, .. }) => {
                if failed {
                    self.fail(&plugin, server).await;
                }
                Err(ToolResult::error("outcome_unknown", error.to_string()))
            }
            Err(error) => Err(unavailable(&error.to_string())),
        }
    }

    /// Compare what a started server offers with the frozen list. A
    /// difference changes nothing installed: it marks the Plugin, and
    /// the user accepts it through the update flow (ADR-0017).
    async fn compare_lists(
        &self,
        plugin: &Plugin,
        frozen: &PluginTools,
        supervisor: &ServerSupervisor,
        client: &crate::supervisor::Client,
    ) {
        let Ok(live) = supervisor.list_tools(client).await else {
            return;
        };
        let mut offered: Vec<String> = live.iter().map(|tool| tool.name.to_string()).collect();
        offered.sort();
        let mut held: Vec<String> = frozen
            .tools
            .iter()
            .filter(|tool| tool.server == supervisor.server())
            .map(|tool| tool.name.clone())
            .collect();
        held.sort();
        if offered != held {
            mark_changed(
                self.deps.catalogs.as_ref(),
                self.deps.bus.as_ref(),
                plugin,
                frozen,
            )
            .await;
        }
    }

    /// Mark the Plugin `failed` after three start failures or three
    /// output overruns in a row.
    async fn fail(&self, plugin: &Plugin, server: &str) {
        if plugin.state == PluginState::Failed {
            return;
        }
        if let Err(error) = self
            .deps
            .plugins
            .set_state(
                &plugin.workspace_id,
                &plugin.id,
                PluginState::Failed,
                now_ms(),
            )
            .await
        {
            tracing::warn!(%error, plugin = %plugin.name, "cannot mark the plugin failed");
            return;
        }
        self.publish(
            plugin,
            "plugin.failed",
            serde_json::json!({ "server": server }),
        )
        .await;
    }

    async fn set_state(&self, plugin: &Plugin, state: PluginState) -> Result<(), HostError> {
        self.deps
            .plugins
            .set_state(&plugin.workspace_id, &plugin.id, state, now_ms())
            .await
            .map_err(|error| HostError::Storage(error.to_string()))?;
        self.publish(
            plugin,
            "plugin.state_changed",
            serde_json::json!({ "state": state.as_str() }),
        )
        .await;
        Ok(())
    }

    async fn publish(&self, plugin: &Plugin, event_type: &str, extra: serde_json::Value) {
        publish(self.deps.bus.as_ref(), plugin, event_type, extra).await;
    }

    fn install_manifest(
        &self,
        plugin: &Plugin,
        package: &PluginPackage,
        frozen: &PluginTools,
    ) -> Result<(), HostError> {
        let manifest = capability_manifest(plugin.id.as_str(), &plugin.name, package, frozen);
        // The Plugin row is the Org's, and this host serves one tenant:
        // the manifest reaches that tenant's own registry and no other
        // tenant's (ADR-0005).
        self.deps
            .manifests
            .install(&self.deps.workspace_id, manifest)
            .map_err(HostError::Refused)
    }

    fn supervisor(&self, plugin_id: &PluginId, server: &str) -> Arc<ServerSupervisor> {
        let mut servers = self.servers.lock().expect("the supervisor map lock");
        Arc::clone(
            servers
                .entry((plugin_id.clone(), server.to_string()))
                .or_insert_with(|| self.new_supervisor(plugin_id, server)),
        )
    }

    /// A supervisor of one declared server of one Plugin, which marks
    /// the Plugin when the running server says that its tool list
    /// changed.
    fn new_supervisor(&self, plugin_id: &PluginId, server: &str) -> Arc<ServerSupervisor> {
        let changed = MarkChanged {
            org_workspace_id: self.deps.org_workspace_id.clone(),
            plugin_id: plugin_id.clone(),
            plugins: Arc::clone(&self.deps.plugins),
            catalogs: Arc::clone(&self.deps.catalogs),
            bus: Arc::clone(&self.deps.bus),
        };
        Arc::new(ServerSupervisor::new(
            server,
            self.deps.workspace_id.clone(),
            Arc::clone(&self.deps.processes),
            self.deps.logs.log(&self.deps.workspace_id, plugin_id),
            Arc::new(changed),
        ))
    }

    /// How one declared server is reached, and the mark that says
    /// which bound values it was started with.
    fn launch(
        &self,
        plugin: &Plugin,
        server: &McpServer,
        bound: &Bound,
    ) -> Result<(Launch, String), HostError> {
        let paths = self
            .deps
            .git
            .paths(&self.deps.org_workspace_id, plugin.id.as_str());
        Ok(match server {
            McpServer::Stdio { .. } => {
                // A stdio server runs inside the tenant's Plugin
                // Computer, so the two directories it reads are
                // the ones the container sees and not the daemon's.
                let paths = container_paths(plugin.id.as_str());
                let spawn = spawn_of(server, &paths, bound)
                    .map_err(|error| HostError::Refused(error.to_string()))?;
                let mark = fingerprint(&spawn.env);
                (Launch::Stdio(spawn), mark)
            }
            McpServer::StreamableHttp { .. } | McpServer::Sse { .. } => {
                let endpoint = endpoint_of(server, &paths, bound)
                    .map_err(|error| HostError::Refused(error.to_string()))?;
                let mut marked = endpoint.headers.clone();
                marked.insert("url".to_string(), endpoint.url.clone());
                let mark = fingerprint(&marked);
                (Launch::Http(endpoint), mark)
            }
        })
    }

    /// The value of every bound field now. A secret comes from the
    /// secret store and a Connection token from the provider, so a
    /// rotated token is read here and nowhere else (ADR-0017).
    async fn bound(&self, plugin: &Plugin, bindings: &[PluginBinding]) -> Result<Bound, HostError> {
        let mut values = BTreeMap::new();
        for binding in bindings {
            match &binding.value {
                PluginBindingValue::Secret { secret_name } => {
                    if let Ok(Some(secret)) = self.deps.secrets.get(secret_name) {
                        values.insert(binding.field.clone(), secret);
                    }
                }
                PluginBindingValue::Connection { connection_id, .. } => {
                    let connection = self.bound_connection(connection_id).await?;
                    if let Some(connection) = connection
                        && let Some(token) = self.deps.tokens.token(&connection).await
                    {
                        values.insert(binding.field.clone(), token);
                    }
                }
                PluginBindingValue::Value { value } => {
                    let text = match value {
                        serde_json::Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    values.insert(binding.field.clone(), text);
                }
            }
        }
        let _ = plugin;
        Ok(Bound::new(values))
    }

    /// The Connection a Binding names (ADR-0017).
    ///
    /// The administrator installs and binds a Plugin once for the Org,
    /// so a `connection` Binding names an Installation Connection of the
    /// Org's Workspace. That Connection is the Org's, not a person's, so
    /// every tenant's server gets its token. A Connection that is gone
    /// binds nothing, and the server starts without the field, as it
    /// does for a missing token.
    async fn bound_connection(
        &self,
        connection_id: &pagis_core::ConnectionId,
    ) -> Result<Option<pagis_core::Connection>, HostError> {
        self.deps
            .connections
            .get(&self.deps.org_workspace_id, connection_id)
            .await
            .map_err(|error| HostError::Storage(error.to_string()))
    }

    /// The package of the installed state, read once per commit.
    fn package(&self, plugin: &Plugin) -> Result<Arc<PluginPackage>, HostError> {
        if let Some((commit, package)) = self
            .packages
            .lock()
            .expect("the package cache lock")
            .get(&plugin.id)
            && commit == &plugin.installed_commit
        {
            return Ok(Arc::clone(package));
        }
        let paths = self
            .deps
            .git
            .paths(&self.deps.org_workspace_id, plugin.id.as_str());
        let package =
            Arc::new(validate(&paths.root).map_err(|error| HostError::Refused(error.to_string()))?);
        self.packages
            .lock()
            .expect("the package cache lock")
            .insert(
                plugin.id.clone(),
                (plugin.installed_commit.clone(), Arc::clone(&package)),
            );
        Ok(package)
    }

    async fn plugin(&self, id: &PluginId) -> Result<Plugin, HostError> {
        self.deps
            .plugins
            .get(&self.deps.org_workspace_id, id)
            .await
            .map_err(|error| HostError::Storage(error.to_string()))?
            .ok_or(HostError::NotFound)
    }

    async fn bindings(&self, id: &PluginId) -> Result<Vec<PluginBinding>, HostError> {
        self.deps
            .plugins
            .list_bindings(&self.deps.org_workspace_id, id)
            .await
            .map_err(|error| HostError::Storage(error.to_string()))
    }
}

/// The host as the install path reaches it (ADR-0017). An
/// update and an uninstall stop the servers before the files move, and
/// an install freezes the tool list before the Plugin is offered.
impl PluginHost {
    /// The freeze as the install path asks for it (ADR-0017): the
    /// tool list of one installed state, frozen once.
    pub async fn freeze_for_install(
        &self,
        plugin: &Plugin,
        package: &PluginPackage,
        bindings: &[PluginBinding],
    ) -> Result<(), String> {
        PluginHost::freeze(self, plugin, package, bindings)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Drop everything this host holds about one Plugin: the servers, the
    /// frozen lists, and the server logs of every Workspace, because an
    /// uninstall removes the Plugin from every Workspace.
    pub async fn forget_plugin(&self, plugin_id: &PluginId) {
        self.stop_plugin(plugin_id).await;
        if let Err(error) = self
            .deps
            .catalogs
            .delete(&self.deps.org_workspace_id, plugin_id)
            .await
        {
            tracing::warn!(%error, plugin = %plugin_id, "cannot delete the frozen tool lists");
        }
        self.deps.logs.remove(plugin_id).await;
    }
}

/// Marks one Plugin when its running server sends
/// `notifications/tools/list_changed`. It reads the installed state when
/// the notification arrives, so the mark goes to the frozen list that is
/// in force then (ADR-0017).
struct MarkChanged {
    org_workspace_id: WorkspaceId,
    plugin_id: PluginId,
    plugins: Arc<dyn PluginStore>,
    catalogs: Arc<dyn PluginToolStore>,
    bus: Arc<dyn EventBus>,
}

#[async_trait]
impl ToolListChanged for MarkChanged {
    async fn tool_list_changed(&self) {
        let plugin = match self
            .plugins
            .get(&self.org_workspace_id, &self.plugin_id)
            .await
        {
            Ok(Some(plugin)) => plugin,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, plugin = %self.plugin_id, "cannot read the plugin whose tools changed");
                return;
            }
        };
        // A server that speaks during the freeze of a new state has no
        // frozen list yet, so there is nothing to mark.
        let frozen = match self
            .catalogs
            .get(&plugin.workspace_id, &plugin.id, &plugin.manifest_version)
            .await
        {
            Ok(Some(frozen)) => frozen,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, plugin = %plugin.name, "cannot read the frozen tool list");
                return;
            }
        };
        mark_changed(self.catalogs.as_ref(), self.bus.as_ref(), &plugin, &frozen).await;
    }
}

/// Record that the live tool list of a Plugin differs from its frozen
/// list, and say so on the bus. A list that is marked already stays
/// marked, with no second event.
async fn mark_changed(
    catalogs: &dyn PluginToolStore,
    bus: &dyn EventBus,
    plugin: &Plugin,
    frozen: &PluginTools,
) {
    if frozen.tools_changed {
        return;
    }
    if let Err(error) = catalogs
        .set_tools_changed(&plugin.workspace_id, &plugin.id, &frozen.version, true)
        .await
    {
        tracing::warn!(%error, plugin = %plugin.name, "cannot record the changed tool list");
        return;
    }
    publish(bus, plugin, "plugin.tools_changed", serde_json::json!({})).await;
}

/// Publish one event about a Plugin. The payload names the Plugin, and
/// `extra` adds its own fields.
async fn publish(bus: &dyn EventBus, plugin: &Plugin, event_type: &str, extra: serde_json::Value) {
    let mut payload = serde_json::json!({
        "plugin_id": plugin.id.as_str(),
        "name": plugin.name,
    });
    if let (Some(payload), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        payload.extend(extra.clone());
    }
    let event = NewEvent {
        workspace_id: plugin.workspace_id.clone(),
        event_type: event_type.to_string(),
        agent_id: None,
        run_id: None,
        channel_id: None,
        payload,
    };
    if let Err(error) = bus.publish(event).await {
        tracing::warn!(%error, plugin = %plugin.name, "cannot publish the plugin event");
    }
}

/// The declared servers, in `mcp.json` order. A package with no
/// `mcp.json` declares none, which is legal (ADR-0017).
fn servers_of(package: &PluginPackage) -> Vec<(&String, &McpServer)> {
    package
        .mcp
        .as_ref()
        .map(|mcp: &McpConfig| mcp.servers.iter().collect())
        .unwrap_or_default()
}

/// One tool of a server, as the freeze records it. The server name is
/// filled in by the freeze.
fn read_tool(tool: &rmcp::model::Tool) -> PluginTool {
    PluginTool {
        server: String::new(),
        name: tool.name.to_string(),
        description: tool
            .description
            .as_ref()
            .map(|text| text.to_string())
            .unwrap_or_default(),
        schema: serde_json::Value::Object((*tool.input_schema).clone()),
    }
}

/// One MCP answer, as the Run reads it. Only text reaches the model:
/// an image or an embedded resource is named and not carried, because
/// a tool result is one string in the Run's messages.
fn tool_result(result: rmcp::model::CallToolResult) -> ToolResult {
    use rmcp::model::ContentBlock;

    let text = result
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.text.clone(),
            ContentBlock::Image(_) => "[an image the plugin returned]".to_string(),
            ContentBlock::Audio(_) => "[audio the plugin returned]".to_string(),
            ContentBlock::Resource(_) | ContentBlock::ResourceLink(_) => {
                "[a resource the plugin returned]".to_string()
            }
            _ => "[content Pagis does not read]".to_string(),
        })
        .collect::<Vec<String>>()
        .join("\n");
    if result.is_error.unwrap_or(false) {
        return ToolResult::plain_error("tool_error", text);
    }
    ToolResult::success(text)
}

fn unavailable(message: &str) -> ToolResult {
    ToolResult::error("temporarily_unavailable", message)
}

/// The idle reaper of the daemon: it stops every plugin server that
/// has been unused for [`crate::IDLE_SHUTDOWN`].
pub fn spawn_idle_reaper(host: Arc<PluginHost>, every: Duration, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        while cancel.run_until_cancelled(ticker.tick()).await.is_some() {
            host.reap_idle(Instant::now()).await;
        }
    });
}
