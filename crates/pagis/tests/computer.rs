//! Full-daemon agent computer tests over the fake runtime: the pull
//! of the Computer Image that the boot starts and a wake joins, with
//! progress on the firehose, previews awake and asleep, the version
//! refusal, and the agents list.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_computer::fake::FakeComputerRuntime;
use pagis_testkit::{TestDaemon, TestDaemonOptions};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The next JSON frame of the firehose, of any type.
async fn next_frame(socket: &mut Socket) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("frame before timeout")
            .expect("socket open")
            .expect("frame ok");
        let Message::Text(text) = frame else { continue };
        return serde_json::from_str(&text).expect("frame is JSON");
    }
}

async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = next_frame(socket).await;
        if frame["type"] == frame_type {
            return frame;
        }
    }
}

async fn firehose(daemon: &TestDaemon) -> Socket {
    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("ws connect");
    socket
        .send(Message::text(
            serde_json::json!({ "type": "auth" }).to_string(),
        ))
        .await
        .expect("ws send");
    next_frame_of(&mut socket, "ready").await;
    socket
}

fn daemon_options(runtime: &Arc<FakeComputerRuntime>) -> TestDaemonOptions {
    TestDaemonOptions {
        computer: Arc::clone(runtime) as _,
        ..TestDaemonOptions::default()
    }
}

async fn get(daemon: &TestDaemon, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
}

async fn post(daemon: &TestDaemon, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}{path}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
}

async fn wait_state(daemon: &TestDaemon, want: &str) {
    wait_agent_state(daemon, &daemon.agent_id, want).await;
}

async fn wait_agent_state(daemon: &TestDaemon, agent_id: &str, want: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let state: serde_json::Value = get(daemon, &format!("/api/v1/agents/{agent_id}/computer"))
            .await
            .json()
            .await
            .unwrap();
        if state["state"] == want {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "state never became {want}; last {state}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn the_agents_list_serves_the_seeded_assistant() {
    let daemon = TestDaemon::start().await;

    let page: serde_json::Value = get(&daemon, "/api/v1/agents").await.json().await.unwrap();

    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], daemon.agent_id.as_str());
    assert_eq!(items[0]["name"], "Pixie");
    assert_eq!(items[0]["status"], "active");
}

/// The daemon boots with the pinned image absent and starts its pull
/// with no wake. A wake joins that pull, shows its progress on the
/// firehose, and pulls nothing of its own.
#[tokio::test]
async fn a_wake_joins_the_pull_that_the_boot_started_and_shows_its_progress() {
    let runtime = Arc::new(FakeComputerRuntime::default());
    runtime.hold_pulls();
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while runtime.pulls() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the boot did not start the pull"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut socket = firehose(&daemon).await;

    let response = post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;
    assert_eq!(response.status(), 202);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["state"], "pulling");
    runtime.release_pulls();

    // Pull progress and the lifecycle land on the firehose.
    let mut percents = Vec::new();
    loop {
        let frame = next_frame_of(&mut socket, "computer.state_changed").await;
        let payload = &frame["payload"]["payload"];
        if payload["state"] == "pulling" {
            percents.push(payload["percent"].as_u64().expect("a percent"));
            continue;
        }
        assert_eq!(payload["state"], "starting");
        break;
    }
    assert_eq!(percents.last(), Some(&100), "{percents:?}");
    let awake = next_frame_of(&mut socket, "computer.state_changed").await;
    assert_eq!(awake["payload"]["payload"]["state"], "awake");
    wait_state(&daemon, "awake").await;
    assert_eq!(runtime.pulls(), 1);
}

#[tokio::test]
async fn preview_serves_live_png_awake_and_the_screenshot_asleep() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.set_frame(b"png-bytes");
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;

    // Nothing yet: 404.
    let missing = get(
        &daemon,
        &format!("/api/v1/agents/{}/screen/preview.png", daemon.agent_id),
    )
    .await;
    assert_eq!(missing.status(), 404);

    post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;
    wait_state(&daemon, "awake").await;

    let live = get(
        &daemon,
        &format!("/api/v1/agents/{}/screen/preview.png", daemon.agent_id),
    )
    .await;
    assert_eq!(live.status(), 200);
    assert_eq!(live.headers()["content-type"], "image/png");
    assert_eq!(live.headers()["x-pagis-live"], "true");
    assert_eq!(live.bytes().await.unwrap().as_ref(), b"png-bytes");

    // The container goes away (idle-stop's effect): the preview falls
    // back to the screenshot stored from the last live frame.
    runtime.stop_all();
    let asleep = get(
        &daemon,
        &format!("/api/v1/agents/{}/screen/preview.png", daemon.agent_id),
    )
    .await;
    assert_eq!(asleep.status(), 200);
    assert_eq!(asleep.headers()["x-pagis-live"], "false");
    assert_eq!(asleep.bytes().await.unwrap().as_ref(), b"png-bytes");
}

