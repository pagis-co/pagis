//! Mail: the Mailbox Provider seams and the hosts behind them.
//!
//! A Mailbox Provider is a Connection with two duties that have
//! different credentials and different failure modes, so it has two
//! seams (ADR-0019). [`MailboxHost`] creates, deletes and lists the
//! mailboxes of one domain with the host API key. [`MailTransport`]
//! reads over IMAP, waits with IDLE and sends over SMTP with the
//! mailbox's own password, so the API key stays out of the mail path.
//!
//! Two hosts sit behind the first seam: [`MigaduHost`], which makes a
//! mailbox in one call, and [`ManualHost`], where the user makes the
//! mailbox at any IMAP host and pastes its address and password.
//! [`fake`] holds a fake of each seam, for the tests of the daemon
//! above them.
//!
//! [`mailbox_provider`] reads the mail settings a Connection carries:
//! the domain, the two endpoints, and what the two seams declare.
//!
//! [`MailboxDesk`] is the one path that makes, proves, recovers,
//! sleeps and deletes an Agent Mailbox over those seams (ADR-0019).
//!
//! [`MailToolRuntime`] executes the `mail__*` tools in the Agent's own
//! mailbox, with the answer caps the tools carry (ADR-0019).
//!
//! [`MailMessageMatcher`] reads the typed filter of the one mail
//! Incoming Event kind, whichever mailbox the mail arrived in
//! (ADR-0019).
//!
//! [`verify_sender`] reads the Mailbox Provider's own
//! Authentication-Results header of an inbound mail. A sender's tier
//! holds only on the aligned DMARC pass it shows (ADR-0019).
//!
//! [`MailBlocks`] mints the `mail` block of an inbound mail that woke
//! the Agent and of a mail it sent (ADR-0019).

pub mod fake;

mod address;
mod blocks;
mod collector;
mod desk;
mod events;
mod host;
mod manual;
mod migadu;
mod mime;
mod no_transport;
mod proof;
mod protocol;
mod provider;
mod sender;
mod standing;
mod tools;
mod transport;

pub use address::{
    LocalPartError, MAX_LOCAL_PART, RESERVED_LOCAL_PARTS, address_domain, mailbox_address,
    suggest_local_part, validate_local_part, with_suffix,
};
pub use blocks::{MailBlockDeps, MailBlocks};
pub use collector::{
    IngestError, MailCollector, MailCollectorDeps, MailIngest, POLL_INTERVAL, SWEEP_INTERVAL,
};
pub use desk::{
    MailboxDeletion, MailboxDesk, MailboxDeskDeps, MailboxError, MailboxOffer, NewMailbox,
    ProofTiming, host_message,
};
pub use events::{
    ListedSenders, MAIL_MATCHER, MAIL_MESSAGE_RECEIVED, MailMessageMatcher, OWN_MAILBOX,
    SenderTrust, bare_address, connection_account, message_event, provider_event_id, sender_domain,
};
pub use host::{
    Deletion, HostAccount, HostCapabilities, HostError, HostErrorCode, HostedMailbox, MailboxHost,
    MailboxPassword,
};
pub use manual::ManualHost;
pub use migadu::MigaduHost;
pub use mime::{BuiltMessage, build_mime, read_mime};
pub use no_transport::NoMailTransport;
pub use proof::{
    Backoff, LOGIN_PROOF_WINDOW, RECONNECT_BACKOFF_MAX, RECONNECT_BACKOFF_START, prove_login,
};
pub use protocol::{
    IMAP_PORT, IMAPS_PORT, MAX_IDLE_TURN, SMTPS_PORT, SUBMISSION_PORT, StandardTransport,
};
pub use provider::{
    ACCOUNT_KEY, CAPABILITIES_KEY, DOMAIN_KEY, IMAP_HOST_KEY, IMAP_PORT_KEY, MAIL_TRANSPORT,
    MIGADU_IMAP, MIGADU_SMTP, MailboxCapabilities, MailboxProvider, SMTP_HOST_KEY, SMTP_PORT_KEY,
    authserv_ids, is_mailbox_provider, mailbox_provider,
};
pub use sender::{SenderEvidence, SenderVerification, verify_sender};
pub use standing::{
    STANDING_MAIL_INSTRUCTION, STANDING_MAIL_RULE_NAME, StandingMailRule, StandingRuleError,
};
pub use tools::{
    MAX_SUMMARIES, MAX_TEXT_BYTES, MAX_THREAD_MESSAGES, MailToolDeps, MailToolRuntime,
    ReservedSendError, STALE_ID, TRUNCATION_MARKER, parse_query,
};
pub use transport::{
    ARCHIVE, Attachment, Cursor, Endpoint, FlagChange, FolderState, INBOX, IdleOutcome, MailQuery,
    MailSession, MailTransport, MailboxCredential, Message, MessageId, MessageIdError,
    MessageSummary, OutgoingMessage, SentMessage, TransportCapabilities, TransportError,
    TransportErrorCode,
};

/// The provider of a Connection whose mailboxes the daemon makes
/// through the Migadu API (ADR-0019).
pub const MIGADU_PROVIDER: &str = "migadu";

/// The provider of a Connection whose mailboxes the user makes at the
/// host (ADR-0019).
pub const MANUAL_PROVIDER: &str = "manual";

/// The messages a day one mailbox may send, when the user states no
/// other number (ADR-0019).
pub const DEFAULT_OUTGOING_CAP: u32 = 20;

/// Where the secret store files one mail account's host API key. The
/// key never reaches the database, the model or a log line (ADR-0013).
///
/// The mail domain belongs to the installation and not to one Person,
/// so the name carries no Workspace: one Org owns one mail
/// domain and an Administrator alone configures it. The mailboxes on
/// that domain stay per Workspace, and
/// [`mailbox_password_secret_name`] keeps the Workspace for that
/// reason.
pub fn host_api_key_secret_name(alias: &str) -> String {
    format!("mail/{alias}/api_key")
}

/// Where the secret store files one mailbox's own password. It leaves
/// the store only to the host client and to the transport. The name
/// carries the Workspace, so two Workspaces that hold the same address
/// keep separate passwords.
pub fn mailbox_password_secret_name(
    workspace_id: &pagis_core::WorkspaceId,
    address: &str,
) -> String {
    pagis_core::workspace_secret_name(
        workspace_id,
        &format!("mailbox_password_{}", address.to_lowercase()),
    )
}
