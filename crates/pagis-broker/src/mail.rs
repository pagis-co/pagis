//! The mail tool set (ADR-0019): one vocabulary of five tools
//! for every mailbox an Agent can act in.
//!
//! Every call names the mailbox it acts in. `own` is the Agent's own
//! Agent Mailbox, which it uses as its own identity with no Grant, so
//! the call takes the [`ToolRoute::Mailbox`] route. The broker resolves
//! the target per call, and the approval card the target answers with
//! names the address the mail leaves from.
//!
//! An allow rule here is one registrable domain of a recipient. The
//! broker derives it from the recipients of the call it is about to
//! make, and the user writes it with `Always allow`; the rule then
//! lives on the mailbox record. A later send is allowed only when every
//! one of its recipients sits on a rule, so an Agent cannot widen its
//! own reach by naming another domain.

use serde_json::Value;

use crate::{
    AllowRuleBuilder, ApprovalPresentation, CapabilityManifest, EffectClass, IncomingEventKind,
    ManifestTool, ToolDef, ToolResult, ToolRoute,
};

/// The namespace of the mail tools. Pagis mints it, so no manifest a
/// user installs may claim it.
pub const MAIL_NAMESPACE: &str = "mail";

/// The provider of a mail Connection. A user's mail account is a
/// Google Connection, so one name serves the tool route and the
/// Incoming Event (ADR-0019). A second mail provider adds its name
/// here and nowhere else.
pub const MAIL_PROVIDER: &str = "google";

/// The synced resource of a mail Connection. The Sync acquires each
/// message of the Person's account as a Source Item under it, and the
/// id of the Source Item is the message id of the provider. The parent
/// of an item is its thread (ADR-0011).
pub const MAIL_SYNC_RESOURCE: &str = "gmail";

pub const MAIL_SEARCH: &str = "mail__search";
pub const MAIL_GET_MESSAGE: &str = "mail__get_message";
pub const MAIL_GET_THREAD: &str = "mail__get_thread";
pub const MAIL_SEND: &str = "mail__send";
pub const MAIL_MODIFY_MESSAGE: &str = "mail__modify_message";

/// The value of the `mailbox` argument that names the Agent's own
/// Agent Mailbox (ADR-0019).
pub const OWN_MAILBOX: &str = "own";

/// The most domains one send card proposes as allow rules. A send that
/// reaches more distinct domains than this proposes none: a rule set
/// the user cannot read is worse than one approval.
pub const MAX_MAIL_RULES: usize = 5;

/// The stable code of a call that names a mailbox which cannot work
/// now (ADR-0019).
pub const MAILBOX_UNAVAILABLE: &str = "mailbox_unavailable";

/// The stable code of a send that would pass the Outgoing Cap.
pub const OUTGOING_CAP_REACHED: &str = "outgoing_cap_reached";

/// The one mail Incoming Event kind, for the Agent's own mailbox and
/// for the user's accounts alike (ADR-0019).
pub const MAIL_MESSAGE_RECEIVED: &str = "mail.message_received";

/// The name of the trusted matcher that reads its typed filter.
pub const MAIL_MATCHER: &str = "mail.message_received";

/// The provider of a Connection whose mailboxes the daemon makes
/// through the Migadu API (ADR-0019).
pub const MIGADU_PROVIDER: &str = "migadu";

/// The provider of a Connection whose mailboxes the user makes at the
/// host (ADR-0019).
pub const MANUAL_PROVIDER: &str = "manual";

/// The Connection providers that supply `mail.message_received` from
/// an Agent's own mailbox. A Gmail Connection joins them with the
/// user's own accounts.
pub const MAILBOX_PROVIDERS: [&str; 2] = [MIGADU_PROVIDER, MANUAL_PROVIDER];

/// Every provider that supplies the mail Incoming Event: the Mailbox
/// Providers behind an Agent Mailbox, and the mail Connection of the
/// user's own account (ADR-0019).
pub const MAIL_EVENT_PROVIDERS: [&str; 3] = [MIGADU_PROVIDER, MANUAL_PROVIDER, MAIL_PROVIDER];

/// The five mail tools (ADR-0019).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailTool {
    Search,
    GetMessage,
    GetThread,
    /// The one tool that reaches another person, so the one that waits
    /// for the user. A reply is a send with `in_reply_to`.
    Send,
    /// Reversible state: read, archived, flagged. Deletion is not a
    /// tool.
    ModifyMessage,
}

