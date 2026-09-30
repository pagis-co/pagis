//! The model seam of the loop. The production brain is the LLM router;
//! tests script the brain (`pagis-testkit`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::{self, BoxStream};
use llm_router::{
    Candidate, ChatRequest, ContentPart, Message, ProviderConfig, Role, Router, RouterConfig,
    StreamEvent, Tool, ToolCall, ToolChoice,
};
pub use llm_router::{JsonSchemaFormat, Usage};
use pagis_broker::ToolDef;
use pagis_core::{Provider, ProviderKeys};

/// A model failure the run records as its error.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct BrainError {
    pub message: String,
    model_attempts: u32,
    refused_key: bool,
}

impl BrainError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            model_attempts: 0,
            refused_key: false,
        }
    }

    /// A failure where the provider refused the installation's key.
    pub fn refused_key(message: impl Into<String>) -> Self {
        Self {
            refused_key: true,
            ..Self::new(message)
        }
    }

    fn from_router(error: llm_router::Error) -> Self {
        Self {
            message: error.to_string(),
            model_attempts: error.attempts(),
            refused_key: error.refuses_key(),
        }
    }

    /// Whether the provider refused the installation's key, which only
    /// an Administrator can change.
    pub fn is_refused_key(&self) -> bool {
        self.refused_key
    }

    /// Router attempts made before this error reached the agent loop.
    pub fn model_attempts(&self) -> u32 {
        self.model_attempts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnRole {
    User,
    Assistant,
    /// A tool result answering one assistant tool call.
    Tool,
}

/// One tool call the model made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocation {
    pub id: String,
    pub name: String,
    /// The raw arguments JSON string.
    pub arguments: String,
}

/// One message in the turn context: prior history flattened to text
/// plus the images the model must see as `data:` URIs, and the
/// in-run transcript of tool calls and their results.
#[derive(Debug, Clone)]
pub struct TurnMessage {
    pub role: TurnRole,
    pub text: String,
    /// Images attached to the message, as `data:` URIs.
    pub images: Vec<String>,
    /// The tool calls an `Assistant` message made.
    pub tool_calls: Vec<ToolInvocation>,
    /// The call id a `Tool` message answers.
    pub tool_call_id: Option<String>,
    /// Trusted host metadata that tells compaction whether the tool
    /// result has a durable evidence reference. Providers do not read it.
    pub conversation_evidence: Option<serde_json::Value>,
}

impl TurnMessage {
    fn text(role: TurnRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            conversation_evidence: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::text(TurnRole::User, text)
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::text(TurnRole::Assistant, text)
    }

    pub fn assistant_with_calls(text: impl Into<String>, calls: Vec<ToolInvocation>) -> Self {
        Self {
            tool_calls: calls,
            ..Self::text(TurnRole::Assistant, text)
        }
    }

    pub fn tool_result(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            ..Self::text(TurnRole::Tool, text)
        }
    }

    pub fn with_conversation_evidence(mut self, status: Option<serde_json::Value>) -> Self {
        self.conversation_evidence = status;
        self
    }

    /// A tool result carrying screenshots as `data:` URIs.
    pub fn tool_result_with_images(
        call_id: impl Into<String>,
        text: impl Into<String>,
        images: Vec<String>,
    ) -> Self {
        Self {
            images,
            ..Self::tool_result(call_id, text)
        }
    }
}

/// One model turn: the rebuilt context for one streaming call.
#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub model_alias: String,
    /// Ordered `provider/model` candidates resolved for this run.
    pub model_candidates: Vec<String>,
    pub system: String,
    pub messages: Vec<TurnMessage>,
    /// The broker's tool list. Empty offers the model no tools.
    pub tools: Vec<ToolDef>,
    /// Offer the provider-native computer tool. The production
    /// brain appends the serving provider's own tool shape.
    pub computer: bool,
    /// Whether the model may call a tool. `false` keeps the tools in the
    /// request, because a provider refuses a history that calls a tool
    /// the request lacks, and forbids a new call (`tool_choice: none`).
    pub allow_tool_calls: bool,
    /// The longest answer the provider may write, in tokens. `None`
    /// takes the provider's own default, which is a few thousand tokens
    /// and cuts a long structured answer.
    pub max_output_tokens: Option<u32>,
    /// A provider-native JSON Schema contract for the final text, when this
    /// turn feeds a typed daemon workflow instead of a conversation.
    pub output_schema: Option<JsonSchemaFormat>,
}

/// The final metadata of a finished turn.
#[derive(Debug, Clone, Default)]
pub struct TurnEnd {
    pub stop_reason: String,
    /// The provider and concrete model that served this turn.
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Provider-reported usage. `None` means unknown, never zero.
    pub usage: Option<Usage>,
    /// Cost from the serving router's model metadata. `None` means unknown.
    pub estimated_cost_usd: Option<f64>,
}

/// One streamed item: a markdown text delta, a complete tool call, or
/// the end of the turn. Tool calls arrive whole, before `Finish`.
#[derive(Debug, Clone)]
pub enum TurnDelta {
    Text(String),
    ToolCall(ToolInvocation),
    /// Provider retries that the router absorbed before it opened the
    /// response stream.
    ModelRetries(u32),
    Finish(TurnEnd),
}

pub type TurnStream = BoxStream<'static, Result<TurnDelta, BrainError>>;

