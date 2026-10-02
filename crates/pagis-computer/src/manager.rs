//! The per-agent computer state machine: wake (join the pull of the
//! Computer Image with progress events, boot), adoption of
//! already-running containers, screen previews (live frame awake,
//! stored screenshot asleep), and the idle-stop sweep.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pagis_core::{AgentId, EventBus, NewEvent, Skills, WorkspaceId, WorkspaceStore};
use tokio_util::sync::CancellationToken;

use crate::image::Preparation;
use crate::{
    BindMount, CONTAINER_LANG, ComputerError, ComputerImage, ComputerImageState, ComputerRuntime,
    ComputerState, DEFAULT_TIMEZONE, ExecOutcome, ExecRequest, IMAGE_VERSION, InputHolder,
    OutputCap, StartedComputer, container_env, mounts_fingerprint,
};

/// Publish pull progress at most every this many percent.
const PROGRESS_STEP: u8 = 5;

/// The shortest wait before a screenshot after an input.
const SETTLE_MIN: Duration = Duration::from_millis(300);
/// The time between two frames that must be equal for a settled screen.
const SETTLE_POLL: Duration = Duration::from_millis(200);
/// The longest wait for a screen that keeps changing.
const SETTLE_MAX: Duration = Duration::from_secs(3);
/// The uid a shell command runs as. It owns `/data/agent`, and
/// it is not root: `pagis-apt` is the one way to root.
const SHELL_USER: &str = "agent";
/// The default working directory of a shell command.
pub const SHELL_HOME: &str = "/data/agent";
/// The grace between the `TERM` and the `KILL` of the in-container
/// timeout, in seconds.
const SHELL_KILL_AFTER: u64 = 5;
/// The output kept per stream: a 12 KiB head, a 4 KiB tail,
/// and a marker for the bytes between. The result goes into the model
/// request, where the context budget counts one token for each byte, so
/// a command that prints more writes to a file and reads the part it
/// needs.
const SHELL_OUTPUT_CAP: OutputCap = OutputCap {
    head: 12 * 1024,
    tail: 4 * 1024,
};

/// One shell command for an agent's own computer. The manager
/// owns the wrapping, the environment and the caps; the caller brings
/// the command, the deadline and the directory.
#[derive(Debug, Clone)]
pub struct ShellCommand {
    pub command: String,
    pub timeout: Duration,
    /// The working directory. Absent takes `/data/agent`.
    pub cwd: Option<String>,
    /// Bytes for the command's stdin. The daemon shuts the write half
    /// down after them, so the command sees the end of its input. A
    /// software tool call takes its arguments this way.
    pub stdin: Option<Vec<u8>>,
    /// How much of each output stream to keep. Absent takes the
    /// shell's own cap; a software tool call asks for more, because
    /// its stdout is the result.
    pub output_cap: Option<OutputCap>,
}

/// The takeover clock: after `idle` of user-input inactivity a
/// `screen.handback_countdown` event runs for `countdown`, then the
/// switch flips back. `poll` is how often the watchdog asks the
/// pipeline for the idle time. Tests shrink all three.
#[derive(Debug, Clone)]
pub struct TakeoverTiming {
    pub idle: Duration,
    pub countdown: Duration,
    pub poll: Duration,
}

impl Default for TakeoverTiming {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(180),
            countdown: Duration::from_secs(15),
            poll: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone)]
enum Phase {
    Off,
    Pulling(u8),
    Starting,
    Awake(StartedComputer),
    Failed(String),
}

struct Entry {
    phase: Phase,
    last_activity: Instant,
    /// The fingerprint of the mount set the running container booted
    /// with; `None` while it is not awake.
    mounts: Option<String>,
    /// The timezone the running container booted with; `None`
    /// while it is not awake, and for an adopted container, which this
    /// process did not boot.
    timezone: Option<String>,
    /// Whether this computer holds one place under the awake cap.
    /// It takes the place from the wake that starts it and
    /// gives it back when the container goes.
    occupied: bool,
    /// The Media Relay path of the live screen. An awake Computer has
    /// at most one: the next offer replaces it, and it closes when the
    /// Computer stops being awake (ADR-0014).
    path: Option<crate::OpenPath>,
}

impl Entry {
    fn off() -> Self {
        Self {
            phase: Phase::Off,
            last_activity: Instant::now(),
            mounts: None,
            timezone: None,
            occupied: false,
            path: None,
        }
    }
}

/// One live hold of the input switch.
#[derive(Debug, Clone, Copy)]
struct Hold {
    holder: InputHolder,
    since: Instant,
}

/// One screen preview: PNG bytes, live or from the stored screenshot.
pub struct Preview {
    pub png: Vec<u8>,
    pub live: bool,
}

pub struct ComputerManager {
    runtime: Arc<dyn ComputerRuntime>,
    /// The one preparation of the Computer Image for the installation.
    /// A wake that finds the image absent joins it.
    image: Arc<ComputerImage>,
    /// The Skills the agent may reach (ADR-0017). Each granted
    /// Plugin's `skills/` directory mounts read-only at boot.
    skills: Arc<dyn Skills>,
    /// The Workspace the timezone of every container comes from.
    workspaces: Arc<dyn WorkspaceStore>,
    /// The Agents of this Workspace. The manager reads this to
    /// refuse an Agent of another tenant: a container is a boundary
    /// between tenants, so a caller that brings the wrong manager must
    /// not boot a container on this tenant's network under this tenant's
    /// labels and cap.
    agents: Arc<dyn pagis_core::AgentStore>,
    bus: Arc<dyn EventBus>,
    workspace_id: WorkspaceId,
    /// Last screenshots, one PNG per agent, served while asleep.
    screens_dir: PathBuf,
    idle_stop: Duration,
    /// How media reaches a browser (ADR-0014). The screen path
    /// calls this seam and nothing else: the relay owns the advertised
    /// address, the ports and the forwarding.
    relay: Arc<dyn crate::MediaRelay>,
    entries: Mutex<HashMap<AgentId, Entry>>,
    /// The screen lease: one action batch at a time per agent.
    leases: Mutex<HashMap<AgentId, Arc<tokio::sync::Mutex<()>>>>,
    /// Active holds of the input switch: who holds each
    /// computer, and since when. Absent means the agent holds.
    holds: Mutex<HashMap<AgentId, Hold>>,
    timing: Mutex<TakeoverTiming>,
    /// How many commands run on each computer now. A computer
    /// with a command in flight never idle-stops, because a stop kills
    /// every exec on it with code 137.
    running_execs: Mutex<HashMap<AgentId, u32>>,
    /// Serializes the inspect-to-phase transition, so concurrent wake
    /// requests cannot both decide to start the same computer.
    wake_preflight: tokio::sync::Mutex<()>,
    /// How many Computers may be awake at once, per tenant and
    /// for the whole server. Every manager of the daemon shares one
    /// ceiling, so the server count is the server's and not one
    /// tenant's.
    ceiling: Arc<crate::AwakeCeiling>,
}

