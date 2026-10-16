//! The Org Audit Log contract on both storage backends.
use super::Backend;
use pagis_core::{AuditEntry, AuditEntryId, Author, WorkspaceId};

pub async fn audit_log_is_append_only_scoped_and_paged(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let stores = backend.stores();
    let org = stores.orgs.list().await.unwrap().remove(0);
    let mut entries = Vec::new();
    for (at, actor) in [
        (30, Author::System),
        (10, Author::Person(workspace.user_id.clone().unwrap())),
        (30, Author::System),
    ] {
        let entry = AuditEntry {
            id: AuditEntryId::generate(),
            org_workspace_id: org.workspace_id.clone(),
            actor,
            action: "record.read".into(),
            subject_workspace_id: workspace.id.clone(),
            reason: None,
            at,
        };
        stores.audit.append(&entry).await.unwrap();
        entries.push(entry);
    }
    assert!(stores.audit.append(&entries[0]).await.is_err());
    assert!(
        stores
            .audit
            .list(&WorkspaceId::generate(), 0, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores.audit.list(&org.workspace_id, 0, 2).await.unwrap(),
        vec![entries[2].clone(), entries[0].clone()]
    );
    assert_eq!(
        stores.audit.list(&org.workspace_id, 2, 2).await.unwrap(),
        vec![entries[1].clone()]
    );
    assert!(
        stores
            .audit
            .list(&org.workspace_id, 0, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn shared_limits_and_agent_versions_are_scoped_and_audited(backend: &Backend) {
    use pagis_core::{AgentId, MailVisibility, SharedWorkspaceLimits, WorkspaceKind};
    let private = backend.seeded_workspace().await;
    let stores = backend.stores();
    let mut workspace = private.clone();
    workspace.id = WorkspaceId::generate();
    workspace.user_id = None;
    workspace.kind = WorkspaceKind::Shared;
    stores.workspaces.create(&workspace).await.unwrap();
    let org = stores.orgs.list().await.unwrap().remove(0);
    let entry = AuditEntry {
        id: AuditEntryId::generate(),
        org_workspace_id: org.workspace_id.clone(),
        actor: Author::Person(private.user_id.unwrap()),
        action: "workspace.limits_changed".into(),
        subject_workspace_id: workspace.id.clone(),
        reason: None,
        at: 10,
    };
    assert_eq!(
        stores.governance.limits(&workspace.id).await.unwrap(),
        SharedWorkspaceLimits::default()
    );
    stores
        .workspaces
        .set_monthly_spend_cap(&workspace.id, Some(12.0))
        .await
        .unwrap();
    let limits = SharedWorkspaceLimits {
        permitted_plugins: Some(vec!["search".into()]),
        permitted_connections: Some(vec![]),
        max_mail_visibility: MailVisibility::Escalations,
    };
    stores.governance.set_limits(&limits, &entry).await.unwrap();
    assert_eq!(
        stores.governance.limits(&workspace.id).await.unwrap(),
        limits
    );
    assert_eq!(
        stores
            .workspaces
            .get(&workspace.id)
            .await
            .unwrap()
            .unwrap()
            .monthly_spend_cap_usd,
        Some(12.0)
    );
    let rejected = SharedWorkspaceLimits {
        permitted_plugins: Some(vec![]),
        ..limits.clone()
    };
    assert!(
        stores
            .governance
            .set_limits(&rejected, &entry)
            .await
            .is_err()
    );
    assert_eq!(
        stores.governance.limits(&workspace.id).await.unwrap(),
        limits
    );
    let agent = pagis_core::Agent {
        id: AgentId::generate(),
        workspace_id: workspace.id.clone(),
        ..crate::fixture::agent(&workspace.id)
    };
    stores.agents.create(&agent).await.unwrap();
    let change = AuditEntry {
        id: AuditEntryId::generate(),
        action: "agent.changed".into(),
        ..entry.clone()
    };
    let first = stores
        .governance
        .record_version(&agent.id, &serde_json::json!({"job": "Review"}), &change)
        .await
        .unwrap();
    assert_eq!(first.version, 1);
    assert_eq!(first.author, change.actor);
    assert!(
        stores
            .governance
            .record_version(&agent.id, &serde_json::json!({}), &change)
            .await
            .is_err()
    );
    let second = stores
        .governance
        .record_version(
            &agent.id,
            &serde_json::json!({"job": "Answer"}),
            &AuditEntry {
                id: AuditEntryId::generate(),
                ..change
            },
        )
        .await
        .unwrap();
    assert_eq!(second.version, 2);
    assert_eq!(
        stores
            .governance
            .versions(&workspace.id, &agent.id)
            .await
            .unwrap(),
        vec![second, first]
    );
    assert!(
        stores
            .governance
            .versions(&private.id, &agent.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        stores
            .audit
            .list(&org.workspace_id, 0, 100)
            .await
            .unwrap()
            .len(),
        3
    );
}

pub async fn lowered_limits_stop_existing_grants_at_use_time(backend: &Backend) {
    use pagis_core::{AgentId, Grant, GrantId, SharedWorkspaceLimits, WorkspaceKind};
    let private = backend.seeded_workspace().await;
    let stores = backend.stores();
    let mut workspace = private.clone();
    workspace.id = WorkspaceId::generate();
    workspace.user_id = None;
    workspace.kind = WorkspaceKind::Shared;
    stores.workspaces.create(&workspace).await.unwrap();
    let agent = crate::fixture::agent(&workspace.id);
    stores.agents.create(&agent).await.unwrap();
    let org = stores.orgs.list().await.unwrap().remove(0);
    for kind in [Grant::PLUGIN_KIND, Grant::CONNECTION_KIND] {
        let grant = Grant {
            id: GrantId::generate(),
            workspace_id: workspace.id.clone(),
            agent_id: agent.id.clone(),
            resource_kind: kind.into(),
            resource_id: Some("resource".into()),
            scope: serde_json::json!({}),
            revision: 1,
            created_by: Author::Person(private.user_id.clone().unwrap()),
            created_at: 10,
            revoked_at: None,
        };
        stores.grants.create(&grant).await.unwrap();
        assert!(
            stores
                .grants
                .live_for_resource(&workspace.id, &agent.id, kind, "resource")
                .await
                .unwrap()
                .is_some()
        );
    }
    stores
        .governance
        .set_limits(
            &SharedWorkspaceLimits {
                permitted_plugins: Some(vec![]),
                permitted_connections: Some(vec![]),
                ..Default::default()
            },
            &AuditEntry {
                id: AuditEntryId::generate(),
                org_workspace_id: org.workspace_id,
                actor: Author::Person(private.user_id.unwrap()),
                action: "workspace.limits_changed".into(),
                subject_workspace_id: workspace.id.clone(),
                reason: None,
                at: 20,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        stores.grants.list_live(&workspace.id).await.unwrap().len(),
        2
    );
    assert!(
        stores
            .grants
            .list_live_for_agent(&workspace.id, &agent.id)
            .await
            .unwrap()
            .is_empty()
    );
    for kind in [Grant::PLUGIN_KIND, Grant::CONNECTION_KIND] {
        assert!(
            stores
                .grants
                .live_for_resource(&workspace.id, &agent.id, kind, "resource")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            stores
                .grants
                .live_grant(&workspace.id, &agent.id, kind)
                .await
                .unwrap()
                .is_none()
        );
    }
    let grant = Grant {
        agent_id: AgentId::generate(),
        ..stores
            .grants
            .list_live(&workspace.id)
            .await
            .unwrap()
            .remove(0)
    };
    assert!(stores.grants.create(&grant).await.is_err());
}

pub async fn concurrent_agent_versions_have_unique_numbers(backend: &Backend) {
    let private = backend.seeded_workspace().await;
    let stores = backend.stores();
    let workspace = pagis_core::Workspace {
        id: WorkspaceId::generate(),
        user_id: None,
        kind: pagis_core::WorkspaceKind::Shared,
        ..private.clone()
    };
    stores.workspaces.create(&workspace).await.unwrap();
    let agent = crate::fixture::agent(&workspace.id);
    stores.agents.create(&agent).await.unwrap();
    let org = stores.orgs.list().await.unwrap().remove(0);
    let person = private.user_id.unwrap();
    let writes = (0..8).map(|i| {
        let agent = agent.id.clone();
        let workspace = workspace.id.clone();
        let org = org.workspace_id.clone();
        let person = person.clone();
        async move {
            stores
                .governance
                .record_version(
                    &agent,
                    &serde_json::json!({"job":i}),
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
    });
    let mut versions = futures::future::join_all(writes).await;
    versions.sort();
    assert_eq!(versions, (1..=8).collect::<Vec<_>>());
    assert_eq!(
        stores
            .audit
            .list(&org.workspace_id, 0, 100)
            .await
            .unwrap()
            .len(),
        8
    );
}

#[macro_export]
macro_rules! store_suite_audit {
    ($emit:path) => {
        $emit!(
            audit,
            audit_log_is_append_only_scoped_and_paged,
            shared_limits_and_agent_versions_are_scoped_and_audited,
            lowered_limits_stop_existing_grants_at_use_time,
            concurrent_agent_versions_have_unique_numbers,
        );
    };
}