/// What thinks for an agent. Always streaming: one call, one
/// stream of deltas, cancel by dropping the stream.
#[async_trait]
pub trait Brain: Send + Sync {
    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError>;

    /// Whether the brain can think at all. A brain over provider keys
    /// cannot while no provider holds a key, and the scheduler then
    /// starts no proactive Run: a Schedule or an arrival that finds no
    /// key waits for one, and never fails as work that needs the person.
    fn ready(&self) -> bool {
        true
    }
}

/// The production brain: `llm-router` over the provider key resolver.
/// Keys resolve on every turn, so a key the onboarding wizard
/// stores takes effect without a restart; the router rebuilds only
/// when the resolved keys change.
/// The cached router build: the router plus the candidates it serves,
/// in route order. Each one decides its own computer tool's shape and
/// whether that model carries the tool at all.
type RouterEntry = (Arc<Router>, Vec<(Provider, String)>);

pub struct RouterBrain {
    keys: Arc<ProviderKeys>,
    /// The Provider Model Lists, which price a model the table does not
    /// know.
    models: Arc<crate::ModelCatalog>,
    cache: Mutex<Option<(u64, RouterEntry)>>,
    /// Base URL overrides per provider. Production uses provider defaults;
    /// integration tests point one provider at a local server.
    base_urls: HashMap<Provider, String>,
    /// The models whose API refused the native computer tool, for the
    /// life of the process. They get the portable function tool.
    refused_native_computer: Mutex<HashSet<(Provider, String)>>,
}

impl RouterBrain {
    pub fn new(keys: Arc<ProviderKeys>, models: Arc<crate::ModelCatalog>) -> Self {
        Self {
            keys,
            models,
            cache: Mutex::new(None),
            base_urls: HashMap::new(),
            refused_native_computer: Mutex::new(HashSet::new()),
        }
    }

    pub fn with_base_url(mut self, provider: Provider, base_url: impl Into<String>) -> Self {
        self.base_urls.insert(provider, base_url.into());
        self
    }

    /// The router for the current keys (rebuilt when they change) and
    /// the candidates it serves, each with its own computer tool.
    fn current_router(
        &self,
        alias: &str,
        configured: &[String],
    ) -> Result<RouterEntry, BrainError> {
        let key_fingerprint = self
            .keys
            .fingerprint()
            .map_err(|err| BrainError::new(err.to_string()))?;
        let mut hasher = DefaultHasher::new();
        key_fingerprint.hash(&mut hasher);
        alias.hash(&mut hasher);
        configured.hash(&mut hasher);
        let fingerprint = hasher.finish();
        let mut cache = self.cache.lock().expect("router cache lock");
        if let Some((cached, entry)) = cache.as_ref()
            && *cached == fingerprint
        {
            return Ok(entry.clone());
        }

        let mut config = RouterConfig::new();
        let mut anthropic_available = false;
        let mut openai_available = false;
        let mut openrouter_available = false;
        for provider in pagis_core::PROVIDERS {
            let Some((key, _)) = self
                .keys
                .resolve(provider)
                .map_err(|err| BrainError::new(err.to_string()))?
            else {
                continue;
            };
            config = config.provider(
                provider.id(),
                provider_config(provider, key, self.base_urls.get(&provider)),
            );
            match provider {
                Provider::Anthropic => anthropic_available = true,
                Provider::OpenAi => openai_available = true,
                Provider::OpenRouter => openrouter_available = true,
            }
        }
        if !anthropic_available && !openai_available && !openrouter_available {
            return Err(BrainError::new(
                "no model provider is configured; add a key in onboarding \
                 or set ANTHROPIC_API_KEY, OPENAI_API_KEY, or OPENROUTER_API_KEY"
                    .to_string(),
            ));
        }
        let mut candidates = Vec::new();
        let mut served = Vec::new();
        for candidate in configured {
            let Some((provider_id, model)) = candidate.split_once('/') else {
                return Err(BrainError::new(format!(
                    "model candidate `{candidate}` must use provider/model format"
                )));
            };
            let provider = Provider::from_id(provider_id).ok_or_else(|| {
                BrainError::new(format!("unknown model provider `{provider_id}`"))
            })?;
            let available = match provider {
                Provider::Anthropic => anthropic_available,
                Provider::OpenAi => openai_available,
                Provider::OpenRouter => openrouter_available,
            };
            if available {
                served.push((provider, model.to_string()));
                candidates.push(Candidate::new(provider_id, model));
            }
        }
        if candidates.is_empty() {
            return Err(BrainError::new(format!(
                "model alias `{alias}` has no candidate with a configured provider"
            )));
        }
        config = config.model(alias, candidates);
        // Building the local route makes no provider attempt. Keep that
        // distinct from a provider error returned after `Router::run` starts.
        let router =
            Arc::new(Router::new(config).map_err(|error| BrainError::new(error.to_string()))?);
        let entry = (Arc::clone(&router), served);
        *cache = Some((fingerprint, entry.clone()));
        Ok(entry)
    }
}

