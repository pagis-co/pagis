//! The Model Request Captures of a Run (ADR-0030), on both backends.
//!
//! Name every new body in `store_suite_model_request_captures!` below; the
//! guard test of the parent module fails while one is missing.

use pagis_core::{ModelRequestCapture, ModelRequestCaptureId, Run, RunId, Workspace};
use serde_json::json;

use super::Backend;
use crate::fixture;

async fn run(backend: &Backend, workspace: &Workspace) -> Run {
    let stores = backend.stores();
    let agent = fixture::agent(&workspace.id);
    stores.agents.create(&agent).await.expect("write the Agent");
    let channel = fixture::channel(&workspace.id);
    stores
        .channels
        .create(&channel)
        .await
        .expect("write the Channel");
    let run = fixture::queued_run(&workspace.id, &agent.id, &channel.id);
    stores.runs.create(&run).await.expect("write the Run");
    run
}

fn capture(run: &Run, phase_request: i64, created_at: i64) -> ModelRequestCapture {
    ModelRequestCapture {
        id: ModelRequestCaptureId::generate(),
        workspace_id: run.workspace_id.clone(),
        run_id: run.id.clone(),
        phase: "reply".into(),
        phase_request,
        request: json!({"system": "Follow the user.", "messages": [{"role": "user", "text": "hi"}]}),
        answer: json!({"outcome": "failed", "status": 402, "body": {"error": "credits"}}),
        created_at,
    }
}

pub async fn a_capture_reads_back_with_its_run_in_request_order(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let run = run(backend, &workspace).await;
    let captures = &backend.stores().model_request_captures;
    let second = capture(&run, 1, 2_000);
    let first = capture(&run, 0, 1_000);
    captures.record(&second).await.unwrap();
    captures.record(&first).await.unwrap();

    let read = captures.list_for_run(&workspace.id, &run.id).await.unwrap();

    assert_eq!(read, vec![first, second]);
}

pub async fn a_workspace_never_reads_the_captures_of_another(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let run = run(backend, &workspace).await;
    let captures = &backend.stores().model_request_captures;
    captures.record(&capture(&run, 0, 1_000)).await.unwrap();
    let other = pagis_core::WorkspaceId::generate();

    assert!(
        captures
            .list_for_run(&other, &run.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        captures
            .list_for_run(&workspace.id, &RunId::generate())
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn the_sweep_deletes_only_the_captures_before_the_cutoff(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let run = run(backend, &workspace).await;
    let captures = &backend.stores().model_request_captures;
    let old = capture(&run, 0, 1_000);
    let new = capture(&run, 1, 5_000);
    captures.record(&old).await.unwrap();
    captures.record(&new).await.unwrap();

    let deleted = captures.delete_before(5_000).await.unwrap();

    assert_eq!(deleted, 1);
    assert_eq!(
        captures.list_for_run(&workspace.id, &run.id).await.unwrap(),
        vec![new]
    );
}

pub async fn turning_the_setting_off_deletes_every_capture(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let run = run(backend, &workspace).await;
    let captures = &backend.stores().model_request_captures;
    captures.record(&capture(&run, 0, 1_000)).await.unwrap();
    captures.record(&capture(&run, 1, 9_000)).await.unwrap();

    assert_eq!(captures.delete_all().await.unwrap(), 2);
    assert!(
        captures
            .list_for_run(&workspace.id, &run.id)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Every body of this module. [`crate::store_suite!`] turns each one
/// into a SQLite test and a Postgres test.
#[macro_export]
macro_rules! store_suite_model_request_captures {
    ($emit:path) => {
        $emit!(
            model_request_captures,
            a_capture_reads_back_with_its_run_in_request_order,
            a_workspace_never_reads_the_captures_of_another,
            the_sweep_deletes_only_the_captures_before_the_cutoff,
            turning_the_setting_off_deletes_every_capture,
        );
    };
}
