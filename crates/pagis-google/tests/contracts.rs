use pagis_broker::{
    MAIL_GET_MESSAGE, MAIL_GET_THREAD, MAIL_MODIFY_MESSAGE, MAIL_SEARCH, MAIL_SEND,
};
use pagis_google::{
    AdapterError, BROWSE_QUERY, CalendarEvent, CalendarEventPatch, ConnectionBinding, EmailDraft,
    GOG_VERSION, GmailSearch, GogCommand, GoogleCall, GoogleCapability, INBOX_LABEL,
    ProcessFailure, ProcessOutput, ProviderErrorCode, STARRED_LABEL, UNREAD_LABEL, call_from_tool,
    is_write, manifest, normalize_output,
};

/// The keyring of these bodies: the password is in memory, so no test
/// touches the machine's keychain.
fn test_keyring() -> std::sync::Arc<dyn pagis_google::GogKeyring> {
    std::sync::Arc::new(pagis_google::SecretStoreKeyring::new(std::sync::Arc::new(
        pagis_core::MemorySecretStore::default(),
    )))
}

#[test]
fn the_manifest_is_calendar_only_and_names_no_mail_tool() {
    assert_eq!(GOG_VERSION, "0.42.0");

    let tools = manifest();
    let contracts: Vec<(&str, GoogleCapability)> = tools
        .iter()
        .map(|tool| (tool.name.as_str(), tool.capability))
        .collect();

    assert_eq!(
        contracts,
        [
            ("google__calendar_events", GoogleCapability::CalendarRead),
            (
                "google__calendar_create_event",
                GoogleCapability::CalendarWrite
            ),
            (
                "google__calendar_update_event",
                GoogleCapability::CalendarWrite
            ),
            (
                "google__calendar_delete_event",
                GoogleCapability::CalendarWrite
            ),
            (
                "google__calendar_respond_event",
                GoogleCapability::CalendarWrite
            ),
        ]
    );
    assert!(tools.iter().all(|tool| tool.parameters["type"] == "object"));
}

#[test]
fn automation_exit_codes_map_to_stable_sanitized_errors() {
    let cases = [
        (4, false, ProviderErrorCode::ReauthRequired, false),
        (6, false, ProviderErrorCode::PermissionRevoked, false),
        (7, false, ProviderErrorCode::TemporarilyUnavailable, true),
        (8, false, ProviderErrorCode::TemporarilyUnavailable, true),
        (8, true, ProviderErrorCode::OutcomeUnknown, false),
        (1, true, ProviderErrorCode::OutcomeUnknown, false),
        (5, false, ProviderErrorCode::NotFound, false),
        (2, false, ProviderErrorCode::InvalidRequest, false),
    ];

    for (status, write, code, retryable) in cases {
        let error = normalize_output(
            ProcessOutput {
                status: Some(status),
                stdout: b"not returned".to_vec(),
            },
            write,
        )
        .unwrap_err();
        assert_eq!(error.code, code);
        assert_eq!(error.retryable, retryable);
        assert_eq!(error.to_string(), code.as_str());
        assert!(!format!("{error:?}").contains("not returned"));
    }

    assert_eq!(
        normalize_output(
            ProcessOutput {
                status: Some(0),
                stdout: br#"{"id":"m1"}"#.to_vec()
            },
            true
        )
        .unwrap(),
        serde_json::json!({"id": "m1"})
    );
    assert_eq!(
        normalize_output(
            ProcessOutput {
                status: Some(3),
                stdout: Vec::new()
            },
            false
        )
        .unwrap(),
        serde_json::json!({})
    );

    let before_dispatch = ProcessFailure::NotStarted.into_provider_error(false);
    assert_eq!(
        before_dispatch.code,
        ProviderErrorCode::TemporarilyUnavailable
    );
    assert!(before_dispatch.retryable);
    let interrupted_write = ProcessFailure::Interrupted.into_provider_error(true);
    assert_eq!(interrupted_write.code, ProviderErrorCode::OutcomeUnknown);
    assert!(!interrupted_write.retryable);
}

