//! The Provider Model Lists: the models each provider lists for the
//! installation's key, kept per provider.
//!
//! A provider key belongs to the installation, so one list serves every
//! Workspace. A list is fresh for [`MODEL_LIST_REFRESH`], and a key
//! change makes it stale at once: the cache keeps a hash of the key it
//! was fetched with. The context budget and the cost of a turn read the
//! cache and never wait on a provider; the Models settings, the key
//! check and the refresher fetch.
//!
//! The metadata of one model comes in layers (`llm_router::ModelMetadata`):
//! what the provider lists for it, then `models.json`, then a
//! conservative default. A model that the provider lists and the table
//! does not know still runs, and its cost is unknown.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use llm_router::{ListedModel, ModelMetadata, Router, RouterConfig};
use pagis_core::{PROVIDERS, Provider, ProviderKeys};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::brain::provider_config;

/// How long one fetched list stays fresh. The providers add models a few
/// times a month, so an hourly list shows a new model on the day it
/// ships and asks each provider 24 times a day.
pub const MODEL_LIST_REFRESH: Duration = Duration::from_secs(60 * 60);

/// Why a list is not available.
#[derive(Debug, thiserror::Error)]
pub enum ModelListError {
    #[error("no key is configured for {0}")]
    NoKey(&'static str),
    #[error("{0}")]
    Secret(String),
    /// The provider's own answer. A refused key is `Provider` with
    /// status 401 or 403 and the provider's message.
    #[error(transparent)]
    Provider(#[from] llm_router::Error),
}

struct CachedList {
    key_hash: u64,
    fetched_at: Instant,
    models: Arc<[ListedModel]>,
}

pub struct ModelCatalog {
    keys: Arc<ProviderKeys>,
    /// Base URL overrides per provider. Production uses the provider
    /// defaults; tests point a provider at a local server.
    base_urls: HashMap<Provider, String>,
    lists: Mutex<HashMap<Provider, CachedList>>,
}

impl ModelCatalog {
    pub fn new(keys: Arc<ProviderKeys>) -> Self {
        Self {
            keys,
            base_urls: HashMap::new(),
            lists: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_base_url(mut self, provider: Provider, base_url: impl Into<String>) -> Self {
        self.base_urls.insert(provider, base_url.into());
        self
    }

    /// The provider's list: the cached one while it is fresh and was
    /// fetched with the current key, otherwise a new fetch.
    pub async fn models(&self, provider: Provider) -> Result<Arc<[ListedModel]>, ModelListError> {
        let (key, key_hash) = self.key(provider)?;
        if let Some(cached) = self.lists().get(&provider)
            && cached.key_hash == key_hash
            && cached.fetched_at.elapsed() < MODEL_LIST_REFRESH
        {
            return Ok(Arc::clone(&cached.models));
        }
        self.fetch(provider, key, key_hash).await
    }

    /// Ask the provider for its list now, whatever the cache holds. This
    /// is one list call: it generates nothing and costs nothing.
    pub async fn refresh(&self, provider: Provider) -> Result<Arc<[ListedModel]>, ModelListError> {
        let (key, key_hash) = self.key(provider)?;
        self.fetch(provider, key, key_hash).await
    }

    /// Ask the provider for its list with `key`, which the installation
    /// need not hold yet. The list is cached for that key, so the key
    /// that is stored next is served from the cache.
    pub async fn check(
        &self,
        provider: Provider,
        key: &str,
    ) -> Result<Arc<[ListedModel]>, ModelListError> {
        self.fetch(provider, key.to_string(), key_hash(key)).await
    }

    /// Drop the provider's list, after its key changed or went away.
    pub fn forget(&self, provider: Provider) {
        self.lists().remove(&provider);
    }

    /// The provider's cached entry for `model`, with no fetch.
    pub fn listed(&self, provider: &str, model: &str) -> Option<ListedModel> {
        let provider = Provider::from_id(provider)?;
        self.lists()
            .get(&provider)?
            .models
            .iter()
            .find(|listed| listed.id == model)
            .cloned()
    }

    /// The layered metadata of one `provider/model` candidate, from the
    /// cache and the table alone.
    pub fn metadata(&self, provider: &str, model: &str) -> ModelMetadata {
        llm_router::model_metadata(model, self.listed(provider, model).as_ref())
    }

    /// Fetch the list of every provider that holds a key, now and then
    /// every [`MODEL_LIST_REFRESH`], until `cancel` fires. A failure is
    /// logged and the old list stays until the next round.
    pub fn spawn_refresher(self: &Arc<Self>, cancel: CancellationToken) {
        let catalog = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                for provider in PROVIDERS {
                    match catalog.models(provider).await {
                        Ok(_) | Err(ModelListError::NoKey(_)) => {}
                        Err(error) => {
                            tracing::warn!(provider = provider.id(), %error, "the model list was not refreshed");
                        }
                    }
                }
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(MODEL_LIST_REFRESH) => {}
                }
            }
        });
    }

    fn key(&self, provider: Provider) -> Result<(String, u64), ModelListError> {
        let (key, _) = self
            .keys
            .resolve(provider)
            .map_err(|error| ModelListError::Secret(error.to_string()))?
            .ok_or(ModelListError::NoKey(provider.id()))?;
        let hash = key_hash(&key);
        Ok((key, hash))
    }

