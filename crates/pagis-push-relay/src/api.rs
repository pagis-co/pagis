//! The HTTP routes of the relay.
//!
//! A Mobile App installation registers its device token and the VAPID
//! key of its server, and gets a Web Push endpoint that holds a random
//! id. A wrong or missing secret and an unknown id answer the same
//! `404`, so a caller cannot find which ids exist.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, MatchedPath, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use serde::Serialize;
use serde_json::json;
use sqlx::SqlitePool;

use crate::limit::RegistrationLimit;
use crate::registration::{self, Invalid, RegistrationId, Secret, secret_hash};
use crate::store::Registrations;
use crate::{PublicOrigin, TrustedProxy};

/// A registration body holds a token of at most 4096 characters and a
/// key of 87, so a larger body is not a registration.
const MAX_BODY_BYTES: usize = 16 * 1024;

struct Relay {
    registrations: Registrations,
    public_origin: PublicOrigin,
    proxy: TrustedProxy,
    limit: RegistrationLimit,
}

/// The routes of the relay over the registrations in `pool`. Serve it
/// with `into_make_service_with_connect_info::<SocketAddr>()`, because
/// the registration limit reads the peer address.
pub fn router(pool: SqlitePool, public_origin: PublicOrigin, proxy: TrustedProxy) -> Router {
    let relay = Arc::new(Relay {
        registrations: Registrations::new(pool),
        public_origin,
        proxy,
        limit: RegistrationLimit::default(),
    });
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/registrations", post(register))
        .route(
            "/v1/registrations/{id}",
            put(change_token).delete(unregister),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(log_request))
        .with_state(relay)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "version": env!("CARGO_PKG_VERSION") }))
}

#[derive(Serialize)]
struct Registered {
    id: String,
    secret: String,
    endpoint: String,
}

async fn register(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Refusal> {
    let address = relay.proxy.client_address(peer, &headers);
    relay
        .limit
        .admit(address, Instant::now())
        .map_err(Refusal::TooMany)?;
    let registration = registration::new_registration(&body)?;
    let id = RegistrationId::random();
    let secret = Secret::random();
    relay
        .registrations
        .insert(&id, &secret_hash(secret.as_str()), &registration)
        .await
        .map_err(Refusal::store)?;
    let endpoint = format!("{}/v1/push/{}", relay.public_origin.as_str(), id.as_str());
    let body = Registered {
        id: id.as_str().to_string(),
        secret: secret.as_str().to_string(),
        endpoint,
    };
    Ok((StatusCode::CREATED, Extension(LoggedId(id)), Json(body)).into_response())
}

async fn change_token(
    State(relay): State<Arc<Relay>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(id) = RegistrationId::parse(&id) else {
        return Refusal::NotFound.into_response();
    };
    let outcome = async {
        let token = registration::token_change(&body)?;
        let hash = secret_hash(bearer(&headers).ok_or(Refusal::NotFound)?);
        let changed = relay
            .registrations
            .change_token(&id, &hash, &token)
            .await
            .map_err(Refusal::store)?;
        found(changed)
    }
    .await;
    (Extension(LoggedId(id)), outcome).into_response()
}

async fn unregister(
    State(relay): State<Arc<Relay>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(id) = RegistrationId::parse(&id) else {
        return Refusal::NotFound.into_response();
    };
    let outcome = async {
        let hash = secret_hash(bearer(&headers).ok_or(Refusal::NotFound)?);
        let removed = relay
            .registrations
            .remove(&id, &hash)
            .await
            .map_err(Refusal::store)?;
        found(removed)
    }
    .await;
    (Extension(LoggedId(id)), outcome).into_response()
}

/// `204` when a registration had this id and this secret, and `404`
/// otherwise.
fn found(found: bool) -> Result<StatusCode, Refusal> {
    if found {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(Refusal::NotFound)
    }
}

/// The secret of `Authorization: Bearer <secret>`.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, secret) = value.split_once(' ')?;
    let secret = secret.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !secret.is_empty()).then_some(secret)
}

/// Why the relay refuses a request. The body is
/// `{error: {code, message}}`, as the daemon's API errors are.
#[derive(Debug)]
enum Refusal {
    Invalid(Invalid),
    NotFound,
    TooMany(Duration),
    Internal,
}

impl Refusal {
    fn store(error: sqlx::Error) -> Self {
        tracing::error!(%error, "the registrations store failed");
        Self::Internal
    }
}

impl From<Invalid> for Refusal {
    fn from(invalid: Invalid) -> Self {
        Self::Invalid(invalid)
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Invalid(Invalid(message)) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "validation", message)
            }
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                "registration not found".to_string(),
            ),
            Self::TooMany(wait) => {
                let mut response = (
                    StatusCode::TOO_MANY_REQUESTS,
                    error_body(
                        "rate_limited",
                        "too many registrations from this address; wait and try again",
                    ),
                )
                    .into_response();
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from(whole_seconds(wait)));
                return response;
            }
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal error".to_string(),
            ),
        };
        (status, error_body(code, &message)).into_response()
    }
}

fn error_body(code: &str, message: &str) -> Json<serde_json::Value> {
    Json(json!({ "error": { "code": code, "message": message } }))
}

/// `wait` rounded up to whole seconds, and at least one.
fn whole_seconds(wait: Duration) -> u64 {
    let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    seconds.max(1)
}

/// The registration id that a handler puts on its response for the log
/// line.
#[derive(Debug, Clone)]
struct LoggedId(RegistrationId);

/// Log the route, the registration id and the status of each request.
/// A log line holds no token, no secret and no VAPID key: the route is
/// the pattern and not the path, and the id comes from the handler.
async fn log_request(route: Option<MatchedPath>, request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let response = next.run(request).await;
    let route = route.as_ref().map_or("unmatched", MatchedPath::as_str);
    let id = response
        .extensions()
        .get::<LoggedId>()
        .map_or("-", |LoggedId(id)| id.as_str());
    let status = response.status().as_u16();
    tracing::info!(%method, %route, %id, status, "request");
    response
}
