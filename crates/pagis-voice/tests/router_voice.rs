//! The production voice provider against local servers
//! (ADR-0020): the OpenAI transcription dialect over a scripted
//! WebSocket, buffered transcription and speech over wiremock, and the
//! degradation when no candidate has a realtime socket.

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use pagis_core::{
    MemorySecretStore, ModelAlias, ModelAliasId, ModelAliasStore, Provider, ProviderKeys,
    StoreError, WorkspaceId, now_ms,
};
use pagis_voice::{
    Clip, RouterVoice, SAMPLE_RATE, SPEAK_ALIAS, TRANSCRIBE_ALIAS, Transcript, VoiceError,
    VoiceProvider,
};
use tokio_tungstenite::tungstenite::Message;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// An alias store with the two voice aliases and nothing else.
struct Aliases {
    workspace_id: WorkspaceId,
    transcribe: Vec<String>,
    speak: Vec<String>,
}

#[async_trait::async_trait]
impl ModelAliasStore for Aliases {
    async fn create(&self, _: &ModelAlias) -> Result<(), StoreError> {
        unreachable!("the tests never create an alias")
    }

    async fn get_by_alias(
        &self,
        _: &WorkspaceId,
        alias: &str,
    ) -> Result<Option<ModelAlias>, StoreError> {
        let candidates = match alias {
            TRANSCRIBE_ALIAS => self.transcribe.clone(),
            SPEAK_ALIAS => self.speak.clone(),
            _ => return Ok(None),
        };
        Ok(Some(ModelAlias {
            id: ModelAliasId::generate(),
            workspace_id: self.workspace_id.clone(),
            alias: alias.to_string(),
            candidates,
            created_at: now_ms(),
            updated_at: now_ms(),
        }))
    }

    async fn list(&self, _: &WorkspaceId) -> Result<Vec<ModelAlias>, StoreError> {
        Ok(Vec::new())
    }

    async fn update_candidates(
        &self,
        _: &WorkspaceId,
        _: &str,
        _: &[String],
        _: i64,
    ) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn delete(&self, _: &WorkspaceId, _: &str) -> Result<bool, StoreError> {
        Ok(false)
    }
}

fn keys(provider: Provider) -> Arc<ProviderKeys> {
    let keys = ProviderKeys::with_env(
        |_| None,
        std::collections::HashMap::new(),
        Arc::new(MemorySecretStore::default()),
    );
    keys.set(provider, "test-key").unwrap();
    Arc::new(keys)
}

/// The one tenant these bodies act as. The voice seam takes the Workspace
/// per call, so the test names the same one the aliases are under.
fn workspace() -> WorkspaceId {
    WorkspaceId::from("ws-voice".to_string())
}

fn voice(provider: Provider, base_url: &str, transcribe: &[&str], speak: &[&str]) -> RouterVoice {
    RouterVoice::new(
        keys(provider),
        Arc::new(Aliases {
            workspace_id: workspace(),
            transcribe: transcribe.iter().map(|c| c.to_string()).collect(),
            speak: speak.iter().map(|c| c.to_string()).collect(),
        }),
    )
    .with_base_url(provider, base_url)
}

