use super::*;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use tower::ServiceExt;

async fn read_sections(sections: AdministrationSections) -> serde_json::Value {
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
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

#[tokio::test]
async fn default_sections_offer_hosts_only_with_native() {
    let body = read_sections(AdministrationSections::default()).await;
    let sections = body["sections"].as_array().unwrap();
    assert_eq!(
        sections.contains(&serde_json::json!("hosts")),
        cfg!(feature = "native"),
        "{body}"
    );
    assert!(sections.contains(&serde_json::json!("sessions")));
}

#[tokio::test]
async fn hidden_hosts_are_absent_from_the_sections_route() {
    let hosts: AdministrationSection = serde_json::from_str("\"hosts\"").unwrap();
    let body = read_sections(AdministrationSections::without(&[hosts])).await;
    let sections = body["sections"].as_array().unwrap();
    assert!(!sections.contains(&serde_json::json!("hosts")), "{body}");
    assert!(sections.contains(&serde_json::json!("sessions")));
}

#[tokio::test]
async fn sections_name_the_public_origin() {
    let body = read_sections(AdministrationSections::default()).await;
    assert_eq!(body["public_origin"], "https://pagis.example.com");
}
