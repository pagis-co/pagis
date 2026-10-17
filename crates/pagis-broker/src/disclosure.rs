//! The broker checks disclosure before a tool can read private data.
use super::*;
use pagis_core::{DisclosureCategory, DisclosureRule};

/// Unknown tools require permission too. A shell, Plugin or Coding Session
/// can read private data through paths outside the memory and mail tools.
fn category(entry: &SnapshotTool) -> Option<DisclosureCategory> {
    match &entry.manifest.route {
        ToolRoute::Ui { .. }
        | ToolRoute::Core {
            tool:
                CoreTool::CalendarFreeBusy | CoreTool::ColleaguesList | CoreTool::MessageColleagues,
        } => None,
        ToolRoute::Calendar { .. }
        | ToolRoute::Core {
            tool: CoreTool::CalendarRead | CoreTool::CalendarListEvents,
        } => Some(DisclosureCategory::CalendarDetails),
        ToolRoute::Core {
            tool:
                CoreTool::MemoryRead
                | CoreTool::MemorySearch
                | CoreTool::MemoryWrite
                | CoreTool::MemoryDelete
                | CoreTool::MemoryReview
                | CoreTool::ConversationPage
                | CoreTool::ConversationSearch
                | CoreTool::ConversationRead
                | CoreTool::SkillLoad,
        } => Some(DisclosureCategory::Memory),
        ToolRoute::Mailbox { .. } | ToolRoute::WorkspaceMailbox { .. } => {
            Some(DisclosureCategory::Mail)
        }
        ToolRoute::Connection { .. }
            if entry
                .manifest
                .capability
                .as_deref()
                .is_some_and(|c| c.starts_with("calendar")) =>
        {
            Some(DisclosureCategory::CalendarDetails)
        }
        ToolRoute::Connection { .. }
            if entry
                .manifest
                .capability
                .as_deref()
                .is_some_and(|c| c.contains("mail")) =>
        {
            Some(DisclosureCategory::Mail)
        }
        _ => Some(DisclosureCategory::OtherPrivateData),
    }
}

impl Broker {
    pub(super) async fn disclosure_needed(
        &self,
        run_id: &RunId,
        context: &CallContext,
    ) -> Result<Vec<DisclosureRule>, BrokerError> {
        let run = self
            .deps
            .runs
            .get(&context.workspace_id, run_id)
            .await?
            .ok_or_else(|| BrokerError::RunNotFound(run_id.clone()))?;
        let Some(source) = pagis_core::disclosure::DisclosureConversation::of(&run)? else {
            return Ok(Vec::new());
        };
        let askers = source
            .askers(self.deps.participants.as_ref(), &context.workspace_id)
            .await?;
        let Some(category) = category(&context.entry) else {
            return Ok(Vec::new());
        };
        match &context.entry.manifest.route {
            ToolRoute::Core {
                tool: CoreTool::FileRead,
            } => {
                if let Some(id) = context.arguments["file_id"].as_str()
                    && source
                        .file_is_attached(
                            self.deps.messages.as_ref(),
                            self.deps.grants.as_ref(),
                            id,
                        )
                        .await?
                {
                    return Ok(Vec::new());
                }
            }
            ToolRoute::Core {
                tool: CoreTool::ConversationRead | CoreTool::ConversationSearch,
            } if context.arguments["channel_id"]
                .as_str()
                .is_none_or(|id| id == source.channel_id.as_str()) =>
            {
                return Ok(Vec::new());
            }
            _ => {}
        }
        let rules = self.deps.disclosure.list(&context.workspace_id).await?;
        Ok(askers
            .into_iter()
            .filter(|asker| !rules.iter().any(|rule| rule.allows(asker, category)))
            .map(|asker| DisclosureRule {
                asker_workspace_id: Some(asker),
                category,
            })
            .collect())
    }

    pub(super) async fn create_disclosure_request(
        &self,
        context: &CallContext,
    ) -> Result<PendingRequest, BrokerError> {
        let rules = &context.disclosure;
        let category = rules
            .first()
            .expect("a disclosure gate has missing rules")
            .category;
        let title = "Share private data with colleagues".to_owned();
        let mut names = Vec::new();
        for rule in rules {
            let asker = rule
                .asker_workspace_id
                .as_ref()
                .expect("one asker per rule");
            let workspace =
                self.deps.workspaces.get(asker).await?.ok_or_else(|| {
                    BrokerError::CorruptSnapshot("Asker Workspace is absent".into())
                })?;
            names.push(workspace.name);
        }
        let body = format!("Share {} with {}?", category.label(), names.join(", "));
        let always_label = format!(
            "Always share {} with {}",
            category.label(),
            names.join(", ")
        );
        let request = Request {
            id: RequestId::generate(),
            workspace_id: context.workspace_id.clone(),
            agent_id: context.agent_id.clone(),
            run_id: Some(context.run_id.clone()),
            kind: Request::TOOL_ACTION_KIND.into(),
            payload: serde_json::json!({"action_title": title, "body": body, "tool_name": context.entry.manifest.definition.name, "disclosure_rules": rules, "always_label": always_label, "resume": context.resume_payload()?}),
            state: RequestState::Pending,
            values: None,
            decided_by: None,
            decided_at: None,
            created_at: pagis_core::now_ms(),
        };
        self.deps.requests.create(&request).await?;
        Ok(PendingRequest {
            request,
            title,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disclosure_tool_categories_cover_memory_mail_and_unknown_tools() {
        let manifest = core_manifest();
        for tool in manifest.tools {
            let expected = match tool.route {
                ToolRoute::Core {
                    tool:
                        CoreTool::CalendarFreeBusy
                        | CoreTool::ColleaguesList
                        | CoreTool::MessageColleagues,
                } => None,
                ToolRoute::Core {
                    tool: CoreTool::CalendarRead | CoreTool::CalendarListEvents,
                } => Some(DisclosureCategory::CalendarDetails),
                ToolRoute::Core {
                    tool:
                        CoreTool::MemoryRead
                        | CoreTool::MemorySearch
                        | CoreTool::ConversationRead
                        | CoreTool::ConversationSearch
                        | CoreTool::SkillLoad
                        | CoreTool::MemoryWrite
                        | CoreTool::MemoryDelete
                        | CoreTool::MemoryReview
                        | CoreTool::ConversationPage,
                } => Some(DisclosureCategory::Memory),
                _ => Some(DisclosureCategory::OtherPrivateData),
            };
            let entry = SnapshotTool {
                namespace: "core".into(),
                deferred: false,
                manifest: tool,
                source_version: manifest.source_version.clone(),
                grants: Vec::new(),
            };
            assert_eq!(category(&entry), expected);
        }
    }
    #[test]
    fn disclosure_mail_reads_require_the_mail_category() {
        for manifest in [
            mail::mail_manifest(),
            workspace_mail::workspace_mail_manifest(),
        ] {
            for tool in manifest.tools {
                let entry = SnapshotTool {
                    namespace: manifest.namespace.clone(),
                    deferred: false,
                    manifest: tool,
                    source_version: manifest.source_version.clone(),
                    grants: Vec::new(),
                };
                assert_eq!(category(&entry), Some(DisclosureCategory::Mail));
            }
        }
    }
}
