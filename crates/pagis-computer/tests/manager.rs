//! Manager tests over the fake runtime: the one pull of the Computer
//! Image that every wake joins, with progress events, the removal of
//! the old images, version refusal, previews awake and asleep, the
//! idle-stop sweep, and volume survival.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_computer::fake::ice_check;
use pagis_computer::fake::{FakeComputerRuntime, FakeWorkspaces};
use pagis_computer::{
    AwakeCaps, AwakeCeiling, BindMount, ComputerError, ComputerImage, ComputerManager,
    ComputerManagerDeps, ComputerState, ExecOutcome, IMAGE, IMAGE_VERSION, IceCredentials,
    ImagePullError, InputHolder, OutputCap, PLUGIN_MOUNT_ROOT, SHELL_HOME, ShellCommand,
    TakeoverTiming, image_repository,
};
use tokio_util::sync::CancellationToken;

use pagis_core::{
    AgentId, Event, EventBus, EventId, EventScope, EventStream, NewEvent, Skill, SkillMount,
    Skills, StoreError, WorkspaceId, WorkspaceStore, now_ms,
};

/// A catalogue a test can change between wakes.
#[derive(Default)]
struct FakeSkills {
    mounts: Mutex<Vec<SkillMount>>,
}

impl FakeSkills {
    fn set(&self, mounts: Vec<SkillMount>) {
        *self.mounts.lock().unwrap() = mounts;
    }
}

#[async_trait]
impl Skills for FakeSkills {
    async fn list(&self, _workspace_id: &WorkspaceId, _agent_id: &AgentId) -> Vec<Skill> {
        Vec::new()
    }

    async fn body(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
        _plugin: &str,
        _skill: &str,
    ) -> Option<String> {
        None
    }

    async fn mounts(&self, _workspace_id: &WorkspaceId, _agent_id: &AgentId) -> Vec<SkillMount> {
        self.mounts.lock().unwrap().clone()
    }
}

/// A bus that records published events; subscribe is unused here.
#[derive(Default)]
struct RecordingBus {
    events: Mutex<Vec<NewEvent>>,
}

impl RecordingBus {
    fn events(&self) -> Vec<NewEvent> {
        self.events.lock().unwrap().clone()
    }

    fn states(&self) -> Vec<String> {
        self.events()
            .iter()
            .filter(|event| event.event_type == "computer.state_changed")
            .map(|event| event.payload["state"].as_str().unwrap_or("?").to_string())
            .collect()
    }
}

#[async_trait]
impl EventBus for RecordingBus {
    async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
        let mut events = self.events.lock().unwrap();
        events.push(event.clone());
        Ok(Event {
            id: EventId::generate(),
            seq: events.len() as i64,
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

/// The Workspace timezone every harness starts with.
const TEST_TIMEZONE: &str = "Australia/Sydney";

struct Harness {
    manager: Arc<ComputerManager>,
    ceiling: Arc<AwakeCeiling>,
    runtime: Arc<FakeComputerRuntime>,
    bus: Arc<RecordingBus>,
    skills: Arc<FakeSkills>,
    workspaces: Arc<FakeWorkspaces>,
    workspace_id: WorkspaceId,
    agent_id: AgentId,
    screens: tempfile::TempDir,
}

fn harness_with(runtime: FakeComputerRuntime, idle_stop: Duration) -> Harness {
    harness_with_caps(runtime, idle_stop, AwakeCaps::default())
}

/// The same harness under a scripted awake cap.
fn harness_with_caps(
    runtime: FakeComputerRuntime,
    idle_stop: Duration,
    caps: AwakeCaps,
) -> Harness {
    harness_with_relay(
        runtime,
        idle_stop,
        caps,
        pagis_computer::fake::loopback_relay(),
    )
}

/// The same harness over a scripted Media Relay.
fn harness_with_relay(
    runtime: FakeComputerRuntime,
    idle_stop: Duration,
    caps: AwakeCaps,
    relay: Arc<dyn pagis_computer::MediaRelay>,
) -> Harness {
    harness_of(runtime, idle_stop, caps, relay, None)
}

/// The exit listener of the Server harness, as a Computer reaches it.
const EXIT_DAEMON: &str = "host.docker.internal:4403";

/// The harness of a Server: its Computers reach the exit listener of
/// the daemon at [`EXIT_DAEMON`] (ADR-0029).
fn server_harness() -> Harness {
    harness_of(
        FakeComputerRuntime::with_image(),
        Duration::from_secs(600),
        AwakeCaps::default(),
        pagis_computer::fake::loopback_relay(),
        Some(EXIT_DAEMON.to_string()),
    )
}

fn harness_of(
    runtime: FakeComputerRuntime,
    idle_stop: Duration,
    caps: AwakeCaps,
    relay: Arc<dyn pagis_computer::MediaRelay>,
    exit_daemon: Option<String>,
) -> Harness {
    let runtime = Arc::new(runtime);
    let bus = Arc::new(RecordingBus::default());
    let skills = Arc::new(FakeSkills::default());
    let workspace_id = WorkspaceId::generate();
    let workspaces = Arc::new(FakeWorkspaces::with_timezone(&workspace_id, TEST_TIMEZONE));
    let screens = tempfile::tempdir().expect("screens dir");
    let ceiling = Arc::new(AwakeCeiling::new(caps));
    let manager = ComputerManager::new(ComputerManagerDeps {
        runtime: Arc::clone(&runtime) as _,
        image: ComputerImage::new(Arc::clone(&runtime) as _),
        skills: Arc::clone(&skills) as _,
        workspaces: Arc::clone(&workspaces) as _,
        agents: Arc::new(pagis_computer::fake::FakeAgents::open()),
        bus: Arc::clone(&bus) as _,
        workspace_id: workspace_id.clone(),
        screens_dir: screens.path().to_path_buf(),
        idle_stop,
        relay,
        ceiling: Arc::clone(&ceiling),
        exit_daemon,
    });
    Harness {
        manager,
        ceiling,
        runtime,
        bus,
        skills,
        workspaces,
        workspace_id,
        agent_id: AgentId::generate(),
        screens,
    }
}

async fn wait_awake(h: &Harness) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while h.manager.state(&h.agent_id).await != ComputerState::Awake {
        assert!(tokio::time::Instant::now() < deadline, "never woke");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn wake_pulls_the_missing_image_with_visible_progress_and_boots() {
    let h = harness_with(FakeComputerRuntime::default(), Duration::from_secs(600));

    let state = h.manager.wake(&h.agent_id).await.unwrap();

    assert_eq!(state, ComputerState::Pulling { percent: 0 });
    wait_awake(&h).await;
    assert_eq!(h.runtime.pulls(), 1);
    assert_eq!(h.runtime.starts(), 1);
    assert!(h.runtime.is_running(&h.agent_id));
    // Progress and lifecycle are visible on the bus.
    let states = h.bus.states();
    assert_eq!(states.first().map(String::as_str), Some("pulling"));
    assert!(states.contains(&"starting".to_string()), "{states:?}");
    assert_eq!(states.last().map(String::as_str), Some("awake"));
    let pull_percents: Vec<u64> = h
        .bus
        .events()
        .iter()
        .filter(|e| e.payload["state"] == "pulling")
        .filter_map(|e| e.payload["percent"].as_u64())
        .collect();
    assert!(pull_percents.contains(&100), "{pull_percents:?}");
}

#[tokio::test]
async fn concurrent_wakes_share_one_server_owned_image_pull() {
    let h = harness_with(FakeComputerRuntime::default(), Duration::from_secs(600));
    h.runtime.set_pull_delay(Duration::from_millis(50));
    let other = AgentId::generate();

    let (first, repeated, second_agent) = tokio::join!(
        h.manager.wake(&h.agent_id),
        h.manager.wake(&h.agent_id),
        h.manager.wake(&other),
    );

    assert!(first.is_ok());
    assert!(repeated.is_ok());
    assert!(second_agent.is_ok());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while h.manager.state(&other).await != ComputerState::Awake {
        assert!(
            tokio::time::Instant::now() < deadline,
            "second computer never woke"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    wait_awake(&h).await;
    assert_eq!(h.runtime.pulls(), 1);
    assert_eq!(h.runtime.starts(), 2);
}

/// The message of the failed state that the Agent's Computer reaches.
async fn wait_failed(h: &Harness) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let ComputerState::Failed { message } = h.manager.state(&h.agent_id).await {
            return message;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "failure was not reported"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_failed_pull_stays_visible_and_a_retry_can_finish() {
    let h = harness_with(FakeComputerRuntime::default(), Duration::from_secs(600));
    h.runtime.fail_pull("registry unavailable");

    h.manager.wake(&h.agent_id).await.unwrap();
    assert_eq!(wait_failed(&h).await, "registry unavailable");

    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    assert_eq!(h.runtime.pulls(), 2);
}

/// The pulled image must carry the version label of the pin. An image
/// that does not fails the wake, and the old images stay.
#[tokio::test]
async fn a_pulled_image_with_another_version_fails_the_wake_and_removes_nothing() {
    let runtime = FakeComputerRuntime::default();
    runtime.pull_installs("9.9.9");
    runtime.add_old_image("sha256:old", false);
    let h = harness_with(runtime, Duration::from_secs(600));

    h.manager.wake(&h.agent_id).await.unwrap();
    let message = wait_failed(&h).await;

    assert!(message.contains("9.9.9"), "{message}");
    assert!(h.runtime.removed_images().is_empty());
    assert_eq!(h.runtime.starts(), 0);
}

/// A pull that a wake starts removes the old images of the repository
/// when it finishes.
#[tokio::test]
async fn a_finished_pull_removes_the_unused_old_images() {
    let runtime = FakeComputerRuntime::default();
    runtime.add_old_image("sha256:old", false);
    let h = harness_with(runtime, Duration::from_secs(600));

    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    assert_eq!(h.runtime.removed_images(), vec!["sha256:old"]);
}

/// Wait until the runtime has started `count` pulls.
async fn wait_pulls(runtime: &FakeComputerRuntime, count: u32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while runtime.pulls() < count {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pull did not start"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The daemon boots with the pinned image absent, and starts one pull
/// for the whole installation. A wake in each of two Workspaces joins
/// that pull, and neither starts a pull of its own.
#[tokio::test]
async fn wakes_in_two_workspaces_join_the_pull_that_the_boot_started() {
    let runtime = Arc::new(FakeComputerRuntime::default());
    runtime.hold_pulls();
    let managers = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());
    let boot = tokio::spawn({
        let managers = Arc::clone(&managers);
        async move { managers.prepare_image().await }
    });
    wait_pulls(&runtime, 1).await;

    let mut woken = Vec::new();
    for _ in 0..2 {
        let manager = managers.get(&WorkspaceId::generate());
        let agent_id = AgentId::generate();
        let state = manager.wake(&agent_id).await.unwrap();
        assert_eq!(state, ComputerState::Pulling { percent: 0 });
        woken.push((manager, agent_id));
    }
    runtime.release_pulls();
    for (manager, agent_id) in &woken {
        wait_awake_of_manager(manager, agent_id).await;
    }
    boot.await.unwrap();

    assert_eq!(runtime.pulls(), 1);
    assert_eq!(runtime.starts(), 2);
}

/// With the pinned image present at boot, the daemon pulls nothing and
/// removes each other image of the repository that no container uses.
/// An image that a container uses stays.
#[tokio::test]
async fn the_boot_removes_the_unused_old_images_and_keeps_an_image_in_use() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.add_old_image("sha256:unused", false);
    runtime.add_old_image("sha256:in-use", true);
    let managers = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());

    managers.prepare_image().await;

    assert_eq!(runtime.pulls(), 0);
    assert_eq!(runtime.removed_images(), vec!["sha256:unused"]);
    assert_eq!(runtime.old_images(), vec!["sha256:in-use"]);
}

/// Where Docker does not answer, the boot does nothing and logs nothing
/// louder than debug, so a Local Installation without Docker starts
/// with no warning.
#[tokio::test]
async fn the_boot_does_nothing_and_warns_of_nothing_when_docker_does_not_answer() {
    let runtime = Arc::new(FakeComputerRuntime::default());
    runtime.add_old_image("sha256:old", false);
    runtime.set_docker_answers(false);
    let managers = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());

    let ((), log) = crate::logs::logged(tracing::Level::INFO, managers.prepare_image()).await;

    assert_eq!(runtime.pulls(), 0);
    assert!(runtime.removed_images().is_empty());
    assert!(log.is_empty(), "{log}");
}

/// The version after the pinned one: the Computer Image that a newer
/// release pins.
fn next_version() -> String {
    let pinned = semver::Version::parse(IMAGE_VERSION).expect("the pin is SemVer");
    semver::Version::new(pinned.major, pinned.minor + 1, 0).to_string()
}

/// The old daemon pulled the Computer Image of the next release for the
/// Client App. That image is newer than the pin, so it stays until the
/// daemon of that release runs. An image of the pinned version or of an
/// older one goes.
#[tokio::test]
async fn the_boot_keeps_an_unused_image_that_is_newer_than_the_pin() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.add_other_image("sha256:next", Some(&next_version()), false);
    runtime.add_other_image("sha256:pinned-again", Some(IMAGE_VERSION), false);
    runtime.add_other_image("sha256:no-label", None, false);
    runtime.add_old_image("sha256:old", false);
    let managers = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());

    managers.prepare_image().await;

    assert_eq!(
        runtime.removed_images(),
        vec!["sha256:pinned-again", "sha256:no-label", "sha256:old"]
    );
    assert_eq!(runtime.old_images(), vec!["sha256:next"]);
}

/// The Computer Image of the next release, by its digest.
fn next_image() -> String {
    format!("{}@sha256:{}", image_repository(IMAGE), "d".repeat(64))
}

/// Before a restart to an Update, the Client App asks the daemon to pull
/// the Computer Image of the next release, and the daemon answers when
/// the pull ends.
#[tokio::test]
async fn the_daemon_pulls_an_image_of_the_computer_image_repository_by_its_digest() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let managers = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());

    managers.pull_image(&next_image()).await.expect("the pull");

    assert_eq!(runtime.pulled_images(), vec![next_image()]);
}

/// The daemon pulls only the repository of its pinned Computer Image,
/// and only by a digest: a tag can move, and another repository is not
/// a Computer Image.
#[tokio::test]
async fn a_pull_refuses_another_repository_and_a_reference_that_is_not_a_digest() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let managers = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());
    let repository = image_repository(IMAGE);
    let digest = "d".repeat(64);

    for image in [
        format!("example.com/other@sha256:{digest}"),
        format!("{repository}-other@sha256:{digest}"),
        format!("{repository}:0.99.0"),
        repository.to_string(),
        format!("{repository}@sha256:{}", "D".repeat(64)),
        format!("{repository}@sha256:{}", "d".repeat(63)),
        format!("{repository}:0.99.0@sha256:{digest}"),
    ] {
        let refused = managers.pull_image(&image).await;
        assert!(
            matches!(refused, Err(ImagePullError::Refused(_))),
            "{image}: {refused:?}"
        );
    }
    assert_eq!(runtime.pulls(), 0);
}

