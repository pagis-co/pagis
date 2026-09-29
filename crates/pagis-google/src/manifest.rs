//! The Google capability manifest and the model-argument parser.
//!
//! The adapter owns both halves of its contract: the tool definitions
//! the broker installs, and the typed call each set of validated
//! arguments becomes. Nothing else parses a Google tool call, so the
//! schemas in `manifest()` and the arms here move together or not at
//! all.
//!
//! Mail is not in this manifest. `mail__*` is one tool set for every
//! mailbox (ADR-0019), and the broker sends a call that names a Gmail
//! Connection here under the Grant for that account. This file holds
//! the map from those five shared calls onto `gog`: Gmail keeps the
//! labels `UNREAD`, `INBOX` and `STARRED` for the three reversible
//! states the shared tool offers, and there is no draft and no custom
//! label.

use pagis_broker::{
    ApprovalPresentation, CapabilityManifest, EffectClass, MAIL_GET_MESSAGE, MAIL_GET_THREAD,
    MAIL_MODIFY_MESSAGE, MAIL_SEARCH, MAIL_SEND, ManifestTool, ToolRoute,
};

use pagis_core::knowledge::SourceRead;

use crate::{
    AdapterError, CalendarEvent, CalendarEventPatch, EmailDraft, GOG_VERSION, GmailSearch,
    GoogleCall, GoogleCapability, manifest,
};

/// The Gmail labels the three reversible states of `mail__modify_message`
/// map onto. Gmail has no archive folder: a message leaves the inbox
/// when `INBOX` is removed.
pub const UNREAD_LABEL: &str = "UNREAD";
pub const INBOX_LABEL: &str = "INBOX";
pub const STARRED_LABEL: &str = "STARRED";

pub const GOOGLE_NAMESPACE: &str = "google";
pub const GOOGLE_PROVIDER: &str = "google";

/// The manifest the broker installs. The effect class is Pagis policy,
/// not the provider's: a read and a reversible change run free, and
/// anything that reaches another person waits for the user.
pub fn capability_manifest() -> CapabilityManifest {
    CapabilityManifest {
        namespace: GOOGLE_NAMESPACE.to_string(),
        source_version: format!("gog-{GOG_VERSION}"),
        tools: manifest()
            .into_iter()
            .map(|tool| ManifestTool {
                capability: Some(tool.capability.as_str().to_string()),
                effect: effect_of(&tool.name),
                presentation: approval_of(&tool.name),
                route: ToolRoute::Connection {
                    provider: GOOGLE_PROVIDER.to_string(),
                },
                call_timeout: None,
                visibility: Default::default(),
                widget: None,
                definition: pagis_broker::ToolDef {
                    name: tool.name,
                    description: tool.description,
                    parameters: tool.parameters,
                },
            })
            .collect(),
        // Inbound mail is `mail.message_received`, declared by the
        // `mail` manifest for every mailbox (ADR-0019).
        event_kinds: Vec::new(),
    }
}

fn effect_of(name: &str) -> EffectClass {
    match name {
        "google__calendar_delete_event" => EffectClass::Destructive,
        "google__calendar_create_event"
        | "google__calendar_update_event"
        | "google__calendar_respond_event" => EffectClass::Outbound,
        // A read is bounded and reversible.
        _ => EffectClass::Free,
    }
}

/// Google supplies no trusted allow-rule builder, so every gated call
/// offers `Approve once` and `Deny` only (ADR-0005).
fn approval_of(name: &str) -> Option<ApprovalPresentation> {
    let (title, body) = match name {
        "google__calendar_create_event" => ("Create a calendar event", "summary"),
        "google__calendar_update_event" => ("Change a calendar event", "event_id"),
        "google__calendar_delete_event" => ("Delete a calendar event", "event_id"),
        "google__calendar_respond_event" => ("Respond to an invitation", "status"),
        _ => return None,
    };
    Some(ApprovalPresentation {
        action_title: title.to_string(),
        body_argument: Some(body.to_string()),
        allow_rule_builder: None,
    })
}

