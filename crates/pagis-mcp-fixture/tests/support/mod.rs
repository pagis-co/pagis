//! The daemon around one installed fixture plugin (ADR-0017).
//!
//! The harness is the whole install path over in-memory stores: a real
//! git store, a real `Plugins`, and a real [`PluginHost`] whose only
//! fake is the sink the Capability Manifests go to. What the tests
//! drive is therefore what the daemon drives.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_broker::{AuthorizedCall, CapabilityManifest, ToolResult, ToolRoute};
use pagis_core::{
    AgentId, EventBus, EventScope, EventStream, MemorySecretStore, NewEvent, Plugin, PluginId,
    PluginStore, PluginTools, RunId, SecretStore, StoreError, WorkspaceId, now_ms,
};
use pagis_plugin::{
    BindingInput, BindingValueInput, Namespaces, PluginGitStore, Plugins, PluginsDeps, SourceInput,
};
use pagis_plugins::{
    CONTAINER_PLUGIN_DATA, CONTAINER_PLUGIN_ROOT, ConnectionTokens, Manifests, NoConnectionTokens,
    PluginHost, PluginHostDeps, PluginLogs, ServerIo, ServerProcesses, Spawn,
};
use pagis_testkit::{
    MemoryConnectionStore, MemoryGrantStore, MemoryPluginStore, MemoryPluginToolStore,
};

/// A bus that keeps the kind of each event, so a test can read what the
/// host published and wait for one kind.
pub struct RecordingBus(tokio::sync::watch::Sender<Vec<String>>);

impl Default for RecordingBus {
    fn default() -> Self {
        Self(tokio::sync::watch::Sender::new(Vec::new()))
    }
}

impl RecordingBus {
    /// The kinds of the published events, in order.
    pub fn kinds(&self) -> Vec<String> {
        self.0.borrow().clone()
    }

    /// Wait until an event of `kind` is published. An event that is
    /// already published ends the wait at once.
    pub async fn published(&self, kind: &str) {
        self.0
            .subscribe()
            .wait_for(|kinds| kinds.iter().any(|held| held == kind))
            .await
            .expect("the bus lives as long as the harness");
    }
}

#[async_trait]
impl EventBus for RecordingBus {
    async fn publish(&self, event: NewEvent) -> Result<pagis_core::Event, StoreError> {
        self.0
            .send_modify(|kinds| kinds.push(event.event_type.clone()));
        Ok(pagis_core::Event {
            id: pagis_core::EventId::generate(),
            seq: 1,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: now_ms(),
        })
    }

    async fn subscribe(&self, _scope: EventScope, _after_seq: Option<i64>) -> EventStream {
        Box::pin(futures::stream::empty())
    }
}

/// The one tenant's host as the install path reaches it. The daemon uses
/// `PluginHosts`, which holds one host per tenant; this harness
/// drives one tenant, so one host is the whole set.
pub struct OneHost(pub Arc<PluginHost>);

#[async_trait]
impl pagis_plugin::PluginServers for OneHost {
    async fn freeze(
        &self,
        plugin: &pagis_core::Plugin,
        package: &pagis_plugin::PluginPackage,
        bindings: &[pagis_core::PluginBinding],
    ) -> Result<(), String> {
        self.0.freeze_for_install(plugin, package, bindings).await
    }

    async fn stop(&self, plugin_id: &pagis_core::PluginId) {
        self.0.stop_plugin(plugin_id).await;
    }

    async fn forget(&self, plugin_id: &pagis_core::PluginId) {
        self.0.forget_plugin(plugin_id).await;
    }
}

/// The Capability Manifests the freeze installed.
#[derive(Default)]
pub struct HeldManifests(pub Mutex<Vec<CapabilityManifest>>);

impl Manifests for HeldManifests {
    fn install(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        manifest: CapabilityManifest,
    ) -> Result<(), String> {
        self.0.lock().expect("manifest lock").push(manifest);
        Ok(())
    }
}

/// The tenant's Plugin Computer, as this harness stands in for it
/// (ADR-0017).
///
/// The daemon starts no server process of its own: it asks the
/// container. This double is the container: it runs the command the
/// daemon asked for, and it maps the two in-container directories back
/// to the host directories the test wrote, which is the translation the
/// bind mounts do in production. It records every start, so a test can
/// read what the daemon asked the container for.
#[derive(Default)]
pub struct PluginComputerStandIn {
    /// The plugin git root and the tenant, for the path translation.
    plugins_root: Mutex<Option<(PathBuf, WorkspaceId)>>,
    /// Every start the daemon asked for: program, args, cwd, env.
    pub starts: Mutex<Vec<StartedServer>>,
}

