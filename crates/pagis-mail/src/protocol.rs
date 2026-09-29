//! The standard-protocol transport (ADR-0019): IMAP to read and to
//! wait, SMTP submission to send, with the mailbox's own password.
//!
//! One [`StandardSession`] holds one IMAP connection and one SMTP
//! transport. The IMAP half is stateful, so one task owns the session
//! and the seam takes `&mut self`.
//!
//! The daemon, not the crate, owns the IDLE turn: `async-imap` resets
//! its own timeout on every server keepalive, so a turn that waits on
//! that timeout can last hours. [`StandardSession`]
//! therefore holds a wall clock over the turn and answers
//! [`IdleOutcome::Deadline`] when it passes, and the caller re-issues
//! IDLE. That is what RFC 2177 asks of a client.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_imap::Client;
use async_imap::extensions::idle::IdleResponse;
use async_imap::types::Fetch;
use async_trait::async_trait;
use futures::TryStreamExt;
use lettre::address::Envelope;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::response::Severity;
use lettre::{
    Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
    transport::smtp::Error as SmtpError,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::mime::{build_mime, read_mime};
use crate::transport::{
    ARCHIVE, Cursor, Endpoint, FlagChange, FolderState, IdleOutcome, MailQuery, MailSession,
    MailTransport, MailboxCredential, Message, MessageId, MessageSummary, OutgoingMessage,
    SentMessage, TransportCapabilities, TransportError, TransportErrorCode,
};

/// IMAP with implicit TLS (RFC 8314).
pub const IMAPS_PORT: u16 = 993;
/// IMAP in the clear, which a host upgrades with STARTTLS.
pub const IMAP_PORT: u16 = 143;
/// SMTP submission with implicit TLS (RFC 8314).
pub const SMTPS_PORT: u16 = 465;
/// SMTP submission, which a host upgrades with STARTTLS.
pub const SUBMISSION_PORT: u16 = 587;

/// The longest one IDLE turn lasts. RFC 2177 asks a client to end IDLE
/// and to issue it again at least every 29 minutes, because a server
/// may log an idle client off after its own timeout.
pub const MAX_IDLE_TURN: Duration = Duration::from_secs(29 * 60);

/// How long one command may take before the host counts as unreachable.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// The most messages one `fetch_since` reads in one pass. The caller
/// keeps the cursor it gets back and asks again, so a folder with a
/// year of mail in it cannot fill the daemon's memory in one call.
const FETCH_BATCH: usize = 50;

/// The bytes of one message, without touching the `\Seen` flag: a read
/// by the collector must not mark mail as read behind the user.
const FETCH_QUERY: &str = "(UID BODY.PEEK[])";

/// Read a mailbox over IMAP and send through the host's submission
/// port, with the mailbox's own password (ADR-0019).
#[derive(Debug, Default)]
pub struct StandardTransport;

impl StandardTransport {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MailTransport for StandardTransport {
    fn capabilities(&self) -> TransportCapabilities {
        TransportCapabilities { idle: true }
    }

    async fn login(
        &self,
        credential: &MailboxCredential,
    ) -> Result<Box<dyn MailSession>, TransportError> {
        let imap = imap_login(credential).await?;
        let smtp = smtp_transport(credential)?;
        Ok(Box::new(StandardSession {
            imap: Some(imap),
            smtp,
            address: credential.address().to_string(),
            selected: None,
        }))
    }
}

/// One open connection to one mailbox.
struct StandardSession {
    /// The IMAP session. It is absent only while an IDLE turn holds
    /// it, because `async-imap` moves the session into the IDLE handle
    /// and gives it back at `done()`.
    imap: Option<async_imap::Session<MailStream>>,
    smtp: AsyncSmtpTransport<Tokio1Executor>,
    address: String,
    /// The folder the last `select` opened.
    selected: Option<String>,
}

impl StandardSession {
    fn imap(&mut self) -> Result<&mut async_imap::Session<MailStream>, TransportError> {
        self.imap
            .as_mut()
            .ok_or(TransportError(TransportErrorCode::Unreachable))
    }

    /// Open the folder and read its numbering. Every read selects,
    /// because a folder the host renumbered or new mail that arrived
    /// shows in the `SELECT` answer and nowhere else.
    async fn select_folder(&mut self, folder: &str) -> Result<FolderState, TransportError> {
        let session = self.imap()?;
        let mailbox = with_timeout(session.select(folder)).await?;
        self.selected = Some(folder.to_string());
        Ok(FolderState {
            folder: folder.to_string(),
            uid_validity: mailbox
                .uid_validity
                .ok_or(TransportError(TransportErrorCode::Unreadable))?,
            next_uid: mailbox
                .uid_next
                .ok_or(TransportError(TransportErrorCode::Unreadable))?,
            exists: mailbox.exists,
        })
    }

    /// Select the folder only when another one is open. A fetch by id
    /// needs the folder open; it does not need fresh numbering.
    async fn ensure_selected(&mut self, folder: &str) -> Result<(), TransportError> {
        if self.selected.as_deref() == Some(folder) {
            return Ok(());
        }
        self.select_folder(folder).await?;
        Ok(())
    }

    /// Read the messages of a UID set, in the order the set names them.
    async fn read_messages(
        &mut self,
        folder: &str,
        uids: &[u32],
    ) -> Result<Vec<Message>, TransportError> {
        if uids.is_empty() {
            return Ok(Vec::new());
        }
        let set = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let session = self.imap()?;
        let stream = with_timeout(session.uid_fetch(set, FETCH_QUERY)).await?;
        let fetched: Vec<Fetch> = with_timeout(stream.try_collect()).await?;

        let mut messages = Vec::with_capacity(fetched.len());
        for uid in uids {
            let Some(fetch) = fetched.iter().find(|fetch| fetch.uid == Some(*uid)) else {
                // The message left the folder between the search and
                // the fetch. It is not an error; it is gone.
                continue;
            };
            let body = fetch
                .body()
                .ok_or(TransportError(TransportErrorCode::Unreadable))?;
            messages.push(read_mime(body, MessageId::new(folder, *uid))?);
        }
        Ok(messages)
    }

    /// The UIDs of the selected folder that match one search.
    async fn search_uids(&mut self, criteria: &str) -> Result<Vec<u32>, TransportError> {
        let session = self.imap()?;
        let found = with_timeout(session.uid_search(criteria)).await?;
        let mut uids: Vec<u32> = found.into_iter().collect();
        uids.sort_unstable();
        Ok(uids)
    }
}

#[async_trait]
impl MailSession for StandardSession {
    async fn select(&mut self, folder: &str) -> Result<FolderState, TransportError> {
        self.select_folder(folder).await
    }

    async fn fetch_since(
        &mut self,
        cursor: &Cursor,
    ) -> Result<(Vec<MessageSummary>, Cursor), TransportError> {
        let state = self.select_folder(&cursor.folder).await?;
        if state.uid_validity != cursor.uid_validity {
            // The host renumbered the folder, so every UID the cursor
            // names is another message now (ADR-0019).
            return Err(TransportError(TransportErrorCode::StaleId));
        }

        let first = cursor.last_uid.saturating_add(1);
        // `N:*` also answers with the last message of a folder whose
        // highest UID is below `N`, so the answer is filtered again.
        let mut uids = self.search_uids(&format!("UID {first}:*")).await?;
        uids.retain(|uid| *uid > cursor.last_uid);
        uids.truncate(FETCH_BATCH);

        let messages = self.read_messages(&cursor.folder, &uids).await?;
        let last_uid = uids.last().copied().unwrap_or(cursor.last_uid);
        Ok((
            messages
                .into_iter()
                .map(|message| message.summary)
                .collect(),
            Cursor {
                folder: cursor.folder.clone(),
                uid_validity: state.uid_validity,
                last_uid,
            },
        ))
    }

    async fn fetch(&mut self, id: &MessageId) -> Result<Message, TransportError> {
        self.ensure_selected(&id.folder).await?;
        self.read_messages(&id.folder, &[id.uid])
            .await?
            .pop()
            .ok_or(TransportError(TransportErrorCode::StaleId))
    }

    async fn search(&mut self, query: &MailQuery) -> Result<Vec<MessageSummary>, TransportError> {
        let folder = self
            .selected
            .clone()
            .ok_or(TransportError(TransportErrorCode::MailboxGone))?;
        let mut uids = self.search_uids(&search_criteria(query)).await?;
        // Newest first, and no more than the caller asked for.
        uids.reverse();
        uids.truncate(query.limit as usize);
        let messages = self.read_messages(&folder, &uids).await?;
        Ok(messages
            .into_iter()
            .map(|message| message.summary)
            .collect())
    }

    async fn fetch_thread(&mut self, thread_id: &str) -> Result<Vec<Message>, TransportError> {
        let folder = self
            .selected
            .clone()
            .ok_or(TransportError(TransportErrorCode::MailboxGone))?;
        let uids = self.search_uids(&thread_criteria(thread_id)).await?;
        let mut messages = self.read_messages(&folder, &uids).await?;
        messages.sort_by_key(|message| message.summary.date);
        Ok(messages)
    }

    async fn idle(&mut self, deadline: Duration) -> Result<IdleOutcome, TransportError> {
        let session = self
            .imap
            .take()
            .ok_or(TransportError(TransportErrorCode::Unreachable))?;
        let turn = deadline.min(MAX_IDLE_TURN);
        let mut handle = session.idle();
        if let Err(error) = handle.init().await {
            // The handle still owns the session; end the turn so the
            // caller can log out.
            self.imap = handle.done().await.ok();
            return Err(map_imap(error));
        }

        let outcome = {
            // The crate's own timeout restarts on every keepalive, so
            // the wall clock outside it is what ends the turn.
            let (waiting, _interrupt) = handle.wait_with_timeout(turn);
            match tokio::time::timeout(turn, waiting).await {
                Ok(Ok(IdleResponse::NewData(_))) => Ok(IdleOutcome::Changed),
                Ok(Ok(IdleResponse::Timeout | IdleResponse::ManualInterrupt)) => {
                    Ok(IdleOutcome::Deadline)
                }
                Ok(Err(error)) => Err(map_imap(error)),
                Err(_) => Ok(IdleOutcome::Deadline),
            }
        };

        let ended = handle.done().await;
        match (outcome, ended) {
            (Ok(outcome), Ok(session)) => {
                self.imap = Some(session);
                Ok(outcome)
            }
            (Err(error), ended) => {
                self.imap = ended.ok();
                Err(error)
            }
            (Ok(_), Err(error)) => Err(map_imap(error)),
        }
    }

    async fn send(&mut self, message: &OutgoingMessage) -> Result<SentMessage, TransportError> {
        let built = build_mime(&self.address, message)?;
        let envelope = Envelope::new(
            Some(mailbox_address(&self.address)?),
            built
                .recipients
                .iter()
                .map(|address| mailbox_address(address))
                .collect::<Result<Vec<Address>, TransportError>>()?,
        )
        .map_err(|_| TransportError(TransportErrorCode::Rejected))?;

        self.smtp
            .send_raw(&envelope, &built.raw)
            .await
            .map_err(map_smtp)?;
        Ok(SentMessage {
            message_id: built.message_id,
        })
    }

    async fn set_flags(&mut self, id: &MessageId, flags: FlagChange) -> Result<(), TransportError> {
        self.ensure_selected(&id.folder).await?;
        for (wanted, flag) in [(flags.seen, "\\Seen"), (flags.flagged, "\\Flagged")] {
            let Some(wanted) = wanted else { continue };
            let sign = if wanted { '+' } else { '-' };
            let session = self.imap()?;
            let stream = with_timeout(
                session.uid_store(id.uid.to_string(), format!("{sign}FLAGS ({flag})")),
            )
            .await?;
            // The answer is a stream of updated messages; it must be
            // read to the end before the next command goes out.
            let _: Vec<Fetch> = with_timeout(stream.try_collect()).await?;
        }
        Ok(())
    }

    async fn move_to_archive(&mut self, id: &MessageId) -> Result<(), TransportError> {
        self.ensure_selected(&id.folder).await?;
        let session = self.imap()?;
        // A host that has no archive folder yet gets one. A host that
        // has it answers `NO`, which is the same outcome.
        let _ = with_timeout(session.create(ARCHIVE)).await;
        let session = self.imap()?;
        with_timeout(session.uid_mv(id.uid.to_string(), ARCHIVE)).await?;
        Ok(())
    }
}

/// The IMAP `SEARCH` of one query. An absent field is not a condition,
/// and a query with no field at all reads the whole folder.
fn search_criteria(query: &MailQuery) -> String {
    let mut criteria: Vec<String> = Vec::new();
    for (key, value) in [
        ("FROM", &query.from),
        ("TO", &query.to),
        ("SUBJECT", &query.subject),
        ("BODY", &query.text),
    ] {
        if let Some(value) = value {
            criteria.push(format!("{key} {}", quoted(value)));
        }
    }
    if let Some(since) = query.since {
        criteria.push(format!("SINCE {}", imap_date(since)));
    }
    if let Some(before) = query.before {
        criteria.push(format!("BEFORE {}", imap_date(before)));
    }
    if criteria.is_empty() {
        return "ALL".to_string();
    }
    criteria.join(" ")
}

/// The search that finds one thread: the root message itself, and
/// every message that names the root in `References` or in
/// `In-Reply-To` (ADR-0019). IMAP matches a header substring, so the
/// id is searched without its angle brackets.
fn thread_criteria(thread_id: &str) -> String {
    let id = quoted(thread_id.trim().trim_matches(['<', '>']));
    format!("OR HEADER MESSAGE-ID {id} OR HEADER REFERENCES {id} HEADER IN-REPLY-TO {id}")
}

/// One IMAP quoted string (RFC 3501 4.3).
fn quoted(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// The date form IMAP `SINCE` and `BEFORE` read: `01-Jan-2026`.
fn imap_date(at: pagis_core::UnixMillis) -> String {
    chrono::DateTime::from_timestamp_millis(at)
        .unwrap_or_default()
        .format("%d-%b-%Y")
        .to_string()
}

fn mailbox_address(address: &str) -> Result<Address, TransportError> {
    address
        .parse::<Address>()
        .map_err(|_| TransportError(TransportErrorCode::Rejected))
}

/// A command the host does not answer inside the timeout leaves the
/// session in an unknown state, which the caller treats as dead.
async fn with_timeout<T>(
    work: impl Future<Output = async_imap::error::Result<T>>,
) -> Result<T, TransportError> {
    match tokio::time::timeout(COMMAND_TIMEOUT, work).await {
        Ok(answer) => answer.map_err(map_imap),
        Err(_) => Err(TransportError(TransportErrorCode::Unreachable)),
    }
}

/// A refused login is a different failure from a network failure: the
/// first makes the mailbox `unavailable`, the second is the reconnect
/// loop's business (ADR-0019).
fn map_imap(error: async_imap::error::Error) -> TransportError {
    use async_imap::error::Error;
    let code = match error {
        Error::No(_) => TransportErrorCode::Unauthorized,
        Error::Bad(_) | Error::Validate(_) | Error::Append => TransportErrorCode::Rejected,
        Error::Parse(_) => TransportErrorCode::Unreadable,
        Error::Io(_) | Error::ConnectionLost => TransportErrorCode::Unreachable,
        _ => TransportErrorCode::Unreachable,
    };
    TransportError(code)
}

fn map_smtp(error: SmtpError) -> TransportError {
    let code = match error.status() {
        // `5.3.z` is the authentication family: the host refused the
        // mailbox password.
        Some(code) if code.severity == Severity::PermanentNegativeCompletion => {
            match code.category as u8 {
                3 => TransportErrorCode::Unauthorized,
                _ => TransportErrorCode::Rejected,
            }
        }
        Some(_) => TransportErrorCode::Unreachable,
        None => TransportErrorCode::Unreachable,
    };
    TransportError(code)
}

// --- the connections ---

/// How one endpoint is encrypted. A host reached over the loopback is
/// the Docker test server and speaks in the clear;
/// every other host gets TLS, implicit on the TLS port and STARTTLS
/// anywhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Security {
    Plain,
    Implicit,
    StartTls,
}

fn security_of(endpoint: &Endpoint, tls_port: u16) -> Security {
    if is_loopback(&endpoint.host) {
        Security::Plain
    } else if endpoint.port == tls_port {
        Security::Implicit
    } else {
        Security::StartTls
    }
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// The one TLS setup of the mail path: the compiled-in web roots and
/// the `ring` provider, named so a second provider in the build cannot
/// make the choice ambiguous.
fn tls_config() -> Arc<ClientConfig> {
    static CONFIG: std::sync::OnceLock<Arc<ClientConfig>> = std::sync::OnceLock::new();
    Arc::clone(CONFIG.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
        Arc::new(
            ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("the ring provider supports the default protocol versions")
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    }))
}

async fn connect(endpoint: &Endpoint) -> Result<TcpStream, TransportError> {
    let stream = tokio::time::timeout(
        COMMAND_TIMEOUT,
        TcpStream::connect((endpoint.host.as_str(), endpoint.port)),
    )
    .await
    .map_err(|_| TransportError(TransportErrorCode::Unreachable))?
    .map_err(|_| TransportError(TransportErrorCode::Unreachable))?;
    // Nothing goes out during an IDLE turn, so the keepalive is what
    // holds a NAT mapping open.
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

async fn upgrade(stream: TcpStream, host: &str) -> Result<MailStream, TransportError> {
    let name = ServerName::try_from(host.to_string())
        .map_err(|_| TransportError(TransportErrorCode::Unreachable))?;
    let stream = TlsConnector::from(tls_config())
        .connect(name, stream)
        .await
        .map_err(|_| TransportError(TransportErrorCode::Unreachable))?;
    Ok(MailStream::Tls(Box::new(stream)))
}

/// Open the IMAP connection, read the greeting, upgrade where the
/// endpoint asks for it, and prove the password.
async fn imap_login(
    credential: &MailboxCredential,
) -> Result<async_imap::Session<MailStream>, TransportError> {
    let endpoint = credential.imap();
    let security = security_of(endpoint, IMAPS_PORT);
    let tcp = connect(endpoint).await?;

    let mut client = match security {
        Security::Implicit => Client::new(upgrade(tcp, &endpoint.host).await?),
        Security::Plain | Security::StartTls => Client::new(MailStream::Plain(tcp)),
    };
    // The greeting is the readiness of the host, and it must leave the
    // stream before the first command.
    match tokio::time::timeout(COMMAND_TIMEOUT, client.read_response()).await {
        Ok(Ok(Some(_greeting))) => {}
        _ => return Err(TransportError(TransportErrorCode::Unreachable)),
    }

    if security == Security::StartTls {
        with_timeout(client.run_command_and_check_ok("STARTTLS", None)).await?;
        let MailStream::Plain(tcp) = client.into_inner() else {
            unreachable!("STARTTLS runs on a plain stream only");
        };
        // A host sends no greeting after STARTTLS.
        client = Client::new(upgrade(tcp, &endpoint.host).await?);
    }

    match tokio::time::timeout(
        COMMAND_TIMEOUT,
        client.login(credential.address(), credential.expose_password()),
    )
    .await
    {
        Ok(Ok(session)) => Ok(session),
        Ok(Err((error, _client))) => Err(map_imap(error)),
        Err(_) => Err(TransportError(TransportErrorCode::Unreachable)),
    }
}

fn smtp_transport(
    credential: &MailboxCredential,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, TransportError> {
    let endpoint = credential.smtp();
    let builder = match security_of(endpoint, SMTPS_PORT) {
        Security::Plain => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&endpoint.host),
        Security::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&endpoint.host)
            .map_err(|_| TransportError(TransportErrorCode::Unreachable))?,
        Security::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&endpoint.host)
            .map_err(|_| TransportError(TransportErrorCode::Unreachable))?,
    };
    Ok(builder
        .port(endpoint.port)
        .timeout(Some(COMMAND_TIMEOUT))
        .credentials(Credentials::new(
            credential.address().to_string(),
            credential.expose_password().to_string(),
        ))
        .build())
}

/// The one stream type the IMAP session runs over, so the session has
/// one type whether the host wanted TLS or not.
#[derive(Debug)]
enum MailStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for MailStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MailStream::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            MailStream::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MailStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            MailStream::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            MailStream::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MailStream::Plain(stream) => Pin::new(stream).poll_flush(cx),
            MailStream::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            MailStream::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            MailStream::Tls(stream) => Pin::new(stream.as_mut()).poll_shutdown(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_with_no_field_reads_the_whole_folder() {
        assert_eq!(search_criteria(&MailQuery::default()), "ALL");
    }

    #[test]
    fn a_query_becomes_one_imap_search() {
        let criteria = search_criteria(&MailQuery {
            from: Some("ada@example.test".into()),
            subject: Some("the \"roof\"".into()),
            since: Some(1_788_343_200_000),
            ..MailQuery::default()
        });
        assert_eq!(
            criteria,
            "FROM \"ada@example.test\" SUBJECT \"the \\\"roof\\\"\" SINCE 02-Sep-2026"
        );
    }

    #[test]
    fn a_loopback_host_speaks_in_the_clear_and_every_other_host_gets_tls() {
        assert_eq!(
            security_of(&Endpoint::new("127.0.0.1", 3143), IMAPS_PORT),
            Security::Plain
        );
        assert_eq!(
            security_of(&Endpoint::new("localhost", 3143), IMAPS_PORT),
            Security::Plain
        );
        assert_eq!(
            security_of(&Endpoint::new("imap.migadu.com", 993), IMAPS_PORT),
            Security::Implicit
        );
        assert_eq!(
            security_of(&Endpoint::new("imap.example.test", 143), IMAPS_PORT),
            Security::StartTls
        );
        assert_eq!(
            security_of(&Endpoint::new("smtp.example.test", 587), SMTPS_PORT),
            Security::StartTls
        );
        assert_eq!(
            security_of(&Endpoint::new("smtp.example.test", 465), SMTPS_PORT),
            Security::Implicit
        );
    }
}