/// What the manager needs to run the computers of one Workspace.
pub struct ComputerManagerDeps {
    pub runtime: Arc<dyn ComputerRuntime>,
    /// The Computer Image of the installation, which every manager
    /// shares.
    pub image: Arc<ComputerImage>,
    pub skills: Arc<dyn Skills>,
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub agents: Arc<dyn pagis_core::AgentStore>,
    pub bus: Arc<dyn EventBus>,
    pub workspace_id: WorkspaceId,
    pub screens_dir: PathBuf,
    pub idle_stop: Duration,
    pub relay: Arc<dyn crate::MediaRelay>,
    pub ceiling: Arc<crate::AwakeCeiling>,
}

impl ComputerManager {
    pub fn new(deps: ComputerManagerDeps) -> Arc<Self> {
        Arc::new(Self {
            runtime: deps.runtime,
            image: deps.image,
            skills: deps.skills,
            workspaces: deps.workspaces,
            agents: deps.agents,
            bus: deps.bus,
            workspace_id: deps.workspace_id,
            screens_dir: deps.screens_dir,
            idle_stop: deps.idle_stop,
            relay: deps.relay,
            entries: Mutex::new(HashMap::new()),
            leases: Mutex::new(HashMap::new()),
            holds: Mutex::new(HashMap::new()),
            timing: Mutex::new(TakeoverTiming::default()),
            running_execs: Mutex::new(HashMap::new()),
            wake_preflight: tokio::sync::Mutex::new(()),
            ceiling: deps.ceiling,
        })
    }

    /// The tenant this manager serves.
    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    /// Who owns one Agent's container and volume: this tenant,
    /// and the Agent. Every Docker object the manager asks for is named
    /// and labelled from it.
    fn owner(&self, agent_id: &AgentId) -> crate::ComputerOwner {
        crate::ComputerOwner::new(self.workspace_id.clone(), agent_id.clone())
    }

    /// Shrink the takeover clock; tests only.
    pub fn set_takeover_timing(&self, timing: TakeoverTiming) {
        *self.timing.lock().expect("takeover timing lock") = timing;
    }

    /// Refuse an Agent that is not of this manager's Workspace.
    ///
    /// The manager names and labels every Docker object from its own
    /// Workspace, so a caller that resolved the wrong manager would put
    /// one tenant's sprite on another tenant's network, under that
    /// tenant's labels and against that tenant's disk figure and cap.
    /// The Plugin Computer is exempt: it is the tenant's own plugin host
    /// and no Agent row stands behind it.
    async fn require_own_agent(&self, agent_id: &AgentId) -> Result<(), ComputerError> {
        if agent_id.as_str() == crate::PLUGIN_AGENT {
            return Ok(());
        }
        let found = self
            .agents
            .get(&self.workspace_id, agent_id)
            .await
            .map_err(|error| ComputerError::Runtime(error.to_string()))?;
        match found.is_some() {
            true => Ok(()),
            false => Err(ComputerError::ForeignAgent),
        }
    }

    /// What kind of Computer one Agent id names. The Plugin
    /// Computer is the tenant's plugin host and every other id is a
    /// sprite's desk.
    fn kind(agent_id: &AgentId) -> crate::ComputerKind {
        match agent_id.as_str() == crate::PLUGIN_AGENT {
            true => crate::ComputerKind::PluginHost,
            false => crate::ComputerKind::Sprite,
        }
    }

    /// Take one place under the awake caps for this computer, or
    /// refuse. A computer that already holds a place keeps it, so a
    /// second wake of an awake computer is still free.
    ///
    /// A Plugin Computer meets the per-server cap and not the per-tenant
    /// one: it is not a sprite's desk, so it takes no desk away, and it
    /// is still a container this machine has to hold.
    fn occupy(&self, agent_id: &AgentId) -> Result<(), ComputerError> {
        let mut entries = self.entries.lock().expect("computer entries lock");
        let entry = entries.entry(agent_id.clone()).or_insert_with(Entry::off);
        if entry.occupied {
            return Ok(());
        }
        self.ceiling
            .claim(Self::kind(agent_id), &self.workspace_id)?;
        entry.occupied = true;
        Ok(())
    }

    /// Count a container this process found already running. Adoption
    /// never refuses: the container runs whatever the count says.
    fn occupy_adopted(&self, agent_id: &AgentId) {
        let mut entries = self.entries.lock().expect("computer entries lock");
        let entry = entries.entry(agent_id.clone()).or_insert_with(Entry::off);
        if entry.occupied {
            return;
        }
        self.ceiling.adopt(Self::kind(agent_id), &self.workspace_id);
        entry.occupied = true;
    }

    /// Give the place back: this computer is not awake any more.
    fn vacate(&self, agent_id: &AgentId) {
        let mut entries = self.entries.lock().expect("computer entries lock");
        if let Some(entry) = entries.get_mut(agent_id)
            && entry.occupied
        {
            entry.occupied = false;
            self.ceiling
                .release(Self::kind(agent_id), &self.workspace_id);
        }
    }

