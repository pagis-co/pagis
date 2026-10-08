//! Full-daemon voice tests (ADR-0020): the user dictates over
//! the channel's dictation socket, buffered on a provider without a
//! realtime socket and live on one with it; a Thread speaks an Agent's
//! `markdown` block and stays silent on every other block; the Agent
//! Voice is validated against the Provider Voice List of the model that
//! speaks; and nothing spoken is written to storage. The voice provider
//! is a fake, so no network is used.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_core::{AuthorKind, Block, MessageStore};
use pagis_storage_sqlite::SqliteMessageStore;
use pagis_testkit::fixture;
use pagis_testkit::{TestDaemon, TestDaemonOptions, test_provider_keys};
use pagis_voice::fake::{FakeVoice, VoiceCall};
use reqwest::StatusCode;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Desk {
    daemon: TestDaemon,
    voice: Arc<FakeVoice>,
}

impl Desk {
    /// A desk with an OpenAI key, so `openai/gpt-4o-mini-tts` speaks and
    /// its voices are the OpenAI voices.
    async fn start(voice: FakeVoice) -> Self {
        Self::start_with(
            voice,
            TestDaemonOptions {
                keys: test_provider_keys(vec![("OPENAI_API_KEY", "sk-test")]),
                ..TestDaemonOptions::default()
            },
        )
        .await
    }

    async fn start_with(voice: FakeVoice, options: TestDaemonOptions) -> Self {
        let voice = Arc::new(voice);
        let daemon = TestDaemon::start_with(TestDaemonOptions {
            voice: Arc::clone(&voice) as _,
            ..options
        })
        .await;
        Self { daemon, voice }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.daemon.base_url)
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(self.url(path))
            .header("cookie", self.daemon.cookie())
            .send()
            .await
            .unwrap()
    }

    async fn post_json(&self, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let response = reqwest::Client::new()
            .post(self.url(path))
            .header("cookie", self.daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (
            status,
            response.json().await.unwrap_or(serde_json::Value::Null),
        )
    }

    async fn put_json(&self, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let response = reqwest::Client::new()
            .put(self.url(path))
            .header("cookie", self.daemon.cookie())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (
            status,
            response.json().await.unwrap_or(serde_json::Value::Null),
        )
    }

    /// Open the dictation socket for a channel. The session cookie
    /// authenticates the upgrade.
    async fn dictate(&self, channel_id: &str) -> Socket {
        self.dictate_as(channel_id, self.daemon.cookie()).await
    }

    /// The same, for the Session of `cookie`.
    async fn dictate_as(&self, channel_id: &str, cookie: &str) -> Socket {
        let url = format!(
            "ws://{}/api/v1/channels/{channel_id}/dictate",
            self.daemon.addr
        );
        let (socket, _) = connect_async(self.daemon.ws_request_as(&url, cookie))
            .await
            .expect("ws connect");
        socket
    }

    /// Open the dictation socket with no cookie at all.
    async fn dictate_unsigned(&self, channel_id: &str) -> reqwest::StatusCode {
        let url = format!(
            "ws://{}/api/v1/channels/{channel_id}/dictate",
            self.daemon.addr
        );
        match connect_async(url).await {
            Ok(_) => panic!("the daemon upgraded a socket with no session"),
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                reqwest::StatusCode::from_u16(response.status().as_u16()).unwrap()
            }
            Err(error) => panic!("unexpected handshake failure: {error}"),
        }
    }

    /// An Agent message in the DM with these blocks, written the way
    /// the agent loop writes one.
    async fn agent_message(&self, blocks: Vec<Block>) -> String {
        let store = SqliteMessageStore::new(self.daemon.pool().clone());
        let workspace_id = pagis_core::WorkspaceId::from(self.workspace_id().await);
        let channel_id = pagis_core::ChannelId::from(self.daemon.dm_channel_id.clone());
        let agent_id = pagis_core::AgentId::from(self.daemon.agent_id.clone());
        let mut message = fixture::agent_message(&workspace_id, &channel_id, &agent_id, "");
        message.text_content = pagis_core::blocks_text(&blocks);
        message.blocks = blocks;
        store.insert(&message).await.unwrap();
        store
            .set_exposures(&workspace_id, &message.id, &[])
            .await
            .unwrap();
        message.id.to_string()
    }

    async fn workspace_id(&self) -> String {
        let channel: serde_json::Value = self.get("/api/v1/channels").await.json().await.unwrap();
        channel["items"][0]["workspace_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql)
            .fetch_one(self.daemon.pool())
            .await
            .unwrap()
    }
}