/// The router entry of one provider: its protocol, key and base URL.
/// The brain and the model list reach a provider the same way.
pub(crate) fn provider_config(
    provider: Provider,
    key: String,
    base_url: Option<&String>,
) -> ProviderConfig {
    let mut config = match provider {
        Provider::Anthropic => {
            // The beta header that serves `computer_20251124`.
            let mut config = ProviderConfig::anthropic(key);
            config.headers.insert(
                "anthropic-beta".to_string(),
                "computer-use-2025-11-24".to_string(),
            );
            config
        }
        // Every turn offers the computer tool (`run.rs`), which is a
        // provider-defined tool the Chat Completions codec rejects
        // outright; the Responses API is the only OpenAI protocol that
        // carries it.
        Provider::OpenAi => ProviderConfig::openai_responses(key),
        Provider::OpenRouter => ProviderConfig::openrouter(key),
    };
    if let Some(base_url) = base_url {
        config.base_url = base_url.clone();
    }
    config
}

/// The portable computer function: one shape for each action, as the
/// OpenAI computer tool has. Each shape requires every field it has and
/// allows no other, so a model sends what the action needs and no
/// placeholders. OpenAI asks for an object at the root of a function's
/// parameters, so the shapes sit under `action`.
fn portable_computer_tool() -> Tool {
    fn shape(kind: &str, fields: serde_json::Value) -> serde_json::Value {
        let mut properties = serde_json::Map::new();
        properties.insert(
            "type".to_string(),
            serde_json::json!({ "type": "string", "enum": [kind] }),
        );
        properties.extend(fields.as_object().cloned().unwrap_or_default());
        let required: Vec<String> = properties.keys().cloned().collect();
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        })
    }
    let x = serde_json::json!({
        "type": "number",
        "description": "Pixels from the left edge of the screenshot."
    });
    let y = serde_json::json!({
        "type": "number",
        "description": "Pixels from the top edge of the screenshot."
    });
    let point = serde_json::json!({ "x": x, "y": y });
    let with_point = |fields: serde_json::Value| {
        let mut all = point.as_object().cloned().unwrap_or_default();
        all.extend(fields.as_object().cloned().unwrap_or_default());
        serde_json::Value::Object(all)
    };
    Tool::function(
        "computer",
        "Use the computer one action at a time. Coordinates are pixels on the 1280 by 720 \
         screenshot. Each result includes a screenshot taken after the screen stops changing.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "anyOf": [
                        shape("screenshot", serde_json::json!({})),
                        shape("click", with_point(serde_json::json!({
                            "button": {
                                "type": "string",
                                "enum": ["left", "right", "middle"]
                            }
                        }))),
                        shape("double_click", point.clone()),
                        shape("move", point.clone()),
                        shape("drag", serde_json::json!({
                            "path": {
                                "type": "array",
                                "description": "The points of the drag, from start to end.",
                                "minItems": 2,
                                "items": {
                                    "type": "object",
                                    "properties": { "x": x, "y": y },
                                    "required": ["x", "y"],
                                    "additionalProperties": false
                                }
                            }
                        })),
                        shape("scroll", with_point(serde_json::json!({
                            "scroll_x": {
                                "type": "number",
                                "description": "Horizontal distance in pixels; negative scrolls left."
                            },
                            "scroll_y": {
                                "type": "number",
                                "description": "Vertical distance in pixels; negative scrolls up."
                            }
                        }))),
                        shape("type", serde_json::json!({
                            "text": {
                                "type": "string",
                                "minLength": 1,
                                "description": "The text to type where the focus is. Click the field first."
                            }
                        })),
                        shape("keypress", serde_json::json!({
                            "keys": {
                                "type": "array",
                                "minItems": 1,
                                "description": "The keys to press together, for example [\"ENTER\"] or [\"CTRL\", \"A\"].",
                                "items": { "type": "string" }
                            }
                        })),
                        shape("wait", serde_json::json!({}))
                    ]
                }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
    )
}

/// One candidate's own computer tool. Claude gets Anthropic's tool. An
/// OpenAI model gets OpenAI's tool, directly or through OpenRouter,
/// unless its API refused it: no provider's model list says which models
/// take it, and the API does, with a 400 (see [`refused_computer_model`]).
/// Every other model gets the portable function tool, which any model
/// that takes tool calls and images drives. The display size matches the
/// screenshots the executor sends.
fn computer_tool(provider: Provider, native: bool) -> Tool {
    match provider {
        Provider::Anthropic => {
            let mut config = serde_json::Map::new();
            config.insert(
                "display_width_px".to_string(),
                pagis_computer::exec::DISPLAY_WIDTH.into(),
            );
            config.insert(
                "display_height_px".to_string(),
                pagis_computer::exec::DISPLAY_HEIGHT.into(),
            );
            Tool::provider_defined("computer_20251124", "computer", config)
        }
        // The GA `computer` tool takes no display or environment config
        // (unlike the deprecated `computer_use_preview` type) — just the
        // bare tool type.
        Provider::OpenAi | Provider::OpenRouter if native => {
            Tool::provider_defined("computer", "computer", serde_json::Map::new())
        }
        Provider::OpenAi | Provider::OpenRouter => portable_computer_tool(),
    }
}

/// Whether `model` may carry OpenAI's native computer tool: an OpenAI
/// model, directly or as OpenRouter's `openai/...`.
fn openai_model(provider: Provider, model: &str) -> bool {
    match provider {
        Provider::OpenAi => true,
        Provider::OpenRouter => model.starts_with("openai/"),
        Provider::Anthropic => false,
    }
}

