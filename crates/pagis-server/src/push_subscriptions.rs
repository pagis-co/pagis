//! The VAPID Key of the installation and the Push Subscriptions of a
//! Person (ADR-0030).
//!
//! A client reads the public half of the VAPID Key, gives it to
//! `PushManager.subscribe` as `applicationServerKey`, and posts the
//! subscription it gets: `PushSubscription.toJSON()`. The subscription
//! belongs to the Session that posts it and ends with that Session.
//!
//! The route takes only an `https` endpoint with a DNS name. The sender
//! checks each address of the name at the time of the send.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use base64::Engine;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use pagis_core::{PushSubscription, PushSubscriptionId, SecretError, SecretStore};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::AppState;
use crate::auth::Tenant;
use crate::error::ApiError;

/// The name of the private half of the VAPID Key in `secrets.enc`.
const VAPID_KEY_SECRET: &str = "vapid_private_key";

/// The longest endpoint the daemon keeps, in characters.
const MAX_ENDPOINT_CHARS: usize = 2048;

/// The length of an uncompressed P-256 point: the tag `0x04`, then the
/// two 32-byte coordinates.
const UNCOMPRESSED_POINT_BYTES: usize = 65;

/// The length of the auth secret of a Push Subscription (RFC 8291).
const AUTH_SECRET_BYTES: usize = 16;

/// Base64url as the Push API writes it: no padding. A decode takes the
/// value with or without padding, and an encode writes none.
const BASE64URL: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// The VAPID Key of the installation, made at the first need.
///
/// A new key goes in only through the create-if-absent operation of the
/// store, and the answer is the key that the store holds. Two first
/// needs that race therefore agree on one key.
pub(crate) fn vapid_key(secrets: &dyn SecretStore) -> Result<SecretKey, SecretError> {
    let minted = BASE64URL.encode(random_secret_key().to_bytes());
    let stored = secrets.get_or_insert(VAPID_KEY_SECRET, &minted)?;
    BASE64URL
        .decode(&stored)
        .ok()
        .and_then(|bytes| SecretKey::from_slice(&bytes).ok())
        .ok_or_else(|| {
            SecretError(format!(
                "the {VAPID_KEY_SECRET} entry of secrets.enc is not a P-256 secret key"
            ))
        })
}

/// A random P-256 secret key. A random 32-byte value is a valid scalar
/// except when it is zero or not less than the order of the curve, which
/// happens with a chance of about 2^-32; such a value is drawn again.
fn random_secret_key() -> SecretKey {
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Ok(key) = SecretKey::from_slice(&bytes) {
            return key;
        }
    }
}

/// The public half of the VAPID Key.
#[derive(Debug, Serialize, ToSchema)]
pub struct VapidKeyDto {
    /// The uncompressed P-256 point as base64url with no padding: the
    /// `applicationServerKey` of `PushManager.subscribe`.
    pub vapid_public_key: String,
}

/// The public half of the installation's VAPID Key, which a client
/// subscribes with.
#[utoipa::path(
    get,
    path = "/api/v1/push/key",
    responses(
        (status = 200, body = VapidKeyDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn get_vapid_key(
    State(state): State<Arc<AppState>>,
) -> Result<Json<VapidKeyDto>, ApiError> {
    let key = vapid_key(state.secrets.as_ref()).map_err(crate::settings::secret_error)?;
    Ok(Json(VapidKeyDto {
        vapid_public_key: BASE64URL.encode(key.public_key().to_encoded_point(false).as_bytes()),
    }))
}

/// The keys of a Push Subscription, as `PushSubscription.toJSON()`
/// writes them.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PushSubscriptionKeys {
    /// The client's P-256 public key: an uncompressed point, as base64url.
    pub p256dh: String,
    /// The client's 16-byte auth secret, as base64url.
    pub auth: String,
}

/// The body of `PushSubscription.toJSON()`. Other members, such as
/// `expirationTime`, are ignored.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SubscribeRequest {
    /// The `https` URL of the push service, with a DNS name and at most
    /// 2048 characters.
    pub endpoint: String,
    pub keys: PushSubscriptionKeys,
}

/// One Push Subscription of the signed-in Person, named by the client of
/// its Session. The endpoint and the keys stay in the daemon.
#[derive(Debug, Serialize, ToSchema)]
pub struct PushSubscriptionDto {
    pub id: String,
    /// The kind of client of the Session: `browser` or `desktop`.
    pub client_kind: String,
    /// The client of the Session, such as "Safari on iPhone". `null`
    /// where the client said none.
    pub client_name: Option<String>,
    pub created_at: i64,
    /// When the daemon last sent a Web Push to it. `null` until the
    /// first one.
    pub last_sent_at: Option<i64>,
    /// True for the Push Subscription of the Session that asks.
    pub current: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PushSubscriptionsDto {
    pub items: Vec<PushSubscriptionDto>,
}

/// Subscribe the client of the asking Session to Web Push. A known
/// endpoint moves to this Session and takes the new keys.
#[utoipa::path(
    post,
    path = "/api/v1/push-subscriptions",
    request_body = SubscribeRequest,
    responses(
        (status = 200, body = PushSubscriptionDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
    )
)]
pub async fn subscribe(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Json(request): Json<SubscribeRequest>,
) -> Result<Json<PushSubscriptionDto>, ApiError> {
    let endpoint = checked_endpoint(&request.endpoint)?;
    let p256dh = checked_p256dh(&request.keys.p256dh)?;
    let auth = checked_auth(&request.keys.auth)?;
    let now = state.clock.now_ms();
    let session = state
        .sessions
        .find_live_by_id(&tenant.session_id, now)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let kept = state
        .push_subscriptions
        .upsert(&PushSubscription {
            id: PushSubscriptionId::generate(),
            workspace_id: tenant.workspace_id.clone(),
            session_id: session.id.clone(),
            endpoint,
            p256dh,
            auth,
            created_at: now,
            last_sent_at: None,
        })
        .await?;
    Ok(Json(dto(kept, &session, &tenant)))
}

