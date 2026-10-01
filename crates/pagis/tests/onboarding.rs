//! Onboarding API tests: status, provider key storage, the key check
//! that lists the provider's models, the one-model default route the
//! person picks, env override,
//! completion persistence across a restart, and the name the shell
//! reads back.

use std::sync::Arc;
use std::time::Duration;

use pagis_agent::RouterBrain;
use pagis_computer::{DockerDiscovery, DockerSearch};
use pagis_core::Provider;
use pagis_testkit::{
    ScriptedBrain, ScriptedDockerPing, TestDaemon, TestDaemonOptions, empty_docker_search,
    test_provider_keys,
};
use reqwest::StatusCode;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn status(daemon: &TestDaemon) -> serde_json::Value {
    let response = client()
        .get(format!("{}/api/v1/settings/onboarding", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

fn provider<'a>(status: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    status["providers"]
        .as_array()
        .expect("providers array")
        .iter()
        .find(|p| p["provider"] == id)
        .expect("provider present")
}

/// The key check of one provider, or `null` when none holds.
fn check_of(status: &serde_json::Value, id: &str) -> serde_json::Value {
    status["checks"]
        .as_array()
        .expect("checks array")
        .iter()
        .find(|check| check["provider"] == id)
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn fresh_daemon_reports_onboarding_needed_and_docker_absent() {
    let daemon = TestDaemon::start().await;

    let status = status(&daemon).await;

    assert_eq!(status["completed"], false);
    assert_eq!(status["docker"]["endpoint"], serde_json::Value::Null);
    assert_eq!(status["docker"]["candidates"], serde_json::json!([]));
    assert_eq!(status["docker_endpoint"], serde_json::Value::Null);
    for id in ["anthropic", "openai", "openrouter"] {
        assert_eq!(provider(&status, id)["configured"], false);
        assert_eq!(provider(&status, id)["source"], serde_json::Value::Null);
    }
}

/// Each provider says what its key does, so the model step can show
/// what a set of keys covers.
#[tokio::test]
async fn each_provider_names_its_uses() {
    let daemon = TestDaemon::start().await;

    let status = status(&daemon).await;

    assert_eq!(
        provider(&status, "openai")["uses"],
        serde_json::json!(["thinking", "spoken_replies", "dictation", "calls"])
    );
    assert_eq!(
        provider(&status, "openrouter")["uses"],
        serde_json::json!(["thinking", "spoken_replies", "dictation"])
    );
    assert_eq!(
        provider(&status, "anthropic")["uses"],
        serde_json::json!(["thinking"])
    );
}

/// A key stored at onboarding gives each alias that no keyed provider
/// serves the preferred models of a provider that serves it. A route that
/// a keyed provider serves stays.
#[tokio::test]
async fn a_stored_key_routes_each_alias_no_keyed_provider_serves() {
    let daemon = TestDaemon::start().await;

    store_key(&daemon, "openrouter").await;

    let transcribe = model_alias(&daemon, "transcribe").await;
    assert_eq!(
        transcribe["candidates"],
        serde_json::json!(["openrouter/openai/gpt-4o-transcribe"])
    );
    assert_eq!(transcribe["reachable"], true);
    assert_eq!(
        model_alias(&daemon, "speak").await["candidates"],
        serde_json::json!(["openrouter/google/gemini-3.8-flash-tts"])
    );
    assert_eq!(model_alias(&daemon, "phone").await["reachable"], false);
    assert_eq!(model_alias(&daemon, "default").await["reachable"], true);

    store_key(&daemon, "openai").await;

    assert_eq!(
        model_alias(&daemon, "transcribe").await["candidates"],
        serde_json::json!(["openrouter/openai/gpt-4o-transcribe"])
    );
    assert_eq!(model_alias(&daemon, "phone").await["reachable"], true);
}

/// A home with one Colima socket, which a ping never opens.
fn colima_home() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join(".colima").join("default");
    std::fs::create_dir_all(&socket).unwrap();
    std::fs::write(socket.join("docker.sock"), "").unwrap();
    let endpoint = format!("unix://{}", socket.join("docker.sock").display());
    (dir, endpoint)
}

/// A daemon whose Docker discovery searches `home` and answers for the
/// endpoints given.
async fn daemon_searching(home: &tempfile::TempDir, answering: &[String]) -> TestDaemon {
    TestDaemon::start_with(TestDaemonOptions {
        docker_discovery: Arc::new(DockerDiscovery::new(
            DockerSearch {
                home: home.path().to_path_buf(),
                ..empty_docker_search()
            },
            Arc::new(ScriptedDockerPing(answering.to_vec())),
            None,
        )),
        ..TestDaemonOptions::default()
    })
    .await
}

#[tokio::test]
async fn docker_discovery_shows_in_status() {
    let (home, endpoint) = colima_home();
    let daemon = daemon_searching(&home, std::slice::from_ref(&endpoint)).await;

    let status = status(&daemon).await;

    assert_eq!(status["docker"]["endpoint"], endpoint);
    assert_eq!(status["docker"]["candidates"][0]["source"], "colima");
    assert_eq!(status["docker"]["candidates"][0]["reachable"], true);
}

/// The computer step saves the Docker endpoint the person typed, after
/// Docker answered at it, and keeps every other System Setting.
#[tokio::test]
async fn the_computer_step_saves_an_endpoint_that_answers() {
    let (home, endpoint) = colima_home();
    let daemon = daemon_searching(&home, std::slice::from_ref(&endpoint)).await;

    let response = client()
        .put(format!(
            "{}/api/v1/settings/onboarding/docker-endpoint",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "docker_endpoint": endpoint }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(status(&daemon).await["docker_endpoint"], endpoint);
    let file = std::fs::read_to_string(daemon.booted.home.join("config.toml")).unwrap();
    assert!(file.contains(&endpoint), "{file}");
    assert!(file.contains("port = 4400"), "{file}");
}

#[tokio::test]
async fn the_computer_step_refuses_an_endpoint_that_does_not_answer() {
    let daemon = TestDaemon::start().await;

    let response = client()
        .put(format!(
            "{}/api/v1/settings/onboarding/docker-endpoint",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "docker_endpoint": "tcp://127.0.0.1:1" }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        status(&daemon).await["docker_endpoint"],
        serde_json::Value::Null
    );
}

/// The first run is the one time the product port writes an
/// installation setting. Once the Workspace finished onboarding, both
/// writes answer `409` and the Administration Interface changes them.
#[tokio::test]
async fn the_first_run_writes_close_when_onboarding_completes() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    verify_model(&daemon, "anthropic").await;
    complete(&daemon, serde_json::json!({})).await;

    for (path, body) in [
        (
            "settings/onboarding/providers/anthropic/key",
            serde_json::json!({ "key": "sk-later" }),
        ),
        (
            "settings/onboarding/docker-endpoint",
            serde_json::json!({ "docker_endpoint": null }),
        ),
        (
            "settings/onboarding/default-model",
            serde_json::json!({ "candidate": null }),
        ),
    ] {
        let response = client()
            .put(format!("{}/api/v1/{path}", daemon.base_url))
            .header("cookie", daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT, "{path}");
    }
    let response = check_typed_key(&daemon, "anthropic", "sk-test").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn storing_a_key_marks_the_provider_configured_from_the_secret_file() {
    let daemon = TestDaemon::start().await;

    let response = client()
        .put(format!(
            "{}/api/v1/settings/onboarding/providers/anthropic/key",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "key": "sk-wizard" }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["provider"], "anthropic");
    assert_eq!(body["configured"], true);
    assert_eq!(body["source"], "secret_file");

    let status = status(&daemon).await;
    assert_eq!(provider(&status, "anthropic")["source"], "secret_file");
    assert_eq!(provider(&status, "openai")["configured"], false);
}

#[tokio::test]
async fn the_env_key_wins_over_a_stored_key() {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        keys: test_provider_keys(vec![("ANTHROPIC_API_KEY", "sk-env")]),
        ..TestDaemonOptions::default()
    })
    .await;

    let response = client()
        .put(format!(
            "{}/api/v1/settings/onboarding/providers/anthropic/key",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "key": "sk-wizard" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let status = status(&daemon).await;
    assert_eq!(provider(&status, "anthropic")["source"], "env");
}

#[tokio::test]
async fn unknown_provider_and_empty_key_are_rejected() {
    let daemon = TestDaemon::start().await;

    let unknown = client()
        .put(format!(
            "{}/api/v1/settings/onboarding/providers/no-such/key",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "key": "sk" }))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let empty = client()
        .put(format!(
            "{}/api/v1/settings/onboarding/providers/anthropic/key",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "key": "  " }))
        .send()
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn completion_requires_a_stored_key() {
    let daemon = TestDaemon::start().await;
    assert_eq!(status(&daemon).await["completed"], false);

    let response = client()
        .post(format!(
            "{}/api/v1/settings/onboarding/complete",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body["error"]["message"],
        "add a model key before you finish setup"
    );
    assert_eq!(status(&daemon).await["completed"], false);
}

#[tokio::test]
async fn a_stored_key_completes_setup_without_a_check() {
    let daemon = TestDaemon::start().await;
    store_key(&daemon, "anthropic").await;

    complete(&daemon, serde_json::json!({})).await;

    let status = status(&daemon).await;
    assert_eq!(status["completed"], true);
    assert_eq!(status["checks"], serde_json::json!([]));
}

#[tokio::test]
async fn settings_require_a_session() {
    let daemon = TestDaemon::start().await;

    for (method, path) in [
        (reqwest::Method::GET, "settings/onboarding"),
        (reqwest::Method::POST, "settings/onboarding/complete"),
        (
            reqwest::Method::PUT,
            "settings/onboarding/providers/anthropic/key",
        ),
        (reqwest::Method::PUT, "settings/onboarding/docker-endpoint"),
        (reqwest::Method::POST, "settings/providers/anthropic/check"),
    ] {
        let response = client()
            .request(method.clone(), format!("{}/api/v1/{path}", daemon.base_url))
            .json(&serde_json::json!({ "key": "sk" }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }
}

async fn user(daemon: &TestDaemon) -> serde_json::Value {
    let response = client()
        .get(format!("{}/api/v1/user", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

async fn complete(daemon: &TestDaemon, body: serde_json::Value) {
    let response = client()
        .post(format!(
            "{}/api/v1/settings/onboarding/complete",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn the_name_the_wizard_records_is_the_name_the_shell_reads() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    assert_eq!(user(&daemon).await["name"], serde_json::Value::Null);
    verify_model(&daemon, "anthropic").await;

    complete(&daemon, serde_json::json!({ "user_name": "Ada" })).await;

    assert_eq!(user(&daemon).await["name"], "Ada");
    // The name outlives the process that took it.
    let daemon = daemon.restart(TestDaemonOptions::default()).await;
    assert_eq!(user(&daemon).await["name"], "Ada");
}

#[tokio::test]
async fn a_wizard_the_user_finishes_without_a_name_leaves_none() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    verify_model(&daemon, "anthropic").await;

    complete(&daemon, serde_json::json!({ "user_name": "  " })).await;

    assert_eq!(user(&daemon).await["name"], serde_json::Value::Null);
}

// ---- The key check and the default model ----

/// The models every fake provider lists, newest first. The first one is
/// in no built-in table: it ships after the release.
const LISTED: [&str; 2] = ["vendor-new-model", "vendor-older-model"];

fn listed_models() -> serde_json::Value {
    serde_json::json!({
        "data": [
            {"id": LISTED[0], "created": 2, "context_length": 400_000,
             "pricing": {"prompt": "0.000002", "completion": "0.000008"}},
            {"id": LISTED[1], "created": 1},
        ],
        "has_more": false,
    })
}

/// A fake provider that serves the model list of every protocol:
/// `/models` (Anthropic, OpenAI) and `/models/user` (OpenRouter), for
/// the key `sk-test` alone.
async fn listing_provider() -> MockServer {
    let provider = MockServer::start().await;
    for (list_path, auth, value) in [
        ("/models", "x-api-key", "sk-test"),
        ("/models", "authorization", "Bearer sk-test"),
        ("/models/user", "authorization", "Bearer sk-test"),
    ] {
        Mock::given(method("GET"))
            .and(path(list_path))
            .and(header(auth, value))
            .respond_with(ResponseTemplate::new(200).set_body_json(listed_models()))
            .mount(&provider)
            .await;
    }
    provider
}

/// Serve one provider's model list for the key `sk-test`. `models` are
/// ids with their release order, newest first.
async fn serve_list(server: &MockServer, provider: Provider, models: &[(&str, u64)]) {
    let (list_path, auth, value) = match provider {
        Provider::Anthropic => ("/models", "x-api-key", "sk-test"),
        Provider::OpenAi => ("/models", "authorization", "Bearer sk-test"),
        Provider::OpenRouter => ("/models/user", "authorization", "Bearer sk-test"),
        Provider::Deepgram => panic!("Deepgram lists its projects first: use serve_deepgram"),
    };
    let data: Vec<serde_json::Value> = models
        .iter()
        .map(|(id, created)| serde_json::json!({ "id": id, "created": created }))
        .collect();
    Mock::given(method("GET"))
        .and(path(list_path))
        .and(header(auth, value))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "data": data, "has_more": false })),
        )
        .mount(server)
        .await;
}

/// Serve Deepgram's list for the key `sk-test`: one project, whose
/// models are Nova-3 and the Aura-2 voices Thalia and Andromeda.
async fn serve_deepgram(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/projects"))
        .and(header("authorization", "Token sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "projects": [{ "project_id": "proj-1", "name": "Pagis" }]
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/projects/proj-1/models"))
        .and(header("authorization", "Token sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "stt": [{ "canonical_name": "nova-3-general", "architecture": "nova-3" }],
            "tts": [
                { "canonical_name": "aura-2-thalia-en", "architecture": "aura-2" },
                { "canonical_name": "aura-2-andromeda-en", "architecture": "aura-2" }
            ]
        })))
        .mount(server)
        .await;
}

async fn daemon_listing(provider: &MockServer) -> TestDaemon {
    TestDaemon::start_with(TestDaemonOptions {
        model_list_base_url: Some(provider.uri()),
        ..TestDaemonOptions::default()
    })
    .await
}

async fn check_model(daemon: &TestDaemon, provider: &str) -> reqwest::Response {
    client()
        .post(format!(
            "{}/api/v1/settings/providers/{provider}/check",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
}

async fn check_typed_key(daemon: &TestDaemon, provider: &str, key: &str) -> reqwest::Response {
    client()
        .post(format!(
            "{}/api/v1/settings/onboarding/providers/{provider}/key/check",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "key": key }))
        .send()
        .await
        .unwrap()
}

async fn verify_model(daemon: &TestDaemon, provider: &str) {
    store_key(daemon, provider).await;
    let response = check_model(daemon, provider).await;
    assert_eq!(response.status(), StatusCode::OK);
}

async fn store_key(daemon: &TestDaemon, provider: &str) {
    store_key_as(daemon, provider, "sk-test").await;
}

async fn store_key_as(daemon: &TestDaemon, provider: &str, key: &str) {
    let response = client()
        .put(format!(
            "{}/api/v1/settings/onboarding/providers/{provider}/key",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "key": key }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// The model step's pick: one `provider/model` candidate, or `None` for
/// the daemon's preselection.
async fn pick_default_model(daemon: &TestDaemon, candidate: Option<&str>) -> reqwest::Response {
    client()
        .put(format!(
            "{}/api/v1/settings/onboarding/default-model",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "candidate": candidate }))
        .send()
        .await
        .unwrap()
}

async fn model_alias(daemon: &TestDaemon, name: &str) -> serde_json::Value {
    let aliases: serde_json::Value = client()
        .get(format!("{}/api/v1/settings/model-aliases", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    aliases["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|alias| alias["alias"] == name)
        .unwrap_or_else(|| panic!("the {name} alias"))
        .clone()
}

async fn default_route(daemon: &TestDaemon) -> serde_json::Value {
    model_alias(daemon, "default").await["candidates"].clone()
}

async fn model_lists(daemon: &TestDaemon) -> serde_json::Value {
    client()
        .get(format!("{}/api/v1/settings/models", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// The key check is one list call: it counts the listed models, and it
/// asks no model for anything, so it costs nothing.
#[tokio::test]
async fn the_key_check_lists_the_models_and_generates_nothing() {
    let provider = listing_provider().await;
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        model_list_base_url: Some(provider.uri()),
        ..TestDaemonOptions::default()
    })
    .await;
    store_key(&daemon, "anthropic").await;

    let response = check_model(&daemon, "anthropic").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["provider"], "anthropic");
    assert_eq!(body["available"], 2);
    assert!(brain.requests().is_empty(), "the check asked the brain");
    let requests = provider.received_requests().await.unwrap();
    assert!(!requests.is_empty());
    for request in requests {
        assert_eq!(request.method.as_str(), "GET", "{}", request.url);
        assert_eq!(request.url.path(), "/models", "{}", request.url);
    }
    assert_eq!(
        check_of(&status(&daemon).await, "anthropic")["available"],
        2
    );
}

/// The check does not compare the list with any model name: a list that
/// names none of the seeded models still proves the key.
#[tokio::test]
async fn the_key_check_passes_on_any_list() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "openai").await;

    let response = check_model(&daemon, "openai").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["available"], 2);
    assert!(body.get("missing").is_none(), "{body}");
}

#[tokio::test]
async fn a_refused_key_fails_with_the_providers_words() {
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "type": "error",
            "error": {"type": "authentication_error", "message": "invalid x-api-key"}
        })))
        .mount(&provider)
        .await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "anthropic").await;

    let response = check_model(&daemon, "anthropic").await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("invalid x-api-key"),
        "{body}"
    );
    assert_eq!(status(&daemon).await["checks"], serde_json::json!([]));
}

#[tokio::test]
async fn a_forbidden_key_fails_with_the_providers_words() {
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "error": {"message": "this key has no access to the models", "code": "forbidden"}
        })))
        .mount(&provider)
        .await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "openai").await;

    let response = check_model(&daemon, "openai").await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("this key has no access to the models"),
        "{body}"
    );
}

