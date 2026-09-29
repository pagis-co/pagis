//! One Computer manager per tenant.
//!
//! The daemon serves many people. Each one has their own Workspace,
//! their own Tenant Network, their own containers and their own
//! volumes, so each one needs its own [`ComputerManager`]: the manager
//! is what names and labels every Docker object it asks for. This
//! module holds the managers, makes one on first use, and holds the one
//! thing they share — the ceiling on how many Computers are awake at
//! once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pagis_core::{EventBus, Skills, WorkspaceId, WorkspaceStore};
use tokio_util::sync::CancellationToken;

use crate::{AwakeCaps, ComputerError, ComputerManager, ComputerManagerDeps, ComputerRuntime};

/// What kind of Computer takes a place under the caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputerKind {
    /// A sprite's desk. It takes a place under the tenant's cap and
    /// under the server's.
    Sprite,
    /// The tenant's Plugin Computer (ADR-0017). It is where the tenant's
    /// installed plugins run, not a sprite's desk, so it takes no place
    /// away from a sprite and the per-tenant cap does not count it. It
    /// is a container on this machine like any other, so the per-server
    /// cap does count it.
    PluginHost,
}

/// How many Computers are awake, and how many may be. Idle
/// eviction says how long one stays awake; this says how many there
/// are. Every manager of the daemon shares one ceiling, so the server
/// count is the server's and not one tenant's.
pub struct AwakeCeiling {
    caps: AwakeCaps,
    /// The sprite desks awake, per tenant.
    sprites: Mutex<HashMap<WorkspaceId, u32>>,
    /// The Plugin Computers awake, per tenant. They are held apart
    /// because they count under one cap and not the other.
    plugin_hosts: Mutex<HashMap<WorkspaceId, u32>>,
}

impl AwakeCeiling {
    pub fn new(caps: AwakeCaps) -> Self {
        Self {
            caps,
            sprites: Mutex::new(HashMap::new()),
            plugin_hosts: Mutex::new(HashMap::new()),
        }
    }

    /// Count one more Computer of this tenant, or refuse. The refusal
    /// names which cap it met, because the two answers ask the person
    /// for different things: their own computer has to sleep, or the
    /// server is full now.
    ///
    /// A Plugin Computer meets the server's cap alone: a person who runs
    /// a plugin tool loses no desk over it.
    pub fn claim(
        &self,
        kind: ComputerKind,
        workspace_id: &WorkspaceId,
    ) -> Result<(), ComputerError> {
        let mut sprites = self.sprites.lock().expect("the awake count lock");
        let mut plugin_hosts = self.plugin_hosts.lock().expect("the awake count lock");
        if kind == ComputerKind::Sprite {
            let tenant = sprites.get(workspace_id).copied().unwrap_or(0);
            if tenant >= self.caps.per_tenant {
                return Err(ComputerError::AwakeCapReached {
                    scope: "your office",
                    cap: self.caps.per_tenant,
                });
            }
        }
        let server: u32 = sprites.values().sum::<u32>() + plugin_hosts.values().sum::<u32>();
        if server >= self.caps.per_server {
            return Err(ComputerError::AwakeCapReached {
                scope: "this server",
                cap: self.caps.per_server,
            });
        }
        let counts = match kind {
            ComputerKind::Sprite => &mut *sprites,
            ComputerKind::PluginHost => &mut *plugin_hosts,
        };
        *counts.entry(workspace_id.clone()).or_insert(0) += 1;
        Ok(())
    }

    /// Count a container that already runs. A daemon that restarts
    /// adopts what it finds: the count follows the machine, and a cap
    /// the adoption passes bounds the next wake and not this one.
    pub fn adopt(&self, kind: ComputerKind, workspace_id: &WorkspaceId) {
        let mut counts = self.counts(kind);
        *counts.entry(workspace_id.clone()).or_insert(0) += 1;
    }

    /// One Computer of this tenant stops being awake.
    pub fn release(&self, kind: ComputerKind, workspace_id: &WorkspaceId) {
        let mut counts = self.counts(kind);
        if let Some(count) = counts.get_mut(workspace_id) {
            *count = count.saturating_sub(1);
        }
    }

    fn counts(&self, kind: ComputerKind) -> std::sync::MutexGuard<'_, HashMap<WorkspaceId, u32>> {
        match kind {
            ComputerKind::Sprite => self.sprites.lock().expect("the awake count lock"),
            ComputerKind::PluginHost => self.plugin_hosts.lock().expect("the awake count lock"),
        }
    }

    /// The caps themselves, for the read that says how near the
    /// installation is to them.
    pub fn caps(&self) -> AwakeCaps {
        self.caps
    }

    /// How many sprite desks of one tenant are awake now. This is the
    /// figure the per-tenant cap bounds, so the Plugin Computer is not
    /// in it.
    pub fn awake(&self, workspace_id: &WorkspaceId) -> u32 {
        self.sprites
            .lock()
            .expect("the awake count lock")
            .get(workspace_id)
            .copied()
            .unwrap_or(0)
    }

    /// How many Computers the whole server holds awake now, of both
    /// kinds: this is the figure the per-server cap bounds.
    pub fn awake_on_server(&self) -> u32 {
        let sprites: u32 = self
            .sprites
            .lock()
            .expect("the awake count lock")
            .values()
            .sum();
        let plugin_hosts: u32 = self
            .plugin_hosts
            .lock()
            .expect("the awake count lock")
            .values()
            .sum();
        sprites + plugin_hosts
    }
}

