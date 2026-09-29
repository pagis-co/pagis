//! Gmail's Signal Catalogue and its default Reflection Filter (ADR-0011).

use pagis_core::reflection_filter::{
    Catalogue, Condition, ConditionValue, ListFormat, Operator, ReflectionFilter, Rule, Signal,
    ValueKind, Verdict,
};

use crate::{GogRunner, GoogleProvider, ProviderError};

/// The Gmail categories a thread can have. A thread with no CATEGORY_
/// label is primary.
pub const CATEGORIES: [&str; 5] = ["primary", "promotions", "social", "updates", "forums"];

/// The trust tiers the Trust List resolves for a sender.
pub const TRUST_TIERS: [&str; 3] = ["owner", "trusted", "unknown"];

/// The labels Gmail gives the mail that is not in the mailbox.
/// Acquisition reads none of it, so no rule can name it.
pub const OUTSIDE_THE_MAILBOX: [&str; 2] = ["SPAM", "TRASH"];

/// The labels Gmail gives every account. The account's own labels come
/// from `users.labels.list` and join these.
pub const SYSTEM_LABELS: [&str; 11] = [
    "INBOX",
    "SENT",
    "DRAFT",
    "UNREAD",
    "STARRED",
    "IMPORTANT",
    "CATEGORY_PERSONAL",
    "CATEGORY_SOCIAL",
    "CATEGORY_PROMOTIONS",
    "CATEGORY_UPDATES",
    "CATEGORY_FORUMS",
];

fn strings(values: impl IntoIterator<Item = impl Into<String>>) -> Vec<String> {
    values.into_iter().map(Into::into).collect()
}

fn signal(id: &str, label: &str, kind: ValueKind) -> Signal {
    Signal {
        id: id.into(),
        label: label.into(),
        kind,
    }
}

/// What the Gmail resource offers the filter editor. `labels` holds the
/// account's own labels beside the system labels.
pub fn catalogue(account_labels: &[String]) -> Catalogue {
    let mut labels = strings(SYSTEM_LABELS);
    for label in account_labels {
        // `users.labels.list` names the system labels too, SPAM and
        // TRASH among them.
        if !labels.contains(label) && !OUTSIDE_THE_MAILBOX.contains(&label.as_str()) {
            labels.push(label.clone());
        }
    }
    Catalogue {
        signals: vec![
            signal("subject", "Subject", ValueKind::Text),
            signal("sender_name", "Sender name", ValueKind::Text),
            signal(
                "sender_address",
                "Sender address",
                ValueKind::List {
                    format: ListFormat::Email,
                },
            ),
            signal(
                "sender_domain",
                "Sender domain",
                ValueKind::List {
                    format: ListFormat::Domain,
                },
            ),
            signal("newest_at", "Newest message", ValueKind::DateTime),
            signal("oldest_at", "Oldest message", ValueKind::DateTime),
            signal("message_count", "Messages in the thread", ValueKind::Number),
            signal(
                "sender_threads_in_window",
                "Threads from this sender domain",
                ValueKind::Number,
            ),
            signal("owner_replied", "The owner replied", ValueKind::Boolean),
            signal("has_attachments", "Has attachments", ValueKind::Boolean),
            signal(
                "category",
                "Category",
                ValueKind::Choice {
                    options: strings(CATEGORIES),
                },
            ),
            signal(
                "sender_trust",
                "Sender trust",
                ValueKind::Choice {
                    options: strings(TRUST_TIERS),
                },
            ),
            signal("labels", "Labels", ValueKind::TagSet { options: labels }),
        ],
    }
}

fn rule(verdict: Verdict, signal: &str, operator: Operator, value: ConditionValue) -> Rule {
    Rule {
        verdict,
        conditions: vec![Condition {
            signal: signal.into(),
            operator,
            value,
        }],
    }
}

fn label(name: &str) -> ConditionValue {
    ConditionValue::Choice { value: name.into() }
}

/// The filter a Gmail resource has until the user writes their own.
/// The last line is skip, so a page reflects because a rule says it is
/// worth a Run, never because no rule spoke.
pub fn default_filter() -> ReflectionFilter {
    ReflectionFilter {
        rules: vec![
            rule(
                Verdict::Reflect,
                "labels",
                Operator::Has,
                label("IMPORTANT"),
            ),
            rule(Verdict::Reflect, "labels", Operator::Has, label("STARRED")),
            rule(
                Verdict::Reflect,
                "owner_replied",
                Operator::Is,
                ConditionValue::Boolean { value: true },
            ),
            rule(
                Verdict::Reflect,
                "sender_trust",
                Operator::In,
                ConditionValue::Choices {
                    values: strings(["owner", "trusted"]),
                },
            ),
        ],
        default: Verdict::Skip,
    }
}

impl<R: GogRunner> GoogleProvider<R> {
    /// The account's own label names, for the Signal Catalogue.
    pub async fn gmail_labels(&self) -> Result<Vec<String>, ProviderError> {
        let response = self
            .gmail_read("users.labels.list", serde_json::json!({"userId":"me"}))
            .await?;
        Ok(response["labels"]
            .as_array()
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| label["id"].as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_filter_is_valid_for_the_catalogue() {
        assert_eq!(default_filter().validate(&catalogue(&[])), Ok(()));
    }

    #[test]
    fn the_default_filter_reflects_only_what_a_rule_names() {
        let filter = default_filter();
        assert_eq!(filter.default, Verdict::Skip);
        assert!(
            filter
                .rules
                .iter()
                .all(|rule| rule.verdict == Verdict::Reflect),
            "a skip rule can only hold where the last line already skips"
        );
    }

    #[test]
    fn the_catalogue_offers_no_label_the_mailbox_does_not_sync() {
        let catalogue = catalogue(&["SPAM".into(), "TRASH".into(), "Receipts".into()]);
        let labels = catalogue.signal("labels").expect("the labels signal");
        let options = labels.kind.options().expect("the label options");
        for outside in OUTSIDE_THE_MAILBOX {
            assert!(
                !options.iter().any(|option| option == outside),
                "a rule on {outside} can never hold, so the editor must not offer it"
            );
        }
        assert!(options.contains(&"Receipts".to_string()));
    }

    #[test]
    fn the_catalogue_joins_the_account_labels_to_the_system_labels_once() {
        let catalogue = catalogue(&["Receipts".into(), "INBOX".into()]);
        let labels = catalogue.signal("labels").expect("the labels signal");
        let options = labels.kind.options().expect("the label options");
        assert_eq!(options.iter().filter(|o| *o == "INBOX").count(), 1);
        assert!(options.contains(&"Receipts".to_string()));
        assert!(options.contains(&"IMPORTANT".to_string()));
    }
}
