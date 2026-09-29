use std::time::Duration;

use llm_router::{Candidate, ProtocolKind, ProviderConfig, RetryConfig, Router, RouterConfig};

/// A router with one provider `p` (key `test-key`) and one alias `m` that
/// maps to `concrete-model` on that provider. Retries are off so error tests
/// stay deterministic and fast.
pub fn single_provider_router(kind: ProtocolKind, base_url: &str) -> Router {
    let config = RouterConfig::new()
        .provider("p", ProviderConfig::new(kind, base_url, "test-key"))
        .model("m", [Candidate::new("p", "concrete-model")])
        .retry(RetryConfig {
            max_attempts: 1,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(1),
        });
    Router::new(config).unwrap()
}