async fn next_frame(socket: &mut Socket) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("frame before timeout")
            .expect("socket open")
            .expect("frame ok");
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).expect("frame is JSON");
        }
    }
}

async fn commit(socket: &mut Socket) {
    socket
        .send(Message::text(
            serde_json::json!({ "type": "commit" }).to_string(),
        ))
        .await
        .unwrap();
}

/// 10 ms of silence at 24 kHz PCM16.
fn frame() -> Vec<u8> {
    vec![0u8; 480]
}

#[tokio::test]
async fn a_provider_without_a_realtime_socket_transcribes_the_clip_on_release() {
    let desk = Desk::start(FakeVoice::hearing("book the room for tuesday")).await;
    let mut socket = desk.dictate(&desk.daemon.dm_channel_id).await;

    let ready = next_frame(&mut socket).await;
    assert_eq!(ready, serde_json::json!({ "type": "ready", "live": false }));

    for _ in 0..3 {
        socket.send(Message::binary(frame())).await.unwrap();
    }
    commit(&mut socket).await;

    // No live text, the same final text: one buffered transcription of
    // the whole clip.
    let final_frame = next_frame(&mut socket).await;
    assert_eq!(
        final_frame,
        serde_json::json!({ "type": "transcript.final", "text": "book the room for tuesday" })
    );
    assert_eq!(
        desk.voice.calls(),
        vec![VoiceCall::Transcribe { bytes: 3 * 480 }]
    );

    // The transcript is a draft: no message was written.
    let timeline: serde_json::Value = desk
        .get(&format!(
            "/api/v1/channels/{}/messages",
            desk.daemon.dm_channel_id
        ))
        .await
        .json()
        .await
        .unwrap();
    let items = timeline["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "only the initial greeting remains");
    assert_eq!(items[0]["author_kind"], "agent");
    assert!(items[0]["run_id"].is_null());
    assert_eq!(
        items[0]["text_content"],
        "Hi, I'm Pixie, your Chief of Staff."
    );
}

#[tokio::test]
async fn a_provider_with_a_realtime_socket_streams_text_as_the_user_speaks() {
    let desk = Desk::start(FakeVoice::hearing("book the room").with_live_transcription()).await;
    let mut socket = desk.dictate(&desk.daemon.dm_channel_id).await;

    let ready = next_frame(&mut socket).await;
    assert_eq!(ready, serde_json::json!({ "type": "ready", "live": true }));

    let mut heard = Vec::new();
    for _ in 0..3 {
        socket.send(Message::binary(frame())).await.unwrap();
        heard.push(next_frame(&mut socket).await);
    }
    commit(&mut socket).await;
    heard.push(next_frame(&mut socket).await);

    assert_eq!(
        heard,
        vec![
            serde_json::json!({ "type": "transcript.delta", "text": "book" }),
            serde_json::json!({ "type": "transcript.delta", "text": " the" }),
            serde_json::json!({ "type": "transcript.delta", "text": " room" }),
            serde_json::json!({ "type": "transcript.final", "text": "book the room" }),
        ]
    );
    // The clip went to the live session and nowhere else.
    assert_eq!(desk.voice.calls(), vec![VoiceCall::Dictate]);
}

#[tokio::test]
async fn a_release_with_nothing_said_is_an_empty_draft() {
    let desk = Desk::start(FakeVoice::hearing("never heard")).await;
    let mut socket = desk.dictate(&desk.daemon.dm_channel_id).await;
    next_frame(&mut socket).await;

    commit(&mut socket).await;

    assert_eq!(
        next_frame(&mut socket).await,
        serde_json::json!({ "type": "transcript.final", "text": "" })
    );
    assert!(desk.voice.calls().is_empty());
}

#[tokio::test]
async fn the_dictation_socket_refuses_no_session_and_an_unknown_channel() {
    let desk = Desk::start(FakeVoice::default()).await;

    assert_eq!(
        desk.dictate_unsigned(&desk.daemon.dm_channel_id).await,
        reqwest::StatusCode::UNAUTHORIZED
    );

    let mut socket = desk.dictate("no-such-channel").await;
    let error = next_frame(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["code"], "not_found");
}

#[tokio::test]
async fn a_provider_failure_reaches_the_composer_as_an_error_frame() {
    let desk = Desk::start(FakeVoice::default()).await;
    desk.voice.fail_with(Some("the provider is down"));
    let mut socket = desk.dictate(&desk.daemon.dm_channel_id).await;
    next_frame(&mut socket).await;

    socket.send(Message::binary(frame())).await.unwrap();
    commit(&mut socket).await;

    let error = next_frame(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["code"], "transcription_failed");
    assert_eq!(error["message"], "the provider is down");
}

#[tokio::test]
async fn a_thread_speaks_the_markdown_block_in_the_agent_voice_and_stores_nothing() {
    let desk = Desk::start(FakeVoice::default()).await;
    let (status, _) = desk
        .put_json(
            &format!("/api/v1/agents/{}", desk.daemon.agent_id),
            serde_json::json!({
                "name": "Pixie",
                "job": "general assistant",
                "personality": "warm",
                "voice": "nova",
            }),
        )
        .await;
    assert_eq!(status, 200);
    let message_id = desk
        .agent_message(vec![
            Block::markdown("Here is the report."),
            Block::file("art-1", "report.pdf", None, None),
            Block::markdown("Tell me if it reads well."),
        ])
        .await;
    let base = format!(
        "/api/v1/channels/{}/messages/{message_id}",
        desk.daemon.dm_channel_id
    );

    // The one message read: the client learns which blocks to speak.
    let message: serde_json::Value = desk.get(&base).await.json().await.unwrap();
    assert_eq!(message["id"], message_id);
    assert_eq!(message["blocks"].as_array().unwrap().len(), 3);

    let response = desk.get(&format!("{base}/speech?block=0")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "audio/mpeg");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-pagis-voice"], "nova");
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        b"speech:nova:Here is the report."
    );

    let response = desk.get(&format!("{base}/speech?block=2")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        b"speech:nova:Tell me if it reads well."
    );

    // A file block is rendered and silent, and there is no caption.
    let response = desk.get(&format!("{base}/speech?block=1")).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("a `file` block is silent"),
        "{body}"
    );
    let response = desk.get(&format!("{base}/speech?block=3")).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    assert_eq!(
        desk.voice.calls(),
        vec![
            VoiceCall::Speak {
                text: "Here is the report.".to_string(),
                voice: "nova".to_string(),
            },
            VoiceCall::Speak {
                text: "Tell me if it reads well.".to_string(),
                voice: "nova".to_string(),
            },
        ]
    );

    // Nothing spoken is stored: no artifact, no blob, no event, and
    // the message is the same record it was.
    assert_eq!(desk.count("SELECT COUNT(*) FROM artifacts").await, 0);
    let blob_dir = desk.daemon.booted.home.join("artifacts");
    let blobs = std::fs::read_dir(&blob_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(blobs, 0);
    assert_eq!(
        desk.count("SELECT COUNT(*) FROM events WHERE event_type LIKE '%speech%' OR event_type LIKE '%voice%'")
            .await,
        0
    );
    let after: serde_json::Value = desk.get(&base).await.json().await.unwrap();
    assert_eq!(after, message);
}

