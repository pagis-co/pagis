//! Live telephone model adapters selected through Workspace aliases.
//!
//! The call loop uses provider-neutral commands and events. This file
//! owns the OpenAI Realtime and GPT-Live wire contracts. An adapter
//! for another provider implements the same [`ModelSession`] seam.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use llm_router::{
    Candidate, ProviderConfig, RealtimeConnection, RealtimeMessage, RealtimeProtocol, Router,
    RouterConfig,
};
use pagis_core::{ModelAliasStore, Provider, ProviderKeys, ProviderUse, WorkspaceId};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::brief::REPORT_ANSWER;
use crate::model::{
    AUDIO_FORMAT, ClientCommand, ModelError, ModelSession, ModelSessions, ServerEvent,
    SessionConfig, function_tools,
};

/// The Workspace-wide telephone model. The first candidate is the
/// default. A user can reorder this alias in Settings.
pub const PHONE_ALIAS: &str = "phone";
pub const PHONE_MODELS: [&str; 2] = ["openai/gpt-live-1", "openai/gpt-realtime-2.1"];

/// The Responses model that reasons and selects tools for GPT-Live.
pub const GPT_LIVE_REASONING_ALIAS: &str = "gpt-live-reasoning";
pub const GPT_LIVE_REASONING_MODELS: [&str; 1] = ["openai/gpt-5.6-terra"];

/// One model-specific setting shown inside the telephone model editor.
pub struct PhoneModelSetting {
    pub alias: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub when_candidates: &'static [&'static str],
    pub default_candidates: &'static [&'static str],
}

/// Settings that belong to one telephone model instead of every call.
pub const PHONE_MODEL_SETTINGS: [PhoneModelSetting; 1] = [PhoneModelSetting {
    alias: GPT_LIVE_REASONING_ALIAS,
    label: "GPT-Live reasoning model",
    description: "The Responses model that reasons and selects tools for GPT-Live.",
    when_candidates: &["openai/gpt-live-1"],
    default_candidates: &GPT_LIVE_REASONING_MODELS,
}];

/// The audio-capable detector used before an outbound conversation.
pub const PHONE_CLASSIFIER_ALIAS: &str = "phone-classifier";
pub const PHONE_CLASSIFIER_MODELS: [&str; 1] = ["openai/gpt-realtime-2.1"];

/// The Realtime candidate the classifier uses.
pub const REALTIME_MODEL: &str = "gpt-realtime-2.1";

/// Builds call sessions from provider keys and Workspace aliases. One
/// instance serves every tenant: the aliases come from the Workspace the
/// call belongs to.
pub struct KeyedModelSessions {
    keys: Arc<ProviderKeys>,
    aliases: Arc<dyn ModelAliasStore>,
    http: reqwest::Client,
    base_urls: HashMap<Provider, String>,
}

impl KeyedModelSessions {
    pub fn new(keys: Arc<ProviderKeys>, aliases: Arc<dyn ModelAliasStore>) -> Self {
        Self {
            keys,
            aliases,
            http: reqwest::Client::new(),
            base_urls: HashMap::new(),
        }
    }

    pub fn with_base_url(mut self, provider: Provider, base_url: impl Into<String>) -> Self {
        self.base_urls.insert(provider, base_url.into());
        self
    }

