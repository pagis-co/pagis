//! A live screen in a real browser, against a real Computer, when the
//! Session of the viewer ends.
//!
//! Headless Chrome opens the live screen the way `LiveScreen` does: a
//! WebRTC session through the Media Relay to screend in the Computer,
//! with the input data channel. The viewer takes over the Computer, and
//! its mouse reaches screend. Then the viewer signs out. The Media Relay
//! path of that Session closes, so the browser receives no more video
//! and its input no longer reaches screend. The Takeover stays in place,
//! because another Session of the Person can hold it.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use pagis_computer::test_docker::TestDocker;
use pagis_computer::{
    BollardRuntime, ComputerLimits, ComputerOwner, ComputerRuntime, DockerDiscovery, IMAGE,
    InputHolder, RuntimeOptions, StartedComputer,
};
use pagis_core::AgentId;
use pagis_testkit::browser::{Browser, Tab};
use pagis_testkit::{TestDaemon, TestDaemonOptions};

/// A page that repaints the whole screen ten times a second, so screend
/// sends video for as long as a viewer can receive it.
const REPAINTING_PAGE: &str = "<!doctype html><body style=\"margin:0\">\
    <div id=\"paint\" style=\"width:100vw;height:100vh\"></div><script>\
    let step = 0;\
    setInterval(() => {\
      step = (step + 37) % 256;\
      document.getElementById('paint').style.background =\
        'rgb(' + step + ',' + (255 - step) + ',128)';\
    }, 100);\
    </script>";

fn require_docker_and_image() {
    let image = std::process::Command::new("docker")
        .args(["image", "inspect", IMAGE])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    assert!(
        image,
        "Docker is not reachable or image {IMAGE} is not built. Start Docker, set DOCKER_HOST \
         (for example `unix:///Users/<you>/.colima/default/docker.sock`) and build the image \
         with `docker build -t {IMAGE} computer`."
    );
}

async fn post(daemon: &TestDaemon, cookie: &str, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}{path}", daemon.base_url))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
}

async fn computer_of(daemon: &TestDaemon, cookie: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!(
            "{}/api/v1/agents/{}/computer",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// How much video the viewer of the tab has received: `{frames, bytes}`.
async fn received(tab: &Tab) -> (u64, u64) {
    let stats: serde_json::Value = tab
        .evaluate(
            "async () => {
                for (const report of (await window.liveScreen.pc.getStats()).values()) {
                    if (report.type === 'inbound-rtp' && report.kind === 'video') {
                        return { frames: report.framesReceived ?? 0, bytes: report.bytesReceived ?? 0 }
                    }
                }
                return { frames: 0, bytes: 0 }
            }",
        )
        .await;
    (
        stats["frames"].as_u64().unwrap_or(0),
        stats["bytes"].as_u64().unwrap_or(0),
    )
}

/// The viewer moves the mouse, over the input data channel of the live
/// screen, as `LiveScreen` sends it during a Takeover.
async fn move_the_mouse(tab: &Tab, x: u32) {
    let sent: bool = tab
        .evaluate(&format!(
            "() => {{
                const channel = window.liveScreen.channel
                if (channel.readyState !== 'open') return false
                channel.send(JSON.stringify({{ op: 'move', x: {x}, y: 400 }}))
                return true
            }}"
        ))
        .await;
    assert!(sent, "the input channel is not open");
}

async fn idle_ms(runtime: &BollardRuntime, computer: &StartedComputer) -> u64 {
    runtime
        .user_input_idle_ms(computer)
        .await
        .expect("screend reports the user-input idle time")
}

/// An awake Computer whose browser shows a page that keeps changing,
/// so its screen sends video without a pause, and the daemon it belongs
/// to.
struct Repainting {
    _docker: TestDocker,
    runtime: Arc<BollardRuntime>,
    daemon: TestDaemon,
    /// Another Session of the seeded Person, for the test's own reads.
    observer: String,
    owner: ComputerOwner,
    computer: StartedComputer,
}

