//! Platform secret stores. Every platform keeps its secrets in
//! one encrypted file under the data directory, `secrets.enc`, and the
//! Installation Key seals it (ADR-0013). Only where that key lives
//! differs:
//!
//! - **A server on macOS** keeps it in one keychain item, so the
//!   keychain asks once for one binary and never once per secret.
//! - **A server on Linux** reads it from the Key File that
//!   `secrets.key_file` names. The key never sits in `config.toml`, the
//!   daemon refuses a Key File any group or any other user can read, and
//!   it does not start without one: a store it cannot open is not a
//!   store.
//! - **A local installation** keeps it in the same keychain item on
//!   macOS, and in the desktop keyring through the Secret Service on
//!   Linux. Where that store does not answer, it keeps it in a Key File
//!   it generates in the state directory, and a Key File that is there
//!   wins.
//!
//! The environment variables win at provider-key resolution, so a
//! box that only needs a model key needs no secret file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use pagis_core::{SecretError, SecretStore};
use rand::RngCore;
use serde_json::Value;
use tempfile::NamedTempFile;

use crate::Installation;
use crate::config::Config;

/// The Key File a local installation generates in its state directory
/// when the keychain or the Secret Service does not answer.
pub const GENERATED_KEY_FILE: &str = "installation-key";

/// The keyring item that holds the Installation Key: the macOS
/// keychain item, and the Secret Service item on Linux.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const KEYRING_SERVICE: &str = "pagis";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const KEYRING_ACCOUNT: &str = "safe-storage";

/// The store for this platform and kind of installation: the encrypted
/// file, keyed from where this platform keeps the Installation Key.
pub fn platform_secret_store(
    home: &Path,
    config: &Config,
    installation: Installation,
) -> anyhow::Result<Arc<dyn SecretStore>> {
    let (key, source) = match installation {
        #[cfg(target_os = "macos")]
        Installation::Server => {
            let _ = config;
            (
                server_keychain_key(&desktop_secret_service())?,
                KeySource::Keychain,
            )
        }
        #[cfg(not(target_os = "macos"))]
        Installation::Server => {
            let file = config.secrets.key_file();
            (read_key_file(file)?, KeySource::KeyFile(file.to_path_buf()))
        }
        Installation::Local => local_installation_key(home, &desktop_secret_service())?,
    };
    source.log();
    Ok(Arc::new(EncryptedFileSecretStore::new(
        home.join("secrets.enc"),
        key,
    )))
}

/// Where the daemon found the Installation Key, which it logs at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// The keychain item on macOS.
    Keychain,
    /// The Secret Service item on Linux.
    SecretService,
    /// A Key File: the configured one of a server on Linux, or the one a
    /// local installation generated.
    KeyFile(PathBuf),
}

impl KeySource {
    fn log(&self) {
        match self {
            KeySource::Keychain => tracing::info!(
                "the Installation Key is in the keychain (service `pagis`, account `safe-storage`)"
            ),
            KeySource::SecretService => tracing::info!(
                "the Installation Key is in the Secret Service (service `pagis`, account `safe-storage`)"
            ),
            KeySource::KeyFile(path) => tracing::info!(
                path = %path.display(),
                "the Installation Key is in a Key File"
            ),
        }
    }
}

/// The name of the source in a message, such as "the keychain".
impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySource::Keychain => f.write_str("the keychain"),
            KeySource::SecretService => f.write_str("the Secret Service"),
            KeySource::KeyFile(path) => write!(f, "the Key File {}", path.display()),
        }
    }
}

/// Why the keychain or the Secret Service did not answer with a key.
#[derive(Debug)]
pub enum SecretServiceError {
    /// The store does not answer. On Linux: no session bus, no keyring
    /// daemon, or a keyring that stays locked. On macOS: no keychain the
    /// daemon can use, or a keychain that cannot show its prompt. A local
    /// installation uses a Key File.
    Absent(String),
    /// The store answered and the call failed, for example a keychain
    /// prompt that the person denied. The daemon does not start, because
    /// a second key would orphan the first.
    Failed(String),
}

/// The keyring item of the Installation Key: the keychain on macOS, or
/// the desktop keyring through the Secret Service on Linux. The item
/// holds 64 hexadecimal characters. The seam lets a test stand in for a
/// desktop session.
pub trait SecretService {
    /// Where the key is when this store holds it. The start log and the
    /// errors of the key order name it.
    fn source(&self) -> KeySource;
    /// The key the item holds, or `None` when the store has no item.
    fn read(&self) -> Result<Option<String>, SecretServiceError>;
    /// Store the key in the item.
    fn write(&self, hex: &str) -> Result<(), SecretServiceError>;
}