/// A one-session realtime server that speaks the transcription dialect
/// back: it records every client event, answers the commit with two
/// deltas and a completed transcript, and hands the recorded events
/// over when the client goes away.
// The handshake callback's Result type comes from tokio-tungstenite.
#[allow(clippy::result_large_err)]
async fn transcription_server() -> (
    String,
    tokio::task::JoinHandle<(String, Vec<serde_json::Value>)>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut seen_path = String::new();
        let mut socket = tokio_tungstenite::accept_hdr_async(
            stream,
            |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
                seen_path = req.uri().to_string();
                Ok(resp)
            },
        )
        .await
        .unwrap();
        socket
            .send(Message::text(r#"{"type":"session.created"}"#))
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(Ok(frame)) = socket.next().await {
            let Message::Text(text) = frame else { break };
            let event: serde_json::Value = serde_json::from_str(&text).unwrap();
            let kind = event["type"].as_str().unwrap().to_string();
            events.push(event);
            match kind.as_str() {
                "session.update" => {
                    socket
                        .send(Message::text(r#"{"type":"session.updated"}"#))
                        .await
                        .unwrap();
                }
                "input_audio_buffer.commit" => {
                    for frame in [
                        r#"{"type":"input_audio_buffer.committed","item_id":"item_1"}"#,
                        r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"item_1","delta":"Book the"}"#,
                        r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"item_1","delta":" room"}"#,
                        r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"item_1","transcript":"Book the room."}"#,
                    ] {
                        socket.send(Message::text(frame)).await.unwrap();
                    }
                }
                _ => {}
            }
        }
        (seen_path, events)
    });
    (format!("http://127.0.0.1:{}/v1", addr.port()), handle)
}

/// A one-session Deepgram `/listen` server: it records the audio frames
/// and the control messages, answers `CloseStream` with an interim and
/// two final results and the closing metadata, and hands the record over.
#[allow(clippy::result_large_err)]
async fn listen_server() -> (
    String,
    tokio::task::JoinHandle<(String, String, Vec<Vec<u8>>, Vec<serde_json::Value>)>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut seen_path = String::new();
        let mut seen_auth = String::new();
        let mut socket = tokio_tungstenite::accept_hdr_async(
            stream,
            |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
                seen_path = req.uri().to_string();
                seen_auth = req
                    .headers()
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                Ok(resp)
            },
        )
        .await
        .unwrap();
        let mut audio = Vec::new();
        let mut controls = Vec::new();
        while let Some(Ok(frame)) = socket.next().await {
            match frame {
                Message::Binary(bytes) => audio.push(bytes.to_vec()),
                Message::Text(text) => {
                    let control: serde_json::Value = serde_json::from_str(&text).unwrap();
                    let close = control["type"] == "CloseStream";
                    controls.push(control);
                    if close {
                        for frame in [
                            r#"{"type":"Results","is_final":true,"channel":{"alternatives":[{"transcript":"Book the"}]}}"#,
                            r#"{"type":"Results","is_final":false,"channel":{"alternatives":[{"transcript":"room"}]}}"#,
                            r#"{"type":"Results","is_final":true,"channel":{"alternatives":[{"transcript":"room."}]}}"#,
                            r#"{"type":"Metadata","request_id":"r1"}"#,
                        ] {
                            socket.send(Message::text(frame)).await.unwrap();
                        }
                        socket.close(None).await.ok();
                        break;
                    }
                }
                _ => break,
            }
        }
        (seen_path, seen_auth, audio, controls)
    });
    (format!("http://127.0.0.1:{}/v1", addr.port()), handle)
}

/// Deepgram streams PCM frames to `/listen` and finishes on
/// `CloseStream`. Each final result adds to the draft; an interim one
/// waits for its final, and the whole draft ends the session.
#[tokio::test]
async fn a_live_dictation_on_deepgram_streams_frames_and_ends_on_close_stream() {
    let (base_url, server) = listen_server().await;
    let voice = voice(Provider::Deepgram, &base_url, &["deepgram/nova-3"], &[]);

    let mut session = voice
        .dictate(&workspace())
        .await
        .unwrap()
        .expect("Deepgram has a live socket");
    session.input.append(&[1, 0, 2, 0]).await.unwrap();
    session.input.commit().await.unwrap();

    let mut heard = Vec::new();
    while let Some(piece) = session.transcripts.next().await {
        let piece = piece.unwrap();
        let done = matches!(piece, Transcript::Final(_));
        heard.push(piece);
        if done {
            break;
        }
    }
    assert_eq!(
        heard,
        vec![
            Transcript::Delta("Book the".to_string()),
            Transcript::Delta(" room.".to_string()),
            Transcript::Final("Book the room.".to_string()),
        ]
    );
    drop(session);

    let (path, auth, audio, controls) = server.await.unwrap();
    assert!(
        path.starts_with("/v1/listen?model=nova-3&encoding=linear16&sample_rate=24000"),
        "{path}"
    );
    assert_eq!(auth, "Token test-key");
    assert_eq!(audio, vec![vec![1, 0, 2, 0]]);
    assert_eq!(controls, vec![serde_json::json!({ "type": "CloseStream" })]);
}