    async fn router(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<(Router, Option<String>), ModelError> {
        let mut config = RouterConfig::new();
        let mut available = Vec::new();
        for provider in pagis_core::PROVIDERS {
            let Some((key, _)) = self
                .keys
                .resolve(provider)
                .map_err(|error| ModelError(error.to_string()))?
            else {
                continue;
            };
            if !provider.serves(ProviderUse::Calls) {
                continue;
            }
            let mut provider_config = match provider {
                Provider::OpenAi => ProviderConfig::openai(key),
                // `Provider::uses` gives them no call use.
                Provider::Anthropic
                | Provider::OpenRouter
                | Provider::Deepgram
                | Provider::ElevenLabs => continue,
            };
            if let Some(base_url) = self.base_urls.get(&provider) {
                provider_config.base_url = base_url.clone();
            }
            config = config.provider(provider.id(), provider_config);
            available.push(provider.id());
        }
        if available.is_empty() {
            return Err(ModelError(
                "no OpenAI key is configured; a call needs one to talk. Add it in Settings or set OPENAI_API_KEY"
                    .to_string(),
            ));
        }

        for alias in [PHONE_ALIAS, PHONE_CLASSIFIER_ALIAS] {
            let stored = self
                .aliases
                .get_by_alias(workspace_id, alias)
                .await
                .map_err(|error| ModelError(error.to_string()))?
                .ok_or_else(|| {
                    ModelError(format!("the `{alias}` model alias is not configured"))
                })?;
            let candidates: Vec<Candidate> = stored
                .candidates
                .iter()
                .filter_map(|candidate| candidate.split_once('/'))
                .filter(|(provider, _)| available.contains(provider))
                .map(|(provider, model)| Candidate::new(provider, model))
                .collect();
            if candidates.is_empty() {
                return Err(ModelError(format!(
                    "the `{alias}` model alias has no provider with a configured key"
                )));
            }
            config = config.model(alias, candidates);
        }

        let backend_model = self
            .aliases
            .get_by_alias(workspace_id, GPT_LIVE_REASONING_ALIAS)
            .await
            .map_err(|error| ModelError(error.to_string()))?
            .and_then(|stored| {
                stored.candidates.into_iter().find_map(|candidate| {
                    let (provider, model) = candidate.split_once('/')?;
                    available.contains(&provider).then(|| model.to_string())
                })
            });

        let router = Router::with_client(self.http.clone(), config)
            .map_err(|error| ModelError(error.to_string()))?;
        Ok((router, backend_model))
    }
}

#[async_trait]
impl ModelSessions for KeyedModelSessions {
    async fn open(&self, workspace_id: &WorkspaceId) -> Result<Box<dyn ModelSession>, ModelError> {
        let (router, backend_model) = self.router(workspace_id).await?;
        Ok(Box::new(AdaptiveSession {
            router: Arc::new(router),
            backend_model,
            active: None,
            active_alias: None,
            translate_classifier_result: false,
        }))
    }
}

/// Opens a fixed router alias. This is useful for injected routers.
pub struct RouterModelSessions {
    router: Arc<Router>,
    model: String,
    backend_model: String,
}

impl RouterModelSessions {
    pub fn new(router: Arc<Router>, model: impl Into<String>) -> Self {
        Self {
            router,
            model: model.into(),
            backend_model: GPT_LIVE_REASONING_MODELS[0]
                .split_once('/')
                .expect("a qualified backend model")
                .1
                .to_string(),
        }
    }
}

#[async_trait]
impl ModelSessions for RouterModelSessions {
    async fn open(&self, _workspace_id: &WorkspaceId) -> Result<Box<dyn ModelSession>, ModelError> {
        Ok(Box::new(
            RouterSession::connect(&self.router, &self.model, Some(&self.backend_model)).await?,
        ))
    }
}

/// Selects the classifier or conversation alias from the typed session
/// configuration. The call loop does not know which provider serves it.
struct AdaptiveSession {
    router: Arc<Router>,
    backend_model: Option<String>,
    active: Option<RouterSession>,
    active_alias: Option<&'static str>,
    translate_classifier_result: bool,
}

impl AdaptiveSession {
    async fn configure(&mut self, config: SessionConfig) -> Result<(), ModelError> {
        let alias = if config.tools.iter().any(|tool| tool.name == REPORT_ANSWER) {
            PHONE_CLASSIFIER_ALIAS
        } else {
            PHONE_ALIAS
        };
        if self.active_alias != Some(alias) {
            if let Some(active) = self.active.take() {
                active.finish().await;
            }
            self.translate_classifier_result =
                self.active_alias == Some(PHONE_CLASSIFIER_ALIAS) && alias == PHONE_ALIAS;
            self.active = Some(
                RouterSession::connect(&self.router, alias, self.backend_model.as_deref()).await?,
            );
            self.active_alias = Some(alias);
        }
        self.active
            .as_mut()
            .expect("the active model was opened")
            .send(ClientCommand::Configure(config))
            .await
    }

    fn active_mut(&mut self) -> Result<&mut RouterSession, ModelError> {
        self.active
            .as_mut()
            .ok_or_else(|| ModelError("configure the model session before using it".to_string()))
    }
}

#[async_trait]
impl ModelSession for AdaptiveSession {
    async fn send(&mut self, command: ClientCommand) -> Result<(), ModelError> {
        match command {
            ClientCommand::Configure(config) => self.configure(config).await,
            ClientCommand::FunctionCallOutput { output, .. }
                if self.translate_classifier_result =>
            {
                self.active_mut()?
                    .send(ClientCommand::AddInstructions(format!(
                        "The outbound answer detector completed: {output}. Follow the current conversation instructions now."
                    )))
                    .await
            }
            ClientCommand::ContinueResponse if self.translate_classifier_result => {
                self.translate_classifier_result = false;
                self.active_mut()?
                    .send(ClientCommand::ContinueResponse)
                    .await
            }
            command => self.active_mut()?.send(command).await,
        }
    }

    async fn next(&mut self) -> Option<Result<ServerEvent, ModelError>> {
        self.active_mut().ok()?.next().await
    }