/// The Installation Key of a local installation.
///
/// The order is what keeps one key for the life of the data:
///
/// 1. A Key File that an earlier start generated wins, so a start where
///    the keyring answers never replaces the key that sealed the file.
/// 2. The keyring item (the keychain on macOS, the Secret Service on
///    Linux), which the first start makes when the keyring answers.
/// 3. Where the keyring does not answer, a new Key File, mode 600, in
///    the state directory.
///
/// A `secrets.enc` that exists while neither holds its key is refused:
/// a new key would seal new secrets and orphan every old one without a
/// word.
pub fn local_installation_key(
    home: &Path,
    service: &dyn SecretService,
) -> anyhow::Result<([u8; 32], KeySource)> {
    let key_file = home.join(GENERATED_KEY_FILE);
    if key_file.exists() {
        return Ok((read_key_file(&key_file)?, KeySource::KeyFile(key_file)));
    }
    let keyring = service.source();
    let sealed = home.join("secrets.enc").exists();
    let absent = match service.read() {
        Ok(Some(hex)) => {
            let key = key_from_hex(&hex).map_err(|error| {
                anyhow::anyhow!("the Installation Key in {keyring} is invalid: {error}")
            })?;
            return Ok((key, keyring));
        }
        Ok(None) if sealed => anyhow::bail!(
            "{} exists, and neither {keyring} nor {} holds the Installation Key \
             that sealed it. Restore the key, or remove secrets.enc to start with no \
             stored secrets",
            home.join("secrets.enc").display(),
            key_file.display()
        ),
        Ok(None) => {
            let key = generate_key();
            match service.write(&key_to_hex(&key)) {
                Ok(()) => return Ok((key, keyring)),
                Err(SecretServiceError::Absent(reason)) => reason,
                Err(SecretServiceError::Failed(reason)) => {
                    anyhow::bail!("{keyring} could not store the Installation Key: {reason}")
                }
            }
        }
        Err(SecretServiceError::Absent(reason)) if sealed => anyhow::bail!(
            "{} is sealed with the Installation Key in {keyring}, which does not \
             answer: {reason}. Start Pagis in a desktop session with the keyring unlocked",
            home.join("secrets.enc").display()
        ),
        Err(SecretServiceError::Absent(reason)) => reason,
        Err(SecretServiceError::Failed(reason)) => {
            anyhow::bail!("{keyring} could not read the Installation Key: {reason}")
        }
    };
    tracing::info!(reason = %absent, "{keyring} does not answer; generating a Key File");
    let key = generate_key();
    write_key_file(&key_file, &key)?;
    Ok((key, KeySource::KeyFile(key_file)))
}

/// The Installation Key of a server on macOS: the keychain item, which
/// the first start makes. A server has no Key File to fall back to, so
/// a keychain that does not answer stops the start.
#[cfg(target_os = "macos")]
fn server_keychain_key(keychain: &dyn SecretService) -> anyhow::Result<[u8; 32]> {
    match keychain.read() {
        Ok(Some(hex)) => key_from_hex(&hex).map_err(|error| {
            anyhow::anyhow!("the Installation Key in the keychain is invalid: {error}")
        }),
        Ok(None) => {
            let key = generate_key();
            match keychain.write(&key_to_hex(&key)) {
                Ok(()) => Ok(key),
                Err(SecretServiceError::Absent(reason) | SecretServiceError::Failed(reason)) => {
                    anyhow::bail!("the keychain could not store the Installation Key: {reason}")
                }
            }
        }
        Err(SecretServiceError::Absent(reason) | SecretServiceError::Failed(reason)) => {
            anyhow::bail!("the keychain could not read the Installation Key: {reason}")
        }
    }
}

