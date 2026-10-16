//! Spam Verdict and Trust Tier metadata. Mail bodies stay at the mail host.
use crate::{StoreError, TrustTier, WorkspaceId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpamAction {
    #[serde(rename = "no action")]
    NoAction,
    #[serde(rename = "add header")]
    AddHeader,
    #[serde(rename = "rewrite subject")]
    RewriteSubject,
    #[serde(rename = "soft reject")]
    SoftReject,
    Reject,
    Junk,
}

impl SpamAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoAction => "no action",
            Self::AddHeader => "add header",
            Self::RewriteSubject => "rewrite subject",
            Self::SoftReject => "soft reject",
            Self::Reject => "reject",
            Self::Junk => "junk",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpamVerdict {
    /// The score in thousandths of one point.
    pub score: i32,
    pub action: SpamAction,
    pub reasons: Vec<String>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailAssessment {
    pub verdict: SpamVerdict,
    pub trust_tier: TrustTier,
}

#[async_trait]
pub trait MailAssessmentStore: Send + Sync {
    /// Key by the RFC 5322 Message-ID within the Workspace and mailbox.
    /// A message without that header uses its IMAP id.
    async fn get(
        &self,
        workspace: &WorkspaceId,
        mailbox: &str,
        message: &str,
    ) -> Result<Option<MailAssessment>, StoreError>;
    /// Store the Spam Verdict and Trust Tier in one write.
    async fn put(
        &self,
        workspace: &WorkspaceId,
        mailbox: &str,
        message: &str,
        assessment: &MailAssessment,
    ) -> Result<(), StoreError>;
}
