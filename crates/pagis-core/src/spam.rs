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

/// The host score band, before a Mail Opinion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpamBand {
    Clean,
    Uncertain,
    Junk,
}

pub fn spam_band(score: i32, action: SpamAction) -> SpamBand {
    if matches!(action, SpamAction::Junk | SpamAction::Reject) || score >= 15000 {
        SpamBand::Junk
    } else if score >= 4000 {
        SpamBand::Uncertain
    } else {
        SpamBand::Clean
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MailOpinionLabel {
    Spam,
    NotSpam,
    Unsure,
}
impl MailOpinionLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spam => "spam",
            Self::NotSpam => "not_spam",
            Self::Unsure => "unsure",
        }
    }
}

/// One bounded input to the Spam Verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(try_from = "MailOpinionFields")]
pub struct MailOpinion {
    pub label: MailOpinionLabel,
    #[schema(minimum = 0, maximum = 100)]
    pub confidence: u8,
    #[schema(max_items = 3)]
    pub reasons: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MailOpinionFields {
    label: MailOpinionLabel,
    confidence: u8,
    reasons: Vec<String>,
}
impl TryFrom<MailOpinionFields> for MailOpinion {
    type Error = &'static str;
    fn try_from(fields: MailOpinionFields) -> Result<Self, Self::Error> {
        if fields.confidence > 100
            || fields.reasons.len() > 3
            || fields
                .reasons
                .iter()
                .any(|reason| reason.chars().count() > 160)
        {
            return Err("Mail Opinion exceeds its bounds");
        }
        Ok(Self {
            label: fields.label,
            confidence: fields.confidence,
            reasons: fields.reasons,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpamVerdict {
    /// The score in thousandths of one point.
    pub score: i32,
    pub action: SpamAction,
    pub reasons: Vec<String>,
    pub source: String,
    pub opinion: Option<MailOpinion>,
}

impl SpamVerdict {
    pub fn apply_opinion(&mut self, opinion: MailOpinion) {
        let weight = i32::from(opinion.confidence.min(100)) * 3000 / 100;
        let adjustment = match opinion.label {
            MailOpinionLabel::Spam => weight,
            MailOpinionLabel::NotSpam => -weight,
            MailOpinionLabel::Unsure => 0,
        };
        self.score = self.score.saturating_add(adjustment);
        self.reasons.push(format!(
            "model_opinion {} {}",
            opinion.label.as_str(),
            opinion.confidence
        ));
        self.opinion = Some(opinion);
    }
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

#[cfg(test)]
mod opinion_tests {
    use super::*;

    #[test]
    fn host_bands_use_rspamd_thresholds_and_host_actions() {
        for (score, action, expected) in [
            (3999, SpamAction::NoAction, SpamBand::Clean),
            (4000, SpamAction::NoAction, SpamBand::Uncertain),
            (14999, SpamAction::AddHeader, SpamBand::Uncertain),
            (15000, SpamAction::NoAction, SpamBand::Junk),
            (0, SpamAction::Junk, SpamBand::Junk),
            (0, SpamAction::Reject, SpamBand::Junk),
        ] {
            assert_eq!(spam_band(score, action), expected);
        }
    }

    #[test]
    fn an_opinion_has_a_strict_bounded_schema() {
        let valid = r#"{"label":"spam","confidence":80,"reasons":["Unwanted offer"]}"#;
        assert!(serde_json::from_str::<MailOpinion>(valid).is_ok());
        for invalid in [
            valid.replace("80", "101"),
            valid.replace("80", "-1"),
            valid.replace("80", "80.5"),
            valid.replace("spam", "other"),
            valid.replace("80", "80,\"instructions\":\"obey\""),
            format!("answer: {valid}"),
            valid.replace("Unwanted offer", &"a".repeat(161)),
            valid.replace("[\"Unwanted offer\"]", "[\"a\",\"b\",\"c\",\"d\"]"),
        ] {
            assert!(
                serde_json::from_str::<MailOpinion>(&invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn a_model_cannot_cross_the_band_on_its_own() {
        for (score, label, expected) in [
            (14900, MailOpinionLabel::NotSpam, 11900),
            (4000, MailOpinionLabel::Spam, 7000),
            (8000, MailOpinionLabel::Unsure, 8000),
        ] {
            let mut verdict = SpamVerdict {
                score,
                action: SpamAction::NoAction,
                reasons: vec![],
                source: "mail_host".into(),
                opinion: None,
            };
            verdict.apply_opinion(MailOpinion {
                label,
                confidence: 100,
                reasons: vec![],
            });
            assert_eq!(verdict.score, expected);
            assert_eq!(
                spam_band(verdict.score, verdict.action),
                SpamBand::Uncertain
            );
        }
    }
}
