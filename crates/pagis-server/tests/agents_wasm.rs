//! The Agent routes over fake stores, with no listener on either target.

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

async fn response(
    embedder: &Embedder,
    method: &str,
    path: &str,
    body: Value,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()))
        .header("content-type", "application/json");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let response = embedder
        .routers
        .product
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn lists_only_the_agents_of_the_session_workspace() {
    let embedder = Embedder::new().await;
    let own = embedder.agent(&embedder.workspace, "Ada").await;
    let other = embedder.agent(&embedder.other_workspace, "Grace").await;
    let (status, body) = response(
        &embedder,
        "GET",
        "/api/v1/agents",
        Value::Null,
        Some(&embedder.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], own.id.as_str());
    assert_eq!(items[0]["name"], "Ada");
    assert!(items.iter().all(|item| item["id"] != other.id.as_str()));
    let (status, body) = response(
        &embedder,
        "GET",
        "/api/v1/agents",
        Value::Null,
        Some(&embedder.other_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], other.id.as_str());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn creates_an_agent_its_dm_participants_greeting_and_events() {
    let embedder = Embedder::new().await;
    let (status, body) = response(
        &embedder,
        "POST",
        "/api/v1/agents",
        json!({"name": "Ada", "job": "Assistant", "voice": "alloy"}),
        Some(&embedder.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = AgentId::from(body["id"].as_str().unwrap().to_string());
    let agent = embedder
        .stores
        .agents
        .get(&embedder.workspace, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent.name, "Ada");
    assert_eq!(agent.job, "Assistant");
    assert_eq!(agent.voice.as_deref(), Some("alloy"));
    assert!(
        embedder
            .stores
            .agents
            .get(&embedder.other_workspace, &id)
            .await
            .unwrap()
            .is_none()
    );
    let channels = embedder
        .stores
        .channels
        .list_by_workspace(&embedder.workspace)
        .await
        .unwrap();
    assert_eq!(channels.len(), 1);
    let dm = &channels[0];
    assert_eq!(dm.kind, ChannelKind::Dm);
    assert_eq!(dm.title.as_deref(), Some("Ada"));
    assert_eq!(
        embedder
            .stores
            .channels
            .find_user_dm(&embedder.workspace, &id)
            .await
            .unwrap()
            .unwrap()
            .id,
        dm.id
    );
    let participants = embedder
        .stores
        .participants
        .list_for_channel(&embedder.workspace, &dm.id)
        .await
        .unwrap();
    assert_eq!(participants.len(), 2);
    assert!(
        participants
            .iter()
            .all(|p| p.workspace_id == embedder.workspace && p.channel_id == dm.id)
    );
    assert_eq!(
        participants
            .iter()
            .filter(|p| p.kind == ParticipantKind::User && p.agent_id.is_none())
            .count(),
        1
    );
    assert_eq!(
        participants
            .iter()
            .filter(|p| p.kind == ParticipantKind::Agent && p.agent_id.as_ref() == Some(&id))
            .count(),
        1
    );
    let messages = embedder
        .stores
        .messages
        .list_top_level(&embedder.workspace, &dm.id, None, 10)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    let greeting = &messages[0].message;
    assert_eq!(greeting.workspace_id, embedder.workspace);
    assert_eq!(greeting.author_kind, AuthorKind::Agent);
    assert_eq!(greeting.author_agent_id, Some(id.clone()));
    assert_eq!(
        greeting.blocks,
        [Block::markdown("Hi, I'm Ada, your Assistant.")]
    );
    assert_eq!(greeting.text_content, "Hi, I'm Ada, your Assistant.");
    assert_eq!(greeting.status, MessageStatus::Complete);
    assert_eq!(greeting.parent_message_id, None);
    let events = embedder
        .stores
        .events
        .list_after(Some(&embedder.workspace), 0, 10)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| e.event_type.as_str())
            .collect::<Vec<_>>(),
        ["agent.created", "channel.created", "message.completed"]
    );
    assert!(
        events
            .iter()
            .all(|e| e.workspace_id == embedder.workspace && e.agent_id.as_ref() == Some(&id))
    );
    assert_eq!(events[0].payload["name"], "Ada");
    assert_eq!(events[1].channel_id.as_ref(), Some(&dm.id));
    assert_eq!(events[2].channel_id.as_ref(), Some(&dm.id));
    assert_eq!(events[2].payload["message_id"], greeting.id.as_str());
    for event in &events {
        assert_eq!(
            embedder.routers.state.ring.position(event.id.as_str()),
            Some(event.seq)
        );
    }
    assert!(
        embedder
            .stores
            .events
            .list_after(Some(&embedder.other_workspace), 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        embedder
            .stores
            .channels
            .list_by_workspace(&embedder.other_workspace)
            .await
            .unwrap()
            .is_empty()
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn refuses_requests_without_a_session() {
    let embedder = Embedder::new().await;
    for cookie in [None, Some("pagis_session=unknown")] {
        for (method, path, body) in [
            ("GET", "/api/v1/agents", Value::Null),
            (
                "POST",
                "/api/v1/agents",
                json!({"name": "Ada", "job": "Assistant"}),
            ),
            ("GET", "/api/v1/agents/agent/computer", Value::Null),
        ] {
            let (status, _) = response(&embedder, method, path, body, cookie).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
    }
    assert!(
        embedder
            .stores
            .agents
            .list_by_workspace(&embedder.workspace)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        embedder
            .stores
            .channels
            .list_by_workspace(&embedder.workspace)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        embedder
            .stores
            .events
            .list_after(Some(&embedder.workspace), 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_computer_is_not_available_without_the_native_part() {
    let embedder = Embedder::new().await;
    let agent = embedder.agent(&embedder.workspace, "Ada").await;
    let (status, body) = response(
        &embedder,
        "GET",
        &format!("/api/v1/agents/{}/computer", agent.id),
        Value::Null,
        Some(&embedder.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(body["error"]["code"], "not_available");
}
