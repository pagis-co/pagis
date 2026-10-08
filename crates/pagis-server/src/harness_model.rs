//! The Harness Model Endpoint (ADR-0033): the model API that the daemon
//! serves to the harness of a Coding Session in a Computer.
//!
//! No credential enters a Computer (ADR-0005). The harness sends its
//! model requests here with a token of its session. The endpoint finds
//! the session of the token, checks the Spend Cap of its Workspace, and
//! forwards the request with the Org's provider key through
//! `Router::forward`. The answer streams back unchanged, and the usage
//! that it reports becomes one Usage Record of the Run that started the
//! session.
//!
//! The first segment of the path names the provider, whose Org key the
//! request uses, and the rest names the API: the Anthropic Messages API
//! under `/anthropic/v1`, and the OpenAI Responses and Chat Completions
//! APIs under `/openai/v1` and `/openrouter/v1`. Each refusal has the
//! error shape of the API of the request, so the harness shows the
//! reason to the Agent.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONNECTION, CONTENT_TYPE, HeaderName};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::StreamExt;
use llm_router::{ForwardRequest, ProtocolKind, ProviderConfig, Router, RouterConfig};
use pagis_agent::{CapReads, ModelCatalog};
use pagis_core::{
    Clock, CodingSessionId, CodingSessionStore, ModelTokenOwner, Provider, ProviderKeys,
    StoreError, UsageId, UsageRecord, UsageStore, UserStore, WorkspaceId, WorkspaceStore,
};
use rand::RngCore;
use serde::Deserialize;
use serde_json::Value;

use crate::auth::hash_secret;

/// The largest request body, the size that the Messages API takes.
pub const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

/// The headers that end at the endpoint: the connection headers of RFC
/// 9110 section 7.6.1. Each header that `Connection` names ends here
/// too.
const HOP_BY_HOP: [&str; 6] = [
    "connection",
    "proxy-connection",
    "keep-alive",
    "te",
    "transfer-encoding",
    "upgrade",
];

/// The tokens of the Harness Model Endpoint. The store holds the hash of
/// each token and never the token.
pub struct HarnessModelTokens {
    sessions: Arc<dyn CodingSessionStore>,
}

impl HarnessModelTokens {
    pub fn new(sessions: Arc<dyn CodingSessionStore>) -> Self {
        Self { sessions }
    }

    /// A new token for one session of the Workspace: 32 random bytes in
    /// base64url. The new token replaces the old one of the session, so
    /// the old token stops. `None` when the Workspace holds no such
    /// session.
    pub async fn mint(
        &self,
        workspace_id: &WorkspaceId,
        session_id: &CodingSessionId,
    ) -> Result<Option<String>, StoreError> {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token = URL_SAFE_NO_PAD.encode(bytes);
        let written = self
            .sessions
            .set_model_token(workspace_id, session_id, &hash_secret(&token))
            .await?;
        Ok(written.then_some(token))
    }
}

/// What the endpoint reads and writes.
pub struct HarnessModelDeps {
    pub sessions: Arc<dyn CodingSessionStore>,
    /// The Org's provider keys, read at each request.
    pub keys: Arc<ProviderKeys>,
    /// Base URL overrides per provider, the ones the Provider Model
    /// Lists use. Production keeps the provider defaults.
    pub provider_base_urls: HashMap<Provider, String>,
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub users: Arc<dyn UserStore>,
    pub usage: Arc<dyn UsageStore>,
    pub models: Arc<ModelCatalog>,
    pub clock: Arc<dyn Clock>,
}

/// The endpoint as one router, which the daemon serves on its own
/// listener.
pub fn router(deps: HarnessModelDeps) -> axum::Router {
    let endpoint = Arc::new(Endpoint {
        deps,
        providers: Mutex::new(HashMap::new()),
    });
    axum::Router::new()
        .route("/{*path}", post(serve))
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .with_state(endpoint)
}

struct Endpoint {
    deps: HarnessModelDeps,
    /// The router of each provider's key, and a hash of that key. A new
    /// key builds a new router, as `RouterBrain` does.
    providers: Mutex<HashMap<Provider, (u64, Arc<Router>)>>,
}

