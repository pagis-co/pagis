//! The HTTP routes of the relay.
//!
//! A Mobile App installation registers its device token and the VAPID
//! key of its server, and gets a Web Push endpoint that holds a random
//! id. A wrong or missing secret and an unknown id answer the same
//! `404`, so a caller cannot find which ids exist.
//!
//! A server posts a Web Push to the endpoint. The relay checks the
//! VAPID token, the size and the rate, and forwards the body unchanged
//! to the transport of the platform.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{
    ConnectInfo, DefaultBodyLimit, FromRequest, MatchedPath, Path, Request, State,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use serde::Serialize;
use serde_json::json;
use sqlx::SqlitePool;

use crate::clock::Clock;
use crate::limit::RegistrationLimit;
use crate::registration::{self, Invalid, RegistrationId, Secret, secret_hash};
use crate::store::Registrations;
use crate::transport::{Delivery, Message, Transports, Urgency};
use crate::vapid::{self, InvalidToken};
use crate::{PublicOrigin, TrustedProxy, push};

/// A registration body holds a token of at most 4096 characters and a
/// key of 87, so a larger body is not a registration.
const MAX_REGISTRATION_BYTES: usize = 16 * 1024;

/// The largest body of a Web Push. RFC 8030 asks a push service to take
/// 4096 bytes, but APNs and FCM take 4096 bytes after base64 and the
/// envelope, and a server sends at most 2800 bytes (ADR-0030).
const MAX_PUSH_BYTES: usize = 2800;

/// How many pushes one registration may have in one UTC day.
const MAX_PUSHES_A_DAY: i64 = 1000;

const SECONDS_A_DAY: u64 = 24 * 60 * 60;

/// The `TTL` header of RFC 8030.
const TTL: HeaderName = HeaderName::from_static("ttl");

struct Relay {
    registrations: Registrations,
    public_origin: PublicOrigin,
    proxy: TrustedProxy,
    limit: RegistrationLimit,
    transports: Transports,
    clock: Arc<dyn Clock>,
}