/// How many stderr chunks wait for the supervisor at most, as in the
/// exec stream of a real Plugin Computer.
const STDERR_CHUNKS: usize = 16;

/// What one start of a server carried.
#[derive(Debug, Clone)]
pub struct StartedServer {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: std::collections::BTreeMap<String, String>,
}

impl PluginComputerStandIn {
    fn translate(&self, text: &str) -> String {
        let held = self.plugins_root.lock().expect("the stand-in lock");
        let Some((root, workspace_id)) = held.as_ref() else {
            return text.to_string();
        };
        let workspace = root.join(workspace_id.as_str());
        let mut out = text.replace(CONTAINER_PLUGIN_ROOT, &workspace.display().to_string());
        // The writable directory of a Plugin is `<id>.data` on the host
        // and `<data root>/<id>` in the container.
        while let Some(start) = out.find(CONTAINER_PLUGIN_DATA) {
            let rest = &out[start + CONTAINER_PLUGIN_DATA.len()..];
            let rest = rest.strip_prefix('/').unwrap_or(rest);
            let end = rest
                .find(|character: char| character == '/' || character.is_whitespace())
                .unwrap_or(rest.len());
            let plugin = &rest[..end];
            let host = workspace.join(format!("{plugin}.data"));
            let replaced = format!("{}{}{}", &out[..start], host.display(), &rest[end..]);
            out = replaced;
        }
        out
    }
}

#[async_trait]
impl ServerProcesses for PluginComputerStandIn {
    async fn start(&self, _workspace_id: &WorkspaceId, spawn: &Spawn) -> Result<ServerIo, String> {
        let program = self.translate(&spawn.program);
        let args: Vec<String> = spawn.args.iter().map(|arg| self.translate(arg)).collect();
        let cwd = self.translate(&spawn.cwd.display().to_string());
        let env: std::collections::BTreeMap<String, String> = spawn
            .env
            .iter()
            .map(|(name, value)| (name.clone(), self.translate(value)))
            .collect();
        // What the daemon asked for, before the translation: a test
        // reads the container paths the daemon named.
        self.starts
            .lock()
            .expect("the stand-in lock")
            .push(StartedServer {
                program: spawn.program.clone(),
                args: spawn.args.clone(),
                cwd: spawn.cwd.display().to_string(),
                env: spawn.env.clone(),
            });
        // The real adapter makes the plugin's writable directory inside
        // the container before the server starts.
        if let Some(data) = env.get("PLUGIN_DATA") {
            std::fs::create_dir_all(data).map_err(|error| error.to_string())?;
        }
        let mut command = tokio::process::Command::new(&program);
        command
            .args(&args)
            .env_clear()
            .envs(&env)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().expect("the child has stdin");
        let stdout = child.stdout.take().expect("the child has stdout");
        let mut stderr = child.stderr.take().expect("the child has stderr");
        // The channel is bounded as the exec stream's is, so a server
        // that writes faster than the log takes it waits.
        let (sender, receiver) = tokio::sync::mpsc::channel(STDERR_CHUNKS);
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buffer = vec![0u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 || sender.send(buffer[..read].to_vec()).await.is_err() {
                    break;
                }
            }
        });
        // The child lives as long as its streams do.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
        Ok(ServerIo {
            stdin: Box::pin(stdin),
            stdout: Box::pin(stdout),
            stderr: receiver,
        })
    }
}

struct NoNamespaces;

impl Namespaces for NoNamespaces {
    fn holds(&self, _workspace_id: &pagis_core::WorkspaceId, _name: &str) -> bool {
        false
    }
}

pub struct Harness {
    pub plugins: Plugins,
    /// The Plugin logs, which every tenant's host of this harness
    /// shares, as the daemon's hosts do.
    pub logs: Arc<PluginLogs>,
    tokens: Arc<dyn ConnectionTokens>,
    /// The tenant's Plugin Computer, as this harness stands in for it.
    pub computer: Arc<PluginComputerStandIn>,
    pub host: Arc<PluginHost>,
    pub manifests: Arc<HeldManifests>,
    pub bus: Arc<RecordingBus>,
    pub secrets: Arc<MemorySecretStore>,
    pub store: Arc<MemoryPluginStore>,
    pub catalogs: Arc<MemoryPluginToolStore>,
    pub git: Arc<PluginGitStore>,
    pub connections: Arc<MemoryConnectionStore>,
    pub grants: Arc<MemoryGrantStore>,
    /// The Org's Workspace, where the Plugin is installed and bound.
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    home: tempfile::TempDir,
}

