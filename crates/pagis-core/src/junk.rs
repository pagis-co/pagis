//! Junk policy and message corrections.

use crate::{AuditEntry, SpamAction, StoreError, WorkspaceId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrgSensitivity {
    Low,
    #[default]
    Normal,
    High,
}
impl OrgSensitivity {
    pub fn is_junk(self, score: i32, action: SpamAction) -> bool {
        match crate::spam_band(score, action) {
            crate::SpamBand::Junk => true,
            crate::SpamBand::Clean => false,
            crate::SpamBand::Uncertain => match self {
                Self::Low => false,
                Self::Normal => score >= 8000,
                Self::High => true,
            },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MailLists {
    pub allow: Vec<String>,
    pub block: Vec<String>,
}
impl MailLists {
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.allow.len() + self.block.len() > 1000
            || self.allow.iter().chain(&self.block).any(|entry| {
                let domain = entry
                    .rsplit_once('@')
                    .map_or(entry.as_str(), |(_, domain)| domain);
                entry.len() > 254
                    || url::Host::parse(domain).is_err()
                    || entry.trim() != entry
                    || entry.chars().any(char::is_control)
                    || if entry.contains('@') {
                        crate::mail_address(entry).is_none()
                    } else {
                        crate::mail_domain(entry).is_none()
                    }
            })
        {
            return Err(StoreError::Conflict(
                "Use an address or domain in each allow or block list entry".into(),
            ));
        }
        Ok(())
    }
}

pub fn list_verdict(org: &MailLists, workspace: &MailLists, address: &str) -> Option<bool> {
    let domain = address.rsplit_once('@').map(|(_, d)| d);
    let matches = |entries: &[String]| {
        entries.iter().any(|entry| {
            entry.eq_ignore_ascii_case(address)
                || domain.is_some_and(|d| d.eq_ignore_ascii_case(entry))
        })
    };
    [
        (org, true),
        (org, false),
        (workspace, true),
        (workspace, false),
    ]
    .into_iter()
    .find_map(|(lists, block)| {
        matches(if block { &lists.block } else { &lists.allow }).then_some(block)
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgJunkPolicy {
    pub sensitivity: OrgSensitivity,
    pub lists: MailLists,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JunkMessage {
    #[schema(value_type = String)]
    pub workspace_id: WorkspaceId,
    pub mailbox: String,
    pub message_key: String,
    pub host_id: String,
    pub from: String,
    pub subject: String,
    pub reasons: Vec<String>,
    pub person_mark: Option<bool>,
    pub quarantined: bool,
    pub released: bool,
}

/// Policy and message decisions are separate from computed assessments.
#[async_trait]
pub trait JunkStore: Send + Sync {
    async fn org_policy(&self) -> Result<OrgJunkPolicy, StoreError>;
    async fn set_org_policy(&self, policy: &OrgJunkPolicy) -> Result<(), StoreError>;
    async fn workspace_lists(&self, workspace: &WorkspaceId) -> Result<MailLists, StoreError>;
    async fn set_workspace_lists(
        &self,
        workspace: &WorkspaceId,
        lists: &MailLists,
    ) -> Result<(), StoreError>;
    async fn message(
        &self,
        workspace: &WorkspaceId,
        mailbox: &str,
        key: &str,
    ) -> Result<Option<JunkMessage>, StoreError>;
    /// Update the host location and quarantine evidence. Keep Person decisions.
    async fn observe(&self, message: &JunkMessage) -> Result<(), StoreError>;
    async fn relocate(&self, old_key: &str, message: &JunkMessage) -> Result<(), StoreError>;
    async fn mark(&self, message: &JunkMessage, spam: bool) -> Result<(), StoreError>;
    async fn quarantine(&self) -> Result<Vec<JunkMessage>, StoreError>;
    /// Release and its Org Audit Log entry share one transaction.
    async fn release(&self, mailbox: &str, key: &str, audit: &AuditEntry)
    -> Result<(), StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SpamAction;

    #[test]
    fn sensitivity_uses_the_existing_spam_bands() {
        for (sensitivity, threshold) in [
            (OrgSensitivity::Low, 15000),
            (OrgSensitivity::Normal, 8000),
            (OrgSensitivity::High, 4000),
        ] {
            assert!(!sensitivity.is_junk(threshold - 1, SpamAction::NoAction));
            assert!(sensitivity.is_junk(threshold, SpamAction::NoAction));
            assert!(sensitivity.is_junk(0, SpamAction::Junk));
            assert!(sensitivity.is_junk(0, SpamAction::Reject));
        }
    }

    #[test]
    fn org_lists_take_priority_and_domains_match_exactly() {
        let org = MailLists {
            block: vec!["blocked@example.com".into()],
            allow: vec!["example.com".into()],
        };
        let workspace = MailLists {
            block: vec!["example.com".into()],
            allow: vec!["blocked@example.com".into()],
        };
        assert_eq!(
            list_verdict(&org, &workspace, "BLOCKED@example.com"),
            Some(true)
        );
        assert_eq!(
            list_verdict(&org, &workspace, "sender@example.com"),
            Some(false)
        );
        assert_eq!(
            list_verdict(&org, &workspace, "sender@other.example.com"),
            None
        );
        assert_eq!(
            list_verdict(&org, &workspace, "sender@fakeexample.com"),
            None
        );
        assert!(
            MailLists {
                allow: vec!["bad value".into()],
                block: vec![]
            }
            .validate()
            .is_err()
        );
    }
}
