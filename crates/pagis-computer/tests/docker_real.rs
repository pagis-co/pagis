//! Docker-real lifecycle tests: `#[ignore]`-tagged; the
//! gate runs them only where Docker is reachable
//! (`cargo nextest run --workspace --run-ignored only`) in the pinned
//! image the gate's `computer-image` step builds from `computer/`.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use pagis_computer::fake::FakeWorkspaces;
use pagis_computer::{
    AwakeCaps, AwakeCeiling, BollardRuntime, ComputerLimits, ComputerManager, ComputerManagerDeps,
    ComputerOwner, ComputerRuntime, ComputerState, DockerDiscovery, ExecRequest, ExitMode, IMAGE,
    InputHolder, OutputCap, RuntimeOptions, SHELL_HOME, ShellCommand,
    test_docker::{TestDocker, marked_objects},
};
use pagis_core::{AgentId, WorkspaceId};

/// The gate's `computer-image` step builds the pinned image from
/// `computer/` before these tests run; a test builds nothing, so the
/// build happens once and not once per test process.
fn require_image() {
    let present = Command::new("docker")
        .args(["image", "inspect", IMAGE])
        .output()
        .expect("docker image inspect runs")
        .status
        .success();
    assert!(
        present,
        "image {IMAGE} is not built; run `docker build -t {IMAGE} computer` at the workspace root"
    );
}

/// One test's real runtime, in a Workspace of its own, so no two tests
/// share a Tenant Network. Dropping it removes every container, volume
/// and Tenant Network the runtime created, also when the test fails.
pub(crate) struct Real {
    pub(crate) runtime: Arc<BollardRuntime>,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) docker: TestDocker,
}

impl Real {
    /// The real runtime, with the default limits and the screend
    /// tokens under the test target directory. A colima bind mount from
    /// the system temporary directory arrives empty, so the token file
    /// has to live here.
    pub(crate) fn new() -> Self {
        Self::with_limits(ComputerLimits::default())
    }

    /// The same, with the limits of one test.
    pub(crate) fn with_limits(limits: ComputerLimits) -> Self {
        require_image();
        let docker = TestDocker::new();
        let runtime = Arc::new(BollardRuntime::new(
            Arc::new(DockerDiscovery::production(None)),
            RuntimeOptions {
                limits,
                tokens_dir: std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
                    .join("screend-tokens"),
                labels: docker.labels(),
            },
        ));
        Self {
            runtime,
            workspace_id: WorkspaceId::generate(),
            docker,
        }
    }

    pub(crate) fn owner(&self, agent_id: &AgentId) -> ComputerOwner {
        ComputerOwner::new(self.workspace_id.clone(), agent_id.clone())
    }

    /// A manager over the real runtime. The returned tempdir holds the
    /// stored screenshots and must outlive the manager.
    pub(crate) fn manager(
        &self,
        idle_after: Duration,
    ) -> (Arc<ComputerManager>, tempfile::TempDir) {
        self.manager_with(idle_after, Arc::new(pagis_core::NoSkills), "UTC")
    }

    /// The same, over a scripted Skills catalogue and one Workspace
    /// timezone.
    fn manager_with(
        &self,
        idle_after: Duration,
        skills: Arc<dyn pagis_core::Skills>,
        timezone: &str,
    ) -> (Arc<ComputerManager>, tempfile::TempDir) {
        let screens = tempfile::tempdir().expect("screens dir");
        let manager = ComputerManager::new(ComputerManagerDeps {
            runtime: Arc::clone(&self.runtime) as _,
            image: pagis_computer::ComputerImage::new(Arc::clone(&self.runtime) as _),
            skills,
            workspaces: Arc::new(FakeWorkspaces::with_timezone(&self.workspace_id, timezone)),
            agents: Arc::new(pagis_computer::fake::FakeAgents::open()),
            bus: Arc::new(pagis_audit::AuditEventBus::new(Arc::new(NoopLog))),
            workspace_id: self.workspace_id.clone(),
            screens_dir: screens.path().to_path_buf(),
            idle_stop: idle_after,
            relay: pagis_computer::fake::loopback_relay(),
            ceiling: Arc::new(AwakeCeiling::new(AwakeCaps::default())),
        });
        (manager, screens)
    }
}

