//! Host messages are bounded text for the Person, separate from log output.
use pagis_mail::{HostError, HostErrorCode, MailboxError, host_message};

#[test]
fn host_messages_are_bounded_at_a_utf8_boundary() {
    let message = format!("{}éextra", "a".repeat(511));
    let error = HostError::with_message(HostErrorCode::Refused, &message);
    assert_eq!(host_message(&error), "a".repeat(511));
    let error = HostError::with_message(HostErrorCode::Refused, "é".repeat(300));
    assert_eq!(host_message(&error), "é".repeat(256));
    let error = HostError::with_message(HostErrorCode::Refused, "a".repeat(512));
    assert_eq!(host_message(&error).len(), 512);
}

#[test]
fn logs_keep_only_the_code_even_through_the_desk_error() {
    let error = HostError::with_message(HostErrorCode::Refused, "private host account condition");
    assert_eq!(error.code(), HostErrorCode::Refused);
    assert_eq!(host_message(&error), "private host account condition");
    assert_eq!(
        error.to_string(),
        "the mail host did not complete this: refused"
    );
    assert_eq!(format!("{error:?}"), "HostError { code: Refused }");
    let error = MailboxError::Host(error);
    assert!(!format!("{error:?}").contains("private"));
    assert!(!error.to_string().contains("private"));
}

#[test]
fn an_absent_or_blank_message_keeps_the_fixed_code_text() {
    for error in [
        HostError::new(HostErrorCode::Unauthorized),
        HostError::with_message(HostErrorCode::Unauthorized, " \n\t "),
    ] {
        assert_eq!(
            host_message(&error),
            "the mail host refused its API key. Reconnect the mailbox provider."
        );
    }
}