#[test]
fn model_schemas_pin_the_typed_surface_and_hide_trusted_configuration() {
    let contracts: Vec<(String, Vec<String>, Vec<String>)> = manifest()
        .into_iter()
        .map(|tool| {
            let properties = tool.parameters["properties"]
                .as_object()
                .expect("properties object");
            let required = tool.parameters["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            // The schema keeps its properties in the order the code
            // writes them; the contract is the set of names.
            let mut property_names: Vec<String> = properties.keys().cloned().collect();
            property_names.sort();
            (
                tool.name,
                property_names,
                required
                    .iter()
                    .map(|value| value.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();

    let expected: Vec<(String, Vec<String>, Vec<String>)> = [
        (
            "google__calendar_events",
            vec!["calendar_id", "from", "max", "page", "query", "to"],
            vec![],
        ),
        (
            "google__calendar_create_event",
            vec![
                "attendees",
                "calendar_id",
                "description",
                "from",
                "location",
                "send_updates",
                "summary",
                "timezone",
                "to",
                "with_meet",
            ],
            vec!["calendar_id", "from", "summary", "to"],
        ),
        (
            "google__calendar_update_event",
            vec![
                "attendees",
                "calendar_id",
                "description",
                "event_id",
                "from",
                "location",
                "send_updates",
                "summary",
                "timezone",
                "to",
            ],
            vec!["calendar_id", "event_id"],
        ),
        (
            "google__calendar_delete_event",
            vec!["calendar_id", "event_id", "send_updates"],
            vec!["calendar_id", "event_id"],
        ),
        (
            "google__calendar_respond_event",
            vec!["calendar_id", "comment", "event_id", "status"],
            vec!["calendar_id", "event_id", "status"],
        ),
    ]
    .into_iter()
    .map(|(name, properties, required)| {
        (
            name.to_string(),
            properties.into_iter().map(str::to_string).collect(),
            required.into_iter().map(str::to_string).collect(),
        )
    })
    .collect();
    assert_eq!(contracts, expected);

    let encoded = serde_json::to_string(&manifest()).unwrap();
    for forbidden in ["account", "client_secret", "credential", "shell", "token"] {
        assert!(
            !encoded.contains(forbidden),
            "manifest contains {forbidden}"
        );
    }
}

/// Each call puts its flags in front of `--`, each flag with its value in
/// one item, and its positional values after `--`.
#[test]
fn every_tool_call_builds_one_fixed_gog_argument_array() {
    let binding = ConnectionBinding::new(
        "alice@example.com",
        "pagis-google-01",
        "/tmp/pagis-test-gog-home",
    )
    .unwrap();
    let cases = [
        (
            GoogleCall::GmailSearch(GmailSearch {
                query: "label:inbox newer_than:7d".into(),
                max: Some(25),
                page: Some("next".into()),
            }),
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "gmail.search",
                "--no-input",
                "--wrap-untrusted",
                "--readonly",
                "--json",
                "gmail",
                "search",
                "--max=25",
                "--page=next",
                "--",
                "label:inbox newer_than:7d",
            ],
        ),
        (
            GoogleCall::GmailGetMessage {
                message_id: "m1".into(),
            },
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "gmail.get",
                "--no-input",
                "--wrap-untrusted",
                "--readonly",
                "--json",
                "gmail",
                "get",
                "--sanitize-content",
                "--",
                "m1",
            ],
        ),
        (
            GoogleCall::GmailGetThread {
                thread_id: "t1".into(),
            },
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "gmail.thread.get",
                "--no-input",
                "--wrap-untrusted",
                "--readonly",
                "--json",
                "gmail",
                "thread",
                "get",
                "--sanitize-content",
                "--",
                "t1",
            ],
        ),
        (
            GoogleCall::CalendarEvents {
                calendar_id: Some("primary".into()),
                from: Some("2026-08-29T00:00:00Z".into()),
                to: Some("2026-08-30T00:00:00Z".into()),
                query: Some("planning".into()),
                max: Some(10),
                page: None,
            },
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "calendar.events",
                "--no-input",
                "--wrap-untrusted",
                "--readonly",
                "--json",
                "calendar",
                "events",
                "--from=2026-08-29T00:00:00Z",
                "--to=2026-08-30T00:00:00Z",
                "--query=planning",
                "--max=10",
                "--",
                "primary",
            ],
        ),
        (
            GoogleCall::GmailSend(EmailDraft {
                to: vec!["bob@example.com".into()],
                cc: vec!["carol@example.com".into()],
                bcc: vec![],
                subject: "Hello".into(),
                body: "Hi Bob".into(),
                in_reply_to: Some("<abc@example.com>".into()),
            }),
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "gmail.send",
                "--no-input",
                "--wrap-untrusted",
                "--json",
                "gmail",
                "send",
                "--to=bob@example.com",
                "--cc=carol@example.com",
                "--subject=Hello",
                "--body=Hi Bob",
                "--reply-to-message-id=<abc@example.com>",
                "--",
            ],
        ),
        (
            GoogleCall::GmailModifyMessage {
                message_id: "m1".into(),
                add_labels: vec!["STARRED".into()],
                remove_labels: vec!["INBOX".into()],
            },
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "gmail.messages.modify",
                "--no-input",
                "--wrap-untrusted",
                "--json",
                "gmail",
                "messages",
                "modify",
                "--add=STARRED",
                "--remove=INBOX",
                "--",
                "m1",
            ],
        ),
        (
            GoogleCall::CalendarCreateEvent(CalendarEvent {
                calendar_id: "primary".into(),
                summary: "Planning".into(),
                from: "2026-09-01T09:00:00-07:00".into(),
                to: "2026-09-01T10:00:00-07:00".into(),
                description: Some("Roadmap".into()),
                location: None,
                attendees: vec!["bob@example.com".into()],
                timezone: Some("America/Los_Angeles".into()),
                send_updates: Some("all".into()),
                with_meet: true,
            }),
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "calendar.create",
                "--no-input",
                "--wrap-untrusted",
                "--json",
                "calendar",
                "create",
                "--summary=Planning",
                "--from=2026-09-01T09:00:00-07:00",
                "--to=2026-09-01T10:00:00-07:00",
                "--description=Roadmap",
                "--attendees=bob@example.com",
                "--timezone=America/Los_Angeles",
                "--send-updates=all",
                "--with-meet",
                "--",
                "primary",
            ],
        ),
        (
            GoogleCall::CalendarUpdateEvent(CalendarEventPatch {
                calendar_id: "primary".into(),
                event_id: "e1".into(),
                summary: Some("New title".into()),
                from: None,
                to: None,
                description: None,
                location: None,
                attendees: None,
                timezone: None,
                send_updates: None,
            }),
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "calendar.update",
                "--no-input",
                "--wrap-untrusted",
                "--json",
                "calendar",
                "update",
                "--summary=New title",
                "--",
                "primary",
                "e1",
            ],
        ),
        (
            GoogleCall::CalendarUpdateEvent(CalendarEventPatch {
                calendar_id: "primary".into(),
                event_id: "e1".into(),
                summary: None,
                from: Some("2026-09-01T09:00:00".into()),
                to: Some("2026-09-01T10:00:00".into()),
                description: None,
                location: None,
                attendees: None,
                timezone: Some("America/Los_Angeles".into()),
                send_updates: None,
            }),
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "calendar.update",
                "--no-input",
                "--wrap-untrusted",
                "--json",
                "calendar",
                "update",
                "--from=2026-09-01T09:00:00",
                "--to=2026-09-01T10:00:00",
                "--start-timezone=America/Los_Angeles",
                "--end-timezone=America/Los_Angeles",
                "--",
                "primary",
                "e1",
            ],
        ),
        (
            GoogleCall::CalendarDeleteEvent {
                calendar_id: "primary".into(),
                event_id: "e1".into(),
                send_updates: Some("all".into()),
            },
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "calendar.delete",
                "--no-input",
                "--wrap-untrusted",
                "--json",
                "calendar",
                "delete",
                "--force",
                "--send-updates=all",
                "--",
                "primary",
                "e1",
            ],
        ),
        (
            GoogleCall::CalendarRespondEvent {
                calendar_id: "primary".into(),
                event_id: "e1".into(),
                status: "accepted".into(),
                comment: Some("See you there".into()),
            },
            vec![
                "--account",
                "alice@example.com",
                "--client",
                "pagis-google-01",
                "--enable-commands-exact",
                "calendar.respond",
                "--no-input",
                "--wrap-untrusted",
                "--json",
                "calendar",
                "respond",
                "--status=accepted",
                "--comment=See you there",
                "--",
                "primary",
                "e1",
            ],
        ),
    ];

    for (call, expected) in cases {
        let command = GogCommand::for_call(&binding, &call).unwrap();
        assert_eq!(command.args(), expected);
    }
}

