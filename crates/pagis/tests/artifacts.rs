//! Full-daemon artifact tests: multipart upload with dedup and
//! the size cap, byte download through the daemon only, the served
//! type of each download, and the image/file context feed into the
//! model turn.

use std::sync::Arc;
use std::time::Duration;

use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};

fn options(brain: &Arc<ScriptedBrain>) -> TestDaemonOptions {
    TestDaemonOptions {
        brain: Arc::clone(brain) as _,
        ..TestDaemonOptions::default()
    }
}

pub(crate) async fn upload(
    daemon: &TestDaemon,
    filename: &str,
    mime: &str,
    bytes: Vec<u8>,
) -> reqwest::Response {
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(mime)
        .unwrap();
    reqwest::Client::new()
        .post(format!("{}/api/v1/artifacts", daemon.base_url))
        .header("cookie", daemon.cookie())
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .unwrap()
}

async fn send_with_artifacts(
    daemon: &TestDaemon,
    pending_id: &str,
    text: &str,
    artifact_ids: &[&str],
) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "pending_id": pending_id,
            "text": text,
            "artifact_ids": artifact_ids,
        }))
        .send()
        .await
        .unwrap()
}

/// Poll until the agent reply settles in the timeline.
async fn wait_for_agent_reply(daemon: &TestDaemon) -> serde_json::Value {
    for _ in 0..100 {
        let page: serde_json::Value = reqwest::Client::new()
            .get(format!(
                "{}/api/v1/channels/{}/messages",
                daemon.base_url, daemon.dm_channel_id
            ))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let items = page["items"].as_array().unwrap();
        if let Some(reply) = items.iter().find(|m| {
            m["author_kind"] == "agent" && m["status"] == "complete" && !m["run_id"].is_null()
        }) {
            return reply.clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("agent reply did not settle");
}

#[tokio::test]
async fn upload_stores_and_serves_bytes_through_the_daemon() {
    let daemon = TestDaemon::start().await;

    let response = upload(&daemon, "shot.png", "image/png", b"png-bytes".to_vec()).await;

    assert_eq!(response.status(), 201);
    let dto: serde_json::Value = response.json().await.unwrap();
    assert_eq!(dto["filename"], "shot.png");
    assert_eq!(dto["mime"], "image/png");
    assert_eq!(dto["size_bytes"], 9);
    assert_eq!(dto["sha256"].as_str().unwrap().len(), 64);
    let id = dto["id"].as_str().unwrap();

    // The bytes come back through the daemon with the stored mime.
    let download = reqwest::Client::new()
        .get(format!("{}/api/v1/artifacts/{id}", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200);
    assert_eq!(download.headers()["content-type"], "image/png");
    assert_eq!(download.bytes().await.unwrap().as_ref(), b"png-bytes");

    // Without a Session the daemon serves nothing.
    let unauthorized = reqwest::get(format!("{}/api/v1/artifacts/{id}", daemon.base_url))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);

    // An unknown artifact is a JSON 404.
    let missing = reqwest::Client::new()
        .get(format!("{}/api/v1/artifacts/01UNKNOWN", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

/// Upload one file and download it again by its Artifact URL.
async fn round_trip(
    daemon: &TestDaemon,
    filename: &str,
    mime: &str,
    bytes: &[u8],
) -> reqwest::Response {
    let dto: serde_json::Value = upload(daemon, filename, mime, bytes.to_vec())
        .await
        .json()
        .await
        .unwrap();
    let download = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/artifacts/{}",
            daemon.base_url,
            dto["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200, "{filename}");
    download
}

fn header<'a>(response: &'a reqwest::Response, name: &str) -> &'a str {
    response
        .headers()
        .get(name)
        .unwrap_or_else(|| panic!("the download has no {name} header"))
        .to_str()
        .unwrap()
}

/// A file that a browser can run as a document, such as an HTML page
/// or an SVG image with a script, comes back as bytes to save and
/// never as a page at the Product App origin.
#[tokio::test]
async fn an_active_artifact_downloads_as_inert_bytes() {
    let daemon = TestDaemon::start().await;

    for (filename, mime, bytes) in [
        (
            "page.html",
            "text/html",
            b"<!doctype html><script>fetch('/api/v1/requests')</script>".as_slice(),
        ),
        (
            "chart.svg",
            "image/svg+xml",
            b"<svg xmlns='http://www.w3.org/2000/svg'><script>fetch('/api/v1/requests')</script></svg>"
                .as_slice(),
        ),
        ("report.pdf", "application/pdf", b"%PDF-1.7".as_slice()),
    ] {
        let download = round_trip(&daemon, filename, mime, bytes).await;

        assert_eq!(
            header(&download, "content-type"),
            "application/octet-stream",
            "{filename}"
        );
        assert_eq!(
            header(&download, "content-disposition"),
            format!("attachment; filename=\"{filename}\""),
        );
        assert_eq!(header(&download, "x-content-type-options"), "nosniff");
        assert_eq!(
            header(&download, "content-security-policy"),
            "sandbox; default-src 'none'",
            "{filename}"
        );
        assert_eq!(download.bytes().await.unwrap().as_ref(), bytes);
    }
}

/// A passive image, audio, video or plain-text file keeps its type and
/// shows inline, under the same two headers.
#[tokio::test]
async fn a_passive_artifact_keeps_its_type_inline() {
    let daemon = TestDaemon::start().await;

    for (filename, mime) in [
        ("shot.png", "image/png"),
        ("notes.txt", "text/plain"),
        ("call.wav", "audio/wav"),
        ("clip.mp4", "video/mp4"),
    ] {
        let download = round_trip(&daemon, filename, mime, filename.as_bytes()).await;

        assert_eq!(header(&download, "content-type"), mime);
        assert_eq!(
            header(&download, "content-disposition"),
            format!("inline; filename=\"{filename}\""),
        );
        assert_eq!(header(&download, "x-content-type-options"), "nosniff");
        assert_eq!(
            header(&download, "content-security-policy"),
            "sandbox; default-src 'none'",
            "{filename}"
        );
    }
}

/// The served type comes from the allowlist and never from the
/// client: a parameter does not pass through, and a type that is not
/// on the list is an attachment.
#[tokio::test]
async fn the_served_type_is_the_allowlisted_one() {
    let daemon = TestDaemon::start().await;

    for (filename, stored, served) in [
        ("shot.png", "image/png; name=shot", "image/png"),
        ("notes.txt", "text/plain; charset=utf-8", "text/plain"),
        ("page.xml", "video/mp4+xml", "application/octet-stream"),
    ] {
        let download = round_trip(&daemon, filename, stored, filename.as_bytes()).await;

        assert_eq!(header(&download, "content-type"), served, "{stored}");
    }
}

#[tokio::test]
async fn duplicate_content_dedups_to_the_original_artifact() {
    let daemon = TestDaemon::start().await;

    let first = upload(&daemon, "shot.png", "image/png", b"same-bytes".to_vec()).await;
    assert_eq!(first.status(), 201);
    let original: serde_json::Value = first.json().await.unwrap();

    let second = upload(&daemon, "copy.png", "image/png", b"same-bytes".to_vec()).await;
    assert_eq!(second.status(), 200);
    let dedup: serde_json::Value = second.json().await.unwrap();

    assert_eq!(dedup["id"], original["id"]);
    assert_eq!(dedup["filename"], "shot.png");
}

#[tokio::test]
async fn oversize_upload_is_payload_too_large() {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        artifact_max_bytes: 1024,
        ..TestDaemonOptions::default()
    })
    .await;

    let response = upload(&daemon, "big.bin", "application/zip", vec![0u8; 4096]).await;

    assert_eq!(response.status(), 413);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "payload_too_large");
}

#[tokio::test]
async fn an_image_attachment_reaches_the_model_as_an_image_part() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["A red square."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;

    let dto: serde_json::Value = upload(&daemon, "shot.png", "image/png", b"png-bytes".to_vec())
        .await
        .json()
        .await
        .unwrap();
    let artifact_id = dto["id"].as_str().unwrap();

    let sent =
        send_with_artifacts(&daemon, "p1", "What is in this screenshot?", &[artifact_id]).await;
    assert_eq!(sent.status(), 201);
    let message: serde_json::Value = sent.json().await.unwrap();
    assert_eq!(message["blocks"][0]["type"], "markdown");
    assert_eq!(message["blocks"][1]["type"], "image");
    assert_eq!(message["blocks"][1]["artifact_id"], artifact_id);

    let reply = wait_for_agent_reply(&daemon).await;
    assert_eq!(reply["text_content"], "A red square.");

    // The model turn carried the image as a data-URI content part.
    let requests = brain.requests();
    let turn = requests.first().unwrap();
    let user = turn.messages.last().unwrap();
    // The text is the block projection (ADR-0004): the prose, then the
    // image block's alt text.
    assert_eq!(user.text, "User: What is in this screenshot?\n\nshot.png");
    assert_eq!(user.images.len(), 1);
    use base64::Engine as _;
    let expected = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(b"png-bytes")
    );
    assert_eq!(user.images[0], expected);
}

