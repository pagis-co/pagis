//! The Run journal contract, shared by both Storage Backends.
use super::Backend;
use pagis_core::RunId;

pub async fn checkpoint_calls_and_resume_survive_store_reads(backend: &Backend) {
    let workspace = backend.seeded_workspace().await;
    let agent = crate::fixture::agent(&workspace.id);
    let channel = crate::fixture::channel(&workspace.id);
    backend.stores().agents.create(&agent).await.unwrap();
    backend.stores().channels.create(&channel).await.unwrap();
    let run = crate::fixture::queued_run(&workspace.id, &agent.id, &channel.id);
    backend.stores().runs.create(&run).await.unwrap();
    let journal = &backend.stores().run_journal;
    assert!(journal.last(&run.id).await.unwrap().is_none());
    let state = serde_json::json!({"transcript": ["first", "second"], "loaded": ["mail"], "seen": ["message"], "speakers": {"agent": "Scout"}});
    journal.checkpoint(&run.id, 2, &state).await.unwrap();
    journal
        .turn_started(&run.id, &serde_json::json!({"calls": ["send", "read"]}))
        .await
        .unwrap();
    journal
        .tool_started(&run.id, "send", "outbound")
        .await
        .unwrap();
    journal.tool_started(&run.id, "read", "host").await.unwrap();
    journal
        .tool_ended(
            &run.id,
            "read",
            &serde_json::json!({"text": "read result", "images": ["image"]}),
        )
        .await
        .unwrap();
    let last = journal.last(&run.id).await.unwrap().unwrap();
    assert_eq!(last.checkpoint.as_ref().unwrap().turn, 2);
    assert_eq!(last.checkpoint.unwrap().state, state);
    assert_eq!(last.calls.len(), 2);
    assert!(
        last.calls
            .iter()
            .find(|call| call.call_id == "send")
            .unwrap()
            .result
            .is_none()
    );
    assert_eq!(
        last.calls
            .iter()
            .find(|call| call.call_id == "read")
            .unwrap()
            .result
            .as_ref()
            .unwrap()["text"],
        "read result"
    );
    assert!(journal.mark_resumed(&run.id).await.unwrap());
    assert!(!journal.mark_resumed(&run.id).await.unwrap());
    assert!(journal.last(&run.id).await.unwrap().unwrap().resumed);
    journal.checkpoint(&run.id, 3, &state).await.unwrap();
    let next = journal.last(&run.id).await.unwrap().unwrap();
    assert!(next.calls.is_empty());
    assert!(next.pending_turn.is_none());
    assert!(journal.mark_resumed(&run.id).await.unwrap());
    journal.delete(&run.id).await.unwrap();
    journal.delete(&run.id).await.unwrap();
    assert!(journal.last(&run.id).await.unwrap().is_none());
    assert!(journal.last(&RunId::generate()).await.unwrap().is_none());
}

#[macro_export]
macro_rules! store_suite_run_journal {
    ($emit:path) => {
        $emit!(run_journal, checkpoint_calls_and_resume_survive_store_reads);
    };
}
