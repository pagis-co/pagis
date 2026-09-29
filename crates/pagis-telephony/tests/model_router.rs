//! The realtime session the daemon opens from the Provider Keys.
//! The key resolves on every call, so a key the wizard stores
//! takes effect without a restart, and a daemon with no OpenAI key
//! fails the call with a stated reason instead of dialing a line
//! nobody can talk on.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use pagis_core::{
    MemorySecretStore, ModelAlias, ModelAliasId, ModelAliasStore, Provider, ProviderKeys,
    StoreError, WorkspaceId,
};
use pagis_telephony::model::{ClientCommand, ModelSessions, SessionConfig};
use pagis_telephony::{
    GPT_LIVE_REASONING_ALIAS, GPT_LIVE_REASONING_MODELS, KeyedModelSessions, PHONE_ALIAS,
    PHONE_CLASSIFIER_ALIAS, PHONE_CLASSIFIER_MODELS, PHONE_MODELS,
};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

fn keys(env: Vec<(&'static str, &'static str)>) -> Arc<ProviderKeys> {
    Arc::new(ProviderKeys::with_env(
        move |name| {
            env.iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| value.to_string())
        },
        HashMap::new(),
        Arc::new(MemorySecretStore::default()),
    ))
}

struct Aliases {
    phone: Vec<String>,
    reasoning: Option<Vec<String>>,
}

impl Default for Aliases {
    fn default() -> Self {
        Self {
            phone: PHONE_MODELS.iter().map(|model| model.to_string()).collect(),
            reasoning: Some(
                GPT_LIVE_REASONING_MODELS
                    .iter()
                    .map(|model| model.to_string())
                    .collect(),
            ),
        }
    }
}

#[async_trait]
impl ModelAliasStore for Aliases {
    async fn create(&self, _: &ModelAlias) -> Result<(), StoreError> {
        unreachable!()
    }

    async fn get_by_alias(
        &self,
        workspace_id: &WorkspaceId,
        alias: &str,
    ) -> Result<Option<ModelAlias>, StoreError> {
        let candidates = match alias {
            PHONE_ALIAS => {
                return Ok(Some(model_alias(workspace_id, alias, self.phone.clone())));
            }
            GPT_LIVE_REASONING_ALIAS => {
                let Some(candidates) = self.reasoning.clone() else {
                    return Ok(None);
                };
                return Ok(Some(model_alias(workspace_id, alias, candidates)));
            }
            PHONE_CLASSIFIER_ALIAS => PHONE_CLASSIFIER_MODELS.to_vec(),
            _ => return Ok(None),
        };
        Ok(Some(model_alias(
            workspace_id,
            alias,
            candidates.into_iter().map(str::to_string).collect(),
        )))
    }

    async fn list(&self, _: &WorkspaceId) -> Result<Vec<ModelAlias>, StoreError> {
        unreachable!()
    }

    async fn update_candidates(
        &self,
        _: &WorkspaceId,
        _: &str,
        _: &[String],
        _: i64,
    ) -> Result<bool, StoreError> {
        unreachable!()
    }

    async fn delete(&self, _: &WorkspaceId, _: &str) -> Result<bool, StoreError> {
        unreachable!()
    }
}

fn sessions(keys: Arc<ProviderKeys>) -> KeyedModelSessions {
    sessions_with_aliases(keys, Aliases::default())
}

fn sessions_with_aliases(keys: Arc<ProviderKeys>, aliases: Aliases) -> KeyedModelSessions {
    KeyedModelSessions::new(keys, Arc::new(aliases))
}

/// The one tenant these bodies act as. The session seam takes the
/// Workspace per call.
fn workspace() -> WorkspaceId {
    WorkspaceId::from("ws-calls".to_string())
}

fn model_alias(workspace_id: &WorkspaceId, alias: &str, candidates: Vec<String>) -> ModelAlias {
    ModelAlias {
        id: ModelAliasId::generate(),
        workspace_id: workspace_id.clone(),
        alias: alias.to_string(),
        candidates,
        created_at: 0,
        updated_at: 0,
    }
}

#[tokio::test]
async fn a_daemon_with_no_openai_key_names_the_missing_key() {
    let sessions = sessions(keys(vec![("ANTHROPIC_API_KEY", "sk-ant")]));

    let error = sessions
        .open(&workspace())
        .await
        .err()
        .expect("no session opens");

    assert!(
        error.0.contains("OpenAI"),
        "the reason names the provider: {}",
        error.0
    );
}

