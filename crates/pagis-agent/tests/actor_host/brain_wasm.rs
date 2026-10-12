//! The Brain keeps its router and fetch futures on the host's thread.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use pagis_agent::{Brain, ModelCatalog, RouterBrain, TurnDelta, TurnMessage, TurnRequest};
use pagis_core::{MemorySecretStore, ModelEndUserKey, Provider, ProviderKeys, UserId};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
let savedFetch;
let calls;
export function installFetch(body) {
    savedFetch = globalThis.fetch;
    calls = 0;
    globalThis.fetch = async (request) => {
        if (request.url !== "https://provider.test/messages") {
            throw new Error("unexpected provider URL");
        }
        if (request.headers.get("x-api-key") !== "test-key") {
            throw new Error("the provider key is absent");
        }
        calls += 1;
        await new Promise(resolve => setTimeout(resolve, 5));
        const response = new Response(body, {
            status: 200,
            headers: { "content-type": "text/event-stream" },
        });
        Object.defineProperty(response, "url", { value: request.url });
        return response;
    };
}
export function restoreFetch() {
    globalThis.fetch = savedFetch;
    return calls;
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = installFetch)]
    fn install_fetch(body: &str);
    #[wasm_bindgen(js_name = restoreFetch)]
    fn restore_fetch() -> u32;
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn brain_streams_two_turns_through_the_cached_router() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    install_fetch(sse);
    let keys = Arc::new(ProviderKeys::with_env(
        |_| None,
        HashMap::new(),
        Arc::new(MemorySecretStore::default()),
    ));
    keys.set(Provider::Anthropic, "test-key").unwrap();
    let brain: Arc<dyn Brain> = Arc::new(
        RouterBrain::new(
            keys.clone(),
            Arc::new(ModelCatalog::new(keys)),
            ModelEndUserKey::from_installation_key(&[7; 32]),
        )
        .with_base_url(Provider::Anthropic, "https://provider.test"),
    );
    let mut turns = Vec::new();
    for _ in 0..2 {
        let request = TurnRequest {
            person_id: Some(UserId::from("person".to_string())),
            model_alias: "default".to_string(),
            model_candidates: vec!["anthropic/claude-sonnet-4-6".to_string()],
            system: "system".to_string(),
            messages: vec![TurnMessage::user("Hello")],
            tools: Vec::new(),
            computer: false,
            allow_tool_calls: true,
            max_output_tokens: Some(1024),
            output_schema: None,
        };
        let turn = brain.turn(request).await;
        turns.push(match turn {
            Ok(stream) => stream.collect::<Vec<_>>().await,
            Err(error) => vec![Err(error)],
        });
    }
    let calls = restore_fetch();
    assert_eq!(calls, 2);
    for turn in turns {
        let turn = turn.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
        assert!(matches!(&turn[0], TurnDelta::Text(text) if text == "Hello"));
        assert!(
            matches!(&turn[1], TurnDelta::Finish(end) if end.provider.as_deref() == Some("anthropic"))
        );
    }
}
