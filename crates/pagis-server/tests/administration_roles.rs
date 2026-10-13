//! Role changes through the Administration Port, with no listener.

#![cfg(not(feature = "native"))]

mod support {
    pub mod agents;
}

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use pagis_core::*;
use serde_json::{Value, json};
use support::agents::Embedder;
use tower::ServiceExt;

async fn people(embedder: &Embedder) -> (User, User) {
    let first = embedder
        .stores
        .workspaces
        .get(&embedder.workspace)
        .await
        .unwrap()
        .unwrap();
    let second = embedder
        .stores
        .workspaces
        .get(&embedder.other_workspace)
        .await
        .unwrap()
        .unwrap();
    (
        embedder
            .stores
            .users
            .get(&first.user_id)
            .await
            .unwrap()
            .unwrap(),
        embedder
            .stores
            .users
            .get(&second.user_id)
            .await
            .unwrap()
            .unwrap(),
    )
}

async fn set_role(
    embedder: &Embedder,
    person: &UserId,
    role: &str,
    cookie: &str,
) -> (StatusCode, Value) {
    let response = embedder
        .routers
        .administration
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/administration/people/{person}/role"))
                .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()))
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .body(Body::from(json!({"role": role}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn audit(embedder: &Embedder, org: &OrgId) -> Vec<Event> {
    let workspace = embedder
        .stores
        .orgs
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|row| &row.id == org)
        .unwrap()
        .workspace_id;
    embedder
        .stores
        .events
        .list_by_types(&workspace, &["person.role_changed"], None, 100)
        .await
        .unwrap()
}

#[tokio::test]
async fn role_changes_are_stored_and_audited_and_sessions_use_the_current_role() {
    let embedder = Embedder::new().await;
    let (first, second) = people(&embedder).await;
    let agent = embedder.agent(&embedder.other_workspace, "Grace").await;
    let (status, body) = set_role(&embedder, &second.id, "member", &embedder.cookie).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["role"], "member");
    assert_eq!(
        embedder
            .stores
            .users
            .get(&second.id)
            .await
            .unwrap()
            .unwrap()
            .role,
        UserRole::Member
    );
    assert!(
        embedder
            .stores
            .agents
            .get(&embedder.other_workspace, &agent.id)
            .await
            .unwrap()
            .is_some()
    );
    let events = audit(&embedder, &first.org_id).await;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].payload,
        json!({"administrator_id": first.id, "user_id": second.id, "previous_role": "administrator", "role": "member"})
    );
    assert_eq!(
        set_role(&embedder, &first.id, "member", &embedder.other_cookie)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        set_role(&embedder, &second.id, "administrator", &embedder.cookie)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        embedder
            .stores
            .users
            .get(&second.id)
            .await
            .unwrap()
            .unwrap()
            .role,
        UserRole::Administrator
    );
    // Another enabled Administrator permits a self-demotion.
    assert_eq!(
        set_role(&embedder, &first.id, "member", &embedder.cookie)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        set_role(&embedder, &first.id, "administrator", &embedder.cookie)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(audit(&embedder, &first.org_id).await.len(), 3);
}

#[tokio::test]
async fn the_last_enabled_administrator_cannot_be_demoted_even_by_themselves() {
    let embedder = Embedder::new().await;
    let (first, second) = people(&embedder).await;
    embedder
        .stores
        .users
        .set_disabled(&second.id, Some(now_ms()), now_ms())
        .await
        .unwrap();
    let (status, body) = set_role(&embedder, &first.id, "member", &embedder.cookie).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("enabled Administrator")
    );
    assert_eq!(
        embedder
            .stores
            .users
            .get(&first.id)
            .await
            .unwrap()
            .unwrap()
            .role,
        UserRole::Administrator
    );
    assert!(audit(&embedder, &first.org_id).await.is_empty());
    // Keeping the role is permitted; a disabled Administrator can be demoted.
    assert_eq!(
        set_role(&embedder, &first.id, "administrator", &embedder.cookie)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        set_role(&embedder, &second.id, "member", &embedder.cookie)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        set_role(&embedder, &first.id, "member", &embedder.cookie)
            .await
            .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn role_changes_refuse_unknown_roles_missing_people_and_other_orgs() {
    let embedder = Embedder::new().await;
    let (first, _) = people(&embedder).await;
    assert_eq!(
        set_role(&embedder, &first.id, "owner", &embedder.cookie)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        set_role(&embedder, &UserId::generate(), "member", &embedder.cookie)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let foreign = seed_org_and_administrator(
        embedder.stores.orgs.as_ref(),
        embedder.stores.users.as_ref(),
        "Other Org",
        now_ms(),
    )
    .await
    .unwrap();
    assert_eq!(
        set_role(&embedder, &foreign.id, "member", &embedder.cookie)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert!(audit(&embedder, &first.org_id).await.is_empty());
}
