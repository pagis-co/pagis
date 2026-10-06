//! The VAPID token of a Web Push (RFC 8292): an ES256 JWT that the VAPID
//! Key of the installation signs, sent with the public half of the key.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::SecretKey;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use url::Url;
use web_push_native::jwt_simple::algorithms::{ECDSAP256KeyPairLike, ES256KeyPair};
use web_push_native::jwt_simple::claims::Claims;
use web_push_native::jwt_simple::prelude::Duration;

/// The contact of an installation whose Public Origin is not `https`.
/// Apple refuses a token with no `sub`, and the RFC asks for a `mailto:`
/// or an `https` URL.
const PROJECT: &str = "https://github.com/pagis-co/pagis";

/// How long a token stays good. The `TTL` of a Web Push is apart from it.
const TOKEN_LIFETIME_HOURS: u64 = 12;

/// The VAPID Key and the contact that sign each Web Push.
pub(crate) struct Vapid {
    key: ES256KeyPair,
    /// The public half of the key, as `k` sends it: the base64url of the
    /// uncompressed point.
    public_key: String,
    contact: String,
}

impl Vapid {
    pub(crate) fn new(key: &SecretKey, public_origin: &str) -> Result<Self, String> {
        let public_key =
            URL_SAFE_NO_PAD.encode(key.public_key().to_encoded_point(false).as_bytes());
        let key = ES256KeyPair::from_bytes(&key.to_bytes())
            .map_err(|error| format!("the VAPID Key is not an ES256 key: {error}"))?;
        Ok(Self {
            key,
            public_key,
            contact: contact(public_origin),
        })
    }

    /// The `Authorization` header of a Web Push to `endpoint`.
    ///
    /// `aud` is the origin of the endpoint, with its port when the port
    /// is not the default one. `VapidSignature::sign` of `web-push-native`
    /// drops the port, so this signs the claims itself, with the same
    /// `jwt-simple` key.
    pub(crate) fn authorization(&self, endpoint: &Url) -> Result<String, String> {
        let claims = Claims::create(Duration::from_hours(TOKEN_LIFETIME_HOURS))
            .with_audience(endpoint.origin().ascii_serialization())
            .with_subject(&self.contact);
        let token = self
            .key
            .sign(claims)
            .map_err(|error| format!("sign the VAPID token: {error}"))?;
        Ok(format!("vapid t={token}, k={}", self.public_key))
    }
}

/// The `sub` of each token: the Public Origin when it is `https`, or else
/// the project.
fn contact(public_origin: &str) -> String {
    match Url::parse(public_origin) {
        Ok(origin) if origin.scheme() == "https" => origin.origin().ascii_serialization(),
        _ => PROJECT.to_string(),
    }
}