#[tokio::test]
async fn a_pull_answers_that_docker_does_not_answer() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.set_docker_answers(false);
    let managers = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());

    let answer = managers.pull_image(&next_image()).await;

    assert!(
        matches!(answer, Err(ImagePullError::NoDocker(_))),
        "{answer:?}"
    );
    assert_eq!(runtime.pulls(), 0);
}

#[tokio::test]
async fn a_failed_pull_answers_the_reason() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.fail_pull("registry denied the manifest");
    let managers = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());

    let answer = managers.pull_image(&next_image()).await;

    assert_eq!(
        answer,
        Err(ImagePullError::Failed(
            "registry denied the manifest".to_string()
        ))
    );
    assert!(runtime.pulled_images().is_empty());
}

/// A second request for the same image joins the pull that runs, and
/// each request gets the end of that one pull.
#[tokio::test]
async fn a_second_request_for_the_same_image_joins_the_pull() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.hold_pulls();
    let managers = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());
    let request = || {
        let managers = Arc::clone(&managers);
        tokio::spawn(async move { managers.pull_image(&next_image()).await })
    };

    let first = request();
    wait_pulls(&runtime, 1).await;
    let second = request();
    tokio::task::yield_now().await;
    runtime.release_pulls();

    assert_eq!(first.await.unwrap(), Ok(()));
    assert_eq!(second.await.unwrap(), Ok(()));
    assert_eq!(runtime.pulls(), 1);
}

/// The pull runs in a task of its own, so it ends also when the request
/// that started it goes away.
#[tokio::test]
async fn a_pull_goes_on_when_its_request_goes_away() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.hold_pulls();
    let managers = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());
    let request = tokio::spawn({
        let managers = Arc::clone(&managers);
        async move { managers.pull_image(&next_image()).await }
    });
    wait_pulls(&runtime, 1).await;

    request.abort();
    runtime.release_pulls();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while runtime.pulled_images().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pull stopped with its request"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(runtime.pulled_images(), vec![next_image()]);
}

#[tokio::test]
async fn wake_with_the_image_present_skips_the_pull() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    let state = h.manager.wake(&h.agent_id).await.unwrap();

    assert_eq!(state, ComputerState::Starting);
    wait_awake(&h).await;
    assert_eq!(h.runtime.pulls(), 0);
}

#[tokio::test]
async fn a_mismatched_image_version_is_refused() {
    let h = harness_with(FakeComputerRuntime::default(), Duration::from_secs(600));
    h.runtime.set_image_version(Some("0.0.1-dev"));

    let error = h.manager.wake(&h.agent_id).await.unwrap_err();

    assert!(matches!(
        error,
        ComputerError::VersionMismatch { found: Some(ref v) } if v == "0.0.1-dev"
    ));
    assert_eq!(h.manager.state(&h.agent_id).await, ComputerState::Off);
    assert_eq!(h.runtime.starts(), 0);
}

