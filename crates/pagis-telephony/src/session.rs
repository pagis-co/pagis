//! Session tools act on a live Call. Hand-off needs a confirmed Trust Tier.

use pagis_broker::ToolDef;

use pagis_core::TrustTier;

use crate::brief::CallBrief;
use crate::hub::DtmfPresence;
use crate::transport::TransportCapabilities;

pub const HAND_OFF_CALL: &str = "hand_off_call";

pub const HANG_UP: &str = "hang_up";
pub const SEND_DIGITS: &str = "send_digits";

/// The tools one live call binds: the session tools, and the brief's
/// tools when the tier lets the Remote Party's words move the Agent.
/// The Unknown tier binds no brief tool (ADR-0021).
///
/// `send_digits` is absent when the carrier cannot send DTMF, and also
/// when the negotiated leg has no telephone-event payload type, as
/// ADR-0005 requires. `negotiated` is `None` before the leg exists,
/// when only the transport says what it can do.
pub fn session_tools(
    brief: &CallBrief,
    capabilities: TransportCapabilities,
    negotiated: Option<DtmfPresence>,
) -> Vec<ToolDef> {
    let mut tools = vec![ToolDef {
        name: HANG_UP.to_string(),
        description: "End the call now. Say goodbye first when a person is on the line."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "Why the call ends, in a few words."
                }
            },
            "required": ["reason"]
        }),
    }];
    let leg_sends = negotiated.is_none_or(|presence| presence.send_dtmf);
    if capabilities.send_dtmf && leg_sends {
        tools.push(ToolDef {
            name: SEND_DIGITS.to_string(),
            description: "Press keypad digits on the call, for a phone tree or an extension."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "digits": {
                        "type": "string",
                        "pattern": "^[0-9*#]{1,32}$",
                        "description": "The digits to press, in order."
                    }
                },
                "required": ["digits"]
            }),
        });
    }
    if brief.tier != TrustTier::Unknown {
        tools.push(ToolDef { name: HAND_OFF_CALL.into(), description: "Hand this Call to an active Agent of this Workspace. Name the Agent id.".into(), parameters: serde_json::json!({"type":"object", "properties":{"agent":{"type":"string"}}, "required":["agent"], "additionalProperties":false}) });
        tools.extend(brief.tools.iter().cloned());
    }
    tools
}