/// The Push Subscriptions of the signed-in Person, oldest first, each
/// named by the client of its Session.
#[utoipa::path(
    get,
    path = "/api/v1/push-subscriptions",
    responses(
        (status = 200, body = PushSubscriptionsDto),
        (status = 401, body = crate::error::ErrorBody),
    )
)]
pub async fn list_push_subscriptions(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
) -> Result<Json<PushSubscriptionsDto>, ApiError> {
    let now = state.clock.now_ms();
    let sessions: HashMap<_, _> = state
        .sessions
        .list_live_for_user(&tenant.user_id, now)
        .await?
        .into_iter()
        .map(|session| (session.id.clone(), session))
        .collect();
    // A row whose Session expired and waits for the expiry sweep belongs
    // to no live client, so the list leaves it out.
    let items = state
        .push_subscriptions
        .list(&tenant.workspace_id)
        .await?
        .into_iter()
        .filter_map(|row| {
            let session = sessions.get(&row.session_id)?;
            Some(dto(row, session, &tenant))
        })
        .collect();
    Ok(Json(PushSubscriptionsDto { items }))
}

/// Remove one Push Subscription of the signed-in Person. A Push
/// Subscription of another Person reads as absent.
#[utoipa::path(
    delete,
    path = "/api/v1/push-subscriptions/{push_subscription_id}",
    params(("push_subscription_id" = String, Path, description = "One Push Subscription of the signed-in Person")),
    responses(
        (status = 204, description = "The Push Subscription ended"),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn remove_push_subscription(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(push_subscription_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    match state
        .push_subscriptions
        .delete(
            &tenant.workspace_id,
            &PushSubscriptionId::from(push_subscription_id),
        )
        .await?
    {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::not_found("that push subscription")),
    }
}

/// What the push service answered to a test Notification.
#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PushOutcomeDto {
    /// The push service took it.
    Delivered,
    /// The push service ended the Push Subscription, and the daemon
    /// deleted it.
    Gone,
    /// The push service refused the size of the body.
    TooLarge,
    /// The push service asks the daemon to wait, for
    /// `retry_after_seconds` when it says how long.
    RateLimited { retry_after_seconds: Option<u64> },
    /// Every other answer, a refused endpoint and a transport error.
    /// `status` is `null` when no answer came.
    Failed { status: Option<u16>, error: String },
}

impl From<pagis_push::Outcome> for PushOutcomeDto {
    fn from(outcome: pagis_push::Outcome) -> Self {
        match outcome {
            pagis_push::Outcome::Delivered => Self::Delivered,
            pagis_push::Outcome::Gone => Self::Gone,
            pagis_push::Outcome::TooLarge => Self::TooLarge,
            pagis_push::Outcome::RateLimited { retry_after } => Self::RateLimited {
                retry_after_seconds: retry_after.map(|delay| delay.as_secs()),
            },
            pagis_push::Outcome::Failed { status, error } => Self::Failed {
                status: status.map(|status| status.as_u16()),
                error,
            },
        }
    }
}

/// Send a test Notification to one Push Subscription of the signed-in
/// Person, and answer what the push service answered. A Push
/// Subscription of another Person reads as absent.
#[utoipa::path(
    post,
    path = "/api/v1/push-subscriptions/{push_subscription_id}/test",
    params(("push_subscription_id" = String, Path, description = "One Push Subscription of the signed-in Person")),
    responses(
        (status = 200, body = PushOutcomeDto),
        (status = 401, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
    )
)]
pub async fn send_test_notification(
    State(state): State<Arc<AppState>>,
    tenant: Tenant,
    Path(push_subscription_id): Path<String>,
) -> Result<Json<PushOutcomeDto>, ApiError> {
    let row = state
        .push_subscriptions
        .list(&tenant.workspace_id)
        .await?
        .into_iter()
        .find(|row| row.id.as_str() == push_subscription_id)
        .ok_or_else(|| ApiError::not_found("that push subscription"))?;
    let outcome = state.notifications.send_test(&row).await;
    Ok(Json(outcome.into()))
}