impl MailTool {
    pub fn as_str(self) -> &'static str {
        match self {
            MailTool::Search => MAIL_SEARCH,
            MailTool::GetMessage => MAIL_GET_MESSAGE,
            MailTool::GetThread => MAIL_GET_THREAD,
            MailTool::Send => MAIL_SEND,
            MailTool::ModifyMessage => MAIL_MODIFY_MESSAGE,
        }
    }
}

/// The mail manifest. Every tool takes the same `mailbox` argument;
/// the description of that argument names the mailboxes the calling
/// Agent has, and the broker writes it into each Run's snapshot.
pub fn mail_manifest() -> CapabilityManifest {
    let mailbox = serde_json::json!({
        "type": "string",
        "description": "The mailbox to act in."
    });
    let tool = |tool: MailTool,
                description: &str,
                properties: Value,
                required: Vec<&str>,
                effect: EffectClass,
                presentation: Option<ApprovalPresentation>| {
        let mut all = serde_json::Map::new();
        all.insert("mailbox".to_string(), mailbox.clone());
        if let Value::Object(properties) = properties {
            all.extend(properties);
        }
        let mut names = vec!["mailbox"];
        names.extend(required);
        ManifestTool {
            definition: ToolDef {
                name: tool.as_str().to_string(),
                description: description.to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": Value::Object(all),
                    "required": names
                }),
            },
            capability: None,
            effect,
            presentation,
            route: ToolRoute::Mailbox { tool },
            call_timeout: None,
            visibility: Default::default(),
            widget: None,
        }
    };
    CapabilityManifest {
        namespace: MAIL_NAMESPACE.to_string(),
        source_version: env!("CARGO_PKG_VERSION").to_string(),
        tools: vec![
            tool(
                MailTool::Search,
                "Find mail in one of your mailboxes. The query narrows the search with `from:`, \
                 `to:`, `subject:`, `since:` and `before:`; every other word is text to find in \
                 the body. An empty query answers with the newest mail. The answer holds message \
                 summaries with a short snippet, and a page token when there is more.",
                serde_json::json!({
                    "query": {
                        "type": "string",
                        "description": "What to look for, for example `from:alice@example.com invoice` or `subject:report since:2026-01-01`."
                    },
                    "max": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 25,
                        "description": "The most summaries to answer with. The default and the maximum are 25."
                    },
                    "page": {
                        "type": "string",
                        "description": "The page token from an earlier answer, to read the next page."
                    }
                }),
                vec!["query"],
                EffectClass::Free,
                None,
            ),
            tool(
                MailTool::GetMessage,
                "Read one message: its headers, its text body, and the name and size of each \
                 attachment. HTML becomes text. The bytes of an attachment stay at the host.",
                serde_json::json!({
                    "message_id": {
                        "type": "string",
                        "description": "The message id from a search. Do not write one yourself."
                    }
                }),
                vec!["message_id"],
                EffectClass::Free,
                None,
            ),
            tool(
                MailTool::GetThread,
                "Read one thread, oldest message first. A long thread answers with its newest \
                 messages and a page token for the older ones.",
                serde_json::json!({
                    "thread_id": {
                        "type": "string",
                        "description": "The thread id from a search or from a message."
                    },
                    "page": {
                        "type": "string",
                        "description": "The page token from an earlier answer, to read the older messages."
                    }
                }),
                vec!["thread_id"],
                EffectClass::Free,
                None,
            ),
            tool(
                MailTool::Send,
                "Send one email from a mailbox. The user approves it. To reply inside a thread, \
                 set `in_reply_to` to the `Message-ID` of the message you answer, and write the \
                 subject yourself. There is no draft tool: what you write here is what the user \
                 reads on the card.",
                serde_json::json!({
                    "to": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1,
                        "description": "The recipients."
                    },
                    "cc": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "The copy recipients."
                    },
                    "bcc": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "The blind copy recipients."
                    },
                    "subject": {"type": "string"},
                    "body": {"type": "string", "description": "The message text."},
                    "in_reply_to": {
                        "type": "string",
                        "description": "The `Message-ID` of the message this one answers, for example `<abc@example.com>`."
                    }
                }),
                vec!["to", "subject", "body"],
                EffectClass::Outbound,
                Some(ApprovalPresentation {
                    action_title: "Send an email".to_string(),
                    body_argument: Some("subject".to_string()),
                    allow_rule_builder: Some(AllowRuleBuilder::MailRecipientDomain),
                }),
            ),
            tool(
                MailTool::ModifyMessage,
                "Change the state of one message: mark it read or unread, move it to the archive, \
                 or flag it. Every change here is reversible. There is no tool that deletes mail.",
                serde_json::json!({
                    "message_id": {
                        "type": "string",
                        "description": "The message id from a search. Do not write one yourself."
                    },
                    "mark_read": {"type": "boolean", "description": "Mark the message read, or unread with false."},
                    "archive": {"type": "boolean", "description": "Move the message to the archive."},
                    "flag": {"type": "boolean", "description": "Flag the message, or clear the flag with false."}
                }),
                vec!["message_id"],
                EffectClass::Free,
                None,
            ),
        ],
        event_kinds: MAIL_EVENT_PROVIDERS
            .iter()
            .copied()
            .map(message_received)
            .collect(),
    }
}

