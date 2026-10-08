//! The transport of the `ios` platform: one HTTP/2 request to APNs for
//! each push.
//!
//! The relay connects to APNs with a provider token: an ES256 JWT that it
//! signs with the `.p8` key of the APNs environment of the registration.
//! Each environment has its own key, as Apple scopes a key for both
//! environments to the whole team. The client is `reqwest` and
//! `p256`. `apns-h2` is not used, because it needs `aws-lc-rs` or OpenSSL
//! and the workspace uses ring.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::clock::Clock;
use crate::registration::{Environment, Platform};
use crate::settings::{ApnsKey, ApnsSettings, apns_key_path_variable};
use crate::transport::{Delivery, Message, Registration, Transport, Urgency, error_chain};

/// How long the transport keeps one provider token. APNs refuses a token
/// older than one hour, and a new token more often than once in 20
/// minutes.
const TOKEN_LIFETIME: Duration = Duration::from_secs(50 * 60);

/// How long one request may take, from the connection to the answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The visible alert. The relay cannot read the content, so the
/// Notification Service Extension of the Mobile App replaces it with the
/// decrypted text. iOS shows it as it is when the extension fails.
const ALERT_TITLE: &str = "Pagis";
const ALERT_BODY: &str = "Something needs you";

/// The base URL of each APNs environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApnsBaseUrls {
    pub production: String,
    pub sandbox: String,
}

impl ApnsBaseUrls {
    /// The servers of Apple.
    pub fn apple() -> Self {
        Self {
            production: "https://api.push.apple.com".to_string(),
            sandbox: "https://api.sandbox.push.apple.com".to_string(),
        }
    }

    fn of(&self, environment: Environment) -> &str {
        match environment {
            Environment::Production => &self.production,
            Environment::Sandbox => &self.sandbox,
        }
    }
}

/// An APNs key that stops the relay at start. The message names the
/// variable and the file, and never holds the key.
#[derive(Debug, thiserror::Error)]
pub enum ApnsError {
    #[error("{variable} names {}, which cannot be read", .path.display())]
    Read {
        variable: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(
        "{variable} names {}, which is not an APNs key: the .p8 file, a P-256 \
         private key in PKCS#8 PEM",
        .path.display()
    )]
    Key {
        variable: &'static str,
        path: PathBuf,
    },
    #[error("build the APNs client: {0}")]
    Client(#[from] reqwest::Error),
}

/// A provider token and the time the transport made it.
struct Kept {
    token: String,
    made_at: SystemTime,
}

/// The key of one APNs environment and the provider token that it
/// signed last.
struct TokenSigner {
    key: SigningKey,
    key_id: String,
    kept: Mutex<Option<Kept>>,
}

impl TokenSigner {
    fn read(environment: Environment, key: &ApnsKey) -> Result<Self, ApnsError> {
        Ok(Self {
            key: read_key(apns_key_path_variable(environment), &key.path)?,
            key_id: key.id.clone(),
            kept: Mutex::new(None),
        })
    }

    /// The kept provider token, or a new one when it is older than
    /// [`TOKEN_LIFETIME`]. The lock covers the signature, so concurrent
    /// pushes make one token and not one each.
    fn provider_token(&self, team_id: &str, now: SystemTime) -> String {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        match &*kept {
            // A clock that went back gives no age, and the token stays.
            Some(token)
                if now.duration_since(token.made_at).unwrap_or_default() < TOKEN_LIFETIME =>
            {
                token.token.clone()
            }
            _ => {
                let token = provider_token(&self.key, &self.key_id, team_id, now);
                *kept = Some(Kept {
                    token: token.clone(),
                    made_at: now,
                });
                token
            }
        }
    }

