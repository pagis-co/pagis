//! The custody split of every tenant secret in the database
//! (ADR-0013): the secret sits in a row as ciphertext, and the
//! Tenant Data Key that opens it sits in `secrets.enc`, which the
//! Installation Key seals.
//!
//! One key per tenant. Unsealing a row unwraps that Workspace's key and
//! no other, so a bug that reads another tenant's row still reads
//! ciphertext it holds no key for. The Installation Key wraps every one
//! of them, because `secrets.enc` is sealed with it.
//!
//! The vault's Credentials and a Google Connection's refresh token are
//! both sealed this way, and there is one key store for both.
//! There is no export path and no accessor that hands a plaintext
//! secret to a caller that does not need it.
//!
//! A Tenant Data Key also derives a key for each other use, with HKDF
//! and the fixed label of that use. The Forget suppression key is such
//! a key. It lives in `secrets.enc` with the Tenant Data Key and never
//! in the database (ADR-0008).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

use crate::secrets::{SecretStore, workspace_secret_name};
use crate::{ForgetKeys, SealedSecret, SuppressionKey, WorkspaceId};

/// A Tenant Data Key that cannot be read, minted or used.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("tenant key error: {0}")]
pub struct SealError(pub String);

/// The name `secrets.enc` files one Workspace's Tenant Data Key under.
pub fn data_key_name(workspace_id: &WorkspaceId) -> String {
    workspace_secret_name(workspace_id, "vault_data_key")
}

const NONCE_BYTES: usize = 24;

/// One Workspace's Tenant Data Key, read from `secrets.enc` and minted
/// there on first use.
pub struct DataKey {
    key: [u8; 32],
}

impl DataKey {
    /// Load one Workspace's data key, creating it on the first use by
    /// that Workspace.
    ///
    /// A new key goes in only through the create-if-absent operation of
    /// the store, and the load uses the key that the store answers. Two
    /// first uses that race therefore agree on the key that
    /// `secrets.enc` holds, and a key that loses the race is dropped.
    pub fn load(secrets: &dyn SecretStore, workspace_id: &WorkspaceId) -> Result<Self, SealError> {
        let mut minted = [0u8; 32];
        rand::rng().fill_bytes(&mut minted);
        let stored = secrets
            .get_or_insert(&data_key_name(workspace_id), &hex_encode(&minted))
            .map_err(|error| SealError(error.to_string()))?;
        Self::from_hex(&stored)
    }

    /// A key from raw bytes; tests use it in place of the keychain.
    pub fn from_bytes(key: [u8; 32]) -> Self {
        Self { key }
    }

    fn from_hex(stored: &str) -> Result<Self, SealError> {
        let bytes = hex_decode(stored)
            .ok_or_else(|| SealError("the Tenant Data Key is not hex".to_string()))?;
        let key: [u8; 32] = bytes
            .try_into()
            .map_err(|_| SealError("the Tenant Data Key is not 32 bytes".to_string()))?;
        Ok(Self { key })
    }

    /// A key for one other use of the Tenant Data Key: HKDF-SHA256 over
    /// it with the fixed `info` label of that use. A distinct label
    /// gives an independent key for each use (RFC 5869), so one key for
    /// each Workspace serves every use.
    fn derive(&self, info: &[u8]) -> [u8; 32] {
        let mut derived = [0u8; 32];
        Hkdf::<Sha256>::new(None, &self.key)
            .expand(info, &mut derived)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        derived
    }

    /// Seal one secret for storage.
    pub fn seal(&self, plaintext: &str) -> Result<SealedSecret, SealError> {
        let mut nonce = [0u8; NONCE_BYTES];
        rand::rng().fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.key));
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext.as_bytes())
            .map_err(|_| SealError("cannot seal the secret".to_string()))?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&ciphertext);
        Ok(SealedSecret(sealed))
    }

    /// Open one sealed secret.
    pub fn open(&self, sealed: &SealedSecret) -> Result<String, SealError> {
        if sealed.0.len() <= NONCE_BYTES {
            return Err(SealError("the sealed secret is truncated".to_string()));
        }
        let (nonce, ciphertext) = sealed.0.split_at(NONCE_BYTES);
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.key));
        let plaintext = cipher
            .decrypt(XNonce::from_slice(nonce), ciphertext)
            .map_err(|_| SealError("cannot open the sealed secret".to_string()))?;
        String::from_utf8(plaintext)
            .map_err(|_| SealError("the sealed secret is not text".to_string()))
    }
}

