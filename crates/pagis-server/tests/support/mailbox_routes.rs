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
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
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
async fn a_provider_without_endpoints_provisions_lists_refuses_reset_and_deletes_through_the_routes()
 {
    let embedder = Embedder::new().await;
    let connection = provider(&embedder).await;
    let agent = embedder.agent(&embedder.workspace, "Ada").await;
    let path = format!("/api/v1/agents/{}/mailbox", agent.id);
    let (status, body) = response(&embedder, "GET", &path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["offers"][0]["connection_id"], connection.as_str());
    assert_eq!(body["offers"][0]["password"], "none");
    assert!(body["offers"][0].get("mints_password").is_none());
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
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"]["message"], "this mailbox has no login");
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

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn workspace_mailbox_reads_through_the_product_api_and_isolates_workspaces() {
    let embedder = Embedder::new().await;
    let connection = provider(&embedder).await;
    let org_workspace = &embedder.routers.state.org_workspace_id;
    let mut record = embedder
        .stores
        .connections
        .get(org_workspace, &connection)
        .await
        .unwrap()
        .unwrap();
    record.config["imap_host"] = json!("imap.test");
    record.config["imap_port"] = json!(993);
    record.config["smtp_host"] = json!("smtp.test");
    record.config["smtp_port"] = json!(465);
    embedder
        .stores
        .connections
        .set_config(org_workspace, &connection, &record.config)
        .await
        .unwrap();
    let (status, page) = response(&embedder, "GET", "/api/v1/workspace/mailbox", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page["mailbox"].is_null());
    let create = json!({"connection_id": connection, "local_part": "person"});
    let (status, mailbox) = response(
        &embedder,
        "POST",
        "/api/v1/workspace/mailbox",
        create.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{mailbox}");
    assert_eq!(mailbox["address"], "person@example.com");
    assert!(mailbox.get("password").is_none());
    assert_eq!(
        response(&embedder, "POST", "/api/v1/workspace/mailbox", create)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let transport = &embedder.mail_transport;
    transport.deliver("sender@example.test", "Invoice", "invoice first");
    let (status, found) = response(&embedder, "GET", "/api/v1/search?q=invoice", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found["items"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["items"][0]["kind"], "mail");
    assert_eq!(found["items"][0]["title"], "Invoice");
    assert!(
        found["items"][0]["url"]
            .as_str()
            .unwrap()
            .starts_with("/mail?message=")
    );

    let thread = transport.summaries("INBOX")[0].thread_id.clone();
    transport.deliver_reply(
        &thread,
        "sender@example.test",
        "Re: Invoice",
        "invoice second",
    );
    let (status, list) = response(
        &embedder,
        "GET",
        "/api/v1/workspace/mail/messages",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"].as_array().unwrap().len(), 2);
    assert_eq!(list["items"][0]["seen"], false);
    let id = list["items"][0]["id"].as_str().unwrap();
    let (status, opened) = response(
        &embedder,
        "GET",
        &format!("/api/v1/workspace/mail/messages/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(opened["text"], "invoice second");
    assert_eq!(opened["seen"], true);
    let thread_id: String =
        url::form_urlencoded::byte_serialize(opened["thread_id"].as_str().unwrap().as_bytes())
            .collect();
    let (status, thread) = response(
        &embedder,
        "GET",
        &format!("/api/v1/workspace/mail/threads/{}", thread_id),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(thread["items"].as_array().unwrap().len(), 2);
    let (_, found) = response(
        &embedder,
        "GET",
        "/api/v1/workspace/mail/messages?q=invoice",
        Value::Null,
    )
    .await;
    assert_eq!(found["items"].as_array().unwrap().len(), 2);
    assert_eq!(found["items"][0]["seen"], true);
    let (status, _) = super::response(
        &embedder,
        "GET",
        "/api/v1/workspace/mail/messages",
        Value::Null,
        Some(&embedder.other_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for path in [
        format!("/api/v1/workspace/mail/messages/{id}"),
        format!("/api/v1/workspace/mail/threads/{}", thread_id),
        "/api/v1/workspace/mail/folders".into(),
    ] {
        let (status, _) = super::response(
            &embedder,
            "GET",
            &path,
            Value::Null,
            Some(&embedder.other_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        let (status, _) = super::response(&embedder, "GET", &path, Value::Null, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    let (_, other) = super::response(
        &embedder,
        "GET",
        "/api/v1/workspace/mailbox",
        Value::Null,
        Some(&embedder.other_cookie),
    )
    .await;
    assert!(other["mailbox"].is_null());
    let (_, folders) = response(
        &embedder,
        "GET",
        "/api/v1/workspace/mail/folders",
        Value::Null,
    )
    .await;
    assert!(
        folders["items"]
            .as_array()
            .unwrap()
            .contains(&json!("INBOX"))
    );
    assert_eq!(transport.logins().len(), transport.logouts().len());
}

async fn workspace_provider(embedder: &Embedder) -> ConnectionId {
    let connection = provider(embedder).await;
    let org = &embedder.routers.state.org_workspace_id;
    let mut record = embedder
        .stores
        .connections
        .get(org, &connection)
        .await
        .unwrap()
        .unwrap();
    record.config["imap_host"] = json!("imap.test");
    record.config["imap_port"] = json!(993);
    record.config["smtp_host"] = json!("smtp.test");
    record.config["smtp_port"] = json!(465);
    embedder
        .stores
        .connections
        .set_config(org, &connection, &record.config)
        .await
        .unwrap();
    connection
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn workspace_mailbox_grants_move_with_designation_and_keep_manual_access() {
    let embedder = Embedder::new().await;
    let connection = workspace_provider(&embedder).await;
    let chief = embedder.agent(&embedder.workspace, "Chief").await;
    let other = embedder.agent(&embedder.workspace, "Other").await;
    assert_eq!(
        response(
            &embedder,
            "PUT",
            "/api/v1/workspace/chief-of-staff",
            json!({"agent_id":chief.id})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        response(
            &embedder,
            "POST",
            "/api/v1/workspace/mailbox",
            json!({"connection_id":connection,"local_part":"person"})
        )
        .await
        .0,
        201
    );
    let (_, page) = response(&embedder, "GET", "/api/v1/grants", Value::Null).await;
    let original = &page["items"][0];
    assert_eq!(original["resource_kind"], "workspace_mailbox");
    assert_eq!(original["agent_id"], chief.id.as_str());
    assert_eq!(original["is_default"], true);
    let (status, manual) = response(
        &embedder,
        "POST",
        "/api/v1/grants/workspace-mailbox",
        json!({"agent_id":other.id}),
    )
    .await;
    assert_eq!(status, 201, "{manual}");
    assert_eq!(manual["is_default"], false);
    let path = format!("/api/v1/grants/{}/rules", manual["id"].as_str().unwrap());
    let (status, changed) = response(
        &embedder,
        "PUT",
        &path,
        json!({"allow":[" Mail.Example.COM "]}),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["allow"], json!(["example.com"]));
    assert_eq!(
        response(
            &embedder,
            "PUT",
            &path,
            json!({"allow":["https://example.com"]})
        )
        .await
        .0,
        422
    );
    assert_eq!(
        response(
            &embedder,
            "PUT",
            "/api/v1/workspace/chief-of-staff",
            json!({"agent_id":other.id})
        )
        .await
        .0,
        200
    );
    let (_, page) = response(&embedder, "GET", "/api/v1/grants", Value::Null).await;
    let grants = page["items"].as_array().unwrap();
    assert_eq!(grants.len(), 2);
    assert!(grants.iter().all(|g| g["agent_id"] == other.id.as_str()));
    assert!(grants.iter().any(|g| g["id"] == manual["id"]));
    assert_eq!(
        response(
            &embedder,
            "POST",
            &format!("/api/v1/agents/{}/archive", other.id),
            json!({})
        )
        .await
        .0,
        200
    );
    let (_, page) = response(&embedder, "GET", "/api/v1/grants", Value::Null).await;
    let default = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["is_default"] == true)
        .unwrap();
    assert_eq!(default["agent_id"], chief.id.as_str());
    let foreign = embedder.agent(&embedder.other_workspace, "Foreign").await;
    assert_eq!(
        response(
            &embedder,
            "POST",
            "/api/v1/grants/workspace-mailbox",
            json!({"agent_id":foreign.id})
        )
        .await
        .0,
        404
    );
    assert_eq!(
        response(
            &embedder,
            "POST",
            "/api/v1/grants/workspace-mailbox",
            json!({"agent_id":other.id})
        )
        .await
        .0,
        404
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn workspace_mailbox_shared_grants_need_a_manager_and_always_allow_edits_the_same_grant() {
    let embedder = Embedder::new().await;
    let connection = workspace_provider(&embedder).await;
    let person = embedder
        .stores
        .workspaces
        .get(&embedder.workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id
        .unwrap();
    let responder = embedder
        .stores
        .workspaces
        .get(&embedder.other_workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id
        .unwrap();
    let member = WorkspaceMember {
        workspace_id: WorkspaceId::generate(),
        person_id: person.clone(),
        role: WorkspaceRole::Manager,
        author: Author::System,
    };
    let shared = pagis_server::provisioning::WorkspaceSeed::from(&embedder.stores)
        .shared(
            &member,
            "Team",
            "UTC",
            now_ms(),
            embedder.stores.workspace_members.as_ref(),
        )
        .await
        .unwrap();
    embedder
        .stores
        .workspace_members
        .add(&WorkspaceMember {
            workspace_id: shared.id.clone(),
            person_id: responder,
            role: WorkspaceRole::Responder,
            author: Author::System,
        })
        .await
        .unwrap();
    let prefix = format!("/api/v1/workspaces/{}", shared.id);
    let (status, mailbox) = response(
        &embedder,
        "POST",
        &format!("{prefix}/workspace/mailbox"),
        json!({"connection_id":connection,"local_part":"team"}),
    )
    .await;
    assert_eq!(status, 201, "{mailbox}");
    let chief = shared.chief_of_staff_agent_id.unwrap();
    let grant = embedder
        .stores
        .grants
        .list_live(&shared.id)
        .await
        .unwrap()
        .into_iter()
        .find(|g| g.agent_id == chief)
        .unwrap();
    assert_eq!(grant.created_by, Author::System);
    assert_eq!(grant.scope["default"], true);
    let other = embedder.agent(&shared.id, "Other").await;
    let path = format!("{prefix}/grants/workspace-mailbox");
    let (status, _) = super::response(
        &embedder,
        "POST",
        &path,
        json!({"agent_id":other.id}),
        Some(&embedder.other_cookie),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(
        response(&embedder, "POST", &path, json!({"agent_id":other.id}))
            .await
            .0,
        201
    );
    let request = pagis_core::Request {
        id: RequestId::generate(),
        workspace_id: shared.id.clone(),
        agent_id: chief,
        run_id: None,
        kind: pagis_core::Request::TOOL_ACTION_KIND.into(),
        payload: json!({"tool_name":"workspace_mail_send","workspace_mailbox_grant_id":grant.id,"proposed_rules":["example.com"]}),
        state: RequestState::Pending,
        values: None,
        decided_by: None,
        decided_at: None,
        created_at: now_ms(),
    };
    embedder.stores.requests.create(&request).await.unwrap();
    let path = format!("{prefix}/requests/{}/decision", request.id);
    let (status, _) = super::response(
        &embedder,
        "POST",
        &path,
        json!({"decision":"approved","scope":"always"}),
        Some(&embedder.other_cookie),
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(
        response(
            &embedder,
            "POST",
            &path,
            json!({"decision":"approved","scope":"always"})
        )
        .await
        .0,
        200
    );
    let changed = embedder
        .stores
        .grants
        .get(&shared.id, &grant.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.allow_rules(), vec!["example.com"]);
    assert_eq!(changed.scope["default"], true);
    assert_eq!(changed.revision, 2);
}

async fn calendar_mailbox(e: &Embedder) {
    let connection = provider(e).await;
    let mut record = e
        .stores
        .connections
        .get(&e.routers.state.org_workspace_id, &connection)
        .await
        .unwrap()
        .unwrap();
    record.config["imap_host"] = json!("imap.example.com");
    record.config["imap_port"] = json!(993);
    record.config["smtp_host"] = json!("smtp.example.com");
    record.config["smtp_port"] = json!(465);
    e.stores
        .connections
        .set_config(&record.workspace_id, &record.id, &record.config)
        .await
        .unwrap();
    let (status, _) = response(
        e,
        "POST",
        "/api/v1/workspace/mailbox",
        json!({"connection_id": connection, "local_part": "reader"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn imip_http_flow_receives_answers_and_cancels_through_fake_mail() {
    let e = Embedder::new().await;
    calendar_mailbox(&e).await;
    let fixture = include_str!("../../../pagis-core/tests/fixtures/imip/google-request.ics");
    let built = pagis_mail::build_mime(
        "organizer@example.org",
        &pagis_mail::OutgoingMessage {
            to: vec!["reader@example.com".into()],
            body: "Please join.".into(),
            attachments: vec![pagis_mail::OutgoingAttachment {
                name: "invite.ics".into(),
                content_type: "text/calendar; method=REQUEST".into(),
                data: fixture.as_bytes().to_vec(),
            }],
            ..Default::default()
        },
        0,
    )
    .unwrap();
    let id = e.mail_transport.deliver_raw(&built.raw);
    let (status, events) = response(
        &e,
        "GET",
        "/api/v1/calendar/events?from=1792454400000&to=1792627200000",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events}");
    assert_eq!(events[0]["event"]["uid"], "meeting@example.org");
    assert_eq!(
        events[0]["event"]["data"]["attendees"][0]["status"],
        "needs-action"
    );
    let (_, message) = response(
        &e,
        "GET",
        &format!("/api/v1/workspace/mail/messages/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(message["invitation"]["uid"], "meeting@example.org");
    let path = format!(
        "/api/v1/calendar/events/{}/response",
        events[0]["event"]["id"].as_str().unwrap()
    );
    let day = now_ms() / 86_400_000;
    let mailbox = e
        .stores
        .workspace_mailboxes
        .get(&e.workspace)
        .await
        .unwrap()
        .unwrap();
    for _ in 0..mailbox.outgoing_cap {
        assert!(
            e.stores
                .workspace_mailboxes
                .reserve_send(&e.workspace, day)
                .await
                .unwrap()
        );
    }
    assert!(
        response(&e, "POST", &path, json!({"status":"accepted"}))
            .await
            .0
            .is_client_error()
    );
    let stored = e
        .stores
        .calendar
        .get_by_uid(&e.workspace, "meeting@example.org")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.data.attendees[0].status,
        ParticipationStatus::NeedsAction
    );
    assert!(e.mail_transport.sent().is_empty());
    for _ in 0..mailbox.outgoing_cap {
        e.stores
            .workspace_mailboxes
            .release_send(&e.workspace, day)
            .await
            .unwrap();
    }
    for answer in ["accepted", "tentative", "declined"] {
        let (status, changed) = response(&e, "POST", &path, json!({"status": answer})).await;
        assert_eq!(status, StatusCode::OK, "{changed}");
        assert_eq!(changed["data"]["attendees"][0]["status"], answer);
        let sent = e.mail_transport.sent();
        let reply = sent.last().unwrap();
        assert_eq!(reply.to, ["organizer@example.org"]);
        let calendar = pagis_core::imip::SchedulingMessage::parse(
            std::str::from_utf8(&reply.attachments[0].data).unwrap(),
        )
        .unwrap();
        assert_eq!(calendar.method, pagis_core::imip::SchedulingMethod::Reply);
        assert_eq!(
            serde_json::to_value(&calendar.attendees[0].status).unwrap(),
            answer
        );
    }
    let cancel = include_str!("../../../pagis-core/tests/fixtures/imip/google-cancel.ics");
    let built = pagis_mail::build_mime(
        "organizer@example.org",
        &pagis_mail::OutgoingMessage {
            to: vec!["reader@example.com".into()],
            body: "Cancelled.".into(),
            attachments: vec![pagis_mail::OutgoingAttachment {
                name: "invite.ics".into(),
                content_type: "text/calendar; method=CANCEL".into(),
                data: cancel.as_bytes().to_vec(),
            }],
            ..Default::default()
        },
        0,
    )
    .unwrap();
    e.mail_transport.deliver_raw(&built.raw);
    let (_, events) = response(
        &e,
        "GET",
        "/api/v1/calendar/events?from=1792454400000&to=1792627200000",
        Value::Null,
    )
    .await;
    assert_eq!(
        events[0]["event"]["data"]["invitation"]["status"],
        "cancelled"
    );
    assert_eq!(
        response(&e, "POST", &path, json!({"status":"accepted"}))
            .await
            .0,
        StatusCode::CONFLICT
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn imip_http_flow_sends_only_outside_the_org_and_applies_replies() {
    let e = Embedder::new().await;
    calendar_mailbox(&e).await;
    let other = e
        .stores
        .workspaces
        .get(&e.other_workspace)
        .await
        .unwrap()
        .unwrap();
    e.stores
        .users
        .set_email_and_password(
            other.user_id.as_ref().unwrap(),
            "colleague@example.com",
            "hash",
            now_ms(),
        )
        .await
        .unwrap();
    let mut body = serde_json::to_value(
        pagis_core::imip::SchedulingMessage::parse(include_str!(
            "../../../pagis-core/tests/fixtures/imip/google-request.ics"
        ))
        .unwrap()
        .data
        .unwrap(),
    )
    .unwrap();
    body["attendees"] = json!([
        {"email":"colleague@example.com", "name":"Colleague", "status":"needs-action"},
        {"email":"guest@example.org", "name":"Guest", "status":"needs-action"},
        {"email":"second@example.org", "name":"Second", "status":"needs-action"}
    ]);
    let (status, event) = response(&e, "POST", "/api/v1/calendar/events", body.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{event}");
    let sent = e.mail_transport.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].to, ["guest@example.org"]);
    assert_eq!(sent[1].to, ["second@example.org"]);
    for message in &sent {
        let built = pagis_mail::build_mime("reader@example.com", message, 0).unwrap();
        let parsed =
            pagis_mail::read_mime(&built.raw, pagis_mail::MessageId::new("INBOX:1")).unwrap();
        let calendar =
            pagis_core::imip::SchedulingMessage::parse(parsed.calendar.as_deref().unwrap())
                .unwrap();
        assert_eq!(calendar.method, pagis_core::imip::SchedulingMethod::Request);
        assert_eq!(calendar.organizer, "reader@example.com");
        assert_eq!(calendar.uid, event["uid"]);
        assert!(!parsed.text.is_empty());
    }
    let reply = include_str!("../../../pagis-core/tests/fixtures/imip/google-reply.ics")
        .replace("meeting@example.org", event["uid"].as_str().unwrap())
        .replace("reader@example.com", "guest@example.org")
        .replace("organizer@example.org", "reader@example.com")
        .replace("SEQUENCE:1", "SEQUENCE:0");
    let built = pagis_mail::build_mime(
        "guest@example.org",
        &pagis_mail::OutgoingMessage {
            to: vec!["reader@example.com".into()],
            body: "Accepted.".into(),
            attachments: vec![pagis_mail::OutgoingAttachment {
                name: "reply.ics".into(),
                content_type: "text/calendar; method=REPLY".into(),
                data: reply.into_bytes(),
            }],
            ..Default::default()
        },
        0,
    )
    .unwrap();
    e.mail_transport.deliver_raw(&built.raw);
    let (_, events) = response(
        &e,
        "GET",
        "/api/v1/calendar/events?from=1792454400000&to=1792627200000",
        Value::Null,
    )
    .await;
    assert_eq!(
        events[0]["event"]["data"]["attendees"][1]["status"],
        "accepted"
    );
    let path = format!("/api/v1/calendar/events/{}", event["id"].as_str().unwrap());
    body["title"] = json!("Changed");
    let (status, changed) = response(&e, "PUT", &path, body).await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["data"]["invitation"]["sequence"], 1);
    assert_eq!(
        response(&e, "DELETE", &path, Value::Null).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, cancelled) = response(&e, "GET", &path, Value::Null).await;
    assert_eq!(cancelled["data"]["invitation"]["status"], "cancelled");
    let sent = e.mail_transport.sent();
    assert_eq!(sent.len(), 6);
    assert!(
        sent[4].attachments[0]
            .content_type
            .contains("method=CANCEL")
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn imip_http_requires_a_mailbox_only_for_outside_attendees() {
    let e = Embedder::new().await;
    let data = pagis_core::imip::SchedulingMessage::parse(include_str!(
        "../../../pagis-core/tests/fixtures/imip/google-request.ics"
    ))
    .unwrap()
    .data
    .unwrap();
    let body = serde_json::to_value(data).unwrap();
    assert_eq!(
        response(&e, "POST", "/api/v1/calendar/events", body.clone())
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let owner = e
        .stores
        .workspaces
        .get(&e.workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id
        .unwrap();
    e.stores
        .users
        .set_email_and_password(&owner, "reader@example.com", "hash", now_ms())
        .await
        .unwrap();
    let (status, event) = response(&e, "POST", "/api/v1/calendar/events", body).await;
    assert_eq!(status, StatusCode::CREATED, "{event}");
    assert!(event["data"]["invitation"].is_null());
    assert!(e.mail_transport.sent().is_empty());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn imip_http_mail_actions_answer_the_moved_occurrence() {
    let e = Embedder::new().await;
    calendar_mailbox(&e).await;
    let mut moved_id = String::new();
    for fixture in [
        include_str!("../../../pagis-core/tests/fixtures/imip/google-request.ics"),
        include_str!("../../../pagis-core/tests/fixtures/imip/google-occurrence-request.ics"),
    ] {
        let raw = pagis_mail::build_mime(
            "organizer@example.org",
            &pagis_mail::OutgoingMessage {
                to: vec!["reader@example.com".into()],
                body: "Please join.".into(),
                attachments: vec![pagis_mail::OutgoingAttachment {
                    name: "invite.ics".into(),
                    content_type: "text/calendar; method=REQUEST".into(),
                    data: fixture.as_bytes().to_vec(),
                }],
                ..Default::default()
            },
            0,
        )
        .unwrap();
        moved_id = e.mail_transport.deliver_raw(&raw.raw).to_string();
        let (status, _) = response(
            &e,
            "GET",
            &format!("/api/v1/workspace/mail/messages/{moved_id}"),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (_, mail) = response(
        &e,
        "GET",
        &format!("/api/v1/workspace/mail/messages/{moved_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(mail["invitation"]["data"]["start"], "2026-11-03T11:00:00");
    assert_eq!(
        mail["invitation"]["data"]["invitation"]["occurrence"]["series_uid"],
        "meeting@example.org"
    );
    let path = format!(
        "/api/v1/calendar/events/{}/response",
        mail["invitation"]["id"].as_str().unwrap()
    );
    let (status, event) = response(&e, "POST", &path, json!({"status": "accepted"})).await;
    assert_eq!(status, StatusCode::OK, "{event}");
    let sent = e.mail_transport.sent();
    let reply = pagis_core::imip::SchedulingMessage::parse(
        std::str::from_utf8(&sent[0].attachments[0].data).unwrap(),
    )
    .unwrap();
    assert_eq!(reply.uid, "meeting@example.org");
    assert_eq!(
        reply.recurrence_id.as_deref(),
        Some("2026-11-03T14:00:00+00:00")
    );
    assert_eq!(
        reply.attendees[0].status,
        pagis_core::ParticipationStatus::Accepted
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn workspace_mail_routes_use_the_supplied_store_without_imap() {
    let host = std::sync::Arc::new(pagis_mail::fake::MemoryWorkspaceMailHost::default());
    let embedder = Embedder::with_workspace_mail(host.clone()).await;
    let connection = provider(&embedder).await;
    let (status, mailbox) = response(
        &embedder,
        "POST",
        "/api/v1/workspace/mailbox",
        json!({"connection_id": connection, "local_part": "person"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{mailbox}");
    let mail = host.store(&embedder.workspace).unwrap();
    let id = mail.deliver("sender@example.net", "Invoice", "invoice text");
    mail.attach(
        &id,
        pagis_mail::OutgoingAttachment {
            name: "invoice.txt".into(),
            content_type: "text/plain".into(),
            data: b"invoice attachment".to_vec(),
        },
    );
    let root = "/api/v1/workspace/mail";
    let message = format!("{root}/messages/{id}");
    let (status, list) = response(
        &embedder,
        "GET",
        &format!("{root}/messages?q=invoice"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"][0]["id"], id.as_str());
    let (status, opened) = response(&embedder, "GET", &message, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_eq!(opened["text"], "invoice text");
    assert_eq!(opened["seen"], true);
    let thread: String =
        url::form_urlencoded::byte_serialize(opened["thread_id"].as_str().unwrap().as_bytes())
            .collect();
    let (_, thread) = response(
        &embedder,
        "GET",
        &format!("{root}/threads/{thread}"),
        Value::Null,
    )
    .await;
    assert_eq!(thread["items"].as_array().unwrap().len(), 1);
    for (method, path, body) in [
        (
            "POST",
            format!("{root}/folders"),
            json!({"name":"Receipts"}),
        ),
        (
            "PATCH",
            format!("{root}/folders/Receipts"),
            json!({"name":"Invoices"}),
        ),
        (
            "POST",
            format!("{message}/labels"),
            json!({"name":"Accounts"}),
        ),
    ] {
        let (status, body) = response(&embedder, method, &path, body).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{path}: {body}");
    }
    let (_, labeled) = response(&embedder, "GET", &message, Value::Null).await;
    assert_eq!(labeled["labels"], json!(["Accounts"]));
    assert_eq!(
        response(
            &embedder,
            "DELETE",
            &format!("{message}/labels/Accounts"),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, cleared) = response(&embedder, "GET", &message, Value::Null).await;
    assert_eq!(cleared["labels"], json!([]));
    assert_eq!(
        response(
            &embedder,
            "POST",
            &format!("{message}/move"),
            json!({"name":"Invoices"})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, inbox) = response(
        &embedder,
        "GET",
        &format!("{root}/messages?folder=INBOX"),
        Value::Null,
    )
    .await;
    assert_eq!(inbox["items"], json!([]));
    let (_, moved) = response(
        &embedder,
        "GET",
        &format!("{root}/messages?folder=Invoices"),
        Value::Null,
    )
    .await;
    assert_eq!(moved["items"][0]["id"], id.as_str());
    let download = embedder
        .routers
        .product
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("{message}/attachments/0"))
                .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()))
                .header("cookie", &embedder.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.headers()["content-type"], "text/plain");
    assert_eq!(
        to_bytes(download.into_body(), 1024).await.unwrap(),
        "invoice attachment"
    );
    assert_eq!(
        response(
            &embedder,
            "POST",
            &format!("{message}/move"),
            json!({"name":"INBOX"})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        response(
            &embedder,
            "DELETE",
            &format!("{root}/folders/Invoices"),
            Value::Null
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (_, folders) = response(&embedder, "GET", &format!("{root}/folders"), Value::Null).await;
    assert_eq!(folders["items"], json!(["INBOX"]));
    let agent = embedder.agent(&embedder.workspace, "Reader").await;
    let desk = &embedder.routers.state.mailbox_desk;
    assert!(
        desk.open_workspace_for_agent(&embedder.workspace, &agent.id)
            .await
            .is_err()
    );
    let (status, grant) = response(
        &embedder,
        "POST",
        "/api/v1/grants/workspace-mailbox",
        json!({"agent_id": agent.id}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let mut store = desk
        .open_workspace_for_agent(&embedder.workspace, &agent.id)
        .await
        .unwrap();
    assert_eq!(
        store.message(&id).await.unwrap().message.text,
        "invoice text"
    );
    store.close().await.unwrap();
    embedder
        .stores
        .grants
        .revoke(
            &embedder.workspace,
            &GrantId::from(grant["id"].as_str().unwrap().to_string()),
            now_ms(),
        )
        .await
        .unwrap();
    assert!(
        desk.open_workspace_for_agent(&embedder.workspace, &agent.id)
            .await
            .is_err()
    );
    assert!(embedder.mail_transport.logins().is_empty());
    assert!(embedder.mailbox_host.addresses().is_empty());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn imip_request_reaches_the_calendar_through_the_supplied_memory_store() {
    for access in ["calendar", "message", "thread"] {
        let host = std::sync::Arc::new(pagis_mail::fake::MemoryWorkspaceMailHost::default());
        let e = Embedder::with_workspace_mail(host.clone()).await;
        let connection = provider(&e).await;
        let (status, mailbox) = response(
            &e,
            "POST",
            "/api/v1/workspace/mailbox",
            json!({"connection_id": connection, "local_part": "reader"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{mailbox}");
        let raw = pagis_mail::build_mime(
            "organizer@example.org",
            &pagis_mail::OutgoingMessage {
                to: vec!["reader@example.com".into()],
                body: "Please join.".into(),
                attachments: vec![pagis_mail::OutgoingAttachment {
                    name: "invite.ics".into(),
                    content_type: "text/calendar; method=REQUEST".into(),
                    data: include_bytes!(
                        "../../../pagis-core/tests/fixtures/imip/google-request.ics"
                    )
                    .to_vec(),
                }],
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mail = host.store(&e.workspace).unwrap();
        let id = mail.deliver_raw(&raw.raw).unwrap();
        if access == "calendar" {
            for _ in 0..101 {
                mail.deliver("sender@example.org", "Notes", "Mail without an invitation.");
            }
        }
        if access != "calendar" {
            let path = if access == "message" {
                format!("/api/v1/workspace/mail/messages/{id}")
            } else {
                let message = pagis_mail::read_mime(&raw.raw, id.clone()).unwrap();
                let thread: String =
                    url::form_urlencoded::byte_serialize(message.summary.thread_id.as_bytes())
                        .collect();
                format!("/api/v1/workspace/mail/threads/{thread}")
            };
            assert_eq!(
                response(&e, "GET", &path, Value::Null).await.0,
                StatusCode::OK
            );
            assert!(
                e.stores
                    .calendar
                    .get_by_uid(&e.workspace, "meeting@example.org")
                    .await
                    .unwrap()
                    .is_some(),
                "{access}"
            );
        }
        let (status, events) = response(
            &e,
            "GET",
            "/api/v1/calendar/events?from=1792454400000&to=1792627200000",
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{events}");
        assert_eq!(events[0]["event"]["uid"], "meeting@example.org");
        assert_eq!(
            events[0]["event"]["data"]["attendees"][0]["status"],
            "needs-action"
        );
        let (_, message) = response(
            &e,
            "GET",
            &format!("/api/v1/workspace/mail/messages/{id}"),
            Value::Null,
        )
        .await;
        assert_eq!(message["invitation"]["uid"], "meeting@example.org");
        assert!(e.mail_transport.logins().is_empty());
        assert!(e.mailbox_host.addresses().is_empty());
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn mail_invitation_response_checks_visibility_with_a_supplied_store() {
    let host = std::sync::Arc::new(pagis_mail::fake::MemoryWorkspaceMailHost::default());
    let e = Embedder::with_workspace_mail(host.clone()).await;
    let mut shared = e
        .stores
        .workspaces
        .get(&e.workspace)
        .await
        .unwrap()
        .unwrap();
    let owner = shared.user_id.take().unwrap();
    shared.id = WorkspaceId::generate();
    shared.kind = WorkspaceKind::Shared;
    e.stores.workspaces.create(&shared).await.unwrap();
    let mut member = WorkspaceMember {
        workspace_id: shared.id.clone(),
        person_id: owner.clone(),
        role: WorkspaceRole::Manager,
        author: Author::Person(owner),
    };
    e.stores.workspace_members.add(&member).await.unwrap();
    let connection = provider(&e).await;
    let prefix = format!("/api/v1/workspaces/{}", shared.id);
    let (status, mailbox) = response(
        &e,
        "POST",
        &format!("{prefix}/workspace/mailbox"),
        json!({"connection_id":connection, "local_part":"reader"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{mailbox}");
    let other = e
        .stores
        .workspaces
        .get(&e.other_workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id
        .unwrap();
    e.stores
        .workspace_members
        .add(&WorkspaceMember {
            workspace_id: shared.id.clone(),
            person_id: other,
            role: WorkspaceRole::Manager,
            author: member.author.clone(),
        })
        .await
        .unwrap();
    member.role = WorkspaceRole::Responder;
    e.stores.workspace_members.change(&member).await.unwrap();
    let raw = pagis_mail::build_mime(
        "organizer@example.org",
        &pagis_mail::OutgoingMessage {
            to: vec!["reader@example.com".into()],
            body: "Please join.".into(),
            attachments: vec![pagis_mail::OutgoingAttachment {
                name: "invite.ics".into(),
                content_type: "text/calendar; method=REQUEST".into(),
                data: include_bytes!("../../../pagis-core/tests/fixtures/imip/google-request.ics")
                    .to_vec(),
            }],
            ..Default::default()
        },
        0,
    )
    .unwrap();
    let mail = host.store(&shared.id).unwrap();
    let id = mail.deliver_raw(&raw.raw).unwrap();
    let path = format!("{prefix}/workspace/mail/messages/{id}/response");
    for visibility in [
        MailVisibility::AgentOnly,
        MailVisibility::Escalations,
        MailVisibility::AllMail,
    ] {
        e.stores
            .workspace_mailboxes
            .set_visibility(&shared.id, visibility)
            .await
            .unwrap();
        let (status, event) = response(&e, "POST", &path, json!({"status":"accepted"})).await;
        if visibility == MailVisibility::AllMail {
            assert_eq!(status, StatusCode::OK, "{event}");
            assert_eq!(event["data"]["attendees"][0]["status"], "accepted");
        } else {
            assert_eq!(status, StatusCode::NOT_FOUND, "{event}");
            let event = e
                .stores
                .calendar
                .get_by_uid(&shared.id, "meeting@example.org")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                event.data.attendees[0].status,
                ParticipationStatus::NeedsAction
            );
        }
    }
    let audit = e
        .stores
        .audit
        .list(&e.routers.state.org_workspace_id, 0, 200)
        .await
        .unwrap();
    assert!(
        audit
            .iter()
            .any(|entry| entry.action == "mail.invitation.respond")
    );
    member.role = WorkspaceRole::Reader;
    e.stores.workspace_members.change(&member).await.unwrap();
    assert_eq!(
        response(&e, "POST", &path, json!({"status":"declined"}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let event = e
        .stores
        .calendar
        .get_by_uid(&shared.id, "meeting@example.org")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.data.attendees[0].status,
        ParticipationStatus::Accepted
    );
}
