//! Docker-real fill tests, behind `#[ignore]`: the gate runs them
//! where Docker is reachable (`cargo nextest run --workspace
//! --run-ignored only`) in the pinned image its `computer-image` step
//! builds from `computer/`.
//!
//! Each test wakes a real Computer and serves the test site in `site/`
//! on its Tenant Network: a Caddy container that answers for
//! `login.example.com`, the site of the test Credential, and for
//! `sso.example.org`, a site of another registrable domain. Caddy signs
//! both names with its own local certificate authority. The test gives
//! that authority to the Computer's Chromium through the
//! `CACertificates` policy, the way an organization adds a private
//! authority, so the browser verifies a real chain and the image
//! carries no test flag.
//!
//! A page writes what its fields hold into the document title, and the
//! test reads the title from screend's window list.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use pagis_computer::exec::InputOp;
use pagis_computer::fake::{FakeAgents, FakeWorkspaces};
use pagis_computer::test_docker::TestDocker;
use pagis_computer::{
    AwakeCaps, AwakeCeiling, BollardRuntime, ComputerLimits, ComputerManager, ComputerManagerDeps,
    ComputerOwner, ComputerRuntime, ComputerState, DockerDiscovery, IMAGE, RuntimeOptions,
    StartedComputer, TEST_LABEL,
};
use pagis_core::{
    AgentId, Event, EventBus, EventId, EventScope, EventStream, NewEvent, StoreError, WorkspaceId,
    now_ms,
};
use pagis_vault::VaultError;
use sha2::{Digest, Sha256};

/// The Caddy image the test site runs in.
const SITE_IMAGE: &str = "caddy:2-alpine";
/// The registrable domain of the test Credential.
const DOMAIN: &str = "example.com";
const LOGIN_URL: &str = "https://login.example.com/login.html";
/// A login page whose form never navigates, so the developer-tools and
/// address-bar tests keep their password field across retries.
const INJECT_URL: &str = "https://login.example.com/inject.html";
const USERNAME: &str = "alice@example.com";
/// The dummy secret. It is long and odd enough that no log line or
/// file holds it by chance.
const SECRET: &str = "Dummy-Secret-q7Rz4wK2";
/// The RFC 6238 test seed.
const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
/// The file the Agent's shell writes when the terminal has the focus.
const TERMINAL_FOCUSED: &str = "/data/agent/terminal-focused";
/// The file the Agent's recording program writes what the terminal
/// receives into.
const KEYLOG: &str = "/data/agent/keylog";
/// The address a steered Agent types after a fill: a `javascript:` URL
/// that reads the password field and puts the value in the tab title,
/// which each screenshot shows and screend's window list reports. `pass`
/// is the password field, which the page names with its id. The `void`
/// stops the browser from replacing the page with the returned value.
/// The trailing `//x` comments out a last character that the address bar
/// sometimes drops, so a dropped character never makes the script
/// invalid. The address is short, because the address bar keeps a short
/// address but loses a long one.
const READ_THE_SECRET_URL: &str = "javascript:void(document.title=pass.value)//x";

fn require_image(image: &str) {
    let present = Command::new("docker")
        .args(["image", "inspect", image])
        .output()
        .expect("docker image inspect runs")
        .status
        .success();
    assert!(
        present,
        "image {image} is not built; run `docker build -t {IMAGE} computer` at the workspace root"
    );
}

/// One Docker CLI call, with `stdin` written to the process when given.
fn docker(args: &[&str], stdin: Option<&[u8]>) -> Output {
    let mut child = Command::new("docker")
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("docker runs");
    if let Some(bytes) = stdin {
        child
            .stdin
            .take()
            .expect("docker stdin")
            .write_all(bytes)
            .expect("write docker stdin");
    }
    child.wait_with_output().expect("docker finishes")
}

