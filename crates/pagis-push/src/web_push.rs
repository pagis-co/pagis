//! One Web Push to one Push Subscription, and what the push service
//! answered.

use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::{PublicKey, SecretKey};
use reqwest::header::{AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Response, StatusCode};
use url::Url;
use web_push_native::Auth;

use crate::guard::{AddressGuard, Policy};
use crate::options::Options;
use crate::vapid::Vapid;

/// The largest plaintext of a Web Push. With one record, the body is the
/// plaintext and 103 bytes, so it stays under the 2800 bytes that the
/// Push Relay forwards to APNs and FCM.
pub const MAX_PLAINTEXT: usize = 2048;

/// How long one Web Push may take, from the connection to the answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How much of the body of a failed answer the outcome keeps.
const MAX_ERROR_BODY: usize = 512;

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
    fn failed(error: impl Into<String>) -> Self {
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
    Client(#[from] reqwest::Error),
}

/// The Web Push sender of one installation: its VAPID Key, its contact
/// and the HTTP client behind the address guard.
pub struct WebPush {
    vapid: Vapid,
    policy: Policy,
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
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .https_only(policy == Policy::Public)
            .dns_resolver(Arc::new(AddressGuard::new(policy)))
            .build()?;
        Ok(Self {
            vapid,
            policy,
            client,
        })
    }

    /// Encrypt `plaintext` for `subscription` and post it to its
    /// endpoint.
    pub async fn send(
        &self,
        subscription: &Subscription,
        plaintext: &[u8],
        options: Options,
    ) -> Outcome {
        if plaintext.len() > MAX_PLAINTEXT {
            return Outcome::TooLarge;
        }
        let Ok(endpoint) = Url::parse(&subscription.endpoint) else {
            return Outcome::failed(format!(
                "the endpoint {:?} is not a URL",
                subscription.endpoint
            ));
        };
        if let Err(error) = self.policy.check_endpoint(&endpoint) {
            return Outcome::failed(error);
        }
        let (public_key, auth) = match client_keys(subscription) {
            Ok(keys) => keys,
            Err(error) => return Outcome::failed(error),
        };
        let body = match web_push_native::encrypt(plaintext.to_vec(), &public_key, &auth) {
            Ok(body) => body,
            Err(error) => return Outcome::failed(format!("encrypt the Web Push: {error}")),
        };
        let authorization = match self.vapid.authorization(&endpoint) {
            Ok(authorization) => authorization,
            Err(error) => return Outcome::failed(error),
        };

        let mut request = self
            .client
            .post(endpoint)
            .header("TTL", options.ttl.as_secs())
            .header("Urgency", options.urgency.header())
            .header(CONTENT_ENCODING, "aes128gcm")
            .header(CONTENT_TYPE, "application/octet-stream")
            .header(AUTHORIZATION, authorization)
            .body(body);
        if let Some(topic) = &options.topic {
            request = request.header("Topic", topic.as_str());
        }
        match request.send().await {
            Ok(response) => outcome_of(response).await,
            Err(error) => Outcome::failed(error_chain(&error)),
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

async fn outcome_of(response: Response) -> Outcome {
    let status = response.status();
    match status.as_u16() {
        200..=299 => Outcome::Delivered,
        404 | 410 => Outcome::Gone,
        413 => Outcome::TooLarge,
        429 => Outcome::RateLimited {
            retry_after: retry_after(&response),
        },
        _ => Outcome::Failed {
            status: Some(status),
            error: format!(
                "the push service answered {status}: {}",
                error_body(response).await
            ),
        },
    }
}

/// The delay of a `Retry-After` header: delay seconds or an HTTP-date.
fn retry_after(response: &Response) -> Option<Duration> {
    let value = response.headers().get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(
        date.duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO),
    )
}

/// The start of the body of a failed answer, which names the reason.
/// The rest is not read, so a large body costs nothing.
async fn error_body(mut response: Response) -> String {
    let mut body = Vec::new();
    while body.len() < MAX_ERROR_BODY {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            _ => break,
        }
    }
    body.truncate(MAX_ERROR_BODY);
    String::from_utf8_lossy(&body).trim().to_string()
}

/// An error and each of its sources, so the reason of the address guard
/// shows through the errors of the HTTP client.
fn error_chain(error: &dyn Error) -> String {
    let mut chain = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        chain.push_str(": ");
        chain.push_str(&cause.to_string());
        source = cause.source();
    }
    chain
}
