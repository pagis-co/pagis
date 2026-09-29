//! One MCP host per tenant (ADR-0017).
//!
//! A Plugin is installed by the administrator for the Org: the Org owns
//! which plugins are available, and the rows, the Bindings and the
//! checkout live in the Org's Workspace, which no person owns. Every
//! Workspace then runs the Org's installed plugins in its own Plugin
//! Computer, with its own Grants and its own broker registry, so two
//! people who call one plugin tool reach two containers.
//!
//! The [`PluginHost`] is what holds the supervisors of one Plugin
//! Computer, so each tenant needs one. This module holds them, makes one
//! on first use, and is the seam the install path writes through.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use pagis_core::{
    ConnectionStore, EventBus, GrantStore, Plugin, PluginBinding, PluginId, PluginStore,
    PluginToolStore, SecretStore, WorkspaceId, WorkspaceStore,
};
use pagis_plugin::{PluginGitStore, PluginPackage, PluginServers};
use tokio_util::sync::CancellationToken;

use crate::host::{ConnectionTokens, Manifests, PluginHost, PluginHostDeps};
use crate::log::PluginLogs;
use crate::process::ServerProcesses;

/// What every tenant's host is built from. Everything here takes the
/// tenant as an argument or is the installation's, so one of each serves
/// the whole daemon.
pub struct PluginHostsDeps {
    /// The Org's Workspace, where the installed Plugins live.
    pub org_workspace_id: WorkspaceId,
    /// Every tenant of the installation, for the loads that must reach
    /// all of them.
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub plugins: Arc<dyn PluginStore>,
    pub catalogs: Arc<dyn PluginToolStore>,
    pub connections: Arc<dyn ConnectionStore>,
    pub grants: Arc<dyn GrantStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub tokens: Arc<dyn ConnectionTokens>,
    pub git: Arc<PluginGitStore>,
    pub manifests: Arc<dyn Manifests>,
    pub bus: Arc<dyn EventBus>,
    pub processes: Arc<dyn ServerProcesses>,
    /// The Plugin stderr logs, one file for each Workspace and Plugin.
    /// Each tenant's host writes and reads its own files.
    pub logs: Arc<PluginLogs>,
}

/// The MCP hosts of the daemon, one per tenant.
pub struct PluginHosts {
    deps: PluginHostsDeps,
    hosts: Mutex<HashMap<WorkspaceId, Arc<PluginHost>>>,
}

impl PluginHosts {
    pub fn new(deps: PluginHostsDeps) -> Arc<Self> {
        Arc::new(Self {
            deps,
            hosts: Mutex::new(HashMap::new()),
        })
    }

    /// The host of one tenant, made on first use.
    pub fn get(&self, workspace_id: &WorkspaceId) -> Arc<PluginHost> {
        let mut hosts = self.hosts.lock().expect("the plugin host lock");
        if let Some(host) = hosts.get(workspace_id) {
            return Arc::clone(host);
        }
        let host = Arc::new(PluginHost::new(PluginHostDeps {
            workspace_id: workspace_id.clone(),
            org_workspace_id: self.deps.org_workspace_id.clone(),
            plugins: Arc::clone(&self.deps.plugins),
            catalogs: Arc::clone(&self.deps.catalogs),
            connections: Arc::clone(&self.deps.connections),
            grants: Arc::clone(&self.deps.grants),
            secrets: Arc::clone(&self.deps.secrets),
            tokens: Arc::clone(&self.deps.tokens),
            git: Arc::clone(&self.deps.git),
            manifests: Arc::clone(&self.deps.manifests),
            bus: Arc::clone(&self.deps.bus),
            processes: Arc::clone(&self.deps.processes),
            logs: Arc::clone(&self.deps.logs),
        }));
        hosts.insert(workspace_id.clone(), Arc::clone(&host));
        host
    }

    /// The host of the Workspace the Org's Plugins live in. The install
    /// path freezes a tool list through it, because a freeze starts the
    /// servers once and the administrator's own Plugin Computer is where
    /// that happens.
    pub fn org(&self) -> Arc<PluginHost> {
        self.get(&self.deps.org_workspace_id.clone())
    }

    /// Give every tenant's broker registry the Capability Manifest of
    /// every Plugin the Org has installed.
    ///
    /// The boot calls it, and so does every install, update and
    /// uninstall: the Org's list changed, so every tenant's registry has
    /// to say so without a restart. One tenant that cannot load does not
    /// stop the others.
    pub async fn load_every_tenant(&self) {
        let tenants = match self.deps.workspaces.list().await {
            Ok(tenants) => tenants,
            Err(error) => {
                tracing::warn!(%error, "the plugin manifests reached no tenant");
                return;
            }
        };
        for tenant in tenants {
            if let Err(error) = self.get(&tenant.id).load().await {
                tracing::warn!(
                    workspace = %tenant.id,
                    %error,
                    "the plugin manifests did not all load for one tenant"
                );
            }
        }
    }

    /// Stop the idle servers of every tenant.
    async fn reap_idle(&self, now: Instant) {
        let hosts: Vec<Arc<PluginHost>> = self
            .hosts
            .lock()
            .expect("the plugin host lock")
            .values()
            .map(Arc::clone)
            .collect();
        for host in hosts {
            host.reap_idle(now).await;
        }
    }
}

/// The install path's seam (ADR-0017).
///
/// An install, an update and an uninstall are the Org's, so the freeze
/// runs on the Org's own host. Every tenant's registry then reads the
/// Org's list again, because the tools a sprite may call changed for
/// everybody.
#[async_trait]
impl PluginServers for PluginHosts {
    async fn freeze(
        &self,
        plugin: &Plugin,
        package: &PluginPackage,
        bindings: &[PluginBinding],
    ) -> Result<(), String> {
        self.org()
            .freeze_for_install(plugin, package, bindings)
            .await?;
        self.load_every_tenant().await;
        Ok(())
    }

    async fn stop(&self, plugin_id: &PluginId) {
        let hosts: Vec<Arc<PluginHost>> = self
            .hosts
            .lock()
            .expect("the plugin host lock")
            .values()
            .map(Arc::clone)
            .collect();
        for host in hosts {
            host.stop_plugin(plugin_id).await;
        }
    }

    async fn forget(&self, plugin_id: &PluginId) {
        self.stop(plugin_id).await;
        self.org().forget_plugin(plugin_id).await;
    }
}

/// Stop the idle plugin servers of every tenant on a timer (ADR-0017).
pub fn spawn_idle_reaper(hosts: Arc<PluginHosts>, every: Duration, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        while cancel.run_until_cancelled(ticker.tick()).await.is_some() {
            hosts.reap_idle(Instant::now()).await;
        }
    });
}