/// The binding of the tests below: the account and the client of the one
/// Connection that the Agent has a Grant for.
fn bound() -> ConnectionBinding {
    ConnectionBinding::new(
        "alice@example.com",
        "pagis-google-01",
        "/tmp/pagis-test-gog-home",
    )
    .unwrap()
}

/// Values that `gog` reads as an option when one argv item holds only
/// the value: another client, another account, a switch that turns off a
/// flag of the adapter, and the help flag.
const OPTION_SHAPED: [&str; 4] = [
    "--client=other",
    "--account=x@example.com",
    "--readonly=false",
    "-h",
];

/// Each call with `text` in each slot that holds free text from the
/// Agent. The ID slots hold valid IDs.
fn calls_with_text(text: &str) -> Vec<GoogleCall> {
    let text = text.to_string();
    vec![
        GoogleCall::GmailSearch(GmailSearch {
            query: text.clone(),
            max: Some(5),
            page: Some(text.clone()),
        }),
        GoogleCall::GmailGetMessage {
            message_id: "m1".into(),
        },
        GoogleCall::GmailGetThread {
            thread_id: "t1".into(),
        },
        GoogleCall::CalendarEvents {
            calendar_id: Some("primary".into()),
            from: Some(text.clone()),
            to: Some(text.clone()),
            query: Some(text.clone()),
            max: Some(5),
            page: Some(text.clone()),
        },
        GoogleCall::GmailSend(EmailDraft {
            to: vec![text.clone()],
            cc: vec![text.clone()],
            bcc: vec![text.clone()],
            subject: text.clone(),
            body: text.clone(),
            in_reply_to: Some(text.clone()),
        }),
        GoogleCall::GmailModifyMessage {
            message_id: "m1".into(),
            add_labels: vec![text.clone()],
            remove_labels: vec![text.clone()],
        },
        GoogleCall::CalendarCreateEvent(CalendarEvent {
            calendar_id: "primary".into(),
            summary: text.clone(),
            from: text.clone(),
            to: text.clone(),
            description: Some(text.clone()),
            location: Some(text.clone()),
            attendees: vec![text.clone()],
            timezone: Some(text.clone()),
            send_updates: None,
            with_meet: true,
        }),
        GoogleCall::CalendarUpdateEvent(CalendarEventPatch {
            calendar_id: "primary".into(),
            event_id: "e1".into(),
            summary: Some(text.clone()),
            from: Some(text.clone()),
            to: Some(text.clone()),
            description: Some(text.clone()),
            location: Some(text.clone()),
            attendees: Some(vec![text.clone()]),
            timezone: Some(text.clone()),
            send_updates: None,
        }),
        GoogleCall::CalendarDeleteEvent {
            calendar_id: "primary".into(),
            event_id: "e1".into(),
            send_updates: None,
        },
        GoogleCall::CalendarRespondEvent {
            calendar_id: "primary".into(),
            event_id: "e1".into(),
            status: "accepted".into(),
            comment: Some(text),
        },
    ]
}

