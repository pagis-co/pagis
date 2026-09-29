//! Credential grants (ADR-0013): the domain rules behind a
//! `credential_action` request, and the match check that lets a live
//! grant approve a vault action without a card.
//!
//! A rule is one registrable domain. It is stricter than the host rules
//! of an approval card in two ways. The daemon derives the rule from
//! the Credential record it is about to use, never from a string the agent supplied,
//! so an agent cannot widen its own reach by naming a domain. And there
//! is no pattern language: a rule matches its own domain and nothing
//! else, because a prefix or wildcard over a domain reads right to left
//! and would let `evil-example.com` ride on `example.com`.
//!
//! Revoking the grant is the off switch. There are no deny rules.

use serde::{Deserialize, Serialize};

/// The cap on the domains one credential grant carries (ADR-0013).
pub const MAX_CREDENTIAL_RULES: usize = 5;

/// Which vault action a `credential_action` request covers. Only the
/// actions that reach a site need one: `vault__list` and `vault__delete`
/// touch no secret and no page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialActionKind {
    Create,
    Fill,
    TotpFill,
}

impl CredentialActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialActionKind::Create => "create",
            CredentialActionKind::Fill => "fill",
            CredentialActionKind::TotpFill => "totp_fill",
        }
    }

    /// The approval card's title.
    pub fn title(self) -> &'static str {
        match self {
            CredentialActionKind::Create => "Create a login",
            CredentialActionKind::Fill => "Fill a saved login",
            CredentialActionKind::TotpFill => "Type a one-time code",
        }
    }
}

/// One vault action awaiting a decision. Every field is read off the
/// Credential record, so the card states what will actually happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialAction {
    pub action: CredentialActionKind,
    /// The record's registrable domain; this is also the rule.
    pub domain: String,
    pub username: String,
    /// The one address the daemon will open before it types.
    pub login_url: String,
}

impl CredentialAction {
    /// The request payload. `proposed_rules` carries the single
    /// domain behind "Always allow", in the shape an approval card already reads.
    pub fn request_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "action": self.action.as_str(),
            "domain": self.domain,
            "username": self.username,
            "login_url": self.login_url,
            "proposed_rules": [self.domain],
        })
    }

    /// The card body: what the user is deciding about, in one line.
    pub fn card_body(&self) -> String {
        format!("{} — {} — {}", self.domain, self.username, self.login_url)
    }
}

/// Normalize one domain for storage and comparison: lowercase, with a
/// trailing root dot dropped. Domains are case-insensitive, so a rule
/// and a record that differ only in case are the same rule.
pub fn normalize_domain(domain: &str) -> String {
    domain.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// True when the record's domain matches an allow rule. The match is
/// exact on the registrable domain: no subdomain and no suffix ride.
/// An empty rule list never matches, which is per-action mode.
pub fn domain_allowed(domain: &str, rules: &[String]) -> bool {
    let domain = normalize_domain(domain);
    !domain.is_empty() && rules.iter().any(|rule| normalize_domain(rule) == domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(list: &[&str]) -> Vec<String> {
        list.iter().map(|rule| rule.to_string()).collect()
    }

    fn action() -> CredentialAction {
        CredentialAction {
            action: CredentialActionKind::Fill,
            domain: "example.com".to_string(),
            username: "alice@example.com".to_string(),
            login_url: "https://example.com/login".to_string(),
        }
    }

    #[test]
    fn a_rule_matches_only_its_own_domain() {
        let allow = rules(&["example.com"]);

        assert!(domain_allowed("example.com", &allow));
        assert!(domain_allowed("EXAMPLE.com.", &allow));
        assert!(!domain_allowed("mail.example.com", &allow));
        assert!(!domain_allowed("evil-example.com", &allow));
        assert!(!domain_allowed("example.com.evil.test", &allow));
    }

    #[test]
    fn no_rules_is_per_action_mode() {
        assert!(!domain_allowed("example.com", &[]));
        assert!(!domain_allowed("", &rules(&["example.com"])));
    }

    #[test]
    fn the_payload_proposes_the_records_own_domain() {
        let payload = action().request_payload();

        assert_eq!(payload["action"], "fill");
        assert_eq!(payload["domain"], "example.com");
        assert_eq!(payload["username"], "alice@example.com");
        assert_eq!(payload["login_url"], "https://example.com/login");
        assert_eq!(
            payload["proposed_rules"],
            serde_json::json!(["example.com"])
        );
    }

    #[test]
    fn the_card_body_states_the_domain_the_user_and_the_address() {
        assert_eq!(
            action().card_body(),
            "example.com — alice@example.com — https://example.com/login"
        );
    }
}
