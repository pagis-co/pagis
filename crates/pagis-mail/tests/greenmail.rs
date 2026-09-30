//! The transport against a real mail server:
//! GreenMail in Docker, one container per test. The tests are
//! `#[ignore]`-tagged the way `crates/pagis-computer/tests/docker_real.rs`
//! gates its containers, so `cargo test --workspace` stays hermetic and
//! the gate opts in with `-- --ignored`.
//!
//! The image is pinned. A fixture that follows `latest` drifts under
//! the tests without a commit.

use std::net::TcpStream;
use std::process::Command;
use std::time::{Duration, Instant};

use pagis_mail::{
    ARCHIVE, Cursor, Endpoint, FlagChange, INBOX, IdleOutcome, MailQuery, MailSession,
    MailTransport, MailboxCredential, OutgoingMessage, StandardTransport, TransportErrorCode,
};

/// The pinned test server. Never `latest`: a fixture that drifts turns
/// a green suite red without a commit.
const IMAGE: &str = "greenmail/standalone:2.1.13";
/// The IMAP and SMTP ports GreenMail listens on in the container. They
/// are the standard ports plus the documented 3000 offset.
const CONTAINER_IMAP: u16 = 3143;
const CONTAINER_SMTP: u16 = 3025;
/// The REST API the fixture makes the mailboxes through.
const CONTAINER_API: u16 = 8080;
const DOMAIN: &str = "pagis.test";
const PASSWORD: &str = "pw";
/// How long the server has to answer with its greeting.
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// One GreenMail container. It is removed when the fixture drops, so a
/// test that panics leaves no container behind.
struct GreenMail {
    name: String,
    imap_port: u16,
    smtp_port: u16,
    api_port: u16,
    /// The mailboxes of this container, by address.
    addresses: Vec<String>,
}

