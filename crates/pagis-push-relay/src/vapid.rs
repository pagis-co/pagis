//! The check of the VAPID token of a Web Push (RFC 8292).
//!
//! A server signs each Web Push with an ES256 JWT and sends the public
//! half of its VAPID Key with it. The relay accepts the push only when
//! the key is the one that the registration binds, the signature
//! verifies with it, the audience is the relay and the token expires in
//! the next 24 hours. The check uses `p256`, `base64` and `serde_json`
//! and no JWT crate.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, header};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde_json::{Value, json};

use crate::registration::VapidKey;

/// The longest time from now to the `exp` of a token (RFC 8292
/// section 2).
const MAX_LIFETIME_SECONDS: u64 = 24 * 60 * 60;

/// The parts of `Authorization: vapid t=<jwt>, k=<key>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Authorization<'a> {
    token: &'a str,
    key: &'a str,
}

impl<'a> Authorization<'a> {
    /// The `vapid` authorization of `headers`, or `None` when it is
    /// absent or has another form.
    pub(crate) fn from_headers(headers: &'a HeaderMap) -> Option<Self> {
        let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
        let (scheme, parameters) = value.trim().split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("vapid") {
            return None;
        }
        let mut token = None;
        let mut key = None;
        for parameter in parameters.split(',') {
            let (name, value) = parameter.split_once('=')?;
            let value = value.trim();
            match name.trim() {
                "t" => token = Some(value),
                "k" => key = Some(value),
                _ => {}
            }
        }
        Some(Self {
            token: token.filter(|token| !token.is_empty())?,
            key: key.filter(|key| !key.is_empty())?,
        })
    }
}

/// Why the relay refuses a token. It names the check that failed and
/// never holds the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InvalidToken(pub(crate) &'static str);

/// Check `authorization` against the key that the registration binds,
/// the origin of the relay and the time `now`.
pub(crate) fn verify(
    authorization: &Authorization<'_>,
    vapid_key: &VapidKey,
    audience: &str,
    now: SystemTime,
) -> Result<(), InvalidToken> {
    // A sender may pad `k`; the key is the same.
    let key = URL_SAFE_NO_PAD
        .decode(authorization.key.trim_end_matches('='))
        .map_err(|_| InvalidToken("k is not base64url"))?;
    if key != vapid_key.as_bytes() {
        return Err(InvalidToken("k is not the key of the registration"));
    }

    // `<header>.<claims>.<signature>`; the signature signs the first two.
    let (signed, signature) = authorization
        .token
        .rsplit_once('.')
        .ok_or(InvalidToken("the token is not a JWS in three parts"))?;
    let (header, claims) = signed
        .split_once('.')
        .filter(|(_, claims)| !claims.contains('.'))
        .ok_or(InvalidToken("the token is not a JWS in three parts"))?;
    let header = json_part(header).ok_or(InvalidToken("the header is not JSON"))?;
    if header != json!({ "typ": "JWT", "alg": "ES256" }) && header != json!({ "alg": "ES256" }) {
        return Err(InvalidToken("the header is not an ES256 JWT"));
    }

    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .ok()
        .and_then(|bytes| Signature::from_slice(&bytes).ok())
        .ok_or(InvalidToken("the signature is not an ES256 signature"))?;
    let verifying_key = VerifyingKey::from_sec1_bytes(vapid_key.as_bytes())
        .map_err(|_| InvalidToken("the key of the registration is not a P-256 key"))?;
    verifying_key
        .verify(signed.as_bytes(), &signature)
        .map_err(|_| InvalidToken("the signature does not verify with k"))?;

    let claims = json_part(claims).ok_or(InvalidToken("the claims are not JSON"))?;
    if claims.get("aud").and_then(Value::as_str) != Some(audience) {
        return Err(InvalidToken("aud is not the origin of the relay"));
    }
    let exp = claims
        .get("exp")
        .and_then(Value::as_u64)
        .ok_or(InvalidToken("exp is not a time in whole seconds"))?;
    let now = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    if exp <= now {
        return Err(InvalidToken("exp is in the past"));
    }
    if exp > now + MAX_LIFETIME_SECONDS {
        return Err(InvalidToken("exp is over 24 hours ahead"));
    }
    Ok(())
}

/// The JSON of one base64url part of a JWS.
fn json_part(part: &str) -> Option<Value> {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).ok()?).ok()
}
