//! Chief of Staff delivery for a conversation between Workspaces.
use pagis_core::{
    AgentStatus, Author, ChannelId, Message, MessageId, NewEvent, Run, RunId, RunState, StoreError,
    TriggerKind, UserId, WorkspaceId,
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
    pub fn returns_home(run: &Run) -> bool {
        run.origin
            .as_ref()
            .is_some_and(|origin| origin.agent_id == run.agent_id)
    }

    pub fn of(run: &Run) -> Option<Self> {
        if run.trigger_kind != TriggerKind::Conversation {
            return None;
        }
        serde_json::from_str(run.trigger_ref.as_deref()?).ok()
    }
}

/// A group message addresses a Person by @name or by a reply to their Workspace.
fn is_addressed(
    text: &str,
    person_name: &str,
    workspace_name: &str,
    workspace_id: &WorkspaceId,
    person_id: &UserId,
    reply: Option<&Message>,
) -> bool {
    crate::mention::is_mentioned(text, person_name)
        || crate::mention::is_mentioned(text, workspace_name)
        || reply.is_some_and(|message| match &message.author {
            Author::Person(id) => id == person_id,
            Author::Agent(_) => message.author_workspace_id.as_ref() == Some(workspace_id),
            Author::System => false,
        })
}

impl AgentSystem {
    /// The Person used a client recently. A failed read counts as away,
    /// so the message goes to the Chief of Staff.
    async fn person_is_active(&self, person: &UserId) -> bool {
        match self
            .deps
            .person_activity
            .hold_until(person, self.deps.clock.now_ms())
            .await
        {
            Ok(hold) => hold.is_some(),
            Err(error) => {
                tracing::warn!(%error, "the activity of a Person cannot be read");
                false
            }
        }
    }