/// Each call with `id` in one ID slot, with the name of that slot.
fn calls_with_id(id: &str) -> Vec<(&'static str, GoogleCall)> {
    let id = id.to_string();
    let event = |calendar_id: &str| CalendarEvent {
        calendar_id: calendar_id.into(),
        summary: "Planning".into(),
        from: "2026-09-01T09:00:00Z".into(),
        to: "2026-09-01T10:00:00Z".into(),
        description: None,
        location: None,
        attendees: vec![],
        timezone: None,
        send_updates: None,
        with_meet: false,
    };
    let patch = |calendar_id: &str, event_id: &str| CalendarEventPatch {
        calendar_id: calendar_id.into(),
        event_id: event_id.into(),
        summary: Some("Planning".into()),
        from: None,
        to: None,
        description: None,
        location: None,
        attendees: None,
        timezone: None,
        send_updates: None,
    };
    let delete = |calendar_id: &str, event_id: &str| GoogleCall::CalendarDeleteEvent {
        calendar_id: calendar_id.into(),
        event_id: event_id.into(),
        send_updates: None,
    };
    let respond = |calendar_id: &str, event_id: &str| GoogleCall::CalendarRespondEvent {
        calendar_id: calendar_id.into(),
        event_id: event_id.into(),
        status: "accepted".into(),
        comment: None,
    };
    vec![
        (
            "message_id",
            GoogleCall::GmailGetMessage {
                message_id: id.clone(),
            },
        ),
        (
            "thread_id",
            GoogleCall::GmailGetThread {
                thread_id: id.clone(),
            },
        ),
        (
            "calendar_id",
            GoogleCall::CalendarEvents {
                calendar_id: Some(id.clone()),
                from: None,
                to: None,
                query: None,
                max: None,
                page: None,
            },
        ),
        (
            "message_id",
            GoogleCall::GmailModifyMessage {
                message_id: id.clone(),
                add_labels: vec![STARRED_LABEL.into()],
                remove_labels: vec![],
            },
        ),
        ("calendar_id", GoogleCall::CalendarCreateEvent(event(&id))),
        (
            "calendar_id",
            GoogleCall::CalendarUpdateEvent(patch(&id, "e1")),
        ),
        (
            "event_id",
            GoogleCall::CalendarUpdateEvent(patch("primary", &id)),
        ),
        ("calendar_id", delete(&id, "e1")),
        ("event_id", delete("primary", &id)),
        ("calendar_id", respond(&id, "e1")),
        ("event_id", respond("primary", &id)),
    ]
}

/// The options and the positional values of one argv: the items in front
/// of the first `--`, and the items after it. An argv without `--` is all
/// options.
fn split_at_terminator(args: &[String]) -> (&[String], &[String]) {
    match args.iter().position(|arg| arg == "--") {
        Some(at) => (&args[..at], &args[at + 1..]),
        None => (args, &[]),
    }
}

/// `gog` reads an argv item that starts with `-` as an option, also after
/// a positional value. So a value from the Agent goes after `--`, or into
/// one `--flag=value` item, and an ID of that shape is refused.
#[test]
fn an_option_shaped_agent_value_never_reaches_gog_as_an_option() {
    for value in OPTION_SHAPED {
        for call in calls_with_text(value) {
            let command = GogCommand::for_call(&bound(), &call)
                .unwrap_or_else(|error| panic!("{call:?}: {error}"));
            let (options, _) = split_at_terminator(command.args());
            for item in options.iter().filter(|item| item.contains(value)) {
                let Some((flag, flag_value)) = item.split_once('=') else {
                    panic!("{value} is an option in {:?}", command.args());
                };
                assert!(
                    flag.starts_with("--") && !flag.contains(value) && flag_value.contains(value),
                    "{value} is an option in {:?}",
                    command.args()
                );
            }
        }
        for (field, call) in calls_with_id(value) {
            assert_eq!(
                GogCommand::for_call(&bound(), &call).unwrap_err(),
                AdapterError::InvalidArguments(field),
                "{call:?}"
            );
        }
    }
}

/// In front of `--`, the only account and the only client are the ones
/// of the bound Connection, whatever the Agent sends.
#[test]
fn every_call_selects_only_the_account_and_the_client_of_its_connection() {
    for value in OPTION_SHAPED.into_iter().chain(["planning"]) {
        for call in calls_with_text(value) {
            let command = GogCommand::for_call(&bound(), &call).unwrap();
            let args = command.args();
            assert!(
                args.iter().any(|arg| arg == "--"),
                "{args:?} has no end of the options"
            );
            let (options, _) = split_at_terminator(args);
            let value_of = |selector: &str| {
                let at: Vec<usize> = options
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| *item == selector)
                    .map(|(at, _)| at)
                    .collect();
                assert_eq!(at.len(), 1, "{selector} in {args:?}");
                options[at[0] + 1].as_str()
            };
            assert_eq!(value_of("--account"), "alice@example.com", "{args:?}");
            assert_eq!(value_of("--client"), "pagis-google-01", "{args:?}");
            assert!(
                !options.iter().any(|item| item.starts_with("--account=")
                    || item.starts_with("--client=")
                    || item.starts_with("-a")),
                "{args:?}"
            );
        }
    }
}

/// A test runner that records each command it is asked to start.
#[derive(Default)]
struct Recorder {
    started: std::sync::Mutex<Vec<Vec<String>>>,
}

#[async_trait::async_trait]
impl pagis_google::GogRunner for Recorder {
    async fn run(&self, command: &GogCommand) -> Result<ProcessOutput, ProcessFailure> {
        self.started.lock().unwrap().push(command.args().to_vec());
        Ok(ProcessOutput {
            status: Some(0),
            stdout: b"{}".to_vec(),
        })
    }
}