/// The one mail Incoming Event kind, as one Mailbox Provider supplies
/// it (ADR-0019). Every provider declares the same metadata and the
/// same filter: what differs between them is the account behind the
/// mailbox, not what a message is.
pub fn message_received(provider: &str) -> IncomingEventKind {
    IncomingEventKind {
        name: MAIL_MESSAGE_RECEIVED.to_string(),
        // Never a body and never a snippet: the Agent reads those with
        // a live `mail__get_message` (ADR-0019).
        metadata_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "mailbox": {"type": "string"},
                "message_id": {"type": "string"},
                "thread_id": {"type": "string"},
                "in_reply_to": {"type": "string"},
                "from": {"type": "string"},
                "from_domain": {"type": "string"},
                "to": {"type": "array", "items": {"type": "string"}},
                "subject": {"type": "string"},
                "has_attachments": {"type": "boolean"},
                "received_at": {"type": "integer"},
                "trust_tier": {"type": "string", "enum": ["owner", "trusted", "unknown"]}
            },
            "required": ["mailbox", "message_id", "received_at"],
            "additionalProperties": false
        }),
        filter_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "mailbox": {
                    "type": "string",
                    "description": "`own` for your own mailbox, or the alias of one of the user's accounts."
                },
                "senders": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Email addresses or bare domains."
                },
                "subject_contains": {"type": "array", "items": {"type": "string"}},
                "min_trust": {
                    "type": "string",
                    "enum": ["owner", "trusted", "unknown"],
                    "description": "The lowest sender tier that wakes you."
                },
                "replies_only": {
                    "type": "boolean",
                    "description": "Only mail that answers a message you sent."
                }
            },
            "additionalProperties": false
        }),
        // The Agent holds its own mailbox as its own identity, with no
        // Grant; the user's account is read under a Grant on its
        // Connection (ADR-0019).
        required_capability: if provider == MAIL_PROVIDER {
            connection_capability(MailTool::Search).to_string()
        } else {
            String::new()
        },
        matcher: MAIL_MATCHER.to_string(),
        provider: provider.to_string(),
        // The Sync of the Person's account acquires each message as a
        // Source Item, and the collector gives the same message id as
        // the provider event id, so a Forget of the message blocks its
        // event too. An Agent Mailbox is the Agent's own identity on an
        // Installation Connection: no Sync acquires it, and a Forget
        // does not apply to it (ADR-0008).
        source_resource: (provider == MAIL_PROVIDER).then(|| MAIL_SYNC_RESOURCE.to_string()),
    }
}

/// The sender tiers, strongest first (ADR-0021, ADR-0019).
pub const TRUST_TIERS: [&str; 3] = ["owner", "trusted", "unknown"];

/// The mailboxes one Agent may name in one tool: its own address when
/// it holds a mailbox, and the alias of every mail Connection it is
/// granted for that tool (ADR-0019).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MailChoices {
    pub own_address: Option<String>,
    pub aliases: Vec<String>,
}

impl MailChoices {
    /// Whether this Agent may call the tool at all. An Agent with
    /// neither a mailbox nor a granted account sees no mail tool.
    pub fn is_empty(&self) -> bool {
        self.own_address.is_none() && self.aliases.is_empty()
    }

    /// The names the `mailbox` argument accepts, own mailbox first.
    fn names(&self) -> Vec<String> {
        let own = self.own_address.iter().map(|_| OWN_MAILBOX.to_string());
        own.chain(self.aliases.iter().cloned()).collect()
    }
}

