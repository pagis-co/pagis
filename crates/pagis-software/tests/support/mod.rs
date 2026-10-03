//! One agent, one fake Computer, and an in-memory version store, so
//! the materialize and run tests see the same seam the daemon uses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use pagis_computer::fake::{FakeComputerRuntime, FakeWorkspaces};
use pagis_computer::{ComputerManager, ComputerState};
use pagis_core::{
    AgentId, Event, EventBus, EventId, EventScope, EventStream, NewEvent, StoreError, WorkspaceId,
    now_ms,
};
use pagis_software::manifest::PackageVersion;
use pagis_software::{Manifest, VersionSource};

/// A bus that keeps nothing: the audit trail has its own tests.
pub struct SilentBus;

#[async_trait]
impl EventBus for SilentBus {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        Ok(Event {
            id: EventId::generate(),
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

/// One stored version: what it declares, and its tar.
type Stored = (PackageVersion, Vec<u8>);

/// An in-memory version source, so the materialize and run tests
/// need no store.
#[allow(dead_code)]
#[derive(Default)]
pub struct MemorySource {
    versions: Mutex<HashMap<(String, String), Stored>>,
}

impl MemorySource {
    #[allow(dead_code)]
    pub fn with_version(package: &str, version: &str, manifest: &str, tar: Vec<u8>) -> Arc<Self> {
        let source = Self::default();
        source.versions.lock().expect("versions").insert(
            (package.to_string(), version.to_string()),
            (
                PackageVersion {
                    manifest: Manifest::parse(manifest).expect("the manifest parses"),
                    schemas: Default::default(),
                    widget_schemas: Default::default(),
                },
                tar,
            ),
        );
        Arc::new(source)
    }
}

#[async_trait]
impl VersionSource for MemorySource {
    async fn version(
        &self,
        _workspace_id: &WorkspaceId,
        package: &str,
        version: &str,
    ) -> Result<PackageVersion, String> {
        self.versions
            .lock()
            .expect("versions")
            .get(&(package.to_string(), version.to_string()))
            .map(|(found, _)| found.clone())
            .ok_or_else(|| format!("no version {package}@{version}"))
    }

    async fn tar(
        &self,
        _workspace_id: &WorkspaceId,
        package: &str,
        version: &str,
    ) -> Result<Vec<u8>, String> {
        self.versions
            .lock()
            .expect("versions")
            .get(&(package.to_string(), version.to_string()))
            .map(|(_, tar)| tar.clone())
            .ok_or_else(|| format!("no version {package}@{version}"))
    }

    async fn sandbox_csp(
        &self,
        _workspace_id: &WorkspaceId,
        _package: &str,
        _version: &str,
        _widget: &str,
    ) -> Option<pagis_software::WidgetCsp> {
        None
    }

    async fn file(
        &self,
        workspace_id: &WorkspaceId,
        package: &str,
        version: &str,
        path: &str,
    ) -> Result<Vec<u8>, String> {
        let tar = self.tar(workspace_id, package, version).await?;
        read_from_tar(&tar, path)
    }
}

/// One entry of a tar, by its path relative to the package root.
#[allow(dead_code)]
pub fn read_from_tar(tar: &[u8], path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;

    let mut archive = tar::Archive::new(tar);
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let held = entry
            .path()
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .trim_start_matches("./")
            .to_string();
        if held == path {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            return Ok(bytes);
        }
    }
    Err(format!("no such file {path}"))
}

pub struct Harness {
    /// Every tenant's manager. The Software half resolves the
    /// manager from the Workspace of the caller, so the harness holds the
    /// registry and not one manager.
    pub managers: Arc<pagis_computer::ComputerManagers>,
    pub manager: Arc<ComputerManager>,
    pub runtime: Arc<FakeComputerRuntime>,
    pub agent_id: AgentId,
    pub workspace_id: WorkspaceId,
    _screens: tempfile::TempDir,
}

pub async fn harness() -> Harness {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let screens = tempfile::tempdir().expect("screens dir");
    let workspace_id = WorkspaceId::generate();
    let managers = pagis_computer::ComputerManagers::new(pagis_computer::ComputerManagersDeps {
        runtime: Arc::clone(&runtime) as _,
        skills: Arc::new(pagis_core::NoSkills) as _,
        workspaces: Arc::new(FakeWorkspaces::with_timezone(&workspace_id, "UTC")) as _,
        agents: Arc::new(pagis_computer::fake::FakeAgents::open()) as _,
        bus: Arc::new(SilentBus) as _,
        screens_dir: screens.path().to_path_buf(),
        idle_stop: std::time::Duration::from_secs(600),
        relay: pagis_computer::fake::loopback_relay(),
        caps: pagis_computer::AwakeCaps::default(),
        cancel: tokio_util::sync::CancellationToken::new(),
        exit: None,
    });
    let manager = managers.get(&workspace_id);
    let agent_id = AgentId::generate();
    manager.wake(&agent_id).await.expect("wake");
    for _ in 0..200 {
        if manager.state(&agent_id).await == ComputerState::Awake {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    Harness {
        managers,
        manager,
        runtime,
        agent_id,
        workspace_id,
        _screens: screens,
    }
}

/// Wake one more agent's Computer on the same manager, so a test can
/// drive two agents.
#[allow(dead_code)]
pub async fn wake(manager: &Arc<ComputerManager>, agent_id: &AgentId) {
    manager.wake(agent_id).await.expect("wake");
    for _ in 0..200 {
        if manager.state(agent_id).await == ComputerState::Awake {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("the computer never woke");
}

/// Every message the Contribution tools posted, as (from, to, text).
#[derive(Default)]
pub struct RecordingMessenger {
    posted: Mutex<Vec<(AgentId, AgentId, String)>>,
}

impl RecordingMessenger {
    #[allow(dead_code)]
    pub fn posted(&self) -> Vec<(AgentId, AgentId, String)> {
        self.posted.lock().expect("messages").clone()
    }
}

#[async_trait]
impl pagis_software::AgentMessenger for RecordingMessenger {
    async fn post(
        &self,
        _workspace_id: &WorkspaceId,
        from: &AgentId,
        to: &AgentId,
        _run_id: &pagis_core::RunId,
        text: &str,
    ) -> Result<(), String> {
        self.posted
            .lock()
            .expect("messages")
            .push((from.clone(), to.clone(), text.to_string()));
        Ok(())
    }
}

/// The commands the fake runtime saw, in order, without the `timeout`
/// wrapper the manager adds. Only the materialize tests read them.
#[allow(dead_code)]
pub fn commands(runtime: &FakeComputerRuntime) -> Vec<String> {
    runtime
        .execs()
        .iter()
        .filter_map(|request| request.argv.last().cloned())
        .collect()
}

/// A bus that keeps every event, so a test reads the audit trail.
#[derive(Default)]
pub struct RecordingBus {
    events: Mutex<Vec<NewEvent>>,
}

impl RecordingBus {
    #[allow(dead_code)]
    pub fn events(&self) -> Vec<NewEvent> {
        self.events.lock().expect("events").clone()
    }
}

#[async_trait]
impl EventBus for RecordingBus {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        self.events.lock().expect("events").push(event.clone());
        Ok(Event {
            id: EventId::generate(),
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

/// The software notes each action writes.
#[derive(Default)]
pub struct RecordingNotes {
    lines: Mutex<Vec<String>>,
}

impl RecordingNotes {
    #[allow(dead_code)]
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().expect("notes").clone()
    }
}

#[async_trait]
impl pagis_software::SoftwareNotes for RecordingNotes {
    async fn record(
        &self,
        _workspace_id: &WorkspaceId,
        agent_id: &AgentId,
        _run_id: &pagis_core::RunId,
        line: &str,
    ) -> Result<(), String> {
        self.lines
            .lock()
            .expect("notes")
            .push(format!("{agent_id}: {line}"));
        Ok(())
    }
}

/// The Agents a publish names as authors.
#[derive(Default)]
pub struct MemoryAgents {
    agents: Mutex<Vec<pagis_core::Agent>>,
}

impl MemoryAgents {
    #[allow(dead_code)]
    pub fn holding(agents: Vec<pagis_core::Agent>) -> Arc<Self> {
        Arc::new(Self {
            agents: Mutex::new(agents),
        })
    }
}

#[async_trait]
impl pagis_core::AgentStore for MemoryAgents {
    async fn create(&self, agent: &pagis_core::Agent) -> Result<(), StoreError> {
        self.agents.lock().expect("agents").push(agent.clone());
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
    ) -> Result<Option<pagis_core::Agent>, StoreError> {
        Ok(self
            .agents
            .lock()
            .expect("agents")
            .iter()
            .find(|agent| &agent.id == id && &agent.workspace_id == workspace_id)
            .cloned())
    }

    async fn list_by_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<pagis_core::Agent>, StoreError> {
        Ok(self
            .agents
            .lock()
            .expect("agents")
            .iter()
            .filter(|agent| &agent.workspace_id == workspace_id)
            .cloned()
            .collect())
    }

    async fn update_avatar(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
        avatar: &pagis_core::AvatarAppearance,
        updated_at: i64,
    ) -> Result<bool, StoreError> {
        let mut agents = self.agents.lock().expect("agents");
        let Some(agent) = agents
            .iter_mut()
            .find(|agent| &agent.id == id && &agent.workspace_id == workspace_id)
        else {
            return Ok(false);
        };
        agent.avatar = avatar.clone();
        agent.updated_at = updated_at;
        Ok(true)
    }

    async fn update(&self, _agent: &pagis_core::Agent) -> Result<(), StoreError> {
        Ok(())
    }
}

/// Every Capability Manifest the Software List installed.
#[derive(Default)]
pub struct RecordingManifests {
    installed: Mutex<Vec<pagis_broker::CapabilityManifest>>,
}

impl RecordingManifests {
    #[allow(dead_code)]
    pub fn installed(&self) -> Vec<pagis_broker::CapabilityManifest> {
        self.installed.lock().expect("manifests").clone()
    }
}

impl pagis_software::ManifestSink for RecordingManifests {
    fn install(
        &self,
        _workspace_id: &WorkspaceId,
        manifest: pagis_broker::CapabilityManifest,
    ) -> Result<(), String> {
        self.installed.lock().expect("manifests").push(manifest);
        Ok(())
    }
}