/// What every tenant's manager is built from. The runtime, the Skills
/// catalogue and the Workspace store all take the tenant as an
/// argument, so one of each serves the whole daemon.
pub struct ComputerManagersDeps {
    pub runtime: Arc<dyn ComputerRuntime>,
    pub skills: Arc<dyn Skills>,
    pub workspaces: Arc<dyn WorkspaceStore>,
    /// The Agents of every Workspace. Each manager reads it to
    /// refuse an Agent that is not its tenant's.
    pub agents: Arc<dyn pagis_core::AgentStore>,
    pub bus: Arc<dyn EventBus>,
    pub screens_dir: PathBuf,
    pub idle_stop: Duration,
    /// How media reaches a browser. One relay serves the whole
    /// installation: the advertised address and the port range are a
    /// deployment fact, not a tenant's.
    pub relay: Arc<dyn crate::MediaRelay>,
    pub caps: AwakeCaps,
    /// The token that stops each manager's idle sweeper at shutdown.
    pub cancel: CancellationToken,
}

/// The Computer managers of the daemon, one per tenant.
pub struct ComputerManagers {
    deps: ComputerManagersDeps,
    ceiling: Arc<AwakeCeiling>,
    managers: Mutex<HashMap<WorkspaceId, Arc<ComputerManager>>>,
}

impl ComputerManagers {
    pub fn new(deps: ComputerManagersDeps) -> Arc<Self> {
        Arc::new(Self {
            ceiling: Arc::new(AwakeCeiling::new(deps.caps)),
            deps,
            managers: Mutex::new(HashMap::new()),
        })
    }

    /// The manager of one tenant, made on first use. Its idle sweeper
    /// starts with it and stops when the daemon stops.
    pub fn get(&self, workspace_id: &WorkspaceId) -> Arc<ComputerManager> {
        let mut managers = self.managers.lock().expect("the computer manager lock");
        if let Some(manager) = managers.get(workspace_id) {
            return Arc::clone(manager);
        }
        let manager = ComputerManager::new(ComputerManagerDeps {
            runtime: Arc::clone(&self.deps.runtime),
            skills: Arc::clone(&self.deps.skills),
            workspaces: Arc::clone(&self.deps.workspaces),
            agents: Arc::clone(&self.deps.agents),
            bus: Arc::clone(&self.deps.bus),
            workspace_id: workspace_id.clone(),
            screens_dir: self.deps.screens_dir.join(workspace_id.to_string()),
            idle_stop: self.deps.idle_stop,
            relay: Arc::clone(&self.deps.relay),
            ceiling: Arc::clone(&self.ceiling),
        });
        manager.spawn_sweeper(self.deps.cancel.clone());
        managers.insert(workspace_id.clone(), Arc::clone(&manager));
        manager
    }

    /// Adopt the running Computers of each tenant in `tenants`, the
    /// Workspaces of this installation. The daemon calls this when it
    /// starts, so a Computer that a restart left running is stopped by
    /// the idle sweep and by the stop for good, and counts under the
    /// awake cap, also when its Person does not come back. A Computer of
    /// a Workspace that is not in the list is another installation's on
    /// the same Docker host, and it is not touched.
    pub async fn adopt_all(&self, tenants: &[WorkspaceId]) {
        let managers: Vec<Arc<ComputerManager>> =
            tenants.iter().map(|tenant| self.get(tenant)).collect();
        futures::future::join_all(managers.iter().map(|manager| manager.adopt_all())).await;
    }

    /// Stop the Computers of every tenant: the daemon stops for good.
    /// A restart for a settings change does not call it, and the next
    /// daemon adopts the Computers that still run.
    pub async fn stop_all(&self) {
        let managers: Vec<Arc<ComputerManager>> = self
            .managers
            .lock()
            .expect("the computer manager lock")
            .values()
            .cloned()
            .collect();
        futures::future::join_all(managers.iter().map(|manager| manager.stop_all())).await;
    }

    /// The shared awake ceiling, for the tests and the System read.
    pub fn ceiling(&self) -> &Arc<AwakeCeiling> {
        &self.ceiling
    }

    /// Whether this machine holds a Computer volume to its size.
    /// The runtime is the installation's, so the answer is not a
    /// tenant's; the Administration Interface reports it.
    pub async fn volume_quota(&self) -> crate::Quota {
        self.deps.runtime.volume_quota().await
    }

    /// Whether this machine holds the writable container layer of a
    /// Computer to its size. Like the volume quota, the answer is the
    /// installation's; the Administration Interface reports it.
    pub async fn container_quota(&self) -> crate::Quota {
        self.deps.runtime.container_quota().await
    }

    /// The ICE servers a browser configures before it offers.
    /// The relay is the installation's, so the answer does not depend on
    /// the tenant.
    pub fn ice_servers(&self) -> Vec<crate::IceServer> {
        self.deps.relay.ice_servers()
    }
}
