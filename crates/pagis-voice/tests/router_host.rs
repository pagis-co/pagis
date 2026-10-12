//! The voice seam and realtime router over frame channels on each target.

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use llm_router::{RealtimeMessage as Message, RealtimeTransport};
use pagis_core::Provider;
use pagis_voice::{Clip, SAMPLE_RATE, Transcript, VoiceError, VoiceProvider};

#[path = "../../llm-router/tests/support/realtime.rs"]
mod realtime;
use realtime::MemoryTransport;
mod support {
    pub mod voice;
}
use support::voice::{voice, workspace};

/// A one-session realtime server that speaks the transcription dialect
/// back: it records every client event, answers the commit with two
/// deltas and a completed transcript, and hands the recorded events
/// over when the client goes away.
fn transcription_server() -> (
    Arc<dyn RealtimeTransport>,
    impl std::future::Future<Output = (String, Vec<serde_json::Value>)>,
) {
    let (transport, mut socket) = MemoryTransport::pair();
    let observed = Arc::clone(&transport);
    let handle = async move {
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
        let url = observed.requests.lock().unwrap()[0].0.clone();
        let url = reqwest::Url::parse(&url).unwrap();
        let seen_path = format!("{}?{}", url.path(), url.query().unwrap());
        (seen_path, events)
    };
    (transport, handle)
}

type ListenRecord = (String, String, Vec<Vec<u8>>, Vec<serde_json::Value>);

/// A one-session Deepgram `/listen` server: it records the audio frames
/// and the control messages, answers `CloseStream` with an interim and
/// two final results and the closing metadata, and hands the record over.
fn listen_server() -> (
    Arc<dyn RealtimeTransport>,
    impl std::future::Future<Output = ListenRecord>,
) {
    let (transport, mut socket) = MemoryTransport::pair();
    let observed = Arc::clone(&transport);
    let handle = async move {
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
                        socket.close().await.ok();
                        break;
                    }
                }
                _ => break,
            }
        }
        let (url, headers, _) = observed.requests.lock().unwrap()[0].clone();
        let url = reqwest::Url::parse(&url).unwrap();
        let seen_path = format!("{}?{}", url.path(), url.query().unwrap());
        let seen_auth = headers["authorization"].to_str().unwrap().to_string();
        (seen_path, seen_auth, audio, controls)
    };
    (transport, handle)
}

/// Deepgram streams PCM frames to `/listen` and finishes on
/// `CloseStream`. Each final result adds to the draft; an interim one
/// waits for its final, and the whole draft ends the session.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_live_dictation_on_deepgram_streams_frames_and_ends_on_close_stream() {
    let (transport, server) = listen_server();
    let base_url = "https://deepgram.test/v1";
    let voice = voice(Provider::Deepgram, base_url, &["deepgram/nova-3"], &[])
        .with_realtime_transport(transport);

    let mut session = voice
        .dictate(&workspace())
        .await
        .unwrap()
        .expect("Deepgram has a live socket");
    session.input.append(&[1, 0, 2, 0]).await.unwrap();
    session.input.commit().await.unwrap();

    let client = async move {
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
    };
    let ((), (path, auth, audio, controls)) = futures::join!(client, server);
    assert!(
        path.starts_with("/v1/listen?model=nova-3&encoding=linear16&sample_rate=24000"),
        "{path}"
    );
    assert_eq!(auth, "Token test-key");
    assert_eq!(audio, vec![vec![1, 0, 2, 0]]);
    assert_eq!(controls, vec![serde_json::json!({ "type": "CloseStream" })]);
}

/// A one-session Scribe realtime server: it records each audio chunk and
/// answers the chunk that commits with a partial and a committed
/// transcript, then closes.
fn scribe_server() -> (
    Arc<dyn RealtimeTransport>,
    impl std::future::Future<Output = (String, Vec<serde_json::Value>)>,
) {
    let (transport, mut socket) = MemoryTransport::pair();
    let observed = Arc::clone(&transport);
    let handle = async move {
        socket
            .send(Message::text(
                r#"{"message_type":"session_started","session_id":"s1"}"#,
            ))
            .await
            .unwrap();
        let mut chunks = Vec::new();
        while let Some(Ok(Message::Text(text))) = socket.next().await {
            let chunk: serde_json::Value = serde_json::from_str(&text).unwrap();
            let commit = chunk["commit"] == true;
            chunks.push(chunk);
            if commit {
                for frame in [
                    r#"{"message_type":"partial_transcript","text":"Book the"}"#,
                    r#"{"message_type":"committed_transcript","text":"Book the room."}"#,
                ] {
                    socket.send(Message::text(frame)).await.unwrap();
                }
                socket.close().await.ok();
                break;
            }
        }
        let url = observed.requests.lock().unwrap()[0].0.clone();
        let url = reqwest::Url::parse(&url).unwrap();
        let seen_path = format!("{}?{}", url.path(), url.query().unwrap());
        (seen_path, chunks)
    };
    (transport, handle)
}

