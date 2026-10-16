//! The `call.ended` Incoming Event (ADR-0020, ADR-0021, ADR-0019).
//!
//! A Call the Agent did not place ends with a transcript and nothing
//! else. ADR-0021 already says what happens next: "The Agent takes the
//! message, and the Run that reads it afterwards reads data." This
//! declaration is the source of that Run.
//!
//! The metadata is the envelope of one call and never its words: who
//! called, which tier they reached, how the call ended, and the id of
//! the transcript File. The Agent reads the words with a live
//! `call_transcript`, which wraps them in the untrusted envelope with
//! the tier.
//!
//! The mail declaration next door is the model: one kind, one matcher,
//! one declaration per provider that supplies it.

use crate::{CapabilityManifest, IncomingEventKind};

/// The namespace for Call and inbound text events.
pub const CALL_NAMESPACE: &str = "call";

/// The one call Incoming Event kind: a Call the Agent answered has
/// ended and its transcript is stored.
pub const CALL_ENDED: &str = "call.ended";

/// An inbound Text Message is stored and ready for its Standing Rule.
pub const TEXT_MESSAGE_RECEIVED: &str = "text.message_received";

/// The name of the trusted matcher that reads its typed filter.
pub const CALL_MATCHER: &str = "call.ended";

/// The carrier providers Pagis connects (ADR-0020). A Workspace
/// holds one carrier Connection, and every number sits on it.
pub const TELNYX_PROVIDER: &str = "telnyx";
pub const TWILIO_PROVIDER: &str = "twilio";
pub const PLIVO_PROVIDER: &str = "plivo";
pub const CARRIER_PROVIDERS: [&str; 3] = [TELNYX_PROVIDER, TWILIO_PROVIDER, PLIVO_PROVIDER];

/// Whether the provider of a Connection is a carrier.
pub fn is_carrier(provider: &str) -> bool {
    CARRIER_PROVIDERS.contains(&provider)
}

/// The call manifest. It declares the one Incoming Event kind and no
/// tool, so the Agent's call tools stay beside each other in the core
/// manifest.
pub fn call_manifest() -> CapabilityManifest {
    CapabilityManifest {
        namespace: CALL_NAMESPACE.to_string(),
        source_version: env!("CARGO_PKG_VERSION").to_string(),
        tools: Vec::new(),
        event_kinds: CARRIER_PROVIDERS
            .iter()
            .copied()
            .flat_map(|provider| [call_ended(provider), text_received(provider)])
            .collect(),
    }
}

/// The one call Incoming Event kind, as one carrier supplies it. Every
/// carrier declares the same metadata and the same filter: what
/// differs between them is the wire under the call, not what a call is.
pub fn call_ended(provider: &str) -> IncomingEventKind {
    IncomingEventKind {
        name: CALL_ENDED.to_string(),
        // Never the transcript: the Agent reads it with a live
        // `call_transcript`, which carries the tier with the words.
        metadata_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "call_id": {"type": "string"},
                "direction": {"type": "string", "enum": ["inbound", "outbound"]},
                "number": {"type": "string"},
                "from": {"type": "string"},
                "trust_tier": {"type": "string", "enum": ["owner", "trusted", "unknown"]},
                "outcome": {"type": "string"},
                "ended_reason": {"type": "string"},
                "duration_ms": {"type": "integer"},
                "transcript_file_id": {"type": "string"},
                "answered_at": {"type": "integer"}
            },
            "required": ["call_id", "direction", "number", "from", "outcome"],
            "additionalProperties": false
        }),
        filter_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "callers": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Caller numbers in E.164 form, such as +14155550123."
                },
                "min_trust": {
                    "type": "string",
                    "enum": ["owner", "trusted", "unknown"],
                    "description": "The lowest caller tier that wakes you."
                },
                "answered_only": {
                    "type": "boolean",
                    "description": "Only a call somebody answered."
                }
            },
            "additionalProperties": false
        }),
        // Inbound Calls need no Grant on the Org's carrier Connection.
        required_capability: String::new(),
        matcher: CALL_MATCHER.to_string(),
        provider: provider.to_string(),
        // No Sync acquires a Call of the desk line, which is on an
        // Installation Connection, so a Forget does not apply to it
        // (ADR-0008).
        source_resource: None,
    }
}

fn text_received(provider: &str) -> IncomingEventKind {
    IncomingEventKind {
        name: TEXT_MESSAGE_RECEIVED.into(),
        metadata_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "text_record_id": {"type": "string"},
                "from": {"type": "string"},
                "number": {"type": "string"},
                "trust_tier": {"type": "string", "enum": ["owner", "trusted", "unknown"]},
                "received_at": {"type": "integer"},
                "segments": {"type": "integer"},
                "has_media": {"type": "boolean"}
            },
            "required": ["text_record_id", "from", "number", "trust_tier", "received_at", "segments", "has_media"],
            "additionalProperties": false
        }),
        filter_schema: serde_json::json!({"type":"object", "properties":{}, "additionalProperties":false}),
        required_capability: String::new(),
        matcher: CALL_MATCHER.into(),
        provider: provider.into(),
        source_resource: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_carrier_declares_the_same_call_event() {
        let manifest = call_manifest();
        assert_eq!(manifest.event_kinds.len(), CARRIER_PROVIDERS.len() * 2);
        for kind in &manifest.event_kinds {
            assert!([CALL_ENDED, TEXT_MESSAGE_RECEIVED].contains(&kind.name.as_str()));
            assert_eq!(kind.matcher, CALL_MATCHER);
            // Inbound Calls and texts need no Grant on the carrier Connection.
            assert!(kind.required_capability.is_empty());
        }
    }

    #[test]
    fn a_call_is_no_source_item_of_a_sync() {
        // The desk line is on an Installation Connection, and no Sync
        // acquires a Call, so a Forget does not apply to it (ADR-0008).
        for kind in &call_manifest().event_kinds {
            assert_eq!(kind.source_resource, None);
        }
    }

    #[test]
    fn text_metadata_names_the_record_without_its_body() {
        let kind = text_received(TELNYX_PROVIDER);
        assert_eq!(kind.name, "text.message_received");
        let properties = kind.metadata_schema["properties"].as_object().unwrap();
        assert!(!properties.contains_key("body"));
        for name in [
            "text_record_id",
            "number",
            "from",
            "trust_tier",
            "received_at",
            "segments",
            "has_media",
        ] {
            assert!(properties.contains_key(name), "missing {name}");
        }
    }

    #[test]
    fn the_metadata_carries_no_words() {
        let kind = call_ended(TELNYX_PROVIDER);
        let properties = kind.metadata_schema["properties"]
            .as_object()
            .expect("the metadata schema declares its properties");
        assert!(!properties.contains_key("transcript"));
        assert!(properties.contains_key("transcript_file_id"));
    }
}
