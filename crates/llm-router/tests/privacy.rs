use llm_router::protocol::{AnthropicMessages, OpenAiChat, OpenAiResponses, Protocol};
use llm_router::{ChatRequest, Message, ProtocolKind, ProviderConfig};
use serde_json::{Value, json};

fn body(provider: &ProviderConfig, request: &ChatRequest, stream: bool) -> Value {
    let codec: &dyn Protocol = match provider.protocol {
        ProtocolKind::AnthropicMessages => &AnthropicMessages,
        ProtocolKind::OpenAiChat => &OpenAiChat,
        ProtocolKind::OpenAiResponses => &OpenAiResponses,
        _ => panic!("chat protocol"),
    };
    let request = codec
        .build_request(
            &reqwest::Client::new(),
            "p",
            provider,
            "model",
            request,
            stream,
        )
        .unwrap()
        .build()
        .unwrap();
    serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap()
}

#[test]
fn codecs_send_the_end_user_in_the_provider_field_only_when_set() {
    let mut openrouter_chat = ProviderConfig::openrouter("key");
    openrouter_chat.protocol = ProtocolKind::OpenAiChat;
    for provider in [
        ProviderConfig::anthropic("key"),
        ProviderConfig::openai("key"),
        ProviderConfig::openai_responses("key"),
        ProviderConfig::openrouter("key"),
        openrouter_chat,
    ] {
        for stream in [false, true] {
            let mut request = ChatRequest::new("m", vec![Message::user("hello")]);
            let absent = body(&provider, &request, stream);
            assert!(absent.pointer("/metadata/user_id").is_none());
            assert!(absent.get("safety_identifier").is_none());
            assert!(absent.get("user").is_none());
            request.end_user = Some("opaque-person-id".into());
            let sent = body(&provider, &request, stream);
            let field = if provider.protocol == ProtocolKind::AnthropicMessages {
                "/metadata/user_id"
            } else if provider.base_url.contains("openrouter") {
                "/user"
            } else {
                "/safety_identifier"
            };
            assert_eq!(sent.pointer(field), Some(&json!("opaque-person-id")));
        }
    }
}

#[test]
fn compatible_servers_without_an_end_user_field_receive_no_id() {
    for protocol in [ProtocolKind::OpenAiChat, ProtocolKind::OpenAiResponses] {
        let provider = ProviderConfig::new(protocol, "http://localhost", "key");
        let mut request = ChatRequest::new("m", vec![Message::user("hello")]);
        request.end_user = Some("opaque-person-id".into());
        let sent = body(&provider, &request, false);
        assert!(sent.get("safety_identifier").is_none());
        assert!(sent.get("user").is_none());
        assert!(sent.get("end_user").is_none());
    }
}

#[test]
fn openrouter_requests_require_no_data_collection_and_zero_data_retention() {
    for protocol in [ProtocolKind::OpenAiChat, ProtocolKind::OpenAiResponses] {
        let mut provider = ProviderConfig::openrouter("key");
        provider.protocol = protocol;
        for stream in [false, true] {
            let mut request = ChatRequest::new("m", vec![Message::user("hello")]);
            request.extra.insert(
                "provider".into(),
                json!({"order": ["chosen"], "data_collection": "allow", "zdr": false}),
            );
            let sent = body(&provider, &request, stream);
            assert_eq!(
                sent["provider"],
                json!({"order": ["chosen"], "data_collection": "deny", "zdr": true})
            );
        }
    }
}

#[test]
fn routing_configuration_applies_to_each_chat_codec() {
    for mut provider in [
        ProviderConfig::anthropic("key"),
        ProviderConfig::openai("key"),
        ProviderConfig::openai_responses("key"),
    ] {
        provider.routing = Some(json!({"data_collection": "deny", "zdr": true}));
        let request = ChatRequest::new("m", vec![Message::user("hello")]);
        assert_eq!(
            body(&provider, &request, false)["provider"],
            provider.routing.unwrap()
        );
    }
}

#[test]
fn the_end_user_keeps_other_metadata_and_cannot_be_replaced_by_extra() {
    let mut request = ChatRequest::new("m", vec![Message::user("hello")]);
    request.end_user = Some("opaque-person-id".into());
    request.extra.insert(
        "metadata".into(),
        json!({"user_id": "raw-person-id", "tag": "test"}),
    );
    request
        .extra
        .insert("safety_identifier".into(), json!("raw-person-id"));
    request.extra.insert("user".into(), json!("raw-person-id"));
    let sent = body(&ProviderConfig::anthropic("key"), &request, false);
    assert_eq!(
        sent["metadata"],
        json!({"user_id": "opaque-person-id", "tag": "test"})
    );
    for provider in [
        ProviderConfig::openai("key"),
        ProviderConfig::openai_responses("key"),
        ProviderConfig::openrouter("key"),
    ] {
        let sent = body(&provider, &request, false);
        let field = if provider.base_url.contains("openrouter") {
            "user"
        } else {
            "safety_identifier"
        };
        assert_eq!(sent[field], "opaque-person-id");
        let other_field = if field == "user" {
            "safety_identifier"
        } else {
            "user"
        };
        assert!(sent.get(other_field).is_none());
    }
}

