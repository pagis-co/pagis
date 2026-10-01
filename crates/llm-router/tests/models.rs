//! Tests for the provider model lists against recorded responses.

use crate::common::single_provider_router;
use llm_router::{
    Error, ListedModel, ModelPrices, ProtocolKind, ProviderConfig, Router, RouterConfig,
};
use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> String {
    let dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    std::fs::read_to_string(dir.join("tests/fixtures/models").join(name)).unwrap()
}

fn json_body(name: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(fixture(name), "application/json")
}

fn ids(models: &[ListedModel]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

#[tokio::test]
async fn openai_lists_ids_newest_first_with_no_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(json_body("openai.json"))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let models = router.list_models("p").await.unwrap();

    assert_eq!(
        ids(&models),
        ["gpt-6-luna", "gpt-5.6", "gpt-5", "text-embedding-3-small"]
    );
    assert!(models.iter().all(|model| model.context_window.is_none()
        && model.max_output_tokens.is_none()
        && model.prices.is_none()));
}

#[tokio::test]
async fn the_responses_protocol_reads_the_same_list() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(json_body("openai.json"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());

    assert_eq!(router.list_models("p").await.unwrap().len(), 4);
}

#[tokio::test]
async fn openrouter_reports_limits_and_per_token_prices() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(json_body("openrouter.json"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());
    let models = router.list_models("p").await.unwrap();

    let sonnet = &models[0];
    assert_eq!(sonnet.id, "anthropic/claude-sonnet-4.6");
    assert_eq!(sonnet.context_window, Some(1_000_000));
    assert_eq!(sonnet.max_output_tokens, Some(64_000));
    let prices = sonnet.prices.unwrap();
    assert!((prices.input_cost - 3.0).abs() < 1e-9);
    assert!((prices.output_cost - 15.0).abs() < 1e-9);
    assert!((prices.cache_read_cost - 0.3).abs() < 1e-9);
    assert!((prices.cache_write_cost - 3.75).abs() < 1e-9);
    // A variable price (-1) is an unknown price, not a negative one.
    let auto = &models[1];
    assert_eq!(auto.id, "openrouter/auto");
    assert_eq!(auto.prices, None::<ModelPrices>);
    assert_eq!(auto.max_output_tokens, None);
}

/// OpenRouter names what each model outputs and the voices of each
/// speech model, so a speech model reads as no chat model and offers its
/// own voices.
#[tokio::test]
async fn openrouter_lists_speech_models_with_their_voices() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(json_body("openrouter.json"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());
    let models = router.list_models("p").await.unwrap();

    let tts = models
        .iter()
        .find(|model| model.id == "google/gemini-3.8-flash-tts")
        .unwrap();
    assert_eq!(
        tts.voices.as_deref(),
        Some(&["Zephyr".to_string(), "Puck".to_string(), "Kore".to_string()][..])
    );
    assert_eq!(
        tts.output_modalities.as_deref(),
        Some(&["speech".to_string()][..])
    );
    assert!(!tts.looks_like_chat());
    let sonnet = &models[0];
    assert_eq!(sonnet.voices, None);
    assert!(sonnet.looks_like_chat());
}

/// Deepgram's public `/models` answers without a key, so it cannot prove
/// one. The list reads the projects of the key first, then the models of
/// its project. A speech model is one Aura generation, whose voices are
/// its models; a transcription model is one Nova generation.
#[tokio::test]
async fn deepgram_lists_the_models_of_the_project_of_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/projects"))
        .and(header("authorization", "Token dg-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "projects": [{ "project_id": "proj-1", "name": "Pagis" }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/projects/proj-1/models"))
        .and(header("authorization", "Token dg-key"))
        .respond_with(json_body("deepgram.json"))
        .expect(1)
        .mount(&server)
        .await;
    let mut provider = ProviderConfig::deepgram("dg-key");
    provider.base_url = server.uri();
    let router = Router::new(RouterConfig::new().provider("deepgram", provider)).unwrap();

    let models = router.list_models("deepgram").await.unwrap();

    assert_eq!(ids(&models), ["nova-3", "nova-2", "aura-2", "aura"]);
    assert_eq!(
        models[0].output_modalities.as_deref(),
        Some(&["transcription".to_string()][..])
    );
    assert_eq!(
        models[2].voices.as_deref(),
        Some(
            &[
                "aura-2-thalia-en".to_string(),
                "aura-2-andromeda-en".to_string()
            ][..]
        )
    );
    assert!(models.iter().all(|model| !model.looks_like_chat()));
}

#[tokio::test]
async fn openrouter_lists_the_models_of_the_key() {
    // OpenRouter's `/models` answers without a key, so it cannot prove
    // one; `/models/user` takes the key. It lists text models alone
    // unless asked for the speech and transcription models too.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models/user"))
        .and(query_param(
            "output_modalities",
            "text,speech,transcription",
        ))
        .and(header("authorization", "Bearer or-key"))
        .respond_with(json_body("openrouter.json"))
        .expect(1)
        .mount(&server)
        .await;
    let mut provider = ProviderConfig::openrouter("or-key");
    provider.base_url = server.uri();
    let router = Router::new(RouterConfig::new().provider("openrouter", provider)).unwrap();

    let models = router.list_models("openrouter").await.unwrap();

    assert_eq!(models[0].id, "anthropic/claude-sonnet-4.6");
}

#[tokio::test]
async fn compatible_servers_report_their_own_limit_fields() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(json_body("groq.json"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let models = router.list_models("p").await.unwrap();

    assert_eq!(models[0].id, "llama-3.3-70b-versatile");
    assert_eq!(models[0].context_window, Some(131_072));
    assert_eq!(models[0].max_output_tokens, Some(32_768));
}

#[tokio::test]
async fn a_local_server_lists_its_pulled_models() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(json_body("ollama.json"))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());

    assert_eq!(
        ids(&router.list_models("p").await.unwrap()),
        ["qwen3:8b", "llama3.2:latest"]
    );
}

#[tokio::test]
async fn anthropic_follows_the_cursor_and_reads_the_limits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("x-api-key", "test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(query_param("limit", "1000"))
        .and(query_param_is_missing("after_id"))
        .respond_with(json_body("anthropic-page-1.json"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(query_param("after_id", "claude-sonnet-4-6"))
        .respond_with(json_body("anthropic-page-2.json"))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let models = router.list_models("p").await.unwrap();

    assert_eq!(
        ids(&models),
        [
            "claude-opus-5",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001"
        ]
    );
    assert_eq!(models[0].context_window, Some(1_000_000));
    assert_eq!(models[0].max_output_tokens, Some(128_000));
    assert_eq!(models[2].context_window, None);
    assert!(models.iter().all(|model| model.prices.is_none()));
}

#[tokio::test]
async fn a_refused_key_returns_the_providers_words() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(401).set_body_raw(
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
            "application/json",
        ))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let error = router.list_models("p").await.unwrap_err();

    assert!(
        matches!(&error, Error::Provider { status: 401, message, .. } if message == "invalid x-api-key"),
        "{error}"
    );
}

#[tokio::test]
async fn a_protocol_without_a_list_endpoint_says_so() {
    let router = single_provider_router(ProtocolKind::Veo, "https://veo.example/v1");

    let error = router.list_models("p").await.unwrap_err();

    assert!(
        matches!(
            error,
            Error::Unsupported {
                feature: "model list",
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn an_unknown_provider_has_no_list() {
    let router = single_provider_router(ProtocolKind::OpenAiChat, "http://localhost:1");

    assert!(matches!(
        router.list_models("nope").await,
        Err(Error::UnknownProvider(_))
    ));
}