/// The candidate whose API refused the native computer tool, when
/// `error` is that refusal: a 400 on `tools` that names the `computer`
/// tool and the model ("Tool 'computer' is not supported with gpt-4.1.").
/// OpenAI sends no error code for it.
fn refused_computer_model<'a>(
    error: &llm_router::Error,
    candidates: &'a [(Provider, String)],
) -> Option<&'a (Provider, String)> {
    let error = match error {
        llm_router::Error::Exhausted { last, .. } => last.as_ref(),
        error => error,
    };
    let llm_router::Error::Provider {
        provider,
        status: 400,
        message,
        raw,
        ..
    } = error
    else {
        return None;
    };
    let on_tools = raw
        .as_ref()
        .and_then(|raw| raw.pointer("/error/param"))
        .and_then(serde_json::Value::as_str)
        == Some("tools");
    if !on_tools || !message.contains("Tool 'computer'") {
        return None;
    }
    candidates.iter().find(|(candidate, model)| {
        candidate.id() == provider && message.contains(model.rsplit('/').next().unwrap_or(model))
    })
}

/// Text plus image-URL parts; an empty text adds no part.
fn content_parts(text: String, images: Vec<String>) -> Vec<ContentPart> {
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(ContentPart::Text { text });
    }
    content.extend(images.into_iter().map(|url| ContentPart::ImageUrl { url }));
    content
}

fn to_router_message(message: TurnMessage) -> Message {
    match message.role {
        // A tool result with images (a computer screenshot)
        // carries them as image parts beside any text.
        TurnRole::Tool if !message.images.is_empty() => {
            let mut content: Vec<ContentPart> = message
                .images
                .into_iter()
                .map(|url| ContentPart::ImageUrl { url })
                .collect();
            if !message.text.is_empty() {
                content.insert(0, ContentPart::Text { text: message.text });
            }
            Message {
                role: Role::Tool,
                content,
                tool_calls: Vec::new(),
                tool_call_id: message.tool_call_id,
            }
        }
        TurnRole::User => Message {
            role: Role::User,
            content: content_parts(message.text, message.images),
            tool_calls: Vec::new(),
            tool_call_id: None,
        },
        TurnRole::Assistant => Message {
            role: Role::Assistant,
            content: content_parts(message.text, message.images),
            tool_calls: message
                .tool_calls
                .into_iter()
                .map(|call| ToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                })
                .collect(),
            tool_call_id: None,
        },
        TurnRole::Tool => Message::tool(message.tool_call_id.unwrap_or_default(), message.text),
    }
}

/// Folds a router event stream into [`TurnDelta`]s: text passes through
/// live, tool-call fragments accumulate by stream index and flush as
/// whole calls at `Finish`.
#[derive(Default)]
struct ToolCallFold {
    calls: BTreeMap<u32, ToolInvocation>,
    serving: Option<ServingModel>,
}

struct ServingModel {
    provider: String,
    model: String,
    metadata: llm_router::ModelMetadata,
}

impl ToolCallFold {
    fn serving(provider: String, model: String, metadata: llm_router::ModelMetadata) -> Self {
        Self {
            calls: BTreeMap::new(),
            serving: Some(ServingModel {
                provider,
                model,
                metadata,
            }),
        }
    }

    fn fold(
        &mut self,
        event: Result<StreamEvent, BrainError>,
    ) -> Vec<Result<TurnDelta, BrainError>> {
        match event {
            Ok(StreamEvent::TextDelta { text }) => vec![Ok(TurnDelta::Text(text))],
            Ok(StreamEvent::ToolCallStart { index, id, name }) => {
                self.calls.entry(index).or_insert(ToolInvocation {
                    id,
                    name,
                    arguments: String::new(),
                });
                Vec::new()
            }
            Ok(StreamEvent::ToolCallDelta { index, arguments }) => {
                if let Some(call) = self.calls.get_mut(&index) {
                    call.arguments.push_str(&arguments);
                }
                Vec::new()
            }
            Ok(StreamEvent::Finish { reason, usage, .. }) => {
                let mut deltas: Vec<Result<TurnDelta, BrainError>> =
                    std::mem::take(&mut self.calls)
                        .into_values()
                        .map(|call| Ok(TurnDelta::ToolCall(call)))
                        .collect();
                deltas.push(Ok(TurnDelta::Finish(TurnEnd {
                    stop_reason: format!("{reason:?}").to_lowercase(),
                    provider: self
                        .serving
                        .as_ref()
                        .map(|serving| serving.provider.clone()),
                    model: self.serving.as_ref().map(|serving| serving.model.clone()),
                    estimated_cost_usd: usage.as_ref().and_then(|usage| {
                        self.serving
                            .as_ref()
                            .and_then(|serving| serving.metadata.cost(usage))
                    }),
                    usage,
                })));
                deltas
            }
            // A Run has no use for reasoning and audio events.
            Ok(_) => Vec::new(),
            Err(err) => vec![Err(err)],
        }
    }
}

#[async_trait]
impl Brain for RouterBrain {
    fn ready(&self) -> bool {
        pagis_core::PROVIDERS
            .into_iter()
            .any(|provider| matches!(self.keys.resolve(provider), Ok(Some(_))))
    }

