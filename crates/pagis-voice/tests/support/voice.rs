use std::sync::Arc;

use pagis_core::{
    MemorySecretStore, ModelAlias, ModelAliasId, ModelAliasStore, Provider, ProviderKeys,
    StoreError, WorkspaceId, now_ms,
};
use pagis_voice::{RouterVoice, SPEAK_ALIAS, TRANSCRIBE_ALIAS};

/// An alias store with the two voice aliases and nothing else.
struct Aliases {
    workspace_id: WorkspaceId,
    transcribe: Vec<String>,
    speak: Vec<String>,
}

#[async_trait::async_trait]
impl ModelAliasStore for Aliases {
    async fn create(&self, _: &ModelAlias) -> Result<(), StoreError> {
        unreachable!("the tests never create an alias")
    }

    async fn get_by_alias(
        &self,
        _: &WorkspaceId,
        alias: &str,
    ) -> Result<Option<ModelAlias>, StoreError> {
        let candidates = match alias {
            TRANSCRIBE_ALIAS => self.transcribe.clone(),
            SPEAK_ALIAS => self.speak.clone(),
            _ => return Ok(None),
        };
        Ok(Some(ModelAlias {
            id: ModelAliasId::generate(),
            workspace_id: self.workspace_id.clone(),
            alias: alias.to_string(),
            candidates,
            created_at: now_ms(),
            updated_at: now_ms(),
        }))
    }

    async fn list(&self, _: &WorkspaceId) -> Result<Vec<ModelAlias>, StoreError> {
        Ok(Vec::new())
    }

    async fn update_candidates(
        &self,
        _: &WorkspaceId,
        _: &str,
        _: &[String],
        _: i64,
    ) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn delete(&self, _: &WorkspaceId, _: &str) -> Result<bool, StoreError> {
        Ok(false)
    }
}

fn keys(provider: Provider) -> Arc<ProviderKeys> {
    let keys = ProviderKeys::with_env(
        |_| None,
        std::collections::HashMap::new(),
        Arc::new(MemorySecretStore::default()),
    );
    keys.set(provider, "test-key").unwrap();
    Arc::new(keys)
}

/// The one tenant these bodies act as. The voice seam takes the Workspace
/// per call, so the test names the same one the aliases are under.
pub fn workspace() -> WorkspaceId {
    WorkspaceId::from("ws-voice".to_string())
}

pub fn voice(
    provider: Provider,
    base_url: &str,
    transcribe: &[&str],
    speak: &[&str],
) -> RouterVoice {
    RouterVoice::new(
        keys(provider),
        Arc::new(Aliases {
            workspace_id: workspace(),
            transcribe: transcribe.iter().map(|c| c.to_string()).collect(),
            speak: speak.iter().map(|c| c.to_string()).collect(),
        }),
    )
    .with_base_url(provider, base_url)
}
