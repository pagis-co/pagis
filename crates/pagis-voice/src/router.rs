//! The production voice provider: `llm-router` over the Workspace's
//! `transcribe` and `speak` aliases and the provider keys (ADR-0020).
//!
//! Keys resolve on every call, so a key the wizard stores takes effect
//! without a restart. The live path speaks the OpenAI transcription
//! dialect the router passes through: a transcription-only session with
//! no turn detection, ended by `input_audio_buffer.commit`.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use llm_router::{
    AudioFormat, Candidate, Error, ProviderConfig, RealtimeConnection, RealtimeMessage, Router,
    RouterConfig, SpeechRequest, TranscriptionRequest,
};
use pagis_core::{ModelAliasStore, Provider, ProviderKeys, ProviderUse, WorkspaceId};

use crate::{
    Clip, DictationInput, DictationSession, SAMPLE_RATE, SPEAK_ALIAS, Speech, TRANSCRIBE_ALIAS,
    Transcript, VoiceError, VoiceProvider, pcm16_wav,
};

/// The model the live session transcribes with, as ADR-0020 states it.
/// The `transcribe` alias picks the provider; the session model is a
/// property of the dialect, because the live model is made for live use
/// and the alias candidate serves the buffered path.
pub const LIVE_TRANSCRIPTION_MODEL: &str = "gpt-live-transcribe";

/// The voice seam over the model router. One instance serves every
/// tenant: the aliases a call resolves are the asking Workspace's own, so
/// the Workspace is an argument and not a field.
pub struct RouterVoice {
    keys: Arc<ProviderKeys>,
    aliases: Arc<dyn ModelAliasStore>,
    http: reqwest::Client,
    /// Base URL overrides per provider; how a test points the daemon at
    /// a local server.
    base_urls: HashMap<Provider, String>,
}

impl RouterVoice {
    pub fn new(keys: Arc<ProviderKeys>, aliases: Arc<dyn ModelAliasStore>) -> Self {
        Self {
            keys,
            aliases,
            http: reqwest::Client::new(),
            base_urls: HashMap::new(),
        }
    }

    /// Send one provider's requests to `base_url` instead of its own
    /// host. For tests.
    pub fn with_base_url(mut self, provider: Provider, base_url: impl Into<String>) -> Self {
        self.base_urls.insert(provider, base_url.into());
        self
    }

    /// The router for one alias: the alias's candidates on every
    /// provider that has a key and serves `provider_use`. Built per
    /// call; the HTTP client and its pool are shared.
    async fn router_for(
        &self,
        workspace_id: &WorkspaceId,
        alias: &str,
        provider_use: ProviderUse,
    ) -> Result<Router, VoiceError> {
        let model_alias = self
            .aliases
            .get_by_alias(workspace_id, alias)
            .await?
            .ok_or_else(|| VoiceError::NoAlias(alias.to_string()))?;
        let mut config = RouterConfig::new();
        let mut available = Vec::new();
        for provider in pagis_core::PROVIDERS {
            let Some((key, _)) = self
                .keys
                .resolve(provider)
                .map_err(|error| VoiceError::Keys(error.to_string()))?
            else {
                continue;
            };
            if !provider.serves(provider_use) {
                continue;
            }
            let mut provider_config = match provider {
                Provider::OpenAi => ProviderConfig::openai(key),
                Provider::OpenRouter => ProviderConfig::openrouter(key),
                // `Provider::uses` gives it no voice use.
                Provider::Anthropic => continue,
            };
            if let Some(base_url) = self.base_urls.get(&provider) {
                provider_config.base_url = base_url.clone();
            }
            config = config.provider(provider.id(), provider_config);
            available.push(provider.id());
        }
        let candidates: Vec<Candidate> = model_alias
            .candidates
            .iter()
            .filter_map(|candidate| candidate.split_once('/'))
            .filter(|(provider, _)| available.contains(provider))
            .map(|(provider, model)| Candidate::new(provider, model))
            .collect();
        if candidates.is_empty() {
            return Err(VoiceError::NoProvider(alias.to_string()));
        }
        config = config.model(alias, candidates);
        Router::with_client(self.http.clone(), config)
            .map_err(|error| VoiceError::Provider(error.to_string()))
    }
}

#[async_trait::async_trait]
impl VoiceProvider for RouterVoice {
    async fn transcribe(
        &self,
        workspace_id: &WorkspaceId,
        clip: Clip,
    ) -> Result<String, VoiceError> {
        let router = self
            .router_for(workspace_id, TRANSCRIBE_ALIAS, ProviderUse::Dictation)
            .await?;
        let request = TranscriptionRequest::new(
            TRANSCRIBE_ALIAS,
            pcm16_wav(&clip.pcm16, SAMPLE_RATE),
            "audio/wav",
        );
        let response = router
            .transcribe(&request)
            .await
            .map_err(|error| VoiceError::Provider(error.to_string()))?;
        Ok(response.text)
    }