#[tokio::test]
async fn a_non_image_file_is_referenced_but_not_fed() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Noted."]));
    let daemon = TestDaemon::start_with(options(&brain)).await;

    let dto: serde_json::Value = upload(&daemon, "notes.txt", "text/plain", b"secret".to_vec())
        .await
        .json()
        .await
        .unwrap();
    let artifact_id = dto["id"].as_str().unwrap();

    let sent = send_with_artifacts(&daemon, "p1", "Keep this file.", &[artifact_id]).await;
    assert_eq!(sent.status(), 201);
    let message: serde_json::Value = sent.json().await.unwrap();
    assert_eq!(message["blocks"][1]["type"], "file");
    assert_eq!(message["blocks"][1]["name"], "notes.txt");

    wait_for_agent_reply(&daemon).await;

    let requests = brain.requests();
    let user = requests.first().unwrap().messages.last().unwrap();
    assert!(
        user.images.is_empty(),
        "file bytes must not reach the model"
    );
    assert!(
        user.text
            .contains("[file attachment: notes.txt (text/plain)"),
        "{}",
        user.text
    );
    assert!(!user.text.contains("secret"));
}

#[tokio::test]
async fn send_with_an_unknown_artifact_is_rejected() {
    let daemon = TestDaemon::start().await;

    let response = send_with_artifacts(&daemon, "p1", "look", &["01NOSUCHARTIFACT"]).await;

    assert_eq!(response.status(), 422);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "validation");
}