async fn repainting_computer() -> Repainting {
    require_docker_and_image();
    let docker = TestDocker::new();
    let runtime = Arc::new(BollardRuntime::new(
        Arc::new(DockerDiscovery::production(None)),
        RuntimeOptions {
            limits: ComputerLimits::default(),
            // A Colima bind mount from the system temporary directory
            // arrives empty, so the screend token lives here.
            tokens_dir: std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("screend-tokens"),
            labels: docker.labels(),
        },
    ));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        computer: Arc::clone(&runtime) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let observer = daemon.cookie_for(&daemon.user_id).await;
    let agent = AgentId::from(daemon.agent_id.clone());
    let owner = ComputerOwner::new(daemon.workspace_id.clone(), agent.clone());

    let woke = post(
        &daemon,
        &observer,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;
    assert_eq!(woke.status(), 202);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while computer_of(&daemon, &observer).await["state"] != "awake" {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Computer never woke"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let computer = runtime
        .running(&owner)
        .await
        .expect("the running query answers")
        .expect("the Computer runs")
        .computer;
    // The browser of the Computer shows a page that keeps changing, so
    // the screen sends video without a pause. The browser channel
    // answers only while the daemon holds the switch.
    runtime
        .set_holder(&computer, InputHolder::Daemon)
        .await
        .expect("the daemon holds the switch");
    runtime
        .browser_open(
            &computer,
            &format!(
                "data:text/html;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(REPAINTING_PAGE)
            ),
        )
        .await
        .expect("the browser of the Computer opens the page");
    runtime
        .set_holder(&computer, InputHolder::Agent)
        .await
        .expect("the switch goes back to the Agent");

    Repainting {
        _docker: docker,
        runtime,
        daemon,
        observer,
        owner,
        computer,
    }
}

#[tokio::test]
#[ignore = "needs Docker, the Computer Image and Chrome; run with --run-ignored"]
async fn a_sign_out_stops_the_video_and_the_input_of_its_live_screen_during_a_takeover() {
    // The tab holds the seeded Session and signs it out. The test reads
    // the daemon with another Session of the same Person.
    let Repainting {
        _docker,
        runtime,
        daemon,
        observer,
        owner,
        computer,
    } = repainting_computer().await;

    let browser = Browser::launch().await;
    let tab = browser.signed_in(&daemon).await;
    // The viewer takes over, and opens the live screen as `LiveScreen`
    // does.
    let started: u16 = tab
        .evaluate(&format!(
            "async () => (await fetch('/api/v1/agents/{}/screen/takeover', {{ method: 'POST' }})).status",
            daemon.agent_id
        ))
        .await;
    assert_eq!(started, 200, "the takeover is refused");
    let offered: String = tab
        .evaluate(&format!(
            "async () => {{
                const ice = await (await fetch('/api/v1/screen/ice')).json()
                const pc = new RTCPeerConnection({{ iceServers: ice.ice_servers }})
                const channel = pc.createDataChannel('input')
                pc.addTransceiver('video', {{ direction: 'recvonly' }})
                pc.ontrack = (event) => {{
                    const video = document.createElement('video')
                    video.muted = true
                    video.autoplay = true
                    video.srcObject = new MediaStream([event.track])
                    document.body.appendChild(video)
                }}
                window.liveScreen = {{ pc, channel }}
                const offer = await pc.createOffer()
                await pc.setLocalDescription(offer)
                const answered = await fetch('/api/v1/agents/{}/screen/offer', {{
                    method: 'POST',
                    headers: {{ 'content-type': 'application/json' }},
                    body: JSON.stringify({{ sdp: offer.sdp }}),
                }})
                if (!answered.ok) return 'the offer answered ' + answered.status
                await pc.setRemoteDescription({{ type: 'answer', sdp: (await answered.json()).sdp }})
                return 'offered'
            }}",
            daemon.agent_id
        ))
        .await;
    assert_eq!(offered, "offered");

    // The live screen plays: the session connects and video arrives.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let state: serde_json::Value = tab
            .evaluate(
                "() => ({ connection: window.liveScreen.pc.connectionState, \
                   channel: window.liveScreen.channel.readyState })",
            )
            .await;
        let (frames, _) = received(&tab).await;
        if state["connection"] == "connected" && state["channel"] == "open" && frames > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the live screen never played: {state}, {frames} frames"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let (before, _) = received(&tab).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (after, _) = received(&tab).await;
    assert!(
        after > before,
        "the live screen sends no video while the screen changes"
    );
    // The mouse of the viewer reaches screend during the Takeover: each
    // input restarts the user-input idle clock.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert!(idle_ms(&runtime, &computer).await >= 1_000);
    move_the_mouse(&tab, 300).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while idle_ms(&runtime, &computer).await >= 1_000 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the input of the viewer never reached screend"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let signed_out: u16 = tab
        .evaluate(
            "async () => (await fetch('/api/v1/sessions/current', { method: 'DELETE' })).status",
        )
        .await;
    assert_eq!(signed_out, 204);

    // Packets already on their way land; then nothing more arrives.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (frames_then, bytes_then) = received(&tab).await;
    let quiet_since = tokio::time::Instant::now();
    for step in 0..30 {
        move_the_mouse(&tab, 100 + step * 20).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (frames_now, bytes_now) = received(&tab).await;
    assert_eq!(
        (frames_now, bytes_now),
        (frames_then, bytes_then),
        "video still reaches the viewer of the signed-out Session"
    );
    let idle = idle_ms(&runtime, &computer).await;
    assert!(
        u128::from(idle) >= quiet_since.elapsed().as_millis(),
        "input of the signed-out viewer reached screend: idle for {idle} ms"
    );
    // The Takeover stays in place: another Session of the Person can hold
    // it, and it ends at its inactivity timeout.
    assert_eq!(computer_of(&daemon, &observer).await["holder"], "user");

    runtime.stop(&owner).await.expect("the Computer stops");
}

/// Open the live screen in `tab` as `LiveScreen` does, and keep the
/// peer connection in `window.liveScreen`.
async fn open_live_screen(tab: &Tab, agent_id: &str) {
    let offered: String = tab
        .evaluate(&format!(
            "async () => {{
                const ice = await (await fetch('/api/v1/screen/ice')).json()
                const pc = new RTCPeerConnection({{ iceServers: ice.ice_servers }})
                const channel = pc.createDataChannel('input')
                pc.addTransceiver('video', {{ direction: 'recvonly' }})
                pc.ontrack = (event) => {{
                    const video = document.createElement('video')
                    video.muted = true
                    video.autoplay = true
                    video.srcObject = new MediaStream([event.track])
                    document.body.appendChild(video)
                }}
                window.liveScreen = {{ pc, channel }}
                const offer = await pc.createOffer()
                await pc.setLocalDescription(offer)
                const answered = await fetch('/api/v1/agents/{agent_id}/screen/offer', {{
                    method: 'POST',
                    headers: {{ 'content-type': 'application/json' }},
                    body: JSON.stringify({{ sdp: offer.sdp }}),
                }})
                if (!answered.ok) return 'the offer answered ' + answered.status
                await pc.setRemoteDescription({{ type: 'answer', sdp: (await answered.json()).sdp }})
                return 'offered'
            }}"
        ))
        .await;
    assert_eq!(offered, "offered");
}

