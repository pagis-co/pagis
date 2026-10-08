//! The Incoming Events of a Coding Session (ADR-0033, ADR-0006).
//!
//! The daemon raises them, and the Session Rule of the session wakes the
//! owning Agent in the session's Thread. The kinds are the end of a turn,
//! a decision that waits, and the end of the session.
//!
//! The metadata names the session and what happened to it, and never the
//! text of the harness. The Agent reads the transcript with a live tool,
//! which wraps the harness text in the untrusted envelope.

use pagis_core::PAGIS_PROVIDER;
use serde_json::{Map, Value, json};

use crate::{CapabilityManifest, IncomingEventKind};

/// The namespace of the Coding Session events. It holds no tool: the
/// Coding Session tools are core tools.
pub const CODING_SESSION_NAMESPACE: &str = "coding_session";

/// A turn of the harness ended, and the session takes a prompt.
pub const CODING_SESSION_TURN_ENDED: &str = "coding_session.turn_ended";

/// A Harness Permission or a question of the harness waits for an
/// answer.
pub const CODING_SESSION_NEEDS_DECISION: &str = "coding_session.needs_decision";

/// The session is interrupted, closed or failed.
pub const CODING_SESSION_ENDED: &str = "coding_session.ended";

/// The three kinds, in the order that a session can raise them.
pub const CODING_SESSION_EVENT_KINDS: [&str; 3] = [
    CODING_SESSION_TURN_ENDED,
    CODING_SESSION_NEEDS_DECISION,
    CODING_SESSION_ENDED,
];

/// The name of the trusted matcher of the three kinds.
pub const CODING_SESSION_MATCHER: &str = "coding_session";

/// The Coding Session manifest. It declares the three Incoming Event
/// kinds and no tool.
pub fn coding_session_manifest() -> CapabilityManifest {
    CapabilityManifest {
        namespace: CODING_SESSION_NAMESPACE.to_string(),
        source_version: env!("CARGO_PKG_VERSION").to_string(),
        tools: Vec::new(),
        event_kinds: vec![
            kind(
                CODING_SESSION_TURN_ENDED,
                [
                    (
                        "stop_reason",
                        json!({
                            "type": "string",
                            "enum": ["end_turn", "max_tokens", "max_turn_requests", "refusal", "cancelled"]
                        }),
                    ),
                    ("seq", json!({"type": "integer"})),
                ],
            ),
            kind(
                CODING_SESSION_NEEDS_DECISION,
                [
                    (
                        "decision_kind",
                        json!({"type": "string", "enum": ["permission", "question"]}),
                    ),
                    ("seq", json!({"type": "integer"})),
                ],
            ),
            kind(
                CODING_SESSION_ENDED,
                [
                    (
                        "state",
                        json!({"type": "string", "enum": ["interrupted", "closed", "failed"]}),
                    ),
                    // The end reason, or the reason of an interruption:
                    // `host_lost` or `daemon_restart`.
                    ("reason", json!({"type": "string"})),
                ],
            ),
        ],
    }
}

/// One kind: the fields that every session event carries, and the two
/// fields of the kind. Every field is required.
fn kind(name: &str, fields: [(&str, Value); 2]) -> IncomingEventKind {
    let mut properties = Map::new();
    properties.insert("coding_session_id".into(), json!({"type": "string"}));
    // The Agent wrote the title.
    properties.insert("title".into(), json!({"type": "string"}));
    // The id of the harness in the Harness Catalog.
    properties.insert("harness".into(), json!({"type": "string"}));
    // The name of the Host that runs the session.
    properties.insert("machine".into(), json!({"type": "string"}));
    for (field, schema) in fields {
        properties.insert(field.into(), schema);
    }
    let required: Vec<&String> = properties.keys().collect();
    IncomingEventKind {
        name: name.to_string(),
        metadata_schema: json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        }),
        // The source of the rule names the one session, so a rule has
        // nothing to narrow.
        filter_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        // A Grant names a Connection, and a Coding Session is the Agent's
        // own work.
        required_capability: String::new(),
        matcher: CODING_SESSION_MATCHER.to_string(),
        provider: PAGIS_PROVIDER.to_string(),
        // No Sync acquires a session, so a Forget does not apply to it
        // (ADR-0008).
        source_resource: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_declares_three_kinds_of_pagis_with_no_capability() {
        let manifest = coding_session_manifest();
        let names: Vec<&str> = manifest
            .event_kinds
            .iter()
            .map(|kind| kind.name.as_str())
            .collect();
        assert_eq!(names, CODING_SESSION_EVENT_KINDS);
        assert!(manifest.tools.is_empty());
        for kind in &manifest.event_kinds {
            assert_eq!(kind.provider, PAGIS_PROVIDER);
            assert!(kind.required_capability.is_empty(), "{}", kind.name);
            assert_eq!(kind.matcher, CODING_SESSION_MATCHER);
            assert_eq!(kind.source_resource, None);
            assert_eq!(
                kind.filter_schema,
                serde_json::json!({"type": "object", "properties": {}, "additionalProperties": false})
            );
        }
    }

    #[test]
    fn no_metadata_property_holds_harness_text() {
        // The harness writes the messages, the commands and the end
        // detail of a failure. None of them is in the metadata.
        let allowed = [
            "coding_session_id",
            "title",
            "harness",
            "machine",
            "stop_reason",
            "decision_kind",
            "state",
            "reason",
            "seq",
        ];
        for kind in coding_session_manifest().event_kinds {
            assert_eq!(kind.metadata_schema["additionalProperties"], false);
            let properties = kind.metadata_schema["properties"]
                .as_object()
                .expect("the metadata schema declares its properties");
            for property in properties.keys() {
                assert!(
                    allowed.contains(&property.as_str()),
                    "{} declares {property}",
                    kind.name
                );
            }
        }
    }

    #[test]
    fn each_kind_names_the_session_the_harness_and_the_machine() {
        for kind in coding_session_manifest().event_kinds {
            let required = kind.metadata_schema["required"]
                .as_array()
                .expect("the required fields");
            for field in ["coding_session_id", "harness", "machine"] {
                assert!(
                    required.contains(&serde_json::json!(field)),
                    "{} requires {field}",
                    kind.name
                );
            }
        }
    }
}
