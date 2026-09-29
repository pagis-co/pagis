use async_trait::async_trait;
use pagis_core::knowledge::*;
use pagis_core::{
    AgentStore, Connection, ConnectionStore, Grant, GrantId, GrantStore, MemorySecretStore,
    TenantKeys, WorkspaceStore,
};
use pagis_knowledge::{Error, Knowledge, Source, SourcePage};
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteConnectionStore, SqliteGrantStore, SqliteKnowledgeStore,
    SqliteWorkspaceStore,
};
use std::sync::Arc;

/// The Tenant Data Keys of a test installation. They derive the
/// suppression key that an acquisition takes.
fn keys() -> Arc<TenantKeys> {
    Arc::new(TenantKeys::new(Arc::new(MemorySecretStore::default())))
}

struct Feed {
    checkpoint: serde_json::Value,
    caught_up: bool,
    changes: Vec<SourceChange>,
}

#[async_trait]
impl Source for Feed {
    async fn collect(&self, _: &serde_json::Value, _: i64) -> Result<SourcePage, Error> {
        Ok(SourcePage {
            historical: true,
            arrivals: vec![],
            checkpoint: self.checkpoint.clone(),
            caught_up: self.caught_up,
            changes: self.changes.clone(),
        })
    }

    async fn read(&self, _: &SourceChange) -> Result<SourceContent, Error> {
        panic!("acquisition does not read source content or call a model")
    }

    async fn metadata(&self, item: &SourceChange) -> Result<serde_json::Value, Error> {
        match item.id.as_str() {
            "lost" => Err(Error::NotFound),
            "away" => Err(Error::Unavailable),
            id => Ok(serde_json::json!({"message_id": id, "received_at": 5})),
        }
    }
}

async fn setup() -> (Arc<SqliteKnowledgeStore>, SourceKey) {
    let pool = pagis_storage_sqlite::connect_memory().await.unwrap();
    pagis_storage_sqlite::MIGRATOR.run(&pool).await.unwrap();
    let workspace = pagis_testkit::fixture::seeded_workspace(&pool).await;
    SqliteWorkspaceStore::new(pool.clone())
        .create(&workspace)
        .await
        .unwrap();
    let agent = pagis_testkit::fixture::agent(&workspace.id);
    SqliteAgentStore::new(pool.clone())
        .create(&agent)
        .await
        .unwrap();
    let connection = Connection {
        id: "g".to_string().into(),
        workspace_id: workspace.id.clone(),
        provider: "google".into(),
        alias: "g".into(),
        display_name: "Gmail".into(),
        status: Connection::CONNECTED.into(),
        auth_mode: Connection::AUTH_MODE_BYO.into(),
        authorized_capabilities: vec!["gmail_read".into()],
        config: serde_json::json!({}),
        created_at: 1,
    };
    SqliteConnectionStore::new(pool.clone())
        .create(&connection)
        .await
        .unwrap();
    SqliteGrantStore::new(pool.clone())
        .create(&Grant {
            id: GrantId::generate(),
            workspace_id: workspace.id.clone(),
            agent_id: agent.id.clone(),
            resource_kind: Grant::CONNECTION_KIND.into(),
            resource_id: Some(connection.id.to_string()),
            scope: Grant::connection_scope(&["gmail_read".into()]),
            revision: 1,
            created_at: 1,
            revoked_at: None,
        })
        .await
        .unwrap();
    let store = Arc::new(SqliteKnowledgeStore::new(pool));
    let status = store
        .configure(
            SyncConfig {
                workspace_id: workspace.id,
                connection_id: connection.id,
                resource: "gmail".into(),
                agent_id: agent.id,
                required_capability: "gmail_read".into(),
                enabled: true,
                since: 0,
                filter: pagis_core::reflection_filter::ReflectionFilter {
                    rules: Vec::new(),
                    default: pagis_core::reflection_filter::Verdict::Reflect,
                },
            },
            10,
        )
        .await
        .unwrap();
    (store, status.key())
}

#[tokio::test]
async fn acquisition_keeps_versions_and_provider_progress() {
    let (store, key) = setup().await;
    let source = SourceChange {
        id: "renewal".into(),
        version: "42".into(),
        kind: "mail".into(),
        parent: Some("policy-1".into()),
        source_at: Some(1_000),
        operation: SourceOperation::Upsert,
        metadata: serde_json::Value::Null,
    };
    Knowledge::new(store.clone(), keys())
        .collect(
            &key,
            &Feed {
                checkpoint: serde_json::json!({"history":"42"}),
                caught_up: true,
                changes: vec![source.clone()],
            },
            10_000,
        )
        .await
        .unwrap();

    let versions = store.versions(&key, 0, 10).await.unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].source, source);
    let status = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert!(status.caught_up);
    assert_eq!(status.checkpoint, serde_json::json!({"history":"42"}));
    assert!(!status.rebuilding);
}

