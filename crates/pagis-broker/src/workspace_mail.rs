//! Workspace Mailbox tools use a Grant and the broker's Approval gate.
use crate::{
    AllowRuleBuilder, ApprovalPresentation, CapabilityManifest, EffectClass, ManifestTool, ToolDef,
    ToolRoute,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMailTool {
    Triage,
    Search,
    GetMessage,
    GetThread,
    Send,
    Reply,
}

impl WorkspaceMailTool {
    pub fn sends(self) -> bool {
        matches!(self, Self::Send | Self::Reply)
    }
}

pub fn workspace_mail_manifest() -> CapabilityManifest {
    let tool = |tool: WorkspaceMailTool,
                name: &str,
                description: &str,
                properties: serde_json::Value,
                required: Vec<&str>| ManifestTool {
        definition: ToolDef {
            name: name.into(),
            description: description.into(),
            parameters: serde_json::json!({
                "type": "object", "properties": properties, "required": required,
            }),
        },
        capability: None,
        effect: if tool.sends() {
            EffectClass::Outbound
        } else {
            EffectClass::Free
        },
        presentation: tool.sends().then(|| ApprovalPresentation {
            action_title: "Send an email".into(),
            body_argument: Some("body".into()),
            allow_rule_builder: Some(AllowRuleBuilder::MailRecipientDomain),
        }),
        route: ToolRoute::WorkspaceMailbox { tool },
        call_timeout: None,
        visibility: Default::default(),
        widget: None,
    };
    CapabilityManifest {
        namespace: "workspace_mail".into(),
        source_version: env!("CARGO_PKG_VERSION").into(),
        tools: vec![
            tool(
                WorkspaceMailTool::Triage,
                "workspace_mail_triage",
                "Record one Mail Triage outcome and a short note for this message. Assigned needs an active Agent with a Workspace Mailbox Grant.",
                serde_json::json!({"message_id":{"type":"string","minLength":1},
                    "outcome":{"type":"string","enum":["handled","replied","assigned","needs_you"]},
                    "note":{"type":"string","minLength":1,"maxLength":crate::DECISION_NOTE_CHARS},
                    "assignee":{"type":"string","minLength":1}}),
                vec!["message_id", "outcome", "note"],
            ),
            tool(
                WorkspaceMailTool::Search,
                "workspace_mail_search",
                "Search the Workspace Mailbox. An empty query reads the newest mail.",
                serde_json::json!({
                    "query": {"type": "string"},
                    "folder": {"type": "string"},
                    "page": {"type": "string"},
                    "max": {"type": "integer", "minimum": 1, "maximum": 25},
                }),
                vec!["query"],
            ),
            tool(
                WorkspaceMailTool::GetMessage,
                "workspace_mail_get_message",
                "Read a message from the Workspace Mailbox.",
                serde_json::json!({"message_id": {"type": "string", "minLength": 1}}),
                vec!["message_id"],
            ),
            tool(
                WorkspaceMailTool::GetThread,
                "workspace_mail_get_thread",
                "Read a thread from the Workspace Mailbox, oldest first. Use page for more messages.",
                serde_json::json!({
                    "thread_id": {"type": "string", "minLength": 1},
                    "page": {"type": "string"},
                }),
                vec!["thread_id"],
            ),
            tool(
                WorkspaceMailTool::Send,
                "workspace_mail_send",
                "Send as the Workspace Mailbox. Each message needs an Approval unless a recipient-domain Allow Rule covers all recipients.",
                serde_json::json!({
                    "to": {"type": "array", "minItems": 1, "items": {"type": "string", "minLength": 1}},
                    "cc": {"type": "array", "items": {"type": "string", "minLength": 1}},
                    "bcc": {"type": "array", "items": {"type": "string", "minLength": 1}},
                    "subject": {"type": "string"},
                    "body": {"type": "string"},
                }),
                vec!["to", "subject", "body"],
            ),
            tool(
                WorkspaceMailTool::Reply,
                "workspace_mail_reply",
                "Reply as the Workspace Mailbox. The reply keeps the thread and needs the same Approval as a send.",
                serde_json::json!({
                    "message_id": {"type": "string", "minLength": 1},
                    "body": {"type": "string"},
                }),
                vec!["message_id", "body"],
            ),
        ],
        event_kinds: vec![],
    }
}
