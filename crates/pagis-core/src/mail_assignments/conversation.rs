//! The one assignment conversation in its Shared Workspace.
use super::*;

impl MailAssignments {
    pub(super) async fn post(
        &self,
        row: &MailAssignment,
        run: Option<&RunId>,
        workspace_name: &str,
    ) -> Result<(), StoreError> {
        let name = self
            .stores
            .agents
            .get(&row.workspace_id, &row.assigning_agent_id)
            .await?
            .ok_or_else(|| StoreError::Conflict("The assigning Agent does not exist.".into()))?
            .name;
        let role = match row.role {
            MailAssignmentRole::Decide => "Decide",
            MailAssignmentRole::Approve => "Approve",
            MailAssignmentRole::Fyi => "FYI",
        };
        let mut link = url::Url::parse("https://pagis.invalid/mail").expect("static URL");
        link.query_pairs_mut()
            .append_pair("workspace", row.workspace_id.as_str())
            .append_pair("message", &row.message_id);
        let text = format!(
            "{name}, for {workspace_name}\n\n{role}: {}\n\n[Open message](/mail?{})",
            row.summary,
            link.query().unwrap(),
        );
        let sender = &row.workspace_id;
        let now = self.clock.now_ms();
        let message = Message {
            id: MessageId::generate(),
            workspace_id: row.workspace_id.clone(),
            channel_id: row.channel_id.clone(),
            author: Author::Agent(row.assigning_agent_id.clone()),
            author_workspace_id: Some(sender.clone()),
            run_id: run.cloned(),
            parent_message_id: None,
            for_person_only: false,
            status: MessageStatus::Complete,
            blocks: vec![Block::markdown(&text)],
            text_content: text,
            pending_id: None,
            created_at: now,
            completed_at: Some(now),
        };
        let exposures = self
            .stores
            .grants
            .list_live_for_agent(sender, &row.assigning_agent_id)
            .await?
            .into_iter()
            .filter(|g| {
                matches!(
                    g.resource_kind.as_str(),
                    Grant::CONNECTION_KIND | Grant::WORKSPACE_MAILBOX_KIND
                )
            })
            .map(|g| MemoryExposure {
                grant_id: g.id,
                revision: g.revision,
            })
            .collect::<Vec<_>>();
        self.stores
            .messages
            .insert_stamped(&message, &exposures)
            .await?;
        for workspace in [&row.workspace_id, &row.private_workspace_id] {
            self.bus
                .publish(NewEvent {
                    workspace_id: workspace.clone(),
                    event_type: "conversation.message".into(),
                    agent_id: message.author.agent_id().cloned(),
                    run_id: run.cloned(),
                    channel_id: Some(row.channel_id.clone()),
                    payload: serde_json::json!({
                        "conversation_id": row.channel_id,
                        "message_id": message.id,
                        "storage_workspace_id": row.workspace_id,
                        "sender_workspace_id": sender,
                        "assignment_id": row.id,
                    }),
                })
                .await?;
        }
        self.changed(row).await
    }
}
