//! One Web Push to one Push Subscription, and what the push service
//! answered.

use std::time::{Duration, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::header::{AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE, RETRY_AFTER};
use http::{HeaderMap, Request, StatusCode};
use p256::{PublicKey, SecretKey};
use pagis_core::UnixMillis;
use url::Url;
use web_push_native::Auth;

use crate::options::Options;
use crate::policy::Policy;
use crate::vapid::Vapid;

/// The largest plaintext of a Web Push. With one record, the body is the
/// plaintext and 103 bytes, so it stays under the 2800 bytes that the
/// Push Relay forwards to APNs and FCM.
pub const MAX_PLAINTEXT: usize = 2048;

/// How much of the body of a failed answer the outcome keeps.
pub(crate) const MAX_ERROR_BODY: usize = 512;

/// The push endpoint and the keys of one client, each as the client gave
/// it (`PushSubscription.toJSON()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub endpoint: String,
    /// The base64url of the client's uncompressed P-256 public key.
    pub p256dh: String,
    /// The base64url of the client's 16-byte auth secret.
    pub auth: String,
}

/// What became of one Web Push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The push service took it (2xx).
    Delivered,
    /// The subscription has ended (404 or 410). The Push Subscription
    /// ends too.
    Gone,
    /// The plaintext is over [`MAX_PLAINTEXT`], or the push service
    /// refused the body (413).
    TooLarge,
    /// The push service asks the sender to wait (429), for `retry_after`
    /// when it says how long.
    RateLimited { retry_after: Option<Duration> },
    /// Every other answer, a refused endpoint and a transport error.
    /// `status` is `None` when no answer came.
    Failed {
        status: Option<StatusCode>,
        error: String,
    },
}

impl Outcome {
    /// Read one answer without I/O. `now` is the time of the answer in
    /// Unix milliseconds. It gives the delay of an HTTP-date in
    /// `Retry-After`. A failed answer keeps at most 512 bytes of `body_start`.
    pub fn of(status: StatusCode, headers: &HeaderMap, body_start: &[u8], now: UnixMillis) -> Self {
        match status.as_u16() {
            200..=299 => Self::Delivered,
            404 | 410 => Self::Gone,
            413 => Self::TooLarge,
            429 => Self::RateLimited {
                retry_after: retry_after(headers, now),
            },
            _ => Self::Failed {
                status: Some(status),
                error: format!(
                    "the push service answered {status}: {}",
                    String::from_utf8_lossy(&body_start[..body_start.len().min(MAX_ERROR_BODY)])
                        .trim()
                ),
            },
        }
    }

    pub(crate) fn failed(error: impl Into<String>) -> Self {
        Outcome::Failed {
            status: None,
            error: error.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("{0}")]
    Key(String),
    #[error("build the Web Push client: {0}")]
    #[cfg(not(target_arch = "wasm32"))]
    Client(#[from] reqwest::Error),
}

/// The Web Push protocol of one installation: its VAPID Key and its
/// contact. On native targets it also holds the HTTP client behind the
/// address guard.
pub struct WebPush {
    vapid: Vapid,
    policy: Policy,
    #[cfg(not(target_arch = "wasm32"))]
    client: reqwest::Client,
}

impl WebPush {
    /// A sender that signs with `vapid_key` and names `public_origin` as
    /// its contact when it is `https`.
    pub fn new(
        vapid_key: &SecretKey,
        public_origin: &str,
        policy: Policy,
    ) -> Result<Self, BuildError> {
        let vapid = Vapid::new(vapid_key, public_origin).map_err(BuildError::Key)?;
        Ok(Self {
            vapid,
            policy,
            #[cfg(not(target_arch = "wasm32"))]
            client: crate::native::client(policy)?,
        })
    }

    /// Make one encrypted Web Push request without network I/O.
    /// The endpoint URL must pass the policy check. A plaintext over
    /// [`MAX_PLAINTEXT`] gives [`Outcome::TooLarge`]; another error gives
    /// [`Outcome::Failed`] with no status.
    ///
    /// On `wasm32-unknown-unknown`, the host posts this request with no
    /// redirect. It refuses an endpoint whose name resolves to an address
    /// that is not public. It connects only to the checked addresses, so
    /// a second DNS lookup cannot change the destination. The host sets
    /// a timeout and gives the answer to [`Outcome::of`].
    pub fn request(
        &self,
        subscription: &Subscription,
        plaintext: &[u8],
        options: Options,
    ) -> Result<Request<Vec<u8>>, Outcome> {
        if plaintext.len() > MAX_PLAINTEXT {
            return Err(Outcome::TooLarge);
        }
        let Ok(endpoint) = Url::parse(&subscription.endpoint) else {
            return Err(Outcome::failed(format!(
                "the endpoint {:?} is not a URL",
                subscription.endpoint
            )));
        };
        self.policy
            .check_endpoint(&endpoint)
            .map_err(Outcome::failed)?;
        let (public_key, auth) = client_keys(subscription).map_err(Outcome::failed)?;
        let body = web_push_native::encrypt(plaintext.to_vec(), &public_key, &auth)
            .map_err(|error| Outcome::failed(format!("encrypt the Web Push: {error}")))?;
        let authorization = self
            .vapid
            .authorization(&endpoint)
            .map_err(Outcome::failed)?;

        let mut request = Request::builder()
            .method(http::Method::POST)
            .uri(endpoint.as_str())
            .header("TTL", options.ttl.as_secs())
            .header("Urgency", options.urgency.header())
            .header(CONTENT_ENCODING, "aes128gcm")
            .header(CONTENT_TYPE, "application/octet-stream")
            .header(AUTHORIZATION, authorization);
        if let Some(topic) = &options.topic {
            request = request.header("Topic", topic.as_str());
        }
        request
            .body(body)
            .map_err(|error| Outcome::failed(format!("build the Web Push request: {error}")))
    }

    /// Encrypt `plaintext` for `subscription` and post it to its endpoint,
    /// with the address guard, no redirect and a 10 second timeout.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn send(
        &self,
        subscription: &Subscription,
        plaintext: &[u8],
        options: Options,
    ) -> Outcome {
        match self.request(subscription, plaintext, options) {
            Ok(request) => crate::native::send(&self.client, request).await,
            Err(outcome) => outcome,
        }
    }
}

/// The client's public key and auth secret of `subscription`.
fn client_keys(subscription: &Subscription) -> Result<(PublicKey, Auth), String> {
    let p256dh = URL_SAFE_NO_PAD
        .decode(&subscription.p256dh)
        .map_err(|_| "the p256dh of the subscription is not base64url".to_string())?;
    let public_key = PublicKey::from_sec1_bytes(&p256dh)
        .map_err(|_| "the p256dh of the subscription is not a P-256 point".to_string())?;
    let auth = URL_SAFE_NO_PAD
        .decode(&subscription.auth)
        .map_err(|_| "the auth of the subscription is not base64url".to_string())?;
    if auth.len() != 16 {
        return Err(format!(
            "the auth of the subscription is {} bytes, not 16",
            auth.len()
        ));
    }
    Ok((public_key, Auth::clone_from_slice(&auth)))
}

/// The delay of a `Retry-After` header: delay seconds or an HTTP-date.
fn retry_after(headers: &HeaderMap, now: UnixMillis) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    let date_ms = date.duration_since(UNIX_EPOCH).ok()?.as_millis() as i128;
    Some(Duration::from_millis(
        (date_ms - i128::from(now)).max(0) as u64
    ))
}
