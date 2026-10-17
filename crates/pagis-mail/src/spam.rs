//! Host Spam Verdicts and Person corrections.
use crate::{MessageSummary, TransportError};
use async_trait::async_trait;
use pagis_core::{SpamAction, SpamVerdict};

#[async_trait]
pub trait SpamVerdictSource: Send + Sync {
    /// Send a Person correction to the host through the same verdict seam.
    async fn correct(
        &self,
        store: &mut dyn crate::WorkspaceMailStore,
        id: &crate::MessageId,
        spam: bool,
    ) -> Result<(), TransportError> {
        let folder = if spam {
            store.junk_folder().await?
        } else {
            "INBOX".into()
        };
        store.move_message(id, &folder).await.map(|_| ())
    }

    async fn verdict(
        &self,
        message: &MessageSummary,
        in_junk: bool,
    ) -> Result<SpamVerdict, TransportError>;
}

pub struct HostSpamVerdicts;
#[async_trait]
impl SpamVerdictSource for HostSpamVerdicts {
    async fn verdict(
        &self,
        message: &MessageSummary,
        in_junk: bool,
    ) -> Result<SpamVerdict, TransportError> {
        Ok(parse_spam_verdict(&message.spam_headers, in_junk))
    }
}

/// Read the host's spam headers without storage or transport operations.
pub fn parse_spam_verdict(headers: &[(String, String)], in_junk: bool) -> SpamVerdict {
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };
    let status = header("X-Spam-Status")
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let (rspamd_score, rspamd_reasons) =
        rspamd_result(header("X-Spamd-Result").unwrap_or_default());
    let score = header("X-Rspamd-Score")
        .and_then(number)
        .or(rspamd_score)
        .or_else(|| header("X-Spam-Score").and_then(number))
        .or_else(|| {
            status
                .split_whitespace()
                .find_map(|part| part.strip_prefix("score=").and_then(number))
        })
        .unwrap_or_else(|| {
            header("X-Spam-Level").map_or(0, |v| {
                v.bytes().filter(|b| *b == b'*').count().min(1000) as i32 * 1000
            })
        });
    let action = if in_junk {
        SpamAction::Junk
    } else {
        match header("X-Rspamd-Action")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "no action" => SpamAction::NoAction,
            "add header" => SpamAction::AddHeader,
            "rewrite subject" => SpamAction::RewriteSubject,
            "soft reject" => SpamAction::SoftReject,
            "reject" => SpamAction::Reject,
            _ if header("X-Spam-Flag").is_some_and(|v| v.trim().eq_ignore_ascii_case("yes"))
                || status.to_ascii_lowercase().starts_with("yes") =>
            {
                SpamAction::AddHeader
            }
            _ => SpamAction::NoAction,
        }
    };
    let mut reasons = rspamd_reasons;
    if let Some((_, tests)) = status.split_once("tests=") {
        let tests = tests
            .split_whitespace()
            .take_while(|s| !s.contains('='))
            .collect::<String>();
        reasons.extend(
            tests
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        );
    }
    if in_junk {
        reasons.push("The mail host stores this message in Junk".into());
    }
    SpamVerdict {
        opinion: None,
        score,
        action,
        reasons,
        source: "mail_host".into(),
    }
}

fn number(value: &str) -> Option<i32> {
    let value = value.split_whitespace().next()?.parse::<f64>().ok()? * 1000.0;
    (value.is_finite() && value >= i32::MIN as f64 && value <= i32::MAX as f64)
        .then(|| value.round() as i32)
}

pub fn is_spam_header(name: &str) -> bool {
    [
        "X-Spam-Status",
        "X-Spam-Score",
        "X-Spam-Flag",
        "X-Rspamd-Score",
        "X-Spamd-Result",
        "X-Rspamd-Action",
        "X-Spam-Level",
    ]
    .iter()
    .any(|key| name.eq_ignore_ascii_case(key))
}

fn rspamd_result(value: &str) -> (Option<i32>, Vec<String>) {
    // A symbol's options can contain semicolons inside square brackets.
    let mut depth = 0usize;
    let mut parts = value.split(|c| match c {
        '[' => {
            depth += 1;
            false
        }
        ']' => {
            depth = depth.saturating_sub(1);
            false
        }
        ';' => depth == 0,
        _ => false,
    });
    let score = parts.next().and_then(|metric| {
        let (_, result) = metric.split_once('[')?;
        let (result, _) = result.split_once(']')?;
        let (score, _) = result.split_once('/')?;
        number(score.trim())
    });
    let reasons = parts
        .filter_map(|symbol| {
            let (name, rest) = symbol.trim().split_once('(')?;
            let (weight, _) = rest.split_once(')')?;
            let weight = weight.trim();
            if name.is_empty() || name.chars().any(char::is_whitespace) || number(weight).is_none()
            {
                return None;
            }
            Some(format!("{name} {weight}"))
        })
        .collect();
    (score, reasons)
}

/// A second opinion on uncertain mail.
#[async_trait]
pub trait MailOpinionSource: Send + Sync {
    async fn opinion(
        &self,
        workspace: &pagis_core::WorkspaceId,
        message: &MessageSummary,
    ) -> Result<Option<pagis_core::MailOpinion>, TransportError>;
}

/// These host symbols report malware or high-confidence phishing.
pub fn quarantine_required(verdict: &SpamVerdict) -> bool {
    verdict.action == SpamAction::Quarantine
        || verdict.reasons.iter().any(|reason| {
            let symbol = reason.split_whitespace().next().unwrap_or_default();
            ["CLAM_VIRUS", "VIRUS_FOUND", "MALWARE"].contains(&symbol)
                || (verdict.score >= 15000
                    && ["PHISHING", "PHISHED", "PHISHING_CONFIRMED"].contains(&symbol))
        })
}