    /// Drop the kept token when it is `used`, so the next push makes a new
    /// one. A token that another push made since then stays.
    fn drop_provider_token(&self, used: &str) {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        if kept.as_ref().is_some_and(|kept| kept.token == used) {
            *kept = None;
        }
    }
}

/// The client of APNs for the Mobile App of one topic, with a key for
/// each APNs environment that it serves.
pub struct ApnsTransport {
    client: reqwest::Client,
    base_urls: ApnsBaseUrls,
    team_id: String,
    topic: String,
    production: Option<TokenSigner>,
    sandbox: Option<TokenSigner>,
    clock: Arc<dyn Clock>,
}

impl ApnsTransport {
    /// Read the key of each environment of `settings` and make a transport
    /// that sends to `base_urls` and reads the time from `clock`.
    pub fn new(
        settings: &ApnsSettings,
        base_urls: ApnsBaseUrls,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, ApnsError> {
        let signer = |environment, key: &Option<ApnsKey>| {
            key.as_ref()
                .map(|key| TokenSigner::read(environment, key))
                .transpose()
        };
        let production = signer(Environment::Production, &settings.production)?;
        let sandbox = signer(Environment::Sandbox, &settings.sandbox)?;
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            base_urls,
            team_id: settings.team_id.clone(),
            topic: settings.topic.clone(),
            production,
            sandbox,
            clock,
        })
    }

    /// The APNs environments that the transport holds a key for.
    pub fn environments(&self) -> Vec<Environment> {
        [Environment::Production, Environment::Sandbox]
            .into_iter()
            .filter(|environment| self.signer(*environment).is_some())
            .collect()
    }

    fn signer(&self, environment: Environment) -> Option<&TokenSigner> {
        match environment {
            Environment::Production => self.production.as_ref(),
            Environment::Sandbox => self.sandbox.as_ref(),
        }
    }
}

#[async_trait]
impl Transport for ApnsTransport {
    async fn send(&self, registration: &Registration, message: &Message) -> Delivery {
        let Platform::Ios(environment) = registration.platform else {
            return Delivery::Failed("APNs serves only platform ios".to_string());
        };
        let Some(signer) = self.signer(environment) else {
            return Delivery::Failed(format!(
                "the relay holds no APNs key for the environment {}",
                environment.as_str()
            ));
        };
        let now = self.clock.now();
        let token = signer.provider_token(&self.team_id, now);
        let request = request(&registration.token, &self.topic, &token, message, now);
        let mut builder = self
            .client
            .post(format!(
                "{}{}",
                self.base_urls.of(environment),
                request.path
            ))
            .body(request.body);
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        let response = match builder.send().await {
            Ok(response) => response,
            // The URL holds the device token, so the error leaves it out.
            Err(error) => {
                return Delivery::Failed(format!(
                    "APNs did not answer: {}",
                    error_chain(&error.without_url())
                ));
            }
        };
        let status = response.status();
        let body = response.bytes().await.unwrap_or_default();
        let reason = reason(&body);
        if is_refused_provider_token(status, reason.as_deref()) {
            signer.drop_provider_token(&token);
        }
        let delivery = delivery(status, reason.as_deref());
        if delivery == Delivery::Gone {
            tracing::info!(
                status = status.as_u16(),
                reason = reason.as_deref(),
                "APNs says that the device token is gone"
            );
        }
        delivery
    }
}

fn read_key(variable: &'static str, path: &Path) -> Result<SigningKey, ApnsError> {
    let pem = std::fs::read_to_string(path).map_err(|source| ApnsError::Read {
        variable,
        path: path.to_path_buf(),
        source,
    })?;
    SigningKey::from_pkcs8_pem(&pem).map_err(|_| ApnsError::Key {
        variable,
        path: path.to_path_buf(),
    })
}