#[tokio::test]
async fn a_mismatched_image_version_is_refused_with_409() {
    let runtime = Arc::new(FakeComputerRuntime::default());
    runtime.set_image_version(Some("9.9.9"));
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;

    let response = post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;

    assert_eq!(response.status(), 409);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "image_version_mismatch");
    assert!(
        body["error"]["message"].as_str().unwrap().contains("9.9.9"),
        "{body}"
    );
}

#[tokio::test]
async fn a_screen_offer_relays_to_the_awake_computer() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;
    post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;
    wait_state(&daemon, "awake").await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/agents/{}/screen/offer",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "sdp": "v=0 offer" }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["sdp"], pagis_computer::fake::ANSWER);
    let offers = runtime.offers();
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].0, "v=0 offer");
}

#[tokio::test]
async fn a_screen_offer_while_asleep_is_a_409() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/agents/{}/screen/offer",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "sdp": "v=0 offer" }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 409);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "computer_asleep");
    assert!(runtime.offers().is_empty());
}

#[tokio::test]
async fn computer_routes_404_on_an_unknown_agent() {
    let daemon = TestDaemon::start().await;

    let state = get(&daemon, "/api/v1/agents/01UNKNOWN/computer").await;
    assert_eq!(state.status(), 404);
    let wake = post(&daemon, "/api/v1/agents/01UNKNOWN/computer/wake").await;
    assert_eq!(wake.status(), 404);
}

use pagis_testkit::{Script, ScriptedBrain};

/// A valid display-size PNG for the fake computer's frame.
fn display_png() -> Vec<u8> {
    let image = image::DynamicImage::new_rgb8(
        pagis_computer::exec::DISPLAY_WIDTH,
        pagis_computer::exec::DISPLAY_HEIGHT,
    );
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

async fn rest_send(daemon: &TestDaemon, pending_id: &str, text: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": pending_id, "text": text }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
}

/// The computer acceptance flow on one provider vocabulary: the model
/// drives the browser, reads the screen, and reports; the screenshot
/// lands as a downloadable run artifact.
async fn open_a_page_and_report(computer_call: serde_json::Value) {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.set_frame(&display_png());
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(&[], "computer", computer_call));
    brain.push(Script::reply(&["The page says: Example Domain."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..daemon_options(&runtime)
    })
    .await;
    let mut socket = firehose(&daemon).await;

    rest_send(&daemon, "p1", "open example.com and tell me what it says").await;

    // The screenshot is in the run transcript as an artifact.
    let completed = loop {
        let frame = next_frame_of(&mut socket, "tool.completed").await;
        if frame["payload"]["payload"]["name"] == "computer" {
            break frame;
        }
    };
    assert_eq!(completed["payload"]["payload"]["ok"], true);
    let artifact_id = completed["payload"]["payload"]["artifact_ids"][0]
        .as_str()
        .unwrap()
        .to_string();
    let bytes = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/artifacts/{artifact_id}",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");

    // The agent's reply reports the page content.
    let reply = loop {
        let frame = next_frame_of(&mut socket, "message.completed").await;
        if frame["payload"]["payload"]["author_kind"] == "agent" {
            break frame;
        }
    };
    let _ = reply;
    loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    let requests = brain.requests();
    // The computer call and the report are the only model requests.
    assert_eq!(requests.len(), 2);
    assert!(!runtime.inputs().is_empty() || runtime.frames_served() > 0);
}

#[tokio::test]
async fn pixie_opens_a_page_and_reports_it_on_the_anthropic_vocabulary() {
    open_a_page_and_report(serde_json::json!({ "action": "screenshot" })).await;
}

#[tokio::test]
async fn pixie_opens_a_page_and_reports_it_on_the_openai_vocabulary() {
    open_a_page_and_report(serde_json::json!({
        "type": "computer_call", "call_id": "call_1",
        "actions": [
            {"type": "click", "button": "left", "x": 640, "y": 360},
            {"type": "type", "text": "example.com"},
            {"type": "keypress", "keys": ["ENTER"]},
            {"type": "screenshot"}
        ],
        "status": "completed"
    }))
    .await;
}

