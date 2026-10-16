//! Chief of Staff delivery for a conversation between Workspaces.
use pagis_core::{
    AgentStatus, ChannelId, MessageId, NewEvent, Run, RunId, RunState, StoreError, TriggerKind,
    WorkspaceId,
};
use serde::{Deserialize, Serialize};

use crate::{AgentSystem, system::chain_of};

/// A reference to the one shared message, never a copy of its words.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ConversationDelivery {
    pub workspace_id: WorkspaceId,
    pub channel_id: ChannelId,
    pub message_id: MessageId,
    pub for_person_only: bool,
}

impl ConversationDelivery {
    pub fn of(run: &Run) -> Option<Self> {
        if run.trigger_kind != TriggerKind::Conversation {
            return None;
        }
        serde_json::from_str(run.trigger_ref.as_deref()?).ok()
    }
}

impl AgentSystem {
    pub(crate) async fn deliver_conversation(
        &self,
        event: &pagis_core::Event,
    ) -> Result<(), StoreError> {
        let Some(source) = event.payload["storage_workspace_id"].as_str() else {
            return Ok(());
        };
        let Some(id) = event.payload["message_id"].as_str() else {
            return Ok(());
        };
        if event.payload["sender_workspace_id"].as_str() == Some(event.workspace_id.as_str()) {
            return Ok(());
        }
        let source = WorkspaceId::from(source.to_string());
        let Some(message) = self
            .deps
            .messages
            .get(&source, &MessageId::from(id.to_string()))
            .await?
        else {
            return Ok(());
        };
        if !pagis_core::message_source_is_live(
            self.deps.messages.as_ref(),
            self.deps.grants.as_ref(),
            &message,
        )
        .await?
        {
            return Ok(());
        }
        let participants = self
            .deps
            .participants
            .list_for_channel(&source, &message.channel_id)
            .await?;
        // Group answer rules are separate. Delivery applies to two Workspaces.
        if participants.len() != 2
            || !participants
                .iter()
                .any(|p| p.participant_workspace_id.as_ref() == Some(&event.workspace_id))
        {
            return Ok(());
        }
        let Some(workspace) = self.deps.workspaces.get(&event.workspace_id).await? else {
            return Ok(());
        };
        let Some(person) = workspace.user_id else {
            return Ok(());
        };
        if !workspace.delegate_everything
            && self
                .deps
                .person_activity
                .hold_until(&person, self.deps.clock.now_ms())
                .is_some()
        {
            return Ok(());
        }
        if self
            .deps
            .users
            .get(&person)
            .await?
            .is_none_or(|person| person.is_disabled())
        {
            return Ok(());
        }
        let Some(agent_id) = workspace.chief_of_staff_agent_id else {
            return Ok(());
        };
        let Some(agent) = self.deps.agents.get(&workspace.id, &agent_id).await? else {
            return Ok(());
        };
        if agent.status != AgentStatus::Active {
            return Ok(());
        }
        let (hop_count, _) = chain_of(&self.deps, &message).await;
        if hop_count > self.deps.config.max_hops {
            return Ok(());
        }
        let Some(channel) = self
            .deps
            .channels
            .find_user_dm(&workspace.id, &agent_id)
            .await?
        else {
            return Ok(());
        };
        let delivery = ConversationDelivery {
            workspace_id: source,
            channel_id: message.channel_id.clone(),
            message_id: message.id.clone(),
            for_person_only: message.for_person_only,
        };
        let run = Run {
            id: RunId::generate(),
            title: pagis_core::run_title(pagis_core::RunTitleSource::Message(
                &message.text_content,
            )),
            workspace_id: workspace.id,
            agent_id,
            channel_id: Some(channel.id),
            root_message_id: None,
            trigger_kind: TriggerKind::Conversation,
            trigger_ref: Some(serde_json::to_string(&delivery).expect("conversation reference")),
            hop_count,
            origin: None,
            state: RunState::Queued,
            failure_kind: None,
            error: None,
            started_at: None,
            ended_at: None,
            created_at: self.deps.clock.now_ms(),
            dismissed_at: None,
        };
        self.deps.runs.create(&run).await?;
        self.deps.bus.publish(NewEvent {
            workspace_id: run.workspace_id.clone(), event_type: "run.created".into(), agent_id: Some(run.agent_id.clone()),
            run_id: Some(run.id.clone()), channel_id: run.channel_id.clone(),
            payload: serde_json::json!({"trigger_kind":run.trigger_kind,"trigger_ref":run.trigger_ref}),
        }).await?;
        self.enqueue_proactive(run);
        Ok(())
    }
}
