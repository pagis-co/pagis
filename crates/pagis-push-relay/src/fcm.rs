//! The transport of the `android` platform: one request to the FCM HTTP v1
//! API for each push.
//!
//! The relay sends a data message with no `notification` member. The
//! Android messaging service of the Mobile App decrypts the body and shows
//! the Notification. The OAuth 2 access token of the service account comes
//! from a [`TokenSource`]: `gcp_auth` in the relay, a fixed token in a
//! test.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use gcp_auth::{CustomServiceAccount, TokenProvider};
use reqwest::StatusCode;
use serde_json::{Value, json};
use url::Url;

use crate::transport::{Delivery, Message, Registration, Transport, Urgency, error_chain};

/// The base URL of FCM.
pub const FCM_BASE_URL: &str = "https://fcm.googleapis.com";

/// The OAuth 2 scope that sends an FCM message.
const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";

/// The longest TTL that FCM takes: four weeks.
const MAX_TTL: Duration = Duration::from_secs(28 * 24 * 60 * 60);

/// How long one request may take, from the connection to the answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

const FCM_ERROR_TYPE: &str = "type.googleapis.com/google.firebase.fcm.v1.FcmError";
const BAD_REQUEST_TYPE: &str = "type.googleapis.com/google.rpc.BadRequest";

/// Where the FCM transport gets the OAuth 2 access token of the service
/// account, for the scope of FCM.
#[async_trait]
pub trait TokenSource: Send + Sync {
    /// A valid access token, or why there is none.
    async fn access_token(&self) -> Result<String, String>;
}

/// The access tokens of the service account of a JSON key file. `gcp_auth`
/// keeps a token and makes a new one before it expires.
pub struct ServiceAccount(CustomServiceAccount);

impl ServiceAccount {
    /// Read the JSON key file at `path`.
    pub fn read(path: &Path) -> Result<Self, FcmError> {
        CustomServiceAccount::from_file(path)
            .map(Self)
            .map_err(|source| FcmError::Credentials {
                path: path.to_path_buf(),
                source,
            })
    }
}

#[async_trait]
impl TokenSource for ServiceAccount {
    async fn access_token(&self) -> Result<String, String> {
        self.0
            .token(&[SCOPE])
            .await
            .map(|token| token.as_str().to_string())
            .map_err(|error| error_chain(&error))
    }
}

/// An FCM setting that stops the relay at start. The message names the
/// variable and the file, and never holds a key.
#[derive(Debug, thiserror::Error)]
pub enum FcmError {
    #[error(
        "PUSH_RELAY_FCM_CREDENTIALS_PATH names {}, which is not the JSON key of a \
         service account",
        .path.display()
    )]
    Credentials {
        path: PathBuf,
        source: gcp_auth::Error,
    },
    #[error("the FCM base URL {0:?} is not an http or https URL")]
    BaseUrl(String),
    #[error("build the FCM client: {0}")]
    Client(#[from] reqwest::Error),
}

/// The client of FCM for the Firebase project of the Mobile App.
pub struct FcmTransport {
    client: reqwest::Client,
    send_url: Url,
    tokens: Arc<dyn TokenSource>,
}

impl FcmTransport {
    /// A transport that sends to the project `project_id` at `base_url`
    /// with the access tokens of `tokens`.
    pub fn new(
        project_id: &str,
        tokens: Arc<dyn TokenSource>,
        base_url: &str,
    ) -> Result<Self, FcmError> {
        let bad_url = || FcmError::BaseUrl(base_url.to_string());
        let mut send_url = Url::parse(base_url).map_err(|_| bad_url())?;
        if !matches!(send_url.scheme(), "http" | "https") {
            return Err(bad_url());
        }
        // Each segment is escaped, so a project id cannot change the path.
        send_url
            .path_segments_mut()
            .map_err(|()| bad_url())?
            .pop_if_empty()
            .extend(["v1", "projects", project_id, "messages:send"]);
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            send_url,
            tokens,
        })
    }
}

#[async_trait]
impl Transport for FcmTransport {
    async fn send(&self, registration: &Registration, message: &Message) -> Delivery {
        let token = match self.tokens.access_token().await {
            Ok(token) => token,
            Err(reason) => return Delivery::Failed(format!("no FCM access token: {reason}")),
        };
        let response = self
            .client
            .post(self.send_url.clone())
            .bearer_auth(token)
            .json(&body(&registration.token, message))
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                return Delivery::Failed(format!("FCM did not answer: {}", error_chain(&error)));
            }
        };
        let status = response.status();
        let body = response.bytes().await.unwrap_or_default();
        let answer = Answer::read(status, &body);
        let delivery = answer.delivery();
        if delivery == Delivery::Gone {
            tracing::info!(
                status = status.as_u16(),
                code = answer.code.as_deref(),
                "FCM says that the device token is gone"
            );
        }
        delivery
    }
}