/// One route of the endpoint: the provider whose Org key the request
/// uses, and the call of that provider's API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Route {
    provider: Provider,
    call: Call,
}

/// The route of a path: `/{provider}/v1{call path}`. `None` for a path
/// that the endpoint does not serve.
fn route(path: &str) -> Option<Route> {
    let (provider, call) = path.strip_prefix('/')?.split_once("/v1/")?;
    let provider = Provider::from_id(provider)?;
    let call = match (provider, call) {
        (Provider::Anthropic, "messages") => Call::Messages,
        (Provider::Anthropic, "messages/count_tokens") => Call::CountTokens,
        (Provider::OpenAi | Provider::OpenRouter, "responses") => Call::Responses,
        (Provider::OpenAi | Provider::OpenRouter, "chat/completions") => Call::ChatCompletions,
        _ => return None,
    };
    Some(Route { provider, call })
}

/// The requests that the endpoint forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    /// `POST /messages` of the Anthropic Messages API.
    Messages,
    /// `POST /messages/count_tokens` of the Anthropic Messages API.
    CountTokens,
    /// `POST /responses` of the OpenAI Responses API.
    Responses,
    /// `POST /chat/completions` of the OpenAI Chat Completions API.
    ChatCompletions,
}

impl Call {
    /// The path under the provider's base URL.
    fn path(self) -> &'static str {
        match self {
            Call::Messages => "/messages",
            Call::CountTokens => "/messages/count_tokens",
            Call::Responses => "/responses",
            Call::ChatCompletions => "/chat/completions",
        }
    }

    /// The format of the request and of its answer.
    fn wire(self) -> ProtocolKind {
        match self {
            Call::Messages | Call::CountTokens => ProtocolKind::AnthropicMessages,
            Call::Responses => ProtocolKind::OpenAiResponses,
            Call::ChatCompletions => ProtocolKind::OpenAiChat,
        }
    }

    fn api(self) -> Api {
        match self {
            Call::Messages | Call::CountTokens => Api::Anthropic,
            Call::Responses | Call::ChatCompletions => Api::OpenAi,
        }
    }

    /// Whether the call costs money. Only such a call meets the Spend
    /// Cap and writes a Usage Record.
    fn costs(self) -> bool {
        self != Call::CountTokens
    }
}

/// The API family of a request, which gives the headers that pass to the
/// provider and the error shape of a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Api {
    Anthropic,
    OpenAi,
}

impl Api {
    /// The API of a path that the endpoint does not serve, from its
    /// provider segment.
    fn of_path(path: &str) -> Api {
        let provider = path
            .strip_prefix('/')
            .and_then(|path| path.split('/').next())
            .and_then(Provider::from_id);
        match provider {
            Some(Provider::OpenAi | Provider::OpenRouter) => Api::OpenAi,
            _ => Api::Anthropic,
        }
    }
}

/// The one field of the body that the endpoint reads.
#[derive(Deserialize)]
struct ModelField {
    model: String,
}

async fn not_found(request: Request) -> Response {
    refusal(
        Api::of_path(request.uri().path()),
        Refusal::NotFound,
        "the Harness Model Endpoint serves POST /anthropic/v1/messages, \
         /anthropic/v1/messages/count_tokens, /openai/v1/responses, \
         /openai/v1/chat/completions, /openrouter/v1/responses and \
         /openrouter/v1/chat/completions",
    )
}

/// Answer one request, and log the session and the status. The log
/// never holds the token or a body.
async fn serve(State(endpoint): State<Arc<Endpoint>>, request: Request) -> Response {
    let Some(route) = route(request.uri().path()) else {
        return not_found(request).await;
    };
    let (session, response) = endpoint.answer(route, request).await;
    let status = response.status().as_u16();
    match session {
        Some(session) => {
            tracing::info!(%session, status, ?route, "the Harness Model Endpoint answered")
        }
        None => tracing::info!(
            status,
            ?route,
            "the Harness Model Endpoint answered a request of no running Coding Session"
        ),
    }
    response
}