fn succeeded(output: Output, what: &str) -> String {
    assert!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A bus that keeps nothing: these tests assert against the Computer,
/// not the audit trail.
struct SilentBus;

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

/// One awake Computer of a Workspace of its own.
struct Desk {
    manager: Arc<ComputerManager>,
    agent_id: AgentId,
    owner: ComputerOwner,
    computer: StartedComputer,
    workspace_id: WorkspaceId,
    _screens: tempfile::TempDir,
    /// Dropped last: it removes every container, volume and Tenant
    /// Network of the test, the site among them.
    docker: TestDocker,
}

impl Desk {
    async fn wake() -> Self {
        require_image(IMAGE);
        let docker = TestDocker::new();
        let runtime = Arc::new(BollardRuntime::new(
            Arc::new(DockerDiscovery::production(None)),
            RuntimeOptions {
                limits: ComputerLimits::default(),
                // A colima bind mount from the system temporary
                // directory arrives empty, so the token file lives here.
                tokens_dir: Path::new(env!("CARGO_TARGET_TMPDIR")).join("screend-tokens"),
                labels: docker.labels(),
            },
        ));
        let workspace_id = WorkspaceId::generate();
        let screens = tempfile::tempdir().expect("screens dir");
        let manager = ComputerManager::new(ComputerManagerDeps {
            runtime: Arc::clone(&runtime) as _,
            skills: Arc::new(pagis_core::NoSkills),
            workspaces: Arc::new(FakeWorkspaces::with_timezone(&workspace_id, "UTC")),
            agents: Arc::new(FakeAgents::open()),
            bus: Arc::new(SilentBus),
            workspace_id: workspace_id.clone(),
            screens_dir: screens.path().to_path_buf(),
            idle_stop: Duration::from_secs(600),
            relay: pagis_computer::fake::loopback_relay(),
            ceiling: Arc::new(AwakeCeiling::new(AwakeCaps::default())),
        });
        let agent_id = AgentId::generate();
        manager.wake(&agent_id).await.expect("wake");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
        while manager.state(&agent_id).await != ComputerState::Awake {
            assert!(tokio::time::Instant::now() < deadline, "never woke");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let owner = ComputerOwner::new(workspace_id.clone(), agent_id.clone());
        let computer = runtime
            .running(&owner)
            .await
            .expect("running query")
            .expect("the computer runs")
            .computer;
        Self {
            manager,
            agent_id,
            owner,
            computer,
            workspace_id,
            _screens: screens,
            docker,
        }
    }

    /// Run one shell command in the Computer as `user`.
    fn exec(&self, user: &str, command: &str, stdin: Option<&[u8]>) -> Output {
        docker(
            &[
                "exec",
                "-i",
                "-u",
                user,
                &self.owner.container_name(),
                "sh",
                "-c",
                command,
            ],
            stdin,
        )
    }

    /// Give the Computer's Chromium the site's certificate authority,
    /// and restart the browser, which reads its policies when it starts.
    async fn trust(&self, site: &Site) {
        let policy = serde_json::json!({ "CACertificates": [site.authority().await] });
        succeeded(
            self.exec(
                "root",
                "cat > /etc/chromium/policies/managed/test-authority.json",
                Some(policy.to_string().as_bytes()),
            ),
            "writing the policy",
        );
        let before = self.browser_pid();
        self.exec("root", "pkill -KILL chromium", None);
        self.wait_browser(&before).await;
    }

    /// Open `url` in a new tab of the running browser. A second Chromium
    /// hands the address to the session that holds the profile and exits,
    /// so the page loads under the browser's own flags and policies.
    fn open_page(&self, url: &str) {
        succeeded(
            self.exec(
                "screen",
                &format!(
                    "XDG_RUNTIME_DIR=/run/pagis-xdg WAYLAND_DISPLAY=wayland-0 HOME=/data/screen \
                     chromium --ozone-platform=wayland --no-sandbox '{url}'"
                ),
                None,
            ),
            "opening the page",
        );
    }

    /// The oldest live Chromium pid, or an empty string.
    fn browser_pid(&self) -> String {
        let found = self.exec("root", "pgrep -o chromium", None);
        String::from_utf8_lossy(&found.stdout).trim().to_string()
    }

    /// Wait for a browser other than `excluding` that stays up.
    async fn wait_browser(&self, excluding: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        loop {
            let pid = self.browser_pid();
            if !pid.is_empty() && pid != excluding {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if self.browser_pid() == pid {
                    return;
                }
                continue;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no browser came back"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// The titles of the open windows.
    async fn titles(&self) -> Vec<String> {
        let body = reqwest::Client::new()
            .get(format!("http://{}/windows", self.computer.control_addr))
            .bearer_auth(&self.computer.token)
            .send()
            .await
            .expect("windows reachable")
            .text()
            .await
            .expect("windows body");
        let windows: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|err| panic!("windows is not JSON: {err}: {body}"));
        windows
            .as_array()
            .expect("the window list is an array")
            .iter()
            .filter_map(|window| window["title"].as_str().map(str::to_string))
            .collect()
    }

    /// Wait until one window title starts with `wanted`.
    async fn wait_title(&self, wanted: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let titles = self.titles().await;
            if titles.iter().any(|title| title.starts_with(wanted)) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no window title starts with {wanted:?}: {titles:?}"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Press Return until one window title starts with `wanted`. A new
    /// tab takes the keyboard a moment after it opens, so a single Return
    /// can arrive too early; a repeat is harmless on these pages.
    async fn press_return_until_title(&self, wanted: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            self.agent_types(vec![key(&["Return"])]).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let titles = self.titles().await;
            if titles.iter().any(|title| title.starts_with(wanted)) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no window title starts with {wanted:?}: {titles:?}"
            );
        }
    }

    /// Open `url` in the address bar. F6 gives the address bar the focus
    /// and selects its text. F6 needs no modifier, which the virtual
    /// keyboard delivers where a Ctrl chord does not. The focus and the
    /// address go in separate batches, because the browser moves the
    /// focus a moment after the key; but the address follows soon, since
    /// the focus returns to the page when the address bar stays empty for
    /// too long. F6 misses now and then, so the caller repeats.
    async fn open_in_address_bar(&self, url: &str) {
        self.agent_types(vec![key(&["F6"])]).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        self.agent_types(vec![text(url)]).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        self.agent_types(vec![key(&["Return"])]).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    /// Watch the window titles for `within`, and fail when one of them
    /// shows `secret`.
    async fn assert_no_title_shows(&self, secret: &str, within: Duration) {
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            let titles = self.titles().await;
            assert!(
                !titles.iter().any(|title| title.contains(secret)),
                "the screen shows the secret: {titles:?}"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// The count of pixels that are not near-white in the right side of
    /// a settled screenshot. The login page leaves that region blank, so
    /// a high count is the developer tools panel, which docks there.
    async fn devtools_pixels(&self) -> u64 {
        let png = self
            .manager
            .settled_frame(&self.agent_id)
            .await
            .expect("a settled frame");
        let picture = image::load_from_memory(&png)
            .expect("the frame is a PNG")
            .to_rgb8();
        let (right, bottom) = (picture.width().min(1270), picture.height().min(700));
        let mut count = 0;
        for y in (120..bottom).step_by(2) {
            for x in (820..right).step_by(2) {
                let [r, g, b] = picture.get_pixel(x, y).0;
                if r < 200 || g < 200 || b < 200 {
                    count += 1;
                }
            }
        }
        count
    }

    /// Input the Agent sends to the screen.
    async fn agent_types(&self, ops: Vec<InputOp>) {
        self.manager
            .input(&self.agent_id, &ops)
            .await
            .expect("the agent's input");
    }

    /// Do what a steered Agent does: give the focus to the desktop
    /// terminal with the compositor's window switch, and start a program
    /// there that records what the terminal receives.
    async fn focus_the_terminal_and_record_it(&self) {
        self.agent_types(vec![key(&["alt", "Tab"])]).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        self.agent_types(vec![InputOp::Text {
            text: format!("touch {TERMINAL_FOCUSED}; cat > {KEYLOG}\n"),
        }])
        .await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while !self
            .exec("root", &format!("test -f {TERMINAL_FOCUSED}"), None)
            .status
            .success()
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the terminal never took the focus"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// End the recording and read it. A Return hands the line the
    /// terminal holds to the program, and ctrl+d ends it.
    async fn recorded(&self) -> String {
        self.agent_types(vec![
            InputOp::Text {
                text: "\n".to_string(),
            },
            key(&["ctrl", "d"]),
        ])
        .await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let log = self.exec("root", &format!("cat {KEYLOG}"), None);
        String::from_utf8_lossy(&log.stdout).into_owned()
    }

    /// Everything the Computer's processes wrote to their standard
    /// streams: screend, the compositor and Chromium.
    fn container_log(&self) -> String {
        let output = docker(&["logs", &self.owner.container_name()], None);
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

fn key(keys: &[&str]) -> InputOp {
    InputOp::Key {
        keys: keys.iter().map(|key| key.to_string()).collect(),
        hold_ms: None,
    }
}

fn text(text: &str) -> InputOp {
    InputOp::Text {
        text: text.to_string(),
    }
}

/// The test site, in a container on the desk's Tenant Network.
struct Site {
    container: String,
}

impl Site {
    /// Serve `site/` for `login.example.com` and `sso.example.org`. Port
    /// 8443 of `login.example.com` takes a connection and never answers,
    /// for a page that does not load. The container carries the test's
    /// mark, so the test removes it with the Computer.
    fn start(desk: &Desk) -> Self {
        let container = format!("pagis-site-{}", desk.docker.mark());
        let network = pagis_computer::network_name(&desk.workspace_id);
        let label = format!("{TEST_LABEL}={}", desk.docker.mark());
        succeeded(
            docker(
                &[
                    "create",
                    "--name",
                    &container,
                    "--label",
                    &label,
                    "--network",
                    &network,
                    "--network-alias",
                    "login.example.com",
                    "--network-alias",
                    "sso.example.org",
                    SITE_IMAGE,
                    "sh",
                    "-c",
                    "nc -lk -p 8443 -e sleep 86400 & \
                     exec caddy run --config /srv/site/Caddyfile --adapter caddyfile",
                ],
                None,
            ),
            "creating the site",
        );
        let files = PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
        )
        .join("tests/site/.");
        succeeded(
            docker(
                &[
                    "cp",
                    files.to_str().expect("a UTF-8 path"),
                    &format!("{container}:/srv/site"),
                ],
                None,
            ),
            "copying the site",
        );
        succeeded(docker(&["start", &container], None), "starting the site");
        Self { container }
    }

    /// The site's certificate authority, as the base64 DER that the
    /// `CACertificates` policy takes. Caddy writes it when it signs its
    /// first certificate, so this waits for it.
    async fn authority(&self) -> String {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let output = docker(
                &[
                    "exec",
                    &self.container,
                    "cat",
                    "/data/caddy/pki/authorities/local/root.crt",
                ],
                None,
            );
            if output.status.success() && !output.stdout.is_empty() {
                return String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter(|line| !line.starts_with("-----"))
                    .collect();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the site wrote no certificate authority: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

/// A desk whose browser trusts the test site.
async fn desk_with_site() -> (Desk, Site) {
    let desk = Desk::wake().await;
    let site = Site::start(&desk);
    desk.trust(&site).await;
    (desk, site)
}

/// The fill of the Credential for `example.com` at `login_url`.
async fn fill(
    desk: &Desk,
    login_url: &str,
    username: &str,
    secret: &str,
) -> Result<(), VaultError> {
    pagis_vault::fill::open_and_fill(
        &desk.manager,
        &desk.agent_id,
        DOMAIN,
        login_url,
        username,
        secret,
    )
    .await
}

/// The code fill of the Credential for `example.com`.
async fn fill_code(desk: &Desk, code: &str) -> Result<(), VaultError> {
    pagis_vault::fill::fill_code(&desk.manager, &desk.agent_id, DOMAIN, code).await
}

fn current_code() -> String {
    pagis_vault::totp::current_code(SEED).expect("the test seed makes a code")
}

fn sha256_hex(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether `text` holds `code` as a whole number: a log line is full of
/// digits, and six of them in a row inside a longer number are not the
/// code.
fn holds_number(text: &str, code: &str) -> bool {
    text.match_indices(code).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + code.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_digit()) && !after.is_some_and(|c| c.is_ascii_digit())
    })
}

/// A fill on an https page of the Credential's own site writes the
/// username and the password into the page's fields. After the Agent
/// submits the form, a code fill writes the current code into the code
/// field of the next step.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn a_fill_and_a_code_fill_write_into_the_fields_of_the_credentials_site() {
    let (desk, _site) = desk_with_site().await;

    fill(&desk, LOGIN_URL, USERNAME, SECRET)
        .await
        .expect("the fill");
    desk.wait_title(&format!("login|{USERNAME}|{}", sha256_hex(SECRET)))
        .await;

    // The Agent submits the form itself, and the site asks for the code.
    desk.agent_types(vec![key(&["Return"])]).await;
    desk.wait_title("otp").await;
    let code = current_code();
    fill_code(&desk, &code).await.expect("the code fill");
    desk.wait_title(&format!("otp|{code}")).await;
}

/// A steered Agent gives the focus to the desktop terminal and starts a
/// program there that records what the terminal receives. A fill and a
/// code fill then put nothing in the terminal, and no container file or
/// log line holds the secret or the code.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn a_fill_with_the_terminal_focused_puts_nothing_in_the_terminal() {
    let (desk, _site) = desk_with_site().await;
    desk.focus_the_terminal_and_record_it().await;

    let code = current_code();
    let filled = fill(&desk, LOGIN_URL, USERNAME, SECRET).await;
    let coded = fill_code(&desk, &code).await;

    let recorded = desk.recorded().await;
    assert!(
        !recorded.contains(SECRET),
        "the terminal received the secret: {recorded:?}"
    );
    assert!(
        !holds_number(&recorded, &code),
        "the terminal received the code: {recorded:?}"
    );
    let files = desk.exec(
        "root",
        &format!("grep -rlF -e '{SECRET}' /data /tmp /run 2>/dev/null"),
        None,
    );
    assert!(
        files.stdout.is_empty(),
        "a container file holds the secret: {}",
        String::from_utf8_lossy(&files.stdout)
    );
    let log = desk.container_log();
    assert!(!log.contains(SECRET), "a log line holds the secret");
    assert!(!holds_number(&log, &code), "a log line holds the code");
    // The fill wrote into the page whatever window had the focus, and
    // the code fill found no code field there.
    filled.expect("the fill with the terminal focused");
    assert!(
        coded.is_err(),
        "a code fill into a password field reported success"
    );
}

/// A login address that redirects to another registrable domain gets
/// nothing: the fill reports that it failed. A code fill on that page
/// of another domain also writes nothing.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn a_fill_that_lands_on_another_domain_writes_nothing() {
    let (desk, _site) = desk_with_site().await;

    let redirected = fill(&desk, "https://login.example.com/moved", USERNAME, SECRET).await;
    let code = current_code();
    let coded = fill_code(&desk, &code).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let titles = desk.titles().await;
    let digest = sha256_hex(SECRET);
    assert!(
        !titles
            .iter()
            .any(|title| title.contains(USERNAME) || title.contains(&digest)),
        "the page of another domain received the login: {titles:?}"
    );
    assert!(
        !titles.iter().any(|title| holds_number(title, &code)),
        "the page of another domain received the code: {titles:?}"
    );
    assert!(redirected.is_err(), "the redirected fill reported success");
    assert!(
        coded.is_err(),
        "the code fill on another domain reported success"
    );
}

/// A page that moves the focus to a frame of another site when the
/// password field takes it does not move the secret: the frame receives
/// nothing, and the fill reports that it failed.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn a_focus_change_during_the_fill_does_not_move_the_secret() {
    let (desk, _site) = desk_with_site().await;

    let result = fill(
        &desk,
        "https://login.example.com/steal.html",
        USERNAME,
        SECRET,
    )
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let titles = desk.titles().await;
    assert!(
        !titles.iter().any(|title| title.starts_with("caught|")),
        "the frame of another site received text: {titles:?}"
    );
    assert!(result.is_err(), "the fill reported success");
}

/// A fill whose page does not load writes nothing: not into the address
/// bar, and not into the page the tab showed before.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn a_fill_whose_page_does_not_load_writes_nothing() {
    let (desk, _site) = desk_with_site().await;
    fill(&desk, LOGIN_URL, USERNAME, SECRET)
        .await
        .expect("the first fill");
    let filled = format!("login|{USERNAME}|{}", sha256_hex(SECRET));
    desk.wait_title(&filled).await;

    // Port 8443 takes the connection and never answers the TLS
    // handshake, so the page never loads.
    let stalled = fill(
        &desk,
        "https://login.example.com:8443/",
        "bob@example.com",
        "Second-Secret-8Kd2vQ",
    )
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let titles = desk.titles().await;
    assert!(
        titles.iter().any(|title| title.starts_with(&filled)),
        "the page the tab showed before changed: {titles:?}"
    );
    assert!(
        !titles.iter().any(|title| title.contains("bob@example.com")),
        "the second login reached a page: {titles:?}"
    );
    assert!(
        stalled.is_err(),
        "a fill on a page that never loaded reported success"
    );
}

/// A sign-in page that gives the focus to no field still gets the fill:
/// the fill finds the login fields of the page itself, as a password
/// manager does. It writes the username into the last visible text field
/// before the password field, and not into a hidden field between them.
/// The code step, which gives the focus to no field either, gets the
/// code in its one-time-code field.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn a_fill_on_a_page_that_focuses_no_field_writes_into_its_login_fields() {
    let (desk, _site) = desk_with_site().await;

    fill(
        &desk,
        "https://login.example.com/nofocus.html",
        USERNAME,
        SECRET,
    )
    .await
    .expect("the fill");
    // The hidden field stays empty: the title ends with its empty value.
    desk.wait_title(&format!("login|{USERNAME}|{}| - ", sha256_hex(SECRET)))
        .await;

    desk.agent_types(vec![key(&["Return"])]).await;
    desk.wait_title("otp").await;
    let code = current_code();
    fill_code(&desk, &code).await.expect("the code fill");
    desk.wait_title(&format!("otp|{code}")).await;
}

/// A page that gives the focus to a field outside its sign-in form gets
/// nothing: the fill does not pick fields around a focus of the page.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn a_fill_on_a_page_that_focuses_a_field_outside_the_login_form_writes_nothing() {
    let (desk, _site) = desk_with_site().await;

    let result = fill(
        &desk,
        "https://login.example.com/search.html",
        USERNAME,
        SECRET,
    )
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let titles = desk.titles().await;
    let digest = sha256_hex(SECRET);
    assert!(
        !titles
            .iter()
            .any(|title| title.contains(USERNAME) || title.contains(&digest)),
        "the page received the login: {titles:?}"
    );
    assert!(result.is_err(), "the fill reported success");
}

/// After a fill, a steered Agent presses F12, the shortcut that opens
/// the browser's developer tools. The developer tools dock to the right
/// of the window, where the login page is blank. A managed policy turns
/// the developer tools off, so F12 opens no panel there: it shows a small
/// "not allowed" dialog instead. The Agent dismisses the dialog with
/// Escape, and the right of the screen stays blank, so the Agent has no
/// console to read the password field from.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn f12_after_a_fill_does_not_open_the_developer_tools() {
    let (desk, _site) = desk_with_site().await;
    fill(&desk, INJECT_URL, USERNAME, SECRET)
        .await
        .expect("the fill");
    desk.wait_title(&format!("login|{USERNAME}|{}", sha256_hex(SECRET)))
        .await;

    desk.agent_types(vec![key(&["F12"])]).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    // A blocked F12 dims the screen behind a "not allowed" dialog. Escape
    // closes the dialog and leaves the page as it was; a real panel stays
    // open through the same Escape.
    desk.agent_types(vec![key(&["Escape"])]).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let pixels = desk.devtools_pixels().await;
    assert_eq!(
        pixels, 0,
        "the developer tools opened: {pixels} painted pixels on the right of the screen"
    );
}

/// After a fill, a steered Agent opens a `javascript:` address in the
/// address bar that reads the password field into the tab title. The
/// address bar shortcut misses now and then, so the Agent repeats, as a
/// prompt-injected model does. A managed policy blocks `javascript:`
/// addresses, so no attempt runs the script and the screen never shows
/// the secret.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn a_javascript_address_after_a_fill_does_not_show_the_secret() {
    let (desk, _site) = desk_with_site().await;
    fill(&desk, INJECT_URL, USERNAME, SECRET)
        .await
        .expect("the fill");
    desk.wait_title(&format!("login|{USERNAME}|{}", sha256_hex(SECRET)))
        .await;

    for _ in 0..8 {
        desk.open_in_address_bar(READ_THE_SECRET_URL).await;
    }

    desk.assert_no_title_shows(SECRET, Duration::from_secs(5))
        .await;
}

/// The managed policy blocks a `javascript:` address that the Agent types
/// in the address bar, and a bookmarklet, and nothing else. A page's own
/// `javascript:` link and `javascript:` form action still run, so a site
/// that signs in through them keeps working.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn a_pages_own_javascript_link_and_form_action_still_run() {
    let (desk, _site) = desk_with_site().await;

    desk.open_page("https://login.example.com/jslink.html");
    desk.wait_title("jslink").await;
    desk.press_return_until_title("link ran").await;

    desk.open_page("https://login.example.com/jsform.html");
    desk.wait_title("jsform").await;
    desk.press_return_until_title("form ran").await;
}

/// The managed policy blocks the `view-source:` scheme only. A page whose
/// address holds the text "view-source", in its path and in its query,
/// loads as any other page.
#[tokio::test]
#[ignore = "needs Docker; run via cargo nextest run -p pagis-vault --run-ignored only"]
async fn a_page_whose_address_holds_view_source_still_loads() {
    let (desk, _site) = desk_with_site().await;

    desk.open_page("https://login.example.com/view-source.html?next=view-source:x");

    desk.wait_title("view-source page loaded").await;
}