    async fn turn(&self, request: TurnRequest) -> Result<TurnStream, BrainError> {
        if request.messages.is_empty() {
            return Err(BrainError::new("model request has no messages"));
        }
        let (router, served) =
            self.current_router(&request.model_alias, &request.model_candidates)?;

        let mut messages = vec![Message::system(request.system)];
        messages.extend(request.messages.into_iter().map(to_router_message));
        let mut chat = ChatRequest::new(request.model_alias, messages);
        chat.max_tokens = request.max_output_tokens;
        chat.output_schema = request.output_schema;
        chat.tools = request
            .tools
            .into_iter()
            .map(|tool| Tool::function(tool.name, tool.description, tool.parameters))
            .collect();
        // Each candidate gets its own computer tool, because each
        // provider names the tool with its own wire type. A candidate the
        // request falls back to never receives another provider's tool.
        // A model that refuses the native tool repeats the turn once with
        // the portable one.
        let tools = chat.tools.clone();
        let mut refused = false;
        let stream = loop {
            chat.tools = tools.clone();
            if request.computer {
                for (provider, model) in &served {
                    let native = openai_model(*provider, model)
                        && !self
                            .refused_native_computer
                            .lock()
                            .expect("refused computer lock")
                            .contains(&(*provider, model.clone()));
                    chat.tools.push(
                        computer_tool(*provider, native)
                            .for_candidate(provider.id(), model.clone()),
                    );
                }
            }
            if !request.allow_tool_calls && !chat.tools.is_empty() {
                chat.tool_choice = Some(ToolChoice::None);
            }
            match router.chat_stream(&chat).await {
                Ok(stream) => break stream,
                Err(error) if request.computer && !refused => {
                    let Some(candidate) = refused_computer_model(&error, &served) else {
                        return Err(BrainError::from_router(error));
                    };
                    tracing::info!(
                        provider = candidate.0.id(),
                        model = %candidate.1,
                        "the model refused the native computer tool; it gets the portable tool"
                    );
                    self.refused_native_computer
                        .lock()
                        .expect("refused computer lock")
                        .insert(candidate.clone());
                    refused = true;
                }
                Err(error) => return Err(BrainError::from_router(error)),
            }
        };

        let retries = stream.retries;
        let provider = stream.provider;
        let model = stream.model;
        let events = stream.events;
        let metadata =
            router.model_metadata(&model, self.models.listed(&provider, &model).as_ref());
        let mut fold = ToolCallFold::serving(provider, model, metadata);
        let retry_delta =
            futures::stream::iter((retries > 0).then_some(Ok(TurnDelta::ModelRetries(retries))));
        let events = events
            .map(|event| event.map_err(|err| BrainError::new(err.to_string())))
            .map(move |event| stream::iter(fold.fold(event)))
            .flatten();
        Ok(retry_delta.chain(events).boxed())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use futures::StreamExt;
    use pagis_core::MemorySecretStore;
    use serde_json::{Value, json};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn keys() -> Arc<ProviderKeys> {
        Arc::new(ProviderKeys::with_env(
            |_| None,
            HashMap::new(),
            Arc::new(MemorySecretStore::default()),
        ))
    }

    fn catalog() -> Arc<crate::ModelCatalog> {
        Arc::new(crate::ModelCatalog::new(keys()))
    }

    fn request() -> TurnRequest {
        TurnRequest {
            model_alias: "default".to_string(),
            model_candidates: vec!["anthropic/claude-sonnet-4-6".to_string()],
            system: "system".to_string(),
            messages: vec![TurnMessage::user("hello")],
            tools: vec![],
            computer: false,
            allow_tool_calls: true,
            max_output_tokens: None,
            output_schema: None,
        }
    }

    #[tokio::test]
    async fn a_turn_without_messages_fails_before_provider_setup() {
        let brain = RouterBrain::new(keys(), catalog());
        let mut request = request();
        request.messages.clear();

        let err = brain.turn(request).await.err().expect("turn fails");

        assert_eq!(err.message, "model request has no messages");
    }

    #[tokio::test]
    async fn turn_without_keys_fails_with_a_clear_error() {
        let brain = RouterBrain::new(keys(), catalog());

        let err = brain.turn(request()).await.err().expect("turn fails");

        assert!(
            err.message.contains("no model provider is configured"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_stored_key_makes_the_router_available() {
        let keys = keys();
        let brain = RouterBrain::new(Arc::clone(&keys), catalog());
        assert!(
            brain
                .current_router("default", &request().model_candidates)
                .is_err()
        );

        keys.set(Provider::Anthropic, "sk-test").unwrap();

        assert!(
            brain
                .current_router("default", &request().model_candidates)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn openrouter_gpt_6_astra_round_trips_native_computer_use() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"computer_call\",\"call_id\":\"call_1\",\"actions\":[]}}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"computer_call\",\"call_id\":\"call_1\",\"actions\":[{\"type\":\"screenshot\"}],\"status\":\"completed\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":6}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("authorization", "Bearer sk-openrouter"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse, "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let keys = keys();
        keys.set(Provider::OpenRouter, "sk-openrouter").unwrap();
        let brain =
            RouterBrain::new(keys, catalog()).with_base_url(Provider::OpenRouter, server.uri());
        let mut request = request();
        request.model_candidates = vec!["openrouter/openai/gpt-6-astra".to_string()];
        request.computer = true;

        let deltas: Vec<TurnDelta> = brain
            .turn(request)
            .await
            .unwrap()
            .map(|item| item.unwrap())
            .collect()
            .await;

        let call = deltas
            .iter()
            .find_map(|delta| match delta {
                TurnDelta::ToolCall(call) => Some(call),
                _ => None,
            })
            .expect("computer call");
        assert_eq!(call.id, "call_1");
        assert_eq!(call.name, "computer");
        assert_eq!(
            serde_json::from_str::<Value>(&call.arguments).unwrap()["actions"][0]["type"],
            "screenshot"
        );

        let sent: Value = server.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        assert_eq!(sent["model"], "openai/gpt-6-astra");
        assert_eq!(sent["tools"][0], json!({"type": "computer"}));
    }

    #[tokio::test]
    async fn a_turn_that_allows_no_tool_calls_keeps_the_tools_and_sends_tool_choice_none() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"Found it.\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":3}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse, "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let keys = keys();
        keys.set(Provider::OpenAi, "sk-openai").unwrap();
        let brain = RouterBrain::new(keys, catalog()).with_base_url(Provider::OpenAi, server.uri());
        let mut request = request();
        request.model_candidates = vec!["openai/gpt-6-luna".to_string()];
        request.computer = true;
        request.allow_tool_calls = false;

        let _: Vec<_> = brain.turn(request).await.unwrap().collect().await;

        let sent: Value = server.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        assert_eq!(sent["tools"][0], json!({"type": "computer"}));
        assert_eq!(sent["tool_choice"], "none");
    }

    #[tokio::test]
    async fn a_turn_that_allows_tool_calls_sends_no_tool_choice() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"Hi.\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse, "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let keys = keys();
        keys.set(Provider::OpenAi, "sk-openai").unwrap();
        let brain = RouterBrain::new(keys, catalog()).with_base_url(Provider::OpenAi, server.uri());
        let mut request = request();
        request.model_candidates = vec!["openai/gpt-6-luna".to_string()];
        request.computer = true;

        let _: Vec<_> = brain.turn(request).await.unwrap().collect().await;

        let sent: Value = server.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        assert!(sent.get("tool_choice").is_none(), "{sent}");
    }

    #[tokio::test]
    async fn a_fallback_to_openai_sends_the_openai_computer_tool() {
        // Anthropic answers overloaded, which the router passes to the
        // next candidate at once. A 500 would first retry Anthropic
        // with real backoffs of 10 s and 20 s.
        let anthropic = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(529).set_body_json(json!({
                "type": "error",
                "error": {"type": "overloaded_error", "message": "Overloaded"}
            })))
            .mount(&anthropic)
            .await;
        let openai = MockServer::start().await;
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse, "text/event-stream"),
            )
            .expect(1)
            .mount(&openai)
            .await;