fn docker_exec(owner: &ComputerOwner, cmd: &[&str]) -> String {
    let output = docker_exec_raw(owner, &[], cmd);
    assert!(
        output.status.success(),
        "docker exec failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The same, allowing a failure and extra `docker exec` flags — the
/// uid checks need `--user` and expect non-zero exits.
pub(crate) fn docker_exec_raw(
    owner: &ComputerOwner,
    flags: &[&str],
    cmd: &[&str],
) -> std::process::Output {
    Command::new("docker")
        .arg("exec")
        .args(flags)
        .arg(owner.container_name())
        .args(cmd)
        .output()
        .expect("docker exec runs")
}

async fn wait_awake(manager: &Arc<ComputerManager>, agent_id: &AgentId) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while manager.state(agent_id).await != ComputerState::Awake {
        assert!(tokio::time::Instant::now() < deadline, "never woke");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The lifecycle of one Computer, and the parts of it that the first
/// wake starts. It holds these contracts:
///
/// - a wake boots the pinned image, and the woken Computer already
///   shows the browser: the Agent's first screenshot comes right after
///   the Computer turns awake, and a screen with a terminal and no
///   browser sends the Agent to start one itself;
/// - screend serves a PNG preview and answers a live view offer;
/// - the input switch flips, and screend refuses a batch of a holder
///   that does not hold the switch, and every batch of the daemon;
/// - the uid split (ADR-0013) keeps the agent's shell out of the
///   Wayland socket and the browser profile;
/// - a file in the agent's home survives idle-stop and re-wake.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn wake_preview_idle_stop_and_rewake_preserve_home() {
    let real = Real::new();
    let runtime = &real.runtime;
    // Sweep-controlled: every sweep idles.
    let (manager, _screens) = real.manager(Duration::from_millis(10));
    let agent_id = AgentId::generate();
    let owner = real.owner(&agent_id);

    // Wake boots the pinned image and the browser window is open when
    // the Computer turns awake.
    manager.wake(&agent_id).await.expect("wake");
    wait_awake(&manager, &agent_id).await;
    let running = runtime
        .running(&owner)
        .await
        .expect("running query")
        .expect("computer is running");
    let open = windows(&running.computer.control_addr, &running.computer.token).await;
    assert!(
        open.as_array()
            .expect("the window list is an array")
            .iter()
            .any(|window| window["app_id"] == "chromium"),
        "the awake computer shows no browser window: {open}"
    );

    // screend answers with a real PNG.
    let live = manager.preview(&agent_id).await.expect("live preview");
    assert!(live.live);
    assert_eq!(&live.png[..8], b"\x89PNG\r\n\x1a\n", "screend serves PNG");

    // The live view path: a real browser-shaped offer through
    // the daemon relay comes back as an ice-lite answer carrying the
    // advertised host candidate and the ICE credentials the path takes;
    // a second offer replaces the path of the first.
    for _ in 0..2 {
        let answer = manager
            .offer(
                &agent_id,
                &browser_offer(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .expect("offer answered");
        assert!(answer.contains("a=ice-lite"), "{answer}");
        assert!(answer.contains("a=candidate"), "{answer}");
        assert!(answer.contains("127.0.0.1"), "{answer}");
        assert!(answer.to_lowercase().contains("h264"), "{answer}");
        // The data channel negotiates alongside the video.
        assert!(answer.contains("webrtc-datachannel"), "{answer}");
    }

    // The input switch: screend's holder endpoint flips and
    // reports the user-input idle clock.
    assert_eq!(
        running.version.as_deref(),
        Some(pagis_computer::IMAGE_VERSION),
        "the running container reports its image version"
    );
    let computer = running.computer;
    runtime
        .set_holder(&computer, InputHolder::User)
        .await
        .expect("holder set to user");
    let idle = runtime
        .user_input_idle_ms(&computer)
        .await
        .expect("holder status");
    assert!(idle < 60_000, "idle clock restarted on takeover: {idle}");
    runtime
        .set_holder(&computer, InputHolder::Agent)
        .await
        .expect("holder set back to agent");

    // The uid split (ADR-0013). The compositor session runs as
    // `screen`; the agent's shell runs as `agent` and cannot reach the
    // Wayland socket, so it cannot inject input behind screend or read
    // the browser profile.
    let session_users = docker_exec(
        &owner,
        &[
            "sh",
            "-c",
            "ps -o user= -C chromium -C pagis-screend | sort -u",
        ],
    );
    assert_eq!(
        session_users.split_whitespace().collect::<Vec<_>>(),
        vec!["screen"],
        "Chromium and screend run as screen"
    );
    let reach = docker_exec_raw(
        &owner,
        &["--user", "agent"],
        &["sh", "-c", "ls /run/pagis-xdg"],
    );
    assert!(
        !reach.status.success(),
        "the agent's shell reached the Wayland runtime directory"
    );
    let profile = docker_exec_raw(
        &owner,
        &["--user", "agent"],
        &["sh", "-c", "ls /data/screen"],
    );
    assert!(
        !profile.status.success(),
        "the agent's shell read the browser profile"
    );
    // The image has no `wtype`: screend types through its own virtual
    // keyboard.
    let wtype = docker_exec_raw(&owner, &[], &["sh", "-c", "command -v wtype"]);
    assert!(!wtype.status.success(), "wtype is still in the image");
    // Chromium's password manager and save bubble are off.
    let policy = docker_exec(
        &owner,
        &["cat", "/etc/chromium/policies/managed/pagis.json"],
    );
    assert!(
        policy.contains("\"PasswordManagerEnabled\": false"),
        "{policy}"
    );
    // HTTP/3 is off: it runs over UDP, which the Exit Proxy does not
    // carry (ADR-0029).
    assert!(policy.contains("\"QuicAllowed\": false"), "{policy}");

    // Typing runs through screend's virtual keyboard, and a batch is
    // refused unless its declared holder holds the switch.
    runtime
        .send_input(
            &computer,
            InputHolder::Agent,
            &[pagis_computer::exec::InputOp::Text {
                text: "pagis".to_string(),
            }],
        )
        .await
        .expect("the agent may type while it holds the switch");
    runtime
        .set_holder(&computer, InputHolder::Daemon)
        .await
        .expect("holder set to daemon");
    let refused = runtime
        .send_input(
            &computer,
            InputHolder::Agent,
            &[pagis_computer::exec::InputOp::Text {
                text: "pagis".to_string(),
            }],
        )
        .await;
    assert!(
        refused.is_err(),
        "screend accepted agent input while the daemon held the switch"
    );
    // The daemon sends no keystrokes: a Vault fill writes through the
    // browser channel, so screend refuses a batch of the daemon.
    let daemon_typed = runtime
        .send_input(
            &computer,
            InputHolder::Daemon,
            &[pagis_computer::exec::InputOp::Text {
                text: "secret".to_string(),
            }],
        )
        .await;
    assert!(
        daemon_typed.is_err(),
        "screend accepted keystrokes of the daemon"
    );
    runtime
        .set_holder(&computer, InputHolder::Agent)
        .await
        .expect("holder set back to agent");

    // A file in the agent's home survives idle-stop and re-wake.
    docker_exec(
        &owner,
        &["sh", "-c", "echo keepsake > /data/agent/keep.txt"],
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    manager.sweep().await;
    assert_eq!(manager.state(&agent_id).await, ComputerState::Off);
    let asleep = manager.preview(&agent_id).await.expect("stored preview");
    assert!(!asleep.live);

    manager.wake(&agent_id).await.expect("re-wake");
    wait_awake(&manager, &agent_id).await;
    let kept = docker_exec(&owner, &["cat", "/data/agent/keep.txt"]);
    assert_eq!(kept.trim(), "keepsake");

    runtime.stop(&owner).await.expect("stop");
}

/// A real recvonly-video SDP offer, generated by str0m standing in for
/// the browser.
fn browser_offer() -> String {
    let mut rtc = str0m::Rtc::builder().build();
    let mut api = rtc.sdp_api();
    api.add_media(
        str0m::media::MediaKind::Video,
        str0m::media::Direction::RecvOnly,
        None,
        None,
        None,
    );
    // The input data channel rides every viewer session.
    api.add_channel("input".to_string());
    let (offer, _pending) = api.apply().expect("offer builds");
    offer.to_sdp_string()
}

/// An event log that drops everything: these tests assert against
/// Docker, not the bus.
struct NoopLog;

#[async_trait::async_trait]
impl pagis_core::EventLog for NoopLog {
    async fn append(
        &self,
        event: pagis_core::NewEvent,
    ) -> Result<pagis_core::Event, pagis_core::StoreError> {
        Ok(pagis_core::Event {
            id: pagis_core::EventId::generate(),
            seq: 0,
            workspace_id: event.workspace_id,
            event_type: event.event_type,
            agent_id: event.agent_id,
            run_id: event.run_id,
            channel_id: event.channel_id,
            payload: event.payload,
            created_at: pagis_core::now_ms(),
        })
    }

    async fn list_after(
        &self,
        _workspace_id: Option<&pagis_core::WorkspaceId>,
        _after_seq: i64,
        _limit: u32,
    ) -> Result<Vec<pagis_core::Event>, pagis_core::StoreError> {
        Ok(Vec::new())
    }

    async fn latest_seq(&self) -> Result<i64, pagis_core::StoreError> {
        Ok(0)
    }

    async fn list_for_run(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        _run_id: &pagis_core::RunId,
    ) -> Result<Vec<pagis_core::Event>, pagis_core::StoreError> {
        Ok(Vec::new())
    }

    async fn list_by_types(
        &self,
        _workspace_id: &pagis_core::WorkspaceId,
        _event_types: &[&str],
        _before: Option<&pagis_core::EventId>,
        _limit: u32,
    ) -> Result<Vec<pagis_core::Event>, pagis_core::StoreError> {
        Ok(Vec::new())
    }
}

/// screend's health report: `{ok, browser}`.
async fn healthz(control_addr: &str) -> serde_json::Value {
    let body = reqwest::get(format!("http://{control_addr}/healthz"))
        .await
        .expect("healthz reachable")
        .text()
        .await
        .expect("healthz body");
    serde_json::from_str(&body).unwrap_or_else(|err| panic!("healthz is not JSON: {err}: {body}"))
}

/// The oldest live Chromium pid, or an empty string when the browser
/// is not running.
fn browser_pid(owner: &ComputerOwner) -> String {
    let found = docker_exec_raw(owner, &[], &["pgrep", "-o", "chromium"]);
    String::from_utf8_lossy(&found.stdout).trim().to_string()
}

/// The user namespace one process runs in. The read happens as
/// `screen`, the uid the browser runs under: a reader of another uid's
/// namespace link needs CAP_SYS_PTRACE, which Docker's default
/// capability set leaves out.
fn user_namespace(owner: &ComputerOwner, pid: &str) -> String {
    let found = docker_exec_raw(
        owner,
        &["-u", "screen"],
        &["readlink", &format!("/proc/{pid}/ns/user")],
    );
    assert!(
        found.status.success(),
        "process {pid} has no readable user namespace: {}",
        String::from_utf8_lossy(&found.stderr)
    );
    String::from_utf8_lossy(&found.stdout).trim().to_string()
}

/// Wait for a browser that is up and stays up, and answer with its
/// pid. The settle window is what separates a running browser from a
/// crash loop, which also puts a Chromium on `/proc` — briefly, over
/// and over. `excluding` skips a pid the caller has already seen, so
/// a respawn is observed as a new process rather than as an up/down
/// edge too short to catch.
async fn wait_browser_settled(
    control_addr: &str,
    owner: &ComputerOwner,
    excluding: &str,
) -> String {
    let settle = Duration::from_secs(5);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let pid = browser_pid(owner);
        if !pid.is_empty() && pid != excluding {
            tokio::time::sleep(settle).await;
            if browser_pid(owner) == pid {
                assert_eq!(
                    healthz(control_addr).await["browser"],
                    "up",
                    "a browser is running but screend reports it down"
                );
                return pid;
            }
            continue;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no browser stayed up: screend reports {}",
            healthz(control_addr).await["browser"]
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// A daemon that starts adopts the running Computers of its tenant,
/// found by the owner labels. After a restart nobody asks for the
/// Agent, and the idle sweep and the stop for good still stop its
/// Computer. The container of another Workspace is not in the list.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_restarted_manager_stops_a_computer_that_nobody_asked_for() {
    let real = Real::new();
    let agent_id = AgentId::generate();
    let owner = real.owner(&agent_id);
    let (before, _before_screens) = real.manager(Duration::from_secs(600));
    let is_running = || async {
        real.runtime
            .running(&owner)
            .await
            .expect("running query")
            .is_some()
    };

    // The idle sweep of the restarted manager.
    before.wake(&agent_id).await.expect("wake");
    wait_awake(&before, &agent_id).await;
    assert_eq!(
        real.runtime
            .running_agents(&real.workspace_id)
            .await
            .expect("running agents"),
        vec![agent_id.clone()]
    );
    assert!(
        real.runtime
            .running_agents(&WorkspaceId::generate())
            .await
            .expect("running agents of another workspace")
            .is_empty(),
        "another Workspace lists this Computer"
    );
    let (after, _after_screens) = real.manager(Duration::from_millis(10));
    after.adopt_all().await;
    assert!(is_running().await, "the adoption stopped the Computer");
    tokio::time::sleep(Duration::from_millis(20)).await;
    after.sweep().await;
    assert!(
        !is_running().await,
        "the idle sweep left the Computer running"
    );

    // The stop for good of the restarted manager. The first manager
    // still takes the Computer for awake, so a new one wakes it.
    let (again, _again_screens) = real.manager(Duration::from_secs(600));
    again.wake(&agent_id).await.expect("wake again");
    wait_awake(&again, &agent_id).await;
    assert!(
        is_running().await,
        "the second wake did not start the Computer"
    );
    let (after, _after_screens) = real.manager(Duration::from_secs(600));
    after.adopt_all().await;
    after.stop_all().await;
    assert!(
        !is_running().await,
        "the stop for good left the Computer running"
    );
}

/// The exec seam against a real daemon, on one Computer. It holds
/// these contracts:
///
/// - a shell command runs as the agent, with its home as `HOME` and as
///   the directory it starts in;
/// - stdin reaches the command and ends with a half-close, so `cat`
///   ends;
/// - long output keeps a head, a tail and a marker between them;
/// - a command past its deadline ends with code 124, and the deadline
///   takes the whole process group with it.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_shell_command_runs_as_the_agent_within_its_deadline_and_output_cap() {
    let real = Real::new();
    let runtime = &real.runtime;
    let (manager, _screens) = real.manager(Duration::from_secs(600));
    let agent_id = AgentId::generate();
    let owner = real.owner(&agent_id);
    manager.wake(&agent_id).await.expect("wake");
    wait_awake(&manager, &agent_id).await;
    let shell = |command: &str, timeout: Duration| {
        manager.shell(
            &agent_id,
            ShellCommand {
                command: command.to_string(),
                timeout,
                cwd: None,
                stdin: None,
                output_cap: None,
            },
        )
    };

    // The uid, the environment and the directory.
    let outcome = shell(
        "id -un; printf '%s %s\\n' \"$HOME\" \"$PWD\"",
        Duration::from_secs(30),
    )
    .await
    .expect("shell");
    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.stdout, "agent\n/data/agent /data/agent\n");
    assert!(!outcome.truncated);

    // Stdin with its half-close.
    let computer = runtime
        .running(&owner)
        .await
        .expect("running query")
        .expect("computer is running")
        .computer;
    let outcome = runtime
        .exec(
            &computer,
            ExecRequest {
                argv: vec!["cat".to_string()],
                user: "agent".to_string(),
                cwd: SHELL_HOME.to_string(),
                env: vec![format!("HOME={SHELL_HOME}")],
                stdin: Some(b"from stdin\n".to_vec()),
                output_cap: OutputCap {
                    head: 4096,
                    tail: 1024,
                },
            },
        )
        .await
        .expect("exec");
    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.stdout, "from stdin\n");

    // The output cap.
    let outcome = shell("seq 1 400000", Duration::from_secs(60))
        .await
        .expect("shell");
    assert_eq!(outcome.exit_code, 0);
    assert!(outcome.truncated);
    assert!(outcome.stdout.starts_with("1\n2\n3\n"));
    assert!(outcome.stdout.contains("bytes truncated"));
    assert!(outcome.stdout.ends_with("400000\n"), "the tail survived");

    // The in-container deadline.
    let outcome = shell(
        "(sleep 30; touch /data/agent/leaked) & wait",
        Duration::from_secs(2),
    )
    .await
    .expect("shell");
    assert_eq!(outcome.exit_code, 124);
    // The deadline takes the whole process group with it.
    let listing = docker_exec(&owner, &["ls", "/data/agent"]);
    assert!(!listing.contains("leaked"), "a child outlived the deadline");
}

/// A catalogue of exactly one mount.
struct OneMount(pagis_core::SkillMount);

#[async_trait::async_trait]
impl pagis_core::Skills for OneMount {
    async fn list(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
    ) -> Vec<pagis_core::Skill> {
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

    async fn mounts(
        &self,
        _workspace_id: &WorkspaceId,
        _agent_id: &AgentId,
    ) -> Vec<pagis_core::SkillMount> {
        vec![self.0.clone()]
    }
}

/// A granted plugin's Skills are in the container, read-only, beside
/// the first-party ones the image ships (ADR-0017).
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_granted_plugin_mounts_its_skills_read_only() {
    let real = Real::new();
    // Under the target directory, not the system temp dir: Docker on
    // this host only shares the user's home into the VM, so a bind
    // mount from /var/folders arrives empty.
    let plugin = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("plugin checkout");
    let skill = plugin.path().join("forecast");
    std::fs::create_dir_all(&skill).expect("skill directory");
    std::fs::write(skill.join("SKILL.md"), "# Forecast\n").expect("skill file");
    let (manager, _screens) = real.manager_with(
        Duration::from_secs(600),
        Arc::new(OneMount(pagis_core::SkillMount {
            plugin: "weather".to_string(),
            skills_dir: plugin.path().to_path_buf(),
        })),
        "UTC",
    );
    let agent_id = AgentId::generate();
    let owner = real.owner(&agent_id);

    manager.wake(&agent_id).await.expect("wake");
    wait_awake(&manager, &agent_id).await;

    let body = docker_exec(
        &owner,
        &["cat", "/opt/plugins/weather/skills/forecast/SKILL.md"],
    );
    assert!(body.contains("# Forecast"), "{body}");
    // The mount is read-only, so the agent cannot rewrite a skill.
    let write = docker_exec_raw(
        &owner,
        &[],
        &[
            "sh",
            "-c",
            "echo x > /opt/plugins/weather/skills/forecast/SKILL.md",
        ],
    );
    assert!(!write.status.success(), "the mount accepted a write");
    // The image ships the first-party set beside it.
    let listed = docker_exec(&owner, &["ls", "/opt/pagis/skills"]);
    assert!(listed.contains("system-packages"), "{listed}");
}

/// A browser that dies comes back on its own, and the restarted browser
/// keeps each launch contract of the first one. It holds these
/// contracts:
///
/// - the supervisor restarts a dead Chromium. Without it, one Chromium
///   exit leaves a Computer with no browser, and the only symptom is an
///   agent that stares at a terminal;
/// - Chromium runs with its own sandbox on. The browser runs without
///   `--no-sandbox`, so it depends on the seccomp profile the runtime
///   gives the container: without the user-namespace calls the sandbox
///   cannot start, Chromium exits at once, and the supervisor turns the
///   failure into a restart loop that `wait_browser_settled` refuses.
///   Without the flag, no page shows the "unsupported command-line
///   flag" bar to the agent;
/// - Chromium starts with the pinned uBlock Origin Lite, with no other
///   extension beside it. Each ad frame, consent script and tracker on
///   a page costs the agent a screenshot and the tokens to read it;
/// - Chromium sends its connections to the Exit Proxy, and the proxy
///   listens on loopback alone (ADR-0029);
/// - the agent cannot open the browser channel of the daemon. That
///   channel is the DevTools pipe that screend and Chromium alone hold
///   (ADR-0013). The browser opens no debugging port, and the Agent's
///   uid can open no file descriptor of a process of the `screen` uid.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn the_supervisor_restarts_a_dead_chromium() {
    let real = Real::new();
    let runtime = &real.runtime;
    let (manager, _screens) = real.manager(Duration::from_secs(600));
    let agent_id = AgentId::generate();
    let owner = real.owner(&agent_id);

    manager.wake(&agent_id).await.expect("wake");
    wait_awake(&manager, &agent_id).await;
    let running = runtime
        .running(&owner)
        .await
        .expect("running query")
        .expect("computer is running");
    let control_addr = running.computer.control_addr;
    let first = wait_browser_settled(&control_addr, &owner, "").await;

    docker_exec_raw(&owner, &[], &["pkill", "-KILL", "chromium"]);

    let pid = wait_browser_settled(&control_addr, &owner, &first).await;
    assert_ne!(first, pid, "the supervisor never restarted Chromium");

    // `/proc/<pid>/cmdline` is NUL-separated: `tr` puts one argument
    // on each line.
    let cmdline = docker_exec(
        &owner,
        &["sh", "-c", &format!("tr '\\0' '\\n' < /proc/{pid}/cmdline")],
    );
    let args: Vec<&str> = cmdline.lines().collect();

    // The sandbox.
    assert!(
        !args.contains(&"--no-sandbox"),
        "the browser still runs unsandboxed: {args:?}"
    );
    // A live browser alone does not prove the sandbox started, so read
    // the namespace itself: Chromium puts the sandboxed zygote in a
    // user namespace of its own, and that is the call Docker's default
    // profile refuses. Chromium also starts one unsandboxed zygote
    // (`--no-zygote-sandbox`), which stays in the browser's namespace.
    let browser_userns = user_namespace(&owner, &pid);
    let zygotes = docker_exec(&owner, &["pgrep", "-f", "type=zygote"]);
    let sandboxed: Vec<&str> = zygotes
        .split_whitespace()
        .filter(|zygote| user_namespace(&owner, zygote) != browser_userns)
        .collect();
    assert!(
        !sandboxed.is_empty(),
        "every Chromium zygote stayed in the browser's user namespace {browser_userns}: \
         the zygotes are {zygotes:?}"
    );

    // The blocker. The unpacked release is in the image, at the path
    // the flags name.
    let manifest = docker_exec(
        &owner,
        &["cat", "/opt/pagis/ublock-origin-lite/manifest.json"],
    );
    assert!(
        manifest.contains("\"manifest_version\": 3"),
        "the extension in the image is not the Manifest V3 build: {manifest}"
    );
    assert!(
        args.contains(&"--load-extension=/opt/pagis/ublock-origin-lite"),
        "Chromium started without the blocker: {args:?}"
    );
    assert!(
        args.contains(&"--disable-extensions-except=/opt/pagis/ublock-origin-lite"),
        "Chromium accepts an extension beside the blocker: {args:?}"
    );
    // The flag is a request, not a result: Chromium refuses an
    // extension whose static filter rules it cannot index, and says so
    // only in its own log. It writes each indexed ruleset into
    // `_metadata/` in the extension directory, so that directory is
    // the proof that the browser took the blocker.
    let indexed = docker_exec(
        &owner,
        &[
            "ls",
            "/opt/pagis/ublock-origin-lite/_metadata/generated_indexed_rulesets",
        ],
    );
    assert!(
        indexed.contains("_ruleset"),
        "Chromium did not index the blocker's filter rules: {indexed:?}"
    );

    // Every connection of the browser goes to the Exit Proxy
    // (ADR-0029).
    assert!(
        args.contains(&"--proxy-server=http://127.0.0.1:3128"),
        "the browser does not send its connections to the Exit Proxy: {args:?}"
    );

    // The browser channel of the daemon.
    assert!(
        args.contains(&"--remote-debugging-pipe"),
        "the browser has no DevTools pipe: {args:?}"
    );
    assert!(
        !args
            .iter()
            .any(|arg| arg.starts_with("--remote-debugging-port")),
        "the browser opens a debugging port: {args:?}"
    );
    // The only TCP listeners are screend's control port and its Exit
    // Proxy, which listens on loopback alone, so no other container
    // reaches it. Docker's own resolver listens on 127.0.0.11 in every
    // container on a user network, and it is not the browser's.
    let listeners = docker_exec_raw(&owner, &["--user", "agent"], &["python3", "-c", LISTENERS]);
    assert!(
        listeners.status.success(),
        "{}",
        String::from_utf8_lossy(&listeners.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&listeners.stdout).trim(),
        format!("0.0.0.0:{}\n127.0.0.1:3128", pagis_computer::CONTROL_PORT),
        "a TCP port other than the control port and the Exit Proxy listens"
    );
    let probes = docker_exec_raw(&owner, &["--user", "agent"], &["python3", "-c", OPEN_FDS]);
    let report = String::from_utf8_lossy(&probes.stdout).into_owned();
    assert!(
        probes.status.success(),
        "{}",
        String::from_utf8_lossy(&probes.stderr)
    );
    assert!(
        report.contains("chromium"),
        "no browser was probed: {report}"
    );
    assert!(
        report.contains("pagis-screend"),
        "screend was not probed: {report}"
    );
    assert!(
        report.lines().all(|line| line.ends_with("refused")),
        "the agent opened a descriptor of the browser or of screend:\n{report}"
    );
}

/// Prints the address and the port of each TCP socket that listens,
/// except the ones of Docker's resolver on 127.0.0.11. The kernel
/// writes an IPv4 address in the byte order of the machine, and both
/// architectures of the image are little-endian.
const LISTENERS: &str = r#"
import socket
listeners = set()
for table in ("/proc/net/tcp", "/proc/net/tcp6"):
    for line in open(table).read().splitlines()[1:]:
        fields = line.split()
        address, port = fields[1].split(":")
        if fields[3] != "0A" or address == "0B00007F":
            continue
        if len(address) == 8:
            address = socket.inet_ntoa(bytes.fromhex(address)[::-1])
        listeners.add(f"{address}:{int(port, 16)}")
print("\n".join(sorted(listeners)))
"#;

/// Tries to list and to open the file descriptors of every Chromium
/// and screend process, and prints one line for each try.
const OPEN_FDS: &str = r#"
import os
for pid in sorted(filter(str.isdigit, os.listdir("/proc")), key=int):
    try:
        comm = open(f"/proc/{pid}/comm").read().strip()
    except OSError:
        continue
    if comm not in ("chromium", "pagis-screend"):
        continue
    tries = {
        "list": lambda: os.listdir(f"/proc/{pid}/fd"),
        "read 3": lambda: open(f"/proc/{pid}/fd/3", "rb").close(),
        "write 4": lambda: open(f"/proc/{pid}/fd/4", "wb").close(),
    }
    for name, attempt in tries.items():
        try:
            attempt()
            print(f"{comm} {pid} {name} opened")
        except PermissionError:
            print(f"{comm} {pid} {name} refused")
        except OSError as error:
            print(f"{comm} {pid} {name} {error.strerror}")
"#;

/// The image trusts the public certificate authorities.
/// Without `ca-certificates` every HTTPS call from python3, node, uv
/// or curl stops at "unable to get local issuer certificate", and the
/// agent falls back to the browser for what is one API call.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn python_opens_an_https_connection() {
    let real = Real::new();
    let (manager, _screens) = real.manager(Duration::from_secs(600));
    let agent_id = AgentId::generate();
    manager.wake(&agent_id).await.expect("wake");
    wait_awake(&manager, &agent_id).await;

    let outcome = manager
        .shell(
            &agent_id,
            ShellCommand {
                command: "python3 -c \"import urllib.request; \
                          urllib.request.urlopen('https://example.com', timeout=30).read()\""
                    .to_string(),
                timeout: Duration::from_secs(60),
                cwd: None,
                stdin: None,
                output_cap: None,
            },
        )
        .await
        .expect("shell");

    assert_eq!(
        outcome.exit_code, 0,
        "python3 could not open an HTTPS connection: {}",
        outcome.stdout
    );
}

/// Open one URL in the browser the supervisor runs. A second Chromium
/// hands the URL to the session that already holds the profile, and
/// exits. The page therefore renders under the supervisor's flags, not
/// under this command's.
fn open_in_the_running_browser(owner: &ComputerOwner, url: &str) {
    let opened = docker_exec_raw(
        owner,
        &[
            "-u",
            "screen",
            "-e",
            "XDG_RUNTIME_DIR=/run/pagis-xdg",
            "-e",
            "WAYLAND_DISPLAY=wayland-0",
            "-e",
            "HOME=/data/screen",
        ],
        &["chromium", "--ozone-platform=wayland", "--no-sandbox", url],
    );
    assert!(
        opened.status.success(),
        "the page never reached the browser: {}",
        String::from_utf8_lossy(&opened.stderr)
    );
}

/// The page the WebGL proof opens, written into the computer.
const WRITE_WEBGL_PAGE: &str = r#"cat > /tmp/webgl.html <<'PAGE'
<body style="margin:0;background:#000">
<canvas id="c" style="display:block;width:100vw;height:100vh"></canvas>
<script>
const gl = document.getElementById("c").getContext("webgl");
if (gl) { gl.clearColor(1, 0, 1, 1); gl.clear(gl.COLOR_BUFFER_BIT); }
</script>
PAGE
"#;

/// screend's window list: one entry per open toplevel, with the
/// title the compositor holds. The read carries the container's token,
/// as the daemon's own reads do.
async fn windows(control_addr: &str, token: &str) -> serde_json::Value {
    let body = reqwest::Client::new()
        .get(format!("http://{control_addr}/windows"))
        .bearer_auth(token)
        .send()
        .await
        .expect("windows reachable")
        .text()
        .await
        .expect("windows body");
    serde_json::from_str(&body).unwrap_or_else(|err| panic!("windows is not JSON: {err}: {body}"))
}

/// The prefix the fingerprint page puts in front of its report.
const FINGERPRINT_PREFIX: &str = "pagis-fingerprint ";

/// Read the fingerprint report out of the window list. The page writes
/// its findings into `document.title`, so the report arrives as the
/// JSON tail of one window title.
async fn fingerprint_report(control_addr: &str, token: &str) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let open = windows(control_addr, token).await;
        let titles: Vec<String> = open
            .as_array()
            .expect("the window list is an array")
            .iter()
            .filter_map(|window| window["title"].as_str().map(str::to_string))
            .collect();
        if let Some(title) = titles
            .iter()
            .find(|title| title.contains(FINGERPRINT_PREFIX))
        {
            let start = title.find('{').expect("the report starts with a brace");
            let end = title.rfind('}').expect("the report ends with a brace");
            return serde_json::from_str(&title[start..=end])
                .unwrap_or_else(|err| panic!("the report is not JSON: {err}: {title}"));
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no window carries a fingerprint report: the titles are {titles:?}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// The browser fingerprint of a real Computer, and the image details
/// and launch flags under it. The timezone, the language, the font
/// list, the WebGL renderer and `navigator.webdriver` together decide
/// whether a site serves the agent a normal page. Each one is an image
/// detail or a launch flag that a change to the image can undo with no
/// symptom until searches fail. It holds these contracts:
///
/// - the container runs on the Workspace clock and the US locale: the
///   Workspace timezone reaches the container environment, and the
///   image holds the data that makes the name a clock and the locale a
///   locale;
/// - the font list is broad and carries colour emoji. Twenty font
///   files render every page in DejaVu, draw emoji as boxes, and leave
///   a font list that bot scorers read as a signal;
/// - the page in the browser reads the Workspace clock, the US
///   language, a broad font list, a full WebGL renderer and no
///   WebDriver;
/// - a WebGL page paints in the real browser. The compositor renders in
///   software, so without the ANGLE flags no WebGL context opens, maps
///   and charts do not draw, and a scorer reads the missing renderer as
///   a bot.
///
/// The fingerprint read path is the window title. The page in the
/// image writes its findings into `document.title`, and screend reports
/// the compositor's window list on `GET /windows`
/// (ext-foreign-toplevel-list-v1). Nothing else in the container
/// returns page state: screend serves pixels, a local page cannot
/// write a file, and the browser runs with no debugging port.
///
/// The WebGL proof is end to end. The page paints its whole viewport
/// with one WebGL clear, the running browser opens it, and screend's
/// frame carries the colour back — the same browser, the same flags and
/// the same compositor the agent sees.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn the_real_browser_reports_a_human_fingerprint() {
    let real = Real::new();
    let runtime = &real.runtime;
    // A timezone that is not the host's: the assertions below show that
    // the container and the browser read the Workspace clock, not a
    // default. Asia/Tokyo has one abbreviation the whole year, so
    // `date +%Z` answers JST whenever this test runs.
    let (manager, _screens) = real.manager_with(
        Duration::from_secs(600),
        Arc::new(pagis_core::NoSkills),
        "Asia/Tokyo",
    );
    let agent_id = AgentId::generate();
    let owner = real.owner(&agent_id);
    manager.wake(&agent_id).await.expect("wake");
    wait_awake(&manager, &agent_id).await;

    // The clock and the locale of the container.
    let outcome = manager
        .shell(
            &agent_id,
            ShellCommand {
                command: "date +%Z; printf '%s\\n' \"$LANG\"; locale -a | grep -ix en_US.utf8"
                    .to_string(),
                timeout: Duration::from_secs(30),
                cwd: None,
                stdin: None,
                output_cap: None,
            },
        )
        .await
        .expect("shell");
    assert_eq!(outcome.exit_code, 0, "stderr: {}", outcome.stderr);
    assert_eq!(outcome.stdout, "JST\nen_US.UTF-8\nen_US.utf8\n");
    // The compositor and Chromium read the same values: they are the
    // container's own environment, not the shell's.
    let container_env = docker_exec(&owner, &["printenv", "TZ"]);
    assert_eq!(container_env, "Asia/Tokyo\n");

    // The font files of the image.
    let listing = docker_exec(&owner, &["fc-list"]);
    let faces = listing.lines().count();
    assert!(faces > 100, "the image carries only {faces} font files");
    assert!(
        listing.contains("Noto Color Emoji"),
        "no colour emoji face in the font list"
    );

    // The fingerprint the page reads.
    let running = runtime
        .running(&owner)
        .await
        .expect("running query")
        .expect("computer is running");
    let control_addr = running.computer.control_addr;
    let token = running.computer.token;
    wait_browser_settled(&control_addr, &owner, "").await;

    open_in_the_running_browser(&owner, "file:///opt/pagis/fingerprint/index.html");
    let report = fingerprint_report(&control_addr, &token).await;

    assert_eq!(
        report["timezone"], "Asia/Tokyo",
        "the browser clock is not the Workspace clock: {report}"
    );
    assert_eq!(
        report["language"], "en-US",
        "the browser language is not en-US: {report}"
    );
    // The floor sits under the count the image gives and over
    // the few faces a bare Debian leaves.
    let fonts = report["fonts"]
        .as_u64()
        .expect("the font count is a number");
    assert!(fonts >= 10, "the page found only {fonts} fonts: {report}");
    // ANGLE on SwiftShader reports a full renderer string. An
    // empty one means that no WebGL context opened; the bare
    // "SwiftShader" is what a scorer reads as a headless browser.
    let renderer = report["webgl"]
        .as_str()
        .expect("the WebGL renderer is a string");
    assert!(!renderer.is_empty(), "no WebGL renderer: {report}");
    assert_ne!(renderer, "SwiftShader", "the renderer is the bare name");
    // The browser takes no automation flag, so the page sees no
    // WebDriver.
    assert_eq!(
        report["webdriver"], false,
        "the browser reports itself automated: {report}"
    );

    // The WebGL paint. The page opens after the fingerprint page, in
    // front of it, and turns magenta only if a WebGL context opened:
    // the canvas sits on a black body, and the clear is the single
    // draw.
    docker_exec(&owner, &["sh", "-c", WRITE_WEBGL_PAGE]);
    open_in_the_running_browser(&owner, "file:///tmp/webgl.html");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let frame = manager.preview(&agent_id).await.expect("preview");
        let picture = image::load_from_memory(&frame.png)
            .expect("the frame is a PNG")
            .to_rgb8();
        let centre = picture
            .get_pixel(picture.width() / 2, picture.height() / 2)
            .0;
        if centre[0] > 200 && centre[1] < 60 && centre[2] > 200 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the WebGL page never painted: the centre pixel is {centre:?}"
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// The page the blinking caret test types into: one text field that
/// takes the focus when the page opens, and writes what it holds into
/// the title of the page.
const WRITE_CARET_PAGE: &str = r#"cat > /tmp/caret.html <<'PAGE'
<title>caret</title>
<body style="margin:40px">
<input autofocus style="font-size:24px" oninput="document.title = 'caret ' + this.value">
</body>
PAGE
"#;

/// A focused text field blinks its caret, and the screen still counts
/// as settled: each frame over one blink cycle matches the settled
/// frame, so the caret does not hold the screenshot after typing until
/// the limit. The comparison is the one `settled_frame` makes
/// ([`pagis_computer::exec::frames_match`]), on the real caret of the
/// real browser.
///
/// The field is on a page of the Computer and not in the address bar,
/// because the address bar fetches search suggestions from the network
/// and draws them whenever they arrive.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_blinking_caret_does_not_hold_a_settled_frame() {
    use pagis_computer::exec::{InputOp, frames_match};
    let real = Real::new();
    let (manager, _screens) = real.manager(Duration::from_secs(600));
    let agent_id = AgentId::generate();
    let owner = real.owner(&agent_id);
    manager.wake(&agent_id).await.expect("wake");
    wait_awake(&manager, &agent_id).await;
    docker_exec(&owner, &["sh", "-c", WRITE_CARET_PAGE]);
    open_in_the_running_browser(&owner, "file:///tmp/caret.html");
    // The page opens in a new tab, which settles before the typing.
    manager
        .settled_frame(&agent_id)
        .await
        .expect("the page opens");
    manager
        .input(
            &agent_id,
            &[InputOp::Text {
                text: "pagis".to_string(),
            }],
        )
        .await
        .expect("typing in the field");
    // The text is in the field, so the field has the focus and its
    // caret blinks.
    let computer = real
        .runtime
        .running(&owner)
        .await
        .expect("running query")
        .expect("computer is running")
        .computer;
    wait_for_title(&computer.control_addr, &computer.token, "caret pagis").await;
    manager.settled_frame(&agent_id).await.expect("first frame");
    let settled = manager
        .settled_frame(&agent_id)
        .await
        .expect("settled frame");

    // Chromium blinks the caret every 500 ms, so frames 250 ms apart
    // over 1.5 s show the caret on and off.
    for sample in 0..6 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let frame = manager.live_frame(&agent_id).await.expect("live frame");
        assert!(
            frames_match(&settled, &frame),
            "frame {sample} of the blinking caret does not match the settled frame"
        );
    }
}

/// The target server of the Exit Proxy test, which runs in a container
/// of its own on the Tenant Network. `/hello` answers at once. `/slow`
/// sends its headers and the first kilobyte of a megabyte, writes one
/// line to `/tmp/target.log`, and then holds the connection open for ten
/// minutes. `/page` is a page whose script reads `/slow` and writes in
/// its title how the read goes.
const EXIT_TARGET: &str = r#"
import http.server, sys, time

PAGE = b"""<!doctype html><title>pagis-exit loading</title>
<script>
fetch("/slow").then(async (response) => {
  const reader = response.body.getReader();
  await reader.read();
  document.title = "pagis-exit streaming";
  try {
    while (!(await reader.read()).done) {}
    document.title = "pagis-exit ended";
  } catch (error) {
    document.title = "pagis-exit closed";
  }
}, () => { document.title = "pagis-exit refused"; });
</script>"""


class Target(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def answer(self, kind, body, length=None):
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(length or len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def do_GET(self):
        if self.path == "/page":
            self.answer("text/html", PAGE)
        elif self.path == "/hello":
            self.answer("text/plain", b"hello from the target")
        elif self.path == "/slow":
            with open("/tmp/target.log", "a") as log:
                log.write("slow\n")
            self.answer("application/octet-stream", b"x" * 1024, 1024 * 1024)
            time.sleep(600)
        else:
            self.send_error(404)

    def log_message(self, *args):
        pass


http.server.ThreadingHTTPServer(("0.0.0.0", int(sys.argv[1])), Target).serve_forever()
"#;

/// The name of the target server on the Tenant Network, and its port.
const EXIT_TARGET_NAME: &str = "exit-target";
const EXIT_TARGET_PORT: u16 = 8080;

/// Start the target server in a container of its own on the Tenant
/// Network of `real`, as the Vault tests start their site: the Computer
/// Image with `python3` for its entrypoint. The container carries the
/// test's mark, so the test removes it with the Computer. It answers
/// the container's name.
fn start_exit_target(real: &Real) -> String {
    let container = format!("pagis-exit-target-{}", real.docker.mark());
    let label = format!("{}={}", pagis_computer::TEST_LABEL, real.docker.mark());
    let network = pagis_computer::network_name(&real.workspace_id);
    let port = EXIT_TARGET_PORT.to_string();
    let created = Command::new("docker")
        .args(["create", "--name", &container, "--label", &label])
        .args(["--network", &network, "--network-alias", EXIT_TARGET_NAME])
        .args(["--entrypoint", "python3", IMAGE, "-c", EXIT_TARGET, &port])
        .output()
        .expect("docker create runs");
    assert!(
        created.status.success(),
        "the target was not created: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let started = Command::new("docker")
        .args(["start", &container])
        .output()
        .expect("docker start runs");
    assert!(
        started.status.success(),
        "the target did not start: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    container
}

/// Wait until a window of the Computer carries `title`.
async fn wait_for_title(control_addr: &str, token: &str, title: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let open = windows(control_addr, token).await;
        let titles: Vec<String> = open
            .as_array()
            .expect("the window list is an array")
            .iter()
            .filter_map(|window| window["title"].as_str().map(str::to_string))
            .collect();
        if titles.iter().any(|shown| shown.contains(title)) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no window carries {title:?}: the titles are {titles:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Every connection of the browser and of the shells leaves through the
/// Exit Proxy (ADR-0029). It holds these contracts:
///
/// - only the daemon reads the proxy's mode: screend asks for the token;
/// - every shell names the proxy: the shell of `computer_shell`, and
///   the shell of the terminal, which sudo starts;
/// - the proxy opens no tunnel to the Computer itself, by loopback or by
///   its Tenant Network address;
/// - a page in the browser, opened through the browser channel, and a
///   shell tool reach a server through the proxy: while a slow request
///   of each is open, the proxy holds their connections;
/// - a switch of the mode closes the connections that the proxy holds,
///   so both slow requests end at once and not after their ten minutes.
///
/// The target runs in a second container on the Tenant Network, because
/// the proxy opens no connection to the Computer itself. Python's
/// `urllib` is the shell tool: it reads `http_proxy` as curl does, and
/// the image has no curl.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn the_browser_and_the_shell_leave_through_the_exit_proxy() {
    let real = Real::new();
    let runtime = &real.runtime;
    let (manager, _screens) = real.manager(Duration::from_secs(600));
    let agent_id = AgentId::generate();
    let owner = real.owner(&agent_id);
    manager.wake(&agent_id).await.expect("wake");
    wait_awake(&manager, &agent_id).await;
    let computer = runtime
        .running(&owner)
        .await
        .expect("running query")
        .expect("computer is running")
        .computer;
    wait_browser_settled(&computer.control_addr, &owner, "").await;
    let shell = |command: String, timeout: Duration| {
        let manager = Arc::clone(&manager);
        let agent_id = agent_id.clone();
        async move {
            manager
                .shell(
                    &agent_id,
                    ShellCommand {
                        command,
                        timeout,
                        cwd: None,
                        stdin: None,
                        output_cap: None,
                    },
                )
                .await
                .expect("shell")
        }
    };

    // The daemon alone reads the mode.
    let refused = reqwest::get(format!("http://{}/exit", computer.control_addr))
        .await
        .expect("the control port answers");
    assert_eq!(refused.status().as_u16(), 401);
    assert_eq!(
        runtime
            .exit_status(&computer)
            .await
            .expect("the exit status")
            .mode,
        ExitMode::Direct
    );

    // Every shell names the proxy.
    let outcome = shell(
        "printenv HTTP_PROXY HTTPS_PROXY http_proxy https_proxy NO_PROXY no_proxy".to_string(),
        Duration::from_secs(30),
    )
    .await;
    assert_eq!(
        outcome.stdout,
        "http://127.0.0.1:3128\n".repeat(4)
            + &"localhost,127.0.0.1,::1,host.docker.internal\n".repeat(2)
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let terminal = docker_exec_raw(
            &owner,
            &["--user", "agent"],
            &[
                "sh",
                "-c",
                "for pid in $(pgrep -u agent -x bash); do tr '\\0' '\\n' < /proc/$pid/environ; done",
            ],
        );
        let terminal = String::from_utf8_lossy(&terminal.stdout).into_owned();
        if terminal
            .lines()
            .any(|entry| entry == "HTTPS_PROXY=http://127.0.0.1:3128")
        {
            assert!(
                terminal
                    .lines()
                    .any(|entry| entry == "no_proxy=localhost,127.0.0.1,::1,host.docker.internal"),
                "{terminal}"
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the shell of the terminal does not name the proxy: {terminal}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // The target, in a container of its own on the Tenant Network.
    let target_container = start_exit_target(&real);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !reaches(&owner, EXIT_TARGET_NAME, EXIT_TARGET_PORT) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the target does not listen"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let target = format!("http://{EXIT_TARGET_NAME}:{EXIT_TARGET_PORT}");

    // The proxy opens a tunnel to the other container, and none to the
    // Computer itself, where screend's control port listens on every
    // address: a page's name that resolves to the Computer reaches
    // nothing in it.
    assert_eq!(
        tunnel_status(&owner, EXIT_TARGET_NAME, EXIT_TARGET_PORT),
        Some(200)
    );
    let own_address = container_ip(&owner);
    for own in ["127.0.0.1", "localhost", own_address.as_str()] {
        assert_eq!(
            tunnel_status(&owner, own, pagis_computer::CONTROL_PORT),
            Some(403),
            "the Exit Proxy opened a tunnel to {own}"
        );
    }

    // A shell tool reaches the target.
    let outcome = shell(
        format!(
            "python3 -c \"import urllib.request; \
             print(urllib.request.urlopen('{target}/hello', timeout=30).read().decode())\""
        ),
        Duration::from_secs(60),
    )
    .await;
    assert_eq!(outcome.exit_code, 0, "{}", outcome.stderr);
    assert_eq!(outcome.stdout, "hello from the target\n");

    // A page in the browser reaches the target, and its script holds a
    // slow request open.
    runtime
        .set_holder(&computer, InputHolder::Daemon)
        .await
        .expect("holder set to daemon");
    let opened = runtime
        .browser_open(&computer, &format!("{target}/page"))
        .await
        .expect("the page opens");
    assert_eq!(opened, format!("{target}/page"));
    runtime
        .set_holder(&computer, InputHolder::Agent)
        .await
        .expect("holder set back to agent");
    wait_for_title(
        &computer.control_addr,
        &computer.token,
        "pagis-exit streaming",
    )
    .await;

    // A shell tool holds a slow request open too.
    let slow = tokio::spawn(shell(
        format!(
            "python3 -c \"import urllib.request; \
             response = urllib.request.urlopen('{target}/slow', timeout=300); \
             response.read()\""
        ),
        Duration::from_secs(120),
    ));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let log = Command::new("docker")
            .args(["exec", &target_container, "cat", "/tmp/target.log"])
            .output()
            .expect("docker exec runs");
        if String::from_utf8_lossy(&log.stdout).lines().count() >= 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the slow requests did not both reach the target"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Both slow requests pass through the proxy: it holds their
    // connections.
    let status = runtime
        .exit_status(&computer)
        .await
        .expect("the exit status");
    assert!(
        status.connections >= 2,
        "the proxy holds {} connections while two slow requests are open",
        status.connections
    );

    // A switch closes them, so both end now.
    let closed = runtime
        .set_exit_mode(&computer, ExitMode::Direct)
        .await
        .expect("the switch");
    assert!(closed >= 2, "the switch closed {closed} connections");
    let outcome = tokio::time::timeout(Duration::from_secs(60), slow)
        .await
        .expect("the slow shell request ends after the switch")
        .expect("the shell task ends");
    assert_ne!(
        outcome.exit_code, 124,
        "the slow shell request ran to its deadline"
    );
    assert!(
        ["IncompleteRead", "ConnectionResetError"]
            .iter()
            .any(|closed| outcome.stderr.contains(closed)),
        "the slow shell request did not end with a closed connection: {}",
        outcome.stderr
    );
    wait_for_title(&computer.control_addr, &computer.token, "pagis-exit closed").await;
    assert_eq!(
        runtime
            .exit_status(&computer)
            .await
            .expect("the exit status")
            .mode,
        ExitMode::Direct
    );
}

/// The address of one container on its Tenant Network.
fn container_ip(owner: &ComputerOwner) -> String {
    let output = Command::new("docker")
        .args([
            "inspect",
            "-f",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
            &owner.container_name(),
        ])
        .output()
        .expect("docker inspect runs");
    assert!(
        output.status.success(),
        "docker inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Asks the Exit Proxy of the Computer for a tunnel to
/// `argv[1]:argv[2]`, and prints the status code of the answer, or
/// nothing when no answer comes in five seconds.
const TUNNEL_PROBE: &str = r#"
import socket, sys
target = f"{sys.argv[1]}:{sys.argv[2]}"
try:
    with socket.create_connection(("127.0.0.1", 3128), timeout=5) as proxy:
        proxy.sendall(f"CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n".encode())
        print(proxy.recv(64).decode(errors="replace").split(" ")[1])
except OSError:
    pass
"#;

/// The status code that the Exit Proxy of `from` answers a tunnel to
/// `address:port` with, for the agent's shell (ADR-0029), or `None` when
/// it gives no answer in five seconds. The proxy dials from the
/// Computer, so the egress rules hold its connections as they hold the
/// shell's own.
pub(crate) fn tunnel_status(from: &ComputerOwner, address: &str, port: u16) -> Option<u16> {
    let output = docker_exec_raw(
        from,
        &["--user", "agent"],
        &["python3", "-c", TUNNEL_PROBE, address, &port.to_string()],
    );
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Whether a TCP connection from inside `from` reaches `address:port`.
/// Bash's own `/dev/tcp` makes the connection, so the answer needs no
/// tool the image might not ship.
pub(crate) fn reaches(from: &ComputerOwner, address: &str, port: u16) -> bool {
    let probe = format!("timeout 5 bash -c 'exec 3<>/dev/tcp/{address}/{port}'");
    Command::new("docker")
        .args(["exec", &from.container_name(), "bash", "-c", &probe])
        .output()
        .expect("docker exec runs")
        .status
        .success()
}

/// The Tenant Network is a boundary (ADR-0014): two containers of
/// one tenant reach each other, and no container reaches a container of
/// another tenant, on any port. The first half is the control: without
/// it a broken probe would pass the test.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_container_reaches_its_own_tenant_and_not_another() {
    let real = Real::new();
    let runtime = &real.runtime;
    let first = real.owner(&AgentId::generate());
    let second = real.owner(&AgentId::generate());
    let stranger = ComputerOwner::new(WorkspaceId::generate(), AgentId::generate());

    let locale = pagis_computer::container_env("UTC");
    let (first_boot, second_boot, stranger_boot) = tokio::join!(
        runtime.start(&first, &[], &locale),
        runtime.start(&second, &[], &locale),
        runtime.start(&stranger, &[], &locale),
    );
    for boot in [first_boot, second_boot, stranger_boot] {
        boot.expect("the container boots");
    }

    let second_ip = container_ip(&second);
    let stranger_ip = container_ip(&stranger);
    assert_ne!(second_ip, "", "the container has no address");
    assert_ne!(stranger_ip, "", "the container has no address");

    // Each probe that finds no answer waits out its own timeout, so the
    // probes run side by side. The control endpoint answers inside one
    // tenant, and nothing of another tenant answers, on the control
    // port, on the media port, or on a port nothing listens on.
    let stranger_ports = [
        pagis_computer::CONTROL_PORT,
        pagis_computer::MEDIA_PORT,
        22,
        80,
    ];
    let (own, strangers) = std::thread::scope(|scope| {
        let own = scope.spawn(|| reaches(&first, &second_ip, pagis_computer::CONTROL_PORT));
        let (first, stranger_ip) = (&first, stranger_ip.as_str());
        let strangers: Vec<_> = stranger_ports
            .into_iter()
            .map(|port| scope.spawn(move || (port, reaches(first, stranger_ip, port))))
            .collect();
        (
            own.join().expect("the probe runs"),
            strangers
                .into_iter()
                .map(|probe| probe.join().expect("the probe runs"))
                .collect::<Vec<_>>(),
        )
    });
    assert!(own, "a container cannot reach its own tenant's container");
    for (port, reached) in strangers {
        assert!(
            !reached,
            "a container reached another tenant's container on port {port}"
        );
    }
}

/// One `docker inspect` of a started container says what it may take
/// and who owns it.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_started_container_carries_the_limits_and_the_owner_labels() {
    let real = Real::new();
    let runtime = &real.runtime;
    let owner = real.owner(&AgentId::generate());
    runtime
        .start(&owner, &[], &pagis_computer::container_env("UTC"))
        .await
        .expect("the container boots");

    let inspected = Command::new("docker")
        .args(["inspect", &owner.container_name()])
        .output()
        .expect("docker inspect runs");
    let inspected: serde_json::Value =
        serde_json::from_slice(&inspected.stdout).expect("docker inspect answers JSON");
    let container = &inspected[0];
    let host = &container["HostConfig"];
    let limits = ComputerLimits::default();

    assert_eq!(
        host["NetworkMode"].as_str(),
        Some(pagis_computer::network_name(&real.workspace_id).as_str())
    );
    // No media port is published at all (ADR-0014): the control
    // port is the one published port, on loopback, and the pipeline
    // registers outbound with the Media Relay at the Docker host.
    let published = host["PortBindings"].as_object().expect("port bindings");
    assert_eq!(published.len(), 1, "{published:?}");
    let control = format!("{}/tcp", pagis_computer::CONTROL_PORT);
    assert_eq!(published[&control][0]["HostIp"].as_str(), Some("127.0.0.1"));
    assert_eq!(
        host["ExtraHosts"],
        serde_json::json!([format!("{}:host-gateway", pagis_computer::RELAY_HOST)])
    );
    assert_eq!(host["Memory"].as_i64(), Some(limits.memory_bytes));
    assert_eq!(host["NanoCpus"].as_i64(), Some(limits.nano_cpus));
    assert_eq!(host["PidsLimit"].as_i64(), Some(limits.pids));
    assert_eq!(host["ShmSize"].as_i64(), Some(limits.shm_bytes));
    assert_eq!(host["Ulimits"][0]["Hard"].as_i64(), Some(limits.open_files));
    let labels = &container["Config"]["Labels"];
    assert_eq!(
        labels[pagis_computer::WORKSPACE_LABEL].as_str(),
        Some(real.workspace_id.as_str())
    );
    assert_eq!(
        labels[pagis_computer::AGENT_LABEL].as_str(),
        Some(owner.agent_id.as_str())
    );
}

/// A test removes every Docker object it created. The runtime marks the
/// container, the volume and the Tenant Network with the test's label,
/// and the drop of the harness removes all three.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_test_leaves_no_container_volume_or_tenant_network_behind() {
    let real = Real::new();
    let owner = real.owner(&AgentId::generate());
    real.runtime
        .start(&owner, &[], &pagis_computer::container_env("UTC"))
        .await
        .expect("the container boots");
    let mark = real.docker.mark().to_string();
    assert_eq!(
        marked_objects(&mark).expect("the test objects are listed"),
        [
            owner.container_name(),
            pagis_computer::network_name(&real.workspace_id),
            owner.volume_name(),
        ]
    );

    drop(real);

    assert_eq!(
        marked_objects(&mark).expect("the test objects are listed"),
        Vec::<String>::new()
    );
}

/// A test that fails removes its Docker objects too: the panic drops
/// the harness.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_failed_test_leaves_no_docker_object_behind() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let failed = tokio::spawn(async move {
        let real = Real::new();
        let owner = real.owner(&AgentId::generate());
        real.runtime
            .start(&owner, &[], &pagis_computer::container_env("UTC"))
            .await
            .expect("the container boots");
        sender
            .send(real.docker.mark().to_string())
            .expect("the mark is sent");
        panic!("the test fails with its container awake");
    })
    .await;

    assert!(failed.expect_err("the test panics").is_panic());
    let mark = receiver.recv().expect("the mark arrives");
    assert_eq!(
        marked_objects(&mark).expect("the test objects are listed"),
        Vec::<String>::new()
    );
}

/// The screend control port asks for the token the daemon minted:
/// reaching the port is not the same as holding it.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn the_control_port_refuses_a_request_without_the_token() {
    let real = Real::new();
    let runtime = &real.runtime;
    let owner = real.owner(&AgentId::generate());
    let computer = runtime
        .start(&owner, &[], &pagis_computer::container_env("UTC"))
        .await
        .expect("the container boots");

    let client = reqwest::Client::new();
    let refused = client
        .get(format!("http://{}/frame.png", computer.control_addr))
        .send()
        .await
        .expect("the control port answers");
    assert_eq!(refused.status().as_u16(), 401);

    let refused = client
        .post(format!("http://{}/input", computer.control_addr))
        .json(&serde_json::json!({ "holder": "agent", "ops": [] }))
        .send()
        .await
        .expect("the control port answers");
    assert_eq!(refused.status().as_u16(), 401);

    // The daemon holds the token, so its own read works.
    let frame = runtime
        .fetch_frame(&computer)
        .await
        .expect("the daemon reads a frame");
    assert!(!frame.is_empty());
    // The sprite's own shell cannot read the token out of its container.
    let unreadable = docker_exec_raw(
        &owner,
        &["--user", "agent"],
        &["cat", "/run/pagis/screend-token"],
    );
    assert!(!unreadable.status.success());
}

/// The tenant's Plugin Computer answers a streaming exec: the
/// daemon writes to the process's stdin, reads its stdout, and gets its
/// stderr apart. A Plugin's MCP server speaks its protocol over exactly
/// this, so no server process runs on the daemon host.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn the_plugin_computer_carries_a_server_over_a_streaming_exec() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let real = Real::new();
    let (manager, _screens) = real.manager(Duration::from_secs(600));
    let computer = manager
        .ensure_plugin_computer(Vec::new())
        .await
        .expect("the plugin computer wakes");
    // The Plugin Computer is the tenant's container, named for the
    // reserved Agent.
    assert!(
        computer.container.len() > 1,
        "the plugin computer has no container"
    );

    let mut stream = manager
        .plugin_server(
            &computer,
            ExecRequest {
                argv: vec![
                    "bash".to_string(),
                    "-c".to_string(),
                    "echo ready >&2; while read line; do echo \"you said $line\"; done".to_string(),
                ],
                user: "agent".to_string(),
                cwd: SHELL_HOME.to_string(),
                env: vec!["HOME=/data/agent".to_string()],
                stdin: None,
                output_cap: OutputCap { head: 0, tail: 0 },
            },
        )
        .await
        .expect("the server starts in the container");

    stream
        .stdin
        .write_all(b"hello\n")
        .await
        .expect("the daemon writes to the server");
    stream.stdin.flush().await.expect("the write reaches it");
    let mut lines = BufReader::new(stream.stdout).lines();
    let answer = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
        .await
        .expect("the server answers in time")
        .expect("the stream is readable")
        .expect("one line");
    assert_eq!(answer, "you said hello");

    // The stderr of the server is a stream of its own, for the log.
    let noise = tokio::time::timeout(Duration::from_secs(30), stream.stderr.recv())
        .await
        .expect("the stderr arrives in time")
        .expect("one chunk");
    assert!(
        String::from_utf8_lossy(&noise).contains("ready"),
        "{}",
        String::from_utf8_lossy(&noise)
    );

    // The Plugin Computer runs screend too, so the Exit Proxy that its
    // environment names listens in it (ADR-0029), and a server's
    // connections do not point at nothing.
    let owner = real.owner(&pagis_computer::plugin_agent());
    assert_eq!(
        docker_exec(&owner, &["printenv", "HTTPS_PROXY"]),
        "http://127.0.0.1:3128\n"
    );
    assert!(
        reaches(&owner, "127.0.0.1", 3128),
        "the Plugin Computer has no Exit Proxy"
    );
}

/// A daemon that has made no volume in this run asks Docker whether it
/// holds a volume to a size, with a probe volume that it removes. So the
/// Administration Interface has the answer after a restart, before the
/// next Computer wakes.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_new_runtime_asks_docker_for_the_volume_quota() {
    let docker = TestDocker::new();
    let runtime = BollardRuntime::new(
        Arc::new(DockerDiscovery::production(None)),
        RuntimeOptions {
            limits: ComputerLimits::default(),
            tokens_dir: std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("screend-tokens"),
            labels: docker.labels(),
        },
    );

    let answer = runtime.volume_quota().await;

    assert_ne!(answer, pagis_computer::Quota::Unknown);
    assert_eq!(runtime.volume_quota().await, answer);
    assert_eq!(
        marked_objects(docker.mark()).expect("the test objects are listed"),
        Vec::<String>::new(),
        "the probe volume stays"
    );
}

/// A wake learns from the container create whether the storage driver
/// holds the writable layer to its size. Where it does, a write outside
/// `/data` stops at that size. Every other host answers `unsupported`,
/// and on no host is the answer still `unknown` after a wake.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_wake_learns_whether_the_writable_layer_is_bounded() {
    const LAYER_MIB: u64 = 512;
    let real = Real::with_limits(ComputerLimits {
        layer_bytes: Some(LAYER_MIB * 1024 * 1024),
        ..ComputerLimits::default()
    });
    let owner = real.owner(&AgentId::generate());
    assert_eq!(
        real.runtime.container_quota().await,
        pagis_computer::Quota::Unknown
    );

    real.runtime
        .start(&owner, &[], &pagis_computer::container_env("UTC"))
        .await
        .expect("the container boots");

    let answer = real.runtime.container_quota().await;
    assert_ne!(answer, pagis_computer::Quota::Unknown);
    if answer == pagis_computer::Quota::Supported {
        let count = format!("count={}", LAYER_MIB + 64);
        let output = docker_exec_raw(
            &owner,
            &["--user", "agent"],
            &["dd", "if=/dev/zero", "of=/tmp/fill", "bs=1M", &count],
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "a write past the layer size went through: {stderr}"
        );
        assert!(stderr.contains("No space left on device"), "{stderr}");
    }
}