#[tokio::test]
async fn preview_serves_the_live_frame_awake_and_the_screenshot_asleep() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_millis(30));
    h.runtime.set_frame(b"live-png");

    // No computer, no screenshot: 404 territory.
    assert!(matches!(
        h.manager.preview(&h.agent_id).await,
        Err(ComputerError::NoPreview)
    ));

    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    let live = h.manager.preview(&h.agent_id).await.unwrap();
    assert!(live.live);
    assert_eq!(live.png, b"live-png");

    // Idle past the limit: the sweep stops the container and keeps a
    // final screenshot.
    tokio::time::sleep(Duration::from_millis(40)).await;
    h.manager.sweep().await;
    assert!(!h.runtime.is_running(&h.agent_id));
    assert_eq!(h.manager.state(&h.agent_id).await, ComputerState::Off);
    let asleep = h.manager.preview(&h.agent_id).await.unwrap();
    assert!(!asleep.live);
    assert_eq!(asleep.png, b"live-png");
}

#[tokio::test]
async fn idle_stop_then_rewake_preserves_the_volume() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_millis(20));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    tokio::time::sleep(Duration::from_millis(30)).await;
    h.manager.sweep().await;
    assert!(!h.runtime.is_running(&h.agent_id));
    assert!(
        h.runtime.has_volume(&h.agent_id),
        "volume survives the stop"
    );

    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    assert_eq!(h.runtime.starts(), 2);
    assert!(h.runtime.has_volume(&h.agent_id));
}

#[tokio::test]
async fn a_fresh_wake_does_not_idle_stop_early() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    h.manager.sweep().await;

    assert!(h.runtime.is_running(&h.agent_id));
    assert_eq!(h.manager.state(&h.agent_id).await, ComputerState::Awake);
}

/// A daemon that stops for good stops every Computer of every tenant,
/// fresh or not, and keeps each volume and last screen. No idle sweep
/// runs after the daemon to stop them.
#[tokio::test]
async fn a_stop_for_good_stops_the_computers_of_every_tenant() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let tenant_a = WorkspaceId::generate();
    let tenant_b = WorkspaceId::generate();
    let screens = tempfile::tempdir().unwrap();
    let managers = pagis_computer::ComputerManagers::new(pagis_computer::ComputerManagersDeps {
        runtime: Arc::clone(&runtime) as _,
        skills: Arc::new(FakeSkills::default()) as _,
        workspaces: Arc::new(FakeWorkspaces::with_timezone(&tenant_a, TEST_TIMEZONE)) as _,
        agents: Arc::new(pagis_computer::fake::FakeAgents::open()) as _,
        bus: Arc::new(RecordingBus::default()) as _,
        screens_dir: screens.path().to_path_buf(),
        idle_stop: Duration::from_secs(600),
        relay: pagis_computer::fake::loopback_relay(),
        caps: AwakeCaps::default(),
        cancel: tokio_util::sync::CancellationToken::new(),
        exit_daemon: None,
    });
    let (agent_a, agent_b) = (AgentId::generate(), AgentId::generate());
    for (tenant, agent) in [(&tenant_a, &agent_a), (&tenant_b, &agent_b)] {
        let manager = managers.get(tenant);
        manager.wake(agent).await.unwrap();
        wait_awake_of_manager(&manager, agent).await;
    }

    managers.stop_all().await;

    for (tenant, agent) in [(&tenant_a, &agent_a), (&tenant_b, &agent_b)] {
        let manager = managers.get(tenant);
        assert!(!runtime.is_running(agent), "{agent} still runs");
        assert!(runtime.has_volume(agent), "{agent} lost its volume");
        assert_eq!(manager.state(agent).await, ComputerState::Off);
        assert!(!manager.preview(agent).await.unwrap().live);
        assert_eq!(managers.ceiling().awake(tenant), 0);
    }
}

#[tokio::test]
async fn an_externally_running_container_is_adopted() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.runtime.boot_externally(&h.agent_id);

    assert_eq!(h.manager.state(&h.agent_id).await, ComputerState::Awake);
    let preview = h.manager.preview(&h.agent_id).await.unwrap();
    assert!(preview.live);
}

#[tokio::test]
async fn a_matching_version_label_cannot_adopt_a_different_image() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.runtime.boot_externally(&h.agent_id);
    h.runtime.set_running_image_matches(&h.agent_id, false);

    assert_eq!(h.manager.state(&h.agent_id).await, ComputerState::Off);
    assert!(!h.runtime.is_running(&h.agent_id));
}

#[tokio::test]
async fn a_running_container_is_not_adopted_until_its_control_endpoint_is_ready() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.runtime.boot_externally_not_ready(&h.agent_id);

    assert_eq!(h.manager.state(&h.agent_id).await, ComputerState::Off);

    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    assert_eq!(h.runtime.starts(), 1);
}

#[tokio::test]
async fn an_externally_running_container_on_a_stale_image_is_replaced() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.runtime
        .boot_externally_with_version(&h.agent_id, "0.0.1-dev");

    // The stale container is not adopted; it is stopped instead.
    assert_eq!(h.manager.state(&h.agent_id).await, ComputerState::Off);
    assert!(!h.runtime.is_running(&h.agent_id));

    // The wake then converges on the pinned image on its own.
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    assert_eq!(h.runtime.starts(), 1);
    assert_eq!(
        h.runtime.version(&h.agent_id).as_deref(),
        Some(IMAGE_VERSION)
    );
    // The agent data survives the replace.
    assert!(h.runtime.has_volume(&h.agent_id));
}

#[tokio::test]
async fn an_offer_relays_to_the_awake_computer_with_the_advertised_candidate() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    let answer = h
        .manager
        .offer(&h.agent_id, "v=0 offer", CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(answer, pagis_computer::fake::ANSWER);
    let offers = h.runtime.offers();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].0, "v=0 offer");
    // The candidate is the Media Relay's advertised address and the
    // port it opened for this session, never the container's own
    // (the harness relay advertises loopback and takes any free port).
    let (address, port) = offers[0]
        .1
        .candidate
        .rsplit_once(':')
        .expect("an ip:port candidate");
    assert_eq!(address, "127.0.0.1");
    assert!(port.parse::<u16>().expect("a UDP port") > 0, "{port}");
}

/// A `daemon` relay over one free port, and that port. A path holds
/// the only port of the range, so a test sees at once whether a path
/// still holds it.
fn one_port_relay() -> (Arc<dyn pagis_computer::MediaRelay>, u16) {
    let port = std::net::UdpSocket::bind("0.0.0.0:0")
        .expect("a free UDP port")
        .local_addr()
        .expect("the address of the free port")
        .port();
    let relay = Arc::new(pagis_computer::DaemonRelay::new(
        pagis_computer::MediaForwarder::new("127.0.0.1".to_string(), port..=port),
    ));
    (relay, port)
}

/// Whether nothing holds `port` now.
fn port_is_free(port: u16) -> bool {
    std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok()
}

/// The ICE credentials of the fake pipeline's answer.
fn answered() -> IceCredentials {
    IceCredentials::of_answer(pagis_computer::fake::ANSWER)
        .expect("the fake answer has credentials")
}

/// Whether a check under `credentials` crosses `path` to a pipeline
/// that registered with it, as screend does.
async fn check_crosses(path: &pagis_computer::MediaPath, credentials: &IceCredentials) -> bool {
    let pipeline = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    pipeline
        .send_to(&path.registration(), ("127.0.0.1", path.port))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let browser = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let check = ice_check(credentials, false);
    browser
        .send_to(&check, path.candidate.as_str())
        .await
        .unwrap();
    let mut buffer = [0u8; 512];
    match tokio::time::timeout(Duration::from_millis(500), pipeline.recv_from(&mut buffer)).await {
        Ok(Ok((read, _))) => buffer[..read] == check[..],
        _ => false,
    }
}

/// The daemon gives the path the ICE credentials of the pipeline's
/// answer. The viewer signs its checks with them, so its checks cross
/// the path, and a check signed with another password does not.
#[tokio::test]
async fn an_offer_gives_its_path_the_credentials_of_the_answer() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    h.manager
        .offer(&h.agent_id, "v=0 offer", CancellationToken::new())
        .await
        .unwrap();

    let path = h.runtime.offers()[0].1.clone();
    let another_password = IceCredentials {
        pwd: "anotherpasswordanother12".to_string(),
        ..answered()
    };
    assert!(
        !check_crosses(&path, &another_password).await,
        "a check under another password crossed the path"
    );
    assert!(
        check_crosses(&path, &answered()).await,
        "the viewer's check did not cross the path"
    );
}

