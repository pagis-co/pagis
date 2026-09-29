//! The direct channel between two Agents, and the message one Agent
//! posts into it (ADR-0003).
//!
//! `send_message` takes this path, and so does a Contribution and its
//! outcome (ADR-0016): both are messages from one Agent to
//! another, so both wake the reader and serialize per Thread the same
//! way.

use std::sync::Arc;

use pagis_core::{
    Agent, AgentId, AuthorKind, Block, Channel, ChannelId, ChannelKind, ChannelParticipant,
    ChannelStore, Clock, EventBus, Message, MessageId, MessageStatus, NewEvent, ParticipantId,
    ParticipantKind, ParticipantStore, RunId, WorkspaceId, blocks_text,
};

pub struct AgentDm {
    pub channels: Arc<dyn ChannelStore>,
    pub participants: Arc<dyn ParticipantStore>,
    pub messages: Arc<dyn pagis_core::MessageStore>,
    pub bus: Arc<dyn EventBus>,
    /// The daemon's logical now: this channel's messages are stamped
    /// from it, as every other conversation row is.
    pub clock: Arc<dyn Clock>,
}

impl AgentDm {
    /// The direct channel of the two Agents, opened when they have
    /// none yet.
    pub async fn between(
        &self,
        workspace_id: &WorkspaceId,
        sender: &Agent,
        target: &Agent,
        run_id: &RunId,
    ) -> Result<Channel, String> {
        if let Some(channel) = self
            .channels
            .find_agent_dm(workspace_id, &sender.id, &target.id)
            .await
            .map_err(|error| error.to_string())?
        {
            return Ok(channel);
        }
        let now = self.clock.now_ms();
        let channel = Channel {
            id: ChannelId::generate(),
            workspace_id: workspace_id.clone(),
            kind: ChannelKind::Dm,
            title: Some(format!("{} ↔ {}", sender.name, target.name)),
            created_at: now,
            updated_at: now,
        };
        self.channels
            .create(&channel)
            .await
            .map_err(|error| error.to_string())?;
        for agent_id in [&sender.id, &target.id] {
            self.participants
                .create(&ChannelParticipant {
                    id: ParticipantId::generate(),
                    workspace_id: workspace_id.clone(),
                    channel_id: channel.id.clone(),
                    kind: ParticipantKind::Agent,
                    agent_id: Some(agent_id.clone()),
                    joined_at: now,
                })
                .await
                .map_err(|error| error.to_string())?;
        }
        self.bus
            .publish(NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: "channel.created".to_string(),
                agent_id: Some(sender.id.clone()),
                run_id: Some(run_id.clone()),
                channel_id: Some(channel.id.clone()),
                payload: serde_json::json!({ "title": channel.title }),
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(channel)
    }

    /// Write one complete Agent message into the channel and announce
    /// it. The announcement is what wakes the other Agent.
    pub async fn post(
        &self,
        workspace_id: &WorkspaceId,
        channel: &Channel,
        sender_id: &AgentId,
        run_id: &RunId,
        text: &str,
        exposures: &[pagis_core::MemoryExposure],
    ) -> Result<MessageId, String> {
        let blocks = vec![Block::markdown(text)];
        let now = self.clock.now_ms();
        let message = Message {
            id: MessageId::generate(),
            workspace_id: workspace_id.clone(),
            channel_id: channel.id.clone(),
            parent_message_id: None,
            author_kind: AuthorKind::Agent,
            author_agent_id: Some(sender_id.clone()),
            run_id: Some(run_id.clone()),
            status: MessageStatus::Complete,
            text_content: blocks_text(&blocks),
            blocks,
            pending_id: None,
            created_at: now,
            completed_at: Some(now),
        };
        self.messages
            .insert_stamped(&message, exposures)
            .await
            .map_err(|error| error.to_string())?;
        self.bus
            .publish(NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: "message.completed".to_string(),
                agent_id: Some(sender_id.clone()),
                run_id: Some(run_id.clone()),
                channel_id: Some(channel.id.clone()),
                payload: serde_json::json!({
                    "message_id": message.id.as_str(),
                    "parent_message_id": serde_json::Value::Null,
                    "author_kind": AuthorKind::Agent,
                }),
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(message.id)
    }
}
