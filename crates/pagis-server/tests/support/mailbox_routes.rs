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
    assert_eq!(folders["items"], json!(["INBOX", "Junk", "Trash"]));
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
    for _ in 0..2 {
        assert_eq!(
            response(&embedder, "DELETE", &message, Value::Null).await.0,
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(
        response(&embedder, "GET", &message, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, trash) = response(
        &embedder,
        "GET",
        &format!("{root}/messages?folder=Trash"),
        Value::Null,
    )
    .await;
    assert_eq!(trash["items"], json!([]));
    assert!(embedder.mail_transport.logins().is_empty());
    assert!(embedder.mailbox_host.addresses().is_empty());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn junk_mail_policy_and_corrections_use_the_supplied_store() {
    let host = std::sync::Arc::new(pagis_mail::fake::MemoryWorkspaceMailHost::default());
    let embedder = Embedder::with_workspace_mail(host.clone()).await;
    let connection = provider(&embedder).await;
    assert_eq!(
        response(
            &embedder,
            "POST",
            "/api/v1/workspace/mailbox",
            json!({"connection_id": connection, "local_part":"person"})
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let mail = host.store(&embedder.workspace).unwrap();
    mail.deliver("sender@example.com", "Offer", "Offer");
    let root = "/api/v1/workspace/mail";
    let policy = format!("{root}/junk-policy");
    assert_eq!(
        response(
            &embedder,
            "PUT",
            &policy,
            json!({"spam_mode":"score_only", "lists":{"allow":[],"block":["example.com"]}})
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
    assert_eq!(inbox["items"][0]["junk"], true);
    assert_eq!(
        response(
            &embedder,
            "PUT",
            &policy,
            json!({"spam_mode":"move_to_junk", "lists":{"allow":[],"block":["example.com"]}})
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
    assert!(inbox["items"].as_array().unwrap().is_empty());
    let (_, junk) = response(
        &embedder,
        "GET",
        &format!("{root}/messages?folder=Junk"),
        Value::Null,
    )
    .await;
    assert_eq!(junk["items"].as_array().unwrap().len(), 1);
    let id = junk["items"][0]["id"].as_str().unwrap();
    assert_eq!(
        response(
            &embedder,
            "POST",
            &format!("{root}/messages/{id}/spam"),
            json!({"spam":false})
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
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1);
    assert_eq!(inbox["items"][0]["spam_verdict"]["source"], "person");
    assert_eq!(
        response(
            &embedder,
            "PUT",
            &policy,
            json!({"spam_mode":"other", "lists":{"allow":[],"block":[]}})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn spam_corrections_check_shared_workspace_message_visibility() {
    let host = std::sync::Arc::new(pagis_mail::fake::MemoryWorkspaceMailHost::default());
    let embedder = Embedder::with_workspace_mail(host.clone()).await;
    let connection = provider(&embedder).await;
    let person = embedder
        .stores
        .workspaces
        .get(&embedder.workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id
        .unwrap();
    let shared = pagis_server::provisioning::WorkspaceSeed::from(&embedder.stores)
        .shared(
            &WorkspaceMember {
                workspace_id: WorkspaceId::generate(),
                person_id: person,
                role: WorkspaceRole::Manager,
                author: Author::System,
            },
            "Team",
            "UTC",
            now_ms(),
            embedder.stores.workspace_members.as_ref(),
        )
        .await
        .unwrap();
    let prefix = format!("/api/v1/workspaces/{}", shared.id);
    assert_eq!(
        response(
            &embedder,
            "POST",
            &format!("{prefix}/workspace/mailbox"),
            json!({"connection_id":connection,"local_part":"team"})
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let mail = host.store(&shared.id).unwrap();
    let id = mail.deliver("sender@test", "Message", "Body");
    for level in [
        MailVisibility::AgentOnly,
        MailVisibility::Escalations,
        MailVisibility::AllMail,
    ] {
        embedder
            .stores
            .workspace_mailboxes
            .set_visibility(&shared.id, level)
            .await
            .unwrap();
        for spam in [false, true] {
            let status = response(
                &embedder,
                "POST",
                &format!("{prefix}/workspace/mail/messages/{id}/spam"),
                json!({"spam":spam}),
            )
            .await
            .0;
            assert_eq!(
                status,
                if level == MailVisibility::AllMail {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::NOT_FOUND
                }
            );
        }
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn quarantine_blocks_message_and_folder_deletes_in_the_supplied_store() {
    use pagis_mail::WorkspaceMailReader;
    let host = std::sync::Arc::new(pagis_mail::fake::MemoryWorkspaceMailHost::default());
    let embedder = Embedder::with_workspace_mail(host.clone()).await;
    let connection = provider(&embedder).await;
    let (status, mailbox) = response(
        &embedder,
        "POST",
        "/api/v1/workspace/mailbox",
        json!({"connection_id": connection, "local_part":"person"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let mut mail = host.store(&embedder.workspace).unwrap();
    let id = mail.deliver("sender@test", "Unsafe", "Unsafe");
    let safe_id = mail.deliver("sender@test", "Safe", "Safe");
    let root = "/api/v1/workspace/mail";
    assert_eq!(
        response(
            &embedder,
            "POST",
            &format!("{root}/folders"),
            json!({"name":"Records"})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    for id in [&id, &safe_id] {
        assert_eq!(
            response(
                &embedder,
                "POST",
                &format!("{root}/messages/{id}/move"),
                json!({"name":"Records"})
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
    }
    let summary = mail.message(&id).await.unwrap().message.summary;
    embedder
        .stores
        .junk
        .observe(&pagis_core::JunkMessage {
            workspace_id: embedder.workspace.clone(),
            mailbox: mailbox["address"].as_str().unwrap().into(),
            message_key: summary.assessment_key().into(),
            host_id: id.to_string(),
            from: summary.from,
            subject: summary.subject,
            reasons: vec!["CLAM_VIRUS".into()],
            person_mark: None,
            quarantined: true,
            released: false,
        })
        .await
        .unwrap();
    for (method, suffix, body) in [
        ("DELETE", format!("messages/{id}"), Value::Null),
        (
            "POST",
            format!("messages/{id}/move"),
            json!({"name":"INBOX"}),
        ),
        ("DELETE", "folders/Records".into(), Value::Null),
    ] {
        assert_eq!(
            response(&embedder, method, &format!("{root}/{suffix}"), body)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let (_, page) = response(
        &embedder,
        "GET",
        &format!("{root}/messages?folder=Records"),
        Value::Null,
    )
    .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["id"], safe_id.as_str());
    let (_, page) = response(
        &embedder,
        "GET",
        &format!("{root}/messages?folder=Trash"),
        Value::Null,
    )
    .await;
    assert_eq!(page["items"], json!([]));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), tokio::test)]
async fn every_workspace_mail_message_route_checks_visibility() {
    use pagis_mail::WorkspaceMailReader;
    let host = std::sync::Arc::new(pagis_mail::fake::MemoryWorkspaceMailHost::default());
    let embedder = Embedder::with_workspace_mail(host.clone()).await;
    let connection = provider(&embedder).await;
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
    let shared = pagis_server::provisioning::WorkspaceSeed::from(&embedder.stores)
        .shared(
            &WorkspaceMember {
                workspace_id: WorkspaceId::generate(),
                person_id: person,
                role: WorkspaceRole::Manager,
                author: Author::System,
            },
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
    assert_eq!(
        response(
            &embedder,
            "POST",
            &format!("{prefix}/workspace/mailbox"),
            json!({"connection_id":connection,"local_part":"team"})
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        response(
            &embedder,
            "POST",
            &format!("{prefix}/workspace/mail/folders"),
            json!({"name":"Records"})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let mut mail = host.store(&shared.id).unwrap();
    let root = format!("{prefix}/workspace/mail/messages");
    let missing = response(&embedder, "GET", &format!("{root}/missing"), Value::Null).await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
    for route in pagis_server::routes::ROUTES.iter().filter(|r| {
        r.path.starts_with("/api/v1/workspace/mail/") && r.path.contains("{message_id}")
    }) {
        for method in route.methods {
            let suffix = route.path.split("{message_id}").nth(1).unwrap();
            let (body, expected) = match (*method, suffix) {
                ("get", "" | "/composer") => (Value::Null, StatusCode::OK),
                ("get", "/attachments/{index}") => (Value::Null, StatusCode::OK),
                ("delete", "" | "/labels/{label}") => (Value::Null, StatusCode::NO_CONTENT),
                ("post", "/move") => (json!({"name":"Records"}), StatusCode::NO_CONTENT),
                ("post", "/labels") => (json!({"name":"Accounts"}), StatusCode::NO_CONTENT),
                ("post", "/spam") => (json!({"spam":false}), StatusCode::NO_CONTENT),
                ("post", "/reply" | "/reply-all" | "/forward") => (
                    json!({"to":["sender@test"],"subject":"Reply","body":"Answer"}),
                    StatusCode::CREATED,
                ),
                _ => panic!("Add a valid request for {} {method}", route.path),
            };
            for cookie in [&embedder.cookie, &embedder.other_cookie] {
                for (level, escalated, visible) in [
                    (MailVisibility::AgentOnly, false, false),
                    (MailVisibility::Escalations, false, false),
                    (MailVisibility::Escalations, true, true),
                    (MailVisibility::AllMail, false, true),
                ] {
                    let id = mail.deliver("sender@test", "Message", "Body");
                    mail.attach(
                        &id,
                        pagis_mail::OutgoingAttachment {
                            name: "note.txt".into(),
                            content_type: "text/plain".into(),
                            data: b"Note".to_vec(),
                        },
                    );
                    if escalated {
                        let message = mail.message(&id).await.unwrap();
                        embedder
                            .stores
                            .workspace_mailboxes
                            .escalate(
                                &shared.id,
                                message.message.summary.assessment_key(),
                                shared.chief_of_staff_agent_id.as_ref().unwrap(),
                            )
                            .await
                            .unwrap();
                    }
                    embedder
                        .stores
                        .workspace_mailboxes
                        .set_visibility(&shared.id, level)
                        .await
                        .unwrap();
                    let path = format!("{prefix}{}", route.path.strip_prefix("/api/v1").unwrap())
                        .replace("{message_id}", id.as_str())
                        .replace("{label}", "Accounts")
                        .replace("{index}", "0");
                    let path = if suffix == "/composer" {
                        format!("{path}?action=reply")
                    } else {
                        path
                    };
                    let result = embedder
                        .routers
                        .product
                        .clone()
                        .oneshot(
                            Request::builder()
                                .method(method.to_uppercase().as_str())
                                .uri(&path)
                                .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()))
                                .header("content-type", "application/json")
                                .header("cookie", cookie)
                                .body(Body::from(body.to_string()))
                                .unwrap(),
                        )
                        .await
                        .unwrap();
                    let status = result.status();
                    let bytes = to_bytes(result.into_body(), 1024 * 1024).await.unwrap();
                    assert_eq!(
                        status,
                        if visible {
                            expected
                        } else {
                            StatusCode::NOT_FOUND
                        },
                        "{method} {path}, {level:?}: {bytes:?}"
                    );
                    if !visible {
                        assert_eq!(
                            serde_json::from_slice::<Value>(&bytes).unwrap(),
                            missing.1,
                            "{method} {path}"
                        );
                        assert_eq!(mail.message(&id).await.unwrap().message.text, "Body");
                    }
                }
            }
        }
    }
}