impl Harness {
    /// One tenant: the Org's Plugins and the running tenant's are the
    /// same Workspace (ADR-0017).
    pub fn new() -> Self {
        let workspace_id = WorkspaceId::generate();
        Self::serving(
            workspace_id.clone(),
            workspace_id,
            Arc::new(NoConnectionTokens),
        )
    }

    /// A host that runs the Org's Plugin for a tenant that is not the
    /// Org's Workspace, and hands a bound Connection's token out.
    pub fn for_a_tenant(tokens: Arc<dyn ConnectionTokens>) -> Self {
        Self::serving(WorkspaceId::generate(), WorkspaceId::generate(), tokens)
    }

    fn serving(
        workspace_id: WorkspaceId,
        tenant_id: WorkspaceId,
        tokens: Arc<dyn ConnectionTokens>,
    ) -> Self {
        let home = tempfile::tempdir().expect("a data root");
        let grants = Arc::new(MemoryGrantStore::default());
        let connections = Arc::new(MemoryConnectionStore::new(Arc::clone(&grants) as _));
        let store = Arc::new(MemoryPluginStore::new(Arc::clone(&grants) as _));
        let catalogs = Arc::new(MemoryPluginToolStore::default());
        let secrets = Arc::new(MemorySecretStore::default());
        let git = Arc::new(PluginGitStore::new(home.path().join("plugins")));
        let manifests = Arc::new(HeldManifests::default());
        let bus = Arc::new(RecordingBus::default());
        let computer = Arc::new(PluginComputerStandIn::default());
        *computer.plugins_root.lock().expect("the stand-in lock") =
            Some((home.path().join("plugins"), workspace_id.clone()));
        let logs = Arc::new(PluginLogs::new(home.path().join("logs").join("plugins")));
        let host = Arc::new(PluginHost::new(PluginHostDeps {
            workspace_id: tenant_id,
            org_workspace_id: workspace_id.clone(),
            plugins: Arc::clone(&store) as _,
            catalogs: Arc::clone(&catalogs) as _,
            connections: Arc::clone(&connections) as _,
            grants: Arc::clone(&grants) as _,
            secrets: Arc::clone(&secrets) as _,
            tokens: Arc::clone(&tokens),
            git: Arc::clone(&git),
            manifests: Arc::clone(&manifests) as _,
            bus: Arc::clone(&bus) as _,
            processes: Arc::clone(&computer) as _,
            logs: Arc::clone(&logs),
        }));
        let plugins = Plugins::new(PluginsDeps {
            plugins: Arc::clone(&store) as _,
            connections: Arc::clone(&connections) as _,
            grants: Arc::clone(&grants) as _,
            secrets: Arc::clone(&secrets) as _,
            git: Arc::clone(&git),
            namespaces: Arc::new(NoNamespaces),
            servers: Arc::new(OneHost(Arc::clone(&host))) as _,
            bus: Arc::clone(&bus) as _,
        });
        Self {
            plugins,
            logs,
            tokens,
            computer,
            host,
            manifests,
            bus,
            secrets,
            store,
            catalogs,
            git,
            connections,
            grants,
            workspace_id,
            agent_id: AgentId::generate(),
            home,
        }
    }

    /// The host of another tenant of the same Org: the same stores, the
    /// same Plugin Computer stand-in and the same Plugin logs, as the
    /// daemon's `PluginHosts` gives each tenant.
    pub fn host_for(&self, tenant_id: WorkspaceId) -> Arc<PluginHost> {
        Arc::new(PluginHost::new(PluginHostDeps {
            workspace_id: tenant_id,
            org_workspace_id: self.workspace_id.clone(),
            plugins: Arc::clone(&self.store) as _,
            catalogs: Arc::clone(&self.catalogs) as _,
            connections: Arc::clone(&self.connections) as _,
            grants: Arc::clone(&self.grants) as _,
            secrets: Arc::clone(&self.secrets) as _,
            tokens: Arc::clone(&self.tokens),
            git: Arc::clone(&self.git),
            manifests: Arc::clone(&self.manifests) as _,
            bus: Arc::clone(&self.bus) as _,
            processes: Arc::clone(&self.computer) as _,
            logs: Arc::clone(&self.logs),
        }))
    }

    /// Install one fixture plugin directory, with the Bindings the
    /// test supplies, and grant the Agent the Plugin.
    pub async fn install(&self, directory: &Path, bindings: &[BindingInput]) -> Plugin {
        let tar = tar_of(directory);
        let installed = self
            .plugins
            .install(&self.workspace_id, &SourceInput::Upload { tar }, bindings)
            .await
            .expect("the fixture plugin installs");
        self.plugins
            .grant(
                &self.workspace_id,
                &self.workspace_id,
                &installed.plugin.id,
                &self.agent_id,
            )
            .await
            .expect("the plugin grant");
        installed.plugin
    }