/// The provider token at `now`: an ES256 JWT with the key id in its
/// header and the team id and the time in its claims.
pub(crate) fn provider_token(
    key: &SigningKey,
    key_id: &str,
    team_id: &str,
    now: SystemTime,
) -> String {
    let header = json!({ "alg": "ES256", "kid": key_id });
    let claims = json!({ "iss": team_id, "iat": unix_seconds(now) });
    let signed = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let signature: Signature = key.sign(signed.as_bytes());
    format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

/// One request to APNs, apart from its base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApnsRequest {
    pub(crate) path: String,
    pub(crate) headers: Vec<(&'static str, String)>,
    pub(crate) body: Vec<u8>,
}

/// The request that sends `message` to the device of `device_token` at
/// `now`. The body holds a placeholder alert and the push body as `p`.
pub(crate) fn request(
    device_token: &str,
    topic: &str,
    provider_token: &str,
    message: &Message,
    now: SystemTime,
) -> ApnsRequest {
    let body = json!({
        "aps": {
            "alert": { "title": ALERT_TITLE, "body": ALERT_BODY },
            "mutable-content": 1,
            "sound": "default",
        },
        "p": URL_SAFE_NO_PAD.encode(&message.body),
    });
    let priority = match message.urgency {
        Urgency::High => "10",
        Urgency::VeryLow | Urgency::Low | Urgency::Normal => "5",
    };
    let expiration = unix_seconds(now).saturating_add(message.ttl.as_secs());
    let mut headers = vec![
        ("authorization", format!("bearer {provider_token}")),
        ("apns-push-type", "alert".to_string()),
        ("apns-topic", topic.to_string()),
        ("apns-priority", priority.to_string()),
        ("apns-expiration", expiration.to_string()),
        ("content-type", "application/json".to_string()),
    ];
    if let Some(collapse_id) = &message.topic {
        headers.push(("apns-collapse-id", collapse_id.clone()));
    }
    ApnsRequest {
        path: format!("/3/device/{device_token}"),
        headers,
        body: body.to_string().into_bytes(),
    }
}

/// The `reason` of the JSON body of an APNs error.
fn reason(body: &[u8]) -> Option<String> {
    let body: Value = serde_json::from_slice(body).ok()?;
    body.get("reason")?.as_str().map(str::to_string)
}

/// `true` when APNs refused the provider token, and a new token can
/// succeed.
fn is_refused_provider_token(status: StatusCode, reason: Option<&str>) -> bool {
    status == StatusCode::FORBIDDEN
        && matches!(
            reason,
            Some("ExpiredProviderToken" | "InvalidProviderToken")
        )
}

/// What the APNs answer of `status` and `reason` means for the push. The
/// reason of a failure holds the status and the APNs reason, and never
/// the device token.
fn delivery(status: StatusCode, reason: Option<&str>) -> Delivery {
    let gone_token = matches!(reason, Some("BadDeviceToken" | "DeviceTokenNotForTopic"));
    match status {
        StatusCode::OK => Delivery::Delivered,
        StatusCode::GONE => Delivery::Gone,
        StatusCode::BAD_REQUEST if gone_token => Delivery::Gone,
        _ => Delivery::Failed(format!(
            "APNs answered {} {}",
            status.as_u16(),
            reason.unwrap_or("with no reason")
        )),
    }
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier;

    use super::*;

    /// 2026-10-06 12:00:00 UTC.
    const NOON: u64 = 1_791_288_000;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn key() -> SigningKey {
        SigningKey::from_slice(&[7; 32]).expect("a P-256 scalar")
    }

    fn message(urgency: Urgency, topic: Option<&str>) -> Message {
        Message {
            body: vec![0xfb, 0xff, 0x00, 0x01],
            ttl: Duration::from_secs(86400),
            urgency,
            topic: topic.map(str::to_string),
        }
    }

    fn header<'a>(request: &'a ApnsRequest, name: &str) -> Option<&'a str> {
        request
            .headers
            .iter()
            .find(|(header, _)| *header == name)
            .map(|(_, value)| value.as_str())
    }

    fn built(message: &Message) -> ApnsRequest {
        request(
            "a1b2c3",
            "co.pagis.mobile",
            "the.provider.token",
            message,
            at(NOON),
        )
    }

    #[test]
    fn the_body_is_a_placeholder_alert_and_the_push_body_as_p() {
        let request = built(&message(Urgency::High, Some("item-42")));

        let body: Value = serde_json::from_slice(&request.body).expect("a JSON body");
        assert_eq!(
            body,
            json!({
                "aps": {
                    "alert": { "title": "Pagis", "body": "Something needs you" },
                    "mutable-content": 1,
                    "sound": "default",
                },
                "p": "-_8AAQ",
            })
        );
        assert_eq!(request.path, "/3/device/a1b2c3");
    }

    #[test]
    fn each_header_of_a_high_urgency_push_with_a_topic() {
        let request = built(&message(Urgency::High, Some("item-42")));

        assert_eq!(
            request.headers,
            vec![
                ("authorization", "bearer the.provider.token".to_string()),
                ("apns-push-type", "alert".to_string()),
                ("apns-topic", "co.pagis.mobile".to_string()),
                ("apns-priority", "10".to_string()),
                ("apns-expiration", (NOON + 86400).to_string()),
                ("content-type", "application/json".to_string()),
                ("apns-collapse-id", "item-42".to_string()),
            ]
        );
    }

    #[test]
    fn each_urgency_but_high_has_priority_5() {
        for urgency in [Urgency::VeryLow, Urgency::Low, Urgency::Normal] {
            let request = built(&message(urgency, None));

            assert_eq!(header(&request, "apns-priority"), Some("5"), "{urgency:?}");
        }
    }

    #[test]
    fn the_expiration_is_now_plus_the_ttl() {
        let mut short = message(Urgency::Normal, None);
        short.ttl = Duration::from_secs(600);

        let request = built(&short);

        assert_eq!(
            header(&request, "apns-expiration"),
            Some((NOON + 600).to_string().as_str())
        );
    }

    #[test]
    fn a_push_with_no_topic_has_no_collapse_id() {
        let request = built(&message(Urgency::High, None));

        assert_eq!(header(&request, "apns-collapse-id"), None);
    }

    #[test]
    fn a_body_of_2800_bytes_gives_a_json_of_4096_bytes_or_less() {
        let mut largest = message(Urgency::High, Some(&"t".repeat(32)));
        largest.body = vec![0xff; 2800];

        let request = built(&largest);

        assert!(request.body.len() <= 4096, "{} bytes", request.body.len());
    }

    #[test]
    fn the_provider_token_verifies_with_the_public_key_and_holds_the_ids_and_the_time() {
        let key = key();

        let token = provider_token(&key, "ABC123DEFG", "DEF123GHIJ", at(NOON));

        let (signed, signature) = token.rsplit_once('.').expect("three parts");
        let (header, claims) = signed.split_once('.').expect("three parts");
        let part = |part: &str| -> Value {
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).expect("base64url")).expect("JSON")
        };
        assert_eq!(part(header), json!({ "alg": "ES256", "kid": "ABC123DEFG" }));
        assert_eq!(part(claims), json!({ "iss": "DEF123GHIJ", "iat": NOON }));
        let signature = Signature::from_slice(
            &URL_SAFE_NO_PAD
                .decode(signature)
                .expect("a base64url signature"),
        )
        .expect("an ES256 signature of 64 bytes");
        VerifyingKey::from(&key)
            .verify(signed.as_bytes(), &signature)
            .expect("the signature verifies with the public key");
    }

    #[test]
    fn each_answer_maps_to_its_delivery() {
        let failed = |text: &str| Delivery::Failed(text.to_string());
        let cases = [
            (200, None, Delivery::Delivered),
            (410, Some("Unregistered"), Delivery::Gone),
            (410, Some("ExpiredToken"), Delivery::Gone),
            (400, Some("BadDeviceToken"), Delivery::Gone),
            (400, Some("DeviceTokenNotForTopic"), Delivery::Gone),
            (400, Some("BadTopic"), failed("APNs answered 400 BadTopic")),
            (
                403,
                Some("ExpiredProviderToken"),
                failed("APNs answered 403 ExpiredProviderToken"),
            ),
            (
                429,
                Some("TooManyRequests"),
                failed("APNs answered 429 TooManyRequests"),
            ),
            (500, None, failed("APNs answered 500 with no reason")),
        ];
        for (status, reason, expected) in cases {
            let status = StatusCode::from_u16(status).expect("a status");

            assert_eq!(delivery(status, reason), expected, "{status} {reason:?}");
        }
    }

    #[test]
    fn only_a_403_for_an_expired_or_invalid_provider_token_drops_the_token() {
        assert!(is_refused_provider_token(
            StatusCode::FORBIDDEN,
            Some("ExpiredProviderToken")
        ));
        assert!(is_refused_provider_token(
            StatusCode::FORBIDDEN,
            Some("InvalidProviderToken")
        ));
        assert!(!is_refused_provider_token(
            StatusCode::FORBIDDEN,
            Some("Forbidden")
        ));
        assert!(!is_refused_provider_token(
            StatusCode::TOO_MANY_REQUESTS,
            Some("TooManyProviderTokenUpdates")
        ));
    }
}
