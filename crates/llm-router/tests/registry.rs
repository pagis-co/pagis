//! Tests for model metadata lookup and cost accounting on responses.

use llm_router::{
    Candidate, ChatRequest, ListedModel, Message, ModelInfo, ModelPrices, ProtocolKind,
    ProviderConfig, Router, RouterConfig, Usage,
};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn info(input_cost: f64, output_cost: f64) -> ModelInfo {
    ModelInfo {
        context_window: 100_000,
        max_output_tokens: 10_000,
        prices: ModelPrices {
            input_cost,
            output_cost,
            cache_read_cost: input_cost / 10.0,
            cache_write_cost: 0.0,
        },
    }
}

#[tokio::test]
async fn chat_response_carries_cost_from_model_info() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "my-model",
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 500,
                "prompt_tokens_details": {"cached_tokens": 200}
            }
        })))
        .mount(&server)
        .await;

    let config = RouterConfig::new()
        .provider(
            "p",
            ProviderConfig::new(ProtocolKind::OpenAiChat, server.uri(), "k"),
        )
        .model("m", [Candidate::new("p", "my-model")])
        .model_info("my-model", info(2.0, 10.0));
    let router = Router::new(config).unwrap();
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let response = router.chat(&req).await.unwrap();

    // 800 fresh * 2.0 + 200 cached * 0.2 + 500 out * 10.0, per million.
    let expected = (800.0 * 2.0 + 200.0 * 0.2 + 500.0 * 10.0) / 1e6;
    assert!((response.cost_usd.unwrap() - expected).abs() < 1e-12);
}

#[tokio::test]
async fn cost_is_none_for_unknown_models() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "mystery",
            "choices": [{
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        })))
        .mount(&server)
        .await;

    let config = RouterConfig::new()
        .provider(
            "p",
            ProviderConfig::new(ProtocolKind::OpenAiChat, server.uri(), "k"),
        )
        .model("m", [Candidate::new("p", "mystery")]);
    let router = Router::new(config).unwrap();
    let req = ChatRequest::new("m", vec![Message::user("hi")]);
    let response = router.chat(&req).await.unwrap();
    assert_eq!(response.cost_usd, None);
}

#[test]
fn builtin_table_resolves_versioned_ids() {
    let router = Router::new(RouterConfig::new()).unwrap();
    let sonnet = router.model_info("claude-sonnet-4-5-20250929").unwrap();
    assert_eq!(sonnet.context_window, 200_000);
    assert_eq!(sonnet.prices.input_cost, 3.0);
    assert!(router.model_info("not-a-model").is_none());

    // Streamed responses price through the same lookup.
    let usage = Usage {
        input_tokens: 1_000_000,
        output_tokens: 0,
        ..Usage::default()
    };
    assert_eq!(router.cost_usd("claude-sonnet-4-5", &usage), Some(3.0));
}

#[test]
fn the_router_layers_the_provider_report_over_its_overrides() {
    let config = RouterConfig::new().model_info("my-model", info(2.0, 10.0));
    let router = Router::new(config).unwrap();
    let listed = ListedModel {
        context_window: Some(1_000_000),
        ..ListedModel::new("my-model")
    };

    let metadata = router.model_metadata("my-model", Some(&listed));

    assert_eq!(metadata.context_window, 1_000_000);
    assert_eq!(metadata.max_output_tokens, 10_000);
    assert_eq!(metadata.prices, Some(info(2.0, 10.0).prices));
    // A listed model no layer prices has an unknown cost.
    let unknown = router.model_metadata("gpt-unlisted", Some(&ListedModel::new("gpt-unlisted")));
    assert_eq!(unknown.prices, None);
    assert_eq!(unknown.context_window, llm_router::DEFAULT_CONTEXT_WINDOW);
}