/// The Tenant Data Keys the daemon has opened, one per Workspace.
///
/// A key is minted on the first use by that Workspace and cached for the
/// daemon's life: every seal and every open goes through [`Self::of`],
/// so no code path can reach a key of the wrong tenant. The daemon holds
/// one of these, and the Vault, the Google broker and Forget share it,
/// so one Workspace has one key in memory.
pub struct TenantKeys {
    secrets: Arc<dyn SecretStore>,
    keys: Mutex<HashMap<WorkspaceId, Arc<DataKey>>>,
}

impl TenantKeys {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            secrets,
            keys: Mutex::new(HashMap::new()),
        }
    }

    /// One Workspace's key.
    ///
    /// The cache lock stays held across the load, so one holder loads
    /// each Workspace's key once. A first load is rare and short, so
    /// one lock for all Workspaces is fast enough.
    pub fn of(&self, workspace_id: &WorkspaceId) -> Result<Arc<DataKey>, SealError> {
        let mut keys = self.keys.lock().expect("tenant key lock");
        if let Some(key) = keys.get(workspace_id) {
            return Ok(Arc::clone(key));
        }
        let key = Arc::new(DataKey::load(self.secrets.as_ref(), workspace_id)?);
        keys.insert(workspace_id.clone(), Arc::clone(&key));
        Ok(key)
    }
}

/// The `info` label of the Forget suppression key. A change of it
/// changes every suppression identity and ends every Forget.
const FORGET_SUPPRESSION: &[u8] = b"pagis forget suppression";

/// The suppression key of a Workspace is one more use of its Tenant
/// Data Key. It lives only in `secrets.enc`, like every other key of the
/// Workspace, and not in the database beside the identities it keys
/// (ADR-0008).
impl ForgetKeys for TenantKeys {
    fn suppression_key(&self, workspace_id: &WorkspaceId) -> Result<SuppressionKey, SealError> {
        Ok(SuppressionKey::derived(
            self.of(workspace_id)?.derive(FORGET_SUPPRESSION),
        ))
    }
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretError;
    use crate::secrets::MemorySecretStore;

    #[test]
    fn a_secret_round_trips_and_never_appears_in_its_ciphertext() {
        let key = DataKey::from_bytes([7u8; 32]);

        let sealed = key.seal("hunter2-and-then-some").unwrap();

        assert_eq!(key.open(&sealed).unwrap(), "hunter2-and-then-some");
        assert!(
            !sealed.0.windows(7).any(|window| window == b"hunter2"),
            "the plaintext leaked into the stored bytes"
        );
        // The debug rendering never prints the bytes.
        assert_eq!(
            format!("{sealed:?}"),
            format!("SealedSecret({} bytes)", sealed.0.len())
        );
    }

    #[test]
    fn two_seals_of_one_secret_differ() {
        let key = DataKey::from_bytes([7u8; 32]);
        assert_ne!(key.seal("same").unwrap(), key.seal("same").unwrap());
    }

    #[test]
    fn another_key_cannot_open_it() {
        let sealed = DataKey::from_bytes([7u8; 32]).seal("secret").unwrap();

        assert!(DataKey::from_bytes([8u8; 32]).open(&sealed).is_err());
    }

    #[test]
    fn the_data_key_is_minted_once_and_reused() {
        let secrets = MemorySecretStore::default();
        let workspace_id = WorkspaceId::from("ws-1".to_string());

        let first = DataKey::load(&secrets, &workspace_id).unwrap();
        let sealed = first.seal("secret").unwrap();
        let second = DataKey::load(&secrets, &workspace_id).unwrap();

        assert_eq!(second.open(&sealed).unwrap(), "secret");
    }

