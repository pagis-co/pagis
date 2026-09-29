//! Tests for audio in chat.

use crate::common;

use common::single_provider_router;
use futures::StreamExt;
use llm_router::{
    AudioFormat, AudioOut, ChatRequest, ContentPart, Error, Message, MessageAccumulator, Modality,
    ProtocolKind, Role, StreamEvent,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn audio_request() -> ChatRequest {
    let mut req = ChatRequest::new(
        "m",
        vec![Message {
            role: Role::User,
            content: vec![
                ContentPart::Text {
                    text: "What is in this recording?".into(),
                },
                ContentPart::InputAudio {
                    data: "UklGRg==".into(),
                    format: AudioFormat::Wav,
                },
            ],
            tool_calls: Vec::new(),
            tool_call_id: None,
        }],
    );
    req.modalities = vec![Modality::Text, Modality::Audio];
    req.audio = Some(AudioOut {
        voice: "alloy".into(),
        format: AudioFormat::Mp3,
    });
    req
}

#[tokio::test]
async fn audio_chat_round_trips_on_openai_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "concrete-model",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "audio": {
                        "id": "audio_abc",
                        "data": "bXAzYnl0ZXM=",
                        "transcript": "A trumpet.",
                        "expires_at": 1700000000u64
                    }
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 40}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let response = router.chat(&audio_request()).await.unwrap();

    assert_eq!(
        response.message.content,
        vec![ContentPart::OutputAudio {
            id: Some("audio_abc".into()),
            data: Some("bXAzYnl0ZXM=".into()),
            transcript: Some("A trumpet.".into()),
            expires_at: Some(1700000000),
        }]
    );

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["modalities"], json!(["text", "audio"]));
    assert_eq!(sent["audio"], json!({"voice": "alloy", "format": "mp3"}));
    assert_eq!(
        sent["messages"][0]["content"][1],
        json!({"type": "input_audio", "input_audio": {"data": "UklGRg==", "format": "wav"}})
    );
}

#[tokio::test]
async fn assistant_audio_replays_by_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"},
                         "finish_reason": "stop"}]
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let req = ChatRequest::new(
        "m",
        vec![
            Message::user("Say hi"),
            Message {
                role: Role::Assistant,
                content: vec![ContentPart::OutputAudio {
                    id: Some("audio_abc".into()),
                    data: None,
                    transcript: Some("Hi!".into()),
                    expires_at: None,
                }],
                tool_calls: Vec::new(),
                tool_call_id: None,
            },
            Message::user("Again"),
        ],
    );
    router.chat(&req).await.unwrap();

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["messages"][1]["audio"], json!({"id": "audio_abc"}));
}

#[tokio::test]
async fn audio_stream_events_rebuild_the_message() {
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"audio\":{\"id\":\"audio_x\",\"transcript\":\"Hel\"}}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"audio\":{\"data\":\"QUJD\",\"transcript\":\"lo\"}}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"audio\":{\"data\":\"REVG\",\"expires_at\":1700000000}}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse, "text/event-stream"),
        )
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let stream = router.chat_stream(&audio_request()).await.unwrap();
    let events: Vec<StreamEvent> = stream.events.map(|e| e.unwrap()).collect::<Vec<_>>().await;

    assert_eq!(
        events,
        vec![
            StreamEvent::AudioTranscriptDelta { text: "Hel".into() },
            StreamEvent::AudioTranscriptDelta { text: "lo".into() },
            StreamEvent::AudioDelta {
                data: "QUJD".into()
            },
            StreamEvent::AudioDelta {
                data: "REVG".into()
            },
            StreamEvent::AudioEnd {
                id: Some("audio_x".into()),
                expires_at: Some(1700000000),
            },
            StreamEvent::Finish {
                reason: llm_router::FinishReason::Stop,
                native_reason: Some("stop".into()),
                usage: None,
            },
        ]
    );

    let mut acc = MessageAccumulator::new();
    for event in &events {
        acc.push(event);
    }
    let streamed = acc.finish();
    assert_eq!(
        streamed.message.content,
        vec![ContentPart::OutputAudio {
            id: Some("audio_x".into()),
            data: Some("QUJDREVG".into()),
            transcript: Some("Hello".into()),
            expires_at: Some(1700000000),
        }]
    );
}

#[tokio::test]
async fn audio_is_unsupported_on_anthropic_protocol() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let error = router.chat(&audio_request()).await.unwrap_err();
    let Error::Exhausted { last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    assert!(matches!(*last, Error::Unsupported { feature, .. } if feature == "audio"));
    assert!(server.received_requests().await.unwrap().is_empty());
}
