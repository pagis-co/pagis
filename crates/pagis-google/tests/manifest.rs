use pagis_broker::{EffectClass, ToolRoute};
use pagis_broker::{
    MAIL_GET_MESSAGE, MAIL_GET_THREAD, MAIL_MODIFY_MESSAGE, MAIL_SEARCH, MAIL_SEND,
};
use pagis_core::knowledge::SourceRead;
use pagis_google::{
    CalendarEvent, GmailSearch, GoogleCall, call_from_tool, capability_manifest, source_reads,
};

fn minimal(name: &str) -> serde_json::Value {
    match name {
        "google__calendar_events" => serde_json::json!({}),
        "google__calendar_create_event" => serde_json::json!({
            "calendar_id": "primary", "summary": "Standup",
            "from": "2026-09-01T09:00:00Z", "to": "2026-09-01T09:15:00Z"
        }),
        "google__calendar_update_event" => {
            serde_json::json!({"calendar_id": "primary", "event_id": "e1"})
        }
        "google__calendar_delete_event" => {
            serde_json::json!({"calendar_id": "primary", "event_id": "e1"})
        }
        "google__calendar_respond_event" => {
            serde_json::json!({"calendar_id": "primary", "event_id": "e1", "status": "accepted"})
        }
        other => panic!("no arguments for {other}"),
    }
}

#[test]
fn every_tool_is_connection_routed_and_carries_its_capability() {
    let manifest = capability_manifest();
    assert_eq!(manifest.namespace, "google");
    assert_eq!(manifest.source_version, "gog-0.42.0");
    assert_eq!(manifest.tools.len(), 5);
    assert!(manifest.tools.iter().all(|tool| {
        tool.capability.is_some()
            && tool.route
                == ToolRoute::Connection {
                    provider: "google".to_string(),
                }
    }));
    assert_eq!(
        manifest.tools[0].capability.as_deref(),
        Some("calendar_read"),
        "grants name capabilities, not scopes"
    );
    // Mail is the `mail` manifest's, and so is its Incoming Event
    // (ADR-0019).
    assert!(
        manifest
            .tools
            .iter()
            .all(|tool| !tool.definition.name.contains("gmail"))
    );
    assert!(manifest.event_kinds.is_empty());
}

#[test]
fn pagis_policy_gates_what_reaches_another_person() {
    let manifest = capability_manifest();
    let effects: Vec<(&str, EffectClass, bool)> = manifest
        .tools
        .iter()
        .map(|tool| {
            (
                tool.definition.name.as_str(),
                tool.effect,
                tool.presentation.is_some(),
            )
        })
        .collect();

    assert_eq!(
        effects,
        [
            ("google__calendar_events", EffectClass::Free, false),
            ("google__calendar_create_event", EffectClass::Outbound, true),
            ("google__calendar_update_event", EffectClass::Outbound, true),
            (
                "google__calendar_delete_event",
                EffectClass::Destructive,
                true
            ),
            (
                "google__calendar_respond_event",
                EffectClass::Outbound,
                true
            ),
        ]
    );
    // No trusted allow-rule builder, so no `Always allow` for Google.
    assert!(
        manifest
            .tools
            .iter()
            .filter_map(|tool| tool.presentation.as_ref())
            .all(|approval| approval.allow_rule_builder.is_none())
    );
}

#[test]
fn the_schemas_and_the_parser_agree_on_every_tool() {
    for tool in capability_manifest().tools {
        let name = tool.definition.name.as_str();
        let call = call_from_tool(name, &minimal(name))
            .unwrap_or_else(|error| panic!("{name} does not parse: {error}"));
        // A validated call always produces a runnable command, and the
        // command agrees with the name-only write classification the
        // dispatcher uses to decide what an unfinished call reports.
        let command = pagis_google::GogCommand::for_call(
            &pagis_google::ConnectionBinding::new(
                "user@example.com",
                "client.json",
                "/tmp/pagis-test-gog-home",
            )
            .unwrap(),
            &call,
        )
        .unwrap_or_else(|error| panic!("{name} builds no command: {error}"));
        assert_eq!(
            command.is_write(),
            pagis_google::is_write(name),
            "{name} is classified differently by name and by command"
        );
    }
}