impl Endpoint {
    async fn answer(&self, route: Route, request: Request) -> (Option<CodingSessionId>, Response) {
        let api = route.call.api();
        let owner = match self.owner(request.headers()).await {
            Ok(Some(owner)) => owner,
            Ok(None) => {
                return (
                    None,
                    refusal(
                        api,
                        Refusal::Token,
                        "the token names no running Coding Session",
                    ),
                );
            }
            Err(error) => {
                tracing::error!(%error, "the Harness Model Endpoint could not read the token");
                return (None, failure(api, StatusCode::INTERNAL_SERVER_ERROR));
            }
        };
        let session = owner.session_id.clone();
        (Some(session), self.forward(route, owner, request).await)
    }

    /// The session of the token of the request. The token comes as
    /// `Authorization: Bearer` or as `x-api-key`.
    async fn owner(&self, headers: &HeaderMap) -> Result<Option<ModelTokenOwner>, StoreError> {
        let Some(token) = token(headers) else {
            return Ok(None);
        };
        self.deps
            .sessions
            .model_token_owner(&hash_secret(token))
            .await
    }

    async fn forward(&self, route: Route, owner: ModelTokenOwner, request: Request) -> Response {
        let Route { provider, call } = route;
        let api = call.api();
        let (parts, body) = request.into_parts();
        let body = match read_body(body).await {
            Ok(body) => body,
            Err(BodyError::TooLarge) => {
                return refusal(
                    api,
                    Refusal::TooLarge,
                    "the request body is larger than 32 MiB",
                );
            }
            Err(BodyError::Unreadable) => {
                return refusal(
                    api,
                    Refusal::BadRequest,
                    "the request body could not be read",
                );
            }
        };
        let Ok(ModelField { model }) = serde_json::from_slice(&body) else {
            return refusal(
                api,
                Refusal::BadRequest,
                "the request body is not a JSON object with a string `model`",
            );
        };
        let router = match self.provider_router(provider) {
            Ok(Some(router)) => router,
            Ok(None) => {
                return refusal(
                    api,
                    Refusal::NoKey,
                    format!(
                        "this installation has no {} key. An administrator of this \
                         installation adds one in the Administration Interface.",
                        provider.name()
                    ),
                );
            }
            Err(error) => {
                tracing::error!(%error, "the Harness Model Endpoint could not build its provider");
                return failure(api, StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        if call.costs() {
            let reads = CapReads {
                workspaces: self.deps.workspaces.as_ref(),
                users: self.deps.users.as_ref(),
                usage: self.deps.usage.as_ref(),
                models: self.deps.models.as_ref(),
                clock: self.deps.clock.as_ref(),
            };
            let candidates = [format!("{}/{model}", provider.id())];
            if let Some(stop) = pagis_agent::cap_stop(reads, &owner.workspace_id, &candidates).await
            {
                return refusal(api, Refusal::SpendCap, stop.refusal());
            }
        }

        let forwarded = match router
            .forward(
                provider.id(),
                ForwardRequest {
                    wire: call.wire(),
                    path: call.path().to_string(),
                    headers: forwarded_headers(api, &parts.headers),
                    body: with_usage_chunk(call, body),
                },
            )
            .await
        {
            Ok(forwarded) => forwarded,
            Err(error @ llm_router::Error::Transport { .. }) => {
                tracing::warn!(%error, session = %owner.session_id, "the provider could not be reached");
                return failure(api, StatusCode::BAD_GATEWAY);
            }
            Err(error) => {
                tracing::error!(%error, session = %owner.session_id, "the forward failed");
                return failure(api, StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        if call.costs() {
            self.record_usage(owner, provider, model, forwarded.metered);
        }

        let mut response = Response::new(Body::from_stream(forwarded.body));
        *response.status_mut() = forwarded.status;
        *response.headers_mut() = answer_headers(&forwarded.headers);
        response
    }

    /// Write one Usage Record when the answer gave a usage. An answer with
    /// no usage writes nothing, as a turn of a Run does.
    fn record_usage(
        &self,
        owner: ModelTokenOwner,
        provider: Provider,
        requested_model: String,
        metered: tokio::sync::oneshot::Receiver<llm_router::Metered>,
    ) {
        let usage_store = Arc::clone(&self.deps.usage);
        let models = Arc::clone(&self.deps.models);
        let clock = Arc::clone(&self.deps.clock);
        tokio::spawn(async move {
            let Ok(metered) = metered.await else {
                return;
            };
            let Some(usage) = metered.usage else {
                return;
            };
            let provider = provider.id();
            let model = metered.model.unwrap_or(requested_model);
            let record = UsageRecord {
                id: UsageId::generate(),
                workspace_id: owner.workspace_id,
                run_id: owner.run_id,
                provider: Some(provider.to_string()),
                // A model that no layer prices costs an unknown amount,
                // which the record keeps as unknown.
                cost_usd: models.metadata(provider, &model).cost(&usage),
                model: Some(model),
                input_tokens: i64::try_from(usage.input_tokens).unwrap_or(i64::MAX),
                output_tokens: i64::try_from(usage.output_tokens).unwrap_or(i64::MAX),
                cache_read_tokens: i64::try_from(usage.cache_read_input_tokens).unwrap_or(i64::MAX),
                cache_write_tokens: i64::try_from(usage.cache_write_input_tokens)
                    .unwrap_or(i64::MAX),
                created_at: clock.now_ms(),
            };
            if let Err(error) = usage_store.record(&record).await {
                tracing::error!(
                    %error,
                    session = %owner.session_id,
                    "the usage record of a harness model request was not kept"
                );
            }
        });
    }

    /// The router of the provider's current key, or `None` when the
    /// installation has none. It holds the provider's own headers alone:
    /// the harness sends its own `anthropic-beta`.
    fn provider_router(&self, provider: Provider) -> Result<Option<Arc<Router>>, String> {
        let Some((key, _)) = self
            .deps
            .keys
            .resolve(provider)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let fingerprint = hasher.finish();
        let mut cached = self.providers.lock().expect("the provider router lock");
        if let Some((held, router)) = cached.get(&provider)
            && *held == fingerprint
        {
            return Ok(Some(Arc::clone(router)));
        }
        let mut config = match provider {
            Provider::Anthropic => ProviderConfig::anthropic(key),
            Provider::OpenAi => ProviderConfig::openai_responses(key),
            Provider::OpenRouter => ProviderConfig::openrouter(key),
            Provider::Deepgram | Provider::ElevenLabs => {
                return Err(format!(
                    "the Harness Model Endpoint serves no model API of {}",
                    provider.id()
                ));
            }
        };
        if let Some(base_url) = self.deps.provider_base_urls.get(&provider) {
            config.base_url = base_url.clone();
        }
        let router = Arc::new(
            Router::new(RouterConfig::new().provider(provider.id(), config))
                .map_err(|error| error.to_string())?,
        );
        cached.insert(provider, (fingerprint, Arc::clone(&router)));
        Ok(Some(router))
    }
}

/// The token of a request: `Authorization: Bearer <token>`, else
/// `x-api-key`.
fn token(headers: &HeaderMap) -> Option<&str> {
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    bearer
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
        })
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

/// Why a request body was not read.
enum BodyError {
    TooLarge,
    Unreadable,
}

/// Read the body, up to [`MAX_REQUEST_BYTES`].
async fn read_body(body: Body) -> Result<Bytes, BodyError> {
    let mut stream = body.into_data_stream();
    let mut read = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| BodyError::Unreadable)?;
        if read.len() + chunk.len() > MAX_REQUEST_BYTES {
            return Err(BodyError::TooLarge);
        }
        read.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(read))
}

/// The body that goes to the provider. A Chat Completions stream carries
/// its usage only when the request sets `stream_options.include_usage`,
/// so the endpoint sets it on the body of such a stream, and the meter
/// reads the usage. This is the one change of a body: every other body
/// goes unchanged.
fn with_usage_chunk(call: Call, body: Bytes) -> Bytes {
    if call != Call::ChatCompletions {
        return body;
    }
    let Ok(Value::Object(mut fields)) = serde_json::from_slice::<Value>(&body) else {
        return body;
    };
    if fields.get("stream") != Some(&Value::Bool(true)) {
        return body;
    }
    let options = fields
        .entry("stream_options")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !options.is_object() {
        *options = Value::Object(serde_json::Map::new());
    }
    if options.get("include_usage") == Some(&Value::Bool(true)) {
        return body;
    }
    options["include_usage"] = Value::Bool(true);
    match serde_json::to_vec(&fields) {
        Ok(changed) => Bytes::from(changed),
        Err(_) => body,
    }
}

/// The headers of a request that go to the provider: `content-type`, and
/// the `anthropic-*` headers of the Messages API or the `accept` header
/// of the OpenAI APIs. The token, the `host` and every other header of
/// the harness stay here.
fn forwarded_headers(api: Api, request: &HeaderMap) -> HeaderMap {
    request
        .iter()
        .filter(|(name, _)| {
            *name == CONTENT_TYPE
                || match api {
                    Api::Anthropic => name.as_str().starts_with("anthropic-"),
                    Api::OpenAi => *name == ACCEPT,
                }
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// The headers of the provider's answer that go to the harness: all but
/// the connection headers.
fn answer_headers(provider: &HeaderMap) -> HeaderMap {
    let mut headers = provider.clone();
    let named: Vec<HeaderName> = provider
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
    headers
}

/// Why the endpoint refuses a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// The token names no running Coding Session.
    Token,
    /// The installation has no key of the provider.
    NoKey,
    /// The Spend Cap of the Workspace stops the request.
    SpendCap,
    TooLarge,
    BadRequest,
    NotFound,
}

impl Refusal {
    fn status(self) -> StatusCode {
        match self {
            Refusal::Token => StatusCode::UNAUTHORIZED,
            Refusal::NoKey | Refusal::SpendCap => StatusCode::FORBIDDEN,
            Refusal::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Refusal::BadRequest => StatusCode::BAD_REQUEST,
            Refusal::NotFound => StatusCode::NOT_FOUND,
        }
    }

    /// The error `type` of the Messages API.
    fn anthropic_type(self) -> &'static str {
        match self {
            Refusal::Token => "authentication_error",
            Refusal::NoKey | Refusal::SpendCap => "permission_error",
            Refusal::TooLarge => "request_too_large",
            Refusal::BadRequest => "invalid_request_error",
            Refusal::NotFound => "not_found_error",
        }
    }

    /// The error `type` and `code` of the OpenAI APIs.
    fn openai_type_and_code(self) -> (&'static str, Option<&'static str>) {
        match self {
            Refusal::Token => ("invalid_request_error", Some("invalid_api_key")),
            Refusal::SpendCap => ("insufficient_quota", Some("insufficient_quota")),
            Refusal::NoKey | Refusal::TooLarge | Refusal::BadRequest | Refusal::NotFound => {
                ("invalid_request_error", None)
            }
        }
    }
}

/// The error body of `api`: `{"type":"error","error":{"type","message"}}`
/// for the Messages API, `{"error":{"message","type","code"}}` for the
/// OpenAI APIs.
fn error_body(
    api: Api,
    anthropic_type: &str,
    (openai_type, code): (&str, Option<&str>),
    message: String,
) -> Value {
    match api {
        Api::Anthropic => serde_json::json!({
            "type": "error",
            "error": { "type": anthropic_type, "message": message },
        }),
        Api::OpenAi => serde_json::json!({
            "error": { "message": message, "type": openai_type, "code": code },
        }),
    }
}

/// A refusal of the endpoint, in the error shape of `api`. The harness
/// does not try the request again.
fn refusal(api: Api, refusal: Refusal, message: impl Into<String>) -> Response {
    let body = error_body(
        api,
        refusal.anthropic_type(),
        refusal.openai_type_and_code(),
        message.into(),
    );
    let mut response = (refusal.status(), Json(body)).into_response();
    response
        .headers_mut()
        .insert("x-should-retry", HeaderValue::from_static("false"));
    response
}

/// A failure of the daemon or of the way to the provider, in the error
/// shape of `api`. The harness may try the request again.
fn failure(api: Api, status: StatusCode) -> Response {
    let message = match status {
        StatusCode::BAD_GATEWAY => "the provider could not be reached",
        _ => "the daemon could not serve the request",
    };
    let body = error_body(
        api,
        "api_error",
        ("server_error", None),
        message.to_string(),
    );
    (status, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use pagis_core::{
        AgentId, CodingSession, CodingSessionEvent, CodingSessionEventKind, CodingSessionState,
        NewCodingSessionEvent,
    };

    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    HeaderName::from_bytes(name.as_bytes()).unwrap(),
                    HeaderValue::from_str(value).unwrap(),
                )
            })
            .collect()
    }

    fn names(headers: &HeaderMap) -> Vec<&str> {
        let mut names: Vec<&str> = headers.keys().map(HeaderName::as_str).collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn content_type_and_the_anthropic_headers_go_to_the_provider() {
        let request = headers(&[
            ("content-type", "application/json"),
            ("anthropic-version", "2023-06-01"),
            ("anthropic-beta", "context-management-2025-06-27"),
            ("anthropic-beta", "interleaved-thinking-2025-05-14"),
        ]);

        let forwarded = forwarded_headers(Api::Anthropic, &request);

        assert_eq!(forwarded, request);
    }

    #[test]
    fn content_type_and_accept_alone_go_to_an_openai_provider() {
        let request = headers(&[
            ("content-type", "application/json"),
            ("accept", "text/event-stream"),
            ("anthropic-beta", "interleaved-thinking-2025-05-14"),
            ("openai-beta", "responses=experimental"),
            ("originator", "codex_cli_rs"),
            ("session_id", "a-session"),
        ]);

        let forwarded = forwarded_headers(Api::OpenAi, &request);

        assert_eq!(names(&forwarded), ["accept", "content-type"]);
    }

    #[test]
    fn the_token_the_host_and_the_harness_headers_stay_at_the_endpoint() {
        let request = headers(&[
            ("content-type", "application/json"),
            ("authorization", "Bearer session-token"),
            ("x-api-key", "session-token"),
            ("host", "host.docker.internal:4404"),
            ("x-claude-code-session-id", "a-session"),
            ("x-stainless-lang", "js"),
            ("user-agent", "claude-cli/2.0"),
        ]);

        for api in [Api::Anthropic, Api::OpenAi] {
            let forwarded = forwarded_headers(api, &request);

            assert_eq!(names(&forwarded), ["content-type"], "{api:?}");
        }
    }

    #[test]
    fn each_route_maps_to_its_provider_wire_and_path() {
        for (path, provider, wire, provider_path) in [
            (
                "/anthropic/v1/messages",
                Provider::Anthropic,
                ProtocolKind::AnthropicMessages,
                "/messages",
            ),
            (
                "/anthropic/v1/messages/count_tokens",
                Provider::Anthropic,
                ProtocolKind::AnthropicMessages,
                "/messages/count_tokens",
            ),
            (
                "/openai/v1/responses",
                Provider::OpenAi,
                ProtocolKind::OpenAiResponses,
                "/responses",
            ),
            (
                "/openrouter/v1/responses",
                Provider::OpenRouter,
                ProtocolKind::OpenAiResponses,
                "/responses",
            ),
            (
                "/openai/v1/chat/completions",
                Provider::OpenAi,
                ProtocolKind::OpenAiChat,
                "/chat/completions",
            ),
            (
                "/openrouter/v1/chat/completions",
                Provider::OpenRouter,
                ProtocolKind::OpenAiChat,
                "/chat/completions",
            ),
        ] {
            let route = route(path).unwrap_or_else(|| panic!("{path} is a route"));
            assert_eq!(route.provider, provider, "{path}");
            assert_eq!(route.call.wire(), wire, "{path}");
            assert_eq!(route.call.path(), provider_path, "{path}");
        }
    }

    #[test]
    fn a_path_of_another_provider_or_api_is_no_route() {
        for path in [
            "/anthropic/v1/responses",
            "/anthropic/v1/complete",
            "/openai/v1/messages",
            "/openai/v1/models",
            "/openai/v1/responses/resp_1",
            "/openrouter/v1/embeddings",
            "/deepgram/v1/listen",
            "/elevenlabs/v1/chat/completions",
            "/v1/chat/completions",
            "/openai/responses",
            "/",
        ] {
            assert_eq!(route(path), None, "{path}");
        }
    }

    #[test]
    fn only_the_openai_routes_refuse_in_the_openai_shape() {
        assert_eq!(Api::of_path("/openai/v1/models"), Api::OpenAi);
        assert_eq!(Api::of_path("/openrouter/v1/models"), Api::OpenAi);
        assert_eq!(Api::of_path("/anthropic/v1/complete"), Api::Anthropic);
        assert_eq!(Api::of_path("/v1/models"), Api::Anthropic);
        assert_eq!(Call::Responses.api(), Api::OpenAi);
        assert_eq!(Call::ChatCompletions.api(), Api::OpenAi);
        assert_eq!(Call::Messages.api(), Api::Anthropic);
        assert_eq!(Call::CountTokens.api(), Api::Anthropic);
    }

    fn json(body: &Bytes) -> Value {
        serde_json::from_slice(body).unwrap()
    }

    #[test]
    fn a_streaming_chat_completions_body_asks_for_the_usage_chunk() {
        let body = Bytes::from_static(
            br#"{"model":"gpt-5","messages":[],"stream":true,"temperature":0.7}"#,
        );

        let sent = with_usage_chunk(Call::ChatCompletions, body);

        assert_eq!(
            std::str::from_utf8(&sent).unwrap(),
            r#"{"model":"gpt-5","messages":[],"stream":true,"temperature":0.7,"stream_options":{"include_usage":true}}"#,
            "the fields keep their order and values"
        );
    }

    #[test]
    fn the_usage_chunk_keeps_the_other_stream_options() {
        let body = Bytes::from_static(
            br#"{"model":"gpt-5","stream":true,"stream_options":{"include_usage":false,"include_obfuscation":false}}"#,
        );

        let sent = with_usage_chunk(Call::ChatCompletions, body);

        assert_eq!(
            json(&sent)["stream_options"],
            serde_json::json!({"include_usage": true, "include_obfuscation": false})
        );
    }

    #[test]
    fn no_other_body_changes() {
        let asked = r#"{"model":"gpt-5", "stream":true,"stream_options":{"include_usage":true}}"#;
        let not_streamed = r#"{"model":"gpt-5", "messages":[],"stream":false}"#;
        let no_stream_field = r#"{"model":"gpt-5", "messages":[]}"#;
        let responses = r#"{"model":"gpt-5", "input":[],"stream":true}"#;
        let messages = r#"{"model":"claude-sonnet-4-5", "messages":[],"stream":true}"#;
        for (call, body) in [
            (Call::ChatCompletions, asked),
            (Call::ChatCompletions, not_streamed),
            (Call::ChatCompletions, no_stream_field),
            (Call::ChatCompletions, "not json"),
            (Call::Responses, responses),
            (Call::Messages, messages),
            (Call::CountTokens, messages),
        ] {
            let sent = with_usage_chunk(call, Bytes::copy_from_slice(body.as_bytes()));

            assert_eq!(sent, body.as_bytes(), "{call:?}: {body}");
        }
    }

    #[test]
    fn the_answer_keeps_every_header_but_the_connection_headers() {
        let provider = headers(&[
            ("content-type", "text/event-stream"),
            ("request-id", "req_1"),
            ("anthropic-ratelimit-tokens-remaining", "1000"),
            ("connection", "keep-alive, x-hop"),
            ("keep-alive", "timeout=5"),
            ("transfer-encoding", "chunked"),
            ("x-hop", "1"),
        ]);

        let answer = answer_headers(&provider);

        assert_eq!(
            names(&answer),
            [
                "anthropic-ratelimit-tokens-remaining",
                "content-type",
                "request-id"
            ]
        );
    }

    #[test]
    fn the_token_comes_as_a_bearer_or_as_an_api_key() {
        assert_eq!(
            token(&headers(&[("authorization", "Bearer abc")])),
            Some("abc")
        );
        assert_eq!(token(&headers(&[("x-api-key", "abc")])), Some("abc"));
        assert_eq!(token(&headers(&[("authorization", "Basic abc")])), None);
        assert_eq!(token(&headers(&[("x-api-key", " ")])), None);
        assert_eq!(token(&HeaderMap::new()), None);
    }

    /// A store that keeps the hashes that the tokens write.
    #[derive(Default)]
    struct Hashes(Mutex<Vec<String>>);

    #[async_trait]
    impl CodingSessionStore for Hashes {
        async fn set_model_token(
            &self,
            _: &WorkspaceId,
            _: &CodingSessionId,
            hash: &str,
        ) -> Result<bool, StoreError> {
            self.0.lock().unwrap().push(hash.to_string());
            Ok(true)
        }

        async fn insert(&self, _: &CodingSession) -> Result<(), StoreError> {
            unreachable!("a token writes only its hash")
        }
        async fn update(&self, _: &CodingSession) -> Result<bool, StoreError> {
            unreachable!("a token writes only its hash")
        }
        async fn get(
            &self,
            _: &WorkspaceId,
            _: &CodingSessionId,
        ) -> Result<Option<CodingSession>, StoreError> {
            unreachable!("a token writes only its hash")
        }
        async fn list(
            &self,
            _: &WorkspaceId,
            _: Option<&AgentId>,
            _: Option<CodingSessionState>,
            _: Option<&CodingSessionId>,
            _: u32,
        ) -> Result<Vec<CodingSession>, StoreError> {
            unreachable!("a token writes only its hash")
        }
        async fn count_open(&self, _: &WorkspaceId, _: &AgentId) -> Result<u32, StoreError> {
            unreachable!("a token writes only its hash")
        }
        async fn list_open(&self) -> Result<Vec<CodingSession>, StoreError> {
            unreachable!("a token writes only its hash")
        }
        async fn append_event(
            &self,
            _: &WorkspaceId,
            _: &CodingSessionId,
            _: NewCodingSessionEvent,
        ) -> Result<Vec<CodingSessionEvent>, StoreError> {
            unreachable!("a token writes only its hash")
        }
        async fn list_events(
            &self,
            _: &WorkspaceId,
            _: &CodingSessionId,
            _: Option<i64>,
            _: u32,
        ) -> Result<Vec<CodingSessionEvent>, StoreError> {
            unreachable!("a token writes only its hash")
        }
        async fn latest_event(
            &self,
            _: &WorkspaceId,
            _: &CodingSessionId,
            _: &[CodingSessionEventKind],
        ) -> Result<Option<CodingSessionEvent>, StoreError> {
            unreachable!("a token writes only its hash")
        }
        async fn model_token_owner(&self, _: &str) -> Result<Option<ModelTokenOwner>, StoreError> {
            unreachable!("a token writes only its hash")
        }
    }

    #[tokio::test]
    async fn the_store_holds_the_hash_of_a_token_and_never_the_token() {
        let store = Arc::new(Hashes::default());
        let tokens = HarnessModelTokens::new(Arc::clone(&store) as _);
        let workspace = WorkspaceId::generate();
        let session = CodingSessionId::generate();

        let first = tokens.mint(&workspace, &session).await.unwrap().unwrap();
        let second = tokens.mint(&workspace, &session).await.unwrap().unwrap();

        assert_ne!(first, second);
        assert_eq!(
            URL_SAFE_NO_PAD.decode(&first).unwrap().len(),
            32,
            "32 random bytes in base64url"
        );
        assert_eq!(
            *store.0.lock().unwrap(),
            [hash_secret(&first), hash_secret(&second)]
        );
        assert!(
            store
                .0
                .lock()
                .unwrap()
                .iter()
                .all(|hash| hash != &first && hash != &second)
        );
    }
}