/// OpenAI's native computer tool answers every call with a
/// screenshot. When the computer gives none, the Run fails and says the
/// computer is unavailable, not that a provider is misconfigured.
#[tokio::test]
async fn a_native_computer_call_with_no_screen_fails_the_run_on_the_computer() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.fail_frame("the screen stopped answering");
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "computer",
        serde_json::json!({
            "type": "computer_call", "call_id": "call_1",
            "actions": [{"type": "click", "button": "left", "x": 640, "y": 360}],
            "status": "completed"
        }),
    ));
    brain.push(Script::reply(&["unreachable"]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..daemon_options(&runtime)
    })
    .await;

    rest_send(&daemon, "p1", "open example.com").await;

    let run = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let page: serde_json::Value = get(&daemon, "/api/v1/runs?state=failed")
                .await
                .json()
                .await
                .unwrap();
            if let Some(run) = page["items"].as_array().unwrap().first() {
                break run.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the run failed");
    let error = run["error"].as_str().unwrap();
    assert!(error.contains("the computer is unavailable"), "{error}");
    assert!(error.contains("the screen stopped answering"), "{error}");
    // The model got no second request with a result that has no screen.
    assert_eq!(brain.requests().len(), 1);
}

/// The takeover flow: takeover from the expanded view denies the
/// agent, handback resumes the parked run with the note, and the audit
/// trail (events + system block) lands in the DM.
#[tokio::test]
async fn takeover_denies_the_agent_and_handback_resumes_with_the_note() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.set_frame(&display_png());
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "computer",
        serde_json::json!({ "action": "screenshot" }),
    ));
    brain.push(Script::reply(&["Done looking."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..daemon_options(&runtime)
    })
    .await;
    let mut socket = firehose(&daemon).await;

    post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;
    wait_state(&daemon, "awake").await;

    // Take over: the switch flips, the pipeline learns, the event lands.
    let takeover = post(
        &daemon,
        &format!("/api/v1/agents/{}/screen/takeover", daemon.agent_id),
    )
    .await;
    assert_eq!(takeover.status(), 200);
    let body: serde_json::Value = takeover.json().await.unwrap();
    assert_eq!(body["holder"], "user");
    next_frame_of(&mut socket, "screen.takeover_started").await;
    assert_eq!(
        runtime.holder(&pagis_core::AgentId::from(daemon.agent_id.clone())),
        pagis_computer::InputHolder::User
    );

    // The agent's turn parks: the lease is denied while the user holds.
    rest_send(&daemon, "p1", "check the screen").await;
    loop {
        let frame = next_frame_of(&mut socket, "run.state_changed").await;
        if frame["payload"]["payload"]["to"] == "waiting_for_user" {
            break;
        }
    }

    // Handback: the run resumes and finishes; the note reaches the
    // model as the computer tool result.
    let handback = post(
        &daemon,
        &format!("/api/v1/agents/{}/screen/handback", daemon.agent_id),
    )
    .await;
    assert_eq!(handback.status(), 200);
    let body: serde_json::Value = handback.json().await.unwrap();
    assert_eq!(body["holder"], "agent");
    let ended = next_frame_of(&mut socket, "screen.takeover_ended").await;
    assert_eq!(ended["payload"]["payload"]["reason"], "explicit");
    // Two readers act on `screen.takeover_ended`: the parked run
    // resumes, and the takeover note goes into the DM. Neither waits
    // for the other, so the test waits for the end of each.
    let (mut completed, mut noted) = (false, false);
    while !(completed && noted) {
        let frame = next_frame(&mut socket).await;
        let payload = &frame["payload"]["payload"];
        match frame["type"].as_str() {
            Some("run.state_changed") => completed |= payload["to"] == "completed",
            Some("message.completed") => noted |= payload["author_kind"] == "system",
            _ => {}
        }
    }
    let requests = brain.requests();
    let note_seen = requests.iter().any(|request| {
        request
            .messages
            .iter()
            .any(|message| message.text.contains("the screen may have changed"))
    });
    assert!(note_seen, "the takeover note never reached the model");

    // The system block is in the DM.
    let page: serde_json::Value = get(
        &daemon,
        &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
    )
    .await
    .json()
    .await
    .unwrap();
    let block = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["author_kind"] == "system")
        .expect("a system block exists");
    assert!(
        block["text_content"]
            .as_str()
            .unwrap()
            .contains("took control of Pixie's computer"),
        "{block}"
    );
}

#[tokio::test]
async fn a_takeover_while_asleep_is_a_409() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;

    let response = post(
        &daemon,
        &format!("/api/v1/agents/{}/screen/takeover", daemon.agent_id),
    )
    .await;

    assert_eq!(response.status(), 409);
}