    /// The idle-stop loop; checks once a minute.
    pub fn spawn_sweeper(self: &Arc<Self>, cancel: CancellationToken) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            while cancel.run_until_cancelled(tick.tick()).await.is_some() {
                manager.sweep().await;
            }
        });
    }

    /// The agent's current state, adopting an already-running
    /// container first (daemon restart recovery).
    pub async fn state(&self, agent_id: &AgentId) -> ComputerState {
        self.adopt_running(agent_id).await;
        match self.phase(agent_id) {
            Phase::Off => ComputerState::Off,
            Phase::Pulling(percent) => ComputerState::Pulling { percent },
            Phase::Starting => ComputerState::Starting,
            Phase::Awake(_) => ComputerState::Awake,
            Phase::Failed(message) => ComputerState::Failed { message },
        }
    }

    pub async fn image_state(&self) -> ComputerImageState {
        match self.runtime.image_version().await {
            Ok(None) => ComputerImageState::Absent,
            Ok(Some(version)) if version == IMAGE_VERSION => ComputerImageState::Present,
            Ok(found) => ComputerImageState::Mismatched { found },
            Err(message) => ComputerImageState::Unavailable { message },
        }
    }

    /// Wake the agent's computer. Refuses a version-mismatched local
    /// image; otherwise kicks off the (pull +) boot in the background
    /// and returns the state the wake put it in. A wake of an awake or
    /// already-waking computer is a no-op that refreshes activity.
    pub async fn wake(
        self: &Arc<Self>,
        agent_id: &AgentId,
    ) -> Result<ComputerState, ComputerError> {
        // The Skills mount set of this wake (ADR-0017). A
        // computer that runs another set is replaced here, so a Grant,
        // an update or an uninstall reaches the agent's next Run.
        let mounts = self.skills.mounts(&self.workspace_id, agent_id).await;
        let mounts: Vec<BindMount> = mounts.iter().map(BindMount::skills).collect();
        self.wake_with(agent_id, mounts).await
    }

    /// The same wake over a mount set the caller brings. The tenant's
    /// Plugin Computer takes this path: its files are the
    /// tenant's Plugins and not one Agent's Skills.
    pub async fn wake_with(
        self: &Arc<Self>,
        agent_id: &AgentId,
        mounts: Vec<BindMount>,
    ) -> Result<ComputerState, ComputerError> {
        self.require_own_agent(agent_id).await?;
        let _preflight = self.wake_preflight.lock().await;
        self.adopt_running(agent_id).await;
        self.replace_stale_mounts(agent_id, &mounts_fingerprint(&mounts))
            .await;
        {
            let mut entries = self.entries.lock().expect("computer entries lock");
            let entry = entries.entry(agent_id.clone()).or_insert_with(Entry::off);
            entry.last_activity = Instant::now();
            match entry.phase {
                Phase::Awake(_) => return Ok(ComputerState::Awake),
                Phase::Pulling(percent) => return Ok(ComputerState::Pulling { percent }),
                Phase::Starting => return Ok(ComputerState::Starting),
                Phase::Failed(_) => {}
                Phase::Off => {}
            }
        }

        // The cap on simultaneously awake Computers. It is read
        // before the image and before the boot, so a refusal costs
        // nothing and says at once what the person has to do.
        self.occupy(agent_id)?;
        let local = match self.runtime.image_version().await {
            Ok(local) => local,
            Err(error) => {
                self.vacate(agent_id);
                return Err(ComputerError::Runtime(error));
            }
        };
        let needs_pull = match local.as_deref() {
            Some(IMAGE_VERSION) => false,
            None => true,
            Some(_) => {
                self.vacate(agent_id);
                return Err(ComputerError::VersionMismatch { found: local });
            }
        };

        let first_state = if needs_pull {
            Phase::Pulling(0)
        } else {
            Phase::Starting
        };
        self.set_phase(agent_id, first_state.clone()).await;

        // The clock and the locale of this boot. A running
        // computer keeps the timezone it booted with; a changed
        // Workspace timezone reaches the agent at its next wake.
        let timezone = self.boot_timezone().await;
        let manager = Arc::clone(self);
        let agent_id = agent_id.clone();
        tokio::spawn(async move {
            if let Err(error) = manager.boot(&agent_id, needs_pull, mounts, timezone).await {
                tracing::error!(%error, %agent_id, "computer wake failed");
                manager.vacate(&agent_id);
                manager.set_phase(&agent_id, Phase::Failed(error)).await;
            }
        });
        Ok(match first_state {
            Phase::Pulling(percent) => ComputerState::Pulling { percent },
            _ => ComputerState::Starting,
        })
    }

    /// Stop an awake computer whose mount set is not the one the
    /// agent must get. A computer with a command in flight is
    /// left alone: a running Run keeps the Skills it started with
    /// (ADR-0017), and a stop would kill its exec.
    async fn replace_stale_mounts(&self, agent_id: &AgentId, wanted: &str) {
        {
            let entries = self.entries.lock().expect("computer entries lock");
            let stale = entries.get(agent_id).is_some_and(|entry| {
                matches!(entry.phase, Phase::Awake(_)) && entry.mounts.as_deref() != Some(wanted)
            });
            if !stale {
                return;
            }
        }
        if self.is_busy(agent_id) {
            tracing::info!(
                %agent_id,
                "a command is running; the new skill mounts wait for the next wake"
            );
            return;
        }
        tracing::info!(%agent_id, "replacing a computer that mounts another set of skills");
        if let Err(error) = self.runtime.stop(&self.owner(agent_id)).await {
            tracing::error!(%error, %agent_id, "stale computer stop failed");
            return;
        }
        self.vacate(agent_id);
        self.set_phase(agent_id, Phase::Off).await;
    }

    /// The Workspace timezone the next boot carries. A store
    /// that cannot answer gives UTC, because a computer that boots on
    /// the wrong clock is better than one that does not boot.
    async fn boot_timezone(&self) -> String {
        match self.workspaces.get(&self.workspace_id).await {
            Ok(Some(workspace)) => workspace.timezone,
            Ok(None) => DEFAULT_TIMEZONE.to_string(),
            Err(error) => {
                tracing::error!(%error, "the workspace timezone read failed; the computer takes UTC");
                DEFAULT_TIMEZONE.to_string()
            }
        }
    }

    /// The background (pull +) boot. The pull is the one of the
    /// installation: this wake joins it and publishes its progress.
    async fn boot(
        self: &Arc<Self>,
        agent_id: &AgentId,
        needs_pull: bool,
        mounts: Vec<BindMount>,
        timezone: String,
    ) -> Result<(), String> {
        if needs_pull {
            let mut preparation = self.image.join();
            let mut published: u8 = 0;
            loop {
                let now = preparation.borrow_and_update().clone();
                match now {
                    Preparation::Pulling(percent) => {
                        if percent >= published.saturating_add(PROGRESS_STEP) || percent == 100 {
                            published = percent;
                            self.set_phase(agent_id, Phase::Pulling(percent)).await;
                        }
                    }
                    Preparation::Ready => {
                        // A wake sees the newest step of the pull only,
                        // so it can miss the last ones. The pull is whole.
                        if published < 100 {
                            self.set_phase(agent_id, Phase::Pulling(100)).await;
                        }
                        break;
                    }
                    Preparation::Failed(error) => return Err(error),
                }
                if preparation.changed().await.is_err() {
                    return Err("the Computer Image preparation stopped".to_string());
                }
            }
            self.set_phase(agent_id, Phase::Starting).await;
        }
        let started = self
            .runtime
            .start(&self.owner(agent_id), &mounts, &container_env(&timezone))
            .await?;
        self.touch(agent_id);
        self.set_phase(agent_id, Phase::Awake(started)).await;
        self.record_mounts(agent_id, Some(mounts_fingerprint(&mounts)));
        self.record_timezone(agent_id, Some(timezone));
        Ok(())
    }

    /// Remember which mount set the running container booted with.
    fn record_mounts(&self, agent_id: &AgentId, fingerprint: Option<String>) {
        let mut entries = self.entries.lock().expect("computer entries lock");
        if let Some(entry) = entries.get_mut(agent_id) {
            entry.mounts = fingerprint;
        }
    }

    /// Remember the timezone the running container booted with,
    /// so a shell command reads the clock the browser reads.
    fn record_timezone(&self, agent_id: &AgentId, timezone: Option<String>) {
        let mut entries = self.entries.lock().expect("computer entries lock");
        if let Some(entry) = entries.get_mut(agent_id) {
            entry.timezone = timezone;
        }
    }

    /// The timezone of the running container, when this process booted
    /// it.
    fn booted_timezone(&self, agent_id: &AgentId) -> Option<String> {
        self.entries
            .lock()
            .expect("computer entries lock")
            .get(agent_id)
            .and_then(|entry| entry.timezone.clone())
    }

    /// The screen lease: held for one action batch, released
    /// between turns. Batches for one agent serialize here. Denied
    /// while the user holds the input switch.
    pub async fn lease(
        &self,
        agent_id: &AgentId,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, ComputerError> {
        let lease = {
            let mut leases = self.leases.lock().expect("computer leases lock");
            Arc::clone(
                leases
                    .entry(agent_id.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let guard = lease.lock_owned().await;
        let holder = self.holder(agent_id);
        if holder != InputHolder::Agent {
            return Err(ComputerError::SwitchHeld { holder });
        }
        Ok(guard)
    }

    /// Who drives the computer's input.
    pub fn holder(&self, agent_id: &AgentId) -> InputHolder {
        self.holds
            .lock()
            .expect("computer holds lock")
            .get(agent_id)
            .map(|hold| hold.holder)
            .unwrap_or(InputHolder::Agent)
    }

    /// Take over: flip the input switch to the user, tell the
    /// pipeline, publish `screen.takeover_started`, and start the
    /// inactivity watchdog. A takeover of a held computer is a no-op.
    pub async fn takeover(self: &Arc<Self>, agent_id: &AgentId) -> Result<(), ComputerError> {
        let Phase::Awake(computer) = self.phase(agent_id) else {
            return Err(ComputerError::Asleep);
        };
        {
            let mut holds = self.holds.lock().expect("computer holds lock");
            if holds.contains_key(agent_id) {
                return Ok(());
            }
            holds.insert(
                agent_id.clone(),
                Hold {
                    holder: InputHolder::User,
                    since: Instant::now(),
                },
            );
        }
        if let Err(error) = self.runtime.set_holder(&computer, InputHolder::User).await {
            self.holds
                .lock()
                .expect("computer holds lock")
                .remove(agent_id);
            return Err(ComputerError::Runtime(error));
        }
        self.touch(agent_id);
        self.publish_screen_event(agent_id, "screen.takeover_started", serde_json::json!({}))
            .await;
        self.spawn_watchdog(agent_id);
        Ok(())
    }

    /// Hand back: flip the switch to the agent, publish
    /// `screen.takeover_ended`. Both paths (explicit and inactivity)
    /// land here. Returns false when the user held nothing.
    pub async fn handback(&self, agent_id: &AgentId, reason: &str) -> Result<bool, ComputerError> {
        let hold = {
            let mut holds = self.holds.lock().expect("computer holds lock");
            match holds.get(agent_id) {
                Some(hold) if hold.holder == InputHolder::User => holds.remove(agent_id),
                _ => None,
            }
        };
        let Some(hold) = hold else {
            return Ok(false);
        };
        if let Phase::Awake(computer) = self.phase(agent_id)
            && let Err(error) = self.runtime.set_holder(&computer, InputHolder::Agent).await
        {
            tracing::error!(%error, %agent_id, "pipeline handback failed");
        }
        self.touch(agent_id);
        self.publish_screen_event(
            agent_id,
            "screen.takeover_ended",
            serde_json::json!({
                "reason": reason,
                "duration_ms": hold.since.elapsed().as_millis() as u64,
            }),
        )
        .await;
        Ok(true)
    }

    /// Hand back every Computer of this tenant that the user holds. When
    /// every Session of the Person ends, nobody of the Workspace can hand
    /// one back, so the daemon does. A Computer that the daemon holds for
    /// a fill is not the user's, and it stays held.
    pub async fn handback_all(&self, reason: &str) {
        let taken: Vec<AgentId> = self
            .holds
            .lock()
            .expect("computer holds lock")
            .iter()
            .filter(|(_, hold)| hold.holder == InputHolder::User)
            .map(|(agent_id, _)| agent_id.clone())
            .collect();
        for agent_id in taken {
            if let Err(error) = self.handback(&agent_id, reason).await {
                tracing::error!(%error, %agent_id, "handback failed");
            }
        }
    }

    /// The inactivity watchdog: poll the pipeline's user-input
    /// idle time; after `timing.idle` publish the countdown event, and
    /// `timing.countdown` later flip the switch back. Any input during
    /// the countdown cancels it.
    fn spawn_watchdog(self: &Arc<Self>, agent_id: &AgentId) {
        let manager = Arc::clone(self);
        let agent_id = agent_id.clone();
        tokio::spawn(async move {
            let timing = manager.timing.lock().expect("takeover timing lock").clone();
            let mut counting_down = false;
            loop {
                tokio::time::sleep(timing.poll).await;
                if manager.holder(&agent_id) != InputHolder::User {
                    return;
                }
                let Phase::Awake(computer) = manager.phase(&agent_id) else {
                    let _ = manager.handback(&agent_id, "computer stopped").await;
                    return;
                };
                let idle_ms = match manager.runtime.user_input_idle_ms(&computer).await {
                    Ok(idle_ms) => idle_ms,
                    Err(error) => {
                        tracing::warn!(%error, %agent_id, "takeover idle poll failed");
                        continue;
                    }
                };
                // The user is driving: the computer is active.
                manager.touch(&agent_id);
                let idle = Duration::from_millis(idle_ms);
                if idle >= timing.idle + timing.countdown {
                    let _ = manager.handback(&agent_id, "inactivity").await;
                    return;
                }
                if idle >= timing.idle && !counting_down {
                    counting_down = true;
                    manager
                        .publish_screen_event(
                            &agent_id,
                            "screen.handback_countdown",
                            serde_json::json!({
                                "seconds": timing.countdown.as_secs(),
                            }),
                        )
                        .await;
                } else if idle < timing.idle && counting_down {
                    counting_down = false;
                    manager
                        .publish_screen_event(
                            &agent_id,
                            "screen.handback_countdown_canceled",
                            serde_json::json!({}),
                        )
                        .await;
                }
            }
        });
    }

    async fn publish_screen_event(
        &self,
        agent_id: &AgentId,
        event_type: &str,
        payload: serde_json::Value,
    ) {
        let event = NewEvent {
            workspace_id: self.workspace_id.clone(),
            event_type: event_type.to_string(),
            agent_id: Some(agent_id.clone()),
            run_id: None,
            channel_id: None,
            payload,
        };
        if let Err(error) = self.bus.publish(event).await {
            tracing::error!(%error, %agent_id, %event_type, "screen event publish failed");
        }
    }

    /// Wake and wait until the computer answers: the executor
    /// needs an awake computer before its first action.
    pub async fn ensure_awake(self: &Arc<Self>, agent_id: &AgentId) -> Result<(), ComputerError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
        loop {
            match self.wake(agent_id).await? {
                ComputerState::Awake => return Ok(()),
                _ => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(ComputerError::Runtime(
                            "the computer did not wake in time".to_string(),
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// The tenant's Plugin Computer, awake, with this mount set
    /// (ADR-0017). The Plugin host calls it before it starts a server:
    /// the server runs inside this container, so no MCP server process
    /// runs on the daemon host. A changed mount set replaces the
    /// container at the next call, as a changed Skills set replaces a
    /// sprite's desk.
    pub async fn ensure_plugin_computer(
        self: &Arc<Self>,
        mounts: Vec<BindMount>,
    ) -> Result<StartedComputer, ComputerError> {
        let agent_id = crate::plugin_agent();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
        loop {
            self.wake_with(&agent_id, mounts.clone()).await?;
            if let Phase::Awake(computer) = self.phase(&agent_id) {
                return Ok(computer);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ComputerError::Runtime(
                    "the plugin computer did not wake in time".to_string(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// One long-lived process in the tenant's Plugin Computer,
    /// with its streams attached. The caller speaks MCP over them.
    pub async fn plugin_server(
        self: &Arc<Self>,
        computer: &StartedComputer,
        request: ExecRequest,
    ) -> Result<crate::ExecStream, ComputerError> {
        self.runtime
            .exec_stream(computer, request)
            .await
            .map_err(ComputerError::Runtime)
    }

    /// Run one command in the agent's own computer. The
    /// computer wakes first, and the deadline runs inside the
    /// container, because the Engine API cannot stop a running exec.
    /// The command runs as uid `agent` in a fixed environment,
    /// and it is not a login shell.
    pub async fn shell(
        self: &Arc<Self>,
        agent_id: &AgentId,
        command: ShellCommand,
    ) -> Result<ExecOutcome, ComputerError> {
        self.require_own_agent(agent_id).await?;
        self.ensure_awake(agent_id).await?;
        let Phase::Awake(computer) = self.phase(agent_id) else {
            return Err(ComputerError::Asleep);
        };
        // The clock and the locale of the container, so a
        // script reads the times the browser shows. An adopted
        // container, which this process did not boot, gives its own
        // `TZ`: the entries here are added to the container's.
        let mut env = vec![
            format!("HOME={SHELL_HOME}"),
            format!("USER={SHELL_USER}"),
            format!("LOGNAME={SHELL_USER}"),
            "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
            format!("LANG={CONTAINER_LANG}"),
        ];
        if let Some(timezone) = self.booted_timezone(agent_id) {
            env.push(format!("TZ={timezone}"));
        }
        let request = ExecRequest {
            argv: vec![
                "timeout".to_string(),
                format!("--kill-after={SHELL_KILL_AFTER}"),
                command.timeout.as_secs().to_string(),
                "bash".to_string(),
                "-c".to_string(),
                command.command,
            ],
            user: SHELL_USER.to_string(),
            cwd: command.cwd.unwrap_or_else(|| SHELL_HOME.to_string()),
            env,
            stdin: command.stdin,
            output_cap: command.output_cap.unwrap_or(SHELL_OUTPUT_CAP),
        };
        // The pin holds the computer awake for the whole command; the
        // idle-stop would otherwise kill it with code 137.
        let pin = self.pin(agent_id);
        self.touch(agent_id);
        let outcome = self.runtime.exec(&computer, request).await;
        self.touch(agent_id);
        drop(pin);
        outcome.map_err(ComputerError::Runtime)
    }

    /// Extract one tar archive into a directory of the agent's own
    /// computer. The directory must exist; a shell command
    /// makes it. The computer wakes first, and the pin holds it awake
    /// for the upload.
    pub async fn upload_archive(
        self: &Arc<Self>,
        agent_id: &AgentId,
        path: &str,
        tar: Vec<u8>,
    ) -> Result<(), ComputerError> {
        self.require_own_agent(agent_id).await?;
        self.ensure_awake(agent_id).await?;
        let Phase::Awake(computer) = self.phase(agent_id) else {
            return Err(ComputerError::Asleep);
        };
        let pin = self.pin(agent_id);
        self.touch(agent_id);
        let result = self.runtime.upload_archive(&computer, path, tar).await;
        self.touch(agent_id);
        drop(pin);
        result.map_err(ComputerError::Runtime)
    }

    /// The uncompressed tar of one path of the agent's own computer,
    /// in a spool file that is open at its first byte. The download
    /// stops with an error when the raw tar passes `MAX_ARCHIVE_BYTES`
    /// (1 GiB). The memory of a download does not grow with the tar,
    /// and the spool file stops one byte past the limit. The computer
    /// wakes first, and the pin holds it awake for the read. The
    /// entries are relative to the parent of `path`.
    pub async fn download_archive(
        self: &Arc<Self>,
        agent_id: &AgentId,
        path: &str,
    ) -> Result<std::fs::File, ComputerError> {
        self.ensure_awake(agent_id).await?;
        let Phase::Awake(computer) = self.phase(agent_id) else {
            return Err(ComputerError::Asleep);
        };
        let pin = self.pin(agent_id);
        self.touch(agent_id);
        let result = match self.runtime.download_archive(&computer, path).await {
            Ok(stream) => spool(stream, path).await,
            Err(error) => Err(error),
        };
        self.touch(agent_id);
        drop(pin);
        result.map_err(ComputerError::Runtime)
    }

    /// Count one command against the agent's computer until the guard
    /// drops.
    fn pin(self: &Arc<Self>, agent_id: &AgentId) -> ExecPin {
        *self
            .running_execs
            .lock()
            .expect("running execs lock")
            .entry(agent_id.clone())
            .or_insert(0) += 1;
        ExecPin {
            manager: Arc::clone(self),
            agent_id: agent_id.clone(),
        }
    }

    /// Whether a command runs on this computer now.
    fn is_busy(&self, agent_id: &AgentId) -> bool {
        self.running_execs
            .lock()
            .expect("running execs lock")
            .get(agent_id)
            .is_some_and(|count| *count > 0)
    }

    /// One live frame for the executor; fails when the computer is not
    /// awake. Also refreshes the stored screenshot and activity.
    /// Refused while the daemon holds the switch: a frame that
    /// can reach the model must never carry a secret being filled.
    pub async fn live_frame(&self, agent_id: &AgentId) -> Result<Vec<u8>, ComputerError> {
        let holder = self.holder(agent_id);
        if holder == InputHolder::Daemon {
            return Err(ComputerError::SwitchHeld { holder });
        }
        let Phase::Awake(computer) = self.phase(agent_id) else {
            return Err(ComputerError::Runtime(
                "the computer is not awake".to_string(),
            ));
        };
        let png = self
            .runtime
            .fetch_frame(&computer)
            .await
            .map_err(ComputerError::Runtime)?;
        self.touch(agent_id);
        self.store_screenshot(agent_id, &png);
        Ok(png)
    }

    /// One frame after the screen stops changing. A page reacts to an
    /// input over several frames, and a screenshot in the middle of
    /// that shows a state that is gone. Anthropic's reference loop waits
    /// a fixed two seconds after each action; this waits at least
    /// [`SETTLE_MIN`], then until two frames [`SETTLE_POLL`] apart match
    /// ([`crate::exec::frames_match`]), and at most [`SETTLE_MAX`] on a
    /// screen that keeps moving. A loading page keeps moving: the tab
    /// spinner turns until the load ends.
    pub async fn settled_frame(&self, agent_id: &AgentId) -> Result<Vec<u8>, ComputerError> {
        let started = tokio::time::Instant::now();
        let deadline = started + SETTLE_MAX;
        tokio::time::sleep(SETTLE_MIN).await;
        let mut frame = self.live_frame(agent_id).await?;
        let mut settled = false;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(SETTLE_POLL).await;
            let next = self.live_frame(agent_id).await?;
            let matched = crate::exec::frames_match(&frame, &next);
            frame = next;
            if matched {
                settled = true;
                break;
            }
        }
        tracing::info!(
            %agent_id,
            settle_ms = started.elapsed().as_millis() as u64,
            settled,
            "screenshot after input"
        );
        Ok(frame)
    }

    /// Inject one input batch on the agent's behalf; the
    /// computer must be awake.
    pub async fn input(
        &self,
        agent_id: &AgentId,
        ops: &[crate::exec::InputOp],
    ) -> Result<(), ComputerError> {
        let Phase::Awake(computer) = self.phase(agent_id) else {
            return Err(ComputerError::Runtime(
                "the computer is not awake".to_string(),
            ));
        };
        self.runtime
            .send_input(&computer, InputHolder::Agent, ops)
            .await
            .map_err(ComputerError::Runtime)?;
        self.touch(agent_id);
        Ok(())
    }

    /// Take the input switch for the daemon (ADR-0013): the vault
    /// fills a secret the model never sees, through the browser channel
    /// of the hold ([`DaemonHold::open`], [`DaemonHold::fill`]).
    /// Model-visible capture stops until the release, and the agent's
    /// screen lease is denied for as long as the hold lasts. Refused
    /// while the user holds.
    ///
    /// The hold waits for the screen lease first, so it never starts in
    /// the middle of an agent batch, and then lets it go: from there
    /// the switch itself, not the lease mutex, is what denies the
    /// agent.
    pub async fn daemon_hold(
        self: &Arc<Self>,
        agent_id: &AgentId,
    ) -> Result<DaemonHold, ComputerError> {
        let Phase::Awake(computer) = self.phase(agent_id) else {
            return Err(ComputerError::Asleep);
        };
        let lease = {
            let mut leases = self.leases.lock().expect("computer leases lock");
            Arc::clone(
                leases
                    .entry(agent_id.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let quiesce = lease.lock_owned().await;
        {
            let mut holds = self.holds.lock().expect("computer holds lock");
            if let Some(hold) = holds.get(agent_id) {
                return Err(ComputerError::SwitchHeld {
                    holder: hold.holder,
                });
            }
            holds.insert(
                agent_id.clone(),
                Hold {
                    holder: InputHolder::Daemon,
                    since: Instant::now(),
                },
            );
        }
        drop(quiesce);
        if let Err(error) = self
            .runtime
            .set_holder(&computer, InputHolder::Daemon)
            .await
        {
            self.holds
                .lock()
                .expect("computer holds lock")
                .remove(agent_id);
            return Err(ComputerError::Runtime(error));
        }
        self.touch(agent_id);
        self.publish_screen_event(
            agent_id,
            "screen.daemon_hold_started",
            serde_json::json!({}),
        )
        .await;
        Ok(DaemonHold {
            manager: Arc::clone(self),
            agent_id: agent_id.clone(),
            live: true,
        })
    }

    /// Release the daemon's switch and re-screenshot, the way a
    /// handback does: the stored frame is stale after a fill,
    /// and the next one the agent sees must be current.
    async fn daemon_release(&self, agent_id: &AgentId) {
        let held = {
            let mut holds = self.holds.lock().expect("computer holds lock");
            match holds.get(agent_id) {
                Some(hold) if hold.holder == InputHolder::Daemon => {
                    holds.remove(agent_id);
                    true
                }
                _ => false,
            }
        };
        if !held {
            return;
        }
        let Phase::Awake(computer) = self.phase(agent_id) else {
            return;
        };
        if let Err(error) = self.runtime.set_holder(&computer, InputHolder::Agent).await {
            tracing::error!(%error, %agent_id, "daemon switch release failed");
        }
        match self.runtime.fetch_frame(&computer).await {
            Ok(png) => self.store_screenshot(agent_id, &png),
            Err(error) => tracing::warn!(%error, %agent_id, "post-fill screenshot failed"),
        }
        self.touch(agent_id);
        self.publish_screen_event(agent_id, "screen.daemon_hold_ended", serde_json::json!({}))
            .await;
    }

    /// Relay one live-view SDP offer to the awake computer and
    /// return the answer. The Media Relay opens the path first and says
    /// which address the browser sends media to; the container
    /// publishes no media port, so that address is the relay's and never
    /// the container's, and the pipeline registers with the path from
    /// inside its Tenant Network. The path then takes the ICE
    /// credentials of the answer, so it accepts the checks of the
    /// browser that holds the answer and of nobody else.
    ///
    /// An awake Computer has one path. The offer closes the path of the
    /// last offer before it opens its own, so a tab that reloads frees
    /// its port, and a second tab takes the live screen from the first.
    /// A refused offer closes its path at once. The path also closes
    /// when `closed` is cancelled: the daemon cancels it when the
    /// Session of the viewer ends, so video and input stop for that
    /// viewer.
    pub async fn offer(
        &self,
        agent_id: &AgentId,
        sdp: &str,
        closed: CancellationToken,
    ) -> Result<String, ComputerError> {
        let Phase::Awake(computer) = self.phase(agent_id) else {
            return Err(ComputerError::Asleep);
        };
        if let Some(replaced) = self.take_path(agent_id) {
            replaced.close().await;
        }
        let path = self
            .relay
            .open(closed)
            .await
            .map_err(|reason| ComputerError::NoMediaPath { reason })?;
        let answer = match self.runtime.relay_offer(&computer, sdp, &path).await {
            Ok(answer) => answer,
            Err(error) => {
                path.close().await;
                return Err(ComputerError::Runtime(error));
            }
        };
        let Some(credentials) = crate::IceCredentials::of_answer(&answer) else {
            path.close().await;
            return Err(ComputerError::Runtime(
                "the answer of the screen pipeline carries no ICE credentials".to_string(),
            ));
        };
        path.authenticate(credentials);
        self.keep_path(agent_id, path).await?;
        self.touch(agent_id);
        Ok(answer)
    }

    /// Take the media path of one Computer out of its entry.
    fn take_path(&self, agent_id: &AgentId) -> Option<crate::OpenPath> {
        self.entries
            .lock()
            .expect("computer entries lock")
            .get_mut(agent_id)
            .and_then(|entry| entry.path.take())
    }

    /// Keep `path` as the media path of an awake Computer. A path that
    /// an offer at the same time kept first closes, and so does `path`
    /// when the Computer stopped while the pipeline answered.
    async fn keep_path(
        &self,
        agent_id: &AgentId,
        path: crate::OpenPath,
    ) -> Result<(), ComputerError> {
        let (closing, kept) = {
            let mut entries = self.entries.lock().expect("computer entries lock");
            match entries.get_mut(agent_id) {
                Some(entry) if matches!(entry.phase, Phase::Awake(_)) => {
                    (entry.path.replace(path), true)
                }
                _ => (Some(path), false),
            }
        };
        if let Some(closing) = closing {
            closing.close().await;
        }
        match kept {
            true => Ok(()),
            false => Err(ComputerError::Asleep),
        }
    }

    /// The ICE servers a browser configures before it offers.
    /// The `daemon` relay needs none; the `turn` relay mints one
    /// credential for each viewer session.
    pub fn ice_servers(&self) -> Vec<crate::IceServer> {
        self.relay.ice_servers()
    }

    /// The screen preview: a current frame when awake, the last
    /// stored screenshot when asleep.
    pub async fn preview(&self, agent_id: &AgentId) -> Result<Preview, ComputerError> {
        self.adopt_running(agent_id).await;
        if let Phase::Awake(computer) = self.phase(agent_id) {
            match self.runtime.fetch_frame(&computer).await {
                Ok(png) => {
                    self.touch(agent_id);
                    // The user watches the fill live, but nothing that
                    // shows a secret is retained.
                    if self.holder(agent_id) != InputHolder::Daemon {
                        self.store_screenshot(agent_id, &png);
                    }
                    return Ok(Preview { png, live: true });
                }
                Err(error) => {
                    tracing::warn!(%error, %agent_id, "live frame fetch failed");
                }
            }
        }
        match std::fs::read(self.screenshot_path(agent_id)) {
            Ok(png) => Ok(Preview { png, live: false }),
            Err(_) => Err(ComputerError::NoPreview),
        }
    }

    /// One idle-stop pass: stop awake computers idle past the limit,
    /// keeping a final screenshot for the sleeping tile.
    pub async fn sweep(&self) {
        let idle: Vec<(AgentId, StartedComputer)> = {
            let entries = self.entries.lock().expect("computer entries lock");
            entries
                .iter()
                .filter_map(|(agent_id, entry)| match &entry.phase {
                    Phase::Awake(computer)
                        if entry.last_activity.elapsed() >= self.idle_stop
                            && !self.is_busy(agent_id) =>
                    {
                        Some((agent_id.clone(), computer.clone()))
                    }
                    _ => None,
                })
                .collect()
        };
        for (agent_id, computer) in idle {
            if let Err(error) = self.stop_computer(&agent_id, Some(&computer)).await {
                tracing::error!(%error, %agent_id, "computer idle-stop failed");
            }
        }
    }

    /// Stop every Computer of this tenant that is awake or starts now,
    /// busy or not: the daemon stops for good, and no idle sweep runs
    /// after it to stop them. A Computer then holds no memory and no CPU
    /// while nothing can reach it. The Computers stop at the same time,
    /// so the daemon exits before its supervisor's grace period ends.
    pub async fn stop_all(&self) {
        let running: Vec<(AgentId, Option<StartedComputer>)> = {
            let entries = self.entries.lock().expect("computer entries lock");
            entries
                .iter()
                .filter_map(|(agent_id, entry)| match &entry.phase {
                    Phase::Awake(computer) => Some((agent_id.clone(), Some(computer.clone()))),
                    Phase::Starting => Some((agent_id.clone(), None)),
                    Phase::Off | Phase::Pulling(_) | Phase::Failed(_) => None,
                })
                .collect()
        };
        let stops = running.iter().map(|(agent_id, computer)| async move {
            if let Err(error) = self.stop_computer(agent_id, computer.as_ref()).await {
                tracing::error!(%error, %agent_id, "computer stop at shutdown failed");
            }
        });
        futures::future::join_all(stops).await;
    }

    /// Stop one Computer and keep its last screen for the sleeping
    /// tile. A Computer that still starts has no screen to keep.
    async fn stop_computer(
        &self,
        agent_id: &AgentId,
        computer: Option<&StartedComputer>,
    ) -> Result<(), String> {
        if let Some(computer) = computer
            && let Ok(png) = self.runtime.fetch_frame(computer).await
        {
            self.store_screenshot(agent_id, &png);
        }
        self.runtime.stop(&self.owner(agent_id)).await?;
        self.vacate(agent_id);
        self.set_phase_with(agent_id, Phase::Off, None).await;
        Ok(())
    }

    /// Put the Agent's computer to sleep now, ahead of the idle sweep.
    /// The stored screenshot is refreshed first, so the Desk
    /// keeps a picture of the screen it stopped. A computer that runs
    /// a command is refused, because a stop would kill it; a
    /// computer that is already off is a no-op.
    pub async fn sleep(&self, agent_id: &AgentId) -> Result<ComputerState, ComputerError> {
        let Phase::Awake(computer) = self.phase(agent_id) else {
            return Ok(self.state(agent_id).await);
        };
        if self.is_busy(agent_id) {
            return Err(ComputerError::Busy);
        }
        self.stop_computer(agent_id, Some(&computer))
            .await
            .map_err(ComputerError::Runtime)?;
        Ok(ComputerState::Off)
    }

    /// What this tenant holds on the host: the containers,
    /// the volumes and the bytes they take. The Desk Panel reads the
    /// disk figure of it, and the Administration Interface reads the
    /// whole set.
    pub async fn resources(&self) -> Result<crate::TenantResources, ComputerError> {
        self.runtime
            .resources(&self.workspace_id)
            .await
            .map_err(ComputerError::Runtime)
    }

    /// Adopt every running Computer of this tenant. The daemon calls
    /// this when it starts: a Computer whose Agent nobody asks for is
    /// then known too, so the idle sweep, the stop for good and the
    /// awake cap include it. Each adoption holds the wake preflight, so
    /// a wake at the same time does not see a half-adopted Computer.
    pub async fn adopt_all(&self) {
        let agents = match self.runtime.running_agents(&self.workspace_id).await {
            Ok(agents) => agents,
            Err(error) => {
                tracing::warn!(%error, workspace_id = %self.workspace_id, "running-computer list failed");
                return;
            }
        };
        for agent_id in agents {
            let _preflight = self.wake_preflight.lock().await;
            self.adopt_running(&agent_id).await;
        }
    }

    /// Adopt a container that is already running but unknown to this
    /// process (a wake before this daemon started). A container from a
    /// different image version is stopped instead of adopted, so
    /// the next wake boots the pinned image; the volume survives, so
    /// the agent keeps its data.
    async fn adopt_running(&self, agent_id: &AgentId) {
        if !matches!(self.phase(agent_id), Phase::Off | Phase::Failed(_)) {
            return;
        }
        match self.runtime.running(&self.owner(agent_id)).await {
            Ok(Some(running)) => {
                if !running.image_matches || running.version.as_deref() != Some(IMAGE_VERSION) {
                    tracing::info!(
                        %agent_id,
                        found = ?running.version,
                        needs = IMAGE_VERSION,
                        image_matches = running.image_matches,
                        "replacing a computer outside the locked image"
                    );
                    if let Err(error) = self.runtime.stop(&self.owner(agent_id)).await {
                        tracing::error!(%error, %agent_id, "stale computer stop failed");
                    }
                    return;
                }
                self.touch(agent_id);
                self.occupy_adopted(agent_id);
                let mounts = running.mounts;
                self.set_phase(agent_id, Phase::Awake(running.computer))
                    .await;
                // The adopted container's own label, so the next wake
                // can tell whether its Skills are still the right ones.
                self.record_mounts(agent_id, mounts);
                // This process did not boot it, so its clock is its
                // own: a shell command inherits the container's `TZ`.
                self.record_timezone(agent_id, None);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%error, %agent_id, "running-container check failed");
            }
        }
    }

    fn phase(&self, agent_id: &AgentId) -> Phase {
        let entries = self.entries.lock().expect("computer entries lock");
        entries
            .get(agent_id)
            .map(|entry| entry.phase.clone())
            .unwrap_or(Phase::Off)
    }

    fn touch(&self, agent_id: &AgentId) {
        let mut entries = self.entries.lock().expect("computer entries lock");
        if let Some(entry) = entries.get_mut(agent_id) {
            entry.last_activity = Instant::now();
        }
    }

    async fn set_phase(&self, agent_id: &AgentId, phase: Phase) {
        self.set_phase_with(agent_id, phase, None).await
    }

    /// Record the phase and publish `computer.state_changed`. A
    /// Computer that is not awake has no screen to watch, so its media
    /// path closes.
    async fn set_phase_with(&self, agent_id: &AgentId, phase: Phase, error: Option<&str>) {
        let (state, closing) = {
            let mut entries = self.entries.lock().expect("computer entries lock");
            let entry = entries.entry(agent_id.clone()).or_insert_with(Entry::off);
            entry.phase = phase;
            let closing = match entry.phase {
                Phase::Awake(_) => None,
                _ => entry.path.take(),
            };
            let state = match &entry.phase {
                Phase::Off => ComputerState::Off,
                Phase::Pulling(percent) => ComputerState::Pulling { percent: *percent },
                Phase::Starting => ComputerState::Starting,
                Phase::Awake(_) => ComputerState::Awake,
                Phase::Failed(message) => ComputerState::Failed {
                    message: message.clone(),
                },
            };
            (state, closing)
        };
        if let Some(closing) = closing {
            closing.close().await;
        }
        let mut payload = serde_json::to_value(&state).expect("state serializes");
        if let Some(error) = error {
            payload["error"] = serde_json::Value::String(error.to_string());
        }
        let event = NewEvent {
            workspace_id: self.workspace_id.clone(),
            event_type: "computer.state_changed".to_string(),
            agent_id: Some(agent_id.clone()),
            run_id: None,
            channel_id: None,
            payload,
        };
        if let Err(error) = self.bus.publish(event).await {
            tracing::error!(%error, %agent_id, "computer.state_changed publish failed");
        }
    }

    fn screenshot_path(&self, agent_id: &AgentId) -> PathBuf {
        self.screens_dir.join(format!("{agent_id}.png"))
    }

    fn store_screenshot(&self, agent_id: &AgentId, png: &[u8]) {
        if let Err(error) = std::fs::create_dir_all(&self.screens_dir)
            .and_then(|()| std::fs::write(self.screenshot_path(agent_id), png))
        {
            tracing::error!(%error, %agent_id, "screenshot store failed");
        }
    }
}

/// One command's claim on an agent's computer. While a claim
/// stands, the idle-stop sweep leaves that computer awake.
struct ExecPin {
    manager: Arc<ComputerManager>,
    agent_id: AgentId,
}

impl Drop for ExecPin {
    fn drop(&mut self) {
        let mut execs = self
            .manager
            .running_execs
            .lock()
            .expect("running execs lock");
        if let Some(count) = execs.get_mut(&self.agent_id) {
            *count = count.saturating_sub(1);
        }
    }
}

/// Write the tar stream of `path` into a spool file in the temporary
/// directory, and stop when the raw byte count passes
/// `MAX_ARCHIVE_BYTES`. The download then reads at most one chunk past
/// the limit, and the spool file holds at most one byte past it. The
/// file has no name, so the system deletes it when the last handle
/// closes, on an error too.
async fn spool(stream: crate::ArchiveStream, path: &str) -> Result<std::fs::File, String> {
    use std::io::Seek;
    use tokio::io::AsyncReadExt;

    let file =
        tempfile::tempfile().map_err(|error| format!("cannot make a spool file: {error}"))?;
    let mut writer = tokio::fs::File::from_std(file);
    let mut reader = tokio_util::io::StreamReader::new(stream).take(crate::MAX_ARCHIVE_BYTES + 1);
    let written = tokio::io::copy_buf(&mut reader, &mut writer)
        .await
        .map_err(|error| format!("cannot download {path}: {error}"))?;
    if written > crate::MAX_ARCHIVE_BYTES {
        return Err(format!(
            "the tar of {path} is larger than the limit of {} bytes",
            crate::MAX_ARCHIVE_BYTES
        ));
    }
    let mut file = writer.into_std().await;
    file.rewind()
        .map_err(|error| format!("cannot read the spool file: {error}"))?;
    Ok(file)
}

/// The daemon's hold on one computer's input switch. It releases
/// the switch when the fill ends — on the normal path through
/// `release`, and on an early return through `Drop`.
pub struct DaemonHold {
    manager: Arc<ComputerManager>,
    agent_id: AgentId,
    live: bool,
}

impl DaemonHold {
    /// Open `url` in the daemon's own tab of the browser and wait for
    /// the load event of the page. The answer is the top-level address
    /// the tab shows after every redirect (ADR-0013).
    pub async fn open(&self, url: &str) -> Result<String, ComputerError> {
        let computer = self.computer()?;
        let page = self
            .manager
            .runtime
            .browser_open(&computer, url)
            .await
            .map_err(ComputerError::Runtime)?;
        self.manager.touch(&self.agent_id);
        Ok(page)
    }

    /// The top-level address the daemon's own tab shows now. Nothing
    /// navigates.
    pub async fn page(&self) -> Result<String, ComputerError> {
        let computer = self.computer()?;
        self.manager
            .runtime
            .browser_page(&computer)
            .await
            .map_err(ComputerError::Runtime)
    }

    /// Write `fields` into verified fields of the daemon's own tab,
    /// while its top-level origin is `origin`. The text goes through the
    /// browser channel and never through the compositor.
    pub async fn fill(
        &self,
        origin: &str,
        fields: &[crate::FillField],
    ) -> Result<(), ComputerError> {
        let computer = self.computer()?;
        self.manager
            .runtime
            .browser_fill(&computer, origin, fields)
            .await
            .map_err(ComputerError::Runtime)?;
        self.manager.touch(&self.agent_id);
        Ok(())
    }

    fn computer(&self) -> Result<StartedComputer, ComputerError> {
        match self.manager.phase(&self.agent_id) {
            Phase::Awake(computer) => Ok(computer),
            _ => Err(ComputerError::Asleep),
        }
    }

    /// Hand the switch back to the agent and refresh the stored frame.
    pub async fn release(mut self) {
        self.live = false;
        self.manager.daemon_release(&self.agent_id).await;
    }
}

impl Drop for DaemonHold {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        let manager = Arc::clone(&self.manager);
        let agent_id = self.agent_id.clone();
        tokio::spawn(async move { manager.daemon_release(&agent_id).await });
    }
}
