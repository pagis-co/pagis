//! The call seam (ADR-0020): signaling and media. It uses the SIP
//! credential and never the REST API key, so the key stays out of the
//! call path. A [`CallTransport`] opens one [`Line`] for one SIP
//! credential, and the line carries every number of the carrier
//! Connection. The endpoint task drives `REGISTER` over it, places the
//! calls of each number over it, and answers the calls that arrive on
//! it by the number that was dialed.

use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::leg::MediaLeg;

/// The carrier's SIP credential: the username and password of a
/// credential SIP Connection, and the registrar it registers at. The
/// password has no accessor that prints it and no `Debug` that shows
/// it.
#[derive(Clone, PartialEq, Eq)]
pub struct SipCredential {
    username: String,
    password: String,
    domain: String,
}

impl SipCredential {
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
        domain: impl Into<String>,
    ) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
            domain: domain.into(),
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    /// The registrar, e.g. `sip.telnyx.com`.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// The one reader. Only a transport calls it.
    pub fn expose_password(&self) -> &str {
        &self.password
    }
}

impl std::fmt::Debug for SipCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SipCredential")
            .field("username", &self.username)
            .field("password", &"redacted")
            .field("domain", &self.domain)
            .finish()
    }
}

/// What a transport can and cannot do (ADR-0020). The seam states the
/// differences between carriers instead of hiding them: a capability
/// that is absent is absent (ADR-0005), not emulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportCapabilities {
    /// Whether DTMF digits can be sent to the Remote Party.
    pub send_dtmf: bool,
    /// Whether the transport classifies an answer as a machine. A
    /// pure-SIP path cannot.
    pub answering_machine_detection: bool,
    /// Whether audio wider than G.711 at 8 kHz can be negotiated.
    pub wideband_audio: bool,
    /// Whether the carrier must reach the daemon from outside. A
    /// transport that registers outward needs no public ingress.
    pub public_ingress_required: bool,
}

/// Why the carrier did not do what it was asked. The codes are stable,
/// because they reach the user and the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportErrorCode {
    /// The registrar refused the credential.
    Unauthorized,
    /// The registrar did not answer: DNS, the socket or a timeout.
    Unreachable,
    /// The registrar answered with a refusal that is not about the
    /// credential.
    Refused,
}

impl TransportErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportErrorCode::Unauthorized => "unauthorized",
            TransportErrorCode::Unreachable => "unreachable",
            TransportErrorCode::Refused => "refused",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the carrier did not complete this: {}", .0.as_str())]
pub struct TransportError(pub TransportErrorCode);

/// One call that arrived on a line. The endpoint task answers it or
/// turns it away; nothing else sees it.
pub struct IncomingCall {
    /// The number the Remote Party dialed, in E.164. `None` when the
    /// `INVITE` names no number, or names one that is not E.164.
    pub dialed_e164: Option<String>,
    pub from_e164: String,
    pub answer: Box<dyn Answer>,
}

/// Why a line turns an `INVITE` away before it answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The number is on a call.
    Busy,
    /// The dialed number is missing or is not E.164, or the
    /// installation does not hold it.
    NotFound,
    /// No Agent holds the dialed number, or the number matches more
    /// than one stored record.
    Unavailable,
}

impl Refusal {
    /// The SIP status the line answers with.
    pub fn status(self) -> u16 {
        match self {
            Refusal::Busy => 486,
            Refusal::NotFound => 404,
            Refusal::Unavailable => 480,
        }
    }
}

/// The two answers to an `INVITE`.
#[async_trait]
pub trait Answer: Send {
    /// `200 OK` with the answer SDP. Media may flow once it returns.
    async fn accept(self: Box<Self>) -> Result<MediaLeg, TransportError>;

    /// A final refusal with the status of the [`Refusal`]. No media
    /// flows and no dialog stays.
    async fn reject(self: Box<Self>, refusal: Refusal);
}

/// The open signaling socket of one carrier credential, and the calls
/// that arrive on it for every number of the carrier. The endpoint task
/// owns both: it registers over the line, refreshes over it, and drops
/// it when the registration is lost, so the next attempt opens a fresh
/// one.
pub struct Opened {
    pub line: Box<dyn Line>,
    pub incoming: mpsc::Receiver<IncomingCall>,
}

#[async_trait]
pub trait Line: Send + Sync {
    /// Send `REGISTER` with this expiry. A fresh registration and a
    /// refresh are the same message. Returns the expiry the registrar
    /// granted, which the task refreshes at half of.
    async fn register(&mut self, expires: Duration) -> Result<Duration, TransportError>;

    /// Send `REGISTER` with `Expires: 0`, so the registrar drops the
    /// binding now instead of at expiry.
    async fn unregister(&mut self) -> Result<(), TransportError>;

    /// Send `INVITE` from one Agent Phone Number to one E.164 number.
    /// Returns as soon as the offer is out; ringing, the answer and the
    /// end arrive on the leg.
    async fn dial(&mut self, from_e164: &str, to_e164: &str) -> Result<MediaLeg, TransportError>;
}

/// Signaling and media at the carrier (ADR-0020).
#[async_trait]
pub trait CallTransport: Send + Sync {
    /// What this transport declares. The broker and the bridge read it;
    /// nothing else decides what a call may do.
    fn capabilities(&self) -> TransportCapabilities;

    /// Open the signaling socket for one SIP credential. Nothing is
    /// sent yet.
    async fn open(&self, credential: &SipCredential) -> Result<Opened, TransportError>;
}