/// The request body that sends `message` to the device of
/// `device_token`: a data message that holds the push body as `p`.
pub(crate) fn body(device_token: &str, message: &Message) -> Value {
    let priority = match message.urgency {
        Urgency::High => "HIGH",
        Urgency::VeryLow | Urgency::Low | Urgency::Normal => "NORMAL",
    };
    let ttl = message.ttl.min(MAX_TTL).as_secs();
    let mut android = json!({ "priority": priority, "ttl": format!("{ttl}s") });
    if let Some(topic) = &message.topic {
        android["collapse_key"] = json!(topic);
    }
    json!({
        "message": {
            "token": device_token,
            "data": { "p": URL_SAFE_NO_PAD.encode(&message.body) },
            "android": android,
        }
    })
}

/// The parts of an FCM answer that decide the delivery.
struct Answer {
    status: StatusCode,
    /// The `errorCode` of the `FcmError` detail, or else the `status` of
    /// the error.
    code: Option<String>,
    /// A `BadRequest` detail names the field `message.token`.
    token_is_invalid: bool,
}

impl Answer {
    fn read(status: StatusCode, body: &[u8]) -> Self {
        let error = serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|body| body.get("error").cloned())
            .unwrap_or(Value::Null);
        let details: &[Value] = error
            .get("details")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice);
        let of_type = |kind: &'static str| {
            details
                .iter()
                .filter(move |detail| detail.get("@type").and_then(Value::as_str) == Some(kind))
        };
        let code = of_type(FCM_ERROR_TYPE)
            .find_map(|detail| detail.get("errorCode").and_then(Value::as_str))
            .or_else(|| error.get("status").and_then(Value::as_str))
            .map(str::to_string);
        let token_is_invalid = of_type(BAD_REQUEST_TYPE)
            .filter_map(|detail| detail.get("fieldViolations").and_then(Value::as_array))
            .flatten()
            .any(|violation| {
                violation.get("field").and_then(Value::as_str) == Some("message.token")
            });
        Self {
            status,
            code,
            token_is_invalid,
        }
    }

    /// `INVALID_ARGUMENT` is gone only when it names the token: the same
    /// code comes for a bad payload, and a fault in the relay must not
    /// remove every registration. The reason of a failure holds the status
    /// and the error code, and never the device token.
    fn delivery(&self) -> Delivery {
        match (self.status, self.code.as_deref()) {
            (StatusCode::OK, _) => Delivery::Delivered,
            (StatusCode::NOT_FOUND, Some("UNREGISTERED")) => Delivery::Gone,
            (StatusCode::BAD_REQUEST, Some("INVALID_ARGUMENT")) if self.token_is_invalid => {
                Delivery::Gone
            }
            (status, code) => Delivery::Failed(format!(
                "FCM answered {} {}",
                status.as_u16(),
                code.unwrap_or("with no error code")
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    /// The size of the data that FCM counts: each key and each value.
    fn data_bytes(data: &Map<String, Value>) -> usize {
        data.iter()
            .map(|(key, value)| key.len() + value.as_str().map_or(0, str::len))
            .sum()
    }

    fn message(body: Vec<u8>, ttl: Duration) -> Message {
        Message {
            body,
            ttl,
            urgency: Urgency::High,
            topic: Some("t".repeat(32)),
        }
    }

    #[test]
    fn a_body_of_2800_bytes_gives_data_of_4096_bytes_or_less() {
        let body = body("token", &message(vec![0xff; 2800], Duration::from_secs(60)));

        let data = body["message"]["data"].as_object().expect("a data map");
        assert!(data_bytes(data) <= 4096, "{} bytes", data_bytes(data));
    }

    #[test]
    fn a_ttl_over_four_weeks_is_four_weeks() {
        let longest = body("token", &message(vec![1], Duration::from_secs(2_419_200)));
        let over = body("token", &message(vec![1], Duration::from_secs(2_419_201)));

        assert_eq!(longest["message"]["android"]["ttl"], "2419200s");
        assert_eq!(over["message"]["android"]["ttl"], "2419200s");
    }

    #[test]
    fn a_base_url_that_is_not_http_is_refused() {
        struct NoToken;
        #[async_trait]
        impl TokenSource for NoToken {
            async fn access_token(&self) -> Result<String, String> {
                Err("no token".to_string())
            }
        }

        for base_url in ["fcm.googleapis.com", "mailto:fcm@example.com"] {
            let refused = FcmTransport::new("pagis-mobile", Arc::new(NoToken), base_url);

            assert!(
                matches!(refused, Err(FcmError::BaseUrl(_))),
                "{base_url} is refused"
            );
        }
        let transport =
            FcmTransport::new("a/b", Arc::new(NoToken), FCM_BASE_URL).expect("the FCM transport");
        assert_eq!(
            transport.send_url.as_str(),
            "https://fcm.googleapis.com/v1/projects/a%2Fb/messages:send",
            "a project id stays one segment"
        );
    }
}
