//! The contract of the Google adapter with the pinned `gog`.
//!
//! The contract tests check the argv that the adapter builds. The tests
//! here give that argv to the `gog` release of `GOG_VERSION`, and show
//! how that `gog` reads it: each value from the Agent stays a value, and
//! each call keeps the account and the client of its Connection.
//!
//! They need no Google account and send no request to Google. The `gog`
//! home holds a Desktop client and no token, so `gog` stops each call
//! where it looks for the token of the account. It then exits with code
//! 4 and names that account. Each HTTP request goes to a proxy address
//! that does not answer.
//!
//! The `gog-contract` step of the gate downloads the pinned `gog` of its
//! host, checks the archive against its SHA-256, and runs these tests
//! with `PAGIS_GOG` set to that binary.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use pagis_google::{
    AdapterError, BROWSE_QUERY, CalendarEvent, CalendarEventPatch, ConnectionBinding, EmailDraft,
    GOG_VERSION, GmailSearch, GogCommand, GoogleCall,
};

const ACCOUNT: &str = "alice@example.com";

/// The exit code of `gog` for a call that has no token for its account.
const NO_TOKEN: i32 = 4;

/// Values that `gog` reads as an option when one argv item holds only
/// the value.
const OPTION_SHAPED: [&str; 4] = [
    "--client=other",
    "--account=x@example.com",
    "--readonly=false",
    "-h",
];

/// The pinned `gog` that the gate supplies.
fn pinned_gog() -> Option<PathBuf> {
    std::env::var_os("PAGIS_GOG").map(PathBuf::from)
}

/// What one run of `gog` gave: the exit code, and stdout and stderr.
struct Ran {
    code: Option<i32>,
    output: String,
}

/// Run `args` with `gog` under `home`, with the file keyring that the
/// daemon uses, and with no variable of the environment of this test.
fn run(gog: &Path, home: &Path, args: &[String], stdin: Option<&[u8]>) -> Ran {
    let mut child = Command::new(gog)
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("GOG_HOME", home)
        .env("GOG_KEYRING_BACKEND", "file")
        .env("GOG_KEYRING_PASSWORD", "contract-check")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("start {}: {error}", gog.display()));
    if let Some(input) = stdin {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    let finished = child.wait_with_output().unwrap();
    Ran {
        code: finished.status.code(),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&finished.stdout),
            String::from_utf8_lossy(&finished.stderr)
        ),
    }
}