/// The routes of the relay over the registrations in `pool`. A Web Push
/// goes to the transport of its platform in `transports`, and the checks
/// of time read `clock`. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()`, because the
/// registration limit reads the peer address.
pub fn router(
    pool: SqlitePool,
    public_origin: PublicOrigin,
    proxy: TrustedProxy,
    transports: Transports,
    clock: Arc<dyn Clock>,
) -> Router {
    let relay = Arc::new(Relay {
        registrations: Registrations::new(pool),
        public_origin,
        proxy,
        limit: RegistrationLimit::default(),
        transports,
        clock,
    });
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/registrations", post(register))
        .route(
            "/v1/registrations/{id}",
            put(change_token).delete(unregister),
        )
        .layer(DefaultBodyLimit::max(MAX_REGISTRATION_BYTES))
        .route(
            "/v1/push/{id}",
            post(push).layer(DefaultBodyLimit::max(MAX_PUSH_BYTES)),
        )
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
    if relay.transports.of(registration.platform).is_none() {
        return Err(Refusal::Invalid(Invalid(format!(
            "the relay does not serve platform {}",
            registration.platform.name()
        ))));
    }
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

async fn push(
    State(relay): State<Arc<Relay>>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let Some(id) = RegistrationId::parse(&id) else {
        return Refusal::NotFound.into_response();
    };
    let mut logged = LoggedPush::default();
    let outcome = receive(&relay, &id, request, &mut logged).await;
    (Extension(LoggedId(id)), Extension(logged), outcome).into_response()
}

/// Check one Web Push, forward it to the transport of its platform, and
/// answer what the transport did. A push that the relay refuses or that
/// the transport does not take does not count against the day.
async fn receive(
    relay: &Relay,
    id: &RegistrationId,
    request: Request,
    logged: &mut LoggedPush,
) -> Result<Response, Refusal> {
    let stored = relay
        .registrations
        .find(id)
        .await
        .map_err(Refusal::store)?
        .ok_or(Refusal::NotFound)?;
    let headers = request.headers();
    if !push::is_aes128gcm(headers) {
        return Err(Refusal::UnsupportedEncoding);
    }
    let authorization = vapid::Authorization::from_headers(headers).ok_or(Refusal::Unauthorized)?;
    let now = relay.clock.now();
    vapid::verify(
        &authorization,
        &stored.vapid_key,
        relay.public_origin.as_str(),
        now,
    )
    .map_err(|InvalidToken(reason)| {
        tracing::debug!(id = id.as_str(), reason, "the VAPID token is not valid");
        Refusal::Forbidden
    })?;
    let options = push::options(headers);
    // The route's body limit stops the read at MAX_PUSH_BYTES, so a
    // larger body is never held.
    let body = Bytes::from_request(request, &())
        .await
        .map_err(|rejection| match rejection.status() {
            StatusCode::PAYLOAD_TOO_LARGE => Refusal::TooLarge,
            _ => Refusal::BadRequest(rejection.body_text()),
        })?;
    logged.body_bytes = Some(body.len());
    let options = options.map_err(Refusal::BadRequest)?;
    logged.urgency = Some(options.urgency);
    let counted = relay
        .registrations
        .count_push(id, now, MAX_PUSHES_A_DAY)
        .await
        .map_err(Refusal::store)?;
    if !counted {
        return Err(Refusal::DayLimit(until_utc_midnight(now)));
    }

    let message = Message {
        body: body.to_vec(),
        ttl: options.ttl,
        urgency: options.urgency,
        topic: options.topic,
    };
    let platform = stored.registration.platform;
    let delivery = match relay.transports.of(platform) {
        Some(transport) => transport.send(&stored.registration, &message).await,
        None => Delivery::Failed(format!(
            "the relay does not serve platform {}",
            platform.name()
        )),
    };
    // The answer tells the sender what became of the push, so a store
    // error after the forward is logged and does not change it.
    match delivery {
        Delivery::Delivered => {
            log_store_error(relay.registrations.delivered(id, now).await);
            let location = format!(
                "{}/v1/messages/{}",
                relay.public_origin.as_str(),
                registration::random_base64url::<16>()
            );
            let headers = [
                (header::LOCATION, location),
                (TTL, message.ttl.as_secs().to_string()),
            ];
            Ok((StatusCode::CREATED, headers).into_response())
        }
        Delivery::Gone => {
            log_store_error(relay.registrations.remove_gone(id).await);
            Err(Refusal::Gone)
        }
        Delivery::Failed(reason) => {
            tracing::warn!(id = id.as_str(), %reason, "the forward failed");
            log_store_error(relay.registrations.uncount_push(id, now).await);
            Err(Refusal::BadGateway)
        }
    }
}

/// The time from `now` to the next UTC midnight, when the count of a
/// day starts again.
fn until_utc_midnight(now: SystemTime) -> Duration {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    Duration::from_secs(SECONDS_A_DAY - seconds % SECONDS_A_DAY)
}

fn log_store_error(result: Result<(), sqlx::Error>) {
    if let Err(error) = result {
        tracing::error!(%error, "the registrations store failed");
    }
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

/// Why the relay refuses a request, or why a push did not reach the
/// device. The body is `{error: {code, message}}`, as the daemon's API
/// errors are.
#[derive(Debug)]
enum Refusal {
    Invalid(Invalid),
    BadRequest(String),
    Unauthorized,
    Forbidden,
    NotFound,
    Gone,
    TooLarge,
    UnsupportedEncoding,
    /// Too many registrations from one address, for this long.
    TooMany(Duration),
    /// Too many pushes to one registration in this UTC day, for this
    /// long.
    DayLimit(Duration),
    Internal,
    BadGateway,
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
        let mut extra = None;
        let (status, code, message) = match self {
            Self::Invalid(Invalid(message)) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "validation", message)
            }
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, "bad_request", message),
            Self::Unauthorized => {
                extra = Some((header::WWW_AUTHENTICATE, HeaderValue::from_static("vapid")));
                (
                    StatusCode::UNAUTHORIZED,
                    "unauthorized",
                    "a Web Push needs Authorization: vapid t=<jwt>, k=<key>".to_string(),
                )
            }
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "the VAPID token is not valid for this endpoint".to_string(),
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                "registration not found".to_string(),
            ),
            Self::Gone => (
                StatusCode::GONE,
                "gone",
                "the device of this registration is gone".to_string(),
            ),
            Self::TooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                format!("the body of a Web Push is at most {MAX_PUSH_BYTES} bytes"),
            ),
            Self::UnsupportedEncoding => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_encoding",
                "the body of a Web Push must have Content-Encoding: aes128gcm".to_string(),
            ),
            Self::TooMany(wait) => {
                extra = Some(retry_after(wait));
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    "too many registrations from this address; wait and try again".to_string(),
                )
            }
            Self::DayLimit(wait) => {
                extra = Some(retry_after(wait));
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    format!(
                        "this endpoint had {MAX_PUSHES_A_DAY} pushes in this UTC day; \
                         wait until the next UTC midnight"
                    ),
                )
            }
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal error".to_string(),
            ),
            Self::BadGateway => (
                StatusCode::BAD_GATEWAY,
                "bad_gateway",
                "the push service of the device did not take the push".to_string(),
            ),
        };
        let mut response = (status, error_body(code, &message)).into_response();
        if let Some((name, value)) = extra {
            response.headers_mut().insert(name, value);
        }
        response
    }
}

fn error_body(code: &str, message: &str) -> Json<serde_json::Value> {
    Json(json!({ "error": { "code": code, "message": message } }))
}

/// `Retry-After: <seconds>` for `wait`.
fn retry_after(wait: Duration) -> (HeaderName, HeaderValue) {
    (header::RETRY_AFTER, HeaderValue::from(whole_seconds(wait)))
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

/// What the push route puts on its response for the log line: the
/// urgency and the size of the body, when the relay read them.
#[derive(Debug, Clone, Default)]
struct LoggedPush {
    urgency: Option<Urgency>,
    body_bytes: Option<usize>,
}

/// Log the route, the registration id and the status of each request,
/// and the urgency and the body size of a push. A log line holds no
/// token, no secret, no VAPID key, no VAPID token and no body: the route
/// is the pattern and not the path, and the id comes from the handler.
async fn log_request(route: Option<MatchedPath>, request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let response = next.run(request).await;
    let route = route.as_ref().map_or("unmatched", MatchedPath::as_str);
    let id = response
        .extensions()
        .get::<LoggedId>()
        .map_or("-", |LoggedId(id)| id.as_str());
    let status = response.status().as_u16();
    match response.extensions().get::<LoggedPush>() {
        Some(push) => tracing::info!(
            %method,
            %route,
            %id,
            status,
            urgency = push.urgency.map(Urgency::as_str),
            body_bytes = push.body_bytes,
            "request"
        ),
        None => tracing::info!(%method, %route, %id, status, "request"),
    }
    response
}