#[tokio::test]
async fn a_user_message_is_not_spoken_and_a_missing_message_is_not_found() {
    let desk = Desk::start(FakeVoice::default()).await;
    let (status, sent) = desk
        .post_json(
            &format!("/api/v1/channels/{}/messages", desk.daemon.dm_channel_id),
            serde_json::json!({ "pending_id": "p1", "text": "read this back" }),
        )
        .await;
    assert_eq!(status, 201);
    let stored = SqliteMessageStore::new(desk.daemon.pool().clone())
        .get(
            &desk.daemon.workspace_id,
            &pagis_core::MessageId::from(sent["id"].as_str().unwrap().to_string()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.author_kind, AuthorKind::User);

    let response = desk
        .get(&format!(
            "/api/v1/channels/{}/messages/{}/speech?block=0",
            desk.daemon.dm_channel_id,
            sent["id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = desk
        .get(&format!(
            "/api/v1/channels/{}/messages/missing/speech?block=0",
            desk.daemon.dm_channel_id
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(desk.voice.calls().is_empty());
}

#[tokio::test]
async fn an_agent_with_no_voice_speaks_with_the_provider_default() {
    let desk = Desk::start(FakeVoice::default()).await;
    let message_id = desk.agent_message(vec![Block::markdown("Hello.")]).await;

    let response = desk
        .get(&format!(
            "/api/v1/channels/{}/messages/{message_id}/speech?block=0",
            desk.daemon.dm_channel_id
        ))
        .await;

    // The provider default is the first voice of the model that speaks.
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-pagis-voice"], "alloy");
    assert_eq!(
        desk.voice.calls(),
        vec![VoiceCall::Speak {
            text: "Hello.".to_string(),
            voice: "alloy".to_string(),
        }]
    );
}

/// A desk whose one key is OpenRouter's, whose `speak` alias names
/// Gemini TTS on OpenRouter, and whose OpenRouter list names the voices
/// of that model.
async fn openrouter_desk() -> (Desk, wiremock::MockServer) {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models/user"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{
                "id": "google/gemini-3.8-flash-tts",
                "created": 1,
                "architecture": { "output_modalities": ["speech"] },
                "supported_voices": ["Zephyr", "Puck", "Kore"]
            }]
        })))
        .mount(&provider)
        .await;
    let desk = Desk::start_with(
        FakeVoice::default(),
        TestDaemonOptions {
            keys: test_provider_keys(vec![("OPENROUTER_API_KEY", "sk-test")]),
            provider_base_url: Some(provider.uri()),
            ..TestDaemonOptions::default()
        },
    )
    .await;
    let (status, body) = desk
        .put_json(
            "/api/v1/settings/model-aliases/speak",
            serde_json::json!({ "candidates": ["openrouter/google/gemini-3.8-flash-tts"] }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    (desk, provider)
}

/// The Provider Voice List comes from the provider and the model that
/// speak: OpenRouter names the voices of each speech model.
#[tokio::test]
async fn the_voice_list_is_the_voices_of_the_model_that_speaks() {
    let (desk, _provider) = openrouter_desk().await;

    let voices: serde_json::Value = desk
        .get("/api/v1/settings/voices")
        .await
        .json()
        .await
        .unwrap();

    assert_eq!(voices["provider"], "openrouter");
    assert_eq!(voices["model"], "google/gemini-3.8-flash-tts");
    assert_eq!(
        voices["items"],
        serde_json::json!([
            { "id": "Zephyr", "name": null },
            { "id": "Puck", "name": null },
            { "id": "Kore", "name": null }
        ])
    );
    let (status, kore) = desk
        .post_json(
            "/api/v1/agents",
            serde_json::json!({ "name": "Kay", "job": "researcher", "voice": "Kore" }),
        )
        .await;
    assert_eq!(status, 201, "{kore}");
    let (status, body) = desk
        .post_json(
            "/api/v1/agents",
            serde_json::json!({ "name": "Nova", "job": "researcher", "voice": "nova" }),
        )
        .await;
    assert_eq!(status, 422, "{body}");
}

/// A voice the model that speaks does not have is absent: the reply
/// takes the model's first voice and says so.
#[tokio::test]
async fn a_voice_the_speaking_model_lacks_falls_back_to_its_first_voice() {
    let (desk, _provider) = openrouter_desk().await;
    let mut agent = desk
        .daemon
        .stores()
        .agents
        .get(
            &pagis_core::WorkspaceId::from(desk.workspace_id().await),
            &pagis_core::AgentId::from(desk.daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    agent.voice = Some("nova".to_string());
    desk.daemon.stores().agents.update(&agent).await.unwrap();
    let message_id = desk.agent_message(vec![Block::markdown("Hello.")]).await;

    let response = desk
        .get(&format!(
            "/api/v1/channels/{}/messages/{message_id}/speech?block=0",
            desk.daemon.dm_channel_id
        ))
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-pagis-voice"], "Zephyr");
}

/// With no key that serves spoken replies, the list is empty and a reply
/// is not spoken; the error names the keys that would speak it.
#[tokio::test]
async fn no_spoken_replies_without_a_key_that_serves_them() {
    let desk = Desk::start_with(
        FakeVoice::default(),
        TestDaemonOptions {
            keys: test_provider_keys(vec![("ANTHROPIC_API_KEY", "sk-test")]),
            ..TestDaemonOptions::default()
        },
    )
    .await;
    let message_id = desk.agent_message(vec![Block::markdown("Hello.")]).await;

    let voices: serde_json::Value = desk
        .get("/api/v1/settings/voices")
        .await
        .json()
        .await
        .unwrap();
    let response = desk
        .get(&format!(
            "/api/v1/channels/{}/messages/{message_id}/speech?block=0",
            desk.daemon.dm_channel_id
        ))
        .await;

    assert_eq!(voices["items"], serde_json::json!([]));
    assert!(voices["provider"].is_null());
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("OpenAI or OpenRouter"),
        "{body}"
    );
    assert!(desk.voice.calls().is_empty());
}

#[tokio::test]
async fn the_agent_voice_is_validated_against_the_voices_of_the_model_that_speaks() {
    let desk = Desk::start(FakeVoice::default()).await;

    let voices: serde_json::Value = desk
        .get("/api/v1/settings/voices")
        .await
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = voices["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(voices["provider"], "openai");
    assert!(names.contains(&"alloy"), "{names:?}");
    assert!(names.contains(&"nova"), "{names:?}");

    let (status, rex) = desk
        .post_json(
            "/api/v1/agents",
            serde_json::json!({ "name": "Rex", "job": "researcher", "voice": "nova" }),
        )
        .await;
    assert_eq!(status, 201);
    assert_eq!(rex["voice"], "nova");

    let (status, body) = desk
        .post_json(
            "/api/v1/agents",
            serde_json::json!({ "name": "Robo", "job": "researcher", "voice": "robot" }),
        )
        .await;
    assert_eq!(status, 422, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("`robot` is not a voice"),
        "{body}"
    );

    // Absent declares absent: the voice clears, and the seeded
    // assistant never had one.
    let (status, updated) = desk
        .put_json(
            &format!("/api/v1/agents/{}", rex["id"].as_str().unwrap()),
            serde_json::json!({ "name": "Rex", "job": "researcher", "personality": "" }),
        )
        .await;
    assert_eq!(status, 200);
    assert!(updated["voice"].is_null());
    let agents: serde_json::Value = desk.get("/api/v1/agents").await.json().await.unwrap();
    let pixie = agents["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Pixie")
        .unwrap();
    assert!(pixie["voice"].is_null());
}

#[tokio::test]
async fn the_voice_aliases_are_seeded_with_the_workspace() {
    let desk = Desk::start(FakeVoice::default()).await;

    let listed: serde_json::Value = desk
        .get("/api/v1/settings/model-aliases")
        .await
        .json()
        .await
        .unwrap();
    let items = listed["items"].as_array().unwrap();
    let find = |alias: &str| {
        items
            .iter()
            .find(|item| item["alias"] == alias)
            .unwrap_or_else(|| panic!("alias `{alias}` seeded"))
    };
    assert_eq!(
        find("transcribe")["candidates"],
        serde_json::json!(["openai/gpt-4o-transcribe"])
    );
    assert_eq!(
        find("speak")["candidates"],
        serde_json::json!(["openai/gpt-4o-mini-tts"])
    );
    assert_eq!(
        find("phone")["candidates"],
        serde_json::json!(["openai/gpt-live-1", "openai/gpt-realtime-2.1"])
    );
    assert_eq!(
        find("phone")["settings"],
        serde_json::json!([{
            "alias": "gpt-live-reasoning",
            "label": "GPT-Live reasoning model",
            "description": "The Responses model that reasons and selects tools for GPT-Live.",
            "when_candidates": ["openai/gpt-live-1"],
            "candidates": ["openai/gpt-5.6-terra"]
        }])
    );
    assert_eq!(
        find("gpt-live-reasoning")["candidates"],
        serde_json::json!(["openai/gpt-5.6-terra"])
    );
    assert_eq!(
        find("phone-classifier")["candidates"],
        serde_json::json!(["openai/gpt-realtime-2.1"])
    );
}

#[tokio::test]
async fn speech_refuses_retained_sourced_text_after_revocation_before_calling_the_provider() {
    use pagis_core::{AgentId, AgentStore, ConnectionStore, GrantStore, MemoryExposure, now_ms};
    use pagis_storage_sqlite::{SqliteAgentStore, SqliteConnectionStore, SqliteGrantStore};
    let desk = Desk::start(FakeVoice::default()).await;
    let pool = desk.daemon.pool().clone();
    let agent = SqliteAgentStore::new(pool.clone())
        .get(
            &desk.daemon.workspace_id,
            &AgentId::from(desk.daemon.agent_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let connection = pagis_core::Connection {
        id: pagis_core::ConnectionId::generate(),
        workspace_id: agent.workspace_id.clone(),
        provider: "google".into(),
        alias: "source".into(),
        display_name: "Source".into(),
        status: "connected".into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: serde_json::json!({}),
        created_at: now_ms(),
    };
    SqliteConnectionStore::new(pool.clone())
        .create(&connection)
        .await
        .unwrap();
    let grants = SqliteGrantStore::new(pool.clone());
    let grant = pagis_core::Grant {
        id: pagis_core::GrantId::generate(),
        workspace_id: agent.workspace_id.clone(),
        agent_id: agent.id.clone(),
        resource_kind: pagis_core::Grant::CONNECTION_KIND.into(),
        resource_id: Some(connection.id.to_string()),
        scope: pagis_core::Grant::connection_scope(&["gmail_read".into()]),
        revision: 1,
        created_at: now_ms(),
        revoked_at: None,
    };
    grants.create(&grant).await.unwrap();
    let exposures = vec![MemoryExposure {
        grant_id: grant.id.clone(),
        revision: grant.revision,
    }];
    let message_id = desk
        .agent_message(vec![Block::markdown("source secret")])
        .await;
    let messages = SqliteMessageStore::new(pool);
    let id = pagis_core::MessageId::from(message_id.clone());
    messages
        .set_exposures(&grant.workspace_id, &id, &exposures)
        .await
        .unwrap();
    let path = format!(
        "/api/v1/channels/{}/messages/{message_id}/speech?block=0",
        desk.daemon.dm_channel_id
    );
    assert_eq!(desk.get(&path).await.status(), StatusCode::OK);
    let before = desk.voice.calls();
    assert_eq!(before.len(), 1);
    grants
        .revoke(&grant.workspace_id, &grant.id, now_ms())
        .await
        .unwrap();
    let response = desk.get(&path).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!response.text().await.unwrap().contains("source secret"));
    assert_eq!(
        desk.voice.calls(),
        before,
        "revoked text reached the speech provider"
    );
}

/// A held button does not outlive the Session. A sign-out closes the
/// dictation socket with 1008 and transcribes nothing.
#[tokio::test]
async fn a_sign_out_closes_the_dictation_socket_with_1008() {
    let desk = Desk::start(FakeVoice::hearing("never heard")).await;
    let signing_out = desk.daemon.cookie_for(&desk.daemon.user_id).await;
    let mut socket = desk
        .dictate_as(&desk.daemon.dm_channel_id, &signing_out)
        .await;
    assert_eq!(next_frame(&mut socket).await["type"], "ready");
    socket.send(Message::binary(frame())).await.unwrap();

    desk.daemon.sign_out(&signing_out).await;

    let closed = pagis_testkit::read_until_closed(&mut socket).await;
    assert_eq!(closed.code, 1008);
    assert!(closed.frames.is_empty(), "{:?}", closed.frames);
    assert!(desk.voice.calls().is_empty(), "{:?}", desk.voice.calls());
}