    /// A Person's answer has no Run. Its parent reference selects the request.
    /// An answer without a parent uses the last request from the asking Workspace.
    /// A Person's own message ends that Delegation. An Agent's final shared reply
    /// is not a new request.
    async fn last_agent_request(
        &self,
        answer: &pagis_core::Message,
        workspace: &WorkspaceId,
        person: Option<&pagis_core::UserId>,
    ) -> Result<Option<pagis_core::Message>, StoreError> {
        let mut before = answer.id.clone();
        loop {
            let page = self
                .deps
                .messages
                .list_channel(&answer.workspace_id, &answer.channel_id, Some(&before), 64)
                .await?;
            let Some(last) = page.last() else {
                return Ok(None);
            };
            before = last.id.clone();
            for message in page {
                if person.is_some() && message.author.person_id() == person {
                    return Ok(None);
                }
                if answer
                    .parent_message_id
                    .as_ref()
                    .is_some_and(|parent| parent != &message.id)
                {
                    continue;
                }
                if message.author_workspace_id.as_ref() != Some(workspace) {
                    continue;
                }
                let Some(run_id) = &message.run_id else {
                    return Ok(None);
                };
                let Some(sender) = self.deps.runs.get(workspace, run_id).await? else {
                    return Ok(None);
                };
                if ConversationDelivery::of(&sender).is_some_and(|delivery| {
                    delivery.channel_id == message.channel_id
                        && !ConversationDelivery::returns_home(&sender)
                }) {
                    return Ok(None);
                }
                return Ok(pagis_core::message_source_is_live(
                    self.deps.messages.as_ref(),
                    self.deps.grants.as_ref(),
                    &message,
                )
                .await?
                .then_some(message));
            }
        }
    }

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
        if !participants
            .iter()
            .any(|p| p.participant_workspace_id.as_ref() == Some(&event.workspace_id))
        {
            return Ok(());
        }
        let Some(workspace) = self.deps.workspaces.get(&event.workspace_id).await? else {
            return Ok(());
        };
        let assignment = self
            .deps
            .mail_assignments
            .by_channel(&message.channel_id)
            .await?;
        if assignment
            .as_ref()
            .is_some_and(|a| a.role == pagis_core::MailAssignmentRole::Fyi)
        {
            return Ok(());
        }
        if let Some(assignment) = &assignment
            && event.workspace_id == assignment.private_workspace_id
            && message.author_workspace_id.as_ref() == Some(&assignment.workspace_id)
            && !assignment.state.is_open()
        {
            return Ok(());
        }
        if let Some(assignment) = &assignment
            && self
                .deps
                .workspace_members
                .get(&assignment.workspace_id, &assignment.assignee)
                .await?
                .is_none()
        {
            return Ok(());
        }
        let person = workspace.user_id.clone();
        let request = if message.author.person_id().is_some() {
            self.last_agent_request(&message, &workspace.id, person.as_ref())
                .await?
        } else {
            None
        };
        let (hop_count, origin) = chain_of(&self.deps, request.as_ref().unwrap_or(&message)).await;
        // A delegated answer wakes the Agent that asked, even while its Person is active.
        let returning_agent = if let Some(origin) = &origin {
            self.deps
                .agents
                .get(&workspace.id, &origin.agent_id)
                .await?
                .filter(|agent| agent.status == AgentStatus::Active)
        } else {
            None
        };
        if person.is_none() && returning_agent.is_none() {
            return Ok(());
        }
        let user = match &person {
            Some(person) => match self.deps.users.get(person).await? {
                Some(user) if !user.is_disabled() => Some(user),
                _ => return Ok(()),
            },
            None => None,
        };
        let workspace_count = participants
            .iter()
            .filter_map(|p| p.participant_workspace_id.as_ref())
            .collect::<std::collections::HashSet<_>>()
            .len();
        if workspace_count > 2 {
            // A Workspace without a Person answers no group message.
            let (Some(person), Some(user)) = (&person, &user) else {
                return Ok(());
            };
            let reply = match &message.parent_message_id {
                Some(id) => self.deps.messages.get(&source, id).await?,
                None => None,
            };
            let reply = reply
                .as_ref()
                .filter(|m| m.channel_id == message.channel_id)
                .filter(|_| message.author.person_id().is_some() || returning_agent.is_some());
            if !is_addressed(
                &message.text_content,
                user.name.as_deref().or(user.email.as_deref()).unwrap_or(""),
                &workspace.name,
                &workspace.id,
                person,
                reply,
            ) {
                return Ok(());
            }
        }
        if hop_count > self.deps.config.max_hops {
            return Ok(());
        }
        if assignment.is_none()
            && returning_agent.is_none()
            && !workspace.delegate_everything
            && match &person {
                Some(person) => self.person_is_active(person).await,
                None => false,
            }
        {
            return Ok(());
        }
        let Some(agent_id) = returning_agent
            .map(|agent| agent.id)
            .or(workspace.chief_of_staff_agent_id)
        else {
            return Ok(());
        };
        let Some(agent) = self.deps.agents.get(&workspace.id, &agent_id).await? else {
            return Ok(());
        };
        if agent.status != AgentStatus::Active {
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
            origin,
            state: RunState::Queued,
            failure_kind: None,
            error: None,
            started_at: None,
            ended_at: None,
            created_at: self.deps.clock.now_ms(),
            dismissed_at: None,
        };
        self.deps.runs.create(&run).await?;
        self.deps
            .bus
            .publish(NewEvent {
                workspace_id: run.workspace_id.clone(),
                event_type: "run.created".into(),
                agent_id: Some(run.agent_id.clone()),
                run_id: Some(run.id.clone()),
                channel_id: run.channel_id.clone(),
                payload: serde_json::json!({
                    "trigger_kind": run.trigger_kind,
                    "trigger_ref": run.trigger_ref,
                }),
            })
            .await?;
        self.enqueue_proactive(run);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pagis_core::{Author, Message, UserId};

    fn message(author: Author, author_workspace: Option<&str>) -> Message {
        serde_json::from_value(serde_json::json!({
            "id":"message",
            "workspace_id":"org",
            "channel_id":"conversation",
            "author":author,
            "author_workspace_id":author_workspace,
            "status":"complete",
            "blocks":[],
            "text_content":"",
            "created_at":0,
        }))
        .unwrap()
    }

    #[test]
    fn group_conversation_addressing_uses_mentions_or_the_reply_author() {
        let workspace = WorkspaceId::from("recipient".to_string());
        let person = UserId::from("person".to_string());
        let person_message = message(Author::Person(person.clone()), None);
        let agent_message = message(Author::Agent("agent".to_string().into()), Some("recipient"));
        let other_message = message(Author::Person("other".to_string().into()), None);
        let other_agent = message(Author::Agent("agent".to_string().into()), Some("other"));
        for (text, reply, expected) in [
            ("@Alex please", None, true),
            ("@Research Office?", None, true),
            ("@alex, status?", None, true),
            ("Alex please", None, false),
            ("user@Alex.example", None, false),
            ("@Alexander", None, false),
            ("Anyone?", None, false),
            ("Please explain", Some(&person_message), true),
            ("Please explain", Some(&agent_message), true),
            ("Please explain", Some(&other_message), false),
            ("Please explain", Some(&other_agent), false),
        ] {
            assert_eq!(
                is_addressed(text, "Alex", "Research Office", &workspace, &person, reply),
                expected,
                "{text}"
            );
        }
    }
}
