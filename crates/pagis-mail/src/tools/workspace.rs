//! Resolve Workspace Mailbox reads and sends under a live Grant.
use super::*;

impl MailToolRuntime {
    pub(super) async fn prepare_mail(
        &self,
        workspace: &WorkspaceId,
        agent: &pagis_core::AgentId,
        route: &ToolRoute,
        mut arguments: serde_json::Value,
    ) -> Result<serde_json::Value, ToolResult> {
        let workspace_tool = match route {
            ToolRoute::WorkspaceMailbox { tool } => Some(*tool),
            _ => None,
        };
        let own_reply = matches!(
            route,
            ToolRoute::Mailbox {
                tool: MailTool::Send
            }
        ) && arguments["reply_to_workspace_message_id"].is_string();
        if workspace_tool.is_none() && !own_reply {
            return Ok(arguments);
        }
        self.deps
            .workspace
            .require_workspace_grant(workspace, agent)
            .await
            .map_err(workspace_error)?;
        let mailbox = self
            .deps
            .workspace
            .workspace_mailbox(workspace)
            .await
            .map_err(workspace_error)?
            .ok_or_else(|| {
                ToolResult::error(pagis_broker::MAILBOX_UNAVAILABLE, "no Workspace Mailbox")
            })?;
        let reply_id = if own_reply {
            arguments["reply_to_workspace_message_id"].as_str()
        } else if workspace_tool == Some(pagis_broker::WorkspaceMailTool::Reply) {
            arguments["message_id"].as_str()
        } else {
            None
        }
        .map(str::to_string);
        if let Some(id) = reply_id {
            let mut store = self
                .deps
                .workspace
                .open_workspace_for_agent(workspace, agent)
                .await
                .map_err(workspace_error)?;
            let original = store.message(&MessageId::new(id)).await;
            let close = store.close().await;
            let original = original.map_err(transport_error)?;
            close.map_err(transport_error)?;
            let mut reply = crate::reply_message(&original.message, &mailbox.address, false);
            if own_reply {
                reply.cc.push(mailbox.address.clone());
            }
            arguments["to"] = serde_json::json!(reply.to);
            arguments["cc"] = serde_json::json!(reply.cc);
            // A reply has one destination derived from the original message.
            arguments["bcc"] = serde_json::json!([]);
            arguments["subject"] = serde_json::json!(reply.subject);
            arguments["in_reply_to"] = serde_json::json!(reply.in_reply_to);
            arguments["references"] = serde_json::json!(reply.references);
        }
        if workspace_tool.is_some() {
            arguments["from"] = serde_json::json!(mailbox.address);
        }
        if workspace_tool.is_some_and(|tool| tool.sends()) {
            let mut send = arguments.clone();
            send["mailbox"] = serde_json::json!("own");
            pagis_broker::validate_arguments(MailTool::Send, &send)?;
        }
        Ok(arguments)
    }

