//! The trusted matcher of `mail.message_received` (ADR-0019).
//!
//! ADR-0006 fixes the rules: different fields combine with AND, values
//! inside one field combine with OR, and text matching ignores case.

use pagis_core::EventMatcher;
use pagis_mail::MailMessageMatcher;

fn mail() -> serde_json::Value {
    serde_json::json!({
        "mailbox": "work",
        "message_id": "m1",
        "from": "Ada <ada@example.com>",
        "from_domain": "example.com",
        "subject": "Quarterly Report",
        "received_at": 1_800_000_000_000_i64,
    })
}

#[test]
fn a_rule_can_name_one_mailbox() {
    let matcher = MailMessageMatcher;

    assert!(matcher.matches(&serde_json::json!({"mailbox": "work"}), &mail()));
    assert!(!matcher.matches(&serde_json::json!({"mailbox": "own"}), &mail()));
}

#[test]
fn min_trust_stops_the_weaker_tiers_and_unstamped_mail_is_unknown() {
    let matcher = MailMessageMatcher;
    let tiered = |tier: &str| {
        let mut mail = mail();
        mail["trust_tier"] = tier.into();
        mail
    };

    assert!(matcher.matches(
        &serde_json::json!({"min_trust": "trusted"}),
        &tiered("owner")
    ));
    assert!(matcher.matches(
        &serde_json::json!({"min_trust": "trusted"}),
        &tiered("trusted")
    ));
    assert!(!matcher.matches(
        &serde_json::json!({"min_trust": "trusted"}),
        &tiered("unknown")
    ));
    // Mail that nothing stamped is the weakest tier, never the
    // strongest: a filter cannot be passed by leaving a field out.
    assert!(!matcher.matches(&serde_json::json!({"min_trust": "trusted"}), &mail()));
    assert!(matcher.matches(&serde_json::json!({"min_trust": "unknown"}), &mail()));
}
