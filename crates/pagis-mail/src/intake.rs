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
    pub workspace_mailboxes: Arc<dyn pagis_core::WorkspaceMailboxRecordStore>,
    pub junk: Arc<dyn pagis_core::JunkStore>,
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
        message.summary.require_visible()?;
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

    pub async fn moves_junk(&self) -> Result<bool, TransportError> {
        Ok(self
            .intake
            .workspace_mailboxes
            .get(&self.workspace)
            .await
            .map_err(store_error)?
            .filter(|m| m.address.eq_ignore_ascii_case(&self.address))
            .is_none_or(|m| m.spam_mode == pagis_core::MailScoringMode::MoveToJunk))
    }
    pub async fn mark_spam(
        &self,
        store: &mut dyn crate::WorkspaceMailStore,
        id: &crate::MessageId,
        spam: bool,
    ) -> Result<(), TransportError> {
        let message = store.message(id).await?;
        // A correction is an explicit Person action, including in score-only mode.
        self.intake
            .junk
            .mark(
                &pagis_core::JunkMessage {
                    workspace_id: self.workspace.clone(),
                    mailbox: self.address.clone(),
                    message_key: message.message.summary.assessment_key().into(),
                    host_id: id.to_string(),
                    from: message.message.summary.from.clone(),
                    subject: message.message.summary.subject.clone(),
                    reasons: message
                        .message
                        .summary
                        .assessment
                        .as_ref()
                        .map(|a| a.verdict.reasons.clone())
                        .unwrap_or_default(),
                    person_mark: None,
                    quarantined: false,
                    released: false,
                },
                spam,
            )
            .await
            .map_err(store_error)?;
        self.intake.source.correct(store, id, spam).await
    }

    pub async fn relocate(
        &self,
        summary: &MessageSummary,
        id: &crate::MessageId,
    ) -> Result<(), TransportError> {
        let old_key = summary.assessment_key();
        let Some(mut message) = self
            .intake
            .junk
            .message(&self.workspace, &self.address, old_key)
            .await
            .map_err(store_error)?
        else {
            return Ok(());
        };
        message.host_id = id.to_string();
        if summary.message_id.is_empty() {
            message.message_key = id.to_string();
        }
        self.intake
            .junk
            .relocate(old_key, &message)
            .await
            .map_err(store_error)
    }

    pub async fn assess(
        &self,
        summary: &mut MessageSummary,
        in_junk: bool,
    ) -> Result<(), TransportError> {
        let previous = self
            .intake
            .junk
            .message(&self.workspace, &self.address, summary.assessment_key())
            .await
            .map_err(store_error)?;
        let mut verdict = self.intake.source.verdict(summary, in_junk).await?;
        let quarantined = crate::spam::quarantine_required(&verdict)
            || previous.as_ref().is_some_and(|m| m.quarantined);
        if !quarantined
            && pagis_core::spam_band(verdict.score, verdict.action)
                == pagis_core::SpamBand::Uncertain
        {
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
        let policy = self.intake.junk.org_policy().await.map_err(store_error)?;
        let lists = self
            .intake
            .junk
            .workspace_lists(&self.workspace)
            .await
            .map_err(store_error)?;
        self.intake
            .junk
            .observe(&pagis_core::JunkMessage {
                workspace_id: self.workspace.clone(),
                mailbox: self.address.clone(),
                message_key: summary.assessment_key().into(),
                host_id: summary.id.to_string(),
                from: summary.from.clone(),
                subject: summary.subject.clone(),
                reasons: previous
                    .as_ref()
                    .filter(|m| m.quarantined)
                    .map_or_else(|| verdict.reasons.clone(), |m| m.reasons.clone()),
                person_mark: None,
                quarantined,
                released: false,
            })
            .await
            .map_err(store_error)?;
        let mut junk = false;
        if quarantined && !previous.as_ref().is_some_and(|m| m.released) {
            verdict.action = pagis_core::SpamAction::Quarantine;
        } else if let Some(spam) = previous.and_then(|m| m.person_mark) {
            junk = spam;
            verdict.action = if spam {
                pagis_core::SpamAction::Junk
            } else {
                pagis_core::SpamAction::NoAction
            };
            verdict.source = "person".into();
        } else if let Some(block) = pagis_core::list_verdict(&policy.lists, &lists, &summary.from) {
            junk = block;
            verdict.action = if block {
                pagis_core::SpamAction::Junk
            } else {
                pagis_core::SpamAction::NoAction
            };
            verdict
                .reasons
                .push(if block { "block_list" } else { "allow_list" }.into());
        } else {
            junk = policy.sensitivity.is_junk(verdict.score, verdict.action);
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
            junk,
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
    pub fn is_junk(&self) -> bool {
        self.assessment.as_ref().is_some_and(|a| a.junk)
    }
    pub fn require_visible(&self) -> Result<(), TransportError> {
        if self
            .assessment
            .as_ref()
            .is_some_and(|a| a.verdict.action == pagis_core::SpamAction::Quarantine)
        {
            return Err(TransportError(TransportErrorCode::StaleId));
        }
        Ok(())
    }

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
