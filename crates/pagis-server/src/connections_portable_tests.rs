//! Portable Provider Key and Model Alias reads.

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use pagis_core::*;
use tower::ServiceExt;

use crate::auth::Tenant;

fn keys() -> Arc<ProviderKeys> {
    Arc::new(ProviderKeys::with_env(
        |_| None,
        Default::default(),
        Arc::new(MemorySecretStore::default()),
    ))
}

fn request(path: &str) -> Request<Body> {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    request.extensions_mut().insert(Tenant {
        workspace_id: WorkspaceId::from("workspace".to_string()),
        user_id: UserId::generate(),
        role: UserRole::Administrator,
        session_id: SessionId::generate(),
        session_expires_at: i64::MAX,
    });
    request
}

#[tokio::test]
async fn portable_provider_keys_answer_with_their_source_and_no_secret() {
    let keys = keys();
    keys.set(Provider::Anthropic, "test-key").unwrap();
    let router = Router::new()
        .route(
            "/api/v1/administration/providers",
            get(crate::providers::list_provider_setups),
        )
        .with_state(crate::providers::ProviderSetupState { keys });
    let response = router
        .oneshot(request("/api/v1/administration/providers"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(!std::str::from_utf8(&bytes).unwrap().contains("test-key"));
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), PROVIDERS.len());
    let provider = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["provider"] == "anthropic")
        .unwrap();
    assert_eq!(provider["group"], "models");
    assert_eq!(provider["parts"][0]["id"], "key");
    assert_eq!(provider["parts"][0]["kind"], "model_key");
    assert_eq!(provider["parts"][0]["configured"], true);
    assert_eq!(provider["parts"][0]["facts"][0]["value"], "secrets.enc");
}

#[tokio::test]
async fn portable_model_aliases_answer_and_read_only_the_persons_workspace() {
    let stores = pagis_core::fake::stores();
    let keys = keys();
    keys.set(Provider::Anthropic, "test-key").unwrap();
    for (workspace, alias) in [("workspace", "default"), ("another", "private")] {
        stores
            .model_aliases
            .create(&ModelAlias {
                id: ModelAliasId::generate(),
                workspace_id: WorkspaceId::from(workspace.to_string()),
                alias: alias.into(),
                candidates: vec!["anthropic/claude-sonnet-4-6".into()],
                created_at: 1,
                updated_at: 1,
            })
            .await
            .unwrap();
    }
    let router = Router::new()
        .route(
            "/api/v1/settings/model-aliases",
            get(crate::settings::list_model_aliases),
        )
        .with_state(crate::settings::ModelAliasState {
            model_aliases: stores.model_aliases,
            keys,
        });
    let response = router
        .oneshot(request("/api/v1/settings/model-aliases"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["alias"], "default");
    assert_eq!(body["items"][0]["reachable"], true);
    assert_eq!(
        body["items"][0]["candidates"],
        serde_json::json!(["anthropic/claude-sonnet-4-6"])
    );
}