    /// Each Workspace gets a key of its own, and one Workspace's key
    /// cannot open another's ciphertext.
    #[test]
    fn each_workspace_gets_its_own_key_and_neither_opens_the_others() {
        let secrets = MemorySecretStore::default();
        let a = WorkspaceId::from("ws-a".to_string());
        let b = WorkspaceId::from("ws-b".to_string());

        let a_sealed = DataKey::load(&secrets, &a).unwrap().seal("A's").unwrap();
        let b_sealed = DataKey::load(&secrets, &b).unwrap().seal("B's").unwrap();

        assert!(
            DataKey::load(&secrets, &b)
                .unwrap()
                .open(&a_sealed)
                .is_err(),
            "B's key opened A's credential"
        );
        assert!(
            DataKey::load(&secrets, &a)
                .unwrap()
                .open(&b_sealed)
                .is_err(),
            "A's key opened B's credential"
        );
        assert_eq!(
            DataKey::load(&secrets, &a)
                .unwrap()
                .open(&a_sealed)
                .unwrap(),
            "A's"
        );
        // The two keys are two entries of `secrets.enc`, under names
        // that carry the Workspace.
        assert_ne!(data_key_name(&a), data_key_name(&b));
        assert!(secrets.get(&data_key_name(&a)).unwrap().is_some());
        assert!(secrets.get(&data_key_name(&b)).unwrap().is_some());
    }

    /// The holder mints once per Workspace and answers the same key
    /// after that.
    #[test]
    fn the_key_holder_mints_once_per_workspace() {
        let keys = TenantKeys::new(Arc::new(crate::secrets::MemorySecretStore::default()));
        let a = WorkspaceId::from("ws-a".to_string());
        let b = WorkspaceId::from("ws-b".to_string());

        let sealed = keys.of(&a).unwrap().seal("A's").unwrap();

        assert_eq!(keys.of(&a).unwrap().open(&sealed).unwrap(), "A's");
        assert!(keys.of(&b).unwrap().open(&sealed).is_err());
    }

    fn gmail_of(workspace_id: &WorkspaceId) -> crate::knowledge::SourceKey {
        crate::knowledge::SourceKey {
            workspace_id: workspace_id.clone(),
            connection_id: "google".to_string().into(),
            resource: "gmail".into(),
        }
    }

    /// Two Workspaces derive two suppression keys. A Workspace derives
    /// the same key after a daemon restart, which is a new holder over
    /// the same `secrets.enc`, so a Forget stays in force.
    #[test]
    fn each_workspace_derives_its_own_suppression_key_and_keeps_it_across_a_restart() {
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        let a = WorkspaceId::from("ws-a".to_string());
        let b = WorkspaceId::from("ws-b".to_string());
        let source = gmail_of(&a);
        let keys = TenantKeys::new(Arc::clone(&secrets));

        let identity_of_a = keys.suppression_key(&a).unwrap().identity(&source, "m-1");
        let identity_of_b = keys.suppression_key(&b).unwrap().identity(&source, "m-1");

        assert_ne!(
            identity_of_a, identity_of_b,
            "two Workspaces derived one suppression key"
        );
        let restarted = TenantKeys::new(secrets);
        assert_eq!(
            restarted
                .suppression_key(&a)
                .unwrap()
                .identity(&source, "m-1"),
            identity_of_a,
            "the Workspace derived another suppression key after a restart"
        );
    }

    /// The suppression key is HKDF-SHA256 over the Tenant Data Key with
    /// the fixed label `pagis forget suppression`, and not the Tenant
    /// Data Key itself. A change of the label changes every identity
    /// and ends every Forget, so this test holds it.
    #[test]
    fn the_suppression_key_derives_from_the_tenant_data_key_with_a_fixed_label() {
        let secrets = Arc::new(MemorySecretStore::default());
        let keys = TenantKeys::new(Arc::clone(&secrets) as Arc<dyn SecretStore>);
        let workspace_id = WorkspaceId::from("ws-1".to_string());
        let source = gmail_of(&workspace_id);

        let identity = keys
            .suppression_key(&workspace_id)
            .unwrap()
            .identity(&source, "m-1");

        let data_key = hex_decode(&secrets.get(&data_key_name(&workspace_id)).unwrap().unwrap())
            .expect("the Tenant Data Key is hex");
        let mut derived = [0u8; 32];
        hkdf::Hkdf::<sha2::Sha256>::new(None, &data_key)
            .expand(b"pagis forget suppression", &mut derived)
            .unwrap();
        assert_eq!(
            identity,
            crate::suppression_identity(&derived, &source, "m-1")
        );
        assert_ne!(
            identity,
            crate::suppression_identity(&data_key, &source, "m-1"),
            "the Tenant Data Key itself keys the identity"
        );
    }

