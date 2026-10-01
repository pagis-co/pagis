//! Provider API keys. One resolver decides where a key
//! comes from: environment first, then the config file, then the
//! platform secret store. The wizard writes to the secret store; the
//! environment and the config file stay read-only overrides.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::sync::Mutex;

use crate::WorkspaceId;

/// The name one Workspace's secret is filed under.
///
/// Most secret names are built from a string the person chose: a
/// Connection alias, a mailbox address. Those strings are unique inside
/// a Workspace and nowhere else, so two people who both call their
/// carrier Connection `telnyx` would otherwise overwrite each other's
/// carrier key. The Workspace leads the name, so they cannot.
///
/// An installation-level secret is not named through this. The provider
/// API keys ([`Provider::secret_name`]) are the installation's own: one
/// Org shares them, and a Workspace does not hold one.
pub fn workspace_secret_name(workspace_id: &WorkspaceId, name: &str) -> String {
    format!("workspace/{workspace_id}/{name}")
}

/// A model provider the daemon can hold a key for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Anthropic,
    OpenAi,
    OpenRouter,
}

/// Every known provider, in display order.
pub const PROVIDERS: [Provider; 3] = [Provider::Anthropic, Provider::OpenAi, Provider::OpenRouter];

impl Provider {
    /// The stable API identifier (`anthropic`, `openai`, or `openrouter`).
    pub fn id(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
            Provider::OpenRouter => "openrouter",
        }
    }

    /// The environment variable that overrides the stored key.
    pub fn env_var(self) -> &'static str {
        match self {
            Provider::Anthropic => "ANTHROPIC_API_KEY",
            Provider::OpenAi => "OPENAI_API_KEY",
            Provider::OpenRouter => "OPENROUTER_API_KEY",
        }
    }

    /// The name the secret store files the key under. A provider key
    /// belongs to the installation and not to a Workspace, so the name
    /// carries no tenant: one Org shares one key per provider,
    /// and the environment and the config file override it the same way
    /// for everybody.
    pub fn secret_name(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic_api_key",
            Provider::OpenAi => "openai_api_key",
            Provider::OpenRouter => "openrouter_api_key",
        }
    }

    pub fn from_id(id: &str) -> Option<Provider> {
        PROVIDERS.into_iter().find(|p| p.id() == id)
    }

    /// What Pagis does with this provider's key. A provider serves a
    /// Model Alias only for a use it lists here.
    pub fn uses(self) -> &'static [ProviderUse] {
        match self {
            Provider::Anthropic => &[ProviderUse::Thinking],
            // OpenRouter transcribes a held clip and has no realtime
            // socket, so dictation is transcribed on release.
            Provider::OpenRouter => &[
                ProviderUse::Thinking,
                ProviderUse::SpokenReplies,
                ProviderUse::Dictation,
            ],
            Provider::OpenAi => &[
                ProviderUse::Thinking,
                ProviderUse::SpokenReplies,
                ProviderUse::Dictation,
                ProviderUse::Calls,
            ],
        }
    }

    pub fn serves(self, provider_use: ProviderUse) -> bool {
        self.uses().contains(&provider_use)
    }
}

/// One thing a provider's key does in Pagis, as the Person reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderUse {
    /// The Runs of every Agent: the `default` alias and the others the
    /// Person adds.
    Thinking,
    /// The Agent Voice of spoken replies: the `speak` alias.
    SpokenReplies,
    /// Speech to text: the `transcribe` alias.
    Dictation,
    /// Live telephone conversations: the `phone` aliases.
    Calls,
}

impl ProviderUse {
    pub fn id(self) -> &'static str {
        match self {
            ProviderUse::Thinking => "thinking",
            ProviderUse::SpokenReplies => "spoken_replies",
            ProviderUse::Dictation => "dictation",
            ProviderUse::Calls => "calls",
        }
    }
}

/// Where a resolved key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Env,
    Config,
    /// `secrets.enc`, which the Installation Key seals. The key is not
    /// in the file (ADR-0013).
    SecretFile,
}

impl KeySource {
    pub fn as_str(self) -> &'static str {
        match self {
            KeySource::Env => "env",
            KeySource::Config => "config",
            KeySource::SecretFile => "secret_file",
        }
    }

    /// Where an Administrator finds the key of `provider`, in words.
    pub fn label(self, provider: Provider) -> String {
        match self {
            KeySource::Env => format!("the {} environment variable", provider.env_var()),
            KeySource::Config => "config.toml".to_string(),
            KeySource::SecretFile => "secrets.enc".to_string(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("secret store error: {0}")]
pub struct SecretError(pub String);

/// The secret seam: `secrets.enc`, which the Installation Key seals, on
/// every platform, and memory in tests.
pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, name: &str, value: &str) -> Result<(), SecretError>;
    /// Store `value` under `name` only when the name holds no value, and
    /// answer the value that the store holds after the call: the value
    /// that was there, or `value`. The read and the write are one step,
    /// so callers that race with different values all get the one value
    /// that the store keeps. A Tenant Data Key is minted through this.
    fn get_or_insert(&self, name: &str, value: &str) -> Result<String, SecretError>;
    /// Forget one secret. A name that is not stored is not an error.
    fn delete(&self, name: &str) -> Result<(), SecretError>;
}