/// A typed key that the provider answers is stored, and the check that
/// proved it holds.
#[tokio::test]
async fn a_typed_key_that_the_provider_answers_is_stored_and_checked() {
    let fake = listing_provider().await;
    let daemon = daemon_listing(&fake).await;

    let response = check_typed_key(&daemon, "anthropic", "sk-test").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["available"], 2);
    let status = status(&daemon).await;
    assert_eq!(provider(&status, "anthropic")["configured"], true);
    assert_eq!(check_of(&status, "anthropic")["available"], 2);
}

/// A typed key that the provider refuses is not stored, so nothing says
/// Pagis holds a key and setup cannot finish on it.
#[tokio::test]
async fn a_typed_key_that_the_provider_refuses_is_not_stored() {
    let fake = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "type": "error",
            "error": {"type": "authentication_error", "message": "invalid x-api-key"}
        })))
        .mount(&fake)
        .await;
    let daemon = daemon_listing(&fake).await;

    let response = check_typed_key(&daemon, "anthropic", "sk-wrong").await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("invalid x-api-key"),
        "{body}"
    );
    let status = status(&daemon).await;
    assert_eq!(provider(&status, "anthropic")["configured"], false);
    assert_eq!(status["checks"], serde_json::json!([]));
}

/// A refused key does not replace the key that the installation holds.
#[tokio::test]
async fn a_refused_typed_key_keeps_the_stored_key() {
    let fake = listing_provider().await;
    let daemon = daemon_listing(&fake).await;
    verify_model(&daemon, "anthropic").await;

    let response = check_typed_key(&daemon, "anthropic", "sk-wrong").await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        check_model(&daemon, "anthropic").await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_checked_key_allows_completion_and_survives_a_restart() {
    let provider = listing_provider().await;
    let keys = test_provider_keys(Vec::new());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        keys: Arc::clone(&keys),
        model_list_base_url: Some(provider.uri()),
        ..TestDaemonOptions::default()
    })
    .await;
    verify_model(&daemon, "anthropic").await;
    assert_eq!(
        check_of(&status(&daemon).await, "anthropic")["available"],
        2
    );

    complete(&daemon, serde_json::json!({})).await;

    let daemon = daemon
        .restart(TestDaemonOptions {
            keys,
            model_list_base_url: Some(provider.uri()),
            ..TestDaemonOptions::default()
        })
        .await;
    assert_eq!(status(&daemon).await["completed"], true);
    assert_eq!(
        check_of(&status(&daemon).await, "anthropic")["available"],
        2
    );
}

