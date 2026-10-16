//! New People receive the Org defaults through the Administration Port.
#![cfg(not(feature = "native"))]
#[allow(dead_code)]
mod support {
    pub mod agents;
}
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use pagis_core::*;
use serde_json::{Value, json};
use support::agents::Embedder;
use tower::ServiceExt;

async fn memory_embedder() -> (
    Embedder,
    std::sync::Arc<pagis_mail::fake::MemoryWorkspaceMailHost>,
) {
    let host = std::sync::Arc::new(pagis_mail::fake::MemoryWorkspaceMailHost::default());
    (Embedder::with_workspace_mail(host.clone()).await, host)
}

async fn request(e: &Embedder, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = e
        .routers
        .administration
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("cookie", &e.cookie)
                .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()))
                .body(Body::from(body.to_string()))
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

#[tokio::test]
async fn defaults_are_validated_stored_and_audited() {
    let (e, _host) = memory_embedder().await;
    let path = "/api/v1/administration/person-defaults";
    let (status, body) = request(&e, "GET", path, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["defaults"]["address_rule"], Value::Null);
    let invalid = json!({"address_rule": null, "mailbox_connection_id": null,
        "shared_workspaces": [{"workspace_id": e.workspace, "role": "responder"}]});
    assert_eq!(
        request(&e, "PUT", path, invalid).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let defaults =
        json!({"address_rule": null, "mailbox_connection_id": null, "shared_workspaces": []});
    assert_eq!(
        request(&e, "PUT", path, defaults.clone()).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&e, "GET", path, Value::Null).await.1["defaults"],
        defaults
    );
    let org = e.stores.orgs.list().await.unwrap().remove(0);
    let entries = e
        .stores
        .audit
        .list(&org.workspace_id, 0, 100)
        .await
        .unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.action == "person_defaults.changed"
                && matches!(entry.actor, Author::Person(_)))
    );
}