#[test]
fn routing_rules_apply_to_json_requests_for_other_model_uses() {
    let mut provider = ProviderConfig::openrouter("key");
    provider.protocol = ProtocolKind::OpenAiChat;
    let http = reqwest::Client::new();
    let requests = [
        OpenAiChat
            .build_embeddings_request(
                &http,
                "p",
                &provider,
                "model",
                &llm_router::EmbeddingsRequest::new("m", vec!["text".into()]),
            )
            .unwrap(),
        OpenAiChat
            .build_image_request(
                &http,
                "p",
                &provider,
                "model",
                &llm_router::ImageRequest::new("m", "image"),
            )
            .unwrap(),
        OpenAiChat
            .build_video_create_request(
                &http,
                "p",
                &provider,
                "model",
                &llm_router::VideoRequest::new("m", "video"),
            )
            .unwrap(),
        OpenAiResponses
            .build_speech_request(
                &http,
                "p",
                &provider,
                "model",
                &llm_router::SpeechRequest::new("m", "text", "voice"),
            )
            .unwrap(),
    ];
    for request in requests {
        let request = request.build().unwrap();
        let sent: Value =
            serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(
            sent["provider"],
            json!({"data_collection": "deny", "zdr": true})
        );
    }
}

#[tokio::test]
async fn routing_rules_apply_to_multipart_transcription_without_duplicate_provider_fields() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let mut provider = ProviderConfig::openrouter("key");
    provider.base_url = server.uri();
    let mut request = llm_router::TranscriptionRequest::new("m", vec![1, 2, 3], "audio/wav");
    request.extra.insert(
        "provider".into(),
        json!({"order": ["chosen"], "data_collection": "allow", "zdr": false}),
    );
    OpenAiResponses
        .build_transcription_request(&reqwest::Client::new(), "p", &provider, "model", &request)
        .unwrap()
        .send()
        .await
        .unwrap();
    let sent = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&sent[0].body);
    assert_eq!(body.matches("name=\"provider\"").count(), 1);
    let field = body.split("name=\"provider\"").nth(1).unwrap();
    let value = field
        .split("\r\n\r\n")
        .nth(1)
        .unwrap()
        .split("\r\n")
        .next()
        .unwrap();
    let routing: Value = serde_json::from_str(value).unwrap();
    assert_eq!(
        routing,
        json!({"data_collection": "deny", "order": ["chosen"], "zdr": true})
    );
}

#[test]
fn extra_cannot_send_an_end_user_when_the_request_or_provider_has_none() {
    let mut request = ChatRequest::new("m", vec![Message::user("hello")]);
    request.extra.insert(
        "metadata".into(),
        json!({"user_id": "raw-person-id", "tag": "test"}),
    );
    request
        .extra
        .insert("safety_identifier".into(), json!("raw-person-id"));
    request.extra.insert("user".into(), json!("raw-person-id"));
    let sent = body(&ProviderConfig::anthropic("key"), &request, false);
    assert!(sent.pointer("/metadata/user_id").is_none());
    assert_eq!(sent["metadata"]["tag"], "test");
    for provider in [
        ProviderConfig::openai("key"),
        ProviderConfig::openai_responses("key"),
        ProviderConfig::openrouter("key"),
    ] {
        let sent = body(&provider, &request, false);
        assert!(sent.get("safety_identifier").is_none());
        assert!(sent.get("user").is_none());
    }
    request.end_user = Some("opaque-person-id".into());
    for protocol in [ProtocolKind::OpenAiChat, ProtocolKind::OpenAiResponses] {
        let provider = ProviderConfig::new(protocol, "http://localhost", "key");
        let sent = body(&provider, &request, false);
        assert!(sent.get("safety_identifier").is_none());
        assert!(sent.get("user").is_none());
    }
}

#[test]
fn routing_rules_must_be_an_object() {
    for routing in [json!(null), json!("deny"), json!(["deny"])] {
        for mut provider in [
            ProviderConfig::anthropic("key"),
            ProviderConfig::openai("key"),
            ProviderConfig::openai_responses("key"),
        ] {
            provider.routing = Some(routing.clone());
            let codec: &dyn Protocol = match provider.protocol {
                ProtocolKind::AnthropicMessages => &AnthropicMessages,
                ProtocolKind::OpenAiChat => &OpenAiChat,
                ProtocolKind::OpenAiResponses => &OpenAiResponses,
                _ => unreachable!(),
            };
            let request = ChatRequest::new("m", vec![Message::user("hello")]);
            assert!(matches!(
                codec.build_request(
                    &reqwest::Client::new(),
                    "p",
                    &provider,
                    "model",
                    &request,
                    false
                ),
                Err(llm_router::Error::InvalidConfig(_))
            ));
        }
    }
}