/// A failed offer gives its port back at once. A viewer that retries
/// an offer the pipeline refused, or a Member who sends offers in a
/// loop, holds no port for a path that no browser uses.
#[tokio::test]
async fn a_failed_offer_releases_its_port_at_once() {
    let (relay, port) = one_port_relay();
    let h = harness_with_relay(
        FakeComputerRuntime::with_image(),
        Duration::from_secs(600),
        AwakeCaps::default(),
        relay,
    );
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    h.runtime
        .answer_offers_with(Err("offer refused: the offer does not parse"));

    let error = h
        .manager
        .offer(&h.agent_id, "v=0 offer", CancellationToken::new())
        .await
        .unwrap_err();

    assert!(matches!(error, ComputerError::Runtime(_)), "{error}");
    assert_eq!(
        h.runtime.offers().len(),
        1,
        "the offer reached the pipeline"
    );
    assert!(port_is_free(port), "the refused offer still holds its port");
}

/// An answer with no ICE credentials is refused, and its port is free
/// at once. Without the credentials, the relay cannot tell the viewer's
/// checks from a stranger's.
#[tokio::test]
async fn an_answer_without_ice_credentials_is_refused_and_releases_its_port() {
    let (relay, port) = one_port_relay();
    let h = harness_with_relay(
        FakeComputerRuntime::with_image(),
        Duration::from_secs(600),
        AwakeCaps::default(),
        relay,
    );
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    h.runtime
        .answer_offers_with(Ok("v=0 an answer with no ICE credentials"));

    let error = h
        .manager
        .offer(&h.agent_id, "v=0 offer", CancellationToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("ICE credentials"), "{error}");
    assert!(
        port_is_free(port),
        "the refused answer still holds its port"
    );
}

/// One media path for each awake Computer: a second offer for the same
/// Computer closes the first path and frees its port before it opens
/// its own. Repeated offers, from a tab that reloads or from a loop,
/// hold one port, so the range of one port serves every offer.
#[tokio::test]
async fn a_second_offer_for_the_same_computer_closes_the_first_path_and_frees_its_port() {
    let (relay, port) = one_port_relay();
    let h = harness_with_relay(
        FakeComputerRuntime::with_image(),
        Duration::from_secs(600),
        AwakeCaps::default(),
        relay,
    );
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    for _ in 0..3 {
        h.manager
            .offer(&h.agent_id, "v=0 offer", CancellationToken::new())
            .await
            .expect("the offer takes the port that the last path freed");
    }

    let offers = h.runtime.offers();
    assert_eq!(offers.len(), 3);
    assert!(offers.iter().all(|(_, path)| path.port == port));
    // The pipeline of the first path cannot register with the path that
    // holds the port now: its token is of a closed path.
    assert!(!check_crosses(&offers[0].1, &answered()).await);
    assert!(check_crosses(&offers[2].1, &answered()).await);
}

/// Each awake Computer keeps a path of its own: an offer for one
/// Computer closes no path of another.
#[tokio::test]
async fn an_offer_for_one_computer_leaves_the_path_of_another_open() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    let other = AgentId::generate();
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    h.manager.wake(&other).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while h.manager.state(&other).await != ComputerState::Awake {
        assert!(tokio::time::Instant::now() < deadline, "never woke");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    h.manager
        .offer(&h.agent_id, "v=0 offer", CancellationToken::new())
        .await
        .unwrap();
    h.manager
        .offer(&other, "v=0 offer", CancellationToken::new())
        .await
        .unwrap();

    let offers = h.runtime.offers();
    assert!(
        check_crosses(&offers[0].1, &answered()).await,
        "the offer for another Computer closed this path"
    );
}

/// The path of a Computer closes when the Computer goes to sleep. No
/// screen is left to watch, and the port is free at once.
#[tokio::test]
async fn the_path_of_a_computer_closes_when_the_computer_goes_to_sleep() {
    let (relay, port) = one_port_relay();
    let h = harness_with_relay(
        FakeComputerRuntime::with_image(),
        Duration::from_secs(600),
        AwakeCaps::default(),
        relay,
    );
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    h.manager
        .offer(&h.agent_id, "v=0 offer", CancellationToken::new())
        .await
        .unwrap();
    assert!(!port_is_free(port), "the live path holds no port");

    h.manager.sleep(&h.agent_id).await.unwrap();

    assert!(
        port_is_free(port),
        "the path of the sleeping Computer still holds its port"
    );
}

/// A relay with no port left refuses the viewer and never reaches the
/// pipeline. The range is how many people watch at once, so the
/// refusal names the setting that widens it.
#[tokio::test]
async fn an_offer_with_no_media_port_left_is_refused() {
    // A range of one port, and something else already on it.
    let held = std::net::UdpSocket::bind("0.0.0.0:0").expect("a socket to hold the only port");
    let taken = held.local_addr().expect("the held address").port();
    let relay = Arc::new(pagis_computer::DaemonRelay::new(
        pagis_computer::MediaForwarder::new("127.0.0.1".to_string(), taken..=taken),
    ));
    let h = harness_with_relay(
        FakeComputerRuntime::with_image(),
        Duration::from_secs(600),
        AwakeCaps::default(),
        relay,
    );
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    let error = h
        .manager
        .offer(&h.agent_id, "v=0 offer", CancellationToken::new())
        .await
        .unwrap_err();

    assert!(
        matches!(error, ComputerError::NoMediaPath { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("media_port_first"), "{error}");
    assert!(h.runtime.offers().is_empty());
}

#[tokio::test]
async fn an_offer_to_an_asleep_computer_is_refused() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    let error = h
        .manager
        .offer(&h.agent_id, "v=0 offer", CancellationToken::new())
        .await
        .unwrap_err();

    assert!(matches!(error, ComputerError::Asleep));
    assert!(h.runtime.offers().is_empty());
}

fn fast_takeover_timing() -> TakeoverTiming {
    TakeoverTiming {
        idle: Duration::from_millis(60),
        countdown: Duration::from_millis(60),
        poll: Duration::from_millis(10),
    }
}

async fn wait_for_event(h: &Harness, event_type: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if h.bus
            .events()
            .iter()
            .any(|event| event.event_type == event_type)
        {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{event_type} never published"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn takeover_denies_the_agents_lease_until_handback() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    assert!(h.manager.lease(&h.agent_id).await.is_ok());
    h.manager.takeover(&h.agent_id).await.unwrap();
    assert!(matches!(
        h.manager.lease(&h.agent_id).await,
        Err(ComputerError::SwitchHeld {
            holder: InputHolder::User
        })
    ));

    assert!(h.manager.handback(&h.agent_id, "explicit").await.unwrap());
    assert!(h.manager.lease(&h.agent_id).await.is_ok());
}

#[tokio::test]
async fn takeover_flips_the_pipeline_switch_and_publishes_events() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    h.manager.takeover(&h.agent_id).await.unwrap();
    assert_eq!(h.runtime.holder(&h.agent_id), InputHolder::User);
    assert_eq!(h.manager.holder(&h.agent_id), InputHolder::User);
    wait_for_event(&h, "screen.takeover_started").await;

    h.manager.handback(&h.agent_id, "explicit").await.unwrap();
    assert_eq!(h.runtime.holder(&h.agent_id), InputHolder::Agent);
    wait_for_event(&h, "screen.takeover_ended").await;
    let ended = h
        .bus
        .events()
        .into_iter()
        .find(|event| event.event_type == "screen.takeover_ended")
        .unwrap();
    assert_eq!(ended.payload["reason"], "explicit");
    assert!(ended.payload["duration_ms"].is_u64());
}

/// When every Session of the Person ends, nobody can hand a Computer
/// back, so the daemon hands back each one the Person holds. A Computer
/// that the daemon holds for a fill is not the Person's to give back.
#[tokio::test]
async fn handing_back_every_takeover_gives_each_computer_the_person_holds_to_its_agent() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    let second = AgentId::generate();
    let filling = AgentId::generate();
    for agent_id in [&h.agent_id, &second, &filling] {
        h.manager.wake(agent_id).await.unwrap();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    for agent_id in [&h.agent_id, &second, &filling] {
        while h.manager.state(agent_id).await != ComputerState::Awake {
            assert!(tokio::time::Instant::now() < deadline, "never woke");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    h.manager.takeover(&h.agent_id).await.unwrap();
    h.manager.takeover(&second).await.unwrap();
    let _hold = h.manager.daemon_hold(&filling).await.unwrap();

    h.manager.handback_all("sessions ended").await;

    for agent_id in [&h.agent_id, &second] {
        assert_eq!(h.manager.holder(agent_id), InputHolder::Agent);
        assert_eq!(h.runtime.holder(agent_id), InputHolder::Agent);
    }
    assert_eq!(h.manager.holder(&filling), InputHolder::Daemon);
    let ended: Vec<_> = h
        .bus
        .events()
        .into_iter()
        .filter(|event| event.event_type == "screen.takeover_ended")
        .collect();
    assert_eq!(ended.len(), 2, "{ended:?}");
    assert!(
        ended
            .iter()
            .all(|event| event.payload["reason"] == "sessions ended"),
        "{ended:?}"
    );
}

#[tokio::test]
async fn takeover_of_an_asleep_computer_is_refused() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    assert!(matches!(
        h.manager.takeover(&h.agent_id).await,
        Err(ComputerError::Asleep)
    ));
}

#[tokio::test]
async fn handback_without_a_takeover_is_a_no_op() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    assert!(!h.manager.handback(&h.agent_id, "explicit").await.unwrap());
    assert!(
        !h.bus
            .events()
            .iter()
            .any(|event| event.event_type == "screen.takeover_ended")
    );
}

#[tokio::test]
async fn inactivity_runs_the_countdown_and_hands_back() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.set_takeover_timing(fast_takeover_timing());
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    h.runtime.set_user_idle_ms(70);

    h.manager.takeover(&h.agent_id).await.unwrap();

    wait_for_event(&h, "screen.handback_countdown").await;
    h.runtime.set_user_idle_ms(500);
    wait_for_event(&h, "screen.takeover_ended").await;
    let ended = h
        .bus
        .events()
        .into_iter()
        .find(|event| event.event_type == "screen.takeover_ended")
        .unwrap();
    assert_eq!(ended.payload["reason"], "inactivity");
    assert_eq!(h.manager.holder(&h.agent_id), InputHolder::Agent);
    assert_eq!(h.runtime.holder(&h.agent_id), InputHolder::Agent);
}

