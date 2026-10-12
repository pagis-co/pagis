//! Conversation routes run over the same fake state on native targets and in Node.

#![cfg(not(feature = "native"))]

mod support {
    pub mod agents;
    pub mod conversation;
}

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use futures::{SinkExt, StreamExt};
use pagis_core::*;
use pagis_server::Message as Frame;
use pagis_server::fake::{MemorySocket, message_socket_pair};
use serde_json::{Value, json};
use support::agents::Embedder;
use support::conversation::Model;
use tower::ServiceExt;

async fn request(
    e: &Embedder,
    method: &str,
    path: &str,
    body: Value,
    cookie: &str,
) -> (StatusCode, Vec<u8>) {
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
    (
        status,
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
}

async fn json_response(
    e: &Embedder,
    method: &str,
    path: &str,
    body: Value,
    cookie: &str,
    status: StatusCode,
) -> Value {
    let (actual, bytes) = request(e, method, path, body, cookie).await;
    assert_eq!(
        actual,
        status,
        "{method} {path}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

async fn read(e: &Embedder, path: &str) -> Value {
    json_response(e, "GET", path, Value::Null, &e.cookie, StatusCode::OK).await
}

async fn hidden(e: &Embedder, path: &str) {
    json_response(
        e,
        "GET",
        path,
        Value::Null,
        &e.other_cookie,
        StatusCode::NOT_FOUND,
    )
    .await;
}

async fn empty(e: &Embedder, path: &str, field: &str) {
    let body = json_response(e, "GET", path, Value::Null, &e.other_cookie, StatusCode::OK).await;
    assert_eq!(body[field], json!([]), "{path}: {body}");
}

async fn channel(e: &Embedder, agents: &[AgentId]) -> String {
    json_response(
        e,
        "POST",
        "/api/v1/channels",
        json!({"title": "Work", "agent_ids": agents}),
        &e.cookie,
        StatusCode::CREATED,
    )
    .await["id"]
        .as_str()
        .unwrap()
        .into()
}

async fn post(e: &Embedder, channel: &str, key: &str, text: &str, parent: Option<&str>) -> Value {
    json_response(
        e,
        "POST",
        &format!("/api/v1/channels/{channel}/messages"),
        json!({"pending_id": key, "text": text, "parent_message_id": parent}),
        &e.cookie,
        StatusCode::CREATED,
    )
    .await
}

async fn wait_run(e: &Embedder, state: RunState) -> Run {
    pagis_rt::timeout(Duration::from_secs(5), async {
        loop {
            let runs = e
                .stores
                .runs
                .list(&e.workspace, None, None, &[], None, 10)
                .await
                .unwrap();
            if let Some(run) = runs.into_iter().find(|run| run.state == state) {
                return run;
            }
            pagis_rt::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the Run reaches its state")
}

async fn model_embedder(model: &Arc<Model>) -> Embedder {
    let e = Embedder::with_model(model.clone(), model.clone(), model.clone()).await;
    e.stores
        .model_aliases
        .create(&ModelAlias {
            id: ModelAliasId::generate(),
            workspace_id: e.workspace.clone(),
            alias: "default".into(),
            candidates: vec!["anthropic/claude-sonnet-4-6".into()],
            created_at: 0,
            updated_at: 0,
        })
        .await
        .unwrap();
    e
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn channels_messages_and_threads_stay_in_the_session_workspace() {
    let e = Embedder::new().await;
    let c = channel(&e, &[]).await;
    let root = post(&e, &c, "root", "Plan the work.", None).await;
    let id = root["id"].as_str().unwrap();
    let reply = post(&e, &c, "reply", "Read the plan.", Some(id)).await;
    let channels = read(&e, "/api/v1/channels").await;
    assert_eq!(channels["items"].as_array().unwrap().len(), 1);
    assert_eq!(channels["items"][0]["id"], c);
    assert_eq!(channels["items"][0]["title"], "Work");
    empty(&e, "/api/v1/channels", "items").await;
    let path = format!("/api/v1/channels/{c}/messages");
    let timeline = read(&e, &path).await;
    assert_eq!(timeline["items"].as_array().unwrap().len(), 1);
    assert_eq!(timeline["items"][0]["id"], id);
    assert_eq!(timeline["items"][0]["reply_count"], 1);
    hidden(&e, &path).await;
    for message in [&root, &reply] {
        let path = format!(
            "/api/v1/channels/{c}/messages/{}",
            message["id"].as_str().unwrap()
        );
        assert_eq!(
            read(&e, &path).await["text_content"],
            message["text_content"]
        );
        hidden(&e, &path).await;
    }
    let path = format!("/api/v1/channels/{c}/threads/{id}");
    let thread = read(&e, &path).await;
    assert_eq!(thread["root"]["id"], id);
    assert_eq!(thread["replies"].as_array().unwrap().len(), 1);
    assert_eq!(thread["replies"][0]["id"], reply["id"]);
    assert_eq!(thread["replies"][0]["parent_message_id"], id);
    hidden(&e, &path).await;
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn a_message_starts_a_scripted_run_and_cancel_ends_it() {
    let model = Arc::new(Model::default());
    model.tool(
        "add_block",
        json!({"block": {"type": "markdown", "text": "The work starts."}}),
    );
    model.hang();
    let e = model_embedder(&model).await;
    let agent = e.agent(&e.workspace, "Ada").await;
    let c = channel(&e, std::slice::from_ref(&agent.id)).await;
    let message = post(&e, &c, "work", "@Ada Start the work.", None).await;
    let run = wait_run(&e, RunState::Running).await;
    pagis_rt::timeout(Duration::from_secs(5), async {
        while model.requests.lock().unwrap().len() < 2 {
            pagis_rt::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(run.channel_id.as_ref().unwrap().as_str(), c);
    assert_eq!(run.trigger_ref.as_deref(), message["id"].as_str());
    assert_eq!(run.root_message_id, None);
    assert_eq!(run.agent_id, agent.id);
    let runs = read(&e, "/api/v1/runs").await;
    assert_eq!(runs["items"].as_array().unwrap().len(), 1);
    assert_eq!(runs["items"][0]["id"], run.id.as_str());
    empty(&e, "/api/v1/runs", "items").await;
    let steps_path = format!("/api/v1/runs/{}/steps", run.id);
    let steps = read(&e, &steps_path).await;
    assert_eq!(steps["run_id"], run.id.as_str());
    assert_eq!(steps["steps"].as_array().unwrap().len(), 1);
    hidden(&e, &steps_path).await;
    let events_path = format!("/api/v1/runs/{}/events", run.id);
    let events = read(&e, &events_path).await;
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["event_type"] == "tool.completed")
    );
    assert_eq!(events["usage"]["input_tokens"], 7);
    assert_eq!(events["usage"]["output_tokens"], 11);
    hidden(&e, &events_path).await;
    let cancel = format!("/api/v1/runs/{}/cancel", run.id);
    json_response(
        &e,
        "POST",
        &cancel,
        Value::Null,
        &e.other_cookie,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(
        json_response(
            &e,
            "POST",
            &cancel,
            Value::Null,
            &e.cookie,
            StatusCode::ACCEPTED
        )
        .await["outcome"],
        "canceling"
    );
    let ended = wait_run(&e, RunState::Canceled).await;
    assert_eq!(ended.id, run.id);
    assert!(ended.ended_at.is_some());
    assert_eq!(read(&e, &steps_path).await["state"], "canceled");
    hidden(&e, &steps_path).await;
    assert_eq!(read(&e, &events_path).await["run"]["state"], "canceled");
    hidden(&e, &events_path).await;
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn an_approval_parks_the_run_and_always_allow_wakes_it_and_writes_the_rule() {
    let model = Arc::new(Model::default());
    model.tool("mail__send", json!({"mailbox": "own", "to": ["reader@example.com"], "subject": "The plan", "body": "Read the plan."}));
    model.reply("The plan is sent.");
    let e = model_embedder(&model).await;
    let agent = e.agent(&e.workspace, "Ada").await;
    let mailbox: AgentMailbox = serde_json::from_value(json!({
        "id": "mailbox", "workspace_id": e.workspace, "agent_id": agent.id, "connection_id": "mail-provider", "address": "ada@pagis.test", "state": "active", "outgoing_cap": 100, "allow_rules": [], "sends_today": 0, "created_at": 0,
    })).unwrap();
    e.stores.agent_mailboxes.create(&mailbox).await.unwrap();
    let c = channel(&e, std::slice::from_ref(&agent.id)).await;
    post(&e, &c, "mail", "@Ada Send the plan.", None).await;
    let run = wait_run(&e, RunState::WaitingForApproval).await;
    assert!(model.calls.lock().unwrap().is_empty());
    let requests = read(&e, "/api/v1/requests").await;
    assert_eq!(requests["items"].as_array().unwrap().len(), 1);
    let approval = &requests["items"][0];
    assert_eq!(approval["kind"], "tool_action");
    assert_eq!(approval["state"], "pending");
    assert_eq!(approval["run_id"], run.id.as_str());
    assert_eq!(
        approval["payload"]["proposed_rules"],
        json!(["example.com"])
    );
    empty(&e, "/api/v1/requests", "items").await;
    let id = approval["id"].as_str().unwrap();
    let path = format!("/api/v1/requests/{id}");
    assert_eq!(read(&e, &path).await["id"], id);
    hidden(&e, &path).await;
    let queue = read(&e, "/api/v1/needs-you").await;
    assert_eq!(queue["count"], 1);
    assert_eq!(queue["items"][0]["kind"], "approval");
    assert_eq!(queue["items"][0]["request_id"], id);
    assert_eq!(queue["items"][0]["agent_id"], agent.id.as_str());
    empty(&e, "/api/v1/needs-you", "items").await;
    let decision = json!({"decision": "approved", "scope": "always"});
    json_response(
        &e,
        "POST",
        &format!("{path}/decision"),
        decision.clone(),
        &e.other_cookie,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert!(model.calls.lock().unwrap().is_empty());
    assert_eq!(
        json_response(
            &e,
            "POST",
            &format!("{path}/decision"),
            decision,
            &e.cookie,
            StatusCode::OK
        )
        .await["state"],
        "approved"
    );
    assert_eq!(wait_run(&e, RunState::Completed).await.id, run.id);
    let calls = model.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].workspace_id, e.workspace);
    assert_eq!(calls[0].run_id, run.id);
    assert_eq!(calls[0].arguments["to"], json!(["reader@example.com"]));
    let mailbox = e
        .stores
        .agent_mailboxes
        .get(&e.workspace, &mailbox.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mailbox.allow_rules, ["example.com"]);
    assert!(
        e.stores
            .agent_mailboxes
            .get(&e.other_workspace, &mailbox.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(read(&e, &path).await["state"], "approved");
    hidden(&e, &path).await;
    assert_eq!(read(&e, "/api/v1/needs-you").await["count"], 0);
    empty(&e, "/api/v1/needs-you", "items").await;
    let timeline = read(&e, &format!("/api/v1/channels/{c}/messages")).await;
    hidden(&e, &format!("/api/v1/channels/{c}/messages")).await;
    let reply = timeline["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["text_content"] == "The plan is sent.")
        .unwrap();
    let speech = format!(
        "/api/v1/channels/{c}/messages/{}/speech?block=0",
        reply["id"].as_str().unwrap()
    );
    let (status, bytes) = request(&e, "GET", &speech, Value::Null, &e.cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .contains("The plan is sent.")
    );
    hidden(&e, &speech).await;
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn memory_feed_pages_file_and_forget_preview_read_the_workspace_store() {
    let e = Embedder::new().await;
    let agent = e.agent(&e.workspace, "Ada").await;
    let grant = Grant {
        id: GrantId::generate(),
        workspace_id: e.workspace.clone(),
        agent_id: agent.id.clone(),
        resource_kind: Grant::CONNECTION_KIND.into(),
        resource_id: Some("source".into()),
        scope: json!({"capabilities": ["mail"]}),
        revision: 1,
        created_at: 0,
        revoked_at: None,
    };
    e.stores.grants.create(&grant).await.unwrap();
    let content = "---\ntitle: Read a plan\nkind: Concept\n---\n# Read a plan\n\nRead each step.\n";
    let path = ScopedPath {
        scope: MemoryScope::Shared,
        rel: "plan.md".into(),
    };
    let mut changes = MemoryChangeset::default();
    changes.writes.insert(path.clone(), content.into());
    let sha = e
        .routers
        .state
        .memory
        .commit(
            &e.workspace,
            &agent.id,
            &MemoryAccess::agent(
                agent.id.clone(),
                [MemoryExposure {
                    grant_id: grant.id,
                    revision: grant.revision,
                }],
            ),
            &MemoryAuthor {
                name: "Ada".into(),
                email: format!("{}@agents.pagis.local", agent.id),
            },
            &changes,
            None,
            "Read a plan",
            None,
        )
        .await
        .unwrap();
    e.routers.state.bus.publish(NewEvent {
        workspace_id: e.workspace.clone(), event_type: "memory.committed".into(), agent_id: Some(agent.id.clone()), run_id: None, channel_id: None,
        payload: json!({"sha": sha, "message": "Read a plan", "scopes": ["shared"], "files": ["shared/plan.md"], "titles": ["Read a plan"]}),
    }).await.unwrap();
    let feed = read(&e, "/api/v1/memory/feed").await;
    assert_eq!(feed["revision"], sha);
    assert_eq!(feed["items"].as_array().unwrap().len(), 1);
    assert_eq!(feed["items"][0]["sha"], sha);
    assert_eq!(feed["items"][0]["files"], json!(["shared/plan.md"]));
    empty(&e, "/api/v1/memory/feed", "items").await;
    let pages = read(&e, "/api/v1/memory/pages?scope=shared").await;
    assert_eq!(pages["total"], 1);
    assert_eq!(pages["pages"][0]["title"], "Read a plan");
    assert_eq!(pages["pages"][0]["path"], "plan.md");
    empty(&e, "/api/v1/memory/pages?scope=shared", "pages").await;
    let file = "/api/v1/memory/file?scope=shared&path=plan.md";
    assert_eq!(read(&e, file).await["content"], content);
    hidden(&e, file).await;
    let target = json!({"target": {"kind": "account", "connection_id": "source"}});
    let preview = json_response(
        &e,
        "POST",
        "/api/v1/knowledge/forget/preview",
        target.clone(),
        &e.cookie,
        StatusCode::OK,
    )
    .await;
    assert_eq!(preview["memory_revision"], sha);
    assert_eq!(preview["memory_paths"], 1);
    assert_eq!(preview["memory_revisions"], 1);
    let other = json_response(
        &e,
        "POST",
        "/api/v1/knowledge/forget/preview",
        target,
        &e.other_cookie,
        StatusCode::OK,
    )
    .await;
    assert_eq!(other["memory_revision"], Value::Null);
    assert_eq!(other["memory_paths"], 0);
}

async fn socket(
    e: &Embedder,
    cookie: &str,
) -> (MemorySocket, futures::channel::oneshot::Receiver<()>) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("cookie", cookie.parse().unwrap());
    let tenant = pagis_server::resolve_session(&e.routers.state, &headers)
        .await
        .unwrap()
        .unwrap();
    let session_ended = e.routers.state.live_connections.bind(&tenant);
    let live = pagis_server::LiveTenant {
        tenant,
        session_ended,
    };
    let (server, client) = message_socket_pair();
    let state = e.routers.state.clone();
    let (done, task) = futures::channel::oneshot::channel();
    pagis_rt::spawn(async move {
        pagis_server::ws::serve_events(server, state, live)
            .await
            .unwrap();
        done.send(()).unwrap();
    });
    (client, task)
}

async fn send(socket: &mut MemorySocket, frame: Value) {
    socket.send(Frame::Text(frame.to_string())).await.unwrap();
}

async fn next(socket: &mut MemorySocket) -> Frame {
    pagis_rt::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

async fn frame(socket: &mut MemorySocket) -> Value {
    let Frame::Text(text) = next(socket).await else {
        panic!("the frame is text")
    };
    serde_json::from_str(&text).unwrap()
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn live_events_check_the_first_frame_channel_subscription_and_session_sign_out() {
    let e = Embedder::new().await;
    let c = channel(&e, &[]).await;
    let (mut invalid, invalid_task) = socket(&e, &e.cookie).await;
    send(&mut invalid, json!({"type": "subscribe", "channel_id": c})).await;
    assert_eq!(frame(&mut invalid).await["payload"]["code"], "unauthorized");
    invalid_task.await.unwrap();
    let (mut own, own_task) = socket(&e, &e.cookie).await;
    let (mut other, other_task) = socket(&e, &e.other_cookie).await;
    for socket in [&mut own, &mut other] {
        send(socket, json!({"type": "auth"})).await;
        assert_eq!(frame(socket).await["type"], "ready");
        send(socket, json!({"type": "subscribe", "channel_id": c})).await;
        send(socket, json!({"type": "ping"})).await;
    }
    while frame(&mut own).await["type"] != "pong" {}
    let refused = frame(&mut other).await;
    assert_eq!(refused["type"], "error");
    assert_eq!(refused["payload"]["code"], "not_found");
    assert_eq!(frame(&mut other).await["type"], "pong");
    let message = post(&e, &c, "live", "Read this event.", None).await;
    loop {
        let event = frame(&mut own).await;
        if event["type"] == "message.completed" {
            assert_eq!(event["payload"]["channel_id"], c);
            assert_eq!(event["payload"]["payload"]["message_id"], message["id"]);
            break;
        }
    }
    let stream = MessageId::generate();
    e.routers.state.hub.begin(
        ChannelId::from(c.clone()),
        stream.clone(),
        RunId::generate(),
        AgentId::generate(),
        None,
    );
    e.routers.state.hub.push(&stream, "Live words");
    let delta = frame(&mut own).await;
    assert_eq!(delta["type"], "message.delta");
    assert_eq!(delta["payload"]["channel_id"], c);
    assert_eq!(delta["payload"]["text"], "Live words");
    e.routers.state.hub.end(&stream);
    assert!(
        pagis_rt::timeout(Duration::from_millis(50), other.next())
            .await
            .is_err()
    );
    send(&mut other, json!({"type": "ping"})).await;
    assert_eq!(frame(&mut other).await["type"], "pong");
    let (status, _) = request(
        &e,
        "DELETE",
        "/api/v1/sessions/current",
        Value::Null,
        &e.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let close = next(&mut own).await;
    assert!(matches!(&close, Frame::Close(Some(close)) if close.code == 1008));
    own.send(Frame::Close(None)).await.unwrap();
    own_task.await.unwrap();
    send(&mut other, json!({"type": "ping"})).await;
    assert_eq!(frame(&mut other).await["type"], "pong");
    other.send(Frame::Close(None)).await.unwrap();
    other_task.await.unwrap();
}
