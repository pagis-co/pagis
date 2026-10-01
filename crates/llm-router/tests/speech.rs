//! Tests for speech synthesis and transcription.

use crate::common;

use common::single_provider_router;
use llm_router::{
    AudioFormat, Error, ProtocolKind, SpeechRequest, TranscriptSegment, TranscriptWord,
    TranscriptionRequest,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn speech_round_trips_on_openai_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(b"mp3-bytes".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let mut req = SpeechRequest::new("m", "Hello there", "alloy");
    req.format = Some(AudioFormat::Mp3);
    req.speed = Some(1.5);
    let response = router.speech(&req).await.unwrap();

    assert_eq!(response.audio.as_ref(), b"mp3-bytes");
    assert_eq!(response.media_type, "audio/mpeg");
    assert_eq!(response.provider, "p");

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["model"], "concrete-model");
    assert_eq!(sent["input"], "Hello there");
    assert_eq!(sent["voice"], "alloy");
    assert_eq!(sent["response_format"], "mp3");
    assert_eq!(sent["speed"], 1.5);
}

/// The Responses protocol carries OpenRouter, whose `/audio/speech` and
/// `/audio/transcriptions` take the OpenAI request shape. A buffered
/// clip and a spoken reply go there, not to `/responses`.
#[tokio::test]
async fn speech_and_transcription_take_the_openai_audio_routes_on_the_responses_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(b"mp3-bytes".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "text": "Hello world.",
            "usage": {"seconds": 1.5, "cost": 0.0001}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let router = single_provider_router(ProtocolKind::OpenAiResponses, &server.uri());

    let mut speech = SpeechRequest::new("m", "Hello there", "Kore");
    speech.format = Some(AudioFormat::Mp3);
    let spoken = router.speech(&speech).await.unwrap();
    let transcribed = router
        .transcribe(&TranscriptionRequest::new(
            "m",
            b"wav-bytes".to_vec(),
            "audio/wav",
        ))
        .await
        .unwrap();

    assert_eq!(spoken.audio.as_ref(), b"mp3-bytes");
    assert_eq!(transcribed.text, "Hello world.");
    let requests = server.received_requests().await.unwrap();
    let sent: Value = requests[0].body_json().unwrap();
    assert_eq!(sent["voice"], "Kore");
    assert_eq!(sent["response_format"], "mp3");
    let body = String::from_utf8_lossy(&requests[1].body);
    assert!(body.contains("concrete-model"));
    assert!(body.contains("name=\"response_format\""));
    assert!(!body.contains("verbose_json"));
}

/// Deepgram's voice is its model: `aura-2-thalia-en` is the Thalia voice
/// of Aura-2. The text goes in the body and the format in the query.
#[tokio::test]
async fn speech_round_trips_on_deepgram_protocol_with_the_voice_as_the_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/speak"))
        .and(query_param("model", "aura-2-thalia-en"))
        .and(query_param("encoding", "mp3"))
        .and(header("authorization", "Token test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(b"mp3-bytes".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    let router = single_provider_router(ProtocolKind::Deepgram, &server.uri());

    let mut req = SpeechRequest::new("m", "Hello there", "aura-2-thalia-en");
    req.format = Some(AudioFormat::Mp3);
    let response = router.speech(&req).await.unwrap();

    assert_eq!(response.audio.as_ref(), b"mp3-bytes");
    assert_eq!(response.media_type, "audio/mpeg");
    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent, json!({ "text": "Hello there" }));
}

#[tokio::test]
async fn transcription_round_trips_on_openai_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "text": "Hello world.",
            "language": "english",
            "duration": 1.5,
            "segments": [{"id": 0, "start": 0.0, "end": 1.5, "text": "Hello world."}],
            "words": [
                {"word": "Hello", "start": 0.0, "end": 0.6},
                {"word": "world", "start": 0.7, "end": 1.4}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::OpenAiChat, &server.uri());
    let mut req = TranscriptionRequest::new("m", b"wav-bytes".to_vec(), "audio/wav");
    req.timestamps = true;
    req.language = Some("en".into());
    let response = router.transcribe(&req).await.unwrap();

    assert_eq!(response.text, "Hello world.");
    assert_eq!(response.duration_s, Some(1.5));
    assert_eq!(
        response.segments,
        vec![TranscriptSegment {
            start_s: 0.0,
            end_s: 1.5,
            text: "Hello world.".into()
        }]
    );
    assert_eq!(response.words.len(), 2);

    // The request is multipart/form-data with the audio file and the fields.
    let request = &server.received_requests().await.unwrap()[0];
    let content_type = request
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.starts_with("multipart/form-data"));
    let body = String::from_utf8_lossy(&request.body);
    assert!(body.contains("name=\"model\""));
    assert!(body.contains("concrete-model"));
    assert!(body.contains("name=\"file\""));
    assert!(body.contains("filename=\"audio.wav\""));
    assert!(body.contains("wav-bytes"));
    assert!(body.contains("verbose_json"));
    assert!(body.contains("name=\"timestamp_granularities[]\""));
}

