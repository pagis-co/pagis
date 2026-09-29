//! The knowledge-invalidation trait tests of the suite.
//!
//! Each body is one test. It takes a [`Backend`], reads the store set
//! from it, and never names a pool type, so it runs on both backends
//! from one text. Name every new body in `store_suite_knowledge_events!`
//! below: the guard test of the parent module fails while one is
//! missing.

use pagis_core::WorkspaceId;

use super::Backend;

pub async fn knowledge_notifications_survive_restart_and_acknowledge_only_their_workspace(
    backend: &Backend,
) {
    backend
        .execute(
            "INSERT INTO knowledge_invalidations(id,workspace_id,connection_id,resource,reason) \
             VALUES (1,'owner','personal','gmail','source_changed'),\
             (2,'owner','work','gmail','claims_changed'),\
             (3,'other','private','gmail','source_changed')",
            &[],
        )
        .await
        .expect("plant the invalidations");
    let owner = WorkspaceId::from("owner".to_string());
    let store = &backend.stores().knowledge;
    let first = store.pending_invalidations(&owner, 1).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, 1);
    assert_eq!(first[0].source.connection_id.as_str(), "personal");
    // A failed publication has no acknowledgment, so a worker that reads
    // again after a restart finds the same record. The store holds a
    // pool and no state of its own, so this read is that read.
    assert_eq!(store.pending_invalidations(&owner, 1).await.unwrap(), first);
    store.acknowledge_invalidation(&owner, 3).await.unwrap();
    assert_eq!(
        store
            .pending_invalidations(&WorkspaceId::from("other".to_string()), 10)
            .await
            .unwrap()
            .len(),
        1
    );
    store
        .acknowledge_invalidation(&owner, first[0].id)
        .await
        .unwrap();
    let remaining = store.pending_invalidations(&owner, 10).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, 2);
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_knowledge_events {
    ($emit:path) => {
        $emit!(
            knowledge_events,
            knowledge_notifications_survive_restart_and_acknowledge_only_their_workspace,
        );
    };
}