    /// A store where another caller minted the key between the read and
    /// the write of this caller: a read finds nothing, and the
    /// create-if-absent answers the key that the other caller stored.
    struct MintedElsewhere(String);

    impl SecretStore for MintedElsewhere {
        fn get(&self, _name: &str) -> Result<Option<String>, SecretError> {
            Ok(None)
        }

        fn set(&self, _name: &str, _value: &str) -> Result<(), SecretError> {
            Ok(())
        }

        fn get_or_insert(&self, _name: &str, _value: &str) -> Result<String, SecretError> {
            Ok(self.0.clone())
        }

        fn delete(&self, _name: &str) -> Result<(), SecretError> {
            Ok(())
        }
    }

    /// The key that a load answers is the key that the store keeps, and
    /// not a key that the load minted and lost to another caller.
    #[test]
    fn a_load_answers_the_key_that_the_store_keeps() {
        let stored = [9u8; 32];
        let sealed = DataKey::from_bytes(stored).seal("secret").unwrap();
        let secrets = MintedElsewhere(hex_encode(&stored));

        let key = DataKey::load(&secrets, &WorkspaceId::from("ws-1".to_string())).unwrap();

        assert_eq!(
            key.open(&sealed),
            Ok("secret".to_string()),
            "the load answered a key that the store does not keep"
        );
    }

    /// A store that counts the loads of Tenant Data Keys. Each load
    /// answers only after a second load arrives or a short time passes,
    /// so two loads that start together overlap every time, whatever the
    /// timing of the threads.
    #[derive(Default)]
    struct Rendezvous {
        inner: MemorySecretStore,
        loads: Mutex<usize>,
        arrived: std::sync::Condvar,
    }

    impl Rendezvous {
        fn load<T>(&self, read: impl FnOnce() -> T) -> T {
            let value = read();
            let mut loads = self.loads.lock().unwrap();
            *loads += 1;
            self.arrived.notify_all();
            let wait = std::time::Duration::from_millis(200);
            drop(
                self.arrived
                    .wait_timeout_while(loads, wait, |loads| *loads < 2)
                    .unwrap(),
            );
            value
        }
    }

    impl SecretStore for Rendezvous {
        fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
            self.load(|| self.inner.get(name))
        }

        fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
            self.inner.set(name, value)
        }

        fn get_or_insert(&self, name: &str, value: &str) -> Result<String, SecretError> {
            self.load(|| self.inner.get_or_insert(name, value))
        }

        fn delete(&self, name: &str) -> Result<(), SecretError> {
            self.inner.delete(name)
        }
    }

    /// First uses of one Workspace's key that race in one holder load the
    /// key once, and every caller seals with the key that the store
    /// keeps.
    #[test]
    fn racing_first_uses_load_a_workspace_key_once() {
        const CALLERS: usize = 4;
        let secrets = Arc::new(Rendezvous::default());
        let keys = TenantKeys::new(Arc::clone(&secrets) as Arc<dyn SecretStore>);
        let workspace_id = WorkspaceId::from("ws-1".to_string());
        let barrier = std::sync::Barrier::new(CALLERS);

        let sealed = std::thread::scope(|scope| {
            let callers = (0..CALLERS)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        keys.of(&workspace_id).unwrap().seal("secret").unwrap()
                    })
                })
                .collect::<Vec<_>>();
            callers
                .into_iter()
                .map(|caller| caller.join().expect("caller"))
                .collect::<Vec<_>>()
        });

        assert_eq!(
            *secrets.loads.lock().unwrap(),
            1,
            "the holder loaded one Workspace's key more than once"
        );
        let kept = DataKey::load(&secrets.inner, &workspace_id).unwrap();
        for sealed in &sealed {
            assert_eq!(kept.open(sealed), Ok("secret".to_string()));
        }
    }
}
