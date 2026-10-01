//! The deterministic server half of client-only local setup. Native OS
//! package trust stays behind the desktop fixture; this test starts at the
//! authenticated runtime identity and ends at the first model reply.

use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;
use sha2::Sha256;

#[tokio::test]
async fn verified_runtime_finishes_local_setup_and_answers_the_first_message() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::reply(&["Hello from Pixie."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain,
        ..TestDaemonOptions::default()
    })
    .await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let challenge = "a".repeat(64);
    let identity: serde_json::Value = client
        .get(format!(
            "{}/api/v1/runtime/identity?challenge={challenge}",
            daemon.base_url
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(identity["status"], "ok");
    assert_eq!(identity["release"], pagis_server::VERSION);
    assert_eq!(identity["port"], daemon.addr.port());
    assert_eq!(identity["computer_image"], pagis_computer::IMAGE);
    let message = format!(
        "pagis-runtime-identity-v1\0{}\0{}\0{}\0{}\0{}",
        daemon.addr.port(),
        pagis_server::VERSION,
        identity["workspace_id"].as_str().unwrap(),
        pagis_computer::IMAGE,
        challenge,
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(daemon.client_credential().as_bytes()).unwrap();
    mac.update(message.as_bytes());
    assert_eq!(identity["proof"], hex::encode(mac.finalize().into_bytes()));

    let (status, key) = daemon
        .set_up_provider(
            "anthropic",
            "key",
            serde_json::json!({ "api_key": "sk-fixture" }),
        )
        .await;
    assert_eq!(status, 200, "{key}");
    // The model step names the default model; with no list to pick
    // from, the daemon takes the preferred model of the keyed provider.
    let model = client
        .put(format!(
            "{}/api/v1/settings/onboarding/default-model",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "candidate": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(model.status(), StatusCode::NO_CONTENT);

    let complete = client
        .post(format!(
            "{}/api/v1/settings/onboarding/complete",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(complete.status(), StatusCode::NO_CONTENT);

    let sent = client
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": "first", "text": "Hello Pixie" }))
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), StatusCode::CREATED);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let timeline: serde_json::Value = client
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
        if timeline["items"].as_array().unwrap().iter().any(|message| {
            message["author_kind"] == "agent" && message["text_content"] == "Hello from Pixie."
        }) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "first reply did not finish: {timeline}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
