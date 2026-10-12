use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use pagis_core::{Agent, AgentId, AgentStore, AvatarAppearance, StoreError, WorkspaceId};
use pagis_testkit::store_suite::{Backend, Rows, stores};

struct CountingAgentStore {
    inner: Arc<dyn AgentStore>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl AgentStore for CountingAgentStore {
    async fn create(&self, agent: &Agent) -> Result<(), StoreError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.create(agent).await
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
    ) -> Result<Option<Agent>, StoreError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.get(workspace_id, id).await
    }

    async fn list_by_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<Agent>, StoreError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.list_by_workspace(workspace_id).await
    }

    async fn update(&self, agent: &Agent) -> Result<(), StoreError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.update(agent).await
    }

    async fn update_avatar(
        &self,
        workspace_id: &WorkspaceId,
        id: &AgentId,
        avatar: &AvatarAppearance,
        updated_at: i64,
    ) -> Result<bool, StoreError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner
            .update_avatar(workspace_id, id, avatar, updated_at)
            .await
    }
}

#[tokio::test]
async fn sqlite_with_runs_a_suite_body_through_the_supplied_store() {
    let calls = Arc::new(AtomicUsize::new(0));
    let builder_calls = Arc::clone(&calls);
    let backend: Backend = Backend::sqlite_with(move |pool| {
        let mut stores = pagis_storage_sqlite::stores(pool);
        stores.agents = Arc::new(CountingAgentStore {
            inner: stores.agents,
            calls: builder_calls,
        });
        stores
    })
    .await;

    stores::agent_update_writes_the_mutable_fields(&backend).await;
    assert_eq!(calls.load(Ordering::Relaxed), 6);
    assert_eq!(
        backend
            .count("SELECT COUNT(*) FROM agents", &[])
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        backend
            .rows()
            .text("SELECT name FROM agents", &[])
            .await
            .unwrap(),
        Some("Rex".into())
    );

    assert_eq!(
        backend
            .execute("UPDATE agents SET name = ?", &["Direct write".into()])
            .await
            .unwrap(),
        1
    );
    let workspace_id = backend.stores().workspaces.list().await.unwrap()[0]
        .id
        .clone();
    let agents = backend
        .stores()
        .agents
        .list_by_workspace(&workspace_id)
        .await
        .unwrap();
    assert_eq!(agents[0].name, "Direct write");
}

#[tokio::test]
async fn sqlite_with_owns_a_fresh_migrated_database_and_its_files() {
    let backend: Backend = Backend::sqlite_with(pagis_storage_sqlite::stores).await;
    backend.seeded_workspace().await;
    let other: Backend = Backend::sqlite_with(pagis_storage_sqlite::stores).await;
    assert_eq!(
        other
            .count("SELECT COUNT(*) FROM workspaces", &[])
            .await
            .unwrap(),
        0
    );

    let files = backend.database_files();
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|file| file.is_file()));
    assert_ne!(files, other.database_files());
    let Rows::Sqlite(pool) = backend.rows() else {
        panic!("the rows use SQLite");
    };
    let (_, name, file): (i64, String, String) = sqlx::query_as("PRAGMA database_list")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(name, "main");
    assert_eq!(
        std::fs::canonicalize(file).unwrap(),
        files[0].canonicalize().unwrap()
    );
    assert_eq!(files[1], files[0].with_file_name("pagis.db-wal"));
    assert!(!backend.is_postgres());

    drop(backend);
    assert!(files.iter().all(|file| !file.exists()));
}
