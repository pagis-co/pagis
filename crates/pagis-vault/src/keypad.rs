//! The Keypad Code (ADR-0021): the digits a caller enters to
//! prove a Trust Tier above Unknown on a call Pagis answers.
//!
//! Each Workspace holds one code, so the code is the Person's. It is 6
//! to 8 digits, and it is stored as an Argon2 hash in the daemon-owned
//! secret store of ADR-0013, under a name that carries the Workspace.
//! [`MIN_DIGITS`] and [`MAX_DIGITS`] are the one length rule: the
//! telephony keypad reads them too.
//! A hash is not a secret to read back: the card in Settings says the
//! code has no reveal and no export, and this module has no accessor
//! that returns one.
//!
//! Only the daemon's tier check verifies a code. No tool reads it, no
//! prompt carries it, and the digits reach the daemon from the media
//! hub's typed keypad events, so they are not in the transcript and not
//! in the recording.

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use pagis_core::{SecretError, SecretStore, WorkspaceId, workspace_secret_name};

/// Where the secret store files the hash of one Workspace's Keypad Code.
pub fn keypad_code_secret_name(workspace_id: &WorkspaceId) -> String {
    workspace_secret_name(workspace_id, "keypad_code_6_to_8_hash")
}

/// The shortest code the user may set. Six digits give a million codes.
pub const MIN_DIGITS: usize = 6;
/// The longest code the user may set. A caller who enters this many
/// digits needs no `#`.
pub const MAX_DIGITS: usize = 8;

/// The code the user tried to set is not a code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeypadError {
    #[error("a keypad code is {MIN_DIGITS} to {MAX_DIGITS} digits")]
    BadLength,
    #[error("a keypad code is digits only")]
    NotDigits,
    #[error("the keypad code store is unusable: {0}")]
    Store(String),
}

impl From<SecretError> for KeypadError {
    fn from(error: SecretError) -> Self {
        KeypadError::Store(error.to_string())
    }
}

/// The one Keypad Code of one Workspace.
pub struct KeypadCode<'a> {
    secrets: &'a dyn SecretStore,
    name: String,
}

impl<'a> KeypadCode<'a> {
    pub fn new(secrets: &'a dyn SecretStore, workspace_id: &WorkspaceId) -> Self {
        Self {
            secrets,
            name: keypad_code_secret_name(workspace_id),
        }
    }

    /// True when a code is set. It is the only thing the API reports
    /// about the code.
    pub fn is_set(&self) -> Result<bool, KeypadError> {
        Ok(self.secrets.get(&self.name)?.is_some())
    }

    /// Set or replace the code. The digits are hashed here and the
    /// plaintext is dropped with this call.
    pub fn set(&self, digits: &str) -> Result<(), KeypadError> {
        check_shape(digits)?;
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(digits.as_bytes(), &salt)
            .map_err(|error| KeypadError::Store(error.to_string()))?
            .to_string();
        self.secrets.set(&self.name, &hash)?;
        Ok(())
    }

    /// Forget the code. Inbound calls then reach Unknown only, because
    /// no caller can prove a tier.
    pub fn clear(&self) -> Result<(), KeypadError> {
        self.secrets.delete(&self.name)?;
        Ok(())
    }

    /// Whether these digits are the code. `false` when no code is set,
    /// because a Workspace with no code proves nothing, and `false` for
    /// digits that are not the shape of a code, whatever the store holds.
    pub fn verify(&self, digits: &str) -> Result<bool, KeypadError> {
        if check_shape(digits).is_err() {
            return Ok(false);
        }
        let Some(stored) = self.secrets.get(&self.name)? else {
            return Ok(false);
        };
        let hash = PasswordHash::new(&stored)
            .map_err(|error: argon2::password_hash::Error| KeypadError::Store(error.to_string()))?;
        Ok(Argon2::default()
            .verify_password(digits.as_bytes(), &hash)
            .is_ok())
    }
}