        let keys = keys();
        keys.set(Provider::Anthropic, "sk-anthropic").unwrap();
        keys.set(Provider::OpenAi, "sk-openai").unwrap();
        let brain = RouterBrain::new(keys, catalog())
            .with_base_url(Provider::Anthropic, anthropic.uri())
            .with_base_url(Provider::OpenAi, openai.uri());
        let mut request = request();
        request.model_candidates = vec![
            "anthropic/claude-sonnet-4-6".to_string(),
            "openai/gpt-5.6-luna".to_string(),
        ];
        request.computer = true;

        let deltas: Vec<TurnDelta> = brain
            .turn(request)
            .await
            .unwrap()
            .map(|item| item.unwrap())
            .collect()
            .await;
        assert!(
            deltas
                .iter()
                .any(|delta| matches!(delta, TurnDelta::Finish(_)))
        );

        let to_anthropic: Value = anthropic.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        assert!(
            to_anthropic["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["type"] == "computer_20251124")
        );
        let to_openai: Value = openai.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        let tools = to_openai["tools"].as_array().unwrap();
        assert!(tools.contains(&json!({"type": "computer"})), "{to_openai}");
        assert!(
            !tools.iter().any(|tool| tool["type"] == "computer_20251124"),
            "{to_openai}"
        );
    }

