//! The VAPID Key of the installation.

use base64::{
    Engine as _, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use p256::SecretKey;
use pagis_core::{SecretError, SecretStore};
use rand::RngCore;

/// The name of the private half of the VAPID Key in the secret store.
pub const VAPID_KEY_SECRET: &str = "vapid_private_key";
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
pub fn vapid_key(secrets: &dyn SecretStore) -> Result<SecretKey, SecretError> {
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