#[test]
fn typed_calls_carry_the_optional_fields_and_ignore_the_alias() {
    let search = call_from_tool(
        pagis_broker::MAIL_SEARCH,
        &serde_json::json!({"mailbox": "work", "query": "is:unread", "max": 5}),
    )
    .unwrap();
    assert_eq!(
        search,
        GoogleCall::GmailSearch(GmailSearch {
            query: "is:unread".to_string(),
            max: Some(5),
            page: None,
        })
    );

    let event = call_from_tool(
        "google__calendar_create_event",
        &serde_json::json!({
            "calendar_id": "primary", "summary": "Standup",
            "from": "2026-09-01T09:00:00Z", "to": "2026-09-01T09:15:00Z",
            "attendees": ["a@example.com"], "with_meet": true, "connection": "work"
        }),
    )
    .unwrap();
    assert_eq!(
        event,
        GoogleCall::CalendarCreateEvent(CalendarEvent {
            calendar_id: "primary".to_string(),
            summary: "Standup".to_string(),
            from: "2026-09-01T09:00:00Z".to_string(),
            to: "2026-09-01T09:15:00Z".to_string(),
            description: None,
            location: None,
            attendees: vec!["a@example.com".to_string()],
            timezone: None,
            send_updates: None,
            with_meet: true,
        })
    );
}

#[test]
fn an_unknown_tool_never_becomes_a_call() {
    assert!(call_from_tool("google__gmail_delete_everything", &serde_json::json!({})).is_err());
}

fn item(id: &str) -> SourceRead {
    SourceRead::Item {
        resource: pagis_broker::MAIL_SYNC_RESOURCE.to_string(),
        id: id.to_string(),
    }
}

fn parent(id: &str) -> SourceRead {
    SourceRead::Parent {
        resource: pagis_broker::MAIL_SYNC_RESOURCE.to_string(),
        id: id.to_string(),
    }
}

/// A read names the synced Gmail content that it returned, so a Forget
/// of one message reaches each Run that read it (ADR-0008). The message
/// and the thread come from the arguments of the call, and the messages
/// of a search from its result.
#[test]
fn a_read_names_the_synced_messages_that_it_returned() {
    let none = serde_json::json!({});
    assert_eq!(
        source_reads(
            MAIL_GET_MESSAGE,
            &serde_json::json!({"mailbox": "work", "message_id": "m-1"}),
            &serde_json::json!({"id": "m-1", "threadId": "t-1"}),
        ),
        [item("m-1")]
    );
    assert_eq!(
        source_reads(
            MAIL_GET_THREAD,
            &serde_json::json!({"mailbox": "work", "thread_id": "t-1"}),
            &none,
        ),
        [parent("t-1")]
    );
    assert_eq!(
        source_reads(
            MAIL_SEARCH,
            &serde_json::json!({"mailbox": "work", "query": "invoice"}),
            &serde_json::json!({
                "messages": [
                    {"id": "m-1", "threadId": "t-1", "subject": "Invoice"},
                    {"id": "m-2", "subject": "No thread"},
                    {"subject": "No id"}
                ],
                "nextPageToken": "page-2"
            }),
        ),
        [item("m-1"), parent("t-1"), item("m-2")]
    );
}

/// A write and a calendar read return no synced Gmail content.
#[test]
fn a_write_and_a_calendar_read_name_no_synced_message() {
    for (tool, arguments) in [
        (
            MAIL_MODIFY_MESSAGE,
            serde_json::json!({"mailbox": "work", "message_id": "m-1", "read": true}),
        ),
        (
            MAIL_SEND,
            serde_json::json!({"mailbox": "work", "to": ["a@example.com"], "subject": "Hi", "body": "Hi"}),
        ),
        ("google__calendar_events", serde_json::json!({})),
    ] {
        assert_eq!(
            source_reads(
                tool,
                &arguments,
                &serde_json::json!({"id": "m-1", "threadId": "t-1"})
            ),
            Vec::<SourceRead>::new(),
            "{tool} names a synced message"
        );
    }
}