/// Each call of the adapter, with `text` in each slot that `gog` reads as
/// free text, and the `gog` service of the call. The other slots hold
/// values that `gog` accepts.
fn calls(text: &str) -> Vec<(&'static str, GoogleCall)> {
    let text = text.to_string();
    vec![
        (
            "gmail",
            GoogleCall::GmailSearch(GmailSearch {
                query: text.clone(),
                max: Some(5),
                page: Some(text.clone()),
            }),
        ),
        (
            "gmail",
            GoogleCall::GmailSearch(GmailSearch {
                query: BROWSE_QUERY.into(),
                max: None,
                page: None,
            }),
        ),
        (
            "gmail",
            GoogleCall::GmailGetMessage {
                message_id: "18c2f0a1b2c3d4e5".into(),
            },
        ),
        (
            "gmail",
            GoogleCall::GmailGetThread {
                thread_id: "18c2f0a1b2c3d4e5".into(),
            },
        ),
        (
            "calendar",
            GoogleCall::CalendarEvents {
                calendar_id: Some("primary".into()),
                from: Some(text.clone()),
                to: Some(text.clone()),
                query: Some(text.clone()),
                max: Some(5),
                page: Some(text.clone()),
            },
        ),
        (
            "calendar",
            GoogleCall::CalendarEvents {
                calendar_id: None,
                from: None,
                to: None,
                query: None,
                max: None,
                page: None,
            },
        ),
        (
            "gmail",
            GoogleCall::GmailSend(EmailDraft {
                to: vec!["bob@example.com".into()],
                cc: vec!["carol@example.com".into()],
                bcc: vec!["dan@example.com".into()],
                subject: text.clone(),
                body: text.clone(),
                in_reply_to: Some(text.clone()),
            }),
        ),
        (
            "gmail",
            GoogleCall::GmailModifyMessage {
                message_id: "18c2f0a1b2c3d4e5".into(),
                add_labels: vec![text.clone()],
                remove_labels: vec!["INBOX".into()],
            },
        ),
        (
            "calendar",
            GoogleCall::CalendarCreateEvent(CalendarEvent {
                calendar_id: "primary".into(),
                summary: text.clone(),
                from: "2026-09-01T09:00:00Z".into(),
                to: "2026-09-01T10:00:00Z".into(),
                description: Some(text.clone()),
                location: Some(text.clone()),
                attendees: vec!["bob@example.com".into()],
                timezone: Some("America/Los_Angeles".into()),
                send_updates: Some("none".into()),
                with_meet: true,
            }),
        ),
        (
            "calendar",
            GoogleCall::CalendarUpdateEvent(CalendarEventPatch {
                calendar_id: "primary".into(),
                event_id: "e1".into(),
                summary: Some(text.clone()),
                from: Some("2026-09-01T09:00:00Z".into()),
                to: Some("2026-09-01T10:00:00Z".into()),
                description: Some(text.clone()),
                location: Some(text.clone()),
                attendees: Some(vec!["bob@example.com".into()]),
                timezone: Some("America/Los_Angeles".into()),
                send_updates: Some("none".into()),
            }),
        ),
        (
            "calendar",
            GoogleCall::CalendarDeleteEvent {
                calendar_id: "primary".into(),
                event_id: "e1".into(),
                send_updates: Some("none".into()),
            },
        ),
        (
            "calendar",
            GoogleCall::CalendarRespondEvent {
                calendar_id: "primary".into(),
                event_id: "e1".into(),
                status: "accepted".into(),
                comment: Some(text),
            },
        ),
    ]
}

/// A `gog` home with a client under the name of the bound Connection and
/// no token, so each call stops on the missing token and not on the
/// missing client. The daemon stores no client: it gives `gog` an access
/// token for each call.
fn home_with_a_client(gog: &Path) -> (tempfile::TempDir, ConnectionBinding) {
    let home = tempfile::tempdir().unwrap();
    let binding = ConnectionBinding::new(ACCOUNT, "pagis-google-01", home.path()).unwrap();
    let client = serde_json::json!({
        "installed": {
            "client_id": "id.apps.googleusercontent.com",
            "client_secret": "not-a-secret",
            "auth_uri": "https://accounts.google.com/o/oauth2/auth",
            "token_uri": "https://oauth2.googleapis.com/token",
        }
    })
    .to_string();
    let args = [
        "--client",
        "pagis-google-01",
        "--no-input",
        "--json",
        "auth",
        "credentials",
        "set",
        "-",
    ]
    .map(String::from);
    let installed = run(gog, home.path(), &args, Some(client.as_bytes()));
    assert_eq!(installed.code, Some(0), "{}", installed.output);
    (home, binding)
}

/// The pinned `gog` reads each argv that the adapter builds, and reads
/// each option-shaped value from the Agent as a value. Each call stops on
/// the missing token of the bound account: `gog` did not change the
/// client (that stops on a missing Desktop client with another code), did
/// not change the account, and did not print its help.
#[test]
#[ignore = "needs the pinned gog: the gog-contract step of the gate runs it with PAGIS_GOG"]
fn the_pinned_gog_reads_each_agent_value_as_a_value() {
    let Some(gog) = pinned_gog() else {
        eprintln!("PAGIS_GOG is not set; the pinned gog test did nothing");
        return;
    };
    let (home, binding) = home_with_a_client(&gog);
    let version = run(&gog, home.path(), &["--version".to_string()], None);
    assert!(
        version
            .output
            .split_whitespace()
            .any(|word| word == format!("v{GOG_VERSION}")),
        "{} is not gog {GOG_VERSION}: {}",
        gog.display(),
        version.output
    );

    for value in OPTION_SHAPED.into_iter().chain(["planning"]) {
        for (service, call) in calls(value) {
            let command = GogCommand::for_call(&binding, &call)
                .unwrap_or_else(|error| panic!("{call:?}: {error}"));
            let ran = run(&gog, home.path(), command.args(), None);
            assert_eq!(
                ran.code,
                Some(NO_TOKEN),
                "{:?}\n{}",
                command.args(),
                ran.output
            );
            assert!(
                ran.output.contains(&format!("{service} {ACCOUNT}")),
                "{:?}\n{}",
                command.args(),
                ran.output
            );
            assert!(!ran.output.contains("x@example.com"), "{}", ran.output);
        }
    }
}

