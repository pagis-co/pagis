//! Agent Mailbox routes for a Mailbox Provider supplied by an Embedder.
#![cfg(not(feature = "native"))]

use crate::support::agents::Embedder;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use pagis_core::*;
use pagis_mail::{HostError, HostErrorCode};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn response(
    embedder: &Embedder,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = embedder
        .routers
        .product
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()))
                .header("content-type", "application/json")
                .header("cookie", &embedder.cookie)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn provider(embedder: &Embedder) -> ConnectionId {
    let id = ConnectionId::generate();
    let config = json!({
        "domain": "example.com",
        "capabilities": {"idle": false, "outgoing_cap": true, "delete_mailbox": true, "reset_password": true}
    });
    embedder
        .stores
        .connections
        .create(&Connection {
            id: id.clone(),
            workspace_id: embedder.routers.state.org_workspace_id.clone(),
            provider: "embedder-mail".into(),
            alias: "mail".into(),
            display_name: "Embedder Mail".into(),
            status: Connection::CONNECTED.into(),
            authorized_capabilities: vec![],
            config,
            created_at: 0,
        })
        .await
        .unwrap();
    embedder
        .routers
        .state
        .secrets
        .set(&pagis_mail::host_api_key_secret_name("mail"), "host-key")
        .unwrap();
    id
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_provider_without_endpoints_provisions_lists_resets_and_deletes_through_the_routes() {
    let embedder = Embedder::new().await;
    let connection = provider(&embedder).await;
    let agent = embedder.agent(&embedder.workspace, "Ada").await;
    let path = format!("/api/v1/agents/{}/mailbox", agent.id);
    let (status, body) = response(&embedder, "GET", &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["offers"][0]["connection_id"], connection.as_str());
    let (status, body) = response(
        &embedder,
        "POST",
        &path,
        json!({"connection_id": connection, "local_part": "ada"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["state"], "active");
    assert_eq!(body["address"], "ada@example.com");
    assert_eq!(embedder.mailbox_host.addresses(), ["ada@example.com"]);
    let (status, body) =
        response(&embedder, "GET", "/api/v1/settings/mailboxes", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["address"], "ada@example.com");
    let (status, body) = response(
        &embedder,
        "POST",
        &format!("{path}/reset-password"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "active");
    let (status, body) = response(
        &embedder,
        "DELETE",
        &path,
        json!({"confirm_address": "ada@example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mailbox"]["state"], "deleted");
    assert!(embedder.mailbox_host.addresses().is_empty());
    let (_, body) = response(&embedder, "GET", "/api/v1/settings/mailboxes", Value::Null).await;
    assert_eq!(body["items"], json!([]));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_host_message_reaches_the_error_body_without_changing_its_shape() {
    let embedder = Embedder::new().await;
    let connection = provider(&embedder).await;
    let agent = embedder.agent(&embedder.workspace, "Ada").await;
    let path = format!("/api/v1/agents/{}/mailbox", agent.id);
    for (error, message) in [
        (
            HostError::with_message(
                HostErrorCode::Refused,
                "Verify your domain before you create a mailbox.",
            ),
            "Verify your domain before you create a mailbox.",
        ),
        (
            HostError::new(HostErrorCode::Refused),
            "the mail host did not complete this (refused)",
        ),
    ] {
        embedder.mailbox_host.fail_with_error(Some(error));
        let (status, body) = response(
            &embedder,
            "POST",
            &path,
            json!({"connection_id": connection, "local_part": "ada"}),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body,
            json!({"error": {"code": "validation", "message": message}})
        );
        assert!(
            embedder
                .stores
                .agent_mailboxes
                .list(&embedder.workspace)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_provider_without_endpoints_reads_mail_through_the_route() {
    let embedder = Embedder::new().await;
    let connection = provider(&embedder).await;
    let agent = embedder.agent(&embedder.workspace, "Ada").await;
    let (status, _) = response(
        &embedder,
        "POST",
        &format!("/api/v1/agents/{}/mailbox", agent.id),
        json!({"connection_id": connection, "local_part": "ada"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = embedder
        .mail
        .deliver("bob@example.net", "Invoice", "Please check this invoice.");
    let (status, body) = response(
        &embedder,
        "GET",
        &format!("/api/v1/mail/ada@example.com/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["text"], "Please check this invoice.");
    assert_eq!(body["subject"], "Invoice");
}
