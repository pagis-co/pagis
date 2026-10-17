//! Calendar writes require the Calendar Grant of their Workspace.
use crate::{
    AllowRuleBuilder, ApprovalPresentation, CapabilityManifest, EffectClass, ManifestTool, ToolDef,
    ToolRoute,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarTool {
    Create,
    Update,
    Delete,
    Respond,
}

pub fn manifest() -> CapabilityManifest {
    let tools = [
        (
            CalendarTool::Create,
            "calendar_create_event",
            "Create an Event in your Workspace Calendar.",
            vec!["event"],
        ),
        (
            CalendarTool::Update,
            "calendar_update_event",
            "Replace an Event in your Workspace Calendar.",
            vec!["event_id", "event"],
        ),
        (
            CalendarTool::Delete,
            "calendar_delete_event",
            "Delete an Event or cancel its invitation.",
            vec!["event_id"],
        ),
        (
            CalendarTool::Respond,
            "calendar_respond",
            "Answer an invitation your Workspace received.",
            vec!["event_id", "status"],
        ),
    ]
    .into_iter()
    .map(|(tool, name, description, required)| ManifestTool {
        definition: ToolDef {
            name: name.into(),
            description: description.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "event_id": {"type": "string", "minLength": 1},
                    "event": event_schema(),
                    "status": {"type": "string", "enum": ["accepted", "tentative", "declined"]}
                },
                "required": required
            }),
        },
        capability: None,
        effect: EffectClass::Outbound,
        presentation: Some(ApprovalPresentation {
            action_title: "Send a calendar invitation".into(),
            body_argument: Some("body".into()),
            allow_rule_builder: Some(AllowRuleBuilder::MailRecipientDomain),
        }),
        route: ToolRoute::Calendar { tool },
        call_timeout: None,
        visibility: Default::default(),
        widget: None,
    })
    .collect();
    CapabilityManifest {
        namespace: "calendar".into(),
        source_version: env!("CARGO_PKG_VERSION").into(),
        tools,
        event_kinds: vec![],
    }
}

fn event_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "title": {"type": "string", "minLength": 1},
            "description": {"type": "string"},
            "location": {"type": "string"},
            "start": {"type": "string", "description": "Local YYYY-MM-DDTHH:MM:SS, or YYYY-MM-DD for an all-day Event."},
            "end": {"type": "string", "description": "Exclusive end, in the same format as start."},
            "timezone": {"type": "string", "description": "IANA time zone."},
            "all_day": {"type": "boolean"},
            "rrule": {"type": ["string", "null"]},
            "exdates": {"type": "array", "items": {"type": "string"}},
            "attendees": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "email": {"type": "string"},
                        "name": {"type": "string"},
                        "status": {"type": "string", "enum": ["needs-action", "accepted", "tentative", "declined"]}
                    },
                    "required": ["email", "name", "status"]
                }
            },
            "reminders": {"type": "array", "items": {"type": "integer", "minimum": 0}},
            "meeting_url": {"type": ["string", "null"], "description": "HTTPS meeting link."}
        },
        "required": ["title", "description", "location", "start", "end", "timezone", "all_day", "rrule", "exdates", "attendees", "reminders", "meeting_url"]
    })
}