/// The pinned `gog` takes the time zone of an end of an event only with the
/// new time of that end: it refuses `--start-timezone` without `--from`
/// with its usage exit, before it looks for a token. So the adapter gives
/// the time zone of a patch only to the ends that the patch moves, and it
/// refuses a patch that holds a time zone and no new time.
#[test]
#[ignore = "needs the pinned gog: the gog-contract step of the gate runs it with PAGIS_GOG"]
fn the_pinned_gog_takes_a_time_zone_only_with_the_time_of_its_end() {
    let Some(gog) = pinned_gog() else {
        eprintln!("PAGIS_GOG is not set; the pinned gog test did nothing");
        return;
    };
    let (home, binding) = home_with_a_client(&gog);
    let patch = CalendarEventPatch {
        calendar_id: "primary".into(),
        event_id: "e1".into(),
        summary: None,
        from: Some("2026-09-01T09:00:00".into()),
        to: None,
        description: None,
        location: None,
        attendees: None,
        timezone: Some("Europe/Rome".into()),
        send_updates: None,
    };
    let command =
        GogCommand::for_call(&binding, &GoogleCall::CalendarUpdateEvent(patch.clone())).unwrap();
    let ran = run(&gog, home.path(), command.args(), None);
    assert_eq!(
        ran.code,
        Some(NO_TOKEN),
        "{:?}\n{}",
        command.args(),
        ran.output
    );

    let without_from: Vec<String> = command
        .args()
        .iter()
        .filter(|arg| !arg.starts_with("--from="))
        .cloned()
        .collect();
    let ran = run(&gog, home.path(), &without_from, None);
    assert_eq!(ran.code, Some(2), "{without_from:?}\n{}", ran.output);
    assert!(
        ran.output.contains("--start-timezone requires --from"),
        "{without_from:?}\n{}",
        ran.output
    );

    let no_time = CalendarEventPatch {
        from: None,
        ..patch
    };
    assert_eq!(
        GogCommand::for_call(&binding, &GoogleCall::CalendarUpdateEvent(no_time)).unwrap_err(),
        AdapterError::InvalidArguments("timezone")
    );
}

/// The pinned `gog` reads an item after `--` as a positional value. The
/// same item with no `--` in front of it is an option, also after a
/// positional value. This is why the adapter ends the options with `--`,
/// and it shows that the test above sees a changed account.
#[test]
#[ignore = "needs the pinned gog: the gog-contract step of the gate runs it with PAGIS_GOG"]
fn the_pinned_gog_reads_an_item_after_the_terminator_as_a_positional_value() {
    let Some(gog) = pinned_gog() else {
        eprintln!("PAGIS_GOG is not set; the pinned gog test did nothing");
        return;
    };
    let (home, binding) = home_with_a_client(&gog);
    let command = GogCommand::for_call(
        &binding,
        &GoogleCall::GmailSearch(GmailSearch {
            query: "invoice".into(),
            max: None,
            page: None,
        }),
    )
    .unwrap();

    let mut after_terminator = command.args().to_vec();
    after_terminator.push("--account=x@example.com".into());
    let ran = run(&gog, home.path(), &after_terminator, None);
    assert_eq!(
        ran.code,
        Some(NO_TOKEN),
        "{after_terminator:?}\n{}",
        ran.output
    );
    assert!(
        ran.output.contains(&format!("gmail {ACCOUNT}")),
        "{after_terminator:?}\n{}",
        ran.output
    );

    let without_terminator: Vec<String> = after_terminator
        .into_iter()
        .filter(|arg| arg != "--")
        .collect();
    let ran = run(&gog, home.path(), &without_terminator, None);
    assert!(
        ran.output.contains("gmail x@example.com"),
        "{without_terminator:?}\n{}",
        ran.output
    );
}
