//! The mail seam (ADR-0019): read over IMAP, wait with IDLE, send over
//! SMTP. It uses the mailbox's own password and never the host API key.
//!
//! A [`MailTransport`] proves one mailbox with `login` and gives back a
//! [`MailSession`], the open connection. The session holds the state a
//! mail connection has: the selected folder and the IDLE turn. The
//! daemon owns the wall-clock deadline of each IDLE turn, the re-issue
//! and the reconnect (ADR-0019), so `idle` returns at the deadline and
//! says nothing arrived.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::UnixMillis;

/// The folder every mailbox has, and the one the collector watches.
pub const INBOX: &str = "INBOX";

/// The folder `move_to_archive` moves a message to.
pub const ARCHIVE: &str = "Archive";

/// One host and port of the mail service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

/// What one mailbox logs in with: its address, its own password, and
/// the two endpoints of its Connection. The password has no accessor
/// that prints it and no `Debug` that shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct MailboxCredential {
    address: String,
    password: String,
    imap: Endpoint,
    smtp: Endpoint,
}

impl MailboxCredential {
    pub fn new(
        address: impl Into<String>,
        password: impl Into<String>,
        imap: Endpoint,
        smtp: Endpoint,
    ) -> Self {
        Self {
            address: address.into(),
            password: password.into(),
            imap,
            smtp,
        }
    }

    /// The address, which is also the IMAP and SMTP username.
    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn imap(&self) -> &Endpoint {
        &self.imap
    }

    pub fn smtp(&self) -> &Endpoint {
        &self.smtp
    }

    /// The one reader. Only a transport calls it.
    pub fn expose_password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for MailboxCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MailboxCredential")
            .field("address", &self.address)
            .field("password", &"redacted")
            .field("imap", &self.imap)
            .field("smtp", &self.smtp)
            .finish()
    }
}

/// What a transport can and cannot do (ADR-0005, ADR-0019).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportCapabilities {
    /// Whether the transport waits for new mail on the connection. A
    /// transport without it is polled instead.
    pub idle: bool,
}

/// One message in one folder: the folder and the IMAP UID. It reads as
/// `INBOX:1234` (ADR-0019) and stays stable while UIDVALIDITY holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageId {
    pub folder: String,
    pub uid: u32,
}

impl MessageId {
    pub fn new(folder: impl Into<String>, uid: u32) -> Self {
        Self {
            folder: folder.into(),
            uid,
        }
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.folder, self.uid)
    }
}

/// The one shape a message id has. Anything else is an id from another
/// mailbox or a typing mistake, and the tools answer `stale_id`.
#[derive(Debug, thiserror::Error)]
#[error("a message id reads as folder:uid, for example INBOX:1234")]
pub struct MessageIdError;

impl FromStr for MessageId {
    type Err = MessageIdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (folder, uid) = text.rsplit_once(':').ok_or(MessageIdError)?;
        if folder.is_empty() {
            return Err(MessageIdError);
        }
        Ok(Self {
            folder: folder.to_string(),
            uid: uid.parse().map_err(|_| MessageIdError)?,
        })
    }
}

/// Where a collector reached in one folder. The Agent Mailbox record
/// carries it between restarts, so the type is the record's own
/// (ADR-0019).
pub use pagis_core::MailboxCursor as Cursor;

/// What a `select` found: the numbering of the folder and how much is
/// in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderState {
    pub folder: String,
    pub uid_validity: u32,
    /// The UID the next message gets.
    pub next_uid: u32,
    /// How many messages the folder holds.
    pub exists: u32,
}

impl FolderState {
    /// A cursor that starts here, so the mail already in the folder is
    /// readable but wakes nobody (ADR-0019).
    pub fn cursor_here(&self) -> Cursor {
        Cursor {
            folder: self.folder.clone(),
            uid_validity: self.uid_validity,
            last_uid: self.next_uid.saturating_sub(1),
        }
    }
}

/// One attachment, as the tools show it: the name and the size, never
/// the bytes (ADR-0019).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub bytes: u64,
}

/// One message as a list answers it (ADR-0019).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageSummary {
    pub id: MessageId,
    /// The `Message-ID` header, in its bracket form. It is the id the
    /// sender wrote, so it is the identity a collector deduplicates on
    /// (ADR-0019), and it is empty where a message carries none.
    pub message_id: String,
    /// The thread the message belongs to: the root of its References.
    pub thread_id: String,
    /// The `Message-ID` this message answers, in its bracket form. A
    /// reply to mail the Agent sent lands in the Thread that sent it
    /// (ADR-0019).
    pub in_reply_to: Option<String>,
    pub from: String,
    /// How many mailboxes the `From` headers name together. Each `From`
    /// header counts one at least, so two headers count two whatever
    /// they hold. The sender check needs exactly one (RFC 7489 6.6.1).
    pub from_mailboxes: usize,
    pub to: Vec<String>,
    pub date: UnixMillis,
    pub subject: String,
    /// The opening of the text body.
    pub snippet: String,
    pub has_attachments: bool,
    /// The value of every Authentication-Results header, topmost first
    /// (RFC 8601). The collector verifies the sender from them
    /// (ADR-0019); the tools never show them.
    pub authentication_results: Vec<String>,
}

