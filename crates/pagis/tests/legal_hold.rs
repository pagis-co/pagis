//! A Legal Hold preserves Calendar Events through the full daemon.
use pagis_testkit::TestDaemon;
use serde_json::{Value, json};

#[tokio::test]
async fn legal_hold_preserves_events_and_records_each_change() {
    let daemon = TestDaemon::start_in_memory().await;
    let client = daemon.client();
    let events = format!("{}/api/v1/calendar/events", daemon.base_url);
    let hold = format!(
        "{}/api/v1/admin/workspaces/{}/legal-hold",
        daemon.administration_base_url, daemon.workspace_id
    );
    let created = client
        .post(&events)
        .header("cookie", daemon.cookie())
        .json(&json!({
            "title": "Preserved", "description": "", "location": "",
            "start": "2026-03-07T09:00:00", "end": "2026-03-07T10:00:00",
            "all_day": false, "timezone": "UTC", "rrule": null, "exdates": [],
            "attendees": [], "reminders": [], "meeting_url": null
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let event: Value = created.json().await.unwrap();
    let item = format!("{}/{}", events, event["id"].as_str().unwrap());
    let started = client
        .put(&hold)
        .header("cookie", daemon.cookie())
        .json(&json!({"reason": "Preserve the records"}))
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 200);
    let refused = client
        .delete(&item)
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 409);
    assert_eq!(
        refused.json::<Value>().await.unwrap()["error"]["message"],
        pagis_core::LegalHold::MESSAGE
    );
    assert_eq!(
        client
            .get(&item)
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .delete(&hold)
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .delete(&item)
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let audit = client
        .get(format!(
            "{}/api/v1/admin/audit",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(audit.status(), 200);
    let audit: Value = audit.json().await.unwrap();
    assert_eq!(audit["items"][0]["action"], "legal_hold.ended");
    assert_eq!(audit["items"][1]["action"], "legal_hold.started");
    assert_eq!(audit["items"][1]["reason"], "Preserve the records");
}

async fn hold(daemon: &TestDaemon, start: bool) {
    let url = format!(
        "{}/api/v1/admin/workspaces/{}/legal-hold",
        daemon.administration_base_url, daemon.workspace_id
    );
    let client = daemon.client();
    let request = if start {
        client.put(url).json(&json!({"reason":"Preserve records"}))
    } else {
        client.delete(url)
    };
    let response = request
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
}

#[tokio::test]
async fn legal_hold_preserves_retention_forget_and_workspace_mail() {
    use object_store::ObjectStoreExt as _;
    use pagis_core::{Connection, ConnectionId, FileKind};
    use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost};
    use pagis_testkit::{TestDaemonOptions, fixture};
    use std::sync::Arc;
    let transport = Arc::new(FakeMailTransport::default());
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        mail_host: Arc::new(FakeMailboxHost::default()),
        mail_transport: transport.clone(),
        ..Default::default()
    })
    .await;
    let client = daemon.client();
    let connection = daemon
        .connect_installation(
            "migadu",
            json!({"account":"owner@example.com", "api_key":"key", "domain":"example.com"}),
        )
        .await;
    let response = client
        .post(format!("{}/api/v1/workspace/mailbox", daemon.base_url))
        .header("cookie", daemon.cookie())
        .json(&json!({"connection_id":connection, "local_part":"held"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let mail = format!("{}/api/v1/workspace/mail", daemon.base_url);
    assert_eq!(
        client
            .post(format!("{mail}/folders"))
            .header("cookie", daemon.cookie())
            .json(&json!({"name":"Records"}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    transport.deliver("sender@example.com", "Preserved mail", "The record");
    let id = transport.summaries("INBOX")[0].id.to_string();
    assert_eq!(
        client
            .post(format!("{mail}/messages/{id}/move"))
            .header("cookie", daemon.cookie())
            .json(&json!({"name":"Records"}))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let stores = daemon.stores();
    let source = Connection {
        id: ConnectionId::generate(),
        workspace_id: daemon.workspace_id.clone(),
        provider: "google".into(),
        alias: "source".into(),
        display_name: "Source".into(),
        status: Connection::CONNECTED.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: json!({}),
        created_at: 1,
    };
    stores.connections.create(&source).await.unwrap();
    use pagis_core::knowledge::{SourceBatch, SourceChange, SourceOperation, SyncConfig};
    let agent = pagis_core::AgentId::from(daemon.agent_id.clone());
    let mut grant = fixture::grant(&daemon.workspace_id, &agent, "connection", &[]);
    grant.resource_id = Some(source.id.to_string());
    grant.scope = pagis_core::Grant::connection_scope(&["gmail_read".into()]);
    stores.grants.create(&grant).await.unwrap();
    let sync = stores
        .knowledge
        .configure(
            SyncConfig {
                workspace_id: daemon.workspace_id.clone(),
                connection_id: source.id.clone(),
                resource: "gmail".into(),
                agent_id: agent,
                required_capability: "gmail_read".into(),
                enabled: true,
                since: 0,
                filter: pagis_core::reflection_filter::ReflectionFilter {
                    rules: vec![],
                    default: pagis_core::reflection_filter::Verdict::Skip,
                },
            },
            1,
        )
        .await
        .unwrap();
    let key = sync.key();
    stores
        .knowledge
        .acquire(
            &key,
            &SourceBatch {
                historical: true,
                arrivals: vec![],
                expected_revision: sync.cursor_revision,
                checkpoint: json!({}),
                caught_up: true,
                changes: vec![SourceChange {
                    id: "preserved-source".into(),
                    version: "1".into(),
                    kind: "mail".into(),
                    parent: Some("thread".into()),
                    source_at: Some(1),
                    operation: SourceOperation::Upsert,
                    metadata: json!({"subject":"Preserved source"}),
                }],
            },
            1,
            daemon.state.forget_keys.as_ref(),
        )
        .await
        .unwrap();

    let forget = format!("{}/api/v1/knowledge/forget", daemon.base_url);
    let target = json!({"kind":"account", "connection_id":source.id});
    let preview: Value = client
        .post(format!("{forget}/preview"))
        .header("cookie", daemon.cookie())
        .json(&json!({"target":target}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut file = fixture::file(&daemon.workspace_id, "held-retention");
    file.kind = FileKind::Screenshot;
    file.created_at = 1;
    stores.files.insert(&file).await.unwrap();
    let blob = object_store::path::Path::from(file.storage_key.clone());
    daemon
        .state
        .blobs
        .put(&blob, b"record".to_vec().into())
        .await
        .unwrap();
    stores
        .retention_policies
        .set(&daemon.workspace_id, file.kind, Some(1), 1)
        .await
        .unwrap();
    let deps = pagis::retention::RetentionDeps {
        workspaces: stores.workspaces.clone(),
        policies: stores.retention_policies.clone(),
        files: stores.files.clone(),
        blobs: daemon.state.blobs.clone(),
        capture: daemon.state.capture.clone(),
        captures: stores.model_request_captures.clone(),
    };
    hold(&daemon, true).await;
    assert_eq!(
        pagis::retention::sweep(&deps, pagis_core::now_ms())
            .await
            .unwrap(),
        0
    );
    assert!(
        stores
            .files
            .get(&daemon.workspace_id, &file.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(daemon.state.blobs.head(&blob).await.is_ok());
    for response in [
        client
            .delete(format!("{mail}/folders/Records"))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap(),
        client
            .post(&forget)
            .header("cookie", daemon.cookie())
            .json(&json!({"target":target,"preview":preview}))
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(response.status(), 409);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"]["message"],
            pagis_core::LegalHold::MESSAGE
        );
    }
    assert!(
        stores
            .forget
            .raw_connection_allowed(&daemon.workspace_id, &source.id)
            .await
            .unwrap()
    );
    let kept: Value = client
        .get(format!("{mail}/messages?folder=Records"))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(kept["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        stores.knowledge.versions(&key, 0, 10).await.unwrap().len(),
        1
    );
    hold(&daemon, false).await;
    assert_eq!(
        pagis::retention::sweep(&deps, pagis_core::now_ms())
            .await
            .unwrap(),
        1
    );
    assert!(
        stores
            .files
            .get(&daemon.workspace_id, &file.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(daemon.state.blobs.head(&blob).await.is_err());
    assert_eq!(
        client
            .delete(format!("{mail}/folders/Records"))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(
        client
            .post(&forget)
            .header("cookie", daemon.cookie())
            .json(&json!({"target":target,"preview":preview}))
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !stores
            .knowledge
            .versions(&key, 0, 10)
            .await
            .unwrap()
            .is_empty()
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn legal_hold_refuses_an_agent_memory_delete_until_the_hold_ends() {
    use pagis_core::{AgentId, MemoryAccess, MemoryAuthor, MemoryChangeset, ScopedPath};
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use std::sync::Arc;
    let brain = Arc::new(ScriptedBrain::default());
    let daemon = TestDaemon::start_in_memory_with(TestDaemonOptions {
        brain: brain.clone(),
        ..Default::default()
    })
    .await;
    let agent = AgentId::from(daemon.agent_id.clone());
    let access = MemoryAccess::Owner {
        agent_id: agent.clone(),
    };
    let path = ScopedPath::parse("shared/record.md").unwrap();
    let mut changeset = MemoryChangeset::default();
    changeset
        .writes
        .insert(path.clone(), "Preserved record".into());
    let revision = daemon
        .state
        .memory
        .commit(
            &daemon.workspace_id,
            &agent,
            &access,
            &MemoryAuthor {
                name: "Person".into(),
                email: "person@example.com".into(),
            },
            &changeset,
            None,
            "Record",
            None,
        )
        .await
        .unwrap();
    hold(&daemon, true).await;
    for (pending, held) in [("held-delete", true), ("released-delete", false)] {
        brain.push(Script::tool_call(
            &[],
            "memory_delete",
            json!({"path":"shared/record.md", "expected_memory_revision":revision}),
        ));
        brain.push(Script::reply(&["Done."]));
        let before = daemon
            .state
            .runs
            .list(&daemon.workspace_id, Some(&agent), None, &[], None, 100)
            .await
            .unwrap();
        let response = daemon
            .client()
            .post(format!(
                "{}/api/v1/channels/{}/messages",
                daemon.base_url, daemon.dm_channel_id
            ))
            .header("cookie", daemon.cookie())
            .json(&json!({"pending_id":pending,"text":"Delete the record."}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let runs = daemon
                    .state
                    .runs
                    .list(&daemon.workspace_id, Some(&agent), None, &[], None, 100)
                    .await
                    .unwrap();
                if runs.iter().any(|run| {
                    !before.iter().any(|r| r.id == run.id)
                        && run.state == pagis_core::RunState::Completed
                }) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let read = daemon
            .state
            .memory
            .read(&daemon.workspace_id, &access, &path)
            .await;
        if held {
            assert!(read.is_ok());
            let requests = format!("{:?}", brain.requests());
            assert!(
                requests.contains(pagis_core::LegalHold::MESSAGE),
                "{requests}"
            );
            hold(&daemon, false).await;
        } else {
            assert!(read.is_err());
        }
    }
}

#[tokio::test]
async fn legal_hold_and_audit_require_an_administrator() {
    use pagis_core::{User, UserRole, WorkspaceId};
    let daemon = TestDaemon::start_in_memory().await;
    let stores = daemon.stores();
    let org = stores.orgs.list().await.unwrap().remove(0);
    let person = User::new(org.id, UserRole::Member, 1);
    stores.users.create(&person).await.unwrap();
    let workspace = pagis_core::Workspace {
        id: WorkspaceId::generate(),
        user_id: Some(person.id.clone()),
        ..pagis_testkit::fixture::workspace()
    };
    stores.workspaces.create(&workspace).await.unwrap();
    let cookie = daemon.cookie_for(&person.id).await;
    let client = daemon.client();
    let base = &daemon.administration_base_url;
    for (method, path) in [
        (
            reqwest::Method::PUT,
            format!(
                "/api/v1/admin/workspaces/{}/legal-hold",
                daemon.workspace_id
            ),
        ),
        (
            reqwest::Method::DELETE,
            format!(
                "/api/v1/admin/workspaces/{}/legal-hold",
                daemon.workspace_id
            ),
        ),
        (reqwest::Method::GET, "/api/v1/admin/audit".into()),
    ] {
        assert_eq!(
            client
                .request(method.clone(), format!("{base}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            client
                .request(method, format!("{base}{path}"))
                .header("cookie", &cookie)
                .json(&json!({"reason":"Preserve"}))
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    let hold = format!(
        "{base}/api/v1/admin/workspaces/{}/legal-hold",
        daemon.workspace_id
    );
    assert_eq!(
        client
            .put(hold)
            .header("cookie", daemon.cookie())
            .json(&json!({"reason":"  "}))
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    assert!(
        stores
            .audit
            .list(&daemon.state.org_workspace_id, 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
}