    /// One authorized call, as the broker hands it to the host.
    pub fn call(
        &self,
        plugin: &Plugin,
        server: &str,
        tool: &str,
        arguments: serde_json::Value,
        timeout: std::time::Duration,
    ) -> AuthorizedCall {
        AuthorizedCall {
            workspace_id: self.workspace_id.clone(),
            agent_id: self.agent_id.clone(),
            run_id: RunId::generate(),
            tool_name: format!("{}__{tool}", plugin.name),
            source_version: plugin.manifest_version.clone(),
            route: ToolRoute::Plugin {
                plugin: plugin.id.to_string(),
                server: server.to_string(),
                tool: tool.to_string(),
            },
            arguments,
            selected_connection: None,
            grant_id: None,
            grant_revision: None,
            call_timeout: Some(timeout),
            tool_call_id: None,
            host: None,
            approved_by_rule: false,
        }
    }

    /// Dispatch one call and answer with the result.
    pub async fn dispatch(
        &self,
        plugin: &Plugin,
        tool: &str,
        arguments: serde_json::Value,
    ) -> ToolResult {
        let call = self.call(
            plugin,
            "fixture",
            tool,
            arguments,
            std::time::Duration::from_secs(30),
        );
        self.host.dispatch(&call).await
    }

    pub async fn catalog(&self, plugin: &Plugin) -> PluginTools {
        self.host
            .catalog(plugin)
            .await
            .expect("the catalog reads")
            .expect("the plugin is frozen")
    }

    pub async fn reread(&self, plugin: &Plugin) -> Plugin {
        self.store
            .get(&self.workspace_id, &plugin.id)
            .await
            .expect("the plugin reads")
            .expect("the plugin is installed")
    }

    /// `PLUGIN_DATA` of one installed Plugin.
    pub fn data(&self, id: &PluginId) -> PathBuf {
        self.git.paths(&self.workspace_id, id.as_str()).data
    }

    /// `PLUGIN_ROOT` of one installed Plugin.
    pub fn root(&self, id: &PluginId) -> PathBuf {
        self.git.paths(&self.workspace_id, id.as_str()).root
    }

    pub fn secret(&self, name: &str) -> Option<String> {
        self.secrets.get(name).expect("the secret store reads")
    }

    pub fn events(&self) -> Vec<String> {
        self.bus.kinds()
    }
}

/// The stored fixture plugin directory, copied to a scratch directory
/// with a `server` command that runs the fixture binary. The command
/// of a stdio server is a bare name or a `./` path inside the plugin,
/// so the test writes the one file the package cannot carry.
pub fn stdio_plugin() -> tempfile::TempDir {
    let directory = copy_fixture("fixture");
    write_server_script(directory.path());
    directory
}

/// The same for the streamable HTTP fixture plugin, which needs no
/// command.
pub fn http_plugin() -> tempfile::TempDir {
    copy_fixture("fixture-http")
}

fn copy_fixture(name: &str) -> tempfile::TempDir {
    let source = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    )
    .join("../../fixtures/plugins")
    .join(name);
    let target = tempfile::tempdir().expect("a scratch directory");
    for entry in std::fs::read_dir(&source).expect("the fixture directory") {
        let entry = entry.expect("a fixture file");
        std::fs::copy(entry.path(), target.path().join(entry.file_name())).expect("a copy");
    }
    target
}

/// The `./server` command of the stdio fixture plugin: a script that
/// runs the built fixture binary.
fn write_server_script(root: &Path) {
    // The script leaves at once while the marker file is there, which
    // is how a test drives the start failures of ADR-0017.
    let script = format!(
        "#!/bin/sh\nif [ -f \"$PLUGIN_DATA/fail\" ]; then exit 1; fi\nexec {} \"$@\"\n",
        fixture_binary()
    );
    let path = root.join("server");
    std::fs::write(&path, script).expect("the server script");
    make_executable(&path);
}

/// The fixture binary Cargo built for this test.
pub fn fixture_binary() -> String {
    env!("CARGO_BIN_EXE_pagis-mcp-fixture").to_string()
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).expect("the script").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("the script is executable");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// One directory as the uncompressed tar an upload carries.
fn tar_of(directory: &Path) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    builder
        .append_dir_all(".", directory)
        .expect("the upload tar");
    builder.into_inner().expect("the upload tar")
}

/// The one Binding of the stdio fixture plugin.
pub fn api_key(secret: &str) -> Vec<BindingInput> {
    vec![BindingInput {
        field: "api_key".to_string(),
        value: BindingValueInput::Secret {
            secret: secret.to_string(),
        },
    }]
}
