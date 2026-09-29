//! Page Signals for a mail Subject Page (ADR-0011).
//!
//! The signals are cheap facts about one page. They come from the
//! metadata that acquisition stored for each message of the thread, and
//! from the Trust List. No signal comes from a model, and no signal
//! reads the source words.

use pagis_core::TrustTier;
use pagis_core::reflection_filter::{PageSignals, SignalValue};
use pagis_mail::{SenderEvidence, SenderVerification, bare_address, sender_domain};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Threads from the same sender domain count over this window.
pub const SENDER_WINDOW_MS: i64 = 180 * 86_400_000;

/// The metadata field that holds the sender domain.
pub const SENDER_DOMAIN_FIELD: &str = "from_domain";

fn labels(message: &Value) -> Vec<String> {
    message["labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Sent mail and drafts are the owner's own messages.
fn outbound(message: &Value) -> bool {
    labels(message)
        .iter()
        .any(|label| label == "SENT" || label == "DRAFT")
}

fn from(message: &Value) -> &str {
    message["from"].as_str().unwrap_or_default()
}

/// The display name of one `From` header, else the address.
fn sender_name(value: &str) -> String {
    match value.split_once('<') {
        Some((name, _)) => name.trim().trim_matches('"').to_string(),
        None => value.trim().to_string(),
    }
}

/// The address and the domain of the newest message that the owner did
/// not send. The newest message of any kind answers a thread that holds
/// only the owner's own mail.
pub fn thread_sender(messages: &[Value]) -> Option<(String, String)> {
    let newest = |inbound_only: bool| {
        messages
            .iter()
            .filter(|message| !inbound_only || !outbound(message))
            .filter(|message| !from(message).is_empty())
            .max_by_key(|message| message["received_at"].as_i64().unwrap_or_default())
    };
    let message = newest(true).or_else(|| newest(false))?;
    let address = bare_address(from(message))?;
    let domain = message["from_domain"]
        .as_str()
        .map(str::to_string)
        .or_else(|| sender_domain(from(message)))?;
    Some((address, domain))
}

/// The sender evidence of one page, for the `sender_trust` signal
/// (ADR-0019). The stored metadata holds no verified authentication
/// result, so the sender is unverified and its tier is Unknown: a
/// forged `From` raises no signal.
pub fn sender_evidence(address: &str) -> SenderEvidence {
    SenderEvidence {
        from: address.to_string(),
        verification: SenderVerification::NoResult,
    }
}

/// The number of threads per sender domain whose newest message is at or
/// after `since`. The whole set is in memory, so the preview counts
/// without one query per thread.
pub fn sender_thread_counts(threads: &[Vec<Value>], since: i64) -> BTreeMap<String, i64> {
    let mut counts = BTreeMap::new();
    for messages in threads {
        let newest = messages
            .iter()
            .filter_map(|message| message["received_at"].as_i64())
            .max()
            .unwrap_or_default();
        if newest < since {
            continue;
        }
        for domain in messages
            .iter()
            .filter_map(|message| message[SENDER_DOMAIN_FIELD].as_str())
            .collect::<BTreeSet<_>>()
        {
            *counts.entry(domain.to_string()).or_default() += 1;
        }
    }
    counts
}

fn category(messages: &[Value]) -> String {
    let all: Vec<String> = messages.iter().flat_map(labels).collect();
    for (label, name) in [
        ("CATEGORY_PROMOTIONS", "promotions"),
        ("CATEGORY_SOCIAL", "social"),
        ("CATEGORY_FORUMS", "forums"),
        ("CATEGORY_UPDATES", "updates"),
    ] {
        if all.iter().any(|held| held == label) {
            return name.into();
        }
    }
    "primary".into()
}

fn text(messages: &[Value], value: impl Fn(&Value) -> String) -> SignalValue {
    let mut values = Vec::new();
    for message in messages {
        let value = value(message);
        if !value.is_empty() && !values.contains(&value) {
            values.push(value);
        }
    }
    SignalValue::Text { values }
}

/// The signals of one mail thread. `sender_threads` is the number of
/// threads from the same sender domain inside the window.
pub fn mail_signals(messages: &[Value], trust: TrustTier, sender_threads: i64) -> PageSignals {
    let mut signals = PageSignals::default();
    if messages.is_empty() {
        return signals;
    }
    signals.insert(
        "subject",
        text(messages, |message| {
            message["subject"].as_str().unwrap_or_default().to_string()
        }),
    );
    signals.insert(
        "sender_name",
        text(messages, |message| sender_name(from(message))),
    );
    signals.insert(
        "sender_address",
        text(messages, |message| {
            bare_address(from(message)).unwrap_or_default()
        }),
    );
    signals.insert(
        "sender_domain",
        text(messages, |message| {
            message["from_domain"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        }),
    );
    let times: Vec<i64> = messages
        .iter()
        .filter_map(|message| message["received_at"].as_i64())
        .collect();
    if let (Some(newest), Some(oldest)) = (times.iter().max(), times.iter().min()) {
        signals.insert("newest_at", SignalValue::DateTime { at: *newest });
        signals.insert("oldest_at", SignalValue::DateTime { at: *oldest });
    }
    signals.insert(
        "message_count",
        SignalValue::Number {
            value: messages.len() as i64,
        },
    );
    signals.insert(
        "sender_threads_in_window",
        SignalValue::Number {
            value: sender_threads,
        },
    );
    signals.insert(
        "owner_replied",
        SignalValue::Boolean {
            value: messages.iter().any(outbound),
        },
    );
    signals.insert(
        "has_attachments",
        SignalValue::Boolean {
            value: messages
                .iter()
                .any(|message| message["has_attachments"] == Value::Bool(true)),
        },
    );
    signals.insert(
        "category",
        SignalValue::Choice {
            value: category(messages),
        },
    );
    signals.insert(
        "sender_trust",
        SignalValue::Choice {
            value: trust.as_str().into(),
        },
    );
    let mut tags: Vec<String> = Vec::new();
    for label in messages.iter().flat_map(labels) {
        if !tags.contains(&label) {
            tags.push(label);
        }
    }
    signals.insert("labels", SignalValue::Tags { values: tags });
    signals
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(from: &str, at: i64, labels: &[&str]) -> Value {
        json!({
            "from": from,
            "from_domain": from.rsplit('@').next().unwrap_or_default(),
            "subject": "Clinic visit",
            "received_at": at,
            "has_attachments": false,
            "labels": labels,
        })
    }

    #[test]
    fn a_thread_reports_its_messages_times_labels_and_category() {
        let messages = vec![
            message("Clinic <care@clinic.test>", 10, &["INBOX", "IMPORTANT"]),
            message("owner@example.com", 20, &["SENT"]),
        ];
        let signals = mail_signals(&messages, TrustTier::Unknown, 3);
        assert_eq!(
            signals.get("message_count"),
            Some(&SignalValue::Number { value: 2 })
        );
        assert_eq!(
            signals.get("newest_at"),
            Some(&SignalValue::DateTime { at: 20 })
        );
        assert_eq!(
            signals.get("oldest_at"),
            Some(&SignalValue::DateTime { at: 10 })
        );
        assert_eq!(
            signals.get("owner_replied"),
            Some(&SignalValue::Boolean { value: true })
        );
        assert_eq!(
            signals.get("sender_threads_in_window"),
            Some(&SignalValue::Number { value: 3 })
        );
        assert_eq!(
            signals.get("category"),
            Some(&SignalValue::Choice {
                value: "primary".into()
            })
        );
        assert_eq!(
            signals.get("sender_trust"),
            Some(&SignalValue::Choice {
                value: "unknown".into()
            })
        );
        assert_eq!(
            signals.get("sender_name"),
            Some(&SignalValue::Text {
                values: vec!["Clinic".into(), "owner@example.com".into()]
            })
        );
        assert_eq!(
            signals.get("sender_address"),
            Some(&SignalValue::Text {
                values: vec!["care@clinic.test".into(), "owner@example.com".into()]
            })
        );
        let SignalValue::Tags { values } = signals.get("labels").expect("the labels") else {
            panic!("labels must be a tag set")
        };
        assert_eq!(values, &vec!["INBOX", "IMPORTANT", "SENT"]);
    }

    #[test]
    fn a_domain_counts_the_threads_whose_newest_message_is_inside_the_window() {
        let threads = vec![
            vec![
                message("a@shop.test", 10, &["INBOX"]),
                message("a@shop.test", 20, &["INBOX"]),
            ],
            vec![message("b@shop.test", 30, &["INBOX"])],
            vec![message("c@shop.test", 5, &["INBOX"])],
            vec![message("d@clinic.test", 40, &["INBOX"])],
        ];
        let counts = sender_thread_counts(&threads, 0);
        assert_eq!(counts.get("shop.test"), Some(&3));
        assert_eq!(counts.get("clinic.test"), Some(&1));
        // A thread whose newest message is older than the window is out.
        assert_eq!(
            sender_thread_counts(&threads, 15).get("shop.test"),
            Some(&2)
        );
    }

    #[test]
    fn a_category_label_names_the_category() {
        let messages = vec![message("shop@store.test", 1, &["CATEGORY_PROMOTIONS"])];
        assert_eq!(
            mail_signals(&messages, TrustTier::Unknown, 0).get("category"),
            Some(&SignalValue::Choice {
                value: "promotions".into()
            })
        );
    }

    #[test]
    fn the_sender_is_the_newest_message_the_owner_did_not_send() {
        let messages = vec![
            message("care@clinic.test", 10, &["INBOX"]),
            message("owner@example.com", 20, &["SENT"]),
        ];
        assert_eq!(
            thread_sender(&messages),
            Some(("care@clinic.test".into(), "clinic.test".into()))
        );
    }

    #[test]
    fn a_thread_of_the_owners_own_mail_still_names_a_sender() {
        let messages = vec![message("owner@example.com", 20, &["SENT"])];
        assert_eq!(
            thread_sender(&messages),
            Some(("owner@example.com".into(), "example.com".into()))
        );
    }

    #[test]
    fn a_thread_with_no_stored_metadata_has_no_signals() {
        assert_eq!(
            mail_signals(&[], TrustTier::Owner, 0),
            PageSignals::default()
        );
        assert_eq!(thread_sender(&[]), None);
    }
}