#[tokio::test]
async fn a_new_key_clears_the_check() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    verify_model(&daemon, "anthropic").await;

    store_key_as(&daemon, "anthropic", "sk-replaced").await;

    assert_eq!(status(&daemon).await["checks"], serde_json::json!([]));
}

/// The check proves the key, not a route, so a new route keeps it.
#[tokio::test]
async fn changing_the_default_route_keeps_the_check() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    verify_model(&daemon, "anthropic").await;

    let response = client()
        .put(format!(
            "{}/api/v1/settings/model-aliases/default",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "candidates": ["openai/gpt-5.6"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        check_of(&status(&daemon).await, "anthropic")["available"],
        2
    );
}

#[tokio::test]
async fn a_provider_without_a_key_cannot_be_checked() {
    let daemon = TestDaemon::start().await;

    let response = check_model(&daemon, "anthropic").await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no key is configured"),
        "{body}"
    );
}

#[tokio::test]
async fn an_unknown_provider_has_no_check() {
    let daemon = TestDaemon::start().await;

    assert_eq!(
        check_model(&daemon, "no-such").await.status(),
        StatusCode::NOT_FOUND
    );
}

/// OpenRouter's public `/models` answers any key, so the check reads
/// `/models/user`, which only the key's owner can read.
#[tokio::test]
async fn openrouter_is_checked_on_the_list_of_the_key() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "openrouter").await;

    let response = check_model(&daemon, "openrouter").await;

    assert_eq!(response.status(), StatusCode::OK);
    let requests = provider.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| request.url.path() == "/models/user")
    );
}