/// An ID must start with an ASCII letter or digit and hold no whitespace
/// and no control character. Any other ID is refused with the name of its
/// field, and `gog` does not start.
#[tokio::test]
async fn an_id_of_another_shape_is_refused_and_gog_does_not_start() {
    let recorder = std::sync::Arc::new(Recorder::default());
    let provider = pagis_google::GoogleProvider::new(bound(), std::sync::Arc::clone(&recorder));
    for id in [
        "-h",
        "--client=other",
        "-primary",
        "",
        "_e1",
        " primary",
        "primary events",
        "e1\t",
        "m1\n",
        "primary\u{a0}",
        "m\u{7}1",
        "m\u{0}1",
        "e1\u{7f}",
    ] {
        for (field, call) in calls_with_id(id) {
            assert_eq!(
                GogCommand::for_call(&bound(), &call).unwrap_err(),
                AdapterError::InvalidArguments(field),
                "{id:?} in {call:?}"
            );
            let refused = provider.invoke(&call).await.unwrap_err();
            assert_eq!(refused.code, ProviderErrorCode::InvalidRequest, "{id:?}");
            assert!(!refused.retryable, "{id:?}");
        }
    }
    assert!(recorder.started.lock().unwrap().is_empty());
}

/// Google message, thread, calendar and event IDs pass the shape check,
/// and reach `gog` after `--`.
#[test]
fn google_ids_pass_the_shape_check() {
    for id in [
        "primary",
        "name@group.calendar.google.com",
        "en.usa#holiday@group.v.calendar.google.com",
        "18c2f0a1b2c3d4e5",
        "7kq2rtd4l9g1b0s8vh6o3m5n2c_20260901T160000Z",
    ] {
        for (field, call) in calls_with_id(id) {
            let command = GogCommand::for_call(&bound(), &call)
                .unwrap_or_else(|error| panic!("{field} {id}: {error}"));
            let (_, positionals) = split_at_terminator(command.args());
            assert!(
                positionals.iter().any(|item| item == id),
                "{field} {id}: {:?}",
                command.args()
            );
        }
    }
}

/// `-label:spam` is Gmail query syntax. The query keeps its leading `-`
/// and reaches `gog` after `--`, as the one positional value. The query
/// that browses starts with `-` too.
#[test]
fn a_gmail_query_that_starts_with_a_dash_reaches_gog_as_the_query() {
    for (query, expected) in [("-label:spam", "-label:spam"), ("", BROWSE_QUERY)] {
        let call = call_from_tool(
            MAIL_SEARCH,
            &serde_json::json!({"mailbox": "work", "query": query}),
        )
        .unwrap();
        let command = GogCommand::for_call(&bound(), &call).unwrap();
        let (options, positionals) = split_at_terminator(command.args());
        assert_eq!(positionals, [expected], "{:?}", command.args());
        assert!(!options.iter().any(|item| item == expected));
    }
}

/// `gog` has no `--timezone` on `calendar update`. It takes the time zone
/// of each end of the event with the new time of that end:
/// `--start-timezone` needs `--from`, and `--end-timezone` needs `--to`. So
/// the time zone of a patch goes to each end that the patch moves, and a
/// patch that moves no end cannot hold a time zone.
#[test]
fn an_update_gives_its_timezone_to_each_end_that_it_moves() {
    let patch = |from: Option<&str>, to: Option<&str>| {
        GoogleCall::CalendarUpdateEvent(CalendarEventPatch {
            calendar_id: "primary".into(),
            event_id: "e1".into(),
            summary: Some("Planning".into()),
            from: from.map(Into::into),
            to: to.map(Into::into),
            description: None,
            location: None,
            attendees: None,
            timezone: Some("Europe/Rome".into()),
            send_updates: None,
        })
    };
    let options = |call: &GoogleCall| {
        let command = GogCommand::for_call(&bound(), call).unwrap();
        let (options, _) = split_at_terminator(command.args());
        options.to_vec()
    };

    let start_only = options(&patch(Some("2026-09-01T09:00:00"), None));
    assert!(start_only.contains(&"--start-timezone=Europe/Rome".to_string()));
    assert!(
        !start_only
            .iter()
            .any(|item| item.contains("-timezone=") && item != "--start-timezone=Europe/Rome")
    );

    let end_only = options(&patch(None, Some("2026-09-01T10:00:00")));
    assert!(end_only.contains(&"--end-timezone=Europe/Rome".to_string()));
    assert!(
        !end_only
            .iter()
            .any(|item| item.contains("-timezone=") && item != "--end-timezone=Europe/Rome")
    );

    assert_eq!(
        GogCommand::for_call(&bound(), &patch(None, None)).unwrap_err(),
        AdapterError::InvalidArguments("timezone")
    );
}

/// The calendar ID of `google__calendar_events` is optional, and it gets
/// the shape check when it is there.
#[test]
fn calendar_events_refuses_an_option_shaped_calendar_id() {
    let call = call_from_tool(
        "google__calendar_events",
        &serde_json::json!({"calendar_id": "--client=other"}),
    )
    .unwrap();
    assert_eq!(
        GogCommand::for_call(&bound(), &call).unwrap_err(),
        AdapterError::InvalidArguments("calendar_id")
    );
}

