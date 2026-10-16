//! A Spam Verdict precedes the Trust Tier and every Agent read.
use crate::{
    MessageSummary, SenderEvidence, SenderTrust, SpamVerdictSource, TransportError,
    TransportErrorCode,
};
use pagis_core::{
    AgentId, AgentStatus, AgentStore, MailAssessment, MailAssessmentStore, TrustTier, WorkspaceId,
    WorkspaceStore,
};
use std::sync::Arc;

pub struct MailIntake {
    pub workspaces: Arc<dyn WorkspaceStore>,
    pub agents: Arc<dyn AgentStore>,
    pub source: Arc<dyn SpamVerdictSource>,
    pub records: Arc<dyn MailAssessmentStore>,
    pub trust: Arc<dyn SenderTrust>,
}

#[derive(Clone)]
pub struct MailReadContext {
    pub intake: Arc<MailIntake>,
    pub workspace: WorkspaceId,
    pub address: String,
    pub agent: Option<AgentId>,
    pub authserv_ids: Vec<String>,
}
impl MailReadContext {
    pub async fn assess(
        &self,
        summary: &mut MessageSummary,
        in_junk: bool,
    ) -> Result<(), TransportError> {
        let verdict = self.intake.source.verdict(summary, in_junk).await?;
        let ids = self
            .authserv_ids
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let sender = SenderEvidence::of_message(summary, &ids);
        let agent = match &self.agent {
            Some(agent) => Some(agent.clone()),
            None => match self
                .intake
                .workspaces
                .get(&self.workspace)
                .await
                .map_err(store_error)?
                .and_then(|workspace| workspace.chief_of_staff_agent_id)
            {
                Some(chief) => self
                    .intake
                    .agents
                    .get(&self.workspace, &chief)
                    .await
                    .map_err(store_error)?
                    .filter(|agent| agent.status == AgentStatus::Active)
                    .map(|agent| agent.id),
                None => None,
            },
        };
        let trust_tier = match &agent {
            Some(agent) => self
                .intake
                .trust
                .tier(&self.workspace, agent, &sender)
                .await
                .map_err(store_error)?,
            None => TrustTier::Unknown,
        };
        let assessment = MailAssessment {
            verdict,
            trust_tier,
        };
        self.intake
            .records
            .put(
                &self.workspace,
                &self.address,
                summary.assessment_key(),
                &assessment,
            )
            .await
            .map_err(store_error)?;
        summary.assessment = Some(assessment);
        Ok(())
    }
}
fn store_error(_: pagis_core::StoreError) -> TransportError {
    TransportError(TransportErrorCode::Unreachable)
}

impl MessageSummary {
    /// The RFC 5322 Message-ID survives folder moves and UIDVALIDITY changes.
    /// Mail without that header uses its IMAP id.
    pub fn assessment_key(&self) -> &str {
        if self.message_id.is_empty() {
            self.id.as_str()
        } else {
            &self.message_id
        }
    }
}