/// The Models settings read every provider's list, newest first, with
/// the layered metadata and an unknown price as `null`.
#[tokio::test]
async fn the_models_settings_read_the_provider_lists() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "openrouter").await;

    let lists: serde_json::Value = client()
        .get(format!("{}/api/v1/settings/models", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let providers = lists["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1, "{lists}");
    assert_eq!(providers[0]["provider"], "openrouter");
    let models = providers[0]["models"].as_array().unwrap();
    assert_eq!(models[0]["candidate"], "openrouter/vendor-new-model");
    assert_eq!(models[0]["context_window"], 400_000);
    assert_eq!(models[0]["input_cost"], 2.0);
    assert_eq!(models[1]["candidate"], "openrouter/vendor-older-model");
    assert_eq!(models[1]["input_cost"], serde_json::Value::Null);
    assert_eq!(
        models[1]["context_window"],
        llm_router::DEFAULT_CONTEXT_WINDOW
    );
}

#[tokio::test]
async fn a_failed_list_names_the_providers_words() {
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {"message": "invalid api key"}
        })))
        .mount(&provider)
        .await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "openai").await;

    let lists: serde_json::Value = client()
        .get(format!("{}/api/v1/settings/models", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(lists["providers"][0]["models"], serde_json::json!([]));
    assert!(
        lists["providers"][0]["error"]
            .as_str()
            .unwrap()
            .contains("invalid api key")
    );
}

/// Before anybody picks, the seed names the first preferred model and
/// no fallback on another provider.
#[tokio::test]
async fn the_seeded_default_route_is_one_model() {
    let daemon = TestDaemon::start().await;

    assert_eq!(
        default_route(&daemon).await,
        serde_json::json!(["openai/gpt-6-luna"])
    );
}

/// The pick replaces the whole default route with the one model.
#[tokio::test]
async fn the_picked_model_is_the_whole_default_route() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "openai").await;

    let response = pick_default_model(&daemon, Some("openai/vendor-older-model")).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        default_route(&daemon).await,
        serde_json::json!(["openai/vendor-older-model"])
    );
}

