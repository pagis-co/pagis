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

#[macro_export]
macro_rules! store_suite_audit {
    ($emit:path) => {
        $emit!(audit, audit_log_is_append_only_scoped_and_paged,);
    };
}
