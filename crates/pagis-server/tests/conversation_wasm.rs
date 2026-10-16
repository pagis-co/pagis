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
        created_by: pagis_core::Author::System,
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
    let found = read(&e, "/api/v1/search?q=plan").await;
    assert_eq!(found["items"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["items"][0]["kind"], "memory");
    assert_eq!(found["items"][0]["title"], "Read a plan");
    let found = json_response(
        &e,
        "GET",
        "/api/v1/search?q=plan",
        Value::Null,
        &e.other_cookie,
        StatusCode::OK,
    )
    .await;
    assert_eq!(found["items"], json!([]));
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

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn workspace_conversations_share_one_history_and_check_participation() {
    let e = Embedder::new().await;
    let state = &e.routers.state;
    let a = state.workspaces.get(&e.workspace).await.unwrap().unwrap();
    let b = state
        .workspaces
        .get(&e.other_workspace)
        .await
        .unwrap()
        .unwrap();
    state
        .users
        .set_name(a.user_id.as_ref().unwrap(), "Alice", now_ms())
        .await
        .unwrap();
    state
        .users
        .set_name(b.user_id.as_ref().unwrap(), "Bob", now_ms())
        .await
        .unwrap();
    let colleagues = read(&e, "/api/v1/colleagues").await;
    assert_eq!(colleagues["items"].as_array().unwrap().len(), 1);
    assert_eq!(colleagues["items"][0]["name"], "Bob");
    assert_eq!(colleagues["items"][0]["presence"], "away");
    state
        .person_activity
        .record(b.user_id.as_ref().unwrap(), state.clock.now_ms());
    assert_eq!(
        read(&e, "/api/v1/colleagues").await["items"][0]["presence"],
        "active"
    );
    let conversation = json_response(
        &e,
        "POST",
        "/api/v1/conversations",
        json!({"workspace_ids": [e.other_workspace]}),
        &e.cookie,
        StatusCode::OK,
    )
    .await;
    let again = json_response(
        &e,
        "POST",
        "/api/v1/conversations",
        json!({"workspace_ids": [e.workspace]}),
        &e.other_cookie,
        StatusCode::OK,
    )
    .await;
    assert_eq!(conversation["id"], again["id"]);
    let id = conversation["id"].as_str().unwrap();
    let path = format!("/api/v1/conversations/{id}/messages");
    let mut events = state
        .bus
        .subscribe(EventScope::workspace(&e.other_workspace), None)
        .await;
    let first = json_response(
        &e,
        "POST",
        &path,
        json!({"pending_id":"a", "text":"Hello Bob"}),
        &e.cookie,
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(first["author"]["name"], "Alice");
    assert_eq!(
        events.next().await.unwrap().event_type,
        "conversation.message"
    );
    let duplicate = json_response(
        &e,
        "POST",
        &path,
        json!({"pending_id":"a", "text":"Hello Bob"}),
        &e.cookie,
        StatusCode::OK,
    )
    .await;
    assert_eq!(first["id"], duplicate["id"]);
    json_response(
        &e,
        "POST",
        &path,
        json!({"pending_id":"b", "text":"Hello Alice"}),
        &e.other_cookie,
        StatusCode::CREATED,
    )
    .await;
    let page = read(&e, &format!("{path}?limit=1")).await;
    assert_eq!(page["items"][0]["author"]["name"], "Bob");
    let before = page["next_cursor"].as_str().unwrap();
    let older = read(&e, &format!("{path}?limit=1&before={before}")).await;
    assert_eq!(older["items"][0]["author"]["name"], "Alice");
    assert!(older["next_cursor"].is_null());
    let list = read(&e, "/api/v1/conversations").await;
    assert_eq!(
        list["items"][0]["last_message"]["text_content"],
        "Hello Alice"
    );
    let channel_id = ChannelId::from(id.to_string());
    assert_eq!(
        state
            .messages
            .list_top_level(&state.org_workspace_id, &channel_id, None, 10)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        state
            .messages
            .list_top_level(&e.other_workspace, &channel_id, None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    // A new group excludes B and keeps B's read and post outside it.
    let org = state
        .users
        .get(a.user_id.as_ref().unwrap())
        .await
        .unwrap()
        .unwrap()
        .org_id;
    let c = User::new(org, UserRole::Member, now_ms());
    state.users.create(&c).await.unwrap();
    let c_workspace = Workspace {
        id: WorkspaceId::generate(),
        user_id: Some(c.id),
        ..a
    };
    state.workspaces.create(&c_workspace).await.unwrap();
    let separate = json_response(
        &e,
        "POST",
        "/api/v1/conversations",
        json!({"workspace_ids": [c_workspace.id]}),
        &e.cookie,
        StatusCode::OK,
    )
    .await;
    let hidden_path = format!(
        "/api/v1/conversations/{}/messages",
        separate["id"].as_str().unwrap()
    );
    for method in ["GET", "POST"] {
        json_response(
            &e,
            method,
            &hidden_path,
            json!({"pending_id":"no", "text":"No"}),
            &e.other_cookie,
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    let group = json_response(
        &e,
        "POST",
        "/api/v1/conversations",
        json!({"workspace_ids": [c_workspace.id, e.other_workspace]}),
        &e.cookie,
        StatusCode::OK,
    )
    .await;
    assert_eq!(group["participants"].as_array().unwrap().len(), 3);
    assert_ne!(group["id"], conversation["id"]);
    json_response(
        &e,
        "POST",
        "/api/v1/conversations",
        json!({"workspace_ids": [state.org_workspace_id]}),
        &e.cookie,
        StatusCode::NOT_FOUND,
    )
    .await;
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn workspace_conversations_sort_by_message_time_then_id_and_empty_creation_time() {
    let e = Embedder::new().await;
    let state = &e.routers.state;
    let owner = &state.org_workspace_id;
    let person = state
        .workspaces
        .get(&e.workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id;
    // Explicit ids keep their order different from their creation times.
    for (channel_id, created_at, message) in [
        ("empty-new", 500, None),
        ("new-message", 1, Some(("a", 400))),
        ("tie-high", 1, Some(("z", 300))),
        ("tie-low", 1, Some(("b", 300))),
        ("old-message", 1, Some(("zz", 200))),
        ("empty-old", 100, None),
    ] {
        let channel_id = ChannelId::from(channel_id.to_string());
        state
            .channels
            .create(&Channel {
                id: channel_id.clone(),
                workspace_id: owner.clone(),
                kind: ChannelKind::Conversation,
                title: None,
                created_at,
                updated_at: created_at,
            })
            .await
            .unwrap();
        state
            .participants
            .create(&ChannelParticipant {
                id: ParticipantId::generate(),
                workspace_id: owner.clone(),
                channel_id: channel_id.clone(),
                kind: ParticipantKind::Workspace,
                participant_workspace_id: Some(e.workspace.clone()),
                person_id: None,
                agent_id: None,
                joined_at: created_at,
            })
            .await
            .unwrap();
        if let Some((id, at)) = message {
            state
                .messages
                .insert(&Message {
                    id: MessageId::from(id.to_string()),
                    workspace_id: owner.clone(),
                    channel_id,
                    parent_message_id: None,
                    author: Author::Person(person.clone().unwrap()),
                    run_id: None,
                    status: MessageStatus::Complete,
                    blocks: vec![Block::markdown("Hello")],
                    text_content: "Hello".into(),
                    pending_id: None,
                    created_at: at,
                    completed_at: Some(at),
                })
                .await
                .unwrap();
        }
    }
    let page = read(&e, "/api/v1/conversations").await;
    let ids: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "empty-new",
            "new-message",
            "tie-high",
            "tie-low",
            "old-message",
            "empty-old"
        ]
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn free_busy_is_an_org_read_without_event_details() {
    let e = Embedder::new().await;
    let state = &e.routers.state;
    let workspace = state.workspaces.get(&e.workspace).await.unwrap().unwrap();
    state
        .users
        .set_email_and_password(
            workspace.user_id.as_ref().unwrap(),
            "alice@example.com",
            "hash",
            now_ms(),
        )
        .await
        .unwrap();
    let body = json!({"title":"Private title", "description":"Private description", "location":"Private room",
        "start":"2026-03-08T09:00:00", "end":"2026-03-08T10:00:00", "timezone":"UTC", "all_day":false,
        "rrule":null, "exdates":[], "attendees":[], "reminders":[], "meeting_url":null});
    let event = json_response(
        &e,
        "POST",
        "/api/v1/calendar/events",
        body,
        &e.cookie,
        StatusCode::CREATED,
    )
    .await;
    let query = json!({"from":"2026-03-08T00:00:00Z", "to":"2026-03-09T00:00:00Z", "addresses":["ALICE@example.com", "outside@example.com"]});
    let answer = json_response(
        &e,
        "POST",
        "/api/v1/free-busy",
        query.clone(),
        &e.other_cookie,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        answer,
        json!({"items":[{"address":"ALICE@example.com", "busy":[{"start":"2026-03-08T09:00:00Z", "end":"2026-03-08T10:00:00Z"}]}]})
    );
    hidden(
        &e,
        &format!("/api/v1/calendar/events/{}", event["id"].as_str().unwrap()),
    )
    .await;
    for (from, to) in [
        ("bad", "2026-03-09T00:00:00Z"),
        ("2026-03-09T00:00:00Z", "2026-03-08T00:00:00Z"),
        ("2026-03-08T00:00:00Z", "2026-05-10T00:00:00Z"),
    ] {
        let mut invalid = query.clone();
        invalid["from"] = json!(from);
        invalid["to"] = json!(to);
        json_response(
            &e,
            "POST",
            "/api/v1/free-busy",
            invalid,
            &e.other_cookie,
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    let mut invalid = query.clone();
    invalid["addresses"] = json!(vec!["alice@example.com"; 51]);
    json_response(
        &e,
        "POST",
        "/api/v1/free-busy",
        invalid,
        &e.other_cookie,
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
    assert_eq!(
        request(&e, "POST", "/api/v1/free-busy", query, "").await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn colleague_meeting_presence_follows_the_clock_and_wins_over_active() {
    let e = Embedder::new().await;
    let start = CalendarRange::parse("2026-03-08T09:00:00Z", "2026-03-08T10:00:00Z")
        .unwrap()
        .from();
    let body = json!({"title":"Private", "description":"", "location":"",
        "start":"2026-03-08T09:00:00", "end":"2026-03-08T10:00:00", "timezone":"UTC", "all_day":false,
        "rrule":null, "exdates":[], "attendees":[], "reminders":[], "meeting_url":null});
    json_response(
        &e,
        "POST",
        "/api/v1/calendar/events",
        body,
        &e.other_cookie,
        StatusCode::CREATED,
    )
    .await;
    for (at, expected) in [
        (start - 1, "away"),
        (start, "in_a_meeting"),
        (start + 3_599_999, "in_a_meeting"),
        (start + 3_600_000, "away"),
    ] {
        e.clock.set(at);
        assert_eq!(
            read(&e, "/api/v1/colleagues").await["items"][0]["presence"],
            expected
        );
    }
    e.clock.set(start);
    let other = e
        .routers
        .state
        .workspaces
        .get(&e.other_workspace)
        .await
        .unwrap()
        .unwrap();
    e.routers
        .state
        .person_activity
        .record(other.user_id.as_ref().unwrap(), start);
    assert_eq!(
        read(&e, "/api/v1/colleagues").await["items"][0]["presence"],
        "in_a_meeting"
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn free_busy_resolves_private_and_shared_workspace_mailboxes() {
    let e = Embedder::new().await;
    let stores = &e.stores;
    let mut shared = stores.workspaces.get(&e.workspace).await.unwrap().unwrap();
    shared.id = WorkspaceId::generate();
    shared.kind = WorkspaceKind::Shared;
    shared.user_id = None;
    stores.workspaces.create(&shared).await.unwrap();
    for (workspace, address) in [
        (&e.workspace, "private@example.com"),
        (&shared.id, "team@example.com"),
    ] {
        stores
            .workspace_mailboxes
            .create(&WorkspaceMailbox {
                workspace_id: workspace.clone(),
                connection_id: ConnectionId::generate(),
                address: address.into(),
                outgoing_cap: 100,
                spam_mode: MailScoringMode::ScoreOnly,
                created_at: now_ms(),
            })
            .await
            .unwrap();
        let event = CalendarEvent::new(workspace.clone(), Author::System, serde_json::from_value(json!({
            "title":"Private", "description":"", "location":"", "start":"2026-03-08T09:00:00", "end":"2026-03-08T10:00:00",
            "timezone":"UTC", "all_day":false, "rrule":null, "exdates":[], "attendees":[], "reminders":[], "meeting_url":null,
        })).unwrap());
        stores.calendar.create(&event).await.unwrap();
    }
    let outside = User::new(OrgId::generate(), UserRole::Member, now_ms());
    stores.users.create(&outside).await.unwrap();
    stores
        .users
        .set_email_and_password(&outside.id, "other-org@example.com", "hash", now_ms())
        .await
        .unwrap();
    let mut outside_workspace = shared.clone();
    outside_workspace.id = WorkspaceId::generate();
    outside_workspace.kind = WorkspaceKind::Private;
    outside_workspace.user_id = Some(outside.id);
    stores.workspaces.create(&outside_workspace).await.unwrap();
    stores
        .workspace_mailboxes
        .create(&WorkspaceMailbox {
            workspace_id: outside_workspace.id,
            connection_id: ConnectionId::generate(),
            address: "other-mailbox@example.com".into(),
            outgoing_cap: 100,
            spam_mode: MailScoringMode::ScoreOnly,
            created_at: now_ms(),
        })
        .await
        .unwrap();
    let answer = json_response(&e, "POST", "/api/v1/free-busy", json!({
        "from":"2026-03-08T00:00:00Z", "to":"2026-03-09T00:00:00Z", "addresses":["private@example.com", "team@example.com", "other-org@example.com", "other-mailbox@example.com"],
    }), &e.other_cookie, StatusCode::OK).await;
    assert_eq!(answer["items"].as_array().unwrap().len(), 2);
    for item in answer["items"].as_array().unwrap() {
        assert_eq!(
            item["busy"],
            json!([{"start":"2026-03-08T09:00:00Z", "end":"2026-03-08T10:00:00Z"}])
        );
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn search_checks_each_workspace_and_removed_membership() {
    let e = Embedder::new().await;
    let owner = e
        .stores
        .workspaces
        .get(&e.workspace)
        .await
        .unwrap()
        .unwrap();
    let reader = e
        .stores
        .workspaces
        .get(&e.other_workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id
        .unwrap();
    let mut shared = owner.clone();
    shared.id = WorkspaceId::generate();
    shared.name = "Support".into();
    shared.kind = WorkspaceKind::Shared;
    shared.user_id = None;
    e.stores
        .workspace_members
        .create_workspace(
            &shared,
            &WorkspaceMember {
                workspace_id: shared.id.clone(),
                person_id: owner.user_id.unwrap(),
                role: WorkspaceRole::Manager,
                author: Author::System,
            },
        )
        .await
        .unwrap();
    e.stores
        .workspace_members
        .add(&WorkspaceMember {
            workspace_id: shared.id.clone(),
            person_id: reader.clone(),
            role: WorkspaceRole::Reader,
            author: Author::System,
        })
        .await
        .unwrap();
    for (workspace, title) in [
        (&e.workspace, "Launch secret"),
        (&shared.id, "Launch review"),
    ] {
        e.stores.calendar.create(&CalendarEvent::new(workspace.clone(), Author::System, serde_json::from_value(json!({
            "title":title, "description":"Plan", "location":"Office", "start":"2026-03-08T09:00:00", "end":"2026-03-08T10:00:00",
            "timezone":"UTC", "all_day":false, "rrule":null, "exdates":[], "attendees":[], "reminders":[], "meeting_url":null,
        })).unwrap())).await.unwrap();
    }
    let found = json_response(
        &e,
        "GET",
        "/api/v1/search?q=launch",
        Value::Null,
        &e.other_cookie,
        StatusCode::OK,
    )
    .await;
    assert_eq!(found["items"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["items"][0]["workspace"]["name"], "Support");
    assert_eq!(found["items"][0]["title"], "Launch review");
    assert_eq!(found["incomplete"], json!([]));
    e.stores
        .workspace_members
        .remove(&shared.id, &reader)
        .await
        .unwrap();
    let found = json_response(
        &e,
        "GET",
        "/api/v1/search?q=launch",
        Value::Null,
        &e.other_cookie,
        StatusCode::OK,
    )
    .await;
    assert_eq!(found["items"], json!([]));
    for query in ["q=x", "q=launch&limit=0", "q=launch&limit=101"] {
        json_response(
            &e,
            "GET",
            &format!("/api/v1/search?{query}"),
            Value::Null,
            &e.cookie,
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn search_reads_channel_threads_and_checks_conversation_participation() {
    let e = Embedder::new().await;
    let c = channel(&e, &[]).await;
    let root = post(&e, &c, "root", "Searchable channel root", None).await;
    // The reply is newer than its root, so the newest-first order is fixed.
    e.clock.set(e.clock.now_ms() + 1_000);
    post(
        &e,
        &c,
        "reply",
        "Searchable thread reply",
        root["id"].as_str(),
    )
    .await;
    let conversation = json_response(
        &e,
        "POST",
        "/api/v1/conversations",
        json!({"workspace_ids":[e.other_workspace]}),
        &e.cookie,
        StatusCode::OK,
    )
    .await;
    let conversation_id = conversation["id"].as_str().unwrap();
    json_response(
        &e,
        "POST",
        &format!("/api/v1/conversations/{conversation_id}/messages"),
        json!({"pending_id":"conversation", "text":"Searchable conversation"}),
        &e.cookie,
        StatusCode::CREATED,
    )
    .await;
    let third = WorkspaceId::generate();
    let mut private = e
        .stores
        .workspaces
        .get(&e.workspace)
        .await
        .unwrap()
        .unwrap();
    private.id = third.clone();
    let owner = e
        .stores
        .users
        .get(private.user_id.as_ref().unwrap())
        .await
        .unwrap()
        .unwrap();
    let person = User::new(owner.org_id, UserRole::Member, now_ms());
    e.stores.users.create(&person).await.unwrap();
    private.user_id = Some(person.id);
    e.stores.workspaces.create(&private).await.unwrap();
    let org = &e.routers.state.org_workspace_id;
    let hidden = e
        .stores
        .channels
        .get_or_create_conversation(org, &[e.other_workspace.clone(), third])
        .await
        .unwrap();
    for at in 1..=3 {
        e.stores
            .messages
            .insert(&Message {
                id: MessageId::generate(),
                workspace_id: org.clone(),
                channel_id: hidden.id.clone(),
                parent_message_id: None,
                author: Author::Person(private.user_id.clone().unwrap()),
                run_id: None,
                status: MessageStatus::Complete,
                blocks: vec![Block::markdown("Searchable conversation hidden")],
                text_content: "Searchable conversation hidden".into(),
                pending_id: None,
                created_at: now_ms() + at * 1000,
                completed_at: Some(now_ms() + at * 1000),
            })
            .await
            .unwrap();
    }
    let found = read(&e, "/api/v1/search?q=searchable").await;
    let items = found["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{found}");
    assert!(
        items
            .iter()
            .any(|hit| hit["snippet"] == "Searchable thread reply")
    );
    assert!(items.iter().any(|hit| {
        hit["url"]
            .as_str()
            .unwrap()
            .starts_with(&format!("/conversations/{conversation_id}?"))
    }));
    assert!(
        !items
            .iter()
            .any(|hit| hit["snippet"] == "Searchable conversation hidden")
    );
    let conversation_only = read(&e, "/api/v1/search?q=conversation&limit=1").await;
    assert_eq!(conversation_only["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        conversation_only["items"][0]["snippet"],
        "Searchable conversation"
    );
    let found = read(&e, "/api/v1/search?q=searchable&limit=1").await;
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
    assert_eq!(found["items"][0], items[0]);
    for count in [2, 200] {
        let query = "é".repeat(count);
        json_response(
            &e,
            "GET",
            &format!(
                "/api/v1/search?q={}",
                url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
            ),
            Value::Null,
            &e.cookie,
            StatusCode::OK,
        )
        .await;
    }
    let query = "x".repeat(201);
    json_response(
        &e,
        "GET",
        &format!("/api/v1/search?q={query}"),
        Value::Null,
        &e.cookie,
        StatusCode::UNPROCESSABLE_ENTITY,
    )
    .await;
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn call_records_are_portable_and_workspace_scoped() {
    let e = Embedder::new().await;
    let call = Call {
        id: CallId::generate(),
        workspace_id: e.workspace.clone(),
        agent_id: AgentId::generate(),
        run_id: RunId::generate(),
        phone_number_id: PhoneNumberId::generate(),
        direction: CallDirection::Inbound,
        remote_e164: "+14155550123".into(),
        agent_name: "Agent".into(),
        own_e164: "+14155550124".into(),
        purpose: "Take a message".into(),
        tools: vec![],
        tier: TrustTier::Unknown,
        state: CallState::Ended,
        outcome: Some(CallOutcome::NoAnswer),
        ended_reason: Some("remote_hangup".into()),
        classification: None,
        message_left: false,
        transcript: vec![TranscriptLine {
            at: 12,
            speaker: Speaker::Caller,
            text: "Hello".into(),
        }],
        recording_file_id: None,
        created_at: 10,
        ringing_at: None,
        answered_at: None,
        ended_at: Some(20),
        dismissed_at: None,
    };
    e.stores.calls.insert(&call).await.unwrap();
    let path = format!("/api/v1/calls/{}", call.id);
    let page = read(&e, "/api/v1/calls?direction=inbound&state=ended&limit=1").await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["id"], call.id.as_str());
    assert_eq!(page["items"][0]["dismissed_at"], Value::Null);
    assert!(page["items"][0].get("transcript").is_none());
    let detail = read(&e, &path).await;
    assert_eq!(detail["run_id"], call.run_id.as_str());
    assert_eq!(detail["transcript"][0]["text"], "Hello");
    empty(&e, "/api/v1/calls", "items").await;
    hidden(&e, &path).await;
    for (cookie, id, status) in [
        (&e.other_cookie, call.id.as_str(), StatusCode::NOT_FOUND),
        (&e.cookie, "absent", StatusCode::NOT_FOUND),
        (&String::new(), call.id.as_str(), StatusCode::UNAUTHORIZED),
    ] {
        assert_eq!(
            request(
                &e,
                "POST",
                &format!("/api/v1/calls/{id}/dismiss"),
                Value::Null,
                cookie
            )
            .await
            .0,
            status
        );
    }
    let mut events = e
        .routers
        .state
        .bus
        .subscribe(EventScope::Workspace(e.workspace.clone()), None)
        .await;
    for at in [100, 200] {
        e.clock.set(at);
        assert_eq!(
            request(
                &e,
                "POST",
                &format!("{path}/dismiss"),
                Value::Null,
                &e.cookie
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            read(&e, "/api/v1/calls").await["items"][0]["dismissed_at"],
            100
        );
    }
    let event = events.next().await.unwrap();
    assert_eq!(event.event_type, "call.dismissed");
    assert_eq!(event.payload["call_id"], call.id.as_str());
}

struct FakeLiveCallControl {
    workspace: WorkspaceId,
    call: CallId,
    hub: Arc<pagis_telephony::hub::MediaHub>,
    operations: std::sync::Mutex<Vec<&'static str>>,
}

#[async_trait::async_trait]
impl pagis_server::calls::LiveCallControl for FakeLiveCallControl {
    fn hub(
        &self,
        workspace: &WorkspaceId,
        call: &CallId,
    ) -> Option<Arc<pagis_telephony::hub::MediaHub>> {
        self.operations.lock().unwrap().push("listen");
        (workspace == &self.workspace && call == &self.call).then(|| self.hub.clone())
    }

    async fn hang_up(&self, workspace: &WorkspaceId, call: &CallId) -> bool {
        self.operations.lock().unwrap().push("hangup");
        workspace == &self.workspace && call == &self.call
    }

    async fn drop_to_unknown(
        &self,
        workspace: &WorkspaceId,
        call: &CallId,
    ) -> Option<Result<(), StoreError>> {
        self.operations.lock().unwrap().push("tier");
        (workspace == &self.workspace && call == &self.call).then_some(Ok(()))
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn live_call_handlers_use_supplied_control() {
    let mut e = Embedder::new().await;
    let call = CallId::generate();
    let (leg, _carrier) =
        pagis_telephony::leg::FrameLeg::pair(pagis_telephony::audio::Codec::Pcmu, 8);
    let control = Arc::new(FakeLiveCallControl {
        workspace: e.workspace.clone(),
        call: call.clone(),
        hub: pagis_telephony::hub::MediaHub::start(leg),
        operations: Default::default(),
    });
    let pagis_server::Routers {
        state,
        product,
        administration,
    } = e.routers;
    drop((product, administration));
    let mut state = Arc::try_unwrap(state).ok().unwrap();
    state.live_call_control = control.clone();
    e.routers = pagis_server::routers(state, axum::Router::new());
    let path = format!("/api/v1/calls/{call}");
    assert_eq!(
        request(
            &e,
            "POST",
            &format!("{path}/hangup"),
            Value::Null,
            &e.cookie
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let tier = json_response(
        &e,
        "POST",
        &format!("{path}/tier"),
        json!({"tier": "unknown"}),
        &e.cookie,
        StatusCode::OK,
    )
    .await;
    assert_eq!(tier, json!({"call_id": call.as_str(), "tier": "unknown"}));
    assert_eq!(*control.operations.lock().unwrap(), ["hangup", "tier"]);
    assert_eq!(
        request(
            &e,
            "POST",
            &format!("{path}/tier"),
            json!({"tier": "owner"}),
            &e.cookie
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(*control.operations.lock().unwrap(), ["hangup", "tier"]);
    for action in ["hangup", "tier"] {
        assert_eq!(
            request(
                &e,
                "POST",
                &format!("{path}/{action}"),
                json!({"tier": "unknown"}),
                &e.other_cookie
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("cookie", e.cookie.parse().unwrap());
    let tenant = pagis_server::resolve_session(&e.routers.state, &headers)
        .await
        .unwrap()
        .unwrap();
    let ended = tokio_util::sync::CancellationToken::new();
    let live = pagis_server::LiveTenant {
        tenant,
        session_ended: ended.clone(),
    };
    let (server, mut client) = message_socket_pair();
    let serve =
        pagis_server::calls::serve_listen(server, e.routers.state.clone(), live, call.to_string());
    let receive = async {
        let Some(Ok(Frame::Text(frame))) = client.next().await else {
            panic!("ready frame")
        };
        assert_eq!(
            serde_json::from_str::<Value>(&frame).unwrap(),
            json!({"type": "ready", "codec": "PCMU"})
        );
        ended.cancel();
        assert!(matches!(client.next().await, Some(Ok(Frame::Close(_)))));
    };
    let (result, ()) = futures::join!(serve, receive);
    result.unwrap();
    assert_eq!(control.operations.lock().unwrap().last(), Some(&"listen"));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn absent_live_call_control_answers_as_a_call_that_is_not_live() {
    let e = Embedder::new().await;
    for action in ["hangup", "tier"] {
        assert_eq!(
            request(
                &e,
                "POST",
                &format!("/api/v1/calls/absent/{action}"),
                json!({"tier": "unknown"}),
                &e.cookie
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("cookie", e.cookie.parse().unwrap());
    let tenant = pagis_server::resolve_session(&e.routers.state, &headers)
        .await
        .unwrap()
        .unwrap();
    let live = pagis_server::LiveTenant {
        tenant,
        session_ended: tokio_util::sync::CancellationToken::new(),
    };
    let (server, mut client) = message_socket_pair();
    pagis_server::calls::serve_listen(server, e.routers.state.clone(), live, "absent".into())
        .await
        .unwrap();
    let Some(Ok(Frame::Text(frame))) = client.next().await else {
        panic!("error frame")
    };
    assert_eq!(
        serde_json::from_str::<Value>(&frame).unwrap()["code"],
        "not_found"
    );
}