#[tokio::test]
async fn historical_arrival_is_not_requeued_after_scope_change() {
    let (store, key) = setup().await;
    let source = SourceChange {
        id: "renewal".into(),
        version: "42".into(),
        kind: "mail".into(),
        parent: Some("policy-1".into()),
        source_at: Some(1_000),
        operation: SourceOperation::Upsert,
        metadata: serde_json::Value::Null,
    };
    let arrival = pagis_core::NormalizedEvent {
        provider_event_id: source.id.clone(),
        metadata: serde_json::json!({"thread_id":"policy-1"}),
        occurred_at: 1_000,
        landing: None,
    };
    let state = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    store
        .acquire(
            &key,
            &SourceBatch {
                historical: true,
                arrivals: vec![arrival.clone()],
                expected_revision: state.cursor_revision,
                checkpoint: serde_json::json!({"history":"first"}),
                caught_up: true,
                changes: vec![source.clone()],
            },
            10_000,
            keys().as_ref(),
        )
        .await
        .unwrap();
    let pending = store.arrivals(&key, 10, 10_000).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].historical);
    assert_eq!(pending[0].arrival.as_ref(), Some(&arrival));
    store
        .acknowledge_arrival(&key, pending[0].sequence, 10_001)
        .await
        .unwrap();

    let mut config = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap()
        .config;
    config.since = 1;
    let state = store.configure(config, 10_002).await.unwrap();
    store
        .acquire(
            &key,
            &SourceBatch {
                historical: true,
                arrivals: vec![arrival],
                expected_revision: state.cursor_revision,
                checkpoint: serde_json::json!({"history":"scope-change"}),
                caught_up: true,
                changes: vec![source.clone()],
            },
            10_003,
            keys().as_ref(),
        )
        .await
        .unwrap();

    assert!(store.arrivals(&key, 10, 10_000).await.unwrap().is_empty());
    let versions = store.versions(&key, 0, 10).await.unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].source, source);
}

#[tokio::test]
async fn an_expired_checkpoint_resets_acquisition() {
    struct Expired;
    #[async_trait]
    impl Source for Expired {
        async fn collect(&self, _: &serde_json::Value, _: i64) -> Result<SourcePage, Error> {
            Err(Error::ResetRequired)
        }
        async fn read(&self, _: &SourceChange) -> Result<SourceContent, Error> {
            unreachable!()
        }
        async fn metadata(&self, _: &SourceChange) -> Result<serde_json::Value, Error> {
            unreachable!()
        }
    }

    let (store, key) = setup().await;
    Knowledge::new(store.clone(), keys())
        .collect(&key, &Expired, 10_000)
        .await
        .unwrap();
    let status = store
        .status(&key, pagis_core::now_ms())
        .await
        .unwrap()
        .unwrap();
    assert!(!status.caught_up);
    assert!(status.rebuilding);
    assert_eq!(status.checkpoint, serde_json::Value::Null);
}

#[tokio::test]
async fn acquisition_keeps_deletions_and_access_loss() {
    let (store, key) = setup().await;
    for (version, operation) in [
        ("1", SourceOperation::Upsert),
        ("2", SourceOperation::Deleted),
        ("3", SourceOperation::AccessLost),
    ] {
        let state = store
            .status(&key, pagis_core::now_ms())
            .await
            .unwrap()
            .unwrap();
        Knowledge::new(store.clone(), keys())
            .collect(
                &key,
                &Feed {
                    checkpoint: serde_json::json!({"version": version}),
                    caught_up: true,
                    changes: vec![SourceChange {
                        id: "renewal".into(),
                        version: version.into(),
                        kind: "mail".into(),
                        parent: Some("policy-1".into()),
                        source_at: Some(1_000),
                        operation,
                        metadata: serde_json::Value::Null,
                    }],
                },
                state.updated_at + 1,
            )
            .await
            .unwrap();
    }

    let versions = store.versions(&key, 0, 10).await.unwrap();
    assert_eq!(versions.len(), 3);
    assert_eq!(versions[1].source.operation, SourceOperation::Deleted);
    assert_eq!(versions[2].source.operation, SourceOperation::AccessLost);
    assert!(
        store
            .status(&key, pagis_core::now_ms())
            .await
            .unwrap()
            .unwrap()
            .caught_up
    );
}

fn bare(id: &str) -> SourceChange {
    SourceChange {
        id: id.into(),
        version: "1".into(),
        kind: "mail".into(),
        parent: Some(format!("thread-{id}")),
        source_at: Some(5),
        operation: SourceOperation::Upsert,
        metadata: serde_json::Value::Null,
    }
}

/// Acquisition completes the metadata of a version it holds without,
/// one item at a time: a lost item completes with empty metadata, and a
/// transient failure stops the pass where it is (ADR-0011).
#[tokio::test]
async fn acquisition_completes_the_metadata_of_incomplete_versions() {
    let (store, key) = setup().await;
    let feed = Feed {
        checkpoint: serde_json::json!({"page": 1}),
        caught_up: true,
        changes: vec![bare("m1"), bare("lost"), bare("away"), bare("m2")],
    };
    let knowledge = Knowledge::new(store.clone(), keys());
    knowledge.collect(&key, &feed, 10_000).await.unwrap();
    assert_eq!(
        store
            .status(&key, 10_000)
            .await
            .unwrap()
            .unwrap()
            .incomplete_versions,
        4
    );

    let error = knowledge.complete(&key, &feed, 10_000).await.unwrap_err();
    assert!(matches!(error, Error::Unavailable), "{error}");
    let versions = store.versions(&key, 0, 10).await.unwrap();
    assert_eq!(
        versions[0].source.metadata,
        serde_json::json!({"message_id": "m1", "received_at": 5})
    );
    assert_eq!(
        versions[1].source.metadata,
        serde_json::json!({}),
        "a lost item completes with empty metadata"
    );
    assert!(
        versions[2].source.metadata.is_null(),
        "the pass stops at the failure"
    );
    assert!(versions[3].source.metadata.is_null());
    assert_eq!(
        store
            .status(&key, 10_000)
            .await
            .unwrap()
            .unwrap()
            .incomplete_versions,
        2
    );
}
