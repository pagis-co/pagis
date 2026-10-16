//! Messages from an Agent to colleague Workspaces under a Grant or Approval.
use super::*;
use pagis_core::{Author, Message, MessageId, MessageStatus, NewEvent, SendOutcome, WorkspaceKind};

impl CoreToolRuntime {
    async fn colleagues(
        &self,
        workspace: &WorkspaceId,
    ) -> Result<(WorkspaceId, Vec<(WorkspaceId, String)>), ToolResult> {
        let unavailable = |error: pagis_core::StoreError| {
            ToolResult::error("temporarily_unavailable", error.to_string())
        };
        let owner = self
            .deps
            .workspaces
            .get(workspace)
            .await
            .map_err(unavailable)?
            .filter(|w| w.kind == WorkspaceKind::Private)
            .and_then(|w| w.user_id)
            .ok_or_else(|| {
                ToolResult::error(
                    "permission_denied",
                    "only a Private Workspace messages colleagues",
                )
            })?;
        let person = self
            .deps
            .users
            .get(&owner)
            .await
            .map_err(unavailable)?
            .filter(|p| !p.is_disabled())
            .ok_or_else(|| ToolResult::error("permission_denied", "the Person is not enabled"))?;
        let org = self
            .deps
            .orgs
            .list()
            .await
            .map_err(unavailable)?
            .into_iter()
            .find(|org| org.id == person.org_id)
            .ok_or_else(|| ToolResult::error("not_found", "Org not found"))?;
        let mut colleagues = Vec::new();
        for person in self
            .deps
            .users
            .list_by_org(&org.id)
            .await
            .map_err(unavailable)?
        {
            if person.id == owner || person.is_disabled() {
                continue;
            }
            if let Some(w) = self
                .deps
                .workspaces
                .for_user(&person.id)
                .await
                .map_err(unavailable)?
            {
                colleagues.push((
                    w.id,
                    person
                        .name
                        .or(person.email)
                        .unwrap_or_else(|| "Person".into()),
                ));
            }
        }
        Ok((org.workspace_id, colleagues))
    }

    pub(super) async fn message_recipients(
        &self,
        workspace: &WorkspaceId,
        arguments: &serde_json::Value,
    ) -> Result<(WorkspaceId, Vec<(WorkspaceId, String)>), ToolResult> {
        let (storage, colleagues) = self.colleagues(workspace).await?;
        let ids = arguments["workspace_ids"]
            .as_array()
            .filter(|ids| !ids.is_empty())
            .ok_or_else(|| {
                ToolResult::error("invalid_request", "select at least one colleague Workspace")
            })?;
        let mut recipients = Vec::new();
        for id in ids {
            let colleague = colleagues
                .iter()
                .find(|(workspace, _)| Some(workspace.as_str()) == id.as_str())
                .ok_or_else(|| ToolResult::error("not_found", "colleague Workspace not found"))?;
            if !recipients.contains(colleague) {
                recipients.push(colleague.clone());
            }
        }
        Ok((storage, recipients))
    }

    pub(super) async fn execute_colleagues_list(&self, call: &AuthorizedCall) -> ToolResult {
        match self.colleagues(&call.workspace_id).await {
            Ok((_, colleagues)) => ToolResult::success(serde_json::json!({"items":colleagues.into_iter().map(|(workspace_id, name)| serde_json::json!({"workspace_id":workspace_id,"name":name})).collect::<Vec<_>>()} ).to_string()),
            Err(error) => error,
        }
    }

    pub(super) async fn execute_message_colleagues(&self, call: &AuthorizedCall) -> ToolResult {
        match self.post_colleague_message(call).await {
            Ok(result) => result,
            Err(error) => error,
        }
    }

    async fn post_colleague_message(
        &self,
        call: &AuthorizedCall,
    ) -> Result<ToolResult, ToolResult> {
        let unavailable = |error: pagis_core::StoreError| {
            ToolResult::error("temporarily_unavailable", error.to_string())
        };
        let (storage, recipients) = self
            .message_recipients(&call.workspace_id, &call.arguments)
            .await?;
        let text = call.arguments["text"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| ToolResult::error("invalid_request", "text is required"))?;
        let mut ids = recipients.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        ids.push(call.workspace_id.clone());
        ids.sort();
        let channel = self
            .deps
            .channels
            .get_or_create_conversation(&storage, &ids)
            .await
            .map_err(unavailable)?;
        let run = self
            .deps
            .runs
            .get(&call.workspace_id, &call.run_id)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| ToolResult::error("not_found", "Run not found"))?;
        if crate::conversations::ConversationDelivery::of(&run)
            .is_some_and(|d| d.channel_id == channel.id)
            && !crate::conversations::ConversationDelivery::returns_home(&run)
        {
            return Err(ToolResult::error(
                "invalid_request",
                "your reply already goes to this conversation; answer in your reply",
            ));
        }
        let exposures = self
            .retained_exposures(&call.workspace_id, &call.run_id, &call.agent_id)
            .await
            .map_err(|e| ToolResult::error("temporarily_unavailable", e))?;
        let now = self.deps.clock.now_ms();
        let message = Message {
            id: MessageId::generate(),
            workspace_id: storage.clone(),
            channel_id: channel.id.clone(),
            author: Author::Agent(call.agent_id.clone()),
            author_workspace_id: Some(call.workspace_id.clone()),
            run_id: Some(call.run_id.clone()),
            parent_message_id: None,
            for_person_only: false,
            status: MessageStatus::Complete,
            blocks: vec![Block::markdown(text)],
            text_content: text.into(),
            pending_id: call
                .tool_call_id
                .as_ref()
                .map(|id| format!("{}:{id}", call.run_id)),
            created_at: now,
            completed_at: Some(now),
        };
        if let SendOutcome::Created(message) = self
            .deps
            .messages
            .insert_stamped(&message, &exposures)
            .await
            .map_err(unavailable)?
        {
            for workspace_id in ids {
                self.deps.bus.publish(NewEvent {
                    workspace_id, event_type: "conversation.message".into(), agent_id: Some(call.agent_id.clone()),
                    run_id: Some(call.run_id.clone()), channel_id: Some(channel.id.clone()),
                    payload: serde_json::json!({"conversation_id":channel.id,"message_id":message.id,"storage_workspace_id":storage,"sender_workspace_id":call.workspace_id}),
                }).await.map_err(unavailable)?;
            }
        }
        Ok(ToolResult::success(format!(
            "sent to conversation {}. Replies arrive as new messages.",
            channel.id
        )))
    }
}
