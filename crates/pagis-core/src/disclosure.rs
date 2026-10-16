//! Disclosure Rules for answers to colleagues.
use crate::{StoreError, WorkspaceId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureCategory {
    Memory,
    Mail,
    CalendarDetails,
    OtherPrivateData,
}

impl DisclosureCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Mail => "mail",
            Self::CalendarDetails => "calendar_details",
            Self::OtherPrivateData => "other_private_data",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Mail => "mail",
            Self::CalendarDetails => "calendar details",
            Self::OtherPrivateData => "other private data",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DisclosureRule {
    /// None permits every Workspace of the Org.
    #[schema(value_type = Option<String>)]
    pub asker_workspace_id: Option<WorkspaceId>,
    pub category: DisclosureCategory,
}
impl DisclosureRule {
    pub fn allows(&self, asker: &WorkspaceId, category: DisclosureCategory) -> bool {
        self.category == category
            && self
                .asker_workspace_id
                .as_ref()
                .is_none_or(|id| id == asker)
    }
}

#[async_trait]
pub trait DisclosureStore: Send + Sync {
    async fn profile(&self, workspace: &WorkspaceId) -> Result<String, StoreError>;
    async fn set_profile(&self, workspace: &WorkspaceId, profile: &str) -> Result<(), StoreError>;
    async fn list(&self, workspace: &WorkspaceId) -> Result<Vec<DisclosureRule>, StoreError>;
    async fn add(&self, workspace: &WorkspaceId, rule: &DisclosureRule) -> Result<(), StoreError>;
    async fn remove(
        &self,
        workspace: &WorkspaceId,
        rule: &DisclosureRule,
    ) -> Result<(), StoreError>;
}

/// The shared Channel named by a conversation Run, supplied by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisclosureConversation {
    pub workspace_id: WorkspaceId,
    pub channel_id: crate::ChannelId,
}
impl DisclosureConversation {
    pub fn of(run: &crate::Run) -> Result<Option<Self>, StoreError> {
        if run.trigger_kind != crate::TriggerKind::Conversation {
            return Ok(None);
        }
        serde_json::from_str(run.trigger_ref.as_deref().unwrap_or(""))
            .map(Some)
            .map_err(|error| StoreError::Corrupt(error.to_string()))
    }

    pub async fn askers(
        &self,
        participants: &dyn crate::ParticipantStore,
        answering: &WorkspaceId,
    ) -> Result<Vec<WorkspaceId>, StoreError> {
        let participants = participants
            .list_for_channel(&self.workspace_id, &self.channel_id)
            .await?;
        if participants.iter().any(|p| {
            p.kind != crate::ParticipantKind::Workspace || p.participant_workspace_id.is_none()
        }) {
            return Err(StoreError::Corrupt(
                "Conversation participants must be Workspaces".into(),
            ));
        }
        let mut ids: Vec<_> = participants
            .into_iter()
            .filter_map(|p| p.participant_workspace_id)
            .collect();
        ids.sort();
        ids.dedup();
        if ids.len() < 2 || !ids.contains(answering) {
            return Err(StoreError::Corrupt(
                "Conversation participants are invalid".into(),
            ));
        }
        ids.retain(|id| id != answering);
        Ok(ids)
    }

    /// Only a live attachment in this Channel permits the default File read.
    pub async fn file_is_attached(
        &self,
        messages: &dyn crate::MessageStore,
        grants: &dyn crate::GrantStore,
        file_id: &str,
    ) -> Result<bool, StoreError> {
        let mut before = None;
        loop {
            let page = messages
                .list_top_level(&self.workspace_id, &self.channel_id, before.as_ref(), 100)
                .await?;
            if page.is_empty() {
                return Ok(false);
            }
            for entry in &page {
                let thread = messages
                    .list_thread(&self.workspace_id, &entry.message.id)
                    .await?;
                for message in thread {
                    if message.channel_id != self.channel_id
                        || message.status != crate::MessageStatus::Complete
                        || !crate::message_source_is_live(messages, grants, &message).await?
                    {
                        continue;
                    }
                    if message.blocks.iter().any(|block| matches!(block,
                        crate::Block::Known(crate::KnownBlock::File { file_id: id, .. } | crate::KnownBlock::Image { file_id: id, .. }) if id == file_id)) { return Ok(true); }
                }
            }
            before = page.last().map(|entry| entry.message.id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disclosure_rules_match_only_the_asker_and_category() {
        let asker = WorkspaceId::generate();
        let other = WorkspaceId::generate();
        let rule = DisclosureRule {
            asker_workspace_id: Some(asker.clone()),
            category: DisclosureCategory::Mail,
        };
        assert!(rule.allows(&asker, DisclosureCategory::Mail));
        assert!(!rule.allows(&other, DisclosureCategory::Mail));
        assert!(!rule.allows(&asker, DisclosureCategory::Memory));
        let org = DisclosureRule {
            asker_workspace_id: None,
            category: DisclosureCategory::CalendarDetails,
        };
        assert!(org.allows(&asker, DisclosureCategory::CalendarDetails));
        assert!(org.allows(&other, DisclosureCategory::CalendarDetails));
        assert!(!org.allows(&asker, DisclosureCategory::Mail));
    }
}