#[tokio::test]
async fn input_during_the_countdown_cancels_it() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.set_takeover_timing(fast_takeover_timing());
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    h.runtime.set_user_idle_ms(70);

    h.manager.takeover(&h.agent_id).await.unwrap();

    wait_for_event(&h, "screen.handback_countdown").await;
    h.runtime.set_user_idle_ms(0);
    wait_for_event(&h, "screen.handback_countdown_canceled").await;
    assert_eq!(h.manager.holder(&h.agent_id), InputHolder::User);
    assert!(
        !h.bus
            .events()
            .iter()
            .any(|event| event.event_type == "screen.takeover_ended")
    );
}

#[tokio::test]
async fn wake_while_awake_is_a_no_op() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    let state = h.manager.wake(&h.agent_id).await.unwrap();

    assert_eq!(state, ComputerState::Awake);
    assert_eq!(h.runtime.starts(), 1);
}

// ---- The daemon's hold on the switch (ADR-0013) ----

/// While the daemon holds the switch, the agent's lease and its input
/// are refused, and the daemon's own channel is the browser: it opens
/// its tab and fills there, and it sends no keystroke.
#[tokio::test]
async fn a_daemon_hold_denies_the_agents_lease_and_its_input() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    let hold = h.manager.daemon_hold(&h.agent_id).await.unwrap();
    assert_eq!(h.manager.holder(&h.agent_id), InputHolder::Daemon);
    assert_eq!(h.runtime.holder(&h.agent_id), InputHolder::Daemon);

    // The lease end.
    assert!(matches!(
        h.manager.lease(&h.agent_id).await,
        Err(ComputerError::SwitchHeld {
            holder: InputHolder::Daemon
        })
    ));
    // The screend end: a batch that speaks for the agent is refused
    // while the daemon holds, so the switch means the same at both.
    let typed = vec![pagis_computer::exec::InputOp::Text {
        text: "hello".to_string(),
    }];
    assert!(h.manager.input(&h.agent_id, &typed).await.is_err());
    // The daemon writes through its browser channel.
    let page = hold.open("https://example.com/signin").await.unwrap();
    assert_eq!(page, "https://example.com/signin");
    hold.fill(
        "https://example.com",
        &[pagis_computer::FillField::password("hunter2")],
    )
    .await
    .unwrap();
    assert_eq!(
        h.runtime.browser_opens(),
        vec!["https://example.com/signin".to_string()]
    );
    assert_eq!(
        h.runtime.browser_fills(),
        vec![(
            "https://example.com".to_string(),
            vec![pagis_computer::FillField::password("hunter2")]
        )]
    );
    assert!(h.runtime.inputs().is_empty(), "the daemon sent keystrokes");

    hold.release().await;
    assert_eq!(h.manager.holder(&h.agent_id), InputHolder::Agent);
    assert!(h.manager.lease(&h.agent_id).await.is_ok());
}

/// A page reacts to a click over a few frames. A settled frame waits
/// until two frames in a row are the same, so the model sees the page
/// after its action and not in the middle of it.
#[tokio::test(start_paused = true)]
async fn a_settled_frame_waits_until_the_screen_stops_changing() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    h.runtime
        .play_frames(&[b"picker-closed", b"picker-opening", b"picker-open"]);

    let frame = h.manager.settled_frame(&h.agent_id).await.unwrap();

    assert_eq!(frame, b"picker-open");
}

/// A screen that never stops changing (a video, a spinner) gives the
/// last frame when the settle time ends.
#[tokio::test(start_paused = true)]
async fn a_settled_frame_stops_waiting_on_a_screen_that_keeps_changing() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    let frames: Vec<Vec<u8>> = (0..1_000)
        .map(|n| format!("frame-{n}").into_bytes())
        .collect();
    let frames: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
    h.runtime.play_frames(&frames);
    let started = tokio::time::Instant::now();

    h.manager.settled_frame(&h.agent_id).await.unwrap();

    assert!(started.elapsed() <= Duration::from_secs(3) + Duration::from_millis(250));
}

#[tokio::test]
async fn a_daemon_hold_suppresses_model_visible_capture_and_refreshes_on_release() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.runtime.set_frame(b"before-fill");
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    // One frame before the fill leaves a stored screenshot behind.
    h.manager.live_frame(&h.agent_id).await.unwrap();

    let hold = h.manager.daemon_hold(&h.agent_id).await.unwrap();
    h.runtime.set_frame(b"secret-on-screen");

    // No run screenshot can be taken while the daemon fills.
    assert!(matches!(
        h.manager.live_frame(&h.agent_id).await,
        Err(ComputerError::SwitchHeld {
            holder: InputHolder::Daemon
        })
    ));
    // The user's live view still serves, and retains nothing.
    let watching = h.manager.preview(&h.agent_id).await.unwrap();
    assert!(watching.live);
    assert_eq!(watching.png, b"secret-on-screen");
    assert_eq!(
        std::fs::read(h.screens.path().join(format!("{}.png", h.agent_id))).unwrap(),
        b"before-fill",
        "the stored frame never carries the fill"
    );

    h.runtime.set_frame(b"after-fill");
    hold.release().await;
    assert_eq!(
        std::fs::read(h.screens.path().join(format!("{}.png", h.agent_id))).unwrap(),
        b"after-fill",
        "the release re-screenshots, as a handback does"
    );
}

#[tokio::test]
async fn a_daemon_hold_is_refused_while_the_user_holds() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    h.manager.takeover(&h.agent_id).await.unwrap();

    assert!(matches!(
        h.manager.daemon_hold(&h.agent_id).await,
        Err(ComputerError::SwitchHeld {
            holder: InputHolder::User
        })
    ));
    // And a handback does not release a hold the user never took.
    assert_eq!(h.manager.holder(&h.agent_id), InputHolder::User);
}

#[tokio::test]
async fn a_daemon_hold_publishes_its_start_and_end() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    let hold = h.manager.daemon_hold(&h.agent_id).await.unwrap();
    wait_for_event(&h, "screen.daemon_hold_started").await;
    hold.release().await;
    wait_for_event(&h, "screen.daemon_hold_ended").await;
}

fn shell(command: &str) -> ShellCommand {
    ShellCommand {
        command: command.to_string(),
        timeout: Duration::from_secs(120),
        cwd: None,
        stdin: None,
        output_cap: None,
    }
}

#[tokio::test]
async fn shell_wakes_the_computer_and_wraps_the_command_in_a_timeout() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.runtime.push_exec_outcome(ExecOutcome {
        exit_code: 0,
        stdout: "hi\n".to_string(),
        stderr: String::new(),
        truncated: false,
    });

    let outcome = h
        .manager
        .shell(&h.agent_id, shell("echo hi"))
        .await
        .unwrap();

    assert_eq!(outcome.stdout, "hi\n");
    assert!(h.runtime.is_running(&h.agent_id), "the shell woke it");
    let request = h.runtime.execs().pop().expect("one exec");
    assert_eq!(
        request.argv,
        vec!["timeout", "--kill-after=5", "120", "bash", "-c", "echo hi"]
    );
    assert_eq!(request.user, "agent");
    assert_eq!(request.cwd, SHELL_HOME);
    assert!(request.stdin.is_none());
}

/// A command's output goes into the model request, so the default cap
/// keeps it small: the head and tail of each stream together hold at
/// most 16 KiB, which is the order of the caps Codex and Claude Code put
/// on a shell result. A command that prints more writes to a file and
/// reads the part it needs.
#[tokio::test]
async fn shell_keeps_at_most_16_kib_of_each_stream_by_default() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    h.manager
        .shell(&h.agent_id, shell("cat big.html"))
        .await
        .unwrap();

    let request = h.runtime.execs().pop().expect("one exec");
    assert!(
        request.output_cap.head + request.output_cap.tail <= 16 * 1024,
        "{:?}",
        request.output_cap
    );
    // The tail keeps the end of the output, where a command reports.
    assert!(request.output_cap.tail > 0);
}