fn check_shape(digits: &str) -> Result<(), KeypadError> {
    if !digits.chars().all(|digit| digit.is_ascii_digit()) {
        return Err(KeypadError::NotDigits);
    }
    if digits.len() < MIN_DIGITS || digits.len() > MAX_DIGITS {
        return Err(KeypadError::BadLength);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use argon2::Argon2;
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::{PasswordHasher, SaltString};
    use pagis_core::{MemorySecretStore, SecretStore, WorkspaceId};

    use super::{KeypadCode, KeypadError, keypad_code_secret_name};

    #[test]
    fn the_right_code_verifies_and_a_wrong_one_does_not() {
        let secrets = MemorySecretStore::default();
        let workspace_id = WorkspaceId::generate();
        let code = KeypadCode::new(&secrets, &workspace_id);
        code.set("246813").unwrap();

        assert!(code.is_set().unwrap());
        assert!(code.verify("246813").unwrap());
        assert!(!code.verify("246814").unwrap());
    }

    #[test]
    fn the_store_holds_a_hash_and_not_the_digits() {
        let secrets = MemorySecretStore::default();
        let workspace_id = WorkspaceId::generate();
        KeypadCode::new(&secrets, &workspace_id)
            .set("246813")
            .unwrap();

        let stored = secrets
            .get(&keypad_code_secret_name(&workspace_id))
            .unwrap()
            .unwrap();
        assert!(stored.starts_with("$argon2"));
        assert!(!stored.contains("246813"));
    }

    /// Each Workspace holds its own code. One person's code does not
    /// replace another's, and it proves nothing in another Workspace.
    #[test]
    fn each_workspace_holds_its_own_code() {
        let secrets = MemorySecretStore::default();
        let ada = WorkspaceId::generate();
        let grace = WorkspaceId::generate();
        KeypadCode::new(&secrets, &ada).set("246813").unwrap();
        KeypadCode::new(&secrets, &grace).set("135792").unwrap();

        let ada_code = KeypadCode::new(&secrets, &ada);
        let grace_code = KeypadCode::new(&secrets, &grace);
        assert!(ada_code.verify("246813").unwrap());
        assert!(!ada_code.verify("135792").unwrap());
        assert!(grace_code.verify("135792").unwrap());
        assert!(!grace_code.verify("246813").unwrap());

        ada_code.clear().unwrap();
        assert!(!ada_code.is_set().unwrap());
        assert!(grace_code.is_set().unwrap());
        assert!(
            !KeypadCode::new(&secrets, &WorkspaceId::generate())
                .is_set()
                .unwrap()
        );
    }

    #[test]
    fn a_workspace_with_no_code_proves_nothing() {
        let secrets = MemorySecretStore::default();
        let workspace_id = WorkspaceId::generate();
        let code = KeypadCode::new(&secrets, &workspace_id);

        assert!(!code.is_set().unwrap());
        assert!(!code.verify("246813").unwrap());
    }

    /// An Argon2 hash of some digits, as the store holds one.
    fn hash_of(digits: &str) -> String {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(digits.as_bytes(), &salt)
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_code_is_six_to_eight_digits() {
        let secrets = MemorySecretStore::default();
        let workspace_id = WorkspaceId::generate();
        let code = KeypadCode::new(&secrets, &workspace_id);

        assert_eq!(code.set("2468"), Err(KeypadError::BadLength));
        assert_eq!(code.set("24681"), Err(KeypadError::BadLength));
        assert_eq!(code.set("123456789"), Err(KeypadError::BadLength));
        assert_eq!(code.set("12ab56"), Err(KeypadError::NotDigits));
        assert_eq!(code.set("246813"), Ok(()));
        assert_eq!(code.set("24681357"), Ok(()));
    }

    /// Digits shorter than a code never verify, even against a hash of
    /// those same digits: a short entry is never a guess the code can
    /// match.
    #[test]
    fn digits_shorter_than_six_never_verify() {
        let secrets = MemorySecretStore::default();
        let workspace_id = WorkspaceId::generate();
        secrets
            .set(&keypad_code_secret_name(&workspace_id), &hash_of("2468"))
            .unwrap();

        assert!(
            !KeypadCode::new(&secrets, &workspace_id)
                .verify("2468")
                .unwrap()
        );
    }

    #[test]
    fn a_cleared_code_is_gone() {
        let secrets = MemorySecretStore::default();
        let workspace_id = WorkspaceId::generate();
        let code = KeypadCode::new(&secrets, &workspace_id);
        code.set("246813").unwrap();

        code.clear().unwrap();

        assert!(!code.is_set().unwrap());
    }
}
