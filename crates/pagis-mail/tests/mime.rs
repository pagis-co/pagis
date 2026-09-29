//! The MIME half of the transport (ADR-0019): the bytes a host
//! gives back become one [`Message`], and one [`OutgoingMessage`]
//! becomes the bytes a host accepts. No socket opens.

use pagis_mail::{Message, MessageId, OutgoingMessage, TransportErrorCode, build_mime, read_mime};

fn read(raw: &str) -> Message {
    read_mime(raw.as_bytes(), MessageId::new("INBOX", 7)).expect("the message reads")
}

const PLAIN: &str = concat!(
    "Message-ID: <a1@example.test>\r\n",
    "From: Ada <ada@example.test>\r\n",
    "To: agent@pagis.test, second@pagis.test\r\n",
    "Subject: The roof\r\n",
    "Date: Tue, 2 Sep 2026 10:00:00 +0000\r\n",
    "\r\n",
    "The roof needs a look.\r\n",
);

#[test]
fn a_plain_message_reads_as_its_summary_and_its_text() {
    let message = read(PLAIN);
    assert_eq!(message.summary.id, MessageId::new("INBOX", 7));
    assert_eq!(message.summary.from, "ada@example.test");
    assert_eq!(
        message.summary.to,
        ["agent@pagis.test", "second@pagis.test"]
    );
    assert_eq!(message.summary.subject, "The roof");
    assert_eq!(message.summary.date, 1_788_343_200_000);
    assert_eq!(message.summary.snippet, "The roof needs a look.");
    assert!(!message.summary.has_attachments);
    assert_eq!(message.text.trim(), "The roof needs a look.");
    assert!(message.attachments.is_empty());
}

#[test]
fn a_message_without_references_threads_on_its_own_message_id() {
    assert_eq!(read(PLAIN).summary.thread_id, "<a1@example.test>");
}

#[test]
fn a_reply_threads_on_the_root_of_its_references() {
    let raw = concat!(
        "Message-ID: <c3@example.test>\r\n",
        "References: <a1@example.test> <b2@example.test>\r\n",
        "In-Reply-To: <b2@example.test>\r\n",
        "From: ada@example.test\r\n",
        "Subject: Re: The roof\r\n",
        "\r\n",
        "Yes.\r\n",
    );
    assert_eq!(read(raw).summary.thread_id, "<a1@example.test>");
}

#[test]
fn the_headers_read_in_the_order_the_message_carries_them() {
    let message = read(PLAIN);
    let names: Vec<&str> = message
        .headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["Message-ID", "From", "To", "Subject", "Date"]);
    let subject = message
        .headers
        .iter()
        .find(|(name, _)| name == "Subject")
        .expect("the Subject header");
    assert_eq!(subject.1, "The roof");
}

/// The collector verifies the sender from these values, so they keep
/// the order the message carries them in, topmost first, and every
/// other header stays out.
#[test]
fn the_authentication_results_read_topmost_first() {
    let raw = concat!(
        "Authentication-Results: aspmx1.migadu.com;\r\n",
        "\tdmarc=pass (policy=none) header.from=example.test\r\n",
        "ARC-Authentication-Results: i=1; aspmx1.migadu.com; dmarc=pass\r\n",
        "Message-ID: <a1@example.test>\r\n",
        "From: Ada <ada@example.test>\r\n",
        "authentication-results: mx.example.test; dmarc=fail header.from=example.test\r\n",
        "Subject: The roof\r\n",
        "\r\n",
        "The roof needs a look.\r\n",
    );
    let results = read(raw).summary.authentication_results;
    assert_eq!(results.len(), 2, "{results:?}");
    assert!(results[0].starts_with("aspmx1.migadu.com;"));
    assert!(results[0].contains("dmarc=pass (policy=none) header.from=example.test"));
    assert!(results[1].starts_with("mx.example.test;"));
    assert!(read(PLAIN).summary.authentication_results.is_empty());
}

/// The sender check needs one author, so the summary counts every
/// mailbox of every `From` header, and one at least for each header.
#[test]
fn the_from_headers_count_their_mailboxes() {
    let mailboxes = |from: &str| {
        read(&format!("{from}Subject: Hi\r\n\r\nHello.\r\n"))
            .summary
            .from_mailboxes
    };
    assert_eq!(mailboxes("From: Ada <ada@example.test>\r\n"), 1);
    assert_eq!(mailboxes("From: ada@example.test, bob@example.test\r\n"), 2);
    assert_eq!(
        mailboxes("From: Team: ada@example.test, bob@example.test;\r\n"),
        2
    );
    assert_eq!(
        mailboxes("From: ada@example.test\r\nFrom: bob@example.test\r\n"),
        2
    );
    // A second `From` header counts even when it names no address.
    assert_eq!(mailboxes("From: ada@example.test\r\nFrom:\r\n"), 2);
    assert_eq!(mailboxes(""), 0);
}

#[test]
fn an_html_only_body_reads_as_text() {
    let raw = concat!(
        "Message-ID: <h1@example.test>\r\n",
        "From: ada@example.test\r\n",
        "Content-Type: text/html; charset=utf-8\r\n",
        "\r\n",
        "<html><body><p>Hello <b>there</b></p></body></html>\r\n",
    );
    let message = read(raw);
    assert!(message.text.contains("Hello"), "{}", message.text);
    assert!(message.text.contains("there"), "{}", message.text);
    assert!(!message.text.contains("<b>"), "{}", message.text);
}

