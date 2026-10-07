//! The values of a registration, and the check of each field that a
//! Mobile App sends.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// The random id in the endpoint of a registration: 128 bits as
/// base64url. It names no device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistrationId(String);

impl RegistrationId {
    pub(crate) fn random() -> Self {
        Self(random_base64url::<16>())
    }

    /// An id in the form that the relay gives, or `None`.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(value).ok()?;
        (bytes.len() == 16).then(|| Self(value.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The secret that changes or removes a registration: 256 bits as
/// base64url. The relay gives it once and keeps only its SHA-256.
pub(crate) struct Secret(String);

impl Secret {
    pub(crate) fn random() -> Self {
        Self(random_base64url::<32>())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The SHA-256 of a secret as the caller sent it.
pub(crate) fn secret_hash(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

/// `N` random bytes as base64url.
pub(crate) fn random_base64url<const N: usize>() -> String {
    let mut bytes = [0u8; N];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The push service of a registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// APNs, in its production or its sandbox environment.
    Ios(Environment),
    /// FCM.
    Android,
}

impl Platform {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Ios(_) => "ios",
            Self::Android => "android",
        }
    }

    pub(crate) fn environment(self) -> Option<&'static str> {
        match self {
            Self::Ios(Environment::Production) => Some("production"),
            Self::Ios(Environment::Sandbox) => Some("sandbox"),
            Self::Android => None,
        }
    }

    /// The platform of a [`name`](Self::name) and an
    /// [`environment`](Self::environment), or `None` for another pair.
    pub(crate) fn from_names(name: &str, environment: Option<&str>) -> Option<Self> {
        match (name, environment) {
            ("ios", Some("production")) => Some(Self::Ios(Environment::Production)),
            ("ios", Some("sandbox")) => Some(Self::Ios(Environment::Sandbox)),
            ("android", None) => Some(Self::Android),
            _ => None,
        }
    }
}

/// The APNs environment that the app build talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Production,
    Sandbox,
}

/// The device token that APNs or FCM gives the app: 1 to 4096
/// characters of `[A-Za-z0-9_:-]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token(String);

impl Token {
    const MAX_LEN: usize = 4096;

    fn parse(value: &str) -> Option<Self> {
        let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-');
        (!value.is_empty() && value.len() <= Self::MAX_LEN && value.chars().all(allowed))
            .then(|| Self(value.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The VAPID public key that each push to the endpoint must be signed
/// with: an uncompressed P-256 point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VapidKey([u8; 65]);

impl VapidKey {
    fn parse(value: &str) -> Option<Self> {
        Self::from_bytes(&URL_SAFE_NO_PAD.decode(value).ok()?)
    }

    /// The key of 65 bytes, or `None` when they are not an uncompressed
    /// P-256 point.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let bytes: [u8; 65] = bytes.try_into().ok()?;
        // A 65-byte SEC1 encoding is the uncompressed form; the parse
        // checks that the point is on the curve.
        p256::PublicKey::from_sec1_bytes(&bytes).ok()?;
        Some(Self(bytes))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// What a Mobile App installation asks to register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewRegistration {
    pub(crate) platform: Platform,
    pub(crate) token: Token,
    pub(crate) vapid_key: VapidKey,
}

/// A body that the relay refuses. The message names the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invalid(pub(crate) String);

/// Read `{platform, environment?, token, vapid_key}`.
pub(crate) fn new_registration(body: &[u8]) -> Result<NewRegistration, Invalid> {
    let fields = object(body)?;
    let platform = match string(&fields, "platform")? {
        Some("ios") => Platform::Ios(match string(&fields, "environment")? {
            Some("production") => Environment::Production,
            Some("sandbox") => Environment::Sandbox,
            _ => {
                return Err(Invalid(
                    "environment must be production or sandbox for platform ios".into(),
                ));
            }
        }),
        Some("android") => {
            if string(&fields, "environment")?.is_some() {
                return Err(Invalid(
                    "environment is only for platform ios; leave it out for android".into(),
                ));
            }
            Platform::Android
        }
        _ => return Err(Invalid("platform must be ios or android".into())),
    };
    let vapid_key = string(&fields, "vapid_key")?
        .and_then(VapidKey::parse)
        .ok_or_else(|| {
            Invalid("vapid_key must be an uncompressed P-256 public key in base64url".into())
        })?;
    Ok(NewRegistration {
        platform,
        token: token(&fields)?,
        vapid_key,
    })
}

/// Read `{token}`.
pub(crate) fn token_change(body: &[u8]) -> Result<Token, Invalid> {
    token(&object(body)?)
}

fn token(fields: &Map<String, Value>) -> Result<Token, Invalid> {
    string(fields, "token")?
        .and_then(Token::parse)
        .ok_or_else(|| {
            Invalid("token must be 1 to 4096 characters of A-Z, a-z, 0-9, _, : and -".into())
        })
}

fn object(body: &[u8]) -> Result<Map<String, Value>, Invalid> {
    match serde_json::from_slice(body) {
        Ok(Value::Object(fields)) => Ok(fields),
        _ => Err(Invalid("the body must be a JSON object".into())),
    }
}

/// The string `name`, or `None` when it is absent or `null`.
fn string<'a>(fields: &'a Map<String, Value>, name: &str) -> Result<Option<&'a str>, Invalid> {
    match fields.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(Invalid(format!("{name} must be a string"))),
    }
}