#[test]
fn the_shared_mail_tools_map_onto_the_pinned_gog_calls() {
    let cases = [
        (
            MAIL_SEARCH,
            serde_json::json!({
                "mailbox": "work",
                "query": "from:alice@example.com since:2026-01-31 invoice",
                "max": 10
            }),
            GoogleCall::GmailSearch(GmailSearch {
                // `since:` is inclusive and Gmail's `after:` is not,
                // so the day before is what Gmail is asked for.
                query: "from:alice@example.com after:2026/01/30 invoice".into(),
                max: Some(10),
                page: None,
            }),
        ),
        (
            MAIL_SEARCH,
            serde_json::json!({"mailbox": "work", "query": "  "}),
            GoogleCall::GmailSearch(GmailSearch {
                query: BROWSE_QUERY.into(),
                max: None,
                page: None,
            }),
        ),
        (
            MAIL_GET_MESSAGE,
            serde_json::json!({"mailbox": "work", "message_id": "m1"}),
            GoogleCall::GmailGetMessage {
                message_id: "m1".into(),
            },
        ),
        (
            MAIL_GET_THREAD,
            serde_json::json!({"mailbox": "work", "thread_id": "t1"}),
            GoogleCall::GmailGetThread {
                thread_id: "t1".into(),
            },
        ),
        (
            MAIL_SEND,
            serde_json::json!({
                "mailbox": "work",
                "to": ["bob@example.com"],
                "subject": "Hello",
                "body": "Hi Bob",
                "in_reply_to": "<abc@example.com>"
            }),
            GoogleCall::GmailSend(EmailDraft {
                to: vec!["bob@example.com".into()],
                cc: vec![],
                bcc: vec![],
                subject: "Hello".into(),
                body: "Hi Bob".into(),
                in_reply_to: Some("<abc@example.com>".into()),
            }),
        ),
        (
            MAIL_MODIFY_MESSAGE,
            serde_json::json!({
                "mailbox": "work",
                "message_id": "m1",
                "mark_read": true,
                "archive": true,
                "flag": true
            }),
            GoogleCall::GmailModifyMessage {
                message_id: "m1".into(),
                add_labels: vec![STARRED_LABEL.into()],
                remove_labels: vec![UNREAD_LABEL.into(), INBOX_LABEL.into()],
            },
        ),
        (
            MAIL_MODIFY_MESSAGE,
            serde_json::json!({"mailbox": "work", "message_id": "m1", "mark_read": false, "flag": false}),
            GoogleCall::GmailModifyMessage {
                message_id: "m1".into(),
                add_labels: vec![UNREAD_LABEL.into()],
                remove_labels: vec![STARRED_LABEL.into()],
            },
        ),
    ];

    for (tool, arguments, expected) in cases {
        assert_eq!(
            call_from_tool(tool, &arguments).unwrap(),
            expected,
            "{tool}"
        );
    }

    // A call that asks for no change at all reaches no provider.
    assert!(
        call_from_tool(
            MAIL_MODIFY_MESSAGE,
            &serde_json::json!({"mailbox": "work", "message_id": "m1"})
        )
        .is_err()
    );
}