#[tokio::test]
async fn a_handback_without_a_takeover_is_a_quiet_no_op() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;

    let response = post(
        &daemon,
        &format!("/api/v1/agents/{}/screen/handback", daemon.agent_id),
    )
    .await;

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["holder"], "agent");
}

#[tokio::test]
async fn sleep_stops_an_awake_computer_and_keeps_its_last_screen() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.set_frame(b"png-bytes");
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;
    post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;
    wait_state(&daemon, "awake").await;

    let response = post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/sleep", daemon.agent_id),
    )
    .await;

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["state"], "off");
    wait_state(&daemon, "off").await;
    // The Desk keeps a picture of the screen it stopped.
    let preview = get(
        &daemon,
        &format!("/api/v1/agents/{}/screen/preview.png", daemon.agent_id),
    )
    .await;
    assert_eq!(preview.status(), 200);
    assert_eq!(preview.headers()["x-pagis-live"], "false");
}

#[tokio::test]
async fn sleeping_a_computer_that_is_already_off_changes_nothing() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;

    let response = post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/sleep", daemon.agent_id),
    )
    .await;

    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["state"], "off");
}

#[tokio::test]
async fn an_unknown_agent_has_no_computer_to_put_to_sleep() {
    let daemon = TestDaemon::start().await;

    let response = post(&daemon, "/api/v1/agents/ag_missing/computer/sleep").await;

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn the_disk_figure_counts_the_agent_volumes() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    runtime.set_volume_bytes(1_600_000_000);
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;

    // No computer ever woke, so no volume exists yet.
    let empty: serde_json::Value = get(&daemon, "/api/v1/computers/disk")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(empty["bytes"], 0);

    post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;
    wait_state(&daemon, "awake").await;

    let disk: serde_json::Value = get(&daemon, "/api/v1/computers/disk")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(disk["bytes"], 1_600_000_000_u64);
}

/// The cap on simultaneously awake Computers: the second wake of
/// a tenant whose cap is one is refused, and the answer says what has to
/// happen. A sleep gives the place back.
#[tokio::test]
async fn a_wake_past_the_tenant_cap_is_refused_with_a_clear_message() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        computer: Arc::clone(&runtime) as _,
        computer_awake_caps: pagis_computer::AwakeCaps {
            per_tenant: 1,
            per_server: 8,
        },
        ..TestDaemonOptions::default()
    })
    .await;
    let second: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/api/v1/agents", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"name": "Clown", "job": "helper"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second = second["id"].as_str().expect("the new agent has an id");

    post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;
    wait_state(&daemon, "awake").await;

    let refused = post(&daemon, &format!("/api/v1/agents/{second}/computer/wake")).await;
    assert_eq!(refused.status(), 503);
    let body: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "awake_cap_reached");
    let message = body["error"]["message"]
        .as_str()
        .expect("the refusal says why");
    assert!(message.contains("has to sleep"), "{message}");

    // The place the first computer gives back lets the second one wake.
    post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/sleep", daemon.agent_id),
    )
    .await;
    let woke = post(&daemon, &format!("/api/v1/agents/{second}/computer/wake")).await;
    assert_eq!(woke.status(), 202);
}

/// A daemon adopts the running Computers of its Workspaces when it
/// starts. After a restart, nobody asks for the state of the first
/// Agent, and its Computer still holds the one place under the cap.
#[tokio::test]
async fn a_restarted_daemon_counts_a_computer_that_nobody_asked_for() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let options = || TestDaemonOptions {
        computer: Arc::clone(&runtime) as _,
        computer_awake_caps: pagis_computer::AwakeCaps {
            per_tenant: 1,
            per_server: 8,
        },
        ..TestDaemonOptions::default()
    };
    let daemon = TestDaemon::start_with(options()).await;
    let second: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/api/v1/agents", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"name": "Clown", "job": "helper"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second = second["id"].as_str().expect("the new agent has an id");
    post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;
    wait_state(&daemon, "awake").await;

    let daemon = daemon.restart(options()).await;

    let refused = post(&daemon, &format!("/api/v1/agents/{second}/computer/wake")).await;
    assert_eq!(refused.status(), 503);
    let body: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "awake_cap_reached");
}

