//! The Harness Model Endpoint (ADR-0033): the model API that the daemon
//! serves to the harness of a Coding Session in a Computer.
//!
//! No credential enters a Computer (ADR-0005). The harness sends its
//! model requests here with a token of its session. The endpoint finds
//! the session of the token, checks the Spend Cap of its Workspace, and
//! forwards the request unchanged with the Org's provider key through
//! `Router::forward`. The answer streams back unchanged, and the usage
//! that it reports becomes one Usage Record of the Run that started the
//! session.
//!
//! The endpoint serves the Anthropic Messages API. Each refusal has the
//! error shape of that API, so the harness shows the reason to the
//! Agent.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, CONNECTION, CONTENT_TYPE, HeaderName};
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
        provider: Mutex::new(None),
    });
    axum::Router::new()
        .route("/anthropic/v1/messages", post(messages))
        .route("/anthropic/v1/messages/count_tokens", post(count_tokens))
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .with_state(endpoint)
}

struct Endpoint {
    deps: HarnessModelDeps,
    /// The router of the Anthropic key, and a hash of that key. A new
    /// key builds a new router, as `RouterBrain` does.
    provider: Mutex<Option<(u64, Arc<Router>)>>,
}

/// The two requests of the Messages API that the endpoint forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Messages,
    CountTokens,
}

impl Call {
    /// The path under the provider's base URL.
    fn path(self) -> &'static str {
        match self {
            Call::Messages => "/messages",
            Call::CountTokens => "/messages/count_tokens",
        }
    }

    /// Whether the call costs money. Only such a call meets the Spend
    /// Cap and writes a Usage Record.
    fn costs(self) -> bool {
        self == Call::Messages
    }
}

/// The one field of the body that the endpoint reads.
#[derive(Deserialize)]
struct ModelField {
    model: String,
}

async fn messages(State(endpoint): State<Arc<Endpoint>>, request: Request) -> Response {
    serve(&endpoint, Call::Messages, request).await
}

async fn count_tokens(State(endpoint): State<Arc<Endpoint>>, request: Request) -> Response {
    serve(&endpoint, Call::CountTokens, request).await
}

async fn not_found() -> Response {
    refusal(
        StatusCode::NOT_FOUND,
        "not_found_error",
        "the Harness Model Endpoint serves POST /anthropic/v1/messages and \
         POST /anthropic/v1/messages/count_tokens",
    )
}

/// Answer one request, and log the session and the status. The log
/// never holds the token or a body.
async fn serve(endpoint: &Endpoint, call: Call, request: Request) -> Response {
    let (session, response) = endpoint.answer(call, request).await;
    let status = response.status().as_u16();
    match session {
        Some(session) => {
            tracing::info!(%session, status, ?call, "the Harness Model Endpoint answered")
        }
        None => tracing::info!(
            status,
            ?call,
            "the Harness Model Endpoint answered a request of no running Coding Session"
        ),
    }
    response
}