/// Scribe takes base64 audio chunks and one chunk that commits on
/// release. The committed transcript is the draft; a partial one is a
/// guess that the commit replaces.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_live_dictation_on_elevenlabs_sends_chunks_and_commits_on_release() {
    let (transport, server) = scribe_server();
    let base_url = "https://elevenlabs.test/v1";
    let voice = voice(
        Provider::ElevenLabs,
        base_url,
        &["elevenlabs/scribe_v2"],
        &[],
    )
    .with_realtime_transport(transport);

    let mut session = voice
        .dictate(&workspace())
        .await
        .unwrap()
        .expect("ElevenLabs has a live socket");
    session.input.append(&[1, 0, 2, 0]).await.unwrap();
    session.input.commit().await.unwrap();

    let client = async move {
        let mut heard = Vec::new();
        while let Some(piece) = session.transcripts.next().await {
            let piece = piece.unwrap();
            let done = matches!(piece, Transcript::Final(_));
            heard.push(piece);
            if done {
                break;
            }
        }
        assert_eq!(heard, vec![Transcript::Final("Book the room.".to_string())]);
        drop(session);
    };
    let ((), (path, chunks)) = futures::join!(client, server);
    assert!(
        path.starts_with("/v1/speech-to-text/realtime?model_id=scribe_v2_realtime"),
        "{path}"
    );
    assert_eq!(
        chunks,
        vec![
            serde_json::json!({
                "message_type": "input_audio_chunk",
                "audio_base_64": "AQACAA==",
                "commit": false,
                "sample_rate": SAMPLE_RATE,
            }),
            serde_json::json!({
                "message_type": "input_audio_chunk",
                "audio_base_64": "",
                "commit": true,
                "sample_rate": SAMPLE_RATE,
            }),
        ]
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_live_dictation_runs_a_transcription_only_session_ended_by_the_commit() {
    let (transport, server) = transcription_server();
    let base_url = "https://openai.test/v1";
    let voice = voice(
        Provider::OpenAi,
        base_url,
        &["openai/gpt-4o-transcribe"],
        &["openai/gpt-4o-mini-tts"],
    )
    .with_realtime_transport(transport);

    let mut session = voice
        .dictate(&workspace())
        .await
        .unwrap()
        .expect("OpenAI has a realtime socket");
    session.input.append(&[1, 0, 2, 0]).await.unwrap();
    session.input.commit().await.unwrap();

    let client = async move {
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
    };
    let ((), (path, events)) = futures::join!(client, server);
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
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
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

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
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

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn the_fake_transcribes_and_speaks_through_the_voice_seam() {
    use pagis_voice::fake::{FakeVoice, VoiceCall};

    let fake = FakeVoice::hearing("Book the room.");
    let voice: &dyn VoiceProvider = &fake;
    assert!(voice.dictate(&workspace()).await.unwrap().is_none());
    assert_eq!(
        voice
            .transcribe(
                &workspace(),
                Clip {
                    pcm16: vec![1, 0, 2, 0].into()
                }
            )
            .await
            .unwrap(),
        "Book the room."
    );
    let speech = voice.speak(&workspace(), "Hello.", "nova").await.unwrap();
    assert_eq!(speech.audio.as_ref(), b"speech:nova:Hello.");
    assert_eq!(speech.media_type, "audio/mpeg");
    assert_eq!(speech.voice, "nova");
    assert_eq!(
        fake.calls(),
        vec![
            VoiceCall::Transcribe { bytes: 4 },
            VoiceCall::Speak {
                text: "Hello.".into(),
                voice: "nova".into()
            },
        ]
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn the_fake_streams_deltas_and_finishes_on_commit() {
    use pagis_voice::fake::{FakeVoice, VoiceCall};

    let fake = FakeVoice::hearing("Book the room.").with_live_transcription();
    let voice: &dyn VoiceProvider = &fake;
    let mut session = voice.dictate(&workspace()).await.unwrap().unwrap();
    for _ in 0..3 {
        session.input.append(&[1, 0]).await.unwrap();
    }
    session.input.commit().await.unwrap();
    let heard: Vec<_> = session.transcripts.map(Result::unwrap).collect().await;
    assert_eq!(
        heard,
        vec![
            Transcript::Delta("Book".into()),
            Transcript::Delta(" the".into()),
            Transcript::Delta(" room.".into()),
            Transcript::Final("Book the room.".into()),
        ]
    );
    assert_eq!(fake.calls(), vec![VoiceCall::Dictate]);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn the_fake_reports_provider_and_audio_errors() {
    use pagis_voice::fake::FakeVoice;

    let fake = FakeVoice::default().with_live_transcription();
    fake.fail_with(Some("refused"));
    let clip = Clip {
        pcm16: vec![0, 0].into(),
    };
    for error in [
        fake.transcribe(&workspace(), clip).await.unwrap_err(),
        fake.speak(&workspace(), "Hello.", "nova")
            .await
            .unwrap_err(),
        fake.dictate(&workspace()).await.err().unwrap(),
    ] {
        assert!(matches!(error, VoiceError::Provider(message) if message == "refused"));
    }
    fake.fail_with(None);
    let mut session = fake.dictate(&workspace()).await.unwrap().unwrap();
    let error = session.input.append(&[1]).await.unwrap_err();
    assert!(error.to_string().contains("odd byte count"));
    session.input.append(&[1, 0]).await.unwrap();
    session.input.commit().await.unwrap();
    assert_eq!(
        session.transcripts.next().await.unwrap().unwrap(),
        Transcript::Delta("hello".into())
    );
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen_test::wasm_bindgen_test]
async fn the_voice_seam_accepts_a_local_transport_and_socket_for_each_dialect() {
    use llm_router::{RealtimeConnectFuture, RealtimeSocket};
    use reqwest::header::HeaderMap;
    use std::rc::Rc;
    use std::time::Duration;

    struct LocalTransport {
        memory: Arc<MemoryTransport>,
        value: Rc<String>,
    }

    impl RealtimeTransport for LocalTransport {
        fn connect<'a>(
            &'a self,
            url: &'a str,
            headers: &'a HeaderMap,
            timeout: Duration,
        ) -> RealtimeConnectFuture<'a> {
            let value = self.value.clone();
            Box::pin(async move {
                let socket = self.memory.connect(url, headers, timeout).await?;
                let socket = socket.map(move |frame| {
                    assert_eq!(value.as_str(), "local");
                    frame
                });
                Ok(Box::pin(socket) as RealtimeSocket)
            })
        }
    }

    for (provider, candidate, event, commit) in [
        (
            Provider::OpenAi,
            "openai/gpt-4o-transcribe",
            serde_json::json!({"type": "conversation.item.input_audio_transcription.completed", "transcript": "Book the room."}),
            serde_json::json!({"type": "input_audio_buffer.commit"}),
        ),
        (
            Provider::Deepgram,
            "deepgram/nova-3",
            serde_json::json!({"type": "Metadata"}),
            serde_json::json!({"type": "CloseStream"}),
        ),
        (
            Provider::ElevenLabs,
            "elevenlabs/scribe_v2",
            serde_json::json!({"message_type": "committed_transcript", "text": "Book the room."}),
            serde_json::json!({"message_type": "input_audio_chunk", "audio_base_64": "", "commit": true, "sample_rate": SAMPLE_RATE}),
        ),
    ] {
        let (memory, mut peer) = MemoryTransport::pair();
        let transport = Arc::new(LocalTransport {
            memory,
            value: Rc::new("local".into()),
        });
        let voice: Box<dyn VoiceProvider> = Box::new(
            voice(provider, "https://provider.test/v1", &[candidate], &[])
                .with_realtime_transport(transport),
        );
        let mut session = voice.dictate(&workspace()).await.unwrap().unwrap();
        if provider == Provider::OpenAi {
            assert!(matches!(
                peer.next().await.unwrap().unwrap(),
                Message::Text(_)
            ));
        }
        session.input.append(&[1, 0]).await.unwrap();
        assert!(peer.next().await.unwrap().is_ok());
        session.input.commit().await.unwrap();
        let Message::Text(text) = peer.next().await.unwrap().unwrap() else {
            panic!("commit is text")
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap(),
            commit
        );
        peer.send(Message::text(event.to_string())).await.unwrap();
        let expected = if provider == Provider::Deepgram {
            ""
        } else {
            "Book the room."
        };
        assert_eq!(
            session.transcripts.next().await.unwrap().unwrap(),
            Transcript::Final(expected.into())
        );
        drop(session);
        assert!(peer.next().await.is_none());
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn native_voice_sessions_and_futures_keep_their_thread_bounds() {
    fn send<T: Send>(_: T) {}
    fn send_sync<T: Send + Sync>() {}

    send_sync::<pagis_voice::RouterVoice>();
    send_sync::<pagis_voice::fake::FakeVoice>();
    let provider = voice(
        Provider::OpenAi,
        "https://openai.test/v1",
        &["openai/gpt-4o-transcribe"],
        &[],
    );
    let workspace = workspace();
    let seam: &dyn VoiceProvider = &provider;
    send(seam.dictate(&workspace));
    send(seam.transcribe(
        &workspace,
        Clip {
            pcm16: vec![0, 0].into(),
        },
    ));
    send(seam.speak(&workspace, "Hello.", "nova"));
    let fake = pagis_voice::fake::FakeVoice::default().with_live_transcription();
    let mut session = fake.dictate(&workspace).await.unwrap().unwrap();
    send(session.input.append(&[0, 0]));
    send(session.input.commit());
    send(session);
}