#[tokio::test]
async fn the_key_is_read_on_every_open() {
    let secrets = Arc::new(MemorySecretStore::default());
    // The process environment stays out of it.
    let keys = Arc::new(ProviderKeys::with_env(
        |_| None,
        HashMap::new(),
        Arc::clone(&secrets) as _,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::text(
                serde_json::json!({ "type": "session.started", "session": { "id": "s1" } })
                    .to_string(),
            ))
            .await
            .unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::text(
                serde_json::json!({ "type": "session.closed" }).to_string(),
            ))
            .await
            .unwrap();
    });
    let sessions = sessions(Arc::clone(&keys)).with_base_url(
        Provider::OpenAi,
        format!("http://127.0.0.1:{}/v1", address.port()),
    );

    let before = sessions
        .open(&workspace())
        .await
        .err()
        .expect("no key, no session");
    assert!(before.0.contains("OpenAI"), "{}", before.0);

    keys.set(Provider::OpenAi, "sk-test")
        .expect("the key is stored");
    let mut session = sessions
        .open(&workspace())
        .await
        .expect("the key opens a session factory");
    session
        .send(ClientCommand::Configure(SessionConfig::new(
            "test",
            Vec::new(),
            None,
        )))
        .await
        .expect("the new key reaches the provider");
    session.close().await;
    server.await.unwrap();
}

#[tokio::test]
async fn gpt_live_starts_with_pcmu_and_the_configured_responses_backend() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let start = socket.next().await.unwrap().unwrap().into_text().unwrap();
        let start: serde_json::Value = serde_json::from_str(&start).unwrap();
        socket
            .send(Message::text(
                serde_json::json!({ "type": "session.started", "session": { "id": "s1" } })
                    .to_string(),
            ))
            .await
            .unwrap();
        let close = socket.next().await.unwrap().unwrap().into_text().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&close).unwrap()["type"],
            "session.close"
        );
        socket
            .send(Message::text(
                serde_json::json!({ "type": "session.closed" }).to_string(),
            ))
            .await
            .unwrap();
        start
    });
    let sessions = sessions(keys(vec![("OPENAI_API_KEY", "sk-test")])).with_base_url(
        Provider::OpenAi,
        format!("http://127.0.0.1:{}/v1", address.port()),
    );

    let mut session = sessions.open(&workspace()).await.unwrap();
    session
        .send(ClientCommand::Configure(SessionConfig::new(
            "Speak plainly.",
            Vec::new(),
            Some("marin".to_string()),
        )))
        .await
        .unwrap();
    session.close().await;
    let start = server.await.unwrap();

    assert_eq!(start["type"], "session.start");
    assert_eq!(start["session"]["model"], "gpt-live-1");
    assert_eq!(start["session"]["audio"]["format"]["type"], "audio/pcmu");
    assert_eq!(start["session"]["audio"]["format"]["rate"], 8000);
    assert_eq!(
        start["session"]["delegation"]["responses"]["model"],
        "gpt-5.6-terra"
    );
}

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn the_phone_alias_can_select_openai_realtime_2_1() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut path = String::new();
        let mut socket =
            tokio_tungstenite::accept_hdr_async(stream, |request: &Request, response| {
                path = request.uri().to_string();
                Ok::<Response, tokio_tungstenite::tungstenite::handshake::server::ErrorResponse>(
                    response,
                )
            })
            .await
            .unwrap();
        let update = socket.next().await.unwrap().unwrap().into_text().unwrap();
        (
            path,
            serde_json::from_str::<serde_json::Value>(&update).unwrap(),
        )
    });
    let sessions = sessions_with_aliases(
        keys(vec![("OPENAI_API_KEY", "sk-test")]),
        Aliases {
            phone: vec!["openai/gpt-realtime-2.1".to_string()],
            reasoning: None,
        },
    )
    .with_base_url(
        Provider::OpenAi,
        format!("http://127.0.0.1:{}/v1", address.port()),
    );

    let mut session = sessions.open(&workspace()).await.unwrap();
    session
        .send(ClientCommand::Configure(SessionConfig::new(
            "Speak plainly.",
            Vec::new(),
            None,
        )))
        .await
        .unwrap();
    session.close().await;
    let (path, update) = server.await.unwrap();

    assert_eq!(path, "/v1/realtime?model=gpt-realtime-2.1");
    assert_eq!(update["type"], "session.update");
}