#[tokio::test]
async fn shell_runs_in_a_fixed_environment() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    h.manager.shell(&h.agent_id, shell("env")).await.unwrap();

    let request = h.runtime.execs().pop().expect("one exec");
    for entry in [
        "HOME=/data/agent",
        "USER=agent",
        "LOGNAME=agent",
        "LANG=en_US.UTF-8",
        "TZ=Australia/Sydney",
    ] {
        assert!(request.env.contains(&entry.to_string()), "missing {entry}");
    }
    assert!(request.env.iter().any(|entry| entry.starts_with("PATH=")));
}

#[tokio::test]
async fn a_container_boots_with_the_workspace_timezone_and_the_locale() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    h.manager.wake(&h.agent_id).await.expect("wake");
    wait_awake(&h).await;

    let env = h.runtime.start_envs().pop().expect("one start");
    assert!(
        env.contains(&format!("TZ={TEST_TIMEZONE}")),
        "the container boots on the workspace clock: {env:?}"
    );
    assert!(
        env.contains(&"LANG=en_US.UTF-8".to_string()),
        "the container boots in the locale of the image: {env:?}"
    );
}

/// Every container the daemon starts runs screend and with it the Exit
/// Proxy (ADR-0029), so every one boots with the proxy entries: an
/// Agent's Computer and the Plugin Computer alike. Each tool reads one
/// case of the names, and loopback and the Docker host stay out of the
/// proxy.
#[tokio::test]
async fn every_computer_boots_with_the_exit_proxy_in_its_environment() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    h.manager.wake(&h.agent_id).await.expect("wake");
    wait_awake(&h).await;
    h.manager
        .ensure_plugin_computer(Vec::new())
        .await
        .expect("the plugin computer wakes");

    let envs = h.runtime.start_envs();
    assert_eq!(envs.len(), 2, "{envs:?}");
    for env in envs {
        for entry in [
            "HTTP_PROXY=http://127.0.0.1:3128",
            "HTTPS_PROXY=http://127.0.0.1:3128",
            "http_proxy=http://127.0.0.1:3128",
            "https_proxy=http://127.0.0.1:3128",
            "NO_PROXY=localhost,127.0.0.1,::1,host.docker.internal",
            "no_proxy=localhost,127.0.0.1,::1,host.docker.internal",
        ] {
            assert!(env.contains(&entry.to_string()), "missing {entry}: {env:?}");
        }
    }
}

/// The exit entries of one start environment.
fn exit_entries(env: &[String]) -> Vec<String> {
    env.iter()
        .filter(|entry| entry.starts_with("PAGIS_EXIT_"))
        .cloned()
        .collect()
}

/// On a Server an Agent's Computer names the exit listener of the
/// daemon, and its Exit Proxy starts in the mode of its Person's choice
/// (ADR-0029): `home` while the Person has a Home Exit, `direct` while
/// they have none. The choice of the store reaches the next wake.
#[tokio::test]
async fn an_agent_computer_on_a_server_starts_in_the_mode_of_its_persons_choice() {
    let h = server_harness();

    h.manager.wake(&h.agent_id).await.expect("wake");
    wait_awake(&h).await;
    let env = h.runtime.start_envs().pop().expect("one start");
    assert_eq!(
        exit_entries(&env),
        [
            format!("PAGIS_EXIT_DAEMON={EXIT_DAEMON}"),
            "PAGIS_EXIT_MODE=direct".to_string(),
        ]
    );
    h.manager.sleep(&h.agent_id).await.expect("sleep");

    h.workspaces
        .set_home_exit(&h.workspace_id, Some(&pagis_core::HostId::generate()))
        .await
        .expect("the Home Exit is written");
    let second = AgentId::generate();
    for agent_id in [&h.agent_id, &second] {
        h.manager.wake(agent_id).await.expect("wake");
        wait_awake_of(&h, agent_id).await;
        let env = h.runtime.start_envs().pop().expect("a start");
        assert_eq!(
            exit_entries(&env),
            [
                format!("PAGIS_EXIT_DAEMON={EXIT_DAEMON}"),
                "PAGIS_EXIT_MODE=home".to_string(),
            ]
        );
    }
}

/// The Plugin Computer serves the Plugins of the Workspace, which call
/// APIs and not sites that score addresses, so it stays in `Direct` mode
/// with no exit listener, whatever the Person chose. A Computer of a
/// Local Installation leaves from the owner's own connection, so it has
/// no exit listener either.
#[tokio::test]
async fn the_plugin_computer_and_a_local_installation_start_direct_with_no_listener() {
    let server = server_harness();
    server
        .workspaces
        .set_home_exit(&server.workspace_id, Some(&pagis_core::HostId::generate()))
        .await
        .expect("the Home Exit is written");
    server
        .manager
        .ensure_plugin_computer(Vec::new())
        .await
        .expect("the plugin computer wakes");
    let env = server.runtime.start_envs().pop().expect("one start");
    assert_eq!(exit_entries(&env), Vec::<String>::new());

    let local = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    local
        .workspaces
        .set_home_exit(&local.workspace_id, Some(&pagis_core::HostId::generate()))
        .await
        .expect("the Home Exit is written");
    local.manager.wake(&local.agent_id).await.expect("wake");
    wait_awake(&local).await;
    let env = local.runtime.start_envs().pop().expect("one start");
    assert_eq!(exit_entries(&env), Vec::<String>::new());
}

/// The exit listener knows a Computer by its token: the token of an
/// awake Agent's Computer names that Computer, and the token of the
/// Plugin Computer, of a Computer that sleeps, and of nobody, name
/// nothing.
#[tokio::test]
async fn a_token_names_the_awake_agent_computer_that_holds_it() {
    use pagis_computer::ComputerTokens;

    let h = server_harness();
    h.manager.wake(&h.agent_id).await.expect("wake");
    wait_awake(&h).await;
    let plugin = h
        .manager
        .ensure_plugin_computer(Vec::new())
        .await
        .expect("the plugin computer wakes");
    let token = format!("fake-token-{}", h.agent_id);

    assert_eq!(
        h.manager.computer_of(&token),
        Some(pagis_computer::ComputerOwner::new(
            h.workspace_id.clone(),
            h.agent_id.clone()
        ))
    );
    assert_eq!(h.manager.computer_of(&plugin.token), None);
    assert_eq!(h.manager.computer_of("fake-token-nobody"), None);
    assert_eq!(h.manager.computer_of(""), None);

    h.manager.sleep(&h.agent_id).await.expect("sleep");
    assert_eq!(h.manager.computer_of(&token), None);
}

#[tokio::test]
async fn a_new_workspace_timezone_reaches_the_next_wake() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_millis(20));
    h.manager.wake(&h.agent_id).await.expect("wake");
    wait_awake(&h).await;

    h.workspaces
        .set_timezone(&h.workspace_id, "Europe/Paris")
        .await
        .expect("the timezone changes");
    // The running computer keeps the clock it booted with: the new
    // value waits for the idle-stop and the next wake.
    tokio::time::sleep(Duration::from_millis(30)).await;
    h.manager.sweep().await;
    assert!(!h.runtime.is_running(&h.agent_id));
    h.manager.wake(&h.agent_id).await.expect("second wake");
    wait_awake(&h).await;

    let env = h.runtime.start_envs().pop().expect("the second start");
    assert!(
        env.contains(&"TZ=Europe/Paris".to_string()),
        "the next wake carries the new timezone: {env:?}"
    );
}

#[tokio::test]
async fn shell_honors_a_working_directory_and_a_deadline() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    h.manager
        .shell(
            &h.agent_id,
            ShellCommand {
                command: "ls".to_string(),
                timeout: Duration::from_secs(30),
                cwd: Some("/data/agent/software".to_string()),
                stdin: None,
                output_cap: None,
            },
        )
        .await
        .unwrap();

    let request = h.runtime.execs().pop().expect("one exec");
    assert_eq!(request.cwd, "/data/agent/software");
    assert_eq!(request.argv[2], "30");
}

#[tokio::test]
async fn an_unscripted_exec_answers_with_exit_zero() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    let outcome = h.manager.shell(&h.agent_id, shell("true")).await.unwrap();

    assert_eq!(outcome.exit_code, 0);
    assert!(outcome.stdout.is_empty());
}

#[tokio::test]
async fn a_command_in_flight_holds_off_the_idle_stop() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_millis(1));
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    h.runtime.set_exec_delay(Duration::from_millis(150));

    let manager = Arc::clone(&h.manager);
    let agent_id = h.agent_id.clone();
    let command = tokio::spawn(async move { manager.shell(&agent_id, shell("sleep 1")).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    h.manager.sweep().await;
    assert!(
        h.runtime.is_running(&h.agent_id),
        "the pin keeps the computer awake"
    );

    // The exec survived the sweep: a stopped container answers 137.
    let outcome = command.await.unwrap().unwrap();
    assert_eq!(outcome.exit_code, 0);

    // The pin is gone, so the next sweep stops the idle computer.
    tokio::time::sleep(Duration::from_millis(10)).await;
    h.manager.sweep().await;
    assert!(!h.runtime.is_running(&h.agent_id));
}

#[tokio::test]
async fn shell_passes_stdin_and_a_wider_output_cap_to_the_runtime() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    h.manager
        .shell(
            &h.agent_id,
            ShellCommand {
                command: "./bin/forecast".to_string(),
                timeout: Duration::from_secs(60),
                cwd: None,
                stdin: Some(b"{\"city\":\"Berlin\"}".to_vec()),
                output_cap: Some(OutputCap {
                    head: 1024,
                    tail: 8,
                }),
            },
        )
        .await
        .unwrap();

    let request = h.runtime.execs().pop().expect("one exec");
    assert_eq!(request.stdin, Some(b"{\"city\":\"Berlin\"}".to_vec()));
    assert_eq!(
        request.output_cap,
        OutputCap {
            head: 1024,
            tail: 8
        }
    );
}