#[tokio::test]
async fn no_rule_keeps_the_new_person_without_a_mailbox() {
    let (e, _host) = memory_embedder().await;
    let (status, body) = request(
        &e,
        "POST",
        "/api/v1/administration/people",
        json!({"email":"ada@example.com", "name":"Ada Lovelace"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let workspace = WorkspaceId::from(body["person"]["workspace_id"].as_str().unwrap().to_owned());
    assert!(
        e.stores
            .workspace_mailboxes
            .get(&workspace)
            .await
            .unwrap()
            .is_none()
    );
    assert!(body["invite"]["url"].is_string());
}

async fn configure(e: &Embedder) -> (Org, WorkspaceId) {
    use pagis_mail::{Endpoint, MailboxCapabilities, MailboxHost, MailboxProvider};
    let org = e.stores.orgs.list().await.unwrap().remove(0);
    let settings = MailboxProvider {
        account: None,
        domain: "example.com".into(),
        imap: Some(Endpoint::new("imap.example.com", 993)),
        smtp: Some(Endpoint::new("smtp.example.com", 465)),
        capabilities: MailboxCapabilities::of(e.mailbox_host.capabilities(), false),
    };
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: org.workspace_id.clone(),
        provider: "test-mail".into(),
        alias: "mail".into(),
        display_name: "Mail".into(),
        status: Connection::CONNECTED.into(),
        config: settings.config(),
        authorized_capabilities: vec![],
        created_at: now_ms(),
    };
    e.secrets
        .set(&pagis_mail::host_api_key_secret_name("mail"), "test-key")
        .unwrap();
    e.stores.connections.create(&connection).await.unwrap();
    let (status, team) = request(e, "POST", "/api/v1/administration/workspaces", json!({
        "name": "Support", "manager_id": e.stores.workspaces.get(&e.workspace).await.unwrap().unwrap().user_id
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{team}");
    let team = WorkspaceId::from(team["id"].as_str().unwrap().to_owned());
    let (status, body) = request(
        e,
        "PUT",
        "/api/v1/administration/person-defaults",
        json!({
            "address_rule": "first_name", "mailbox_connection_id": connection.id,
            "shared_workspaces": [{"workspace_id": team, "role": "reader"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    (org, team)
}

#[tokio::test]
async fn a_new_person_gets_the_mailbox_grant_and_default_memberships_with_the_administrator_author()
{
    let (e, _host) = memory_embedder().await;
    let (_, team) = configure(&e).await;
    let (status, body) = request(
        &e,
        "POST",
        "/api/v1/administration/people",
        json!({"email":"ada-login@example.com", "name":"Ada Lovelace"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = UserId::from(body["person"]["id"].as_str().unwrap().to_owned());
    let workspace = e.stores.workspaces.for_user(&id).await.unwrap().unwrap();
    let administrator = e
        .stores
        .workspaces
        .get(&e.workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id
        .unwrap();
    let mailbox = e
        .stores
        .workspace_mailboxes
        .get(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mailbox.address, "ada@example.com");
    assert_eq!(mailbox.author, Author::Person(administrator.clone()));
    assert_eq!(_host.addresses(), vec!["ada@example.com"]);
    let member = e
        .stores
        .workspace_members
        .get(&team, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(member.role, WorkspaceRole::Reader);
    assert_eq!(member.author, Author::Person(administrator));
    assert!(
        e.stores
            .grants
            .live_for_resource(
                &workspace.id,
                &workspace.chief_of_staff_agent_id.unwrap(),
                Grant::WORKSPACE_MAILBOX_KIND,
                workspace.id.as_str()
            )
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        e.stores
            .phone_numbers
            .list(&workspace.id)
            .await
            .unwrap()
            .is_empty()
    );
    let again = request(
        &e,
        "POST",
        &format!("/api/v1/administration/people/{id}/sign-in-links"),
        Value::Null,
    )
    .await;
    assert_eq!(again.0, StatusCode::CREATED, "{}", again.1);
    assert_eq!(_host.addresses(), vec!["ada@example.com"]);
}

#[tokio::test]
async fn a_taken_address_leaves_no_person_and_retry_uses_the_explicit_choice() {
    let (e, _host) = memory_embedder().await;
    let (org, _) = configure(&e).await;
    let first = request(
        &e,
        "POST",
        "/api/v1/administration/people",
        json!({"email":"ada-login@example.com", "name":"Ada Lovelace"}),
    )
    .await;
    assert_eq!(first.0, StatusCode::CREATED, "{}", first.1);
    let count = e.stores.users.list_by_org(&org.id).await.unwrap().len();
    let (status, body) = request(
        &e,
        "POST",
        "/api/v1/administration/people",
        json!({"email":"ada-other@example.com", "name":"Ada Byron"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("ada@example.com") && message.contains("ada2@example.com"),
        "{body}"
    );
    assert_eq!(
        e.stores.users.list_by_org(&org.id).await.unwrap().len(),
        count
    );
    let (status, body) = request(&e, "POST", "/api/v1/administration/people", json!({
        "email":"ada-other@example.com", "name":"Ada Byron", "mailbox_address":"ada2@example.com"
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        _host.addresses(),
        vec!["ada2@example.com", "ada@example.com"]
    );
}

#[tokio::test]
async fn explicit_address_must_use_the_org_domain_and_defaults_must_be_shared() {
    let (e, _host) = memory_embedder().await;
    let (org, _) = configure(&e).await;
    let count = e.stores.users.list_by_org(&org.id).await.unwrap().len();
    let (status, body) = request(&e, "POST", "/api/v1/administration/people", json!({
        "email":"ada@example.com", "name":"Ada Lovelace", "mailbox_address":"ada@other.example.com"
    })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(
        e.stores.users.list_by_org(&org.id).await.unwrap().len(),
        count
    );
}

#[tokio::test]
async fn member_and_unsigned_requests_cannot_read_or_change_defaults() {
    let (e, _host) = memory_embedder().await;
    let person = e
        .stores
        .workspaces
        .get(&e.other_workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id
        .unwrap();
    e.stores
        .users
        .set_role(&person, UserRole::Member, now_ms())
        .await
        .unwrap();
    for method in ["GET", "PUT"] {
        for (cookie, status) in [
            ("", StatusCode::UNAUTHORIZED),
            (e.other_cookie.as_str(), StatusCode::FORBIDDEN),
        ] {
            let response = e
                .routers
                .administration
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/api/v1/administration/person-defaults")
                        .header("cookie", cookie)
                        .header("content-type", "application/json")
                        .extension(pagis_server::Peer("127.0.0.1:1234".parse().unwrap()))
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
        }
    }
}

#[tokio::test]
async fn concurrent_people_with_one_address_get_one_mailbox_and_no_extra_person() {
    let (e, _host) = memory_embedder().await;
    let (org, _) = configure(&e).await;
    let count = e.stores.users.list_by_org(&org.id).await.unwrap().len();
    let (first, second) = tokio::join!(
        request(
            &e,
            "POST",
            "/api/v1/administration/people",
            json!({"email":"ada-one@example.com", "name":"Ada Lovelace"})
        ),
        request(
            &e,
            "POST",
            "/api/v1/administration/people",
            json!({"email":"ada-two@example.com", "name":"Ada Byron"})
        )
    );
    assert!(
        matches!(
            (first.0, second.0),
            (StatusCode::CREATED, StatusCode::CONFLICT)
                | (StatusCode::CONFLICT, StatusCode::CREATED)
        ),
        "{first:?} {second:?}"
    );
    assert_eq!(
        e.stores.users.list_by_org(&org.id).await.unwrap().len(),
        count + 1
    );
    assert_eq!(_host.addresses(), vec!["ada@example.com"]);
}

#[tokio::test]
async fn default_memberships_apply_without_an_address_rule() {
    let (e, _host) = memory_embedder().await;
    let (_, team) = configure(&e).await;
    assert_eq!(
        request(
            &e,
            "PUT",
            "/api/v1/administration/person-defaults",
            json!({
                "address_rule":null, "mailbox_connection_id":null,
                "shared_workspaces":[{"workspace_id":team,"role":"responder"}]
            })
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (status, body) = request(
        &e,
        "POST",
        "/api/v1/administration/people",
        json!({"email":"ada@example.com", "name":"Ada Lovelace"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = UserId::from(body["person"]["id"].as_str().unwrap().to_owned());
    let workspace = e.stores.workspaces.for_user(&id).await.unwrap().unwrap();
    assert!(
        e.stores
            .workspace_mailboxes
            .get(&workspace.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        e.stores
            .workspace_members
            .get(&team, &id)
            .await
            .unwrap()
            .unwrap()
            .role,
        WorkspaceRole::Responder
    );
}

#[tokio::test]
async fn a_deleted_agent_mailbox_address_is_taken_too() {
    let (e, _host) = memory_embedder().await;
    let (org, _) = configure(&e).await;
    let agent = e.agent(&e.other_workspace, "Ada").await;
    let defaults = e.stores.orgs.person_defaults(&org.id).await.unwrap();
    e.stores
        .agent_mailboxes
        .create(&AgentMailbox {
            id: AgentMailboxId::generate(),
            workspace_id: e.other_workspace.clone(),
            agent_id: agent.id,
            connection_id: defaults.mailbox_connection_id.unwrap(),
            address: "ada@example.com".into(),
            state: AgentMailboxState::Deleted,
            reason: None,
            outgoing_cap: 20,
            allow_rules: vec![],
            cursor: None,
            sends_day: None,
            sends_today: 0,
            created_at: now_ms(),
            deleted_at: Some(now_ms()),
        })
        .await
        .unwrap();
    let (status, body) = request(
        &e,
        "POST",
        "/api/v1/administration/people",
        json!({"email":"ada@example.com", "name":"Ada Lovelace"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ada2@example.com")
    );
}

#[tokio::test]
async fn an_address_at_the_host_has_a_free_choice_before_a_person_is_written() {
    use pagis_mail::{HostAccount, MailboxHost, MailboxPassword};
    let e = Embedder::new().await;
    let (org, _) = configure(&e).await;
    let account = HostAccount::new("example.com", "", Some("test-key".to_owned()));
    for local in ["ada", "ada2"] {
        e.mailbox_host
            .create(
                &account,
                local,
                Some(&MailboxPassword::new("test-password")),
                20,
            )
            .await
            .unwrap();
    }
    let count = e.stores.users.list_by_org(&org.id).await.unwrap().len();
    let (status, body) = request(
        &e,
        "POST",
        "/api/v1/administration/people",
        json!({
            "email":"ada@example.com", "name":"Ada Lovelace"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ada3@example.com")
    );
    assert_eq!(
        e.stores.users.list_by_org(&org.id).await.unwrap().len(),
        count
    );
}

#[tokio::test]
async fn person_defaults_use_the_memory_workspace_mail_host_without_imap() {
    let host = std::sync::Arc::new(pagis_mail::fake::MemoryWorkspaceMailHost::default());
    let e = Embedder::with_workspace_mail(host.clone()).await;
    let (org, team) = configure(&e).await;
    let defaults = e.stores.orgs.person_defaults(&org.id).await.unwrap();
    let id = defaults.mailbox_connection_id.clone().unwrap();
    let mut connection = e
        .stores
        .connections
        .get(&org.workspace_id, &id)
        .await
        .unwrap()
        .unwrap();
    let mut settings = pagis_mail::mailbox_provider(&connection).unwrap();
    settings.imap = None;
    settings.smtp = None;
    connection.config = settings.config();
    e.stores
        .connections
        .set_config(&org.workspace_id, &id, &connection.config)
        .await
        .unwrap();
    e.secrets
        .delete(&pagis_mail::host_api_key_secret_name("mail"))
        .unwrap();
    e.mailbox_host
        .fail_with(Some(pagis_mail::HostErrorCode::Unauthorized));
    let (status, body) = request(
        &e,
        "PUT",
        "/api/v1/administration/person-defaults",
        serde_json::to_value(defaults).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = request(
        &e,
        "POST",
        "/api/v1/administration/people",
        json!({"name":"Ada Lovelace", "email":"ada-login@example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let workspace = WorkspaceId::from(body["person"]["workspace_id"].as_str().unwrap().to_owned());
    let person = UserId::from(body["person"]["id"].as_str().unwrap().to_owned());
    assert!(host.store(&workspace).is_some());
    assert!(e.mailbox_host.calls().is_empty());
    assert!(
        e.stores
            .workspace_members
            .get(&team, &person)
            .await
            .unwrap()
            .is_some()
    );
    let (status, conflict) = request(
        &e,
        "POST",
        "/api/v1/administration/people",
        json!({"name":"Ada Byron", "email":"ada-other@example.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert!(
        conflict["error"]["message"]
            .as_str()
            .unwrap()
            .contains("ada2@example.com")
    );
    let (status, body) = request(&e, "POST", "/api/v1/administration/people", json!({"name":"Ada Byron", "email":"ada-other@example.com", "mailbox_address":"ada2@example.com"})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(
        host.store(&WorkspaceId::from(
            body["person"]["workspace_id"].as_str().unwrap().to_owned()
        ))
        .is_some()
    );
}