/// The connection state of the live screen in `tab`, its local
/// candidates and the state of each candidate pair.
async fn session_report(tab: &Tab) -> serde_json::Value {
    tab.evaluate(
        "async () => {
            const pc = window.liveScreen.pc
            const locals = {}
            const pairs = []
            const stats = await pc.getStats()
            for (const report of stats.values()) {
                if (report.type === 'local-candidate') {
                    locals[report.id] = report.address + ':' + report.port
                }
            }
            for (const report of stats.values()) {
                if (report.type === 'candidate-pair') {
                    pairs.push({
                        local: locals[report.localCandidateId],
                        state: report.state,
                        nominated: report.nominated,
                        requests: report.requestsSent,
                        responses: report.responsesReceived,
                    })
                }
            }
            return { connection: pc.connectionState, ice: pc.iceConnectionState, pairs }
        }",
    )
    .await
}

/// A page that holds a camera or microphone grant gathers a host
/// candidate on every network interface, so the browser checks several
/// pairs against the one address of the Media Relay. The live screen
/// still connects, and it stays connected past the time ICE consent
/// takes to expire (RFC 7675: 30 s), with video arriving.
#[tokio::test]
#[ignore = "needs Docker, the Computer Image and Chrome; run with --run-ignored"]
async fn a_page_with_a_microphone_grant_keeps_its_live_screen() {
    let Repainting {
        _docker,
        runtime,
        daemon,
        owner,
        ..
    } = repainting_computer().await;

    let browser = Browser::launch_with_fake_media().await;
    browser.grant_microphone(&daemon.base_url).await;
    let tab = browser.signed_in(&daemon).await;
    let granted: bool = tab
        .evaluate(
            "async () => {
                window.microphone = await navigator.mediaDevices.getUserMedia({ audio: true })
                return window.microphone.active
            }",
        )
        .await;
    assert!(granted, "the page holds no microphone grant");
    open_live_screen(&tab, &daemon.agent_id).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let report = session_report(&tab).await;
        let (frames, _) = received(&tab).await;
        if report["connection"] == "connected" && frames > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the live screen never played: {report}, {frames} frames"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    let held_until = tokio::time::Instant::now() + Duration::from_secs(45);
    while tokio::time::Instant::now() < held_until {
        let (before, _) = received(&tab).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        let (after, _) = received(&tab).await;
        let report = session_report(&tab).await;
        assert!(
            report["connection"] == "connected" && after > before,
            "the live screen stopped: {report}, {before} -> {after} frames"
        );
    }

    runtime.stop(&owner).await.expect("the Computer stops");
}
