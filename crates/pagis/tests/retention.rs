//! The retention sweep: a class with a window loses its expired
//! rows and blobs; a class kept for ever loses nothing.

use std::sync::Arc;

use object_store::ObjectStoreExt as _;
use pagis_core::{
    Artifact, ArtifactId, ArtifactKind, Workspace, WorkspaceId, WorkspaceStore as _, now_ms,
};
use pagis_storage_sqlite::{SqliteArtifactStore, SqliteRetentionPolicyStore, SqliteWorkspaceStore};
use sqlx::SqlitePool;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

struct Fixture {
    deps: pagis::retention::RetentionDeps,
    workspace_id: WorkspaceId,
}

async fn fixture(pool: SqlitePool) -> Fixture {
    let person =
        pagis_storage_sqlite::seed_org_and_administrator(&pool, "Org", pagis_core::now_ms())
            .await
            .unwrap();
    let workspace = Workspace {
        user_id: person.id.clone(),
        id: WorkspaceId::generate(),
        name: "ws".to_string(),
        timezone: "UTC".to_string(),
        created_at: now_ms(),
        onboarded_at: None,
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
        home_exit_host_id: None,
    };
    let workspaces = Arc::new(SqliteWorkspaceStore::new(pool.clone()));
    workspaces.create(&workspace).await.unwrap();
    Fixture {
        deps: pagis::retention::RetentionDeps {
            workspaces,
            policies: Arc::new(SqliteRetentionPolicyStore::new(pool.clone())),
            artifacts: Arc::new(SqliteArtifactStore::new(pool.clone())),
            blobs: Arc::new(object_store::memory::InMemory::new()),
            capture: Arc::new(pagis_core::CaptureSetting::new(true, 7)),
            captures: Arc::new(pagis_storage_sqlite::SqliteModelRequestCaptureStore::new(
                pool.clone(),
            )),
        },
        workspace_id: workspace.id,
    }
}

/// Store one artifact with its blob and return the row.
async fn store(fixture: &Fixture, kind: ArtifactKind, sha: &str, created_at: i64) -> Artifact {
    let artifact = Artifact {
        id: ArtifactId::generate(),
        workspace_id: fixture.workspace_id.clone(),
        creator_agent_id: None,
        run_id: None,
        kind,
        filename: Some(format!("{sha}.bin")),
        mime: "application/octet-stream".to_string(),
        size_bytes: 3,
        sha256: sha.to_string(),
        storage_key: format!("{}/{sha}", fixture.workspace_id),
        created_at,
    };
    fixture
        .deps
        .blobs
        .put(
            &object_store::path::Path::from(artifact.storage_key.clone()),
            b"bin".to_vec().into(),
        )
        .await
        .unwrap();
    fixture.deps.artifacts.insert(&artifact).await.unwrap();
    artifact
}

