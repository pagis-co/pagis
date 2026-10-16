//! A Spam Verdict precedes the Trust Tier and every Agent read.
mod workspace;
pub(crate) use workspace::CalendarMailStore;

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
    pub opinions: Arc<dyn crate::MailOpinionSource>,
    pub calendar: Arc<dyn pagis_core::CalendarStore>,
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
    pub async fn calendar_message(&self, message: &crate::Message) -> Result<(), TransportError> {
        if message.summary.from_mailboxes == 1
            && let Some(calendar) = &message.calendar
            && let Ok(scheduling) = pagis_core::imip::SchedulingMessage::parse(calendar)
        {
            scheduling
                .apply(
                    self.intake.calendar.as_ref(),
                    &self.workspace,
                    &self.address,
                    &message.summary.from,
                )
                .await
                .map_err(store_error)?;
        }
        Ok(())
    }

    pub async fn assess(
        &self,
        summary: &mut MessageSummary,
        in_junk: bool,
    ) -> Result<(), TransportError> {
        let mut verdict = self.intake.source.verdict(summary, in_junk).await?;
        if pagis_core::spam_band(verdict.score, verdict.action) == pagis_core::SpamBand::Uncertain {
            let stored_opinion = self
                .intake
                .records
                .get(&self.workspace, &self.address, summary.assessment_key())
                .await
                .map_err(store_error)?
                .and_then(|assessment| assessment.verdict.opinion);
            let opinion = match stored_opinion {
                Some(opinion) => Some(opinion),
                None => match self.intake.opinions.opinion(&self.workspace, summary).await {
                    Ok(opinion) => opinion,
                    Err(error) => {
                        tracing::warn!(%error, "Mail Opinion is unavailable");
                        None
                    }
                },
            };
            if let Some(opinion) = opinion {
                verdict.apply_opinion(opinion);
            }
        }
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