    async fn close(mut self: Box<Self>) {
        if let Some(active) = self.active.take() {
            active.finish().await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptSpeaker {
    Caller,
    Agent,
}

struct RouterSession {
    connection: RealtimeConnection,
    backend_model: Option<String>,
    started: bool,
    queued: VecDeque<ServerEvent>,
    transcript: Option<(TranscriptSpeaker, String)>,
    transcript_deadline: Option<Instant>,
}

impl RouterSession {
    async fn connect(
        router: &Router,
        alias: &str,
        backend_model: Option<&str>,
    ) -> Result<Self, ModelError> {
        let connection = router
            .realtime_connect(alias)
            .await
            .map_err(|error| ModelError(error.to_string()))?;
        Ok(Self {
            connection,
            backend_model: backend_model.map(str::to_string),
            started: false,
            queued: VecDeque::new(),
            transcript: None,
            transcript_deadline: None,
        })
    }

    async fn send_json(&mut self, event: Value) -> Result<(), ModelError> {
        self.connection
            .send(RealtimeMessage::Text(event.to_string().into()))
            .await
            .map_err(|error| ModelError(error.to_string()))
    }

    async fn receive_json(&mut self) -> Option<Result<Value, ModelError>> {
        loop {
            match self.connection.next().await? {
                Ok(RealtimeMessage::Text(text)) => {
                    return Some(
                        serde_json::from_str(&text).map_err(|error| {
                            ModelError(format!("the model sent no JSON: {error}"))
                        }),
                    );
                }
                Ok(RealtimeMessage::Close(_)) => return None,
                Ok(_) => continue,
                Err(error) => return Some(Err(ModelError(error.to_string()))),
            }
        }
    }

    async fn start_live(&mut self, config: &SessionConfig) -> Result<(), ModelError> {
        let backend_model = self.backend_model.as_deref().ok_or_else(|| {
            ModelError(format!(
                "the `{GPT_LIVE_REASONING_ALIAS}` model setting is not configured"
            ))
        })?;
        let mut audio = json!({
            "format": { "type": AUDIO_FORMAT, "rate": 8000 },
        });
        if let (Some(voice), Some(audio)) = (&config.voice, audio.as_object_mut()) {
            audio.insert("output".to_string(), json!({ "voice": voice }));
        }
        self.send_json(json!({
            "type": "session.start",
            "session": {
                "model": self.connection.model,
                "instructions": config.instructions,
                "audio": audio,
                "delegation": {
                    "type": "responses",
                    "responses": live_responses(backend_model, config),
                },
            },
        }))
        .await?;
        loop {
            let event = self.receive_json().await.ok_or_else(|| {
                ModelError("the live session closed before it started".to_string())
            })??;
            match event.get("type").and_then(Value::as_str) {
                Some("session.started") => {
                    self.started = true;
                    return Ok(());
                }
                Some("error") => {
                    let message = match ServerEvent::parse_live(&event) {
                        ServerEvent::Error(message) => message,
                        _ => "the live session did not start".to_string(),
                    };
                    return Err(ModelError(message));
                }
                _ => {
                    let parsed = ServerEvent::parse_live(&event);
                    if parsed != ServerEvent::Other {
                        self.queued.push_back(parsed);
                    }
                }
            }
        }
    }

    async fn update_live(&mut self, config: &SessionConfig) -> Result<(), ModelError> {
        let backend_model = self.backend_model.as_deref().ok_or_else(|| {
            ModelError(format!(
                "the `{GPT_LIVE_REASONING_ALIAS}` model setting is not configured"
            ))
        })?;
        self.send_json(json!({
            "type": "session.update",
            "session": {
                "delegation": {
                    "responses": live_responses(backend_model, config),
                },
            },
        }))
        .await?;
        self.send_json(json!({
            "type": "session.instructions.append",
            "delegation_id": null,
            "content": config.instructions,
        }))
        .await
    }

    async fn send(&mut self, command: ClientCommand) -> Result<(), ModelError> {
        if self.connection.protocol == RealtimeProtocol::OpenAiRealtime {
            return self.send_json(command.realtime_event()).await;
        }
        match command {
            ClientCommand::Configure(config) if !self.started => self.start_live(&config).await,
            ClientCommand::Configure(config) => self.update_live(&config).await,
            ClientCommand::AppendAudio(audio) => {
                self.send_json(json!({
                    "type": "session.input_audio.append",
                    "audio": base64::engine::general_purpose::STANDARD.encode(audio),
                }))
                .await
            }
            ClientCommand::Truncate { .. } => Ok(()),
            ClientCommand::FunctionCallOutput { call_id, output } => {
                self.send_json(json!({
                    "type": "response.item.create",
                    "item": {
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": output,
                    },
                }))
                .await
            }
            ClientCommand::AddInstructions(content) => {
                self.send_json(json!({
                    "type": "session.instructions.append",
                    "delegation_id": null,
                    "content": content,
                }))
                .await
            }
            ClientCommand::ContinueResponse => {
                self.send_json(json!({ "type": "response.create" })).await
            }
        }
    }

    fn accept_transcript(&mut self, event: ServerEvent) -> Option<ServerEvent> {
        let (speaker, delta) = match event {
            ServerEvent::CallerTranscript(delta) => (TranscriptSpeaker::Caller, delta),
            ServerEvent::AgentTranscript(delta) => (TranscriptSpeaker::Agent, delta),
            event => return Some(event),
        };
        let previous = match self.transcript.take() {
            Some((previous_speaker, mut text)) if previous_speaker == speaker => {
                text.push_str(&delta);
                self.transcript = Some((speaker, text));
                None
            }
            Some((previous_speaker, text)) => {
                self.transcript = Some((speaker, delta));
                Some(transcript_event(previous_speaker, text))
            }
            None => {
                self.transcript = Some((speaker, delta));
                None
            }
        };
        self.transcript_deadline = Some(Instant::now() + Duration::from_millis(750));
        previous
    }

    fn flush_transcript(&mut self) -> Option<ServerEvent> {
        self.transcript_deadline = None;
        self.transcript
            .take()
            .map(|(speaker, text)| transcript_event(speaker, text))
    }

    async fn next(&mut self) -> Option<Result<ServerEvent, ModelError>> {
        if let Some(event) = self.queued.pop_front() {
            return Some(Ok(event));
        }
        loop {
            let incoming = if let Some(deadline) = self.transcript_deadline {
                tokio::select! {
                    event = self.receive_json() => event,
                    () = tokio::time::sleep_until(deadline) => return self.flush_transcript().map(Ok),
                }
            } else {
                self.receive_json().await
            };
            let Some(incoming) = incoming else {
                return self.flush_transcript().map(Ok);
            };
            let event = match incoming {
                Ok(event) => event,
                Err(error) => return Some(Err(error)),
            };
            let parsed = match self.connection.protocol {
                RealtimeProtocol::OpenAiRealtime => ServerEvent::parse_realtime(&event),
                RealtimeProtocol::OpenAiLive => ServerEvent::parse_live(&event),
                // A call opens no transcription-only socket.
                RealtimeProtocol::DeepgramListen | RealtimeProtocol::ElevenLabsScribe => {
                    ServerEvent::Other
                }
            };
            if self.connection.protocol == RealtimeProtocol::OpenAiLive {
                if let Some(event) = self.accept_transcript(parsed)
                    && event != ServerEvent::Other
                {
                    return Some(Ok(event));
                }
            } else if parsed != ServerEvent::Other {
                return Some(Ok(parsed));
            }
        }
    }

    async fn finish(mut self) {
        if self.connection.protocol == RealtimeProtocol::OpenAiLive
            && self.started
            && self
                .send_json(json!({ "type": "session.close" }))
                .await
                .is_ok()
        {
            let finalized = tokio::time::timeout(Duration::from_secs(15), async {
                while let Some(event) = self.receive_json().await {
                    if matches!(
                        event,
                        Ok(ref value)
                            if value.get("type").and_then(Value::as_str)
                                == Some("session.closed")
                    ) {
                        return true;
                    }
                }
                false
            })
            .await
            .unwrap_or(false);
            if !finalized {
                tracing::debug!("the live model session did not finalize before close");
            }
        }
        if let Err(error) = self.connection.close().await {
            tracing::debug!(%error, "the model socket did not close cleanly");
        }
    }
}

#[async_trait]
impl ModelSession for RouterSession {
    async fn send(&mut self, command: ClientCommand) -> Result<(), ModelError> {
        RouterSession::send(self, command).await
    }

    async fn next(&mut self) -> Option<Result<ServerEvent, ModelError>> {
        RouterSession::next(self).await
    }

    async fn close(self: Box<Self>) {
        self.finish().await;
    }
}

fn live_responses(backend_model: &str, config: &SessionConfig) -> Value {
    json!({
        "model": backend_model,
        "instructions": config.instructions,
        "tools": function_tools(&config.tools),
        "tool_choice": "auto",
        "parallel_tool_calls": false,
    })
}

fn transcript_event(speaker: TranscriptSpeaker, text: String) -> ServerEvent {
    match speaker {
        TranscriptSpeaker::Caller => ServerEvent::CallerTranscript(text),
        TranscriptSpeaker::Agent => ServerEvent::AgentTranscript(text),
    }
}