/// The description one Run's snapshot carries, with the mailboxes the
/// calling Agent may name (ADR-0019).
pub fn describe_choices(description: &str, choices: &MailChoices) -> String {
    let mut named: Vec<String> = Vec::new();
    if let Some(address) = &choices.own_address {
        named.push(format!("`own` ({address}), your own address"));
    }
    for alias in &choices.aliases {
        named.push(format!("`{alias}`, one of the user's mail accounts"));
    }
    format!(
        "{description}\n\nMailboxes you can name: {}.",
        named.join("; ")
    )
}

/// The `mailbox` argument schema of one Run's snapshot: the names this
/// Agent may pass, so a wrong name is a schema error and never a call.
pub fn mailbox_choices(choices: &MailChoices) -> Value {
    serde_json::json!({
        "type": "string",
        "enum": choices.names(),
        "description": "The mailbox to act in. `own` is your own mailbox; every other name is \
                        one of the user's mail accounts."
    })
}

/// Put the mailboxes this Agent has into one tool definition.
pub fn resolve_choices(definition: &mut ToolDef, choices: &MailChoices) {
    definition.description = describe_choices(&definition.description, choices);
    if let Some(properties) = definition.parameters["properties"].as_object_mut() {
        properties.insert("mailbox".to_string(), mailbox_choices(choices));
    }
}

/// The Connection alias one call names, or `None` when it acts in the
/// Agent's own mailbox. The snapshot's enum has already refused every
/// other name, so this only tells the two targets apart (ADR-0019).
pub fn connection_alias(arguments: &Value) -> Option<String> {
    let named = arguments["mailbox"].as_str()?.trim();
    (!named.is_empty() && named != OWN_MAILBOX).then(|| named.to_string())
}

/// The capability a mail call in one of the user's accounts needs on
/// the Grant for its Connection. Each name is a capability of the Gmail
/// Connection, not of one mail tool (ADR-0019).
pub fn connection_capability(tool: MailTool) -> &'static str {
    match tool {
        MailTool::Search | MailTool::GetMessage | MailTool::GetThread => "gmail_read",
        MailTool::Send => "gmail_send",
        MailTool::ModifyMessage => "gmail_modify",
    }
}

/// Refuse a mail call whose text arguments are empty. The schema says
/// a field is a string; this says it must say something.
pub fn validate_arguments(tool: MailTool, arguments: &Value) -> Result<(), ToolResult> {
    let text = |field: &str| {
        arguments[field]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
    };
    let valid = match tool {
        // An empty query is the browse call: the newest mail.
        MailTool::Search => true,
        MailTool::GetMessage | MailTool::ModifyMessage => text("message_id"),
        MailTool::GetThread => text("thread_id"),
        MailTool::Send => {
            text("subject")
                && text("body")
                && !recipients(arguments).is_empty()
                && recipients(arguments)
                    .iter()
                    .all(|recipient| recipient.contains('@'))
        }
    };
    if valid {
        Ok(())
    } else {
        Err(ToolResult::plain_error(
            "invalid_request",
            format!("{}: name a message, a thread or a recipient", tool.as_str()),
        ))
    }
}

/// Every recipient of one send, `to`, `cc` and `bcc` together.
pub fn recipients(arguments: &Value) -> Vec<String> {
    ["to", "cc", "bcc"]
        .iter()
        .filter_map(|field| arguments[*field].as_array())
        .flatten()
        .filter_map(Value::as_str)
        .map(|recipient| recipient.trim().to_string())
        .filter(|recipient| !recipient.is_empty())
        .collect()
}

/// The registrable domain of one email address, lowercased. An address
/// with no domain, or a domain with no public suffix, has none.
pub fn recipient_domain(address: &str) -> Option<String> {
    let host = address
        .trim()
        .trim_end_matches('>')
        .rsplit_once('@')?
        .1
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    psl::domain_str(&host).map(str::to_string)
}

/// The allow rules one send proposes: the registrable domain of every
/// recipient, in the order the call names them. A send that reaches an
/// address Pagis cannot read a domain from, or more domains than
/// [`MAX_MAIL_RULES`], proposes nothing and the user approves it once.
pub fn proposed_domains(arguments: &Value) -> Vec<String> {
    let mut domains: Vec<String> = Vec::new();
    for recipient in recipients(arguments) {
        let Some(domain) = recipient_domain(&recipient) else {
            return Vec::new();
        };
        if !domains.contains(&domain) {
            domains.push(domain);
        }
    }
    if domains.len() > MAX_MAIL_RULES {
        return Vec::new();
    }
    domains
}