#[tokio::test]
async fn upload_archive_wakes_the_computer_and_reaches_the_runtime() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    h.manager
        .upload_archive(&h.agent_id, "/data/agent/.pagis", b"tar".to_vec())
        .await
        .unwrap();

    assert!(h.runtime.is_running(&h.agent_id), "the upload woke it");
    assert_eq!(
        h.runtime.uploads(),
        vec![("/data/agent/.pagis".to_string(), b"tar".to_vec())]
    );
}

#[tokio::test]
async fn a_refused_upload_is_a_runtime_error() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.runtime.fail_upload("no such directory");

    let error = h
        .manager
        .upload_archive(&h.agent_id, "/data/agent/missing", b"tar".to_vec())
        .await
        .expect_err("the upload failed");

    assert!(error.to_string().contains("no such directory"), "{error}");
}

/// The chunks of the stream land in one spool file, and the file
/// comes back at its first byte.
#[tokio::test]
async fn a_download_spools_the_whole_stream_and_reads_from_its_start() {
    use std::io::Read;

    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.runtime
        .set_download_stream("/data/agent/software/weather", || {
            Box::pin(futures::stream::iter([
                Ok(bytes::Bytes::from_static(b"first ")),
                Ok(bytes::Bytes::from_static(b"second")),
            ]))
        });

    let mut spool = h
        .manager
        .download_archive(&h.agent_id, "/data/agent/software/weather")
        .await
        .expect("the download succeeds");

    let mut text = String::new();
    spool.read_to_string(&mut text).expect("the spool reads");
    assert_eq!(text, "first second");
    assert!(h.runtime.is_running(&h.agent_id), "the download woke it");
    assert_eq!(
        h.runtime.downloaded(),
        vec!["/data/agent/software/weather".to_string()]
    );
}

#[tokio::test]
async fn a_download_that_breaks_off_is_a_runtime_error() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.runtime
        .set_download_stream("/data/agent/software/weather", || {
            Box::pin(futures::stream::iter([
                Ok(bytes::Bytes::from_static(b"first ")),
                Err(std::io::Error::other("the connection was reset")),
            ]))
        });

    let error = h
        .manager
        .download_archive(&h.agent_id, "/data/agent/software/weather")
        .await
        .expect_err("the download fails");

    assert!(
        matches!(&error, ComputerError::Runtime(message) if message.contains("the connection was reset")),
        "{error}"
    );
}

/// One mount of a plugin named `weather`.
fn weather_mount() -> SkillMount {
    SkillMount {
        plugin: "weather".to_string(),
        skills_dir: std::path::PathBuf::from("/data/plugins/p1/skills"),
    }
}

#[tokio::test]
async fn a_boot_mounts_the_skills_of_every_granted_plugin_read_only() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.skills.set(vec![weather_mount()]);

    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    let mounts = h.runtime.mounts();
    assert_eq!(mounts.len(), 1);
    assert_eq!(
        mounts[0],
        vec![BindMount {
            host: std::path::PathBuf::from("/data/plugins/p1/skills"),
            container: format!("{PLUGIN_MOUNT_ROOT}/weather/skills"),
            read_only: true,
        }]
    );
    assert_eq!(
        mounts[0][0].spec(),
        "/data/plugins/p1/skills:/opt/plugins/weather/skills:ro"
    );
}

#[tokio::test]
async fn a_wake_after_a_grant_change_replaces_the_computer_with_the_new_mounts() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));

    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    assert_eq!(h.runtime.starts(), 1);

    h.skills.set(vec![weather_mount()]);
    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    assert_eq!(h.runtime.starts(), 2, "the stale computer was replaced");
    assert_eq!(
        h.runtime.mounts().last().expect("the second start").len(),
        1
    );
    assert!(
        h.runtime.has_volume(&h.agent_id),
        "the data volume survived"
    );
}

#[tokio::test]
async fn a_wake_with_the_same_mounts_keeps_the_running_computer() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.skills.set(vec![weather_mount()]);

    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;
    h.manager.wake(&h.agent_id).await.unwrap();

    assert_eq!(h.runtime.starts(), 1);
}

#[tokio::test]
async fn an_adopted_computer_with_other_mounts_is_replaced_at_the_next_wake() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    // A container this process did not boot, carrying no plugin mount.
    h.runtime
        .boot_externally_with(&h.agent_id, IMAGE_VERSION, &[]);
    h.skills.set(vec![weather_mount()]);

    h.manager.wake(&h.agent_id).await.unwrap();
    wait_awake(&h).await;

    assert_eq!(h.runtime.starts(), 1, "the adopted container was replaced");
    assert_eq!(h.runtime.mounts()[0].len(), 1);
}

/// The cap on simultaneously awake Computers, per tenant. The
/// refusal is a sentence the Run can read out.
#[tokio::test]
async fn a_tenant_cannot_hold_more_computers_awake_than_its_cap() {
    let h = harness_with_caps(
        FakeComputerRuntime::with_image(),
        Duration::from_secs(600),
        AwakeCaps {
            per_tenant: 2,
            per_server: 10,
        },
    );
    let first = AgentId::generate();
    let second = AgentId::generate();
    let third = AgentId::generate();

    h.manager.wake(&first).await.expect("the first wakes");
    h.manager.wake(&second).await.expect("the second wakes");

    let refused = h
        .manager
        .wake(&third)
        .await
        .expect_err("the third meets the cap");
    assert!(
        matches!(
            refused,
            ComputerError::AwakeCapReached {
                scope: "your office",
                cap: 2
            }
        ),
        "{refused}"
    );
    assert!(refused.to_string().contains("has to sleep"), "{refused}");
    assert_eq!(h.ceiling.awake(&h.workspace_id), 2);

    // A computer that sleeps gives its place back.
    wait_awake_of(&h, &first).await;
    h.manager.sleep(&first).await.expect("the first sleeps");
    h.manager
        .wake(&third)
        .await
        .expect("the place the first gave back is free");
}

/// The server's own cap holds across tenants: one tenant under
/// its cap still waits when the machine is full.
#[tokio::test]
async fn the_server_cap_holds_across_two_tenants() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let ceiling = Arc::new(AwakeCeiling::new(AwakeCaps {
        per_tenant: 4,
        per_server: 2,
    }));
    let one = tenant_manager(&runtime, &ceiling);
    let two = tenant_manager(&runtime, &ceiling);
    let agent = AgentId::generate();
    let other = AgentId::generate();

    one.wake(&agent).await.expect("the first tenant wakes one");
    one.wake(&other).await.expect("the first tenant wakes two");

    let refused = two
        .wake(&AgentId::generate())
        .await
        .expect_err("the server is full");
    assert!(
        matches!(
            refused,
            ComputerError::AwakeCapReached {
                scope: "this server",
                cap: 2
            }
        ),
        "{refused}"
    );
    assert_eq!(ceiling.awake_on_server(), 2);
}

/// The Plugin Computer is not a sprite's desk (ADR-0017): it takes
/// no place under the per-tenant cap, so a tenant whose plugin host runs
/// still wakes every desk the cap allows. It does take a place under the
/// per-server cap, because it is a container the machine holds.
#[tokio::test]
async fn the_plugin_computer_counts_on_the_server_and_not_on_the_tenant() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let ceiling = Arc::new(AwakeCeiling::new(AwakeCaps {
        per_tenant: 2,
        per_server: 3,
    }));
    let manager = tenant_manager(&runtime, &ceiling);

    manager
        .wake(&pagis_computer::plugin_agent())
        .await
        .expect("the plugin host wakes");
    // It is not in the tenant's count, and it is in the server's.
    assert_eq!(ceiling.awake(manager.workspace_id()), 0);
    assert_eq!(ceiling.awake_on_server(), 1);

    // Both desks the per-tenant cap allows still wake.
    manager
        .wake(&AgentId::generate())
        .await
        .expect("the first desk wakes");
    manager
        .wake(&AgentId::generate())
        .await
        .expect("the second desk wakes");
    assert_eq!(ceiling.awake(manager.workspace_id()), 2);
    assert_eq!(ceiling.awake_on_server(), 3);

    // The machine is full now, and another tenant hears the server's cap.
    let other = tenant_manager(&runtime, &ceiling);
    let refused = other
        .wake(&AgentId::generate())
        .await
        .expect_err("the server is full");
    assert!(
        matches!(
            refused,
            ComputerError::AwakeCapReached {
                scope: "this server",
                cap: 3
            }
        ),
        "{refused}"
    );

    // And a plugin host of a full machine hears the same thing, because
    // it is a container like any other.
    let refused = other
        .wake(&pagis_computer::plugin_agent())
        .await
        .expect_err("the server is full for a plugin host too");
    assert!(
        matches!(
            refused,
            ComputerError::AwakeCapReached {
                scope: "this server",
                cap: 3
            }
        ),
        "{refused}"
    );
}