    async fn fetch(
        &self,
        provider: Provider,
        key: String,
        key_hash: u64,
    ) -> Result<Arc<[ListedModel]>, ModelListError> {
        let config = RouterConfig::new().provider(
            provider.id(),
            provider_config(provider, key, self.base_urls.get(&provider)),
        );
        let router = Router::new(config)?;
        let models: Arc<[ListedModel]> = router.list_models(provider.id()).await?.into();
        self.lists().insert(
            provider,
            CachedList {
                key_hash,
                fetched_at: Instant::now(),
                models: Arc::clone(&models),
            },
        );
        Ok(models)
    }

    fn lists(&self) -> std::sync::MutexGuard<'_, HashMap<Provider, CachedList>> {
        self.lists.lock().expect("model list cache lock")
    }
}

fn key_hash(key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use pagis_core::MemorySecretStore;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn keys() -> Arc<ProviderKeys> {
        Arc::new(ProviderKeys::with_env(
            |_| None,
            HashMap::new(),
            Arc::new(MemorySecretStore::default()),
        ))
    }

    async fn openrouter(server: &MockServer, key: &str, ids: &[&str]) {
        let data: Vec<_> = ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id": id,
                    "created": 1,
                    "context_length": 300_000,
                    "pricing": {"prompt": "0.000001", "completion": "0.000002"},
                    "top_provider": {"max_completion_tokens": 20_000}
                })
            })
            .collect();
        Mock::given(method("GET"))
            .and(path("/models/user"))
            .and(header("authorization", format!("Bearer {key}").as_str()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": data})),
            )
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_fresh_list_is_served_from_the_cache() {
        let server = MockServer::start().await;
        openrouter(&server, "sk-one", &["vendor/new-model"]).await;
        let keys = keys();
        keys.set(Provider::OpenRouter, "sk-one").unwrap();
        let catalog = ModelCatalog::new(keys).with_base_url(Provider::OpenRouter, server.uri());

        catalog.models(Provider::OpenRouter).await.unwrap();
        let models = catalog.models(Provider::OpenRouter).await.unwrap();

        assert_eq!(models[0].id, "vendor/new-model");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_stale_list_is_fetched_again() {
        let server = MockServer::start().await;
        openrouter(&server, "sk-one", &["vendor/new-model"]).await;
        let keys = keys();
        keys.set(Provider::OpenRouter, "sk-one").unwrap();
        let catalog = ModelCatalog::new(keys).with_base_url(Provider::OpenRouter, server.uri());
        catalog.models(Provider::OpenRouter).await.unwrap();

        // Age the cached list past the refresh interval.
        catalog
            .lists()
            .get_mut(&Provider::OpenRouter)
            .expect("a cached list")
            .fetched_at = Instant::now()
            .checked_sub(MODEL_LIST_REFRESH)
            .expect("the clock is past one refresh interval");
        catalog.models(Provider::OpenRouter).await.unwrap();

        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_new_key_fetches_a_new_list() {
        let server = MockServer::start().await;
        openrouter(&server, "sk-one", &["vendor/one"]).await;
        openrouter(&server, "sk-two", &["vendor/two"]).await;
        let keys = keys();
        keys.set(Provider::OpenRouter, "sk-one").unwrap();
        let catalog =
            ModelCatalog::new(Arc::clone(&keys)).with_base_url(Provider::OpenRouter, server.uri());
        catalog.models(Provider::OpenRouter).await.unwrap();

        keys.set(Provider::OpenRouter, "sk-two").unwrap();
        let models = catalog.models(Provider::OpenRouter).await.unwrap();

        assert_eq!(models[0].id, "vendor/two");
    }

    #[tokio::test]
    async fn a_provider_without_a_key_has_no_list() {
        let catalog = ModelCatalog::new(keys());

        assert!(matches!(
            catalog.models(Provider::Anthropic).await,
            Err(ModelListError::NoKey("anthropic"))
        ));
    }

    #[tokio::test]
    async fn the_metadata_layers_the_listed_entry_over_the_table() {
        let server = MockServer::start().await;
        openrouter(&server, "sk-one", &["vendor/new-model"]).await;
        let keys = keys();
        keys.set(Provider::OpenRouter, "sk-one").unwrap();
        let catalog = ModelCatalog::new(keys).with_base_url(Provider::OpenRouter, server.uri());

        // Before the list arrives, the model gets the default limits
        // and no price.
        let before = catalog.metadata("openrouter", "vendor/new-model");
        assert_eq!(before.context_window, llm_router::DEFAULT_CONTEXT_WINDOW);
        assert_eq!(before.prices, None);

        catalog.models(Provider::OpenRouter).await.unwrap();
        let after = catalog.metadata("openrouter", "vendor/new-model");
        assert_eq!(after.context_window, 300_000);
        assert_eq!(after.max_output_tokens, 20_000);
        assert!((after.prices.unwrap().input_cost - 1.0).abs() < 1e-9);

        catalog.forget(Provider::OpenRouter);
        assert_eq!(catalog.metadata("openrouter", "vendor/new-model"), before);
    }
}
