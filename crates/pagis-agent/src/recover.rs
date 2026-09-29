//! Boot recovery: the daemon has no durable mid-turn resume,
//! so a restart marks every non-terminal run `failed`, settles orphaned
//! streaming messages, and expires the requests of parked runs.

use pagis_core::{
    AuthorKind, Block, EventLog, Message, MessageId, MessageStatus, MessageStore, NewEvent,
    RequestStore, Run, RunState, RunStore, StoreError, blocks_text, now_ms,
};

/// The reason on every run a restart failed. A restart is whole-server,
/// so the message says so rather than reading as this run's own fault.
pub const RESTART_ERROR: &str = "the daemon restarted, which ends every run on the server";

/// The note posted into a parked run's channel; the next trigger's
/// context tells the agent what happened.
const RESTART_NOTE: &str = "A daemon restart interrupted this run while it waited for an \
     request. The daemon restart ends every run on the server, so this one was not singled \
     out. The pending request expired; ask again if the action is still needed.";

/// Mark every unfinished run `failed` (with [`RESTART_ERROR`]), fail
/// orphaned streaming messages, and expire the pending requests of
/// runs parked in `waiting_for_approval` — posting a system note into
/// their channel so the agent learns of it on its next trigger.
/// Returns the count of failed runs.
pub async fn fail_unfinished_runs(
    runs: &dyn RunStore,
    messages: &dyn MessageStore,
    requests: &dyn RequestStore,
    events: &dyn EventLog,
) -> Result<usize, StoreError> {
    let unfinished = runs.list_unfinished().await?;
    // Boot recovery settles the runs of the process that died with the
    // machine, so the wall clock is the time they ended. It runs before
    // the daemon has an injected clock.
    let now = now_ms();
    for mut run in unfinished.iter().cloned() {
        let from = run.state;
        run.state = RunState::Failed;
        run.failure_kind = Some(pagis_core::FailureKind::DaemonRestarted);
        // The reason travels with the run, so each person reads why
        // their own run ended and that the restart was the server's.
        run.error = Some(RESTART_ERROR.to_string());
        run.ended_at = Some(now);
        runs.update(&run).await?;
        events
            .append(NewEvent {
                workspace_id: run.workspace_id.clone(),
                event_type: "run.state_changed".to_string(),
                agent_id: Some(run.agent_id.clone()),
                run_id: Some(run.id.clone()),
                channel_id: run.channel_id.clone(),
                payload: serde_json::json!({
                    "from": from,
                    "to": RunState::Failed,
                    "reason": "daemon restarted",
                    "error": run.error,
                    "failure_kind": run.failure_kind,
                }),
            })
            .await?;
        if from == RunState::WaitingForApproval {
            requests
                .expire_pending_for_run(&run.workspace_id, &run.id, now)
                .await?;
            post_restart_note(messages, events, &run, now).await?;
        }
    }
    messages.fail_streaming(now).await?;
    Ok(unfinished.len())
}

/// Post the restart note as a system message where the run was bound.
async fn post_restart_note(
    messages: &dyn MessageStore,
    events: &dyn EventLog,
    run: &Run,
    now: i64,
) -> Result<(), StoreError> {
    let Some(channel_id) = run.channel_id.clone() else {
        return Ok(());
    };
    let blocks = vec![Block::markdown(RESTART_NOTE)];
    let note = Message {
        id: MessageId::generate(),
        workspace_id: run.workspace_id.clone(),
        channel_id,
        parent_message_id: run.root_message_id.clone(),
        author_kind: AuthorKind::System,
        author_agent_id: None,
        run_id: Some(run.id.clone()),
        status: MessageStatus::Complete,
        text_content: blocks_text(&blocks),
        blocks,
        pending_id: None,
        created_at: now,
        completed_at: Some(now),
    };
    // This fixed daemon notice contains no source or model text.
    messages.insert_stamped(&note, &[]).await?;
    events
        .append(NewEvent {
            workspace_id: note.workspace_id.clone(),
            event_type: "message.completed".to_string(),
            agent_id: None,
            run_id: note.run_id.clone(),
            channel_id: Some(note.channel_id.clone()),
            payload: serde_json::json!({
                "message_id": note.id.as_str(),
                "parent_message_id": note.parent_message_id.as_ref().map(|p| p.as_str()),
                "author_kind": AuthorKind::System,
            }),
        })
        .await?;
    Ok(())
}