/// One message with its body. HTML is converted to text, and the bytes
/// of an attachment stay at the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub summary: MessageSummary,
    /// The headers the tools show, in the order the message carries
    /// them.
    pub headers: Vec<(String, String)>,
    pub text: String,
    pub attachments: Vec<Attachment>,
}

/// What a search asks for. An absent field is not a condition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    /// Text in the body.
    pub text: Option<String>,
    pub since: Option<UnixMillis>,
    pub before: Option<UnixMillis>,
    /// The most summaries to answer with.
    pub limit: u32,
}

/// How one IDLE turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOutcome {
    /// The folder changed. The caller fetches from its cursor.
    Changed,
    /// The deadline passed and nothing changed. The caller re-issues
    /// IDLE, which is what RFC 2177 asks of a client.
    Deadline,
}

/// One message to send. A reply is a send with `in_reply_to`
/// (ADR-0019): the transport sets `In-Reply-To` and `References` and
/// the subject from the message that is answered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutgoingMessage {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    /// The `Message-ID` of the message this one answers.
    pub in_reply_to: Option<String>,
    /// The `Message-ID` this send must carry, without the angle brackets.
    /// A reserved effect supplies its reservation identity here, so the
    /// message the host accepted is the one the reservation names.
    /// `None` mints a fresh identity.
    pub message_id: Option<String>,
}

/// What the host accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMessage {
    /// The `Message-ID` the send carried.
    pub message_id: String,
}

/// The reversible state of one message (ADR-0019). An absent field is
/// left as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlagChange {
    pub seen: Option<bool>,
    pub flagged: Option<bool>,
}

/// Why the host did not do what it was asked. The codes are stable,
/// because they reach the user and the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportErrorCode {
    /// The host refused the mailbox password.
    Unauthorized,
    /// The host did not answer: DNS, the socket or a timeout.
    Unreachable,
    /// The host reports the mailbox or the folder gone.
    MailboxGone,
    /// The folder was renumbered, so the id and the cursor are stale.
    StaleId,
    /// The host refused the message.
    Rejected,
    /// The host answered with something this client cannot read.
    Unreadable,
}

impl TransportErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportErrorCode::Unauthorized => "unauthorized",
            TransportErrorCode::Unreachable => "unreachable",
            TransportErrorCode::MailboxGone => "mailbox_gone",
            TransportErrorCode::StaleId => "stale_id",
            TransportErrorCode::Rejected => "rejected",
            TransportErrorCode::Unreadable => "unreadable",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the mail host did not complete this: {}", .0.as_str())]
pub struct TransportError(pub TransportErrorCode);

/// Read and send as one mailbox (ADR-0019).
#[async_trait]
pub trait MailTransport: Send + Sync {
    /// What this transport declares. The daemon reads it and polls the
    /// mailbox when `idle` is absent.
    fn capabilities(&self) -> TransportCapabilities;

    /// Prove the password and open the connection. A mailbox that
    /// logs in is `active` (ADR-0019).
    async fn login(
        &self,
        credential: &MailboxCredential,
    ) -> Result<Box<dyn MailSession>, TransportError>;
}

/// One open connection to one mailbox. It is the stateful half of the
/// seam: the selected folder is its state, and one task owns it.
#[async_trait]
pub trait MailSession: Send {
    /// Open one folder and read its numbering.
    async fn select(&mut self, folder: &str) -> Result<FolderState, TransportError>;

    /// The messages after the cursor, oldest first, with the cursor
    /// that follows them. A cursor from another numbering answers
    /// `stale_id`.
    async fn fetch_since(
        &mut self,
        cursor: &Cursor,
    ) -> Result<(Vec<MessageSummary>, Cursor), TransportError>;

    /// One message with its body.
    async fn fetch(&mut self, id: &MessageId) -> Result<Message, TransportError>;

    /// The messages of the selected folder that match, newest first.
    async fn search(&mut self, query: &MailQuery) -> Result<Vec<MessageSummary>, TransportError>;

    /// Every message of one thread in the selected folder, oldest
    /// first. A thread is the root `Message-ID` and every message that
    /// carries it in `References` or `In-Reply-To` (ADR-0019).
    async fn fetch_thread(&mut self, thread_id: &str) -> Result<Vec<Message>, TransportError>;

    /// Wait for the selected folder to change, for at most `deadline`.
    /// The caller re-issues the turn; the transport never waits longer
    /// than it is asked to.
    async fn idle(&mut self, deadline: Duration) -> Result<IdleOutcome, TransportError>;

    /// Send one message as this mailbox.
    async fn send(&mut self, message: &OutgoingMessage) -> Result<SentMessage, TransportError>;

    /// Set the reversible flags of one message.
    async fn set_flags(&mut self, id: &MessageId, flags: FlagChange) -> Result<(), TransportError>;

    /// Move one message out of the folder it is in and into the
    /// archive.
    async fn move_to_archive(&mut self, id: &MessageId) -> Result<(), TransportError>;
}