/// Whether the mailbox's rules already cover this send. Every
/// recipient must sit on a rule: one recipient outside them sends the
/// call to the card, so an added address cannot ride on an approved
/// one. An empty rule list never matches, which is per-send mode.
pub fn recipients_allowed(arguments: &Value, rules: &[String]) -> bool {
    let recipients = recipients(arguments);
    !recipients.is_empty()
        && !rules.is_empty()
        && recipients.iter().all(|recipient| {
            recipient_domain(recipient).is_some_and(|domain| {
                rules
                    .iter()
                    .any(|rule| rule.trim().to_ascii_lowercase() == domain)
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(to: &[&str], cc: &[&str]) -> Value {
        serde_json::json!({"mailbox": "own", "to": to, "cc": cc, "subject": "s", "body": "b"})
    }

    #[test]
    fn the_mail_event_of_a_mail_connection_is_a_source_item_of_its_sync() {
        // A Forget of a message blocks its event by the same id
        // (ADR-0008).
        assert_eq!(
            message_received(MAIL_PROVIDER).source_resource.as_deref(),
            Some(MAIL_SYNC_RESOURCE)
        );
        // An Agent Mailbox is the Agent's own identity on an
        // Installation Connection, and no Sync acquires it.
        for provider in MAILBOX_PROVIDERS {
            assert_eq!(message_received(provider).source_resource, None);
        }
    }

    #[test]
    fn a_rule_is_the_registrable_domain_of_every_recipient() {
        let call = send(&["alice@example.com"], &["bob@mail.example.org"]);

        assert_eq!(proposed_domains(&call), ["example.com", "example.org"]);
    }

    #[test]
    fn a_rule_covers_a_send_only_when_it_covers_every_recipient() {
        let rules = vec!["example.com".to_string()];

        assert!(recipients_allowed(
            &send(&["alice@example.com"], &[]),
            &rules
        ));
        assert!(!recipients_allowed(
            &send(&["alice@example.com"], &["eve@other.test"]),
            &rules
        ));
        assert!(!recipients_allowed(
            &send(&["alice@evil-example.com"], &[]),
            &rules
        ));
        assert!(!recipients_allowed(&send(&["alice@example.com"], &[]), &[]));
    }

    #[test]
    fn a_send_that_reaches_many_domains_proposes_no_rule() {
        let many = send(
            &[
                "a@one.test",
                "b@two.test",
                "c@three.test",
                "d@four.test",
                "e@five.test",
                "f@six.test",
            ],
            &[],
        );

        assert!(proposed_domains(&many).is_empty());
    }

    #[test]
    fn an_address_with_no_domain_proposes_no_rule_and_matches_none() {
        let broken = send(&["not-an-address"], &[]);

        assert!(proposed_domains(&broken).is_empty());
        assert!(!recipients_allowed(
            &broken,
            &["example.com".to_string(), "test".to_string()]
        ));
    }

    #[test]
    fn the_snapshot_names_the_mailboxes_the_agent_has() {
        let mut definition = mail_manifest().tools[0].definition.clone();

        resolve_choices(
            &mut definition,
            &MailChoices {
                own_address: Some("ava@example.com".to_string()),
                aliases: vec!["work".to_string()],
            },
        );

        assert!(definition.description.contains("ava@example.com"));
        assert!(definition.description.contains("`work`"));
        assert_eq!(
            definition.parameters["properties"]["mailbox"]["enum"],
            serde_json::json!(["own", "work"])
        );
    }

    #[test]
    fn an_agent_with_only_a_granted_account_names_no_own_mailbox() {
        let choices = MailChoices {
            own_address: None,
            aliases: vec!["work".to_string()],
        };

        assert!(!choices.is_empty());
        assert_eq!(
            mailbox_choices(&choices)["enum"],
            serde_json::json!(["work"])
        );
        assert!(MailChoices::default().is_empty());
    }

    #[test]
    fn a_call_names_its_target_with_the_mailbox_argument() {
        assert_eq!(
            connection_alias(&serde_json::json!({"mailbox": "work"})).as_deref(),
            Some("work")
        );
        assert_eq!(
            connection_alias(&serde_json::json!({"mailbox": "own"})),
            None
        );
        assert_eq!(connection_alias(&serde_json::json!({})), None);
    }
}