/// An in-memory secret store for tests.
#[derive(Default)]
pub struct MemorySecretStore {
    values: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemorySecretStore {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
        Ok(self.values.lock().expect("lock").get(name).cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        self.values
            .lock()
            .expect("lock")
            .insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn get_or_insert(&self, name: &str, value: &str) -> Result<String, SecretError> {
        Ok(self
            .values
            .lock()
            .expect("lock")
            .entry(name.to_string())
            .or_insert_with(|| value.to_string())
            .clone())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.values.lock().expect("lock").remove(name);
        Ok(())
    }
}

type EnvReader = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The provider key resolver. Resolution order per provider:
/// environment variable, config file entry, secret store.
pub struct ProviderKeys {
    env: EnvReader,
    config: HashMap<Provider, String>,
    secrets: Arc<dyn SecretStore>,
}

/// One provider's resolved status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderKeyStatus {
    pub provider: Provider,
    pub source: Option<KeySource>,
}

impl ProviderKeys {
    /// Production resolver: keys from the real environment, the parsed
    /// config entries, and the platform secret store.
    pub fn new(config: HashMap<Provider, String>, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            env: Box::new(|name| std::env::var(name).ok()),
            config,
            secrets,
        }
    }

    /// A resolver with an injected environment, for tests.
    pub fn with_env(
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
        config: HashMap<Provider, String>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        Self {
            env: Box::new(env),
            config,
            secrets,
        }
    }

    /// The key for one provider, with where it came from.
    pub fn resolve(&self, provider: Provider) -> Result<Option<(String, KeySource)>, SecretError> {
        if let Some(key) = (self.env)(provider.env_var())
            && !key.is_empty()
        {
            return Ok(Some((key, KeySource::Env)));
        }
        if let Some(key) = self.config.get(&provider)
            && !key.is_empty()
        {
            return Ok(Some((key.clone(), KeySource::Config)));
        }
        match self.secrets.get(provider.secret_name())? {
            Some(key) if !key.is_empty() => Ok(Some((key, KeySource::SecretFile))),
            _ => Ok(None),
        }
    }

    /// Store a key in the secret store. Environment and config
    /// overrides still win at resolution.
    pub fn set(&self, provider: Provider, key: &str) -> Result<(), SecretError> {
        self.secrets.set(provider.secret_name(), key)
    }

    /// Forget the stored key of one provider. An environment or config
    /// override still resolves after the removal.
    pub fn remove(&self, provider: Provider) -> Result<(), SecretError> {
        self.secrets.delete(provider.secret_name())
    }

    /// Every provider's status, in display order.
    pub fn status(&self) -> Result<Vec<ProviderKeyStatus>, SecretError> {
        PROVIDERS
            .into_iter()
            .map(|provider| {
                Ok(ProviderKeyStatus {
                    provider,
                    source: self.resolve(provider)?.map(|(_, source)| source),
                })
            })
            .collect()
    }