/// With no pick, the default route takes the preselection: a list that
/// does not name the preferred model gives its first model, which is
/// its newest.
#[tokio::test]
async fn no_pick_takes_the_newest_listed_model() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "anthropic").await;

    let response = pick_default_model(&daemon, None).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        default_route(&daemon).await,
        serde_json::json!(["anthropic/vendor-new-model"])
    );
}

/// With no pick, a list that names the preferred model gives it, even
/// when the provider lists a newer model.
#[tokio::test]
async fn no_pick_takes_the_preferred_model_the_provider_lists() {
    let provider = MockServer::start().await;
    serve_list(
        &provider,
        Provider::OpenAi,
        &[("vendor-new-model", 2), ("gpt-6-luna", 1)],
    )
    .await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "openai").await;

    let response = pick_default_model(&daemon, None).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        default_route(&daemon).await,
        serde_json::json!(["openai/gpt-6-luna"])
    );
}

/// A provider whose list is not available takes its preferred model.
#[tokio::test]
async fn no_list_takes_the_preferred_model() {
    let daemon = TestDaemon::start().await;
    store_key(&daemon, "openrouter").await;

    let response = pick_default_model(&daemon, None).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        default_route(&daemon).await,
        serde_json::json!(["openrouter/openai/gpt-6-luna"])
    );
}