#[tokio::test]
async fn a_live_dictation_runs_a_transcription_only_session_ended_by_the_commit() {
    let (base_url, server) = transcription_server().await;
    let voice = voice(
        Provider::OpenAi,
        &base_url,
        &["openai/gpt-4o-transcribe"],
        &["openai/gpt-4o-mini-tts"],
    );

    let mut session = voice
        .dictate(&workspace())
        .await
        .unwrap()
        .expect("OpenAI has a realtime socket");
    session.input.append(&[1, 0, 2, 0]).await.unwrap();
    session.input.commit().await.unwrap();

    let mut heard = Vec::new();
    while let Some(piece) = session.transcripts.next().await {
        let piece = piece.unwrap();
        let done = matches!(piece, Transcript::Final(_));
        heard.push(piece);
        if done {
            break;
        }
    }
    assert_eq!(
        heard,
        vec![
            Transcript::Delta("Book the".to_string()),
            Transcript::Delta(" room".to_string()),
            Transcript::Final("Book the room.".to_string()),
        ]
    );
    drop(session);

    let (path, events) = server.await.unwrap();
    assert_eq!(path, "/v1/realtime?intent=transcription");
    assert_eq!(
        events[0],
        serde_json::json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "audio": {
                    "input": {
                        "format": { "type": "audio/pcm", "rate": SAMPLE_RATE },
                        "transcription": { "model": "gpt-live-transcribe" },
                        "turn_detection": null
                    }
                }
            }
        })
    );
    assert_eq!(events[1]["type"], "input_audio_buffer.append");
    assert_eq!(events[1]["audio"], "AQACAA==");
    assert_eq!(
        events[2],
        serde_json::json!({ "type": "input_audio_buffer.commit" })
    );
}

/// A provider that `Provider::uses` gives no voice use is not selected,
/// even with a key and a candidate in the alias.
#[tokio::test]
async fn a_provider_without_a_voice_use_is_not_selected() {
    let voice = voice(
        Provider::Anthropic,
        "http://127.0.0.1:9/v1",
        &["anthropic/claude-sonnet-5-5"],
        &["anthropic/claude-sonnet-5-5"],
    );

    let error = voice
        .transcribe(
            &workspace(),
            Clip {
                pcm16: vec![0, 0].into(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, VoiceError::NoProvider(alias) if alias == "transcribe"));
    let error = voice
        .dictate(&workspace())
        .await
        .err()
        .expect("no dictation");
    assert!(matches!(error, VoiceError::NoProvider(alias) if alias == "transcribe"));
    let error = voice.speak(&workspace(), "x", "alloy").await.unwrap_err();
    assert!(matches!(error, VoiceError::NoProvider(alias) if alias == "speak"));
}

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

#[tokio::test]
async fn an_alias_with_no_keyed_provider_is_a_clear_error() {
    let voice = voice(
        Provider::Anthropic,
        "http://127.0.0.1:9/v1",
        &["openai/gpt-4o-transcribe"],
        &[],
    );

    let error = voice
        .transcribe(
            &workspace(),
            Clip {
                pcm16: vec![0, 0].into(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, VoiceError::NoProvider(alias) if alias == "transcribe"));

    let error = voice.speak(&workspace(), "x", "alloy").await.unwrap_err();
    assert!(matches!(error, VoiceError::NoProvider(alias) if alias == "speak"));
}