impl GreenMail {
    /// Start a container that holds these mailboxes. The mailboxes are
    /// made through the REST API and not through `greenmail.users`,
    /// because that option reads the login and the address apart and a
    /// real host logs a mailbox in under its whole address.
    async fn start(local_parts: &[&str]) -> Self {
        pull_once();
        let mut server = Self {
            name: format!("pagis-greenmail-{}", next_id()),
            imap_port: 0,
            smtp_port: 0,
            api_port: 0,
            addresses: local_parts
                .iter()
                .map(|local_part| format!("{local_part}@{DOMAIN}"))
                .collect(),
        };
        let started = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &server.name,
                "-p",
                &format!("127.0.0.1::{CONTAINER_IMAP}"),
                "-p",
                &format!("127.0.0.1::{CONTAINER_SMTP}"),
                "-p",
                &format!("127.0.0.1::{CONTAINER_API}"),
                "-e",
                "GREENMAIL_OPTS=-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0",
                IMAGE,
            ])
            .output()
            .expect("docker run starts");
        assert!(
            started.status.success(),
            "docker run failed: {}",
            String::from_utf8_lossy(&started.stderr)
        );
        // Docker reserves each port. Probing and releasing a host port
        // first races other tests and processes before container start.
        server.imap_port = server.published_port(CONTAINER_IMAP);
        server.smtp_port = server.published_port(CONTAINER_SMTP);
        server.api_port = server.published_port(CONTAINER_API);
        server.wait_ready().await;
        server
    }

    fn published_port(&self, container_port: u16) -> u16 {
        let mapping = Command::new("docker")
            .args(["port", &self.name, &format!("{container_port}/tcp")])
            .output()
            .expect("docker port starts");
        assert!(
            mapping.status.success(),
            "docker port failed: {}",
            String::from_utf8_lossy(&mapping.stderr)
        );
        String::from_utf8(mapping.stdout)
            .expect("port mapping is UTF-8")
            .trim()
            .strip_prefix("127.0.0.1:")
            .expect("port is bound only to loopback")
            .parse()
            .expect("published port is numeric")
    }

    /// Wait for the greeting, make the mailboxes, and prove one logs
    /// in. Docker's port proxy accepts the connection before the server
    /// listens, so a bare connect is no proof of readiness.
    async fn wait_ready(&self) {
        let deadline = Instant::now() + READY_TIMEOUT;
        let http = reqwest::Client::new();
        while !self.make_mailboxes(&http).await {
            assert!(
                Instant::now() < deadline,
                "{} never took its mailboxes",
                self.name
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let first = self.addresses.first().expect("one mailbox");
        loop {
            if login_probe(self.imap_port, first) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{} never held the mailbox {first}",
                self.name
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Make every mailbox through the REST API. The login is the whole
    /// address, as it is at a real host.
    async fn make_mailboxes(&self, http: &reqwest::Client) -> bool {
        for address in &self.addresses {
            let answered = http
                .post(format!("http://127.0.0.1:{}/api/user", self.api_port))
                .json(&serde_json::json!({
                    "email": address,
                    "login": address,
                    "password": PASSWORD,
                }))
                .send()
                .await;
            match answered {
                Ok(answer) if answer.status().is_success() => {}
                _ => return false,
            }
        }
        true
    }

    /// Restart the server. GreenMail keeps its mail in memory and
    /// numbers a fresh mailbox from the clock, so a restart is what a
    /// host does when it rebuilds a mailbox: every UID is stale.
    async fn restart(&mut self) {
        let restarted = Command::new("docker")
            .args(["restart", &self.name])
            .output()
            .expect("docker restart runs");
        assert!(
            restarted.status.success(),
            "docker restart failed: {}",
            String::from_utf8_lossy(&restarted.stderr)
        );
        self.imap_port = self.published_port(CONTAINER_IMAP);
        self.smtp_port = self.published_port(CONTAINER_SMTP);
        self.api_port = self.published_port(CONTAINER_API);
        self.wait_ready().await;
    }

    fn credential(&self, local_part: &str) -> MailboxCredential {
        MailboxCredential::new(
            format!("{local_part}@{DOMAIN}"),
            PASSWORD,
            Endpoint::new("127.0.0.1", self.imap_port),
            Endpoint::new("127.0.0.1", self.smtp_port),
        )
    }
}

impl Drop for GreenMail {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

/// Pull the pinned image once per test binary, so the readiness
/// deadline covers the server start and not the download. An image
/// that is in the local store already is never pulled again.
fn pull_once() {
    static PULLED: std::sync::Once = std::sync::Once::new();
    PULLED.call_once(|| {
        let present = Command::new("docker")
            .args(["image", "inspect", IMAGE])
            .output()
            .is_ok_and(|out| out.status.success());
        if present {
            return;
        }
        let pulled = Command::new("docker")
            .args(["pull", IMAGE])
            .output()
            .expect("docker pull runs");
        assert!(
            pulled.status.success(),
            "docker pull failed: {}",
            String::from_utf8_lossy(&pulled.stderr)
        );
    });
}

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let counter = NEXT.fetch_add(1, Ordering::Relaxed);
    u64::from(std::process::id()) * 1_000 + counter
}

/// An unused loopback port for the refused-connection test.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("a free port")
        .local_addr()
        .expect("the bound address")
        .port()
}

/// Whether the IMAP port answers with the `* OK` greeting and then
/// accepts this mailbox. The answer to a `LOGIN` is the only proof
/// that the server holds the users it was configured with: it answers
/// the greeting before it adds them.
fn login_probe(port: u16, address: &str) -> bool {
    use std::io::{BufRead, BufReader, Write};
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("a read timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("a second handle"));
    let mut greeting = String::new();
    if reader.read_line(&mut greeting).is_err() || !greeting.starts_with("* OK") {
        return false;
    }
    if write!(
        stream,
        "p1 LOGIN \"{address}\" \"{PASSWORD}\"\r\np2 LOGOUT\r\n"
    )
    .is_err()
    {
        return false;
    }
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }
        if let Some(answer) = line.strip_prefix("p1 ") {
            return answer.starts_with("OK");
        }
    }
}

/// Log in and open the inbox, which is what a mailbox does when it
/// becomes `active` (ADR-0019).
async fn open(server: &GreenMail, local_part: &str) -> (Box<dyn MailSession>, Cursor) {
    let mut session = StandardTransport::new()
        .login(&server.credential(local_part))
        .await
        .expect("the mailbox logs in");
    let state = session.select(INBOX).await.expect("the inbox opens");
    let cursor = state.cursor_here();
    (session, cursor)
}

fn note(to: &str, subject: &str, body: &str) -> OutgoingMessage {
    OutgoingMessage {
        to: vec![to.to_string()],
        subject: subject.to_string(),
        body: body.to_string(),
        ..OutgoingMessage::default()
    }
}

#[tokio::test]
#[ignore = "needs Docker; run via cargo test -p pagis-mail -- --ignored"]
async fn an_idle_turn_ends_at_its_deadline_and_a_send_wakes_the_next_one() {
    let server = GreenMail::start(&["ava", "ada"]).await;
    let (mut ava, cursor) = open(&server, "ava").await;
    let (mut ada, _) = open(&server, "ada").await;

    // Nothing arrives, so the turn ends at the deadline and the caller
    // issues IDLE again (RFC 2177).
    assert_eq!(
        ava.idle(Duration::from_millis(200))
            .await
            .expect("the turn ends"),
        IdleOutcome::Deadline
    );

    // The next turn waits while the other mailbox sends.
    let waiting = tokio::spawn(async move {
        let outcome = ava.idle(Duration::from_secs(60)).await;
        (outcome, ava)
    });
    // The session tells nothing when the server holds the IDLE, and a
    // message that arrives before it does not wake the turn. So the
    // send waits.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let sent = ada
        .send(&note(
            &format!("ava@{DOMAIN}"),
            "The roof",
            "The roof needs a look.",
        ))
        .await
        .expect("the host accepts the message");

    let (outcome, mut ava) = waiting.await.expect("the waiting task ends");
    assert_eq!(
        outcome.expect("the turn ends"),
        IdleOutcome::Changed,
        "the arriving message must wake the turn"
    );

    let (summaries, moved) = ava.fetch_since(&cursor).await.expect("the fetch runs");
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    let summary = &summaries[0];
    assert_eq!(summary.from, format!("ada@{DOMAIN}"));
    assert_eq!(summary.to, [format!("ava@{DOMAIN}")]);
    assert_eq!(summary.subject, "The roof");
    assert_eq!(summary.snippet, "The roof needs a look.");
    assert_eq!(
        summary.thread_id, sent.message_id,
        "the message threads on the id the send reported"
    );
    assert!(!summary.has_attachments);
    assert_eq!(moved.last_uid, summary.id.uid);
    assert_eq!(moved.uid_validity, cursor.uid_validity);

    let message = ava.fetch(&summary.id).await.expect("the body reads");
    assert_eq!(message.text.trim(), "The roof needs a look.");
    assert!(
        message
            .headers
            .iter()
            .any(|(name, value)| name == "Message-ID" && value == &sent.message_id),
        "{:?}",
        message.headers
    );

    // A reply carries the thread of the message it answers. A reserved
    // effect names the `Message-ID` its outcome is reconciled against,
    // and the host delivers the reply under that identity.
    let reserved = format!("<effect/iv-01j@{DOMAIN}>");
    let replied = ada
        .send(&OutgoingMessage {
            in_reply_to: Some(sent.message_id.clone()),
            message_id: Some(reserved.clone()),
            ..note(&format!("ava@{DOMAIN}"), "Re: The roof", "On Tuesday.")
        })
        .await
        .expect("the host accepts the reply");
    assert_eq!(
        replied.message_id, reserved,
        "the send reports the reserved identity"
    );
    let (replies, _) = ava.fetch_since(&moved).await.expect("the fetch runs");
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(replies[0].thread_id, sent.message_id);
    let reply = ava.fetch(&replies[0].id).await.expect("the reply reads");
    assert!(
        reply
            .headers
            .iter()
            .any(|(name, value)| name == "Message-ID" && value == &reserved),
        "the delivered reply carries the reserved identity: {:?}",
        reply.headers
    );

    // The search reads the same folder.
    let found = ava
        .search(&MailQuery {
            subject: Some("roof".into()),
            limit: 25,
            ..MailQuery::default()
        })
        .await
        .expect("the search runs");
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(
        found[0].subject, "Re: The roof",
        "the newest message answers first"
    );
}

#[tokio::test]
#[ignore = "needs Docker; run via cargo test -p pagis-mail -- --ignored"]
async fn a_session_that_reconnects_resyncs_by_state_and_not_by_push() {
    let server = GreenMail::start(&["ava", "ada"]).await;
    let (ava, cursor) = open(&server, "ava").await;
    // The session is gone, so no push can reach it.
    drop(ava);

    let (mut ada, _) = open(&server, "ada").await;
    ada.send(&note(&format!("ava@{DOMAIN}"), "While away", "Read this."))
        .await
        .expect("the host accepts the message");

    let (mut ava, _) = open(&server, "ava").await;
    let (summaries, moved) = ava.fetch_since(&cursor).await.expect("the fetch runs");
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    assert_eq!(summaries[0].subject, "While away");
    assert!(moved.last_uid > cursor.last_uid);

    // A second pass from the moved cursor finds nothing new.
    let (again, still) = ava.fetch_since(&moved).await.expect("the fetch runs");
    assert!(again.is_empty(), "{again:?}");
    assert_eq!(still, moved);
}

#[tokio::test]
#[ignore = "needs Docker; run via cargo test -p pagis-mail -- --ignored"]
async fn flags_and_the_archive_move_change_what_the_folder_holds() {
    let server = GreenMail::start(&["ava", "ada"]).await;
    let (mut ava, cursor) = open(&server, "ava").await;
    let (mut ada, _) = open(&server, "ada").await;
    ada.send(&note(&format!("ava@{DOMAIN}"), "The roof", "A look."))
        .await
        .expect("the host accepts the message");

    let (summaries, _) = ava.fetch_since(&cursor).await.expect("the fetch runs");
    let id = summaries.first().expect("one message").id.clone();

    ava.set_flags(
        &id,
        FlagChange {
            seen: Some(true),
            flagged: Some(true),
        },
    )
    .await
    .expect("the flags are set");

    ava.move_to_archive(&id).await.expect("the message moves");
    let inbox = ava.select(INBOX).await.expect("the inbox opens");
    assert_eq!(inbox.exists, 0, "the message left the inbox");
    let archive = ava.select(ARCHIVE).await.expect("the archive opens");
    assert_eq!(archive.exists, 1, "the message is in the archive");

    let archived = ava
        .search(&MailQuery {
            limit: 25,
            ..MailQuery::default()
        })
        .await
        .expect("the search runs");
    assert_eq!(archived.len(), 1, "{archived:?}");
    assert_eq!(archived[0].subject, "The roof");
    assert_eq!(archived[0].id.folder, ARCHIVE);
}

#[tokio::test]
#[ignore = "needs Docker; run via cargo test -p pagis-mail -- --ignored"]
async fn a_renumbered_folder_answers_stale_id() {
    let mut server = GreenMail::start(&["ava"]).await;
    let (ava, cursor) = open(&server, "ava").await;
    drop(ava);

    server.restart().await;

    let (mut ava, fresh) = open(&server, "ava").await;
    assert_ne!(
        fresh.uid_validity, cursor.uid_validity,
        "the restart must renumber the folder"
    );
    let error = ava
        .fetch_since(&cursor)
        .await
        .expect_err("the old cursor is stale");
    assert_eq!(error.0, TransportErrorCode::StaleId);
}

#[tokio::test]
#[ignore = "needs Docker; run via cargo test -p pagis-mail -- --ignored"]
async fn a_refused_password_and_a_closed_port_are_different_failures() {
    let server = GreenMail::start(&["ava"]).await;
    let transport = StandardTransport::new();

    let wrong = MailboxCredential::new(
        format!("ava@{DOMAIN}"),
        "not-the-password",
        Endpoint::new("127.0.0.1", server.imap_port),
        Endpoint::new("127.0.0.1", server.smtp_port),
    );
    let refused = transport
        .login(&wrong)
        .await
        .err()
        .expect("the host refuses the password");
    assert_eq!(refused.0, TransportErrorCode::Unauthorized);

    let closed = MailboxCredential::new(
        format!("ava@{DOMAIN}"),
        PASSWORD,
        Endpoint::new("127.0.0.1", free_port()),
        Endpoint::new("127.0.0.1", server.smtp_port),
    );
    let unreachable = transport
        .login(&closed)
        .await
        .err()
        .expect("nothing listens on the port");
    assert_eq!(unreachable.0, TransportErrorCode::Unreachable);
}
