//! One-time codes (ADR-0013). The seed is a field on the
//! Credential record and never leaves the daemon; `vault__totp_fill`
//! fills the current code into a verified field through the daemon's
//! browser channel (see `fill`). The agent sees neither the seed nor the
//! code.
//!
//! The vault holds the store and the fill. There is no enrolment at
//! signup: the user enrols a seed in Workspace settings.

use totp_rs::{Algorithm, Builder, Secret, Totp};

use crate::VaultError;

/// The RFC 6238 defaults every authenticator app assumes.
const DIGITS: u8 = 6;
const SKEW: u16 = 1;
const STEP: u64 = 30;

/// The current code for one base32 seed, as an authenticator app shows
/// it. Spaces and lowercase are accepted, the way the sites that print
/// a seed in groups of four write it.
pub fn current_code(seed: &str) -> Result<String, VaultError> {
    Ok(totp(seed)?.generate_current().to_string())
}

/// True when the seed reads as a usable base32 TOTP secret. Settings
/// checks this before it stores one, so a typo surfaces at enrolment
/// and not in the middle of a login.
pub fn check_seed(seed: &str) -> Result<(), VaultError> {
    totp(seed).map(|_| ())
}

fn totp(seed: &str) -> Result<Totp, VaultError> {
    let normalized: String = seed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    if normalized.is_empty() {
        return Err(VaultError::BadTotpSeed("the seed is empty".to_string()));
    }
    let secret = Secret::try_from_base32(&normalized)
        .map_err(|error| VaultError::BadTotpSeed(format!("{error:?}")))?;
    Builder::new()
        .with_algorithm(Algorithm::SHA1)
        .with_digits(DIGITS)
        .with_skew(SKEW)
        .with_step_duration(STEP)
        .with_secret(secret)
        .build()
        .map_err(|error| VaultError::BadTotpSeed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC 6238 test seed, "12345678901234567890" in base32.
    const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn a_seed_yields_a_six_digit_code() {
        let code = current_code(SEED).unwrap();

        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()), "{code}");
    }

    #[test]
    fn spacing_and_case_do_not_matter() {
        assert_eq!(
            current_code("gezd gnbv gy3t qojq gezd gnbv gy3t qojq").unwrap(),
            current_code(SEED).unwrap()
        );
    }

    #[test]
    fn a_seed_that_is_not_base32_is_refused_at_enrolment() {
        assert!(check_seed("not base32 !!!").is_err());
        assert!(check_seed("   ").is_err());
        assert!(check_seed(SEED).is_ok());
    }
}
