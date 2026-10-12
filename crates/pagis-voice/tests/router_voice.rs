//! Buffered transcription and speech over HTTP servers (ADR-0020).

#![cfg(not(target_arch = "wasm32"))]

use pagis_core::Provider;
use pagis_voice::{Clip, VoiceProvider};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod support {
    pub mod voice;
}
use support::voice::{voice, workspace};

/// OpenRouter transcribes a held clip on its `/audio/transcriptions`
/// route. It has no realtime socket, so live dictation is absent, not
/// failed, and the clip is transcribed on release.
#[tokio::test]
async fn openrouter_transcribes_a_held_clip_and_declares_live_dictation_absent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "Book the room."
        })))
        .expect(1)
        .mount(&server)
        .await;
    let voice = voice(
        Provider::OpenRouter,
        &format!("{}/api/v1", server.uri()),
        &["openrouter/openai/gpt-4o-transcribe"],
        &[],
    );

    assert!(voice.dictate(&workspace()).await.unwrap().is_none());
    let text = voice
        .transcribe(
            &workspace(),
            Clip {
                pcm16: vec![1, 0, 2, 0].into(),
            },
        )
        .await
        .unwrap();

    assert_eq!(text, "Book the room.");
    let body =
        String::from_utf8_lossy(&server.received_requests().await.unwrap()[0].body).into_owned();
    assert!(body.contains("openai/gpt-4o-transcribe"), "{body}");
}

#[tokio::test]
async fn a_buffered_clip_posts_as_a_wav_file_to_the_transcribe_alias() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "Book the room."
        })))
        .expect(1)
        .mount(&server)
        .await;
    let voice = voice(
        Provider::OpenAi,
        &format!("{}/v1", server.uri()),
        &["openai/gpt-4o-transcribe"],
        &[],
    );

    let text = voice
        .transcribe(
            &workspace(),
            Clip {
                pcm16: vec![1, 0, 2, 0].into(),
            },
        )
        .await
        .unwrap();

    assert_eq!(text, "Book the room.");
    let request = &server.received_requests().await.unwrap()[0];
    let body = String::from_utf8_lossy(&request.body);
    assert!(body.contains("filename=\"audio.wav\""), "{body}");
    assert!(body.contains("gpt-4o-transcribe"), "{body}");
    assert!(request.body.windows(4).any(|w| w == b"RIFF"));
}

#[tokio::test]
async fn speaking_sends_the_voice_it_is_given() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(b"mp3".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    let voice = voice(
        Provider::OpenAi,
        &format!("{}/v1", server.uri()),
        &[],
        &["openai/gpt-4o-mini-tts"],
    );

    let spoken = voice
        .speak(&workspace(), "Hello there.", "nova")
        .await
        .unwrap();

    assert_eq!(spoken.audio.as_ref(), b"mp3");
    assert_eq!(spoken.media_type, "audio/mpeg");
    assert_eq!(spoken.voice, "nova");
    let requests = server.received_requests().await.unwrap();
    let sent: serde_json::Value = requests[0].body_json().unwrap();
    assert_eq!(sent["model"], "gpt-4o-mini-tts");
    assert_eq!(sent["voice"], "nova");
    assert_eq!(sent["input"], "Hello there.");
    assert_eq!(sent["response_format"], "mp3");
}

/// OpenRouter speaks on its `/audio/speech` route, with the model's own
/// voice: Gemini TTS takes Gemini voices.
#[tokio::test]
async fn openrouter_speaks_with_a_gemini_voice() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/mpeg")
                .set_body_bytes(b"mp3".to_vec()),
        )
        .expect(1)
        .mount(&server)
        .await;
    let voice = voice(
        Provider::OpenRouter,
        &format!("{}/api/v1", server.uri()),
        &[],
        &["openrouter/google/gemini-3.8-flash-tts"],
    );

    let spoken = voice
        .speak(&workspace(), "Hello there.", "Kore")
        .await
        .unwrap();

    assert_eq!(spoken.voice, "Kore");
    let sent: serde_json::Value = server.received_requests().await.unwrap()[0]
        .body_json()
        .unwrap();
    assert_eq!(sent["model"], "google/gemini-3.8-flash-tts");
    assert_eq!(sent["voice"], "Kore");
    assert_eq!(sent["response_format"], "mp3");
}