/// Write a new Key File with mode 600. It never replaces a file, so two
/// starts cannot race to two keys.
#[cfg(unix)]
fn write_key_file(path: &Path, key: &[u8; 32]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            anyhow::anyhow!("cannot create the Key File {}: {error}", path.display())
        })?;
    file.write_all(key_to_hex(key).as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// The keyring item of this platform, through the `keyring` crate: the
/// item in the default keychain of the user on macOS, and on Linux the
/// Secret Service item, which the crate reaches over D-Bus in whatever
/// keyring daemon the session runs (GNOME Keyring, KWallet, KeePassXC).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn desktop_secret_service() -> impl SecretService {
    struct Desktop;

    fn entry() -> Result<keyring::Entry, SecretServiceError> {
        keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(keyring_error)
    }

    impl SecretService for Desktop {
        fn source(&self) -> KeySource {
            if cfg!(target_os = "macos") {
                KeySource::Keychain
            } else {
                KeySource::SecretService
            }
        }

        fn read(&self) -> Result<Option<String>, SecretServiceError> {
            match entry()?.get_password() {
                Ok(hex) => Ok(Some(hex)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(keyring_error(error)),
            }
        }

        fn write(&self, hex: &str) -> Result<(), SecretServiceError> {
            entry()?.set_password(hex).map_err(keyring_error)
        }
    }

    Desktop
}

/// What a Secret Service error means on Linux: a service that cannot be
/// reached, and a platform failure on the way to it, are an absent
/// service.
#[cfg(target_os = "linux")]
fn keyring_error(error: keyring::Error) -> SecretServiceError {
    match error {
        keyring::Error::NoStorageAccess(inner) | keyring::Error::PlatformFailure(inner) => {
            SecretServiceError::Absent(inner.to_string())
        }
        other => SecretServiceError::Failed(other.to_string()),
    }
}

/// The OSStatus `errSecInteractionNotAllowed`: the keychain cannot show
/// its prompt, for example to a start over SSH.
#[cfg(target_os = "macos")]
const INTERACTION_NOT_ALLOWED: i32 = -25308;

/// What a keychain error means on macOS: an absent keychain, where a
/// local installation uses a Key File, or a failure, which stops the
/// start.
#[cfg(target_os = "macos")]
fn keyring_error(error: keyring::Error) -> SecretServiceError {
    let reason = error.to_string();
    if keychain_is_absent(&error) {
        SecretServiceError::Absent(reason)
    } else {
        SecretServiceError::Failed(reason)
    }
}

/// True when a keychain error means that the keychain cannot answer.
///
/// `keyring` reports no keychain, a read-only one and an invalid one as
/// `NoStorageAccess`, and a keychain that cannot show its prompt as a
/// platform failure with `errSecInteractionNotAllowed`. Every other
/// error is a failure. This includes a prompt that the person denies or
/// cancels (`userCanceled`, `errSecAuthFailed`): a denial does not make
/// a second key.
///
/// The check reads only the OSStatus. The message of a Security
/// framework error is a slow bundle lookup, so it stays out of here.
#[cfg(target_os = "macos")]
fn keychain_is_absent(error: &keyring::Error) -> bool {
    match error {
        keyring::Error::NoStorageAccess(_) => true,
        keyring::Error::PlatformFailure(inner) => inner
            .downcast_ref::<security_framework::base::Error>()
            .is_some_and(|status| status.code() == INTERACTION_NOT_ALLOWED),
        _ => false,
    }
}

/// A platform with no Secret Service support in this build.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn desktop_secret_service() -> impl SecretService {
    struct Absent;
    impl SecretService for Absent {
        fn source(&self) -> KeySource {
            KeySource::SecretService
        }
        fn read(&self) -> Result<Option<String>, SecretServiceError> {
            Err(SecretServiceError::Absent(
                "this platform has no Secret Service".into(),
            ))
        }
        fn write(&self, _hex: &str) -> Result<(), SecretServiceError> {
            Err(SecretServiceError::Absent(
                "this platform has no Secret Service".into(),
            ))
        }
    }
    Absent
}

/// The Installation Key from a Key File.
///
/// The file holds the 64 hexadecimal characters of the key and nothing
/// else.
#[cfg(unix)]
pub fn read_key_file(path: &Path) -> anyhow::Result<[u8; 32]> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path).map_err(|error| {
        anyhow::anyhow!(
            "cannot read the secret-file key at {}: {error}. Write 64 hexadecimal \
             characters to it, give it mode 600, and name it as `key_file` in the \
             [secrets] table of config.toml",
            path.display()
        )
    })?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        anyhow::bail!(
            "the secret-file key at {} is mode {mode:o}: a group or another user can \
             read the key that opens every stored secret. Give it mode 600",
            path.display()
        );
    }
    key_from_hex(&std::fs::read_to_string(path)?)
        .map_err(|error| anyhow::anyhow!("the secret-file key at {}: {error}", path.display()))
}

/// A 32-byte key written as 64 hex characters.
pub fn key_to_hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

/// The 32-byte key behind 64 hex characters.
pub fn key_from_hex(hex: &str) -> anyhow::Result<[u8; 32]> {
    let hex = hex.trim().as_bytes();
    if hex.len() != 64 {
        anyhow::bail!("a secret key must be 64 hex characters");
    }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk)?;
        bytes[i] = u8::from_str_radix(s, 16)?;
    }
    Ok(bytes)
}

/// A new random key.
pub fn generate_key() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