#[tokio::test]
async fn speech_round_trips_on_elevenlabs_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/text-to-speech/voice-id-1"))
        .and(query_param("output_format", "mp3_44100_128"))
        .and(header("xi-api-key", "test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(b"eleven-bytes".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::ElevenLabs, &server.uri());
    let mut req = SpeechRequest::new("m", "Hello there", "voice-id-1");
    req.format = Some(AudioFormat::Mp3);
    let response = router.speech(&req).await.unwrap();

    assert_eq!(response.audio.as_ref(), b"eleven-bytes");

    let sent: Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["text"], "Hello there");
    assert_eq!(sent["model_id"], "concrete-model");
}

#[tokio::test]
async fn transcription_round_trips_on_deepgram_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/listen"))
        .and(query_param("model", "concrete-model"))
        .and(query_param("smart_format", "true"))
        .and(header("authorization", "Token test-key"))
        .and(header("content-type", "audio/wav"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": {"duration": 2.0},
            "results": {"channels": [{"alternatives": [{
                "transcript": "Hello world.",
                "confidence": 0.99,
                "words": [
                    {"word": "hello", "punctuated_word": "Hello", "start": 0.1, "end": 0.5},
                    {"word": "world", "punctuated_word": "world.", "start": 0.6, "end": 1.1}
                ]
            }]}]}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::Deepgram, &server.uri());
    let req = TranscriptionRequest::new("m", b"wav-bytes".to_vec(), "audio/wav");
    let response = router.transcribe(&req).await.unwrap();

    assert_eq!(response.text, "Hello world.");
    assert_eq!(response.duration_s, Some(2.0));
    assert_eq!(
        response.words,
        vec![
            TranscriptWord {
                start_s: 0.1,
                end_s: 0.5,
                word: "Hello".into()
            },
            TranscriptWord {
                start_s: 0.6,
                end_s: 1.1,
                word: "world.".into()
            },
        ]
    );

    // The body carried the raw audio bytes.
    assert_eq!(
        server.received_requests().await.unwrap()[0].body,
        b"wav-bytes"
    );
}

#[tokio::test]
async fn speech_is_unsupported_on_anthropic_protocol() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::AnthropicMessages, &server.uri());
    let error = router
        .speech(&SpeechRequest::new("m", "hi", "alloy"))
        .await
        .unwrap_err();
    let Error::Exhausted { last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    assert!(matches!(*last, Error::Unsupported { feature, .. } if feature == "speech synthesis"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn provider_error_normalizes_on_elevenlabs_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "detail": {"status": "invalid_api_key", "message": "Invalid API key."}
        })))
        .mount(&server)
        .await;

    let router = single_provider_router(ProtocolKind::ElevenLabs, &server.uri());
    let error = router
        .speech(&SpeechRequest::new("m", "hi", "voice-id-1"))
        .await
        .unwrap_err();
    let Error::Exhausted { last, .. } = error else {
        panic!("expected Exhausted, got: {error:?}");
    };
    let Error::Provider { kind, message, .. } = *last else {
        panic!("expected Provider error, got: {last:?}");
    };
    assert_eq!(kind, llm_router::ErrorKind::Authentication);
    assert_eq!(message, "Invalid API key.");
}

#[tokio::test]
async fn voice_ids_that_rewrite_the_request_path_are_rejected() {
    let server = MockServer::start().await;
    let router = single_provider_router(ProtocolKind::ElevenLabs, &server.uri());
    let req = SpeechRequest::new("m", "hi", "../../user/delete");
    let error = router.speech(&req).await.unwrap_err();
    assert!(matches!(error, Error::InvalidConfig(_)));
    assert!(server.received_requests().await.unwrap().is_empty());
}