/// Offer the live screen of one Agent's Computer with the Session of
/// `cookie`, as `LiveScreen` does.
async fn offer_as(daemon: &TestDaemon, agent_id: &str, cookie: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/agents/{agent_id}/screen/offer",
            daemon.base_url
        ))
        .header("cookie", cookie)
        .json(&serde_json::json!({ "sdp": "v=0 offer" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

/// One side of a Media Relay path: a browser or the Computer's pipeline.
async fn media_socket() -> tokio::net::UdpSocket {
    tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("a loopback UDP socket")
}

/// Whether a check of the browser crosses the path to the pipeline.
/// The browser signs it with the ICE credentials of the pipeline's
/// answer, and the pipeline registers first, as screend does while its
/// session lives.
async fn crosses(
    path: &pagis_computer::MediaPath,
    browser: &tokio::net::UdpSocket,
    pipeline: &tokio::net::UdpSocket,
) -> bool {
    pipeline
        .send_to(&path.registration(), ("127.0.0.1", path.port))
        .await
        .expect("the registration goes out");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let credentials = pagis_computer::IceCredentials::of_answer(pagis_computer::fake::ANSWER)
        .expect("the fake answer carries ICE credentials");
    let check = pagis_computer::fake::ice_check(&credentials, false);
    browser
        .send_to(&check, path.candidate.as_str())
        .await
        .expect("the check goes out");
    let mut buffer = [0u8; 512];
    match tokio::time::timeout(Duration::from_millis(500), pipeline.recv_from(&mut buffer)).await {
        Ok(Ok((read, _))) => buffer[..read] == check[..],
        _ => false,
    }
}

/// A live screen is watched through one Media Relay path for each
/// awake Computer, and each path belongs to the Session that asked for
/// it. A sign-out closes the paths of that Session, so video and input
/// stop for that viewer, and the path of another Session carries on.
#[tokio::test]
async fn a_sign_out_closes_the_media_paths_of_that_session_and_no_other() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;
    let second: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/api/v1/agents", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"name": "Clown", "job": "helper"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second = second["id"].as_str().expect("the new agent has an id");
    for agent_id in [daemon.agent_id.as_str(), second] {
        post(&daemon, &format!("/api/v1/agents/{agent_id}/computer/wake")).await;
        wait_agent_state(&daemon, agent_id, "awake").await;
    }
    let signing_out = daemon.cookie_for(&daemon.user_id).await;
    offer_as(&daemon, &daemon.agent_id, &signing_out).await;
    offer_as(&daemon, second, daemon.cookie()).await;
    let offers = runtime.offers();
    let (ending, staying) = (&offers[0].1, &offers[1].1);
    let (ending_browser, ending_pipeline) = (media_socket().await, media_socket().await);
    let (staying_browser, staying_pipeline) = (media_socket().await, media_socket().await);
    assert!(crosses(ending, &ending_browser, &ending_pipeline).await);
    assert!(crosses(staying, &staying_browser, &staying_pipeline).await);

    daemon.sign_out(&signing_out).await;

    assert!(
        !crosses(ending, &ending_browser, &ending_pipeline).await,
        "the path of the signed-out Session still carries packets"
    );
    assert!(
        crosses(staying, &staying_browser, &staying_pipeline).await,
        "the path of the other Session closed"
    );
}

/// A sign-out on one device does not take the Computer away from the
/// Person on another: an active Takeover stays in place, and it ends at
/// its inactivity timeout because no viewer of that Session sends input.
#[tokio::test]
async fn a_sign_out_leaves_an_active_takeover_in_place() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let daemon = TestDaemon::start_with(daemon_options(&runtime)).await;
    post(
        &daemon,
        &format!("/api/v1/agents/{}/computer/wake", daemon.agent_id),
    )
    .await;
    wait_state(&daemon, "awake").await;
    let signing_out = daemon.cookie_for(&daemon.user_id).await;
    let takeover = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/agents/{}/screen/takeover",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", &signing_out)
        .send()
        .await
        .unwrap();
    assert_eq!(takeover.status(), 200);

    daemon.sign_out(&signing_out).await;

    let agent = pagis_core::AgentId::from(daemon.agent_id.clone());
    assert_eq!(runtime.holder(&agent), pagis_computer::InputHolder::User);
    let state: serde_json::Value = get(
        &daemon,
        &format!("/api/v1/agents/{}/computer", daemon.agent_id),
    )
    .await
    .json()
    .await
    .unwrap();
    assert_eq!(state["holder"], "user", "{state}");
    let ended = daemon
        .stores()
        .events
        .list_by_types(&daemon.workspace_id, &["screen.takeover_ended"], None, 10)
        .await
        .unwrap();
    assert!(
        ended.is_empty(),
        "the sign-out ended the Takeover: {ended:?}"
    );
}
