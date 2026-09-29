//! The leg of one call at the transport (ADR-0020): what a transport
//! hands the media hub once a call exists, in either direction. The leg
//! is the seam between signaling and media on one side and the hub on
//! the other, so the fake and Telnyx feed the same hub.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc;

use crate::audio::Codec;

/// Why the audio stopped. Every Call ends with one (ADR-0020), and the
/// codes are stable: they land on the Call record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndedReason {
    /// The Remote Party hung up, or the carrier ended the call.
    RemoteHangup,
    /// Pagis hung up.
    LocalHangup,
    /// The number Pagis called was busy.
    Busy,
    /// Nobody answered before the carrier gave up.
    NoAnswer,
    /// The carrier refused the call.
    Refused,
    /// No inbound RTP for the timeout and no `BYE`.
    MediaTimeout,
    /// The media never started: the far side answered cleartext, or
    /// the SRTP session did not come up. The carrier's SIP Connection
    /// must have encrypted media on.
    MediaFailed,
    /// The leg went away without a word: the socket or the task died.
    TransportLost,
}

impl EndedReason {
    pub fn as_str(self) -> &'static str {
        match self {
            EndedReason::RemoteHangup => "remote_hangup",
            EndedReason::LocalHangup => "local_hangup",
            EndedReason::Busy => "busy",
            EndedReason::NoAnswer => "no_answer",
            EndedReason::Refused => "refused",
            EndedReason::MediaTimeout => "media_timeout",
            EndedReason::MediaFailed => "media_failed",
            EndedReason::TransportLost => "transport_lost",
        }
    }
}

/// One RTP packet, as the hub reads and writes it. The transport adds
/// the encryption; the hub owns the sequence, the clock and the SSRC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpPacket {
    pub ssrc: u32,
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub marker: bool,
    pub payload: Bytes,
}

/// What arrives from the far side, in the order it happened. Signaling
/// and packets share one stream, so a typed event keeps its place among
/// the audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegEvent {
    Rtp(RtpPacket),
    /// The far side rings (outbound only).
    Ringing,
    /// Media may flow in both directions.
    Answered,
    /// The far side ended the call, or the transport did on its behalf.
    Ended(EndedReason),
}

/// The half of a leg the hub writes to.
#[async_trait]
pub trait PacketSink: Send + Sync {
    /// Send one packet to the far side. A packet that cannot be sent is
    /// dropped and logged: the next one is 20 ms away.
    async fn send(&self, packet: RtpPacket);

    /// End the call from this side. The leg answers with
    /// [`LegEvent::Ended`] on its inbound stream.
    async fn hangup(&self);
}

/// One call's leg, ready for a hub.
pub struct MediaLeg {
    /// The codec the answer settled on.
    pub codec: Codec,
    /// The payload type the far side accepted for telephone events, or
    /// `None` when it accepted none: then no digit can be sent.
    pub dtmf_payload_type: Option<u8>,
    /// The SSRC the transport advertised for Pagis's own stream.
    pub ssrc: u32,
    pub inbound: mpsc::Receiver<LegEvent>,
    pub sink: Arc<dyn PacketSink>,
}