    /// A hash of the resolved keys. Changes when any key changes, so
    /// callers can rebuild derived state (the model router) lazily.
    pub fn fingerprint(&self) -> Result<u64, SecretError> {
        let mut hasher = DefaultHasher::new();
        for provider in PROVIDERS {
            provider.id().hash(&mut hasher);
            self.resolve(provider)?
                .map(|(key, _)| key)
                .hash(&mut hasher);
        }
        Ok(hasher.finish())
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;

    #[test]
    fn a_source_names_where_an_administrator_finds_the_key() {
        assert_eq!(
            KeySource::SecretFile.label(Provider::Anthropic),
            "secrets.enc"
        );
        assert_eq!(
            KeySource::Env.label(Provider::OpenRouter),
            "the OPENROUTER_API_KEY environment variable"
        );
        assert_eq!(KeySource::Config.label(Provider::OpenAi), "config.toml");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn provider_ids_round_trip() {
        for provider in PROVIDERS {
            assert_eq!(Provider::from_id(provider.id()), Some(provider));
        }
        assert_eq!(Provider::from_id("no-such"), None);
    }

    #[test]
    fn unconfigured_provider_resolves_to_none() {
        let keys = ProviderKeys::with_env(
            no_env,
            HashMap::new(),
            Arc::new(MemorySecretStore::default()),
        );
        assert_eq!(keys.resolve(Provider::Anthropic).unwrap(), None);
    }

    #[test]
    fn stored_key_resolves_from_the_secret_file() {
        let keys = ProviderKeys::with_env(
            no_env,
            HashMap::new(),
            Arc::new(MemorySecretStore::default()),
        );
        keys.set(Provider::Anthropic, "sk-stored").unwrap();
        assert_eq!(
            keys.resolve(Provider::Anthropic).unwrap(),
            Some(("sk-stored".to_string(), KeySource::SecretFile))
        );
    }

    #[test]
    fn a_removed_key_resolves_to_none() {
        let keys = ProviderKeys::with_env(
            no_env,
            HashMap::new(),
            Arc::new(MemorySecretStore::default()),
        );
        keys.set(Provider::Anthropic, "sk-stored").unwrap();
        keys.remove(Provider::Anthropic).unwrap();
        assert_eq!(keys.resolve(Provider::Anthropic).unwrap(), None);
        // A second removal of a key that is not there is not an error.
        keys.remove(Provider::Anthropic).unwrap();
    }

    #[test]
    fn env_wins_over_config_and_the_secret_file() {
        let keys = ProviderKeys::with_env(
            |name| (name == "ANTHROPIC_API_KEY").then(|| "sk-env".to_string()),
            HashMap::from([(Provider::Anthropic, "sk-config".to_string())]),
            Arc::new(MemorySecretStore::default()),
        );
        keys.set(Provider::Anthropic, "sk-stored").unwrap();
        assert_eq!(
            keys.resolve(Provider::Anthropic).unwrap(),
            Some(("sk-env".to_string(), KeySource::Env))
        );
    }

    #[test]
    fn config_wins_over_the_secret_file() {
        let keys = ProviderKeys::with_env(
            no_env,
            HashMap::from([(Provider::OpenAi, "sk-config".to_string())]),
            Arc::new(MemorySecretStore::default()),
        );
        keys.set(Provider::OpenAi, "sk-stored").unwrap();
        assert_eq!(
            keys.resolve(Provider::OpenAi).unwrap(),
            Some(("sk-config".to_string(), KeySource::Config))
        );
    }

    #[test]
    fn status_reports_every_provider_in_order() {
        let keys = ProviderKeys::with_env(
            no_env,
            HashMap::new(),
            Arc::new(MemorySecretStore::default()),
        );
        keys.set(Provider::OpenAi, "sk").unwrap();
        let status = keys.status().unwrap();
        assert_eq!(
            status,
            vec![
                ProviderKeyStatus {
                    provider: Provider::Anthropic,
                    source: None,
                },
                ProviderKeyStatus {
                    provider: Provider::OpenAi,
                    source: Some(KeySource::SecretFile),
                },
                ProviderKeyStatus {
                    provider: Provider::OpenRouter,
                    source: None,
                },
            ]
        );
    }

    /// Callers that race to create one name with different values all
    /// get the one value that the store keeps.
    #[test]
    fn callers_that_race_to_create_one_name_all_get_the_stored_value() {
        const CALLERS: usize = 8;
        let store = MemorySecretStore::default();

        for round in 0..500 {
            let name = format!("workspace/ws-{round}/vault_data_key");
            let barrier = std::sync::Barrier::new(CALLERS);
            let answers = std::thread::scope(|scope| {
                let callers = (0..CALLERS)
                    .map(|caller| {
                        let (store, name, barrier) = (&store, &name, &barrier);
                        scope.spawn(move || {
                            barrier.wait();
                            store.get_or_insert(name, &format!("value-{caller}"))
                        })
                    })
                    .collect::<Vec<_>>();
                callers
                    .into_iter()
                    .map(|caller| caller.join().expect("caller").expect("get_or_insert"))
                    .collect::<Vec<_>>()
            });

            let stored = store.get(&name).unwrap().expect("the name holds a value");
            assert!(
                answers.iter().all(|answer| *answer == stored),
                "round {round}: the callers got {answers:?}, and the store holds {stored}"
            );
        }
    }

    #[test]
    fn fingerprint_changes_when_a_key_changes() {
        let keys = ProviderKeys::with_env(
            no_env,
            HashMap::new(),
            Arc::new(MemorySecretStore::default()),
        );
        let before = keys.fingerprint().unwrap();
        keys.set(Provider::Anthropic, "sk-new").unwrap();
        let after = keys.fingerprint().unwrap();
        assert_ne!(before, after);
        assert_eq!(after, keys.fingerprint().unwrap());
    }
}