impl Endpoint {
    async fn answer(&self, call: Call, request: Request) -> (Option<CodingSessionId>, Response) {
        let owner = match self.owner(request.headers()).await {
            Ok(Some(owner)) => owner,
            Ok(None) => {
                return (
                    None,
                    refusal(
                        StatusCode::UNAUTHORIZED,
                        "authentication_error",
                        "the token names no running Coding Session",
                    ),
                );
            }
            Err(error) => {
                tracing::error!(%error, "the Harness Model Endpoint could not read the token");
                return (None, failure(StatusCode::INTERNAL_SERVER_ERROR));
            }
        };
        let session = owner.session_id.clone();
        (Some(session), self.forward(call, owner, request).await)
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

    async fn forward(&self, call: Call, owner: ModelTokenOwner, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let body = match read_body(body).await {
            Ok(body) => body,
            Err(BodyError::TooLarge) => {
                return refusal(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request_too_large",
                    "the request body is larger than 32 MiB",
                );
            }
            Err(BodyError::Unreadable) => {
                return refusal(
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    "the request body could not be read",
                );
            }
        };
        let Ok(ModelField { model }) = serde_json::from_slice(&body) else {
            return refusal(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "the request body is not a JSON object with a string `model`",
            );
        };
        let router = match self.provider_router() {
            Ok(Some(router)) => router,
            Ok(None) => {
                return refusal(
                    StatusCode::FORBIDDEN,
                    "permission_error",
                    "this installation has no Anthropic key. An administrator of this \
                     installation adds one in the Administration Interface.",
                );
            }
            Err(error) => {
                tracing::error!(%error, "the Harness Model Endpoint could not build its provider");
                return failure(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        let provider = Provider::Anthropic.id();
        if call.costs() {
            let reads = CapReads {
                workspaces: self.deps.workspaces.as_ref(),
                users: self.deps.users.as_ref(),
                usage: self.deps.usage.as_ref(),
                models: self.deps.models.as_ref(),
                clock: self.deps.clock.as_ref(),
            };
            let candidates = [format!("{provider}/{model}")];
            if let Some(stop) = pagis_agent::cap_stop(reads, &owner.workspace_id, &candidates).await
            {
                return refusal(StatusCode::FORBIDDEN, "permission_error", stop.refusal());
            }
        }

        let forwarded = match router
            .forward(
                provider,
                ForwardRequest {
                    wire: ProtocolKind::AnthropicMessages,
                    path: call.path().to_string(),
                    headers: forwarded_headers(&parts.headers),
                    body,
                },
            )
            .await
        {
            Ok(forwarded) => forwarded,
            Err(error @ llm_router::Error::Transport { .. }) => {
                tracing::warn!(%error, session = %owner.session_id, "the provider could not be reached");
                return failure(StatusCode::BAD_GATEWAY);
            }
            Err(error) => {
                tracing::error!(%error, session = %owner.session_id, "the forward failed");
                return failure(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        if call.costs() {
            self.record_usage(owner, model, forwarded.metered);
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
            let provider = Provider::Anthropic.id();
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

    /// The router of the current Anthropic key, or `None` when the
    /// installation has none. It holds the provider's own headers alone:
    /// the harness sends its own `anthropic-beta`.
    fn provider_router(&self) -> Result<Option<Arc<Router>>, String> {
        let Some((key, _)) = self
            .deps
            .keys
            .resolve(Provider::Anthropic)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let fingerprint = hasher.finish();
        let mut cached = self.provider.lock().expect("the provider router lock");
        if let Some((held, router)) = cached.as_ref()
            && *held == fingerprint
        {
            return Ok(Some(Arc::clone(router)));
        }
        let mut config = ProviderConfig::anthropic(key);
        if let Some(base_url) = self.deps.provider_base_urls.get(&Provider::Anthropic) {
            config.base_url = base_url.clone();
        }
        let router = Arc::new(
            Router::new(RouterConfig::new().provider(Provider::Anthropic.id(), config))
                .map_err(|error| error.to_string())?,
        );
        *cached = Some((fingerprint, Arc::clone(&router)));
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

/// The headers of a request that go to the provider: `content-type` and
/// the `anthropic-*` headers. The token, the `host` and every other
/// header of the harness stay here.
fn forwarded_headers(request: &HeaderMap) -> HeaderMap {
    request
        .iter()
        .filter(|(name, _)| *name == CONTENT_TYPE || name.as_str().starts_with("anthropic-"))
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

/// A refusal of the endpoint, in the error shape of the Messages API. The
/// harness does not try the request again.
fn refusal(status: StatusCode, kind: &'static str, message: impl Into<String>) -> Response {
    let mut response = (
        status,
        Json(serde_json::json!({
            "type": "error",
            "error": { "type": kind, "message": message.into() },
        })),
    )
        .into_response();
    response
        .headers_mut()
        .insert("x-should-retry", HeaderValue::from_static("false"));
    response
}

/// A failure of the daemon or of the way to the provider, in the error
/// shape of the Messages API. The harness may try the request again.
fn failure(status: StatusCode) -> Response {
    let message = match status {
        StatusCode::BAD_GATEWAY => "the provider could not be reached",
        _ => "the daemon could not serve the request",
    };
    (
        status,
        Json(serde_json::json!({
            "type": "error",
            "error": { "type": "api_error", "message": message },
        })),
    )
        .into_response()
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

        let forwarded = forwarded_headers(&request);

        assert_eq!(forwarded, request);
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

        let forwarded = forwarded_headers(&request);

        assert_eq!(names(&forwarded), ["content-type"]);
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