    async fn dictate(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<DictationSession>, VoiceError> {
        let router = self
            .router_for(workspace_id, TRANSCRIBE_ALIAS, ProviderUse::Dictation)
            .await?;
        let connection = match router
            .realtime_transcription_connect(TRANSCRIBE_ALIAS)
            .await
        {
            Ok(connection) => connection,
            // No candidate has a realtime socket: absent, not failed.
            Err(Error::Exhausted { last, .. }) if matches!(*last, Error::Unsupported { .. }) => {
                return Ok(None);
            }
            Err(error) => return Err(VoiceError::Provider(error.to_string())),
        };
        Ok(Some(open_transcription_session(connection).await?))
    }

    async fn speak(
        &self,
        workspace_id: &WorkspaceId,
        text: &str,
        voice: &str,
    ) -> Result<Speech, VoiceError> {
        let router = self
            .router_for(workspace_id, SPEAK_ALIAS, ProviderUse::SpokenReplies)
            .await?;
        let voice = voice.to_string();
        let mut request = SpeechRequest::new(SPEAK_ALIAS, text, voice.clone());
        request.format = Some(AudioFormat::Mp3);
        let response = router
            .speech(&request)
            .await
            .map_err(|error| VoiceError::Provider(error.to_string()))?;
        Ok(Speech {
            audio: response.audio,
            media_type: response.media_type,
            voice,
        })
    }
}

/// The `session.update` that makes the socket a transcription-only
/// session with a manual commit (ADR-0020).
fn transcription_session_update() -> serde_json::Value {
    serde_json::json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": SAMPLE_RATE },
                    "transcription": { "model": LIVE_TRANSCRIPTION_MODEL },
                    "turn_detection": null
                }
            }
        }
    })
}

type Socket = llm_router::RealtimeSocket;

async fn open_transcription_session(
    connection: RealtimeConnection,
) -> Result<DictationSession, VoiceError> {
    let provider = connection.provider.clone();
    let (mut sink, stream) = connection.socket.split();
    sink.send(RealtimeMessage::text(
        transcription_session_update().to_string(),
    ))
    .await
    .map_err(|error| VoiceError::Provider(format!("{provider}: {error}")))?;
    let transcripts = stream
        .filter_map(move |frame| {
            let provider = provider.clone();
            async move {
                match frame {
                    Ok(RealtimeMessage::Text(text)) => transcript_event(&text),
                    Ok(RealtimeMessage::Close(_)) => None,
                    Ok(_) => None,
                    Err(error) => Some(Err(VoiceError::Provider(format!("{provider}: {error}")))),
                }
            }
        })
        .boxed();
    Ok(DictationSession {
        input: Box::new(OpenAiInput { sink }),
        transcripts,
    })
}

/// Map one server event to a transcript piece. Events that carry no
/// text (`session.created`, `session.updated`, buffer acknowledgements)
/// are dropped.
fn transcript_event(text: &str) -> Option<Result<Transcript, VoiceError>> {
    let event: serde_json::Value = serde_json::from_str(text).ok()?;
    match event["type"].as_str()? {
        "conversation.item.input_audio_transcription.delta" => Some(Ok(Transcript::Delta(
            event["delta"].as_str().unwrap_or_default().to_string(),
        ))),
        "conversation.item.input_audio_transcription.completed" => Some(Ok(Transcript::Final(
            event["transcript"].as_str().unwrap_or_default().to_string(),
        ))),
        "error" => Some(Err(VoiceError::Provider(
            event["error"]["message"]
                .as_str()
                .unwrap_or("the realtime session failed")
                .to_string(),
        ))),
        _ => None,
    }
}

struct OpenAiInput {
    sink: SplitSink<Socket, RealtimeMessage>,
}

impl OpenAiInput {
    async fn send(&mut self, event: serde_json::Value) -> Result<(), VoiceError> {
        self.sink
            .send(RealtimeMessage::text(event.to_string()))
            .await
            .map_err(|error| VoiceError::Provider(error.to_string()))
    }
}

#[async_trait::async_trait]
impl DictationInput for OpenAiInput {
    async fn append(&mut self, pcm16: &[u8]) -> Result<(), VoiceError> {
        let audio = base64::engine::general_purpose::STANDARD.encode(pcm16);
        self.send(serde_json::json!({
            "type": "input_audio_buffer.append",
            "audio": audio,
        }))
        .await
    }

    async fn commit(&mut self) -> Result<(), VoiceError> {
        self.send(serde_json::json!({ "type": "input_audio_buffer.commit" }))
            .await
    }
}
