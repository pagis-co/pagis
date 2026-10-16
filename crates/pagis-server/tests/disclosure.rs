//! Profile and Disclosure Rule routes enforce the Private Workspace boundary.
#![cfg(not(feature = "native"))]
mod support {
    pub mod agents;
}
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use support::agents::Embedder;
use tower::ServiceExt;

async fn call(
    e: &Embedder,
    cookie: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = e
        .routers
        .product
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()))
                .header("content-type", "application/json")
                .header("cookie", cookie)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())),
    )
}

#[tokio::test]
async fn disclosure_settings_are_private_and_validate_the_asker() {
    let e = Embedder::new().await;
    assert_eq!(
        call(&e, &e.cookie, "GET", "/api/v1/disclosure", Value::Null).await,
        (StatusCode::OK, json!({"profile":"", "rules":[]}))
    );
    let (status, result) = call(
        &e,
        &e.cookie,
        "PUT",
        "/api/v1/disclosure/profile",
        json!({"profile":"I design tools."}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["profile"], "I design tools.");
    assert_eq!(
        call(
            &e,
            &e.other_cookie,
            "GET",
            "/api/v1/disclosure",
            Value::Null
        )
        .await
        .1["profile"],
        ""
    );
    let rule = json!({"asker_workspace_id":e.other_workspace, "category":"mail"});
    assert_eq!(
        call(
            &e,
            &e.cookie,
            "POST",
            "/api/v1/disclosure/rules",
            rule.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &e,
            &e.other_cookie,
            "GET",
            "/api/v1/disclosure",
            Value::Null
        )
        .await
        .1["rules"],
        json!([])
    );
    let invalid = json!({"asker_workspace_id":e.workspace, "category":"mail"});
    assert_eq!(
        call(&e, &e.cookie, "POST", "/api/v1/disclosure/rules", invalid)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(
            &e,
            &e.cookie,
            "POST",
            "/api/v1/disclosure/rules",
            json!({"category":"unknown"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(&e, &e.cookie, "DELETE", "/api/v1/disclosure/rules", rule)
            .await
            .1["rules"],
        json!([])
    );
    let org = json!({"asker_workspace_id":null, "category":"calendar_details"});
    assert_eq!(
        call(
            &e,
            &e.cookie,
            "POST",
            "/api/v1/disclosure/rules",
            org.clone()
        )
        .await
        .1["rules"],
        json!([org])
    );
}

#[tokio::test]
async fn disclosure_always_writes_only_the_requested_rule_and_cannot_be_redecided() {
    use pagis_core::{DisclosureCategory, DisclosureRule, Request, RequestId, RequestState};
    let e = Embedder::new().await;
    let agent = e.agent(&e.workspace, "Helper").await;
    let rule = DisclosureRule {
        asker_workspace_id: Some(e.other_workspace.clone()),
        category: DisclosureCategory::Mail,
    };
    let pending = Request {
        id: RequestId::generate(),
        workspace_id: e.workspace.clone(),
        agent_id: agent.id,
        run_id: None,
        kind: Request::TOOL_ACTION_KIND.into(),
        payload: json!({"disclosure_rules":[rule]}),
        state: RequestState::Pending,
        values: None,
        decided_by: None,
        decided_at: None,
        created_at: pagis_core::now_ms(),
    };
    e.stores.requests.create(&pending).await.unwrap();
    let path = format!("/api/v1/requests/{}/decision", pending.id);
    assert_eq!(
        call(
            &e,
            &e.other_cookie,
            "POST",
            &path,
            json!({"decision":"approved","scope":"always"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &e,
            &e.cookie,
            "POST",
            &path,
            json!({"decision":"approved","scope":"always"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        e.stores.disclosure.list(&e.workspace).await.unwrap(),
        vec![rule]
    );
    assert!(
        e.stores
            .disclosure
            .list(&e.other_workspace)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        call(
            &e,
            &e.cookie,
            "POST",
            &path,
            json!({"decision":"approved","scope":"always"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn disclosure_request_api_hides_resume_and_private_arguments() {
    use pagis_core::{Request, RequestId, RequestState};
    let e = Embedder::new().await;
    let agent = e.agent(&e.workspace, "Helper").await;
    let pending = Request {
        id: RequestId::generate(),
        workspace_id: e.workspace.clone(),
        agent_id: agent.id,
        run_id: None,
        kind: Request::TOOL_ACTION_KIND.into(),
        payload: json!({"disclosure_rules":[{"asker_workspace_id":e.other_workspace,"category":"mail"}],"action_title":"Share mail", "body":"Share mail with a colleague?", "arguments":{"query":"private-query-719"}, "call_context":{"arguments":{"query":"private-query-719"}}, "resume":{"arguments":{"query":"private-query-719"}}}),
        state: RequestState::Pending,
        values: None,
        decided_by: None,
        decided_at: None,
        created_at: pagis_core::now_ms(),
    };
    e.stores.requests.create(&pending).await.unwrap();
    for path in [
        "/api/v1/requests".to_owned(),
        format!("/api/v1/requests/{}", pending.id),
    ] {
        let (status, body) = call(&e, &e.cookie, "GET", &path, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !body.to_string().contains("private-query-719"),
            "private resume data is not Approval presentation data"
        );
        assert!(!body.to_string().contains("call_context"));
    }
}

#[tokio::test]
async fn disclosure_always_stores_each_uncovered_asker() {
    use pagis_core::{DisclosureCategory, DisclosureRule, Request, RequestId, RequestState};
    let e = Embedder::new().await;
    let agent = e.agent(&e.workspace, "Helper").await;
    let rules = vec![
        DisclosureRule {
            asker_workspace_id: Some(e.other_workspace.clone()),
            category: DisclosureCategory::Memory,
        },
        DisclosureRule {
            asker_workspace_id: Some(pagis_core::WorkspaceId::generate()),
            category: DisclosureCategory::Memory,
        },
    ];
    let request = Request {
        id: RequestId::generate(),
        workspace_id: e.workspace.clone(),
        agent_id: agent.id,
        run_id: None,
        kind: Request::TOOL_ACTION_KIND.into(),
        payload: json!({"disclosure_rules":rules}),
        state: RequestState::Pending,
        values: None,
        decided_by: None,
        decided_at: None,
        created_at: pagis_core::now_ms(),
    };
    e.stores.requests.create(&request).await.unwrap();
    let (status, _) = call(
        &e,
        &e.cookie,
        "POST",
        &format!("/api/v1/requests/{}/decision", request.id),
        json!({"decision":"approved","scope":"always"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(e.stores.disclosure.list(&e.workspace).await.unwrap(), rules);
}
