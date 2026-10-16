//! Shared Workspace configuration, author history and Administrator limits.
#![cfg(not(feature = "native"))]
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

async fn call(
    e: &Embedder,
    admin: bool,
    method: &str,
    path: &str,
    body: Value,
    cookie: &str,
) -> (StatusCode, Value) {
    let router = if admin {
        &e.routers.administration
    } else {
        &e.routers.product
    };
    let response = router
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
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn team(e: &Embedder) -> (WorkspaceId, UserId, AgentId) {
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
        .users
        .set_role(&other, UserRole::Member, now_ms())
        .await
        .unwrap();
    let owner = e
        .stores
        .workspaces
        .get(&e.workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id
        .unwrap();
    let (_, created) = call(
        e,
        true,
        "POST",
        "/api/v1/administration/workspaces",
        json!({"name":"Support", "manager_id":owner}),
        &e.cookie,
    )
    .await;
    let workspace = WorkspaceId::from(
        created["id"]
            .as_str()
            .expect("Shared Workspace")
            .to_string(),
    );
    let (status, agent) = call(
        e,
        false,
        "POST",
        &format!("/api/v1/workspaces/{workspace}/agents"),
        json!({"name":"Aria", "job":"Review", "personality":""}),
        &e.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{agent}");
    (
        workspace,
        owner,
        AgentId::from(agent["id"].as_str().unwrap().to_string()),
    )
}
#[tokio::test]
async fn manager_changes_keep_versions_and_the_person_author() {
    let e = Embedder::new().await;
    let (workspace, owner, agent) = team(&e).await;
    let path = format!("/api/v1/workspaces/{workspace}/agents/{agent}");
    let body = json!({"name":"Aria", "job":"Answer", "personality":"Kind", "standing_brief":"Resolve requests"});
    assert_eq!(
        call(&e, false, "PUT", &path, body.clone(), &e.cookie)
            .await
            .0,
        StatusCode::OK
    );
    let (status, versions) = call(
        &e,
        false,
        "GET",
        &format!("{path}/versions"),
        Value::Null,
        &e.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(versions["items"][0]["version"], 2);
    assert_eq!(
        versions["items"][0]["author"],
        json!({"kind":"person", "person_id":owner})
    );
    assert_eq!(
        versions["items"][0]["configuration"]["agent"]["job"],
        "Answer"
    );
    let entries = e
        .stores
        .audit
        .list(&e.routers.state.org_workspace_id, 0, 100)
        .await
        .unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.action == "agent.changed"
                && entry.actor == Author::Person(owner.clone())
                && entry.subject_workspace_id == workspace)
            .count(),
        2
    );
    let reader = e
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
            workspace_id: workspace.clone(),
            person_id: reader,
            role: WorkspaceRole::Reader,
            author: Author::Person(owner),
        })
        .await
        .unwrap();
    assert_eq!(
        call(&e, false, "PUT", &path, body, &e.other_cookie).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &e,
            false,
            "GET",
            &format!("{path}/versions"),
            Value::Null,
            &e.other_cookie
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
#[tokio::test]
async fn administrator_limits_refuse_a_manager_and_block_plugin_grants() {
    let e = Embedder::new().await;
    let (workspace, owner, agent) = team(&e).await;
    let path = format!("/api/v1/administration/workspaces/{workspace}/limits");
    let limits = json!({"permitted_plugins":[], "permitted_connections":[], "max_mail_visibility":"agent_only"});
    let (status, saved) = call(&e, true, "PUT", &path, limits.clone(), &e.cookie).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert!(saved.get("monthly_spend_cap_usd").is_none());
    assert_eq!(
        call(
            &e,
            true,
            "PUT",
            &format!("/api/v1/administration/workspaces/{workspace}/spend-cap"),
            json!({"monthly_spend_cap_usd":5}),
            &e.cookie
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        e.stores
            .workspaces
            .get(&workspace)
            .await
            .unwrap()
            .unwrap()
            .monthly_spend_cap_usd,
        Some(5.0)
    );
    let audit = e
        .stores
        .audit
        .list(&e.routers.state.org_workspace_id, 0, 100)
        .await
        .unwrap();
    assert!(
        audit
            .iter()
            .any(|entry| entry.action == "workspace.limits_changed"
                && entry.actor == Author::Person(owner.clone())
                && entry.subject_workspace_id == workspace)
    );
    assert_eq!(
        call(&e, false, "PUT", &path, json!({}), &e.cookie).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &e,
            true,
            "PUT",
            &format!("/api/v1/administration/workspaces/{workspace}/spend-cap"),
            json!({"monthly_spend_cap_usd":1}),
            &e.other_cookie
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&e, true, "PUT", &path, limits, &e.other_cookie)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&e, true, "GET", &path, Value::Null, &e.other_cookie)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, error) = call(
        &e,
        false,
        "POST",
        &format!("/api/v1/workspaces/{workspace}/plugins/blocked/grants"),
        json!({"agent_id":agent}),
        &e.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert!(error.to_string().contains("Shared Workspace Limits"));
    let (status, error) = call(
        &e,
        false,
        "POST",
        &format!("/api/v1/workspaces/{workspace}/grants"),
        json!({"agent_id":agent, "connection_id":"blocked", "capabilities":["read"]}),
        &e.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert!(error.to_string().contains("Shared Workspace Limits"));
}

#[tokio::test]
async fn a_member_manager_changes_grants_and_versions_require_membership() {
    let e = Embedder::new().await;
    let private_agent = e.agent(&e.workspace, "Private").await;
    assert!(
        e.stores
            .governance
            .versions(&e.workspace, &private_agent.id)
            .await
            .unwrap()
            .is_empty()
    );
    let (workspace, owner, agent) = team(&e).await;
    let manager = e
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
            workspace_id: workspace.clone(),
            person_id: manager.clone(),
            role: WorkspaceRole::Manager,
            author: Author::Person(owner.clone()),
        })
        .await
        .unwrap();
    let grant = Grant {
        id: GrantId::generate(),
        workspace_id: workspace.clone(),
        agent_id: agent.clone(),
        resource_kind: Grant::PLUGIN_KIND.into(),
        resource_id: Some("example".into()),
        scope: json!({}),
        revision: 1,
        created_by: Author::Person(owner.clone()),
        created_at: 1,
        revoked_at: None,
    };
    e.stores.grants.create(&grant).await.unwrap();
    assert_eq!(
        call(
            &e,
            false,
            "DELETE",
            &format!("/api/v1/workspaces/{workspace}/grants/{}", grant.id),
            Value::Null,
            &e.other_cookie
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let versions = e
        .stores
        .governance
        .versions(&workspace, &agent)
        .await
        .unwrap();
    assert_eq!(versions[0].author, Author::Person(manager));
    assert_eq!(versions[0].configuration["grants"], json!([]));
    e.stores
        .workspace_members
        .remove(&workspace, &owner)
        .await
        .unwrap();
    assert_eq!(
        call(
            &e,
            false,
            "GET",
            &format!("/api/v1/workspaces/{workspace}/agents/{agent}/versions"),
            Value::Null,
            &e.cookie
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_responder_approves_once_but_only_a_manager_sets_standing_rules() {
    let e = Embedder::new().await;
    let (workspace, owner, agent) = team(&e).await;
    let responder = e
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
            workspace_id: workspace.clone(),
            person_id: responder.clone(),
            role: WorkspaceRole::Responder,
            author: Author::Person(owner),
        })
        .await
        .unwrap();
    let request = pagis_core::Request {
        id: RequestId::generate(),
        workspace_id: workspace.clone(),
        agent_id: agent.clone(),
        run_id: None,
        kind: pagis_core::Request::CREDENTIAL_ACTION_KIND.into(),
        payload: json!({"proposed_rules":["example.com"]}),
        state: RequestState::Pending,
        values: None,
        decided_by: None,
        decided_at: None,
        created_at: 1,
    };
    e.stores.requests.create(&request).await.unwrap();
    let path = format!(
        "/api/v1/workspaces/{workspace}/requests/{}/decision",
        request.id
    );
    assert_eq!(
        call(
            &e,
            false,
            "POST",
            &path,
            json!({"decision":"approved", "scope":"always"}),
            &e.other_cookie
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        e.stores
            .requests
            .get(&workspace, &request.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        RequestState::Pending
    );
    assert_eq!(
        call(
            &e,
            false,
            "POST",
            &path,
            json!({"decision":"approved", "scope":"once"}),
            &e.other_cookie
        )
        .await
        .0,
        StatusCode::OK
    );
    let versions = e
        .stores
        .governance
        .versions(&workspace, &agent)
        .await
        .unwrap();
    assert_eq!(versions[0].author, Author::Person(responder));
    assert_eq!(
        versions[0].configuration["grants"][0]["resource_kind"],
        "credential"
    );
}

#[tokio::test]
async fn mailbox_approval_rules_are_in_the_agent_version() {
    let e = Embedder::new().await;
    let (workspace, owner, agent) = team(&e).await;
    let mailbox = AgentMailbox {
        id: AgentMailboxId::generate(),
        workspace_id: workspace.clone(),
        agent_id: agent.clone(),
        connection_id: ConnectionId::generate(),
        address: "aria@example.com".into(),
        state: AgentMailboxState::Active,
        reason: None,
        outgoing_cap: 100,
        allow_rules: vec![],
        cursor: None,
        sends_day: None,
        sends_today: 0,
        created_at: 1,
        deleted_at: None,
    };
    e.stores.agent_mailboxes.create(&mailbox).await.unwrap();
    let request = pagis_core::Request {
        id: RequestId::generate(),
        workspace_id: workspace.clone(),
        agent_id: agent.clone(),
        run_id: None,
        kind: pagis_core::Request::TOOL_ACTION_KIND.into(),
        payload: json!({"tool_name":"mail__send", "mailbox_id":mailbox.id, "proposed_rules":["example.com"]}),
        state: RequestState::Pending,
        values: None,
        decided_by: None,
        decided_at: None,
        created_at: 1,
    };
    e.stores.requests.create(&request).await.unwrap();
    let (status, body) = call(
        &e,
        false,
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace}/requests/{}/decision",
            request.id
        ),
        json!({"decision":"approved", "scope":"always"}),
        &e.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let versions = e
        .stores
        .governance
        .versions(&workspace, &agent)
        .await
        .unwrap();
    assert_eq!(versions[0].author, Author::Person(owner));
    assert_eq!(
        versions[0].configuration["mailbox"]["allow_rules"],
        json!(["example.com"])
    );
}

#[tokio::test]
async fn an_administrator_with_reader_membership_cannot_read_versions() {
    let e = Embedder::new().await;
    let (workspace, owner, agent) = team(&e).await;
    let manager = e
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
            workspace_id: workspace.clone(),
            person_id: manager,
            role: WorkspaceRole::Manager,
            author: Author::Person(owner.clone()),
        })
        .await
        .unwrap();
    e.stores
        .workspace_members
        .change(&WorkspaceMember {
            workspace_id: workspace.clone(),
            person_id: owner.clone(),
            role: WorkspaceRole::Reader,
            author: Author::Person(owner.clone()),
        })
        .await
        .unwrap();
    assert_eq!(
        call(
            &e,
            false,
            "GET",
            &format!("/api/v1/workspaces/{workspace}/agents/{agent}/versions"),
            Value::Null,
            &e.cookie
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn concurrent_fake_versions_have_unique_numbers() {
    let e = Embedder::new().await;
    let (workspace, person, agent) = team(&e).await;
    let results = futures::future::join_all((0..8).map(|i| {
        let stores = e.stores.clone();
        let workspace = workspace.clone();
        let agent = agent.clone();
        let person = person.clone();
        let org = e.routers.state.org_workspace_id.clone();
        async move {
            stores
                .governance
                .record_version(
                    &agent,
                    &json!({"job":i}),
                    &AuditEntry {
                        id: AuditEntryId::generate(),
                        org_workspace_id: org,
                        actor: Author::Person(person),
                        action: "agent.changed".into(),
                        subject_workspace_id: workspace,
                        reason: None,
                        at: 1,
                    },
                )
                .await
                .unwrap()
                .version
        }
    }))
    .await;
    let mut numbers = results;
    numbers.sort();
    assert_eq!(numbers, (2..=9).collect::<Vec<_>>());
}

#[tokio::test]
async fn configuration_writes_record_only_the_changed_shared_agent() {
    let e = Embedder::new().await;
    let private = e.agent(&e.workspace, "Private").await;
    let body = json!({"name":"Private", "job":"Answer", "personality":"Kind"});
    assert_eq!(
        call(
            &e,
            false,
            "PUT",
            &format!("/api/v1/agents/{}", private.id),
            body,
            &e.cookie
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        e.stores
            .governance
            .versions(&e.workspace, &private.id)
            .await
            .unwrap()
            .is_empty()
    );
    let (workspace, _, agent) = team(&e).await;
    let other = e.agent(&workspace, "Other").await;
    let path = format!("/api/v1/workspaces/{workspace}/agents/{agent}");
    assert_eq!(
        call(
            &e,
            false,
            "PUT",
            &format!("{path}/appearance"),
            serde_json::to_value(&other.avatar).unwrap(),
            &e.cookie
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        e.stores
            .governance
            .versions(&workspace, &agent)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        e.stores
            .governance
            .versions(&workspace, &other.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        call(
            &e,
            false,
            "PUT",
            &format!("/api/v1/workspaces/{workspace}/workspace/timezone"),
            json!({"timezone":"UTC"}),
            &e.cookie
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        e.stores
            .governance
            .versions(&workspace, &agent)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        call(
            &e,
            false,
            "POST",
            &format!("{path}/archive"),
            Value::Null,
            &e.cookie
        )
        .await
        .0,
        StatusCode::OK
    );
    let versions = e
        .stores
        .governance
        .versions(&workspace, &agent)
        .await
        .unwrap();
    assert_eq!(versions.len(), 3);
    assert_eq!(versions[0].configuration["agent"]["status"], "archived");
}

#[tokio::test]
async fn workspace_mailbox_grant_creation_rules_and_revocation_have_versions() {
    let e = Embedder::new().await;
    let (workspace, owner, agent) = team(&e).await;
    e.stores
        .workspace_mailboxes
        .create(&WorkspaceMailbox {
            workspace_id: workspace.clone(),
            connection_id: ConnectionId::generate(),
            address: "support@example.com".into(),
            outgoing_cap: 100,
            visibility: pagis_core::MailVisibility::AllMail,
            spam_mode: MailScoringMode::ScoreOnly,
            created_at: 1,
            author: Author::Person(owner.clone()),
        })
        .await
        .unwrap();
    let prefix = format!("/api/v1/workspaces/{workspace}");
    let (status, grant) = call(
        &e,
        false,
        "POST",
        &format!("{prefix}/grants/workspace-mailbox"),
        json!({"agent_id":agent}),
        &e.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let id = grant["id"].as_str().unwrap();
    assert_eq!(
        call(
            &e,
            false,
            "PUT",
            &format!("{prefix}/grants/{id}/rules"),
            json!({"allow":["example.com"]}),
            &e.cookie
        )
        .await
        .0,
        StatusCode::OK
    );
    let versions = e
        .stores
        .governance
        .versions(&workspace, &agent)
        .await
        .unwrap();
    assert_eq!(versions.len(), 3);
    assert_eq!(versions[0].author, Author::Person(owner));
    assert_eq!(
        versions[0].configuration["grants"][0]["scope"]["allow"],
        json!(["example.com"])
    );
    assert_eq!(
        call(
            &e,
            false,
            "DELETE",
            &format!("{prefix}/grants/{id}"),
            Value::Null,
            &e.cookie
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let versions = e
        .stores
        .governance
        .versions(&workspace, &agent)
        .await
        .unwrap();
    assert_eq!(versions.len(), 4);
    assert_eq!(versions[0].configuration["grants"], json!([]));
}

#[tokio::test]
async fn workspace_mail_visibility_checks_every_human_read_and_current_limits() {
    use pagis_mail::fake::MemoryWorkspaceMailHost;
    let host = std::sync::Arc::new(MemoryWorkspaceMailHost::default());
    let e = Embedder::with_workspace_mail(host.clone()).await;
    let (workspace, owner, agent) = team(&e).await;
    let connection = ConnectionId::generate();
    e.stores.connections.create(&Connection {
        id: connection.clone(), workspace_id: e.routers.state.org_workspace_id.clone(),
        provider: "embedder-mail".into(), alias: "mail".into(), display_name: "Mail".into(),
        status: Connection::CONNECTED.into(), authorized_capabilities: vec![],
        config: json!({"domain":"example.com","capabilities":{"idle":false,"outgoing_cap":true,"delete_mailbox":true,"reset_password":true}}), created_at: 0,
    }).await.unwrap();
    let prefix = format!("/api/v1/workspaces/{workspace}/workspace");
    let (status, body) = call(
        &e,
        false,
        "POST",
        &format!("{prefix}/mailbox"),
        json!({"connection_id":connection,"local_part":"support"}),
        &e.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let mail = host.store(&workspace).unwrap();
    let hidden = mail.deliver("sender@example.com", "Hidden", "private body");
    let visible = mail.deliver("sender@example.com", "Escalated", "human body");
    mail.attach(
        &visible,
        pagis_mail::OutgoingAttachment {
            name: "note.txt".into(),
            content_type: "text/plain".into(),
            data: b"note".to_vec(),
        },
    );
    let (status, page) = call(
        &e,
        false,
        "GET",
        &format!("{prefix}/mail/messages"),
        Value::Null,
        &e.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let thread = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == visible.as_str())
        .unwrap()["thread_id"]
        .as_str()
        .unwrap();
    // Agent triage and assignments store the same escalation identity.
    let summary = e
        .routers
        .state
        .mailbox_desk
        .open_workspace_for_agent(&workspace, &agent)
        .await;
    assert!(summary.is_err(), "an Agent needs its Grant at every level");
    let mut store = e
        .routers
        .state
        .mailbox_desk
        .open_workspace(&workspace)
        .await
        .unwrap();
    let message = store.message(&visible).await.unwrap();
    e.stores
        .workspace_mailboxes
        .escalate(&workspace, message.message.summary.assessment_key(), &agent)
        .await
        .unwrap();
    store.close().await.unwrap();
    for level in ["escalations", "agent_only", "all_mail"] {
        let (status, body) = call(
            &e,
            false,
            "PUT",
            &format!("{prefix}/mailbox/visibility"),
            json!({"visibility":level}),
            &e.cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, page) = call(
            &e,
            false,
            "GET",
            &format!("{prefix}/mail/messages"),
            Value::Null,
            &e.cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            page["items"].as_array().unwrap().len(),
            match level {
                "all_mail" => 2,
                "escalations" => 1,
                _ => 0,
            }
        );
        for (id, allowed) in [
            (&visible, level != "agent_only"),
            (&hidden, level == "all_mail"),
        ] {
            for suffix in [
                format!("messages/{id}"),
                format!("messages/{id}/composer?action=reply"),
            ] {
                let (status, body) = call(
                    &e,
                    false,
                    "GET",
                    &format!("{prefix}/mail/{suffix}"),
                    Value::Null,
                    &e.cookie,
                )
                .await;
                assert_eq!(
                    status,
                    if allowed {
                        StatusCode::OK
                    } else {
                        StatusCode::NOT_FOUND
                    },
                    "{body}"
                );
            }
        }
        let encoded_thread: String =
            url::form_urlencoded::byte_serialize(thread.as_bytes()).collect();
        let (status, _) = call(
            &e,
            false,
            "GET",
            &format!("{prefix}/mail/threads/{encoded_thread}"),
            Value::Null,
            &e.cookie,
        )
        .await;
        assert_eq!(
            status,
            if level == "agent_only" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::OK
            }
        );
        let (status, _) = call(
            &e,
            false,
            "GET",
            &format!("{prefix}/mail/messages/{visible}/attachments/0"),
            Value::Null,
            &e.cookie,
        )
        .await;
        assert_eq!(
            status,
            if level == "agent_only" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::OK
            }
        );
        let (status, global) = call(
            &e,
            false,
            "GET",
            "/api/v1/search?q=body",
            Value::Null,
            &e.cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{global}");
        assert_eq!(
            global["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["kind"] == "mail")
                .count(),
            if level == "all_mail" { 2 } else { 0 }
        );
        let (_, global) = call(
            &e,
            false,
            "GET",
            "/api/v1/search?q=body&reason=Customer%20request",
            Value::Null,
            &e.cookie,
        )
        .await;
        assert_eq!(
            global["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["kind"] == "mail")
                .count(),
            match level {
                "all_mail" => 2,
                "escalations" => 1,
                _ => 0,
            }
        );
        let (status, _) = call(
            &e,
            false,
            "GET",
            &format!("{prefix}/mail/messages?q=body"),
            Value::Null,
            &e.cookie,
        )
        .await;
        assert_eq!(
            status,
            if level == "escalations" {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                StatusCode::OK
            }
        );
    }
    call(
        &e,
        false,
        "PUT",
        &format!("{prefix}/mailbox/visibility"),
        json!({"visibility":"escalations"}),
        &e.cookie,
    )
    .await;
    let (status, page) = call(
        &e,
        false,
        "GET",
        &format!("{prefix}/mail/messages?q=body&reason=Customer%20request"),
        Value::Null,
        &e.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let (_, audit) = call(
        &e,
        false,
        "GET",
        &format!("{prefix}/mail/audit"),
        Value::Null,
        &e.cookie,
    )
    .await;
    let entries = audit["items"].as_array().unwrap();
    for action in [
        "mail.message.read",
        "mail.thread.read",
        "mail.attachment.read",
        "mail.search",
        "mail.composer.read",
    ] {
        assert!(
            entries.iter().any(|entry| entry["action"] == action
                && entry["actor"] == serde_json::to_value(Author::Person(owner.clone())).unwrap()
                && entry["subject_workspace_id"] == workspace.as_str()),
            "{action}: {audit}"
        );
    }
    assert!(entries.iter().any(|entry| {
        entry["reason"]
            .as_str()
            .unwrap()
            .contains("Customer request")
    }));
    let other = e
        .stores
        .workspaces
        .get(&e.other_workspace)
        .await
        .unwrap()
        .unwrap()
        .user_id
        .unwrap();
    let members = format!("/api/v1/workspaces/{workspace}/members");
    assert_eq!(
        call(
            &e,
            false,
            "POST",
            &members,
            json!({"person_id":other,"role":"reader"}),
            &e.cookie
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    for role in ["reader", "responder"] {
        assert_eq!(
            call(
                &e,
                false,
                "PATCH",
                &format!("{members}/{other}"),
                json!({"role":role}),
                &e.cookie
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            call(
                &e,
                false,
                "GET",
                &format!("{prefix}/mail/messages/{visible}"),
                Value::Null,
                &e.other_cookie
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            call(
                &e,
                false,
                "GET",
                &format!("{prefix}/mail/messages/{hidden}"),
                Value::Null,
                &e.other_cookie
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(
                &e,
                false,
                "PUT",
                &format!("{prefix}/mailbox/visibility"),
                json!({"visibility":"all_mail"}),
                &e.other_cookie
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(
                &e,
                false,
                "GET",
                &format!("{prefix}/mail/audit"),
                Value::Null,
                &e.other_cookie
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        let (status, sent) = call(
            &e,
            false,
            "POST",
            &format!("{prefix}/mail/messages"),
            json!({"to":["customer@example.com"],"subject":"Reply","body":"Answer"}),
            &e.other_cookie,
        )
        .await;
        assert_eq!(
            status,
            if role == "reader" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::CREATED
            },
            "{sent}"
        );
        let disposable = mail.deliver("sender@example.com", "Delete", "Disposable mail");
        let (status, body) = call(
            &e,
            false,
            "DELETE",
            &format!("{prefix}/mail/messages/{disposable}"),
            Value::Null,
            &e.other_cookie,
        )
        .await;
        assert_eq!(
            status,
            if role == "reader" {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::NO_CONTENT
            },
            "{body}"
        );
        if role == "responder" {
            let (_, audit) = call(
                &e,
                false,
                "GET",
                &format!("{prefix}/mail/audit"),
                Value::Null,
                &e.cookie,
            )
            .await;
            assert!(audit["items"].as_array().unwrap().iter().any(|entry| {
                entry["action"] == "mail.sent"
                    && entry["actor"]
                        == serde_json::to_value(Author::Person(other.clone())).unwrap()
                    && entry["reason"]
                        .as_str()
                        .unwrap()
                        .contains(sent["message_id"].as_str().unwrap())
            }));
        }
    }
    let (status, body) = call(&e, true, "PUT", &format!("/api/v1/administration/workspaces/{workspace}/limits"), json!({"permitted_plugins":null,"permitted_connections":null,"max_mail_visibility":"agent_only"}), &e.cookie).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, page) = call(
        &e,
        false,
        "GET",
        &format!("{prefix}/mailbox"),
        Value::Null,
        &e.cookie,
    )
    .await;
    assert_eq!(page["mailbox"]["visibility"], "escalations");
    assert_eq!(page["mailbox"]["effective_visibility"], "agent_only");
    assert_eq!(
        call(
            &e,
            false,
            "GET",
            &format!("{prefix}/mail/messages/{visible}"),
            Value::Null,
            &e.cookie
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &e,
            false,
            "PUT",
            &format!("{prefix}/mailbox/visibility"),
            json!({"visibility":"all_mail"}),
            &e.cookie
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, org_audit) = call(
        &e,
        true,
        "GET",
        "/api/v1/admin/audit",
        Value::Null,
        &e.cookie,
    )
    .await;
    assert!(
        org_audit["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["action"] == "mail.message.read")
    );
}
