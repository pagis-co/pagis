//! The standard-protocol transport (ADR-0019): IMAP to read and to
//! wait, SMTP submission to send, with the mailbox's own password.
//!
//! One [`StandardSession`] holds one IMAP connection and one SMTP
//! transport. The IMAP half is stateful, so one task owns the session
//! and the seam takes `&mut self`.
//!
//! The daemon owns the IDLE deadline. The session ends each turn with
//! DONE before the next command, as RFC 2177 requires.

use crate::ImapMessageId;
use crate::MessageId;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use crate::imap::{COMMAND_TIMEOUT, Driver, timeout};
use async_trait::async_trait;
use imap_next::imap_types::{
    command::CommandBody,
    fetch::MessageDataItem,
    response::{Capability, Code, Data, Response},
};
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
    MailTransport, MailboxCredential, Message, MessageSummary, OutgoingMessage, SentMessage,
    TransportError, TransportErrorCode,
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
    async fn login(
        &self,
        credential: &MailboxCredential,
    ) -> Result<Box<dyn MailSession>, TransportError> {
        let imap = imap_connect(credential.imap()).await?;
        Ok(Box::new(
            StandardSession::authenticated(imap, credential).await?,
        ))
    }
}

/// One IMAP client over the byte stream supplied by its host.
struct StandardSession<S> {
    imap: Driver<S>,
    smtp: AsyncSmtpTransport<Tokio1Executor>,
    address: String,
    idle_supported: bool,
    selected: Option<String>,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> StandardSession<S> {
    #[cfg(test)]
    async fn login(stream: S, credential: &MailboxCredential) -> Result<Self, TransportError> {
        let mut imap = Driver::new(stream);
        imap.greeting().await?;
        Self::authenticated(imap, credential).await
    }

    async fn authenticated(
        mut imap: Driver<S>,
        credential: &MailboxCredential,
    ) -> Result<Self, TransportError> {
        let login = CommandBody::login(
            credential.address().to_string(),
            credential.expose_password().to_string(),
        )
        .map_err(|_| TransportError(TransportErrorCode::Rejected))?;
        imap.command(login).await?;
        // Capabilities can change after authentication. Read the server's
        // current set, rather than the provider's display flag.
        let responses = imap.command(CommandBody::Capability).await?;
        let idle_supported = responses.iter().any(|response| matches!(response,
            Response::Data(Data::Capability(capabilities)) if capabilities.as_ref().contains(&Capability::Idle)));
        Ok(Self {
            imap,
            smtp: smtp_transport(credential)?,
            address: credential.address().to_string(),
            idle_supported,
            selected: None,
        })
    }

    /// Open the folder and read its numbering. Every read selects,
    /// because a folder the host renumbered or new mail that arrived
    /// shows in the `SELECT` answer and nowhere else.
    async fn select_folder(&mut self, folder: &str) -> Result<FolderState, TransportError> {
        let body = CommandBody::select(folder.to_string())
            .map_err(|_| TransportError(TransportErrorCode::Rejected))?;
        let responses = self.imap.command(body).await?;
        let mut uid_validity = None;
        let mut next_uid = None;
        let mut exists = 0;
        for response in &responses {
            match response {
                Response::Data(Data::Exists(count)) => exists = *count,
                Response::Status(status) => match status.code() {
                    Some(Code::UidValidity(value)) => uid_validity = Some(value.get()),
                    Some(Code::UidNext(value)) => next_uid = Some(value.get()),
                    _ => {}
                },
                _ => {}
            }
        }
        self.selected = Some(folder.to_string());
        Ok(FolderState {
            folder: folder.to_string(),
            uid_validity: uid_validity.ok_or(TransportError(TransportErrorCode::Unreadable))?,
            next_uid: next_uid.ok_or(TransportError(TransportErrorCode::Unreadable))?,
            exists,
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
        let responses = self
            .imap
            .command_text(&format!("UID FETCH {set} {FETCH_QUERY}"))
            .await?;
        let mut fetched = Vec::new();
        for response in responses {
            if let Response::Data(Data::Fetch { items, .. }) = response {
                let uid = items.as_ref().iter().find_map(|item| match item {
                    MessageDataItem::Uid(uid) => Some(uid.get()),
                    _ => None,
                });
                let body = items.as_ref().iter().find_map(|item| match item {
                    MessageDataItem::BodyExt {
                        section: None,
                        origin: None,
                        data,
                    } => data.0.as_ref().map(|data| data.as_ref()),
                    _ => None,
                });
                if let Some(uid) = uid {
                    let body = body.ok_or(TransportError(TransportErrorCode::Unreadable))?;
                    fetched.push((
                        uid,
                        read_mime(body, ImapMessageId::new(folder, uid).into())?,
                    ));
                }
            }
        }
        // A message can leave the folder between SEARCH and FETCH.
        Ok(uids
            .iter()
            .filter_map(|uid| {
                fetched
                    .iter()
                    .find(|(found, _)| found == uid)
                    .map(|(_, message)| message.clone())
            })
            .collect())
    }

    /// The UIDs of the selected folder that match one search.
    async fn search_uids(&mut self, criteria: &str) -> Result<Vec<u32>, TransportError> {
        let responses = self
            .imap
            .command_text(&format!("UID SEARCH {criteria}"))
            .await?;
        let mut uids = Vec::new();
        for response in responses {
            if let Response::Data(Data::Search(found)) = response {
                uids.extend(found.into_iter().map(|uid| uid.get()));
            }
        }
        uids.sort_unstable();
        uids.dedup();
        Ok(uids)
    }
}

#[async_trait]
impl<S: AsyncRead + AsyncWrite + Unpin + Send> MailSession for StandardSession<S> {
    async fn folders(&mut self) -> Result<Vec<String>, TransportError> {
        let responses = self.imap.command_text("LIST \"\" \"*\"").await?;
        let mut folders = Vec::new();
        for response in responses {
            if let Response::Data(Data::List { mailbox, items, .. }) = response {
                if items.contains(&imap_next::imap_types::flag::FlagNameAttribute::Noselect) {
                    continue;
                }
                folders.push(match mailbox {
                    imap_next::imap_types::mailbox::Mailbox::Inbox => "INBOX".into(),
                    imap_next::imap_types::mailbox::Mailbox::Other(name) => {
                        String::from_utf8(name.inner().as_ref().to_vec())
                            .map_err(|_| TransportError(TransportErrorCode::Unreadable))?
                    }
                });
            }
        }
        folders.sort();
        Ok(folders)
    }
    async fn metadata(&mut self, id: &MessageId) -> Result<crate::MailMetadata, TransportError> {
        let id = id.as_str().parse::<ImapMessageId>()?;
        self.ensure_selected(&id.folder).await?;
        let responses = self
            .imap
            .command_text(&format!("UID FETCH {} (FLAGS)", id.uid))
            .await?;
        for response in responses {
            if let Response::Data(Data::Fetch { items, .. }) = response {
                if !items
                    .as_ref()
                    .iter()
                    .any(|item| matches!(item, MessageDataItem::Uid(uid) if uid.get() == id.uid))
                {
                    continue;
                }
                for item in items.as_ref() {
                    if let MessageDataItem::Flags(flags) = item {
                        let mut metadata = crate::MailMetadata::default();
                        for flag in flags {
                            let imap_next::imap_types::flag::FlagFetch::Flag(flag) = flag else {
                                continue;
                            };
                            let value = flag.to_string();
                            match value.as_str() {
                                "\\Seen" => metadata.seen = true,
                                "\\Flagged" => metadata.flagged = true,
                                _ if !value.starts_with('\\') => metadata.labels.push(value),
                                _ => {}
                            }
                        }
                        return Ok(metadata);
                    }
                }
            }
        }
        Err(TransportError(TransportErrorCode::StaleId))
    }

    fn idle_supported(&self) -> bool {
        self.idle_supported
    }

    async fn logout(&mut self) -> Result<(), TransportError> {
        self.imap.logout().await
    }

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
        let id = id.as_str().parse::<ImapMessageId>()?;
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
        if !self.idle_supported {
            return Err(TransportError(TransportErrorCode::Rejected));
        }
        self.imap.idle(deadline.min(MAX_IDLE_TURN)).await
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
        let id = id.as_str().parse::<ImapMessageId>()?;
        self.ensure_selected(&id.folder).await?;
        for (wanted, flag) in [(flags.seen, "\\Seen"), (flags.flagged, "\\Flagged")] {
            let Some(wanted) = wanted else { continue };
            let sign = if wanted { '+' } else { '-' };
            self.imap
                .command_text(&format!("UID STORE {} {sign}FLAGS ({flag})", id.uid))
                .await?;
        }
        Ok(())
    }

    async fn move_to_archive(&mut self, id: &MessageId) -> Result<(), TransportError> {
        let id = id.as_str().parse::<ImapMessageId>()?;
        self.ensure_selected(&id.folder).await?;
        // CREATE can answer NO when the archive already exists.
        let _ = self.imap.command_text(&format!("CREATE {ARCHIVE}")).await;
        self.imap
            .command_text(&format!("UID MOVE {} {ARCHIVE}", id.uid))
            .await?;
        Ok(())
    }
}

/// The IMAP `SEARCH` of one query. An absent field is not a condition,
/// and a query with no field at all reads the whole folder.
fn search_criteria(query: &MailQuery) -> String {
    let mut criteria: Vec<String> = Vec::new();
    if let Some(before) = query.before_uid {
        if before <= 1 {
            return "NOT ALL".into();
        }
        criteria.push(format!("UID 1:{}", before - 1));
    }
    for (key, value) in [
        ("FROM", &query.from),
        ("TO", &query.to),
        ("SUBJECT", &query.subject),
        ("TEXT", &query.text),
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
    let stream = timeout(async {
        TlsConnector::from(tls_config())
            .connect(name, stream)
            .await
            .map_err(|_| TransportError(TransportErrorCode::Unreachable))
    })
    .await?;
    Ok(MailStream::Tls(Box::new(stream)))
}

/// Read the greeting and complete the TLS handshake before LOGIN.
async fn imap_connect(endpoint: &Endpoint) -> Result<Driver<MailStream>, TransportError> {
    let security = security_of(endpoint, IMAPS_PORT);
    let tcp = connect(endpoint).await?;
    let stream = match security {
        Security::Implicit => upgrade(tcp, &endpoint.host).await?,
        Security::Plain | Security::StartTls => MailStream::Plain(tcp),
    };
    let mut driver = Driver::new(stream);
    driver.greeting().await?;
    if security == Security::StartTls {
        driver.command(CommandBody::StartTLS).await?;
        let MailStream::Plain(tcp) = driver.stream else {
            unreachable!("STARTTLS uses a plain stream");
        };
        driver.stream = upgrade(tcp, &endpoint.host).await?;
    }
    Ok(driver)
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
    use crate::INBOX;

    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    fn credential() -> MailboxCredential {
        MailboxCredential::new(
            "ada@example.test",
            "password",
            Endpoint::new("localhost", 143),
            Endpoint::new("localhost", 25),
        )
    }

    /// Each response follows the complete command. Small writes split
    /// responses across reads, including the message literal.
    fn script(turns: Vec<(String, String)>) -> (DuplexStream, tokio::task::JoinHandle<()>) {
        let (client, mut server) = tokio::io::duplex(64);
        let handle = tokio::spawn(async move {
            server.write_all(b"* OK ready\r\n").await.unwrap();
            for (command, response) in turns {
                let mut got = vec![0; command.len()];
                server.read_exact(&mut got).await.unwrap();
                assert_eq!(got, command.as_bytes());
                for chunk in response.as_bytes().chunks(7) {
                    server.write_all(chunk).await.unwrap();
                }
            }
        });
        (client, handle)
    }

    fn login_turns(idle: bool) -> Vec<(String, String)> {
        vec![
            (
                "A1 LOGIN ada@example.test password\r\n".into(),
                "A1 OK logged in\r\n".into(),
            ),
            (
                "A2 CAPABILITY\r\n".into(),
                format!(
                    "* CAPABILITY IMAP4rev1{}\r\nA2 OK capabilities\r\n",
                    if idle { " IDLE" } else { "" }
                ),
            ),
        ]
    }

    fn select_turn(tag: &str) -> (String, String) {
        (
            format!("{tag} SELECT INBOX\r\n"),
            format!(
                "* 2 EXISTS\r\n* OK [UIDVALIDITY 7] validity\r\n* OK [UIDNEXT 10] next\r\n{tag} OK [READ-WRITE] selected\r\n"
            ),
        )
    }

    #[tokio::test]
    async fn scripted_login_select_and_uid_fetch_from_a_cursor() {
        let raw = b"Message-ID: <nine@example.test>\r\nFrom: care@example.test\r\nSubject: The roof\r\n\r\nHello.\r\n";
        let mut turns = login_turns(true);
        turns.push(select_turn("A3"));
        turns.push(select_turn("A4"));
        turns.push((
            "A5 UID SEARCH UID 9:*\r\n".into(),
            "* SEARCH 8 9\r\nA5 OK searched\r\n".into(),
        ));
        turns.push((
            "A6 UID FETCH 9 (UID BODY.PEEK[])\r\n".into(),
            format!(
                "* 2 FETCH (UID 9 BODY[] {{{}}}\r\n{})\r\nA6 OK fetched\r\n",
                raw.len(),
                String::from_utf8_lossy(raw)
            ),
        ));
        turns.push((
            "A7 LOGOUT\r\n".into(),
            "* BYE closed\r\nA7 OK logout\r\n".into(),
        ));
        let (stream, server) = script(turns);
        let mut session = StandardSession::login(stream, &credential()).await.unwrap();
        assert!(session.idle_supported());
        let state = session.select(INBOX).await.unwrap();
        assert_eq!(
            (state.uid_validity, state.next_uid, state.exists),
            (7, 10, 2)
        );
        let (messages, next) = session
            .fetch_since(&Cursor {
                folder: INBOX.into(),
                uid_validity: 7,
                last_uid: 8,
            })
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].subject, "The roof");
        assert_eq!(next.last_uid, 9);
        session.logout().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn scripted_idle_sends_done_before_the_next_command() {
        let mut turns = login_turns(true);
        turns.push(select_turn("A3"));
        turns.push(("A4 IDLE\r\n".into(), "+ idling\r\n* 3 EXISTS\r\n".into()));
        turns.push(("DONE\r\n".into(), "A4 OK idle ended\r\n".into()));
        turns.push((
            "A5 LOGOUT\r\n".into(),
            "* BYE closed\r\nA5 OK logout\r\n".into(),
        ));
        let (stream, server) = script(turns);
        let mut session = StandardSession::login(stream, &credential()).await.unwrap();
        session.select(INBOX).await.unwrap();
        assert_eq!(
            session.idle(Duration::from_secs(10)).await.unwrap(),
            IdleOutcome::Changed
        );
        session.logout().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn scripted_idle_deadline_sends_done() {
        let mut turns = login_turns(true);
        turns.push(("A3 IDLE\r\n".into(), "+ idling\r\n".into()));
        turns.push(("DONE\r\n".into(), "A3 OK idle ended\r\n".into()));
        let (stream, server) = script(turns);
        let mut session = StandardSession::login(stream, &credential()).await.unwrap();
        assert_eq!(
            session.idle(Duration::from_secs(10)).await.unwrap(),
            IdleOutcome::Deadline
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn scripted_server_without_idle_sets_the_session_flag() {
        let (stream, server) = script(login_turns(false));
        let session = StandardSession::login(stream, &credential()).await.unwrap();
        assert!(!session.idle_supported());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn scripted_refused_login_is_unauthorized() {
        let (stream, server) = script(vec![(
            "A1 LOGIN ada@example.test password\r\n".into(),
            "A1 NO wrong password\r\n".into(),
        )]);
        let error = StandardSession::login(stream, &credential())
            .await
            .err()
            .unwrap();
        assert_eq!(error.0, TransportErrorCode::Unauthorized);
        server.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn scripted_command_timeout_is_unreachable() {
        let (stream, mut server) = tokio::io::duplex(128);
        server.write_all(b"* OK ready\r\n").await.unwrap();
        let error = StandardSession::login(stream, &credential())
            .await
            .err()
            .unwrap();
        assert_eq!(error.0, TransportErrorCode::Unreachable);
    }

    #[tokio::test(start_paused = true)]
    async fn scripted_cancelled_idle_logs_out_with_done() {
        let mut turns = login_turns(true);
        turns.push(("A3 IDLE\r\n".into(), "+ idling\r\n".into()));
        turns.push(("DONE\r\n".into(), "A3 OK idle ended\r\n".into()));
        turns.push((
            "A4 LOGOUT\r\n".into(),
            "* BYE closed\r\nA4 OK logout\r\n".into(),
        ));
        let (stream, server) = script(turns);
        let mut session = StandardSession::login(stream, &credential()).await.unwrap();
        // Cancellation drops the wait, as the collector does at shutdown.
        assert!(
            tokio::time::timeout(
                Duration::from_secs(1),
                session.idle(Duration::from_secs(10))
            )
            .await
            .is_err()
        );
        session.logout().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn scripted_bad_login_is_rejected_without_a_password_refusal() {
        let (stream, server) = script(vec![(
            "A1 LOGIN ada@example.test password\r\n".into(),
            "A1 BAD login disabled\r\n".into(),
        )]);
        let error = StandardSession::login(stream, &credential())
            .await
            .err()
            .unwrap();
        assert_eq!(error.0, TransportErrorCode::Rejected);
        server.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn scripted_cancelled_partial_idle_write_keeps_the_command_bytes() {
        let (stream, mut server) = tokio::io::duplex(1);
        let handle = tokio::spawn(async move {
            server.write_all(b"* OK ready\r\n").await.unwrap();
            for (command, response) in login_turns(true) {
                let mut got = vec![0; command.len()];
                server.read_exact(&mut got).await.unwrap();
                assert_eq!(got, command.as_bytes());
                server.write_all(response.as_bytes()).await.unwrap();
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            for (command, response) in [
                ("A3 IDLE\r\n", "+ idling\r\n"),
                ("DONE\r\n", "A3 OK idle ended\r\n"),
                ("A4 LOGOUT\r\n", "* BYE closed\r\nA4 OK logout\r\n"),
            ] {
                let mut got = vec![0; command.len()];
                server.read_exact(&mut got).await.unwrap();
                assert_eq!(got, command.as_bytes());
                server.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let mut session = StandardSession::login(stream, &credential()).await.unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_secs(1),
                session.idle(Duration::from_secs(10))
            )
            .await
            .is_err()
        );
        session.logout().await.unwrap();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn scripted_workspace_mail_folders_skip_nonselectable_names() {
        let mut turns = login_turns(false);
        turns.push(("A3 LIST \"\" \"*\"\r\n".into(),
            "* LIST (\\Noselect) \"/\" parent\r\n* LIST () \"/\" INBOX\r\n* LIST () \"/\" Archive\r\nA3 OK list\r\n".into()));
        let (stream, server) = script(turns);
        let mut session = StandardSession::login(stream, &credential()).await.unwrap();
        assert_eq!(session.folders().await.unwrap(), ["Archive", "INBOX"]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn scripted_workspace_mail_reads_flags_and_custom_labels() {
        let mut turns = login_turns(false);
        turns.push(select_turn("A3"));
        turns.push((
            "A4 UID FETCH 9 FLAGS\r\n".into(),
            "* 2 FETCH (UID 9 FLAGS (\\Seen \\Flagged \\Recent invoices))\r\nA4 OK fetched\r\n"
                .into(),
        ));
        let (stream, server) = script(turns);
        let mut session = StandardSession::login(stream, &credential()).await.unwrap();
        let metadata = session.metadata(&MessageId::new("INBOX:9")).await.unwrap();
        assert!(metadata.seen);
        assert!(metadata.flagged);
        assert_eq!(metadata.labels, ["invoices"]);
        server.await.unwrap();
    }

    #[test]
    fn a_text_query_searches_headers_and_body() {
        assert_eq!(
            search_criteria(&MailQuery {
                text: Some("invoice".into()),
                ..Default::default()
            }),
            "TEXT \"invoice\""
        );
    }

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