#[test]
fn is_write_holds_for_every_command_the_adapter_builds() {
    let reads = [
        MAIL_SEARCH,
        MAIL_GET_MESSAGE,
        MAIL_GET_THREAD,
        "google__calendar_events",
    ];
    let writes = [
        MAIL_SEND,
        MAIL_MODIFY_MESSAGE,
        "google__calendar_create_event",
        "google__calendar_update_event",
        "google__calendar_delete_event",
        "google__calendar_respond_event",
    ];
    let binding = ConnectionBinding::new(
        "alice@example.com",
        "pagis-google-01",
        "/tmp/pagis-test-gog-home",
    )
    .unwrap();
    let arguments = |tool: &str| match tool {
        MAIL_SEARCH => serde_json::json!({"mailbox": "work", "query": "invoice"}),
        MAIL_GET_MESSAGE | MAIL_MODIFY_MESSAGE => {
            serde_json::json!({"mailbox": "work", "message_id": "m1", "flag": true})
        }
        MAIL_GET_THREAD => serde_json::json!({"mailbox": "work", "thread_id": "t1"}),
        MAIL_SEND => serde_json::json!({
            "mailbox": "work", "to": ["bob@example.com"], "subject": "s", "body": "b"
        }),
        "google__calendar_events" => serde_json::json!({}),
        "google__calendar_create_event" => serde_json::json!({
            "calendar_id": "primary", "summary": "s",
            "from": "2026-09-01T09:00:00Z", "to": "2026-09-01T10:00:00Z"
        }),
        "google__calendar_respond_event" => serde_json::json!({
            "calendar_id": "primary", "event_id": "e1", "status": "accepted"
        }),
        _ => serde_json::json!({"calendar_id": "primary", "event_id": "e1"}),
    };

    for tool in reads {
        assert!(!is_write(tool), "{tool}");
        let call = call_from_tool(tool, &arguments(tool)).unwrap();
        assert!(
            !GogCommand::for_call(&binding, &call).unwrap().is_write(),
            "{tool}"
        );
    }
    for tool in writes {
        assert!(is_write(tool), "{tool}");
        let call = call_from_tool(tool, &arguments(tool)).unwrap();
        assert!(
            GogCommand::for_call(&binding, &call).unwrap().is_write(),
            "{tool}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn bounded_runner_stops_and_reaps_an_oversized_provider_process() {
    use pagis_google::{GogRunner, SystemGogRunner};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("provider");
    let pid = dir.path().join("pid");
    let finished = dir.path().join("finished");
    let script = format!(
        "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\ni=0\nwhile [ \"$i\" -lt 100000 ]; do\n  printf '%s' '{}'\n  i=$((i + 1))\ndone\nprintf done > '{}'\n",
        pid.display(),
        "x".repeat(1024),
        finished.display()
    );
    std::fs::write(&binary, script).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runner = std::sync::Arc::new(SystemGogRunner::new(&binary, test_keyring()));
    let command = GogCommand::for_call(
        &ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        &GoogleCall::GmailGetThread {
            thread_id: "thread".into(),
        },
    )
    .unwrap();
    // The deadline catches a runner that never stops the producer, not
    // a slow machine: the workspace suite runs many test binaries on
    // the same cores, and this one spawns a process that floods a pipe.
    // A cap-respecting runner returns in milliseconds.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        runner.run_bounded(&command, 2048),
    )
    .await
    .expect("oversized stdout must terminate, not run to the end");
    assert!(result.is_err());
    assert!(
        !finished.exists(),
        "the producer must be stopped when the transport cap is reached"
    );
    let pid = std::fs::read_to_string(pid).unwrap();
    let alive = std::process::Command::new("/bin/kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(
        !alive.success(),
        "the child must be killed and reaped before returning"
    );
    std::fs::write(&binary, "#!/bin/sh\nprintf small\n").unwrap();
    let output = runner.run_bounded(&command, 5).await.unwrap();
    assert_eq!(output.status, Some(0));
    assert_eq!(output.stdout, b"small");
}

#[cfg(unix)]
#[tokio::test]
async fn system_runner_classifies_an_expired_refresh_token_for_its_caller() {
    use pagis_google::{GogRunner, SystemGogRunner};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("provider");
    std::fs::write(
        &binary,
        "#!/bin/sh\nprintf '%s' 'oauth2: \"invalid_grant\" \"Token has been expired or revoked.\"' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runner = SystemGogRunner::new(&binary, test_keyring());
    let command = GogCommand::for_call(
        &ConnectionBinding::new("user@example.com", "main", "/tmp/pagis-test-gog-home").unwrap(),
        &GoogleCall::GmailGetThread {
            thread_id: "thread".into(),
        },
    )
    .unwrap();

    let failure = runner.run(&command).await.unwrap_err();

    assert_eq!(failure, ProcessFailure::ReauthRequired);
    assert_eq!(
        failure.into_provider_error(false).code,
        ProviderErrorCode::ReauthRequired
    );
}

/// Two people both call their Connection `google`. The `gog` client
/// name is the alias, so without a home of its own the second person's
/// credentials would land in the first person's store. The
/// runner puts every call under the Workspace's own `GOG_HOME`.
#[cfg(unix)]
#[tokio::test]
async fn two_workspaces_that_share_an_alias_run_under_two_gog_homes() {
    use pagis_core::WorkspaceId;
    use pagis_google::{GogRunner, SystemGogRunner, workspace_gog_home};
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("gog");
    std::fs::write(&binary, "#!/bin/sh\nprintf '%s' \"$GOG_HOME\"\n").unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runner = SystemGogRunner::new(&binary, test_keyring());
    let root = dir.path().join("workspaces");

    let mut seen = Vec::new();
    for workspace in ["ws-alice", "ws-bob"] {
        let workspace_id = WorkspaceId::from(workspace.to_string());
        let binding = ConnectionBinding::new(
            "person@example.com",
            "google",
            workspace_gog_home(&root, &workspace_id),
        )
        .unwrap();
        let command = GogCommand::for_call(
            &binding,
            &GoogleCall::GmailGetThread {
                thread_id: "thread".into(),
            },
        )
        .unwrap();
        let output = runner.run(&command).await.unwrap();
        seen.push(String::from_utf8(output.stdout).unwrap());
    }

    assert_ne!(seen[0], seen[1], "one alias shared one gog home");
    assert!(seen[0].ends_with("/ws-alice/gog"), "{}", seen[0]);
    assert!(seen[1].ends_with("/ws-bob/gog"), "{}", seen[1]);
    // The runner makes the directory, so the first call of a new
    // Workspace does not fail on a missing path.
    assert!(std::path::Path::new(&seen[0]).is_dir());
}

/// `gog` also reads its settings from `GOG_` variables, such as
/// `GOG_QUOTA_PROJECT`, and a new release can add one. The command decides
/// every setting of its call. So the child gets no `GOG_` variable of the
/// daemon, also one that no release has yet, and it gets the home, the
/// keyring and the token of the command. The other variables of the
/// daemon, such as `PATH`, stay.
#[test]
fn a_gog_child_gets_no_gog_variable_of_the_daemon() {
    use pagis_google::gog_environment;
    use std::ffi::OsString;

    let binding =
        ConnectionBinding::new("person@example.com", "google", "/tmp/pagis-test-gog-home").unwrap();
    let call = GoogleCall::GmailGetThread {
        thread_id: "thread".into(),
    };
    let daemon = || {
        [
            "PATH",
            "HOME",
            "GOG_QUOTA_PROJECT",
            "GOG_SOMETHING_NEW",
            "GOG_ACCOUNT",
            "GOG_HOME",
            "GOG_ACCESS_TOKEN",
            "GOGGLES",
        ]
        .map(OsString::from)
    };
    let set = |environment: &pagis_google::GogEnvironment| {
        environment
            .set
            .iter()
            .map(|(name, value)| (*name, value.to_str().unwrap().to_string()))
            .collect::<Vec<_>>()
    };

    let minted = GogCommand::for_call(&binding, &call)
        .unwrap()
        .with_access_token("ya29.minted-by-the-daemon");
    let environment = gog_environment(daemon(), &minted, "keyring-password").unwrap();
    assert_eq!(
        environment.removed,
        [
            "GOG_QUOTA_PROJECT",
            "GOG_SOMETHING_NEW",
            "GOG_ACCOUNT",
            "GOG_HOME",
            "GOG_ACCESS_TOKEN",
        ]
        .map(OsString::from)
    );
    assert_eq!(
        set(&environment),
        [
            ("GOG_HOME", "/tmp/pagis-test-gog-home".to_string()),
            ("GOG_KEYRING_BACKEND", "file".to_string()),
            ("GOG_KEYRING_PASSWORD", "keyring-password".to_string()),
            ("GOG_ACCESS_TOKEN", "ya29.minted-by-the-daemon".to_string()),
        ]
    );
    let rendered = format!("{environment:?}");
    assert!(!rendered.contains("keyring-password"), "{rendered}");
    assert!(!rendered.contains("ya29."), "{rendered}");

    // A `byo` call has no token of the daemon, so the child has none.
    let byo = GogCommand::for_call(&binding, &call).unwrap();
    let environment = gog_environment(daemon(), &byo, "keyring-password").unwrap();
    assert!(
        environment
            .removed
            .contains(&OsString::from("GOG_ACCESS_TOKEN"))
    );
    assert!(
        !environment
            .set
            .iter()
            .any(|(name, _)| *name == "GOG_ACCESS_TOKEN")
    );
}

/// A Google Connection's token is the daemon's. It reaches `gog`
/// through `GOG_ACCESS_TOKEN`, which bypasses `gog`'s own store, and
/// never through an argument, which would put it in `ps`.
#[cfg(unix)]
#[tokio::test]
async fn the_daemons_access_token_reaches_gog_in_the_environment_only() {
    use pagis_google::{GogRunner, SystemGogRunner};
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("gog");
    std::fs::write(
        &binary,
        "#!/bin/sh\nprintf '%s' \"${GOG_ACCESS_TOKEN:-none}\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let runner = SystemGogRunner::new(&binary, test_keyring());
    let binding =
        ConnectionBinding::new("person@example.com", "google", dir.path().join("home")).unwrap();
    let call = GoogleCall::GmailGetThread {
        thread_id: "thread".into(),
    };

    let byo = GogCommand::for_call(&binding, &call).unwrap();
    let minted = GogCommand::for_call(&binding, &call)
        .unwrap()
        .with_access_token("ya29.minted-by-the-daemon");

    assert!(!byo.has_access_token());
    assert_eq!(runner.run(&byo).await.unwrap().stdout, b"none");
    assert_eq!(
        runner.run(&minted).await.unwrap().stdout,
        b"ya29.minted-by-the-daemon"
    );
    assert!(
        !minted
            .args()
            .iter()
            .any(|arg| arg.contains("ya29.minted-by-the-daemon")),
        "the token must not reach the argument list"
    );
    // The `Debug` rendering says a token is held and never prints it.
    let rendered = format!("{minted:?}");
    assert!(rendered.contains("has_access_token: true"), "{rendered}");
    assert!(!rendered.contains("ya29."), "{rendered}");
}

/// `gog` keeps its own tokens on a file backend under the tenant's own
/// `GOG_HOME`, not in the platform keyring (ADR-0012).
///
/// On macOS the platform keyring is the one system keychain, which
/// `GOG_HOME` does not move: two people of one installation who both name
/// a Connection `google` would share a keychain item, so one person's
/// `byo` token would be the other person's too. The backend and a
/// per-Workspace password are what keep them apart.
#[cfg(unix)]
#[tokio::test]
async fn two_workspaces_keep_two_gog_keyrings_and_no_platform_keychain() {
    use pagis_core::WorkspaceId;
    use pagis_google::{GogRunner, SystemGogRunner, keyring_secret_name, workspace_gog_home};
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("gog");
    std::fs::write(
        &binary,
        "#!/bin/sh\nprintf '%s %s' \"${GOG_KEYRING_BACKEND:-none}\" \"${GOG_KEYRING_PASSWORD:-none}\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let secrets = std::sync::Arc::new(pagis_core::MemorySecretStore::default());
    let runner = SystemGogRunner::new(
        &binary,
        std::sync::Arc::new(pagis_google::SecretStoreKeyring::new(
            std::sync::Arc::clone(&secrets) as _,
        )),
    );
    let root = dir.path().join("workspaces");

    let mut seen = Vec::new();
    for workspace in ["ws-alice", "ws-bob", "ws-alice"] {
        let workspace_id = WorkspaceId::from(workspace.to_string());
        let binding = ConnectionBinding::new(
            "person@example.com",
            "google",
            workspace_gog_home(&root, &workspace_id),
        )
        .unwrap();
        let command = GogCommand::for_call(
            &binding,
            &GoogleCall::GmailGetThread {
                thread_id: "thread".into(),
            },
        )
        .unwrap();
        let output = runner.run(&command).await.unwrap();
        seen.push(String::from_utf8(output.stdout).unwrap());
    }

    // Every call runs on the file backend and never on the platform one.
    for answer in &seen {
        let (backend, password) = answer.split_once(' ').expect("both variables");
        assert_eq!(backend, "file", "{answer}");
        assert_eq!(password.len(), 64, "the password is 32 random bytes");
    }
    // Two people, two passwords. The same person, the same password: the
    // keyring a later call opens is the keyring the first call wrote.
    assert_ne!(seen[0], seen[1], "two people shared one keyring password");
    assert_eq!(seen[0], seen[2]);

    // And the daemon's own store is where the passwords live, one per
    // Workspace.
    for workspace in ["ws-alice", "ws-bob"] {
        let name = keyring_secret_name(&workspace_gog_home(
            &root,
            &WorkspaceId::from(workspace.to_string()),
        ));
        assert!(
            pagis_core::SecretStore::get(secrets.as_ref(), &name)
                .unwrap()
                .is_some(),
            "{name} is not in the secret store"
        );
    }
}