/// The resource figures count the Docker objects of the tenant that
/// asks and no other tenant's.
#[tokio::test]
async fn the_resource_figures_count_one_tenants_objects() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.set_volume_bytes(100);
    let ceiling = Arc::new(AwakeCeiling::new(AwakeCaps::default()));
    let one = tenant_manager(&runtime, &ceiling);
    let two = tenant_manager(&runtime, &ceiling);

    let first = AgentId::generate();
    let second = AgentId::generate();
    one.wake(&first).await.expect("the first tenant wakes one");
    one.wake(&second).await.expect("the first tenant wakes two");
    two.wake(&AgentId::generate())
        .await
        .expect("the second tenant wakes one");
    wait_awake_of_manager(&one, &first).await;
    wait_awake_of_manager(&two, &second).await;

    let first_tenant = one.resources().await.expect("the first tenant's resources");
    assert_eq!(first_tenant.volume_bytes, 200);
    assert_eq!(first_tenant.volumes, 2);
    assert_eq!(first_tenant.containers, 2);
    let second_tenant = two
        .resources()
        .await
        .expect("the second tenant's resources");
    assert_eq!(second_tenant.volume_bytes, 100);
    assert_eq!(second_tenant.volumes, 1);
    assert_eq!(second_tenant.containers, 1);
}

/// Every container and every volume carries its owner, so one
/// tenant's Docker objects are never another's.
#[tokio::test]
async fn a_container_and_a_volume_carry_the_tenant_and_the_agent() {
    let h = harness_with(FakeComputerRuntime::with_image(), Duration::from_secs(600));
    h.manager.wake(&h.agent_id).await.expect("wake");
    wait_awake(&h).await;

    let owners = h.runtime.started_owners();
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].workspace_id, h.workspace_id);
    assert_eq!(owners[0].agent_id, h.agent_id);
    assert!(owners[0].container_name().contains(h.workspace_id.as_str()));
    assert!(owners[0].volume_name().contains(h.workspace_id.as_str()));
}

/// One manager per tenant over one runtime and one ceiling.
fn tenant_manager(
    runtime: &Arc<FakeComputerRuntime>,
    ceiling: &Arc<AwakeCeiling>,
) -> Arc<ComputerManager> {
    let workspace_id = WorkspaceId::generate();
    ComputerManager::new(ComputerManagerDeps {
        runtime: Arc::clone(runtime) as _,
        image: ComputerImage::new(Arc::clone(runtime) as _),
        skills: Arc::new(FakeSkills::default()),
        workspaces: Arc::new(FakeWorkspaces::with_timezone(&workspace_id, TEST_TIMEZONE)),
        agents: Arc::new(pagis_computer::fake::FakeAgents::open()),
        bus: Arc::new(RecordingBus::default()),
        workspace_id,
        screens_dir: std::env::temp_dir().join(format!("pagis-screens-{}", AgentId::generate())),
        idle_stop: Duration::from_secs(600),
        relay: pagis_computer::fake::loopback_relay(),
        ceiling: Arc::clone(ceiling),
        exit_daemon: None,
    })
}

async fn wait_awake_of(h: &Harness, agent_id: &AgentId) {
    wait_awake_of_manager(&h.manager, agent_id).await
}

async fn wait_awake_of_manager(manager: &Arc<ComputerManager>, agent_id: &AgentId) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while manager.state(agent_id).await != ComputerState::Awake {
        assert!(tokio::time::Instant::now() < deadline, "never woke");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The Computer managers of one daemon over `runtime`. A second call
/// over the same runtime is the daemon after a restart: it knows
/// nothing of the Computers that still run.
fn daemon_managers(
    runtime: &Arc<FakeComputerRuntime>,
    idle_stop: Duration,
    caps: AwakeCaps,
) -> Arc<pagis_computer::ComputerManagers> {
    pagis_computer::ComputerManagers::new(pagis_computer::ComputerManagersDeps {
        runtime: Arc::clone(runtime) as _,
        skills: Arc::new(FakeSkills::default()) as _,
        workspaces: Arc::new(FakeWorkspaces::with_timezone(
            &WorkspaceId::generate(),
            TEST_TIMEZONE,
        )) as _,
        agents: Arc::new(pagis_computer::fake::FakeAgents::open()) as _,
        bus: Arc::new(RecordingBus::default()) as _,
        screens_dir: std::env::temp_dir().join(format!("pagis-screens-{}", AgentId::generate())),
        idle_stop,
        relay: pagis_computer::fake::loopback_relay(),
        caps,
        cancel: CancellationToken::new(),
        exit_daemon: None,
    })
}

/// Wake one Computer of `tenant` under a daemon that then restarts:
/// the first daemon exits for a restart and stops nothing.
async fn awake_before_a_restart(
    runtime: &Arc<FakeComputerRuntime>,
    tenant: &WorkspaceId,
) -> AgentId {
    let before = daemon_managers(runtime, Duration::from_secs(600), AwakeCaps::default());
    let agent_id = AgentId::generate();
    let manager = before.get(tenant);
    manager.wake(&agent_id).await.expect("wake");
    wait_awake_of_manager(&manager, &agent_id).await;
    agent_id
}

/// After a restart, a daemon adopts every running Computer of its
/// tenants when it starts. Nobody asks for the state of the Agent, and
/// the stop for good still stops its Computer.
#[tokio::test]
async fn a_restarted_daemon_stops_a_computer_that_nobody_asked_for() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let tenant = WorkspaceId::generate();
    let agent_id = awake_before_a_restart(&runtime, &tenant).await;

    let after = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());
    after.adopt_all(std::slice::from_ref(&tenant)).await;
    after.stop_all().await;

    assert!(!runtime.is_running(&agent_id), "the Computer still runs");
    assert!(
        runtime.has_volume(&agent_id),
        "the Computer lost its volume"
    );
    assert_eq!(after.ceiling().awake(&tenant), 0);
}

/// The idle sweep of the restarted daemon stops an adopted Computer
/// that nobody uses.
#[tokio::test]
async fn the_idle_sweep_of_a_restarted_daemon_stops_a_computer_that_nobody_asked_for() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let tenant = WorkspaceId::generate();
    let agent_id = awake_before_a_restart(&runtime, &tenant).await;

    let after = daemon_managers(&runtime, Duration::from_millis(20), AwakeCaps::default());
    after.adopt_all(std::slice::from_ref(&tenant)).await;
    assert!(runtime.is_running(&agent_id), "the adoption stopped it");
    tokio::time::sleep(Duration::from_millis(30)).await;
    after.get(&tenant).sweep().await;

    assert!(!runtime.is_running(&agent_id), "the Computer still runs");
    assert_eq!(after.ceiling().awake(&tenant), 0);
}

/// A Computer that the restarted daemon adopts takes its place under
/// the awake cap, so the cap counts what runs on the machine.
#[tokio::test]
async fn an_adopted_computer_counts_under_the_awake_cap() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let tenant = WorkspaceId::generate();
    awake_before_a_restart(&runtime, &tenant).await;

    let after = daemon_managers(
        &runtime,
        Duration::from_secs(600),
        AwakeCaps {
            per_tenant: 1,
            per_server: 10,
        },
    );
    after.adopt_all(std::slice::from_ref(&tenant)).await;

    assert_eq!(after.ceiling().awake(&tenant), 1);
    let refused = after
        .get(&tenant)
        .wake(&AgentId::generate())
        .await
        .expect_err("the adopted Computer holds the one place");
    assert!(
        matches!(refused, ComputerError::AwakeCapReached { cap: 1, .. }),
        "{refused}"
    );
}

/// A daemon adopts the Computers of its own tenants and no other: a
/// Workspace that is not in the list is another installation's, and its
/// Computer runs on after this daemon stops.
#[tokio::test]
async fn a_daemon_does_not_adopt_the_computer_of_a_workspace_it_does_not_serve() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let own = WorkspaceId::generate();
    let foreign = WorkspaceId::generate();
    let own_agent = awake_before_a_restart(&runtime, &own).await;
    let foreign_agent = awake_before_a_restart(&runtime, &foreign).await;

    let after = daemon_managers(&runtime, Duration::from_secs(600), AwakeCaps::default());
    after.adopt_all(std::slice::from_ref(&own)).await;
    after.stop_all().await;

    assert!(!runtime.is_running(&own_agent));
    assert!(
        runtime.is_running(&foreign_agent),
        "a foreign Computer stopped"
    );
    assert_eq!(after.ceiling().awake_on_server(), 0);
}
