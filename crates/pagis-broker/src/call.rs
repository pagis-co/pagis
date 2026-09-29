//! The `call.ended` Incoming Event (ADR-0020, ADR-0021, ADR-0019).
//!
//! A Call the Agent did not place ends with a transcript and nothing
//! else. ADR-0021 already says what happens next: "The Agent takes the
//! message, and the Run that reads it afterwards reads data." This
//! declaration is the source of that Run.
//!
//! The metadata is the envelope of one call and never its words: who
//! called, which tier they reached, how the call ended, and the id of
//! the transcript Artifact. The Agent reads the words with a live
//! `call_transcript`, which wraps them in the untrusted envelope with
//! the tier, the way `phone_call` wraps an outbound transcript.
//!
//! The mail declaration next door is the model: one kind, one matcher,
//! one declaration per provider that supplies it.

use crate::{CapabilityManifest, IncomingEventKind};

/// The namespace of the call events. It holds no tool: `phone_call`
/// and `call_transcript` are core tools, because a core tool name
/// carries no namespace prefix.
pub const CALL_NAMESPACE: &str = "call";

/// The one call Incoming Event kind: a Call the Agent answered has
/// ended and its transcript is stored.
pub const CALL_ENDED: &str = "call.ended";

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
        event_kinds: CARRIER_PROVIDERS.iter().copied().map(call_ended).collect(),
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
                "transcript_artifact_id": {"type": "string"},
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
        // The Agent holds its own desk line as its own identity, with
        // no Grant (ADR-0018 allows no phone Grant), so the rule keeps
        // working when the carrier credential does not.
        required_capability: String::new(),
        matcher: CALL_MATCHER.to_string(),
        provider: provider.to_string(),
        // No Sync acquires a Call of the desk line, which is on an
        // Installation Connection, so a Forget does not apply to it
        // (ADR-0008).
        source_resource: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_carrier_declares_the_same_call_event() {
        let manifest = call_manifest();
        assert_eq!(manifest.event_kinds.len(), CARRIER_PROVIDERS.len());
        for kind in &manifest.event_kinds {
            assert_eq!(kind.name, CALL_ENDED);
            assert_eq!(kind.matcher, CALL_MATCHER);
            // The desk line is the Agent's own identity, so the rule
            // needs no Grant on the carrier Connection.
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
    fn the_metadata_carries_no_words() {
        let kind = call_ended(TELNYX_PROVIDER);
        let properties = kind.metadata_schema["properties"]
            .as_object()
            .expect("the metadata schema declares its properties");
        assert!(!properties.contains_key("transcript"));
        assert!(properties.contains_key("transcript_artifact_id"));
    }
}