async fn blob_exists(fixture: &Fixture, artifact: &Artifact) -> bool {
    fixture
        .deps
        .blobs
        .get(&object_store::path::Path::from(
            artifact.storage_key.clone(),
        ))
        .await
        .is_ok()
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn nothing_is_swept_while_every_class_is_kept_for_ever(pool: SqlitePool) {
    let fixture = fixture(pool).await;
    let now = now_ms();
    let old = store(
        &fixture,
        ArtifactKind::Screenshot,
        "old",
        now - 400 * DAY_MS,
    )
    .await;

    let deleted = pagis::retention::sweep(&fixture.deps, now).await.unwrap();

    assert_eq!(deleted, 0, "the default keeps every class");
    assert!(
        fixture
            .deps
            .artifacts
            .get(&fixture.workspace_id, &old.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(blob_exists(&fixture, &old).await);
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_sweep_removes_expired_rows_and_blobs_of_the_set_class_only(pool: SqlitePool) {
    let fixture = fixture(pool).await;
    let now = now_ms();
    let expired = store(
        &fixture,
        ArtifactKind::Screenshot,
        "expired",
        now - 31 * DAY_MS,
    )
    .await;
    let fresh = store(
        &fixture,
        ArtifactKind::Screenshot,
        "fresh",
        now - 3 * DAY_MS,
    )
    .await;
    let other_class = store(
        &fixture,
        ArtifactKind::CallTranscript,
        "transcript",
        now - 31 * DAY_MS,
    )
    .await;

    fixture
        .deps
        .policies
        .set(
            &fixture.workspace_id,
            ArtifactKind::Screenshot,
            Some(30),
            now,
        )
        .await
        .unwrap();
    let deleted = pagis::retention::sweep(&fixture.deps, now).await.unwrap();

    assert_eq!(deleted, 1);
    assert!(
        fixture
            .deps
            .artifacts
            .get(&fixture.workspace_id, &expired.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!blob_exists(&fixture, &expired).await, "the blob is gone");
    assert!(
        fixture
            .deps
            .artifacts
            .get(&fixture.workspace_id, &fresh.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .deps
            .artifacts
            .get(&fixture.workspace_id, &other_class.id)
            .await
            .unwrap()
            .is_some(),
        "a class with no window keeps its artifacts"
    );
}

#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn a_window_that_returns_to_keep_for_ever_stops_the_sweep(pool: SqlitePool) {
    let fixture = fixture(pool).await;
    let now = now_ms();
    let old = store(&fixture, ArtifactKind::File, "old", now - 90 * DAY_MS).await;

    fixture
        .deps
        .policies
        .set(&fixture.workspace_id, ArtifactKind::File, Some(30), now)
        .await
        .unwrap();
    fixture
        .deps
        .policies
        .set(&fixture.workspace_id, ArtifactKind::File, None, now)
        .await
        .unwrap();
    let deleted = pagis::retention::sweep(&fixture.deps, now).await.unwrap();

    assert_eq!(deleted, 0);
    assert!(
        fixture
            .deps
            .artifacts
            .get(&fixture.workspace_id, &old.id)
            .await
            .unwrap()
            .is_some()
    );
}

/// A Model Request Capture lives for the retention of the setting
/// (ADR-0030), whatever the Artifact windows say.
#[sqlx::test(migrations = "../pagis-storage-sqlite/migrations")]
async fn the_sweep_removes_the_model_request_captures_past_their_retention(pool: SqlitePool) {
    let fixture = fixture(pool.clone()).await;
    let agent = pagis_testkit::fixture::agent(&fixture.workspace_id);
    pagis_core::AgentStore::create(
        &pagis_storage_sqlite::SqliteAgentStore::new(pool.clone()),
        &agent,
    )
    .await
    .unwrap();
    let channel = pagis_testkit::fixture::channel(&fixture.workspace_id);
    pagis_core::ChannelStore::create(
        &pagis_storage_sqlite::SqliteChannelStore::new(pool.clone()),
        &channel,
    )
    .await
    .unwrap();
    let run = pagis_testkit::fixture::queued_run(&fixture.workspace_id, &agent.id, &channel.id);
    pagis_core::RunStore::create(
        &pagis_storage_sqlite::SqliteRunStore::new(pool.clone()),
        &run,
    )
    .await
    .unwrap();
    let now = now_ms();
    for (phase_request, age_days) in [(0, 8), (1, 6)] {
        fixture
            .deps
            .captures
            .record(&pagis_core::ModelRequestCapture {
                id: pagis_core::ModelRequestCaptureId::generate(),
                workspace_id: fixture.workspace_id.clone(),
                run_id: run.id.clone(),
                phase: "reply".into(),
                phase_request,
                request: serde_json::json!({}),
                answer: serde_json::json!({}),
                created_at: now - age_days * DAY_MS,
            })
            .await
            .unwrap();
    }

    pagis::retention::sweep(&fixture.deps, now).await.unwrap();

    let left = fixture
        .deps
        .captures
        .list_for_run(&fixture.workspace_id, &run.id)
        .await
        .unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].phase_request, 1);
}