/// One JSON object of secrets, sealed with XChaCha20-Poly1305.
///
/// Each change writes the whole file again and replaces it whole or not
/// at all. The sealed bytes go to a temporary file in the same
/// directory, the file is synced to the disk and renamed over
/// `secrets.enc`, and then the directory is synced. A stop at any point
/// leaves the old file or the new one, and a read during a write opens
/// one of the two.
pub struct EncryptedFileSecretStore {
    path: PathBuf,
    key: [u8; 32],
    // Serialize read-modify-write cycles within this process. A read
    // takes no lock, because the rename replaces the file whole.
    write_lock: Mutex<()>,
}

impl EncryptedFileSecretStore {
    pub fn new(path: PathBuf, key: [u8; 32]) -> Self {
        Self {
            path,
            key,
            write_lock: Mutex::new(()),
        }
    }

    fn read_all(&self) -> Result<serde_json::Map<String, Value>, SecretError> {
        if !self.path.exists() {
            return Ok(serde_json::Map::new());
        }
        let sealed = std::fs::read(&self.path).map_err(|err| SecretError(err.to_string()))?;
        if sealed.len() < 24 {
            return Err(SecretError("secret file is truncated".to_string()));
        }
        let (nonce, ciphertext) = sealed.split_at(24);
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.key));
        let plaintext = cipher
            .decrypt(XNonce::from_slice(nonce), ciphertext)
            .map_err(|_| SecretError("cannot decrypt the secret file".to_string()))?;
        serde_json::from_slice(&plaintext).map_err(|err| SecretError(err.to_string()))
    }

    fn write_all(&self, values: &serde_json::Map<String, Value>) -> Result<(), SecretError> {
        let staged = self.stage(values)?;
        self.commit(staged)
    }

    /// Write the sealed values to a new temporary file in the directory
    /// of `secrets.enc`, and sync the file to the disk. The file has mode
    /// 600 from its creation, so no other user can open it at any time.
    /// A failure removes the file.
    fn stage(&self, values: &serde_json::Map<String, Value>) -> Result<NamedTempFile, SecretError> {
        use std::io::Write;

        let sealed = self.seal(values)?;
        let directory = self.directory();
        let failed = |err: std::io::Error| {
            SecretError(format!(
                "cannot write a new secret file in {}: {err}",
                directory.display()
            ))
        };
        let mut staged = NamedTempFile::new_in(directory).map_err(failed)?;
        staged.write_all(&sealed).map_err(failed)?;
        staged.as_file().sync_all().map_err(failed)?;
        Ok(staged)
    }

    /// Rename the staged file over `secrets.enc`, and then sync the
    /// directory, so that the rename stays after a power loss.
    fn commit(&self, staged: NamedTempFile) -> Result<(), SecretError> {
        staged.persist(&self.path).map_err(|err| {
            SecretError(format!(
                "cannot replace {}: {}",
                self.path.display(),
                err.error
            ))
        })?;
        // A rename is on the disk only when the directory entry is.
        #[cfg(unix)]
        std::fs::File::open(self.directory())
            .and_then(|directory| directory.sync_all())
            .map_err(|err| {
                SecretError(format!(
                    "cannot sync the directory {}: {err}",
                    self.directory().display()
                ))
            })?;
        Ok(())
    }

    /// The directory that holds `secrets.enc`.
    fn directory(&self) -> &Path {
        match self.path.parent() {
            Some(directory) if !directory.as_os_str().is_empty() => directory,
            _ => Path::new("."),
        }
    }

    /// The values as the file holds them: a random nonce, then the
    /// sealed JSON.
    fn seal(&self, values: &serde_json::Map<String, Value>) -> Result<Vec<u8>, SecretError> {
        let plaintext = serde_json::to_vec(values).map_err(|err| SecretError(err.to_string()))?;
        let mut nonce = [0u8; 24];
        rand::rng().fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.key));
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext.as_slice())
            .map_err(|_| SecretError("cannot encrypt the secret file".to_string()))?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }
}

impl SecretStore for EncryptedFileSecretStore {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
        Ok(self
            .read_all()?
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        let _guard = self.write_lock.lock().expect("secret write lock");
        let mut values = self.read_all()?;
        values.insert(name.to_string(), Value::String(value.to_string()));
        self.write_all(&values)
    }

