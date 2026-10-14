use super::*;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use tower::ServiceExt;

#[tokio::test]
async fn sections_name_neither_settings_nor_restart() {
    let sections = AdministrationSections::default();
    assert!(!sections.contains(AdministrationSection::Settings));
    assert!(!sections.contains(AdministrationSection::Restart));
    let router = Router::new()
        .route("/api/v1/administration/sections", get(get_sections))
        .with_state(AdministrationSectionsState {
            sections,
            public_origin: "https://pagis.example.com".into(),
        });
    let mut request = Request::builder()
        .uri("/api/v1/administration/sections")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(Tenant {
        workspace_id: WorkspaceId::generate(),
        user_id: UserId::generate(),
        role: UserRole::Administrator,
        session_id: pagis_core::SessionId::generate(),
        session_expires_at: i64::MAX,
    });
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["links"], serde_json::json!([]));
    assert_eq!(
        body["sections"],
        serde_json::json!([
            "spend",
            "people",
            "sessions",
            "resources",
            "providers",
            "plugins",
            "health"
        ])
    );
}
