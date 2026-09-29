//! The takeover audit block: when a takeover ends, post a system
//! message into the agent's DM so the channel records that the user had
//! control of the computer, and for how long.

use std::sync::Arc;

use futures::StreamExt;
use pagis_core::{
    AuthorKind, Block, ChannelKind, EventScope, Message, MessageId, MessageStatus, NewEvent,
    blocks_text,
};

use crate::system::AgentDeps;

/// Subscribe to the bus and post the system block for every
/// `screen.takeover_ended`. Runs for the daemon's lifetime.
pub(crate) fn spawn_notes(deps: Arc<AgentDeps>) {
    tokio::spawn(async move {
        let mut events = deps.bus.subscribe(EventScope::Installation, None).await;
        while let Some(event) = events.next().await {
            if event.event_type != "screen.takeover_ended" {
                continue;
            }
            let Some(agent_id) = event.agent_id.clone() else {
                continue;
            };
            let duration_ms = event.payload["duration_ms"].as_u64().unwrap_or(0);
            if let Err(error) = post_note(&deps, &event.workspace_id, &agent_id, duration_ms).await
            {
                tracing::error!(%error, %agent_id, "takeover note post failed");
            }
        }
    });
}

/// A human-readable duration: seconds under a minute, minutes after.
fn describe_duration(duration_ms: u64) -> String {
    let seconds = duration_ms / 1000;
    if seconds < 60 {
        format!("{} s", seconds.max(1))
    } else {
        format!("{} min", seconds / 60)
    }
}

async fn post_note(
    deps: &AgentDeps,
    workspace_id: &pagis_core::WorkspaceId,
    agent_id: &pagis_core::AgentId,
    duration_ms: u64,
) -> Result<(), String> {
    let agent = deps
        .agents
        .get(workspace_id, agent_id)
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "agent not found".to_string())?;
    // The agent's DM: the one DM channel the agent participates in.
    let channels = deps
        .channels
        .list_by_workspace(workspace_id)
        .await
        .map_err(|err| err.to_string())?;
    let mut dm = None;
    for channel in channels
        .into_iter()
        .filter(|channel| channel.kind == ChannelKind::Dm)
    {
        let agents = deps
            .participants
            .agents_in_channel(workspace_id, &channel.id)
            .await
            .map_err(|err| err.to_string())?;
        if agents.contains(agent_id) {
            dm = Some(channel);
            break;
        }
    }
    let Some(dm) = dm else {
        return Err("no DM channel for the agent".to_string());
    };

    let text = format!(
        "You took control of {}'s computer \u{b7} {}",
        agent.name,
        describe_duration(duration_ms)
    );
    // A message timestamp.
    let now = deps.clock.now_ms();
    let blocks = vec![Block::markdown(&text)];
    let note = Message {
        id: MessageId::generate(),
        workspace_id: workspace_id.clone(),
        channel_id: dm.id.clone(),
        parent_message_id: None,
        author_kind: AuthorKind::System,
        author_agent_id: None,
        run_id: None,
        status: MessageStatus::Complete,
        text_content: blocks_text(&blocks),
        blocks,
        pending_id: None,
        created_at: now,
        completed_at: Some(now),
    };
    deps.messages
        .insert_stamped(&note, &[])
        .await
        .map_err(|err| err.to_string())?;
    deps.bus
        .publish(NewEvent {
            workspace_id: note.workspace_id.clone(),
            event_type: "message.completed".to_string(),
            agent_id: None,
            run_id: None,
            channel_id: Some(note.channel_id.clone()),
            payload: serde_json::json!({
                "message_id": note.id.as_str(),
                "parent_message_id": serde_json::Value::Null,
                "author_kind": AuthorKind::System,
            }),
        })
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}