    fn get_or_insert(&self, name: &str, value: &str) -> Result<String, SecretError> {
        let _guard = self.write_lock.lock().expect("secret write lock");
        let mut values = self.read_all()?;
        if let Some(stored) = values.get(name).and_then(Value::as_str) {
            return Ok(stored.to_string());
        }
        values.insert(name.to_string(), Value::String(value.to_string()));
        self.write_all(&values)?;
        Ok(value.to_string())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        let _guard = self.write_lock.lock().expect("secret write lock");
        let mut values = self.read_all()?;
        values.remove(name);
        self.write_all(&values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> EncryptedFileSecretStore {
        EncryptedFileSecretStore::new(dir.join("secrets.enc"), [7u8; 32])
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(store(dir.path()).get("anthropic_api_key").unwrap(), None);
    }

    #[test]
    fn secrets_round_trip_and_stay_encrypted_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = store(dir.path());

        secrets.set("anthropic_api_key", "sk-secret").unwrap();
        secrets.set("openai_api_key", "sk-other").unwrap();

        assert_eq!(
            secrets.get("anthropic_api_key").unwrap(),
            Some("sk-secret".to_string())
        );
        assert_eq!(
            secrets.get("openai_api_key").unwrap(),
            Some("sk-other".to_string())
        );
        let raw = std::fs::read(dir.path().join("secrets.enc")).unwrap();
        assert!(!raw.windows(9).any(|w| w == b"sk-secret"));
    }

    /// Start `callers` threads together, and answer what each call
    /// answered, in the order of the callers.
    fn race<T: Send>(callers: usize, call: impl Fn(usize) -> T + Sync) -> Vec<T> {
        let barrier = std::sync::Barrier::new(callers);
        std::thread::scope(|scope| {
            let threads = (0..callers)
                .map(|caller| {
                    let (barrier, call) = (&barrier, &call);
                    scope.spawn(move || {
                        barrier.wait();
                        call(caller)
                    })
                })
                .collect::<Vec<_>>();
            threads
                .into_iter()
                .map(|thread| thread.join().expect("caller"))
                .collect()
        })
    }

    /// The file holds every provider key, sealed. Each write leaves one
    /// file, `secrets.enc`, that only the owner can read, and no
    /// temporary file. This is true also where the old file had a wider
    /// mode, because the rename puts a new file in its place.
    #[cfg(unix)]
    #[test]
    fn each_write_leaves_one_secret_file_that_only_the_owner_reads() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.enc");
        let secrets = store(dir.path());
        let mode = || std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let files = || {
            std::fs::read_dir(dir.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect::<Vec<_>>()
        };

        secrets.set("anthropic_api_key", "sk-secret").unwrap();
        assert_eq!(mode(), 0o600, "a new secret file");
        assert_eq!(files(), ["secrets.enc"]);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        secrets.set("openai_api_key", "sk-other").unwrap();
        assert_eq!(mode(), 0o600, "a secret file written over one of mode 644");
        assert_eq!(files(), ["secrets.enc"]);
        assert_eq!(
            store(dir.path())
                .get("anthropic_api_key")
                .unwrap()
                .as_deref(),
            Some("sk-secret")
        );
    }

    /// A write that stops after the new values are in the temporary file
    /// and before the rename leaves `secrets.enc` whole, with the values
    /// it held before the write. The next write replaces the file.
    #[test]
    fn a_write_that_stops_before_the_rename_keeps_the_previous_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = store(dir.path());
        secrets.set("anthropic_api_key", "sk-before").unwrap();
        secrets.set("openai_api_key", "sk-other").unwrap();

        let mut next = serde_json::Map::new();
        next.insert("anthropic_api_key".into(), "sk-after".into());
        let staged = secrets.stage(&next).unwrap();
        // The process stops here. A stop runs no destructor, so nothing
        // renames the staged file or removes it.
        std::mem::forget(staged);

        let reopened = store(dir.path());
        assert_eq!(
            reopened.get("anthropic_api_key").unwrap().as_deref(),
            Some("sk-before")
        );
        assert_eq!(
            reopened.get("openai_api_key").unwrap().as_deref(),
            Some("sk-other")
        );
        reopened.set("anthropic_api_key", "sk-later").unwrap();
        let restarted = store(dir.path());
        assert_eq!(
            restarted.get("anthropic_api_key").unwrap().as_deref(),
            Some("sk-later")
        );
        assert_eq!(
            restarted.get("openai_api_key").unwrap().as_deref(),
            Some("sk-other")
        );
    }

    /// A read that runs while a write replaces the file answers the value
    /// from before the write or the value from after it, and never an
    /// error.
    #[test]
    fn a_read_during_a_write_answers_the_old_or_the_new_value() {
        const READERS: usize = 4;
        let dir = tempfile::tempdir().unwrap();
        let secrets = store(dir.path());
        let value = |round: usize| format!("sk-{round}");
        secrets.set("anthropic_api_key", &value(0)).unwrap();

        for round in 1..=200 {
            let (old, new) = (value(round - 1), value(round));
            let written = std::sync::atomic::AtomicBool::new(false);
            let reads = race(READERS + 1, |caller| {
                if caller == READERS {
                    secrets.set("anthropic_api_key", &new).unwrap();
                    written.store(true, std::sync::atomic::Ordering::SeqCst);
                    return Ok(());
                }
                loop {
                    let done = written.load(std::sync::atomic::Ordering::SeqCst);
                    match secrets.get("anthropic_api_key") {
                        Ok(Some(read)) if read == old || read == new => {}
                        Ok(other) => return Err(format!("{other:?}, which was never written")),
                        Err(error) => return Err(error.to_string()),
                    }
                    if done {
                        return Ok(());
                    }
                }
            });
            for read in reads {
                assert_eq!(read, Ok(()), "round {round}: a read during a write failed");
            }
        }
    }