/// With keys for several providers and no pick, the default route takes
/// the first preferred model that a keyed provider lists, and the Models
/// settings name it as the preselection.
#[tokio::test]
async fn no_pick_with_several_keys_takes_the_first_preferred_model_a_provider_lists() {
    let provider = MockServer::start().await;
    serve_list(
        &provider,
        Provider::OpenRouter,
        &[("qwen/qwen3.8-27b:free", 2), ("openai/gpt-6-luna", 1)],
    )
    .await;
    serve_list(&provider, Provider::Anthropic, &[("claude-sonnet-5-5", 1)]).await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "anthropic").await;
    store_key(&daemon, "openrouter").await;
    assert_eq!(
        model_lists(&daemon).await["preselected"],
        "openrouter/openai/gpt-6-luna"
    );

    let response = pick_default_model(&daemon, None).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        default_route(&daemon).await,
        serde_json::json!(["openrouter/openai/gpt-6-luna"])
    );
}

#[tokio::test]
async fn no_pick_without_a_key_is_refused() {
    let daemon = TestDaemon::start().await;

    let response = pick_default_model(&daemon, None).await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_pick_names_a_provider_and_a_model() {
    let daemon = TestDaemon::start().await;
    store_key(&daemon, "openai").await;

    let response = pick_default_model(&daemon, Some("gpt-6-luna")).await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// The model step checks each key it takes, and each check holds on its
/// own.
#[tokio::test]
async fn each_provider_keeps_its_own_check() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    verify_model(&daemon, "anthropic").await;
    verify_model(&daemon, "openai").await;

    let status = status(&daemon).await;

    assert_eq!(check_of(&status, "anthropic")["available"], 2);
    assert_eq!(check_of(&status, "openai")["available"], 2);
}

/// An alias is reachable when a candidate's provider holds a key and
/// serves the alias's use, so the model step and the Models settings can
/// name the key it needs.
#[tokio::test]
async fn an_alias_is_reachable_once_a_provider_that_serves_it_holds_a_key() {
    let daemon = TestDaemon::start().await;
    store_key(&daemon, "anthropic").await;
    assert_eq!(model_alias(&daemon, "speak").await["reachable"], false);

    let response = client()
        .put(format!(
            "{}/api/v1/settings/model-aliases/speak",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "candidates": ["anthropic/claude-sonnet-5-5"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(model_alias(&daemon, "speak").await["reachable"], false);

    store_key(&daemon, "openai").await;
    let response = client()
        .put(format!(
            "{}/api/v1/settings/model-aliases/speak",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "candidates": ["openai/gpt-4o-mini-tts"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(model_alias(&daemon, "speak").await["reachable"], true);
    assert_eq!(model_alias(&daemon, "default").await["reachable"], true);
}

/// A key that leaves the default route on no keyed provider routes it
/// to the first preferred model that a keyed provider lists. A later
/// provider's preferred model wins over an earlier provider's newest
/// model.
#[tokio::test]
async fn a_new_key_routes_to_the_first_preferred_model_a_provider_lists() {
    let provider = MockServer::start().await;
    serve_list(
        &provider,
        Provider::OpenRouter,
        &[("qwen/qwen3.8-27b:free", 1)],
    )
    .await;
    serve_list(
        &provider,
        Provider::Anthropic,
        &[("vendor-new-model", 2), ("claude-sonnet-5-5", 1)],
    )
    .await;
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        keys: test_provider_keys(vec![("OPENROUTER_API_KEY", "sk-test")]),
        model_list_base_url: Some(provider.uri()),
        ..TestDaemonOptions::default()
    })
    .await;

    let (status, setup) = daemon
        .set_up_provider(
            "anthropic",
            "key",
            serde_json::json!({ "api_key": "sk-test" }),
        )
        .await;

    assert_eq!(status, 200, "{setup}");
    assert_eq!(
        default_route(&daemon).await,
        serde_json::json!(["anthropic/claude-sonnet-5-5"])
    );
}

#[tokio::test]
async fn a_provider_without_a_key_cannot_be_picked() {
    let daemon = TestDaemon::start().await;

    let response = pick_default_model(&daemon, Some("openai/gpt-5.6")).await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// A person an Administrator creates answers no model question. When
/// no keyed provider serves the Administrator's route, their default
/// route is the preselection of the first provider with a key.
#[tokio::test]
async fn a_created_person_starts_on_the_newest_listed_model() {
    let provider = listing_provider().await;
    let daemon = daemon_listing(&provider).await;
    store_key(&daemon, "anthropic").await;

    let created: serde_json::Value = client()
        .post(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "name": "Grace",
            "password": "correct horse battery",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let workspace_id = pagis_core::WorkspaceId::from(
        created["workspace_id"]
            .as_str()
            .unwrap_or_else(|| panic!("the person has a Workspace: {created}"))
            .to_string(),
    );

    let alias = daemon
        .stores()
        .model_aliases
        .get_by_alias(&workspace_id, "default")
        .await
        .unwrap()
        .expect("the default alias");
    assert_eq!(alias.candidates, vec!["anthropic/vendor-new-model"]);
}

/// A Deepgram key speaks and transcribes. Dictation and spoken replies
/// prefer it, and its voices are the Aura voices its project lists.
#[tokio::test]
async fn a_deepgram_key_serves_dictation_and_spoken_replies_with_its_voices() {
    let server = MockServer::start().await;
    serve_deepgram(&server).await;
    let daemon = daemon_listing(&server).await;

    store_key(&daemon, "deepgram").await;

    assert_eq!(
        model_alias(&daemon, "transcribe").await["candidates"],
        serde_json::json!(["deepgram/nova-3"])
    );
    assert_eq!(
        model_alias(&daemon, "speak").await["candidates"],
        serde_json::json!(["deepgram/aura-2"])
    );
    let voices: serde_json::Value = client()
        .get(format!("{}/api/v1/settings/voices", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(voices["provider"], "deepgram");
    assert_eq!(voices["model"], "aura-2");
    assert_eq!(
        voices["items"],
        serde_json::json!(["aura-2-thalia-en", "aura-2-andromeda-en"])
    );
    let status = status(&daemon).await;
    assert_eq!(
        provider(&status, "deepgram")["uses"],
        serde_json::json!(["spoken_replies", "dictation"])
    );
}

/// The keys a person types at onboarding are stored one at a time, so a
/// route can land on the first key that serves it. The pick of the model
/// ends the step and gives each voice and call alias the provider its
/// preference names first among all the keys.
#[tokio::test]
async fn the_onboarding_pick_gives_each_voice_alias_its_preferred_keyed_provider() {
    let daemon = TestDaemon::start().await;
    store_key(&daemon, "openai").await;
    store_key(&daemon, "deepgram").await;
    assert_eq!(
        model_alias(&daemon, "transcribe").await["candidates"],
        serde_json::json!(["openai/gpt-4o-transcribe"])
    );

    let response = pick_default_model(&daemon, None).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        model_alias(&daemon, "transcribe").await["candidates"],
        serde_json::json!(["deepgram/nova-3"])
    );
    assert_eq!(
        model_alias(&daemon, "speak").await["candidates"],
        serde_json::json!(["deepgram/aura-2"])
    );
    assert_eq!(
        model_alias(&daemon, "phone").await["candidates"],
        serde_json::json!(["openai/gpt-live-1", "openai/gpt-realtime-2.1"])
    );
}

/// A key from the environment comes with no key route, so the boot gives
/// each voice and call alias that no keyed provider serves the preferred
/// models of a provider that serves it.
#[tokio::test]
async fn the_boot_routes_the_voice_aliases_to_the_keys_of_the_environment() {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        keys: test_provider_keys(vec![("OPENROUTER_API_KEY", "sk-test")]),
        ..TestDaemonOptions::default()
    })
    .await;

    assert_eq!(
        model_alias(&daemon, "transcribe").await["candidates"],
        serde_json::json!(["openrouter/openai/gpt-4o-transcribe"])
    );
    assert_eq!(
        model_alias(&daemon, "speak").await["candidates"],
        serde_json::json!(["openrouter/google/gemini-3.8-flash-tts"])
    );
}

/// A Person an Administrator creates gets voice aliases that the keys of
/// the installation serve.
#[tokio::test]
async fn a_created_person_gets_voice_aliases_the_keys_serve() {
    let daemon = TestDaemon::start().await;
    store_key(&daemon, "openrouter").await;

    let created: serde_json::Value = client()
        .post(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "email": "lin@example.com",
            "name": "Lin",
            "password": "correct horse battery",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let workspace_id = pagis_core::WorkspaceId::from(
        created["workspace_id"]
            .as_str()
            .unwrap_or_else(|| panic!("the person has a Workspace: {created}"))
            .to_string(),
    );

    let alias = daemon
        .stores()
        .model_aliases
        .get_by_alias(&workspace_id, "transcribe")
        .await
        .unwrap()
        .expect("the transcribe alias");
    assert_eq!(
        alias.candidates,
        vec!["openrouter/openai/gpt-4o-transcribe"]
    );
}

/// A model that the provider lists and no built-in table knows runs end
/// to end: the check lists it, the pick names it, and the seeded sprite
/// answers on it with the conservative default budget.
#[tokio::test]
async fn a_listed_model_no_table_knows_runs_end_to_end() {
    let provider = listing_provider().await;
    let sse = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"ready\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&provider)
        .await;
    let keys = test_provider_keys(Vec::new());
    let models = Arc::new(
        pagis_agent::ModelCatalog::new(Arc::clone(&keys))
            .with_base_url(Provider::OpenRouter, provider.uri()),
    );
    let brain = Arc::new(
        RouterBrain::new(Arc::clone(&keys), models)
            .with_base_url(Provider::OpenRouter, provider.uri()),
    );
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain,
        keys,
        model_list_base_url: Some(provider.uri()),
        ..TestDaemonOptions::default()
    })
    .await;
    verify_model(&daemon, "openrouter").await;
    let picked = pick_default_model(&daemon, None).await;
    assert_eq!(picked.status(), StatusCode::NO_CONTENT);
    complete(&daemon, serde_json::json!({})).await;
    assert!(llm_router::model_info(LISTED[0]).is_none());

    let response = client()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "pending_id": "listed-first", "text": "Hello Pixie" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let timeline: serde_json::Value = client()
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
        if timeline["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["author_kind"] == "agent" && message["text_content"] == "ready")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Pixie did not answer: {timeline}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let generations: Vec<_> = provider
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method.as_str() == "POST")
        .collect();
    assert!(!generations.is_empty());
    for request in &generations {
        let body: serde_json::Value = request.body_json().unwrap();
        assert_eq!(body["model"], LISTED[0]);
    }
}
