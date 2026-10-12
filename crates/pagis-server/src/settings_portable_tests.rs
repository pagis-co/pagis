use super::*;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
    routing::get,
};
use pagis_core::{MemorySecretStore, UserRole, Workspace, WorkspaceId, WorkspaceStore};
use pagis_storage_sqlite::{
    SqliteOnboardingStore, SqliteWorkspaceStore, connect_memory, seed_org_and_administrator,
};
use tower::ServiceExt;

#[tokio::test]
async fn onboarding_status_without_computers_holds_docker_null() {
    let pool = connect_memory().await.unwrap();
    pagis_storage_sqlite::MIGRATOR.run(&pool).await.unwrap();
    let person = seed_org_and_administrator(&pool, "Pagis", 1).await.unwrap();
    let workspaces = Arc::new(SqliteWorkspaceStore::new(pool.clone()));
    let workspace = Workspace {
        id: WorkspaceId::generate(),
        user_id: person.id.clone(),
        name: "Ada".into(),
        timezone: "UTC".into(),
        created_at: 1,
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    };
    workspaces.create(&workspace).await.unwrap();
    let state = OnboardingState {
        workspaces: workspaces.clone(),
        onboarding: Arc::new(SqliteOnboardingStore::new(pool)),
        keys: Arc::new(ProviderKeys::with_env(
            |_| None,
            Default::default(),
            Arc::new(MemorySecretStore::default()),
        )),
    };
    let router = Router::new()
        .route("/api/v1/settings/onboarding", get(onboarding_status))
        .with_state(state);
    for completed in [false, true] {
        for role in [UserRole::Administrator, UserRole::Member] {
            if completed {
                workspaces.set_onboarded(&workspace.id, 2).await.unwrap();
            }
            let mut request = Request::builder()
                .uri("/api/v1/settings/onboarding")
                .body(Body::empty())
                .unwrap();
            request.extensions_mut().insert(Tenant {
                workspace_id: workspace.id.clone(),
                user_id: person.id.clone(),
                role,
                session_id: pagis_core::SessionId::generate(),
                session_expires_at: i64::MAX,
            });
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert!(body.as_object().unwrap().contains_key("docker"));
            assert_eq!(body["docker"], serde_json::Value::Null);
            assert_eq!(body["docker_endpoint"], serde_json::Value::Null);
            assert_eq!(body["completed"], completed);
            assert_eq!(body["checks"], serde_json::json!([]));
            if role == UserRole::Member {
                assert_eq!(body["providers"], serde_json::json!([]));
            } else {
                assert!(!body["providers"].as_array().unwrap().is_empty());
            }
        }
    }
}