    /// Callers that race to create one name with different values all
    /// get the one value that `secrets.enc` keeps.
    #[test]
    fn callers_that_race_to_create_one_name_all_get_the_stored_value() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = store(dir.path());

        for round in 0..20 {
            let name = format!("workspace/ws-{round}/vault_data_key");
            let answers = race(8, |caller| {
                secrets
                    .get_or_insert(&name, &format!("value-{caller}"))
                    .unwrap()
            });

            let stored = store(dir.path())
                .get(&name)
                .unwrap()
                .expect("the name holds a value");
            assert!(
                answers.iter().all(|answer| *answer == stored),
                "round {round}: the callers got {answers:?}, and secrets.enc holds {stored}"
            );
        }
    }

    #[test]
    fn a_key_makes_a_round_trip_through_hex() {
        let key = generate_key();
        assert_eq!(key_from_hex(&key_to_hex(&key)).unwrap(), key);
        assert!(key_from_hex("abc").is_err());
        assert!(key_from_hex(&"zz".repeat(32)).is_err());
    }

    /// The Key File seam, which a server on Linux and a local
    /// installation read at start.
    #[cfg(unix)]
    mod key_file {
        use std::os::unix::fs::PermissionsExt;

        use super::*;

        fn write_key(dir: &Path, contents: &str, mode: u32) -> PathBuf {
            let path = dir.join("pagis-secrets-key");
            std::fs::write(&path, contents).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            path
        }

        #[test]
        fn an_owner_only_key_file_reads_back_as_the_key() {
            let dir = tempfile::tempdir().unwrap();
            let key = generate_key();
            // A trailing newline is what `echo` and a Docker secret
            // leave behind, and it is not part of the key.
            let path = write_key(dir.path(), &format!("{}\n", key_to_hex(&key)), 0o600);

            assert_eq!(read_key_file(&path).unwrap(), key);
            // Mode 400 is what a read-only mount gives, and it is fine.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
            assert_eq!(read_key_file(&path).unwrap(), key);
        }

        #[test]
        fn a_key_file_a_group_or_another_user_can_read_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let hex = key_to_hex(&generate_key());

            for mode in [0o640, 0o604, 0o644, 0o660, 0o666] {
                let path = write_key(dir.path(), &hex, mode);
                let error = read_key_file(&path)
                    .expect_err("a key another account can read is refused")
                    .to_string();
                assert!(
                    error.contains("mode"),
                    "the refusal of mode {mode:o} says nothing about the mode: {error}"
                );
            }
        }

        #[test]
        fn a_missing_key_file_is_refused_and_says_what_to_write() {
            let dir = tempfile::tempdir().unwrap();

            let error = read_key_file(&dir.path().join("absent"))
                .expect_err("a daemon with no key file does not start")
                .to_string();

            assert!(error.contains("key_file"), "{error}");
            assert!(error.contains("600"), "{error}");
        }

        #[test]
        fn a_key_file_that_is_not_a_key_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let path = write_key(dir.path(), "not a key", 0o600);

            assert!(read_key_file(&path).is_err());
        }
    }

    /// The Installation Key of a local installation. A fake stands in
    /// for the keychain or the Secret Service, so no test needs a desktop
    /// session.
    #[cfg(unix)]
    mod local_key {
        use std::cell::RefCell;
        use std::os::unix::fs::PermissionsExt;

        use super::*;

        /// A Secret Service that answers from memory, or one that does
        /// not answer at all.
        pub(super) enum Fake {
            Answers(RefCell<Option<String>>),
            Absent,
            Fails,
        }

        impl SecretService for Fake {
            fn source(&self) -> KeySource {
                KeySource::SecretService
            }

            fn read(&self) -> Result<Option<String>, SecretServiceError> {
                match self {
                    Fake::Answers(item) => Ok(item.borrow().clone()),
                    Fake::Absent => Err(SecretServiceError::Absent(
                        "org.freedesktop.secrets was not provided by any service".into(),
                    )),
                    Fake::Fails => Err(SecretServiceError::Failed("bad item".into())),
                }
            }

            fn write(&self, hex: &str) -> Result<(), SecretServiceError> {
                match self {
                    Fake::Answers(item) => {
                        *item.borrow_mut() = Some(hex.to_string());
                        Ok(())
                    }
                    Fake::Absent => Err(SecretServiceError::Absent("no session bus".into())),
                    Fake::Fails => Err(SecretServiceError::Failed("bad item".into())),
                }
            }
        }

        pub(super) fn answers() -> Fake {
            Fake::Answers(RefCell::new(None))
        }

        #[test]
        fn the_first_start_keeps_a_new_key_in_the_secret_service() {
            let home = tempfile::tempdir().unwrap();
            let service = answers();

            let (key, source) = local_installation_key(home.path(), &service).unwrap();

            assert_eq!(source, KeySource::SecretService);
            let Fake::Answers(item) = &service else {
                unreachable!()
            };
            assert_eq!(item.borrow().as_deref(), Some(key_to_hex(&key).as_str()));
            assert!(!home.path().join(GENERATED_KEY_FILE).exists());

            let (again, _) = local_installation_key(home.path(), &service).unwrap();
            assert_eq!(again, key, "a second start reads the same key back");
        }

        #[test]
        fn with_no_secret_service_a_mode_600_key_file_is_generated_and_read_again() {
            let home = tempfile::tempdir().unwrap();

            let (key, source) = local_installation_key(home.path(), &Fake::Absent).unwrap();

            let file = home.path().join(GENERATED_KEY_FILE);
            assert_eq!(source, KeySource::KeyFile(file.clone()));
            let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            assert_eq!(read_key_file(&file).unwrap(), key);

            // A secret sealed with it opens on the next start.
            EncryptedFileSecretStore::new(home.path().join("secrets.enc"), key)
                .set("anthropic_api_key", "sk-secret")
                .unwrap();
            let (again, _) = local_installation_key(home.path(), &Fake::Absent).unwrap();
            assert_eq!(
                EncryptedFileSecretStore::new(home.path().join("secrets.enc"), again)
                    .get("anthropic_api_key")
                    .unwrap()
                    .as_deref(),
                Some("sk-secret")
            );
        }

        /// A Secret Service that starts to answer later, when the person
        /// installs a keyring, does not replace the key that sealed the
        /// file.
        #[test]
        fn a_generated_key_file_wins_over_a_secret_service_that_answers_later() {
            let home = tempfile::tempdir().unwrap();
            let (key, _) = local_installation_key(home.path(), &Fake::Absent).unwrap();

            let service = answers();
            let (again, source) = local_installation_key(home.path(), &service).unwrap();

            assert_eq!(again, key);
            assert!(matches!(source, KeySource::KeyFile(_)));
            let Fake::Answers(item) = &service else {
                unreachable!()
            };
            assert!(
                item.borrow().is_none(),
                "the Secret Service gets no second key"
            );
        }

        #[test]
        fn a_secret_service_that_refuses_the_write_falls_back_to_a_key_file() {
            struct ReadsNothingWritesNothing;
            impl SecretService for ReadsNothingWritesNothing {
                fn source(&self) -> KeySource {
                    KeySource::SecretService
                }
                fn read(&self) -> Result<Option<String>, SecretServiceError> {
                    Ok(None)
                }
                fn write(&self, _hex: &str) -> Result<(), SecretServiceError> {
                    Err(SecretServiceError::Absent("the keyring is locked".into()))
                }
            }
            let home = tempfile::tempdir().unwrap();

            let (_, source) =
                local_installation_key(home.path(), &ReadsNothingWritesNothing).unwrap();

            assert_eq!(
                source,
                KeySource::KeyFile(home.path().join(GENERATED_KEY_FILE))
            );
        }

        /// Sealed secrets whose key is nowhere are refused, and no new key
        /// is made to replace it.
        #[test]
        fn sealed_secrets_with_no_key_are_refused_and_no_key_is_made() {
            for service in [Fake::Absent, answers()] {
                let home = tempfile::tempdir().unwrap();
                std::fs::write(home.path().join("secrets.enc"), b"sealed").unwrap();

                let error = local_installation_key(home.path(), &service)
                    .expect_err("an orphaned secrets.enc is refused")
                    .to_string();

                assert!(error.contains("secrets.enc"), "{error}");
                assert!(!home.path().join(GENERATED_KEY_FILE).exists());
                if let Fake::Answers(item) = &service {
                    assert!(item.borrow().is_none());
                }
            }
        }

        #[test]
        fn a_secret_service_that_fails_stops_the_start() {
            let home = tempfile::tempdir().unwrap();

            let error = local_installation_key(home.path(), &Fake::Fails)
                .expect_err("a failing Secret Service is not a missing one")
                .to_string();

            assert!(error.contains("bad item"), "{error}");
            assert!(!home.path().join(GENERATED_KEY_FILE).exists());
        }

        /// A keychain prompt that the person denies is a failure and not
        /// an absent keychain: the start stops, the error names the
        /// keychain, and no Key File is made.
        #[test]
        fn a_denied_keychain_prompt_stops_the_start_and_makes_no_key_file() {
            struct Denied;
            impl SecretService for Denied {
                fn source(&self) -> KeySource {
                    KeySource::Keychain
                }
                fn read(&self) -> Result<Option<String>, SecretServiceError> {
                    Err(SecretServiceError::Failed(
                        "User canceled the operation.".into(),
                    ))
                }
                fn write(&self, _hex: &str) -> Result<(), SecretServiceError> {
                    unreachable!("a failed read makes no key")
                }
            }
            let home = tempfile::tempdir().unwrap();

            let error = local_installation_key(home.path(), &Denied)
                .expect_err("a denied prompt stops the start")
                .to_string();

            assert!(
                error.starts_with("the keychain could not read the Installation Key"),
                "{error}"
            );
            assert!(!home.path().join(GENERATED_KEY_FILE).exists());
        }

        #[test]
        fn an_invalid_key_in_the_secret_service_is_refused() {
            let home = tempfile::tempdir().unwrap();
            let service = Fake::Answers(RefCell::new(Some("not a key".into())));

            assert!(local_installation_key(home.path(), &service).is_err());
        }

        #[test]
        fn a_generated_key_file_others_can_read_is_refused() {
            let home = tempfile::tempdir().unwrap();
            local_installation_key(home.path(), &Fake::Absent).unwrap();
            let file = home.path().join(GENERATED_KEY_FILE);
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

            let error = local_installation_key(home.path(), &Fake::Absent)
                .unwrap_err()
                .to_string();
            assert!(error.contains("mode"), "{error}");
        }
    }

    /// What a keychain error means on macOS. A case that starts from an
    /// OSStatus goes through the error map of the `keyring` crate, and no
    /// test needs a keychain.
    #[cfg(target_os = "macos")]
    mod keychain_errors {
        use super::*;

        fn absent(status: i32) -> bool {
            keychain_is_absent(&keyring::macos::decode_error(status.into()))
        }

        /// `userCanceled` and `errSecAuthFailed`: the person denied or
        /// cancelled the prompt.
        #[test]
        fn a_denied_or_cancelled_prompt_is_a_failure() {
            for status in [-128, -25293] {
                assert!(!absent(status), "OSStatus {status} gives a Key File");
            }
        }

        /// `errSecInteractionNotAllowed`, and the codes that `keyring`
        /// reports as no storage access: `errSecNotAvailable`,
        /// `errSecReadOnly`, `errSecNoSuchKeychain` and
        /// `errSecInvalidKeychain`.
        #[test]
        fn a_keychain_that_cannot_answer_is_absent() {
            for status in [-25308, -25291, -25292, -25294, -25295] {
                assert!(absent(status), "OSStatus {status} stops the start");
            }
        }

        /// The map keeps the variant that the check gives, with the
        /// error as the reason. A platform error with no OSStatus is a
        /// failure.
        #[test]
        fn the_map_follows_the_check() {
            let no_access = keyring::Error::NoStorageAccess("no keychain".into());
            let SecretServiceError::Absent(reason) = keyring_error(no_access) else {
                panic!("no storage access is an absent keychain");
            };
            assert!(reason.contains("no keychain"), "{reason}");

            let other = keyring::Error::PlatformFailure("not an OSStatus".into());
            assert!(matches!(
                keyring_error(other),
                SecretServiceError::Failed(reason) if reason.contains("not an OSStatus")
            ));
        }
    }

    /// The Installation Key of a server on macOS, with a fake keychain.
    #[cfg(target_os = "macos")]
    mod server_keychain {
        use super::local_key::{Fake, answers};
        use super::*;

        #[test]
        fn the_first_start_makes_the_item_and_the_next_start_reads_it() {
            let keychain = answers();

            let key = server_keychain_key(&keychain).unwrap();

            assert_eq!(server_keychain_key(&keychain).unwrap(), key);
        }

        /// A server has no Key File to fall back to.
        #[test]
        fn a_keychain_that_does_not_answer_or_fails_stops_the_start() {
            for keychain in [Fake::Absent, Fake::Fails] {
                assert!(server_keychain_key(&keychain).is_err());
            }
        }
    }

    #[test]
    fn a_wrong_key_cannot_decrypt() {
        let dir = tempfile::tempdir().unwrap();
        store(dir.path()).set("anthropic_api_key", "sk").unwrap();

        let wrong = EncryptedFileSecretStore::new(dir.path().join("secrets.enc"), [8u8; 32]);
        assert!(wrong.get("anthropic_api_key").is_err());
    }
}