    pub(super) async fn run_workspace(
        &self,
        tool: pagis_broker::WorkspaceMailTool,
        call: &AuthorizedCall,
    ) -> Result<serde_json::Value, ToolResult> {
        let mut store = self
            .deps
            .workspace
            .open_workspace_for_agent(&call.workspace_id, &call.agent_id)
            .await
            .map_err(workspace_error)?;
        let result = self.workspace_result(tool, call, store.as_mut()).await;
        // A close failure after SMTP cannot make the message unsent.
        if let Err(error) = store.close().await {
            tracing::warn!(%error,"closing the Workspace Mailbox failed");
        }
        result
    }
    async fn workspace_result(
        &self,
        tool: pagis_broker::WorkspaceMailTool,
        call: &AuthorizedCall,
        store: &mut dyn crate::WorkspaceMailStore,
    ) -> Result<serde_json::Value, ToolResult> {
        use pagis_broker::WorkspaceMailTool;
        match tool {
            WorkspaceMailTool::Escalate => {
                let message = store
                    .message(&MessageId::new(text(&call.arguments, "message_id")))
                    .await
                    .map_err(transport_error)?;
                self.deps
                    .workspace
                    .workspace_mailbox_records()
                    .escalate(
                        &call.workspace_id,
                        message.message.summary.assessment_key(),
                        &call.agent_id,
                    )
                    .await
                    .map_err(|e| workspace_error(MailboxError::Store(e)))?;
                Ok(serde_json::json!({"outcome":"needs_you"}))
            }
            WorkspaceMailTool::Search => {
                let page = store
                    .messages(
                        &text(&call.arguments, "folder"),
                        &text(&call.arguments, "query"),
                        &page_request(
                            &call.arguments,
                            call.arguments["max"].as_u64().unwrap_or(25).clamp(1, 25) as usize,
                        ),
                    )
                    .await
                    .map_err(transport_error)?;
                Ok(
                    serde_json::json!({"messages":page.items.iter().map(|m| summary_json(&m.summary)).collect::<Vec<_>>(),"page":page.next}),
                )
            }
            WorkspaceMailTool::GetMessage => {
                let message = store
                    .message(&MessageId::new(text(&call.arguments, "message_id")))
                    .await
                    .map_err(transport_error)?;
                let (body, truncated) = cut(&message.message.text, MAX_TEXT_BYTES);
                Ok(serde_json::json!({"message":message_json(&message.message, &body, truncated)}))
            }
            WorkspaceMailTool::GetThread => {
                let messages = store
                    .thread(&text(&call.arguments, "thread_id"))
                    .await
                    .map_err(transport_error)?;
                let offset = call.arguments["page"]
                    .as_str()
                    .map(|p| p.parse::<usize>())
                    .transpose()
                    .map_err(|_| ToolResult::error("invalid_request", "invalid page"))?
                    .unwrap_or(0);
                if offset > messages.len() {
                    return Err(ToolResult::error("invalid_request", "invalid page"));
                }
                let mut budget = MAX_TEXT_BYTES;
                let items: Vec<_> = messages
                    .iter()
                    .skip(offset)
                    .take(MAX_THREAD_MESSAGES)
                    .map(|m| {
                        let (body, truncated) = cut(&m.message.text, budget);
                        budget -= body.len();
                        message_json(&m.message, &body, truncated)
                    })
                    .collect();
                let next = offset + items.len();
                Ok(
                    serde_json::json!({"messages":items,"page":(next < messages.len()).then(||next.to_string())}),
                )
            }
            WorkspaceMailTool::Send | WorkspaceMailTool::Reply => {
                let message = OutgoingMessage {
                    to: strings(&call.arguments, "to"),
                    cc: strings(&call.arguments, "cc"),
                    bcc: strings(&call.arguments, "bcc"),
                    subject: text(&call.arguments, "subject"),
                    body: call.arguments["body"].as_str().unwrap_or_default().into(),
                    in_reply_to: call.arguments["in_reply_to"].as_str().map(str::to_string),
                    references: strings(&call.arguments, "references"),
                    ..Default::default()
                };
                let sent = self
                    .deps
                    .workspace
                    .send_workspace(&call.workspace_id, store, &message)
                    .await
                    .map_err(workspace_error)?;
                Ok(
                    serde_json::json!({"message_id":sent.message_id,"from":call.arguments["from"],"to":message.to,"cc":message.cc}),
                )
            }
        }
    }
}

pub(super) fn workspace_error(error: MailboxError) -> ToolResult {
    match &error {
        MailboxError::Conflict(message) if message == "no live Workspace Mailbox Grant" => {
            ToolResult::error("permission_revoked", message)
        }
        MailboxError::Conflict(message) if message == "outgoing_cap_reached" => ToolResult::error(
            pagis_broker::OUTGOING_CAP_REACHED,
            "the Workspace Mailbox has reached its Outgoing Cap",
        ),
        _ => desk_error(error),
    }
}