    /// A model that the provider lists with a price, and that no table
    /// knows, is priced from the provider's list.
    #[tokio::test]
    async fn a_listed_price_prices_a_model_the_table_does_not_know() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{
                "id": "vendor/new-model",
                "created": 1,
                "pricing": {"prompt": "0.000002", "completion": "0.00001"}
            }]})))
            .mount(&server)
            .await;
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1000000,\"output_tokens\":100000}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse, "text/event-stream"),
            )
            .mount(&server)
            .await;
        let keys = keys();
        keys.set(Provider::OpenRouter, "sk-openrouter").unwrap();
        let models = Arc::new(
            crate::ModelCatalog::new(Arc::clone(&keys))
                .with_base_url(Provider::OpenRouter, server.uri()),
        );
        models.models(Provider::OpenRouter).await.unwrap();
        let brain =
            RouterBrain::new(keys, models).with_base_url(Provider::OpenRouter, server.uri());
        let mut request = request();
        request.model_candidates = vec!["openrouter/vendor/new-model".to_string()];

        let deltas: Vec<TurnDelta> = brain
            .turn(request)
            .await
            .unwrap()
            .map(|item| item.unwrap())
            .collect()
            .await;

        let end = deltas
            .iter()
            .find_map(|delta| match delta {
                TurnDelta::Finish(end) => Some(end),
                _ => None,
            })
            .expect("the turn finishes");
        // 1M input at $2/M plus 100k output at $10/M.
        let cost = end.estimated_cost_usd.expect("a listed price");
        assert!((cost - 3.0).abs() < 1e-9, "{cost}");
    }

    /// The portable tool has one shape for each action, as OpenAI's own
    /// computer tool does. Every field of a shape is required and no
    /// other field is allowed, so a model sends what the action needs
    /// and no placeholders: a key press names a key, and a type action
    /// has text and no coordinates.
    #[test]
    fn the_portable_computer_tool_has_one_shape_for_each_action() {
        let parameters = portable_computer_tool().parameters;
        assert_eq!(parameters["type"], "object");
        assert_eq!(parameters["required"], json!(["action"]));
        let shapes = parameters["properties"]["action"]["anyOf"]
            .as_array()
            .expect("the action is one of several shapes");
        let shape = |kind: &str| {
            shapes
                .iter()
                .find(|shape| shape["properties"]["type"]["enum"] == json!([kind]))
                .unwrap_or_else(|| panic!("no shape for {kind}"))
        };
        for shape in shapes {
            let mut required: Vec<&str> = shape["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|field| field.as_str().unwrap())
                .collect();
            let mut fields: Vec<&str> = shape["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            required.sort_unstable();
            fields.sort_unstable();
            assert_eq!(required, fields, "{shape}");
            assert_eq!(shape["additionalProperties"], false, "{shape}");
        }
        assert_eq!(shape("keypress")["properties"]["keys"]["minItems"], 1);
        assert_eq!(shape("type")["properties"]["text"]["minLength"], 1);
        assert!(shape("type")["properties"].get("x").is_none());
        assert_eq!(shape("screenshot")["required"], json!(["type"]));
    }

    #[tokio::test]
    async fn openrouter_other_models_get_a_portable_computer_function() {
        let server = MockServer::start().await;
        let sse = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_2\",\"name\":\"computer\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"action\\\":{\\\"type\\\":\\\"click\\\",\\\"button\\\":\\\"left\\\",\\\"x\\\":100,\\\"y\\\":200}}\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":8,\"output_tokens\":4}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse, "text/event-stream"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let keys = keys();
        keys.set(Provider::OpenRouter, "sk-openrouter").unwrap();
        let brain =
            RouterBrain::new(keys, catalog()).with_base_url(Provider::OpenRouter, server.uri());
        let mut request = request();
        request.model_candidates = vec!["openrouter/anthropic/claude-sonnet-4.6".to_string()];
        request.computer = true;
        request.tools = vec![ToolDef {
            name: "memory_search".to_string(),
            description: "Search memory".to_string(),
            parameters: json!({"type": "object"}),
        }];

        let deltas: Vec<TurnDelta> = brain
            .turn(request)
            .await
            .unwrap()
            .map(|item| item.unwrap())
            .collect()
            .await;
        let call = deltas
            .iter()
            .find_map(|delta| match delta {
                TurnDelta::ToolCall(call) => Some(call),
                _ => None,
            })
            .expect("function call");
        assert_eq!(call.name, "computer");
        assert_eq!(
            call.arguments,
            r#"{"action":{"type":"click","button":"left","x":100,"y":200}}"#
        );

        let sent: Value = server.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        assert_eq!(sent["model"], "anthropic/claude-sonnet-4.6");
        assert_eq!(sent["tools"].as_array().unwrap().len(), 2);
        assert_eq!(sent["tools"][0]["type"], "function");
        assert_eq!(sent["tools"][0]["name"], "memory_search");
        assert_eq!(sent["tools"][1]["type"], "function");
        assert_eq!(sent["tools"][1]["name"], "computer");
        assert_eq!(
            sent["tools"][1]["parameters"]["required"],
            json!(["action"])
        );
        assert!(
            sent["tools"][1]["parameters"]["properties"]["action"]["anyOf"]
                .as_array()
                .unwrap()
                .iter()
                .any(|shape| shape["properties"]["type"]["enum"] == json!(["screenshot"]))
        );
    }

    #[test]
    fn the_fold_flushes_accumulated_tool_calls_at_finish() {
        let mut fold = ToolCallFold::default();

        assert!(matches!(
            fold.fold(Ok(StreamEvent::TextDelta {
                text: "One sec.".to_string()
            }))[0],
            Ok(TurnDelta::Text(_))
        ));
        assert!(
            fold.fold(Ok(StreamEvent::ToolCallStart {
                index: 0,
                id: "call_1".to_string(),
                name: "host_shell".to_string(),
            }))
            .is_empty()
        );
        assert!(
            fold.fold(Ok(StreamEvent::ToolCallDelta {
                index: 0,
                arguments: r#"{"command":"#.to_string(),
            }))
            .is_empty()
        );
        assert!(
            fold.fold(Ok(StreamEvent::ToolCallDelta {
                index: 0,
                arguments: r#" "ls"}"#.to_string(),
            }))
            .is_empty()
        );

        let end = fold.fold(Ok(StreamEvent::Finish {
            reason: llm_router::FinishReason::ToolCalls,
            native_reason: None,
            usage: None,
        }));
        assert_eq!(end.len(), 2);
        let Ok(TurnDelta::ToolCall(call)) = &end[0] else {
            panic!("first delta must be the tool call");
        };
        assert_eq!(call.id, "call_1");
        assert_eq!(call.name, "host_shell");
        assert_eq!(call.arguments, r#"{"command": "ls"}"#);
        assert!(matches!(&end[1], Ok(TurnDelta::Finish(_))));
    }

    #[test]
    fn assistant_tool_calls_and_results_map_to_router_messages() {
        let assistant = to_router_message(TurnMessage::assistant_with_calls(
            "",
            vec![ToolInvocation {
                id: "call_1".to_string(),
                name: "host_shell".to_string(),
                arguments: r#"{"command": "ls"}"#.to_string(),
            }],
        ));
        assert_eq!(assistant.role, Role::Assistant);
        assert!(assistant.content.is_empty());
        assert_eq!(assistant.tool_calls[0].id, "call_1");

        let result = to_router_message(TurnMessage::tool_result("call_1", "exit code: 0"));
        assert_eq!(result.role, Role::Tool);
        assert_eq!(result.tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(result.text_content(), "exit code: 0");
    }

    #[test]
    fn conversation_evidence_status_does_not_enter_the_provider_message() {
        let result = to_router_message(
            TurnMessage::tool_result("call_1", r#"{"body":"exact provider text"}"#)
                .with_conversation_evidence(Some(serde_json::json!({
                    "status": "retained",
                    "reference": "tool:internal-run:internal-call",
                    "grant_revision": 7
                }))),
        );

        assert_eq!(result.text_content(), r#"{"body":"exact provider text"}"#);
        let encoded = format!("{result:?}");
        assert!(!encoded.contains("internal-run"));
        assert!(!encoded.contains("grant_revision"));
    }

    /// The tools the OpenAI Responses endpoint received for one turn on
    /// `model` with the computer on.
    async fn openai_tools_for(model: &str) -> Vec<Value> {
        let openai = MockServer::start().await;
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse, "text/event-stream"),
            )
            .mount(&openai)
            .await;
        let keys = keys();
        keys.set(Provider::OpenAi, "sk-openai").unwrap();
        let brain = RouterBrain::new(keys, catalog()).with_base_url(Provider::OpenAi, openai.uri());
        let mut request = request();
        request.model_candidates = vec![format!("openai/{model}")];
        request.computer = true;

        let _: Vec<_> = brain.turn(request).await.unwrap().collect().await;

        let sent: Value = openai.received_requests().await.unwrap()[0]
            .body_json()
            .unwrap();
        sent["tools"].as_array().cloned().unwrap_or_default()
    }

    /// An OpenAI model gets OpenAI's own computer tool, including a model
    /// newer than any table: the API says whether a model takes it.
    #[tokio::test]
    async fn a_new_openai_model_gets_the_native_computer_tool() {
        let tools = openai_tools_for("gpt-unlisted").await;

        assert_eq!(tools, vec![json!({"type": "computer"})]);
    }

    fn rejection(model: &str) -> ResponseTemplate {
        ResponseTemplate::new(400).set_body_json(json!({
            "error": {
                "message": format!("Tool 'computer' is not supported with {model}."),
                "type": "invalid_request_error",
                "param": "tools",
                "code": null
            }
        }))
    }

    fn text_stream() -> ResponseTemplate {
        let sse = concat!(
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n",
        );
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_raw(sse, "text/event-stream")
    }

    /// A model the API refuses the native computer tool for gets the
    /// portable function tool: the turn repeats at once with it, and the
    /// next turn sends it first.
    #[tokio::test]
    async fn a_model_that_refuses_the_native_computer_tool_gets_the_portable_one() {
        let openai = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(rejection("gpt-4.1"))
            .up_to_n_times(1)
            .mount(&openai)
            .await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(text_stream())
            .mount(&openai)
            .await;
        let keys = keys();
        keys.set(Provider::OpenAi, "sk-openai").unwrap();
        let brain = RouterBrain::new(keys, catalog()).with_base_url(Provider::OpenAi, openai.uri());
        let mut request = request();
        request.model_candidates = vec!["openai/gpt-4.1".to_string()];
        request.computer = true;

        let first: Vec<_> = brain.turn(request.clone()).await.unwrap().collect().await;
        let second: Vec<_> = brain.turn(request).await.unwrap().collect().await;

        assert!(first.iter().chain(&second).all(Result::is_ok));
        let tools: Vec<Value> = openai
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|sent| sent.body_json::<Value>().unwrap()["tools"][0].clone())
            .collect();
        assert_eq!(tools.len(), 3, "one refusal, then one request per turn");
        assert_eq!(tools[0], json!({"type": "computer"}));
        assert_eq!(tools[1]["type"], "function");
        assert_eq!(tools[1]["name"], "computer");
        assert_eq!(tools[2], tools[1]);
    }

    /// An error about another tool is not a refusal of the computer tool.
    #[tokio::test]
    async fn an_error_about_another_tool_fails_the_turn() {
        let openai = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {
                    "message": "Invalid schema for function 'memory_search'.",
                    "type": "invalid_request_error",
                    "param": "tools",
                    "code": null
                }
            })))
            .expect(1)
            .mount(&openai)
            .await;
        let keys = keys();
        keys.set(Provider::OpenAi, "sk-openai").unwrap();
        let brain = RouterBrain::new(keys, catalog()).with_base_url(Provider::OpenAi, openai.uri());
        let mut request = request();
        request.model_candidates = vec!["openai/gpt-4.1".to_string()];
        request.computer = true;

        assert!(brain.turn(request).await.is_err());
    }
}