fn dto(
    row: PushSubscription,
    session: &pagis_core::Session,
    tenant: &Tenant,
) -> PushSubscriptionDto {
    PushSubscriptionDto {
        current: row.session_id == tenant.session_id,
        id: row.id.to_string(),
        client_kind: session.client_kind.as_str().to_string(),
        client_name: session.client_name.clone(),
        created_at: row.created_at,
        last_sent_at: row.last_sent_at,
    }
}

/// The endpoint as the client sent it, when it is an `https` URL with a
/// DNS name and at most [`MAX_ENDPOINT_CHARS`] characters.
fn checked_endpoint(endpoint: &str) -> Result<String, ApiError> {
    if endpoint.chars().count() > MAX_ENDPOINT_CHARS {
        return Err(ApiError::validation(format!(
            "the endpoint is longer than {MAX_ENDPOINT_CHARS} characters"
        )));
    }
    let url = url::Url::parse(endpoint)
        .map_err(|error| ApiError::validation(format!("the endpoint is not a URL: {error}")))?;
    if url.scheme() != "https" {
        return Err(ApiError::validation("the endpoint is not an https URL"));
    }
    match url.host() {
        Some(url::Host::Domain(_)) => Ok(endpoint.to_string()),
        Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => Err(ApiError::validation(
            "the endpoint names an IP address and not a DNS name",
        )),
        None => Err(ApiError::validation("the endpoint names no host")),
    }
}

/// The client's public key as base64url with no padding, when it decodes
/// to an uncompressed point of P-256.
fn checked_p256dh(p256dh: &str) -> Result<String, ApiError> {
    let point = BASE64URL
        .decode(p256dh)
        .map_err(|_| ApiError::validation("p256dh is not base64url"))?;
    let uncompressed = point.len() == UNCOMPRESSED_POINT_BYTES && point[0] == 0x04;
    if !uncompressed || PublicKey::from_sec1_bytes(&point).is_err() {
        return Err(ApiError::validation(
            "p256dh is not an uncompressed P-256 point",
        ));
    }
    Ok(BASE64URL.encode(point))
}

/// The client's auth secret as base64url with no padding, when it
/// decodes to [`AUTH_SECRET_BYTES`] bytes.
fn checked_auth(auth: &str) -> Result<String, ApiError> {
    let secret = BASE64URL
        .decode(auth)
        .map_err(|_| ApiError::validation("auth is not base64url"))?;
    if secret.len() != AUTH_SECRET_BYTES {
        return Err(ApiError::validation(format!(
            "auth is not {AUTH_SECRET_BYTES} bytes"
        )));
    }
    Ok(BASE64URL.encode(secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The receiver public key of RFC 8291, Appendix A.
    const P256DH: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";

    #[test]
    fn two_racing_first_needs_agree_on_one_vapid_key() {
        let secrets = pagis_core::MemorySecretStore::default();

        let keys: Vec<_> = std::thread::scope(|scope| {
            let needs: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| vapid_key(&secrets).unwrap().to_bytes()))
                .collect();
            needs.into_iter().map(|need| need.join().unwrap()).collect()
        });

        assert!(keys.iter().all(|key| *key == keys[0]), "{keys:?}");
        let stored = secrets
            .get(VAPID_KEY_SECRET)
            .unwrap()
            .expect("the key is kept");
        assert_eq!(BASE64URL.decode(stored).unwrap().len(), 32);
    }

    #[test]
    fn a_stored_vapid_key_that_is_not_a_key_is_an_error() {
        let secrets = pagis_core::MemorySecretStore::default();
        secrets.set(VAPID_KEY_SECRET, "not a key").unwrap();

        let error = vapid_key(&secrets).unwrap_err();

        assert!(error.0.contains(VAPID_KEY_SECRET), "{error}");
    }

    #[test]
    fn an_endpoint_with_a_dns_name_is_kept_as_sent() {
        let endpoint = "https://fcm.googleapis.com/fcm/send/abc:def";
        assert_eq!(checked_endpoint(endpoint).unwrap(), endpoint);
        let longest = format!("https://push.example.com/{}", "a".repeat(2048 - 25));
        assert!(checked_endpoint(&longest).is_ok());
    }

    #[test]
    fn an_endpoint_that_is_not_a_url_is_refused() {
        let error = checked_endpoint("push.example.com/x").unwrap_err();
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn padded_keys_are_kept_without_padding() {
        let padded = format!("{P256DH}=");
        assert_eq!(checked_p256dh(&padded).unwrap(), P256DH);
        let secret = [7u8; AUTH_SECRET_BYTES];
        let padded = base64::engine::general_purpose::URL_SAFE.encode(secret);
        assert!(padded.ends_with("=="));
        assert_eq!(checked_auth(&padded).unwrap(), BASE64URL.encode(secret));
    }

    #[test]
    fn a_compressed_point_is_refused() {
        let point = BASE64URL.decode(P256DH).unwrap();
        let public = PublicKey::from_sec1_bytes(&point).unwrap();
        let compressed = BASE64URL.encode(public.to_encoded_point(true).as_bytes());

        assert!(checked_p256dh(&compressed).is_err());
    }
}