/// The synced Gmail content that one call on a Gmail Connection read,
/// for the record of what a Run read (ADR-0008). `mail__get_message`
/// reads the message that it names, `mail__get_thread` reads each
/// message of the thread that it names, and `mail__search` reads each
/// message that its result lists. No other call reads synced content.
pub fn source_reads(
    tool: &str,
    arguments: &serde_json::Value,
    result: &serde_json::Value,
) -> Vec<SourceRead> {
    let item = |id: &str| SourceRead::Item {
        resource: pagis_broker::MAIL_SYNC_RESOURCE.to_string(),
        id: id.to_string(),
    };
    let parent = |id: &str| SourceRead::Parent {
        resource: pagis_broker::MAIL_SYNC_RESOURCE.to_string(),
        id: id.to_string(),
    };
    let named = |key: &str| arguments[key].as_str().filter(|id| !id.is_empty());
    match tool {
        MAIL_GET_MESSAGE => named("message_id").map(item).into_iter().collect(),
        MAIL_GET_THREAD => named("thread_id").map(parent).into_iter().collect(),
        MAIL_SEARCH => crate::gmail_events::listed_messages(result)
            .into_iter()
            .flat_map(|(message, thread)| {
                std::iter::once(item(&message)).chain(thread.as_deref().map(parent))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether an interrupted call may already have changed something.
/// The contract test pins this against the command each tool builds, so
/// the two cannot drift apart. The mail tools are here too: a call that
/// names a Gmail Connection reaches this adapter (ADR-0019).
pub fn is_write(name: &str) -> bool {
    !matches!(
        name,
        MAIL_SEARCH | MAIL_GET_MESSAGE | MAIL_GET_THREAD | "google__calendar_events"
    )
}

/// Turn one validated tool call into its typed provider call. The
/// broker has already checked the arguments against the tool's schema,
/// so a failure here is a schema and parser that disagree.
pub fn call_from_tool(
    name: &str,
    arguments: &serde_json::Value,
) -> Result<GoogleCall, AdapterError> {
    Ok(match name {
        MAIL_SEARCH => GoogleCall::GmailSearch(GmailSearch {
            // The shared query language is Gmail's own for `from:`,
            // `to:` and `subject:`, and its date words differ, so the
            // two `YYYY-MM-DD` fields are rewritten. An empty query
            // browses, which Gmail answers with the newest mail.
            query: gmail_query(&string(arguments, "query").unwrap_or_default()),
            max: max(arguments),
            page: string(arguments, "page"),
        }),
        MAIL_GET_MESSAGE => GoogleCall::GmailGetMessage {
            message_id: required_string(arguments, "message_id")?,
        },
        MAIL_GET_THREAD => GoogleCall::GmailGetThread {
            thread_id: required_string(arguments, "thread_id")?,
        },
        MAIL_SEND => GoogleCall::GmailSend(EmailDraft {
            to: strings(arguments, "to"),
            cc: strings(arguments, "cc"),
            bcc: strings(arguments, "bcc"),
            subject: required_string(arguments, "subject")?,
            body: required_string(arguments, "body")?,
            in_reply_to: string(arguments, "in_reply_to"),
        }),
        MAIL_MODIFY_MESSAGE => modify_message(arguments)?,
        "google__calendar_events" => GoogleCall::CalendarEvents {
            calendar_id: string(arguments, "calendar_id"),
            from: string(arguments, "from"),
            to: string(arguments, "to"),
            query: string(arguments, "query"),
            max: max(arguments),
            page: string(arguments, "page"),
        },
        "google__calendar_create_event" => GoogleCall::CalendarCreateEvent(CalendarEvent {
            calendar_id: required_string(arguments, "calendar_id")?,
            summary: required_string(arguments, "summary")?,
            from: required_string(arguments, "from")?,
            to: required_string(arguments, "to")?,
            description: string(arguments, "description"),
            location: string(arguments, "location"),
            attendees: strings(arguments, "attendees"),
            timezone: string(arguments, "timezone"),
            send_updates: string(arguments, "send_updates"),
            with_meet: arguments["with_meet"].as_bool().unwrap_or(false),
        }),
        "google__calendar_update_event" => GoogleCall::CalendarUpdateEvent(CalendarEventPatch {
            calendar_id: required_string(arguments, "calendar_id")?,
            event_id: required_string(arguments, "event_id")?,
            summary: string(arguments, "summary"),
            from: string(arguments, "from"),
            to: string(arguments, "to"),
            description: string(arguments, "description"),
            location: string(arguments, "location"),
            attendees: arguments["attendees"]
                .as_array()
                .map(|_| strings(arguments, "attendees")),
            timezone: string(arguments, "timezone"),
            send_updates: string(arguments, "send_updates"),
        }),
        "google__calendar_delete_event" => GoogleCall::CalendarDeleteEvent {
            calendar_id: required_string(arguments, "calendar_id")?,
            event_id: required_string(arguments, "event_id")?,
            send_updates: string(arguments, "send_updates"),
        },
        "google__calendar_respond_event" => GoogleCall::CalendarRespondEvent {
            calendar_id: required_string(arguments, "calendar_id")?,
            event_id: required_string(arguments, "event_id")?,
            status: required_string(arguments, "status")?,
            comment: string(arguments, "comment"),
        },
        _ => return Err(AdapterError::InvalidArguments("tool")),
    })
}

/// Map the three reversible states onto Gmail labels. A call that
/// asks for no change at all is a call with nothing to do, and `gog`
/// refuses an empty label change, so it is refused here.
fn modify_message(arguments: &serde_json::Value) -> Result<GoogleCall, AdapterError> {
    let mut add = Vec::new();
    let mut remove = Vec::new();
    let mut label = |wanted: bool, name: &str| {
        if wanted {
            add.push(name.to_string());
        } else {
            remove.push(name.to_string());
        }
    };
    if let Some(read) = arguments["mark_read"].as_bool() {
        // Gmail marks a message unread with a label, so reading it is
        // removing that label.
        label(!read, UNREAD_LABEL);
    }
    if let Some(flagged) = arguments["flag"].as_bool() {
        label(flagged, STARRED_LABEL);
    }
    if arguments["archive"].as_bool() == Some(true) {
        remove.push(INBOX_LABEL.to_string());
    }
    if add.is_empty() && remove.is_empty() {
        return Err(AdapterError::InvalidArguments("labels"));
    }
    Ok(GoogleCall::GmailModifyMessage {
        message_id: required_string(arguments, "message_id")?,
        add_labels: add,
        remove_labels: remove,
    })
}

/// The Gmail query that browses: everything but chat. An empty shared
/// query asks for the newest mail, and `gog` refuses an empty search.
pub const BROWSE_QUERY: &str = "-in:chats";

/// Rewrite the shared query language into Gmail's own. Only the two
/// date fields differ: Gmail reads `after:` and `before:` and takes
/// `YYYY/MM/DD`, and `since:` is inclusive where `after:` is not.
fn gmail_query(query: &str) -> String {
    if query.trim().is_empty() {
        return BROWSE_QUERY.to_string();
    }
    query
        .split_whitespace()
        .map(|token| match token.split_once(':') {
            Some(("since", date)) => rewrite_date("after", date, -1),
            Some(("before", date)) => rewrite_date("before", date, 0),
            _ => token.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One `YYYY-MM-DD` value as a Gmail date word, moved by whole days so
/// an exclusive bound covers the day the caller named.
fn rewrite_date(word: &str, date: &str, days: i64) -> String {
    match chrono::NaiveDate::parse_from_str(date.trim_matches('"'), "%Y-%m-%d") {
        Ok(parsed) => {
            let moved = parsed + chrono::Duration::days(days);
            format!("{word}:{}", moved.format("%Y/%m/%d"))
        }
        // A value Pagis cannot read goes to Gmail as it stands: the
        // provider refuses it, and the model reads why.
        Err(_) => format!("{word}:{date}"),
    }
}

fn required_string(
    arguments: &serde_json::Value,
    name: &'static str,
) -> Result<String, AdapterError> {
    string(arguments, name).ok_or(AdapterError::InvalidArguments(name))
}

fn string(arguments: &serde_json::Value, name: &str) -> Option<String> {
    arguments[name].as_str().map(str::to_string)
}

fn strings(arguments: &serde_json::Value, name: &str) -> Vec<String> {
    arguments[name]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn max(arguments: &serde_json::Value) -> Option<u32> {
    arguments["max"]
        .as_u64()
        .and_then(|max| u32::try_from(max).ok())
}

impl GoogleCapability {
    /// Every capability a Google Connection can be authorized for.
    pub const ALL: [Self; 5] = [
        Self::GmailRead,
        Self::GmailSend,
        Self::GmailModify,
        Self::CalendarRead,
        Self::CalendarWrite,
    ];

    /// The set a new Connection starts at: read, and nothing that
    /// reaches another person. The user widens it by reauthorizing.
    pub const READ_ONLY: [Self; 2] = [Self::GmailRead, Self::CalendarRead];

    /// The name a Connection grant carries for this capability.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GmailRead => "gmail_read",
            Self::GmailSend => "gmail_send",
            Self::GmailModify => "gmail_modify",
            Self::CalendarRead => "calendar_read",
            Self::CalendarWrite => "calendar_write",
        }
    }

    /// Read back a capability the user asked to widen a Connection to.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|capability| capability.as_str() == name)
    }
}