#[test]
fn an_attachment_reads_as_a_name_and_a_size_and_never_as_bytes() {
    let raw = concat!(
        "Message-ID: <m1@example.test>\r\n",
        "From: ada@example.test\r\n",
        "Content-Type: multipart/mixed; boundary=\"sep\"\r\n",
        "\r\n",
        "--sep\r\n",
        "Content-Type: text/plain\r\n",
        "\r\n",
        "See the roof plan.\r\n",
        "--sep\r\n",
        "Content-Type: text/plain; name=\"plan.txt\"\r\n",
        "Content-Disposition: attachment; filename=\"plan.txt\"\r\n",
        "\r\n",
        "0123456789\r\n",
        "--sep--\r\n",
    );
    let message = read(raw);
    assert!(message.summary.has_attachments);
    assert_eq!(message.attachments.len(), 1);
    assert_eq!(message.attachments[0].name, "plan.txt");
    assert_eq!(message.attachments[0].bytes, 10);
    assert!(message.text.contains("roof plan"), "{}", message.text);
}

#[test]
fn a_long_body_gives_a_snippet_of_two_hundred_characters() {
    let body = "z".repeat(500);
    let raw = format!("From: ada@example.test\r\nSubject: Long\r\n\r\n{body}\r\n");
    let message = read(&raw);
    assert_eq!(message.summary.snippet.chars().count(), 200);
    assert!(message.text.len() >= 500);
}

#[test]
fn an_answer_with_no_message_in_it_is_unreadable() {
    let error = read_mime(&[], MessageId::new("INBOX", 1))
        .err()
        .map(|error| error.0);
    assert_eq!(error, Some(TransportErrorCode::Unreadable));
}

// --- the send half ---

#[test]
fn a_send_carries_the_message_id_it_reports() {
    let built = build_mime(
        "agent@pagis.test",
        &OutgoingMessage {
            to: vec!["ada@example.test".into()],
            subject: "The roof".into(),
            body: "On Tuesday.".into(),
            ..OutgoingMessage::default()
        },
    )
    .expect("the message builds");
    let raw = String::from_utf8(built.raw).expect("the message is utf-8");
    assert!(
        raw.contains(&format!("Message-ID: {}", built.message_id)),
        "{raw}"
    );
    assert!(built.message_id.starts_with('<') && built.message_id.ends_with('>'));
    assert!(raw.contains("From: <agent@pagis.test>"), "{raw}");
    assert!(raw.contains("To: <ada@example.test>"), "{raw}");
    assert!(raw.contains("On Tuesday."), "{raw}");
}

#[test]
fn a_reply_carries_in_reply_to_and_references() {
    let built = build_mime(
        "agent@pagis.test",
        &OutgoingMessage {
            to: vec!["ada@example.test".into()],
            subject: "Re: The roof".into(),
            body: "Yes.".into(),
            in_reply_to: Some("<a1@example.test>".into()),
            ..OutgoingMessage::default()
        },
    )
    .expect("the message builds");
    let raw = String::from_utf8(built.raw).expect("the message is utf-8");
    assert!(raw.contains("In-Reply-To: <a1@example.test>"), "{raw}");
    assert!(raw.contains("References: <a1@example.test>"), "{raw}");
}

#[test]
fn a_send_without_a_recipient_is_rejected_before_the_socket() {
    let error = build_mime("agent@pagis.test", &OutgoingMessage::default())
        .err()
        .map(|error| error.0);
    assert_eq!(error, Some(TransportErrorCode::Rejected));
}

#[test]
fn a_bcc_recipient_reaches_the_envelope_and_never_the_headers() {
    let built = build_mime(
        "agent@pagis.test",
        &OutgoingMessage {
            to: vec!["ada@example.test".into()],
            cc: vec!["cc@example.test".into()],
            bcc: vec!["hidden@example.test".into()],
            subject: "The roof".into(),
            body: "On Tuesday.".into(),
            ..OutgoingMessage::default()
        },
    )
    .expect("the message builds");
    assert_eq!(
        built.recipients,
        ["ada@example.test", "cc@example.test", "hidden@example.test"]
    );
    let raw = String::from_utf8(built.raw).expect("the message is utf-8");
    assert!(raw.contains("Cc: <cc@example.test>"), "{raw}");
    assert!(!raw.contains("hidden@example.test"), "{raw}");
}

/// The collector deduplicates on the `Message-ID` and lands a reply by
/// its `In-Reply-To`, so both reach the summary (ADR-0019).
#[test]
fn a_summary_carries_the_message_id_and_the_message_it_answers() {
    let plain = read(PLAIN);
    assert_eq!(plain.summary.message_id, "<a1@example.test>");
    assert_eq!(plain.summary.in_reply_to, None);

    let raw = concat!(
        "Message-ID: <c3@example.test>\r\n",
        "References: <a1@example.test> <b2@example.test>\r\n",
        "In-Reply-To: <b2@example.test>\r\n",
        "From: ada@example.test\r\n",
        "Subject: Re: The roof\r\n",
        "\r\n",
        "Yes.\r\n",
    );
    let reply = read(raw);
    assert_eq!(reply.summary.message_id, "<c3@example.test>");
    assert_eq!(
        reply.summary.in_reply_to.as_deref(),
        Some("<b2@example.test>")
    );
}

/// A message with no `Message-ID` falls back to the folder and the UID,
/// so it still has one identity (ADR-0019).
#[test]
fn a_message_without_a_message_id_carries_none() {
    let raw = concat!(
        "From: ada@example.test\r\n",
        "Subject: No id\r\n",
        "\r\n",
        "Anonymous.\r\n",
    );
    let message = read(raw);
    assert_eq!(message.summary.message_id, "");
    assert_eq!(message.summary.thread_id, "INBOX:7");
}
