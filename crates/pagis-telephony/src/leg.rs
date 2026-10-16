//! The leg of one call at the transport (ADR-0020): what a transport
//! hands the media hub once a call exists, in either direction. The leg
//! is the seam between signaling and media on one side and the hub on
//! the other, so the fake and Telnyx feed the same hub.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc;

use crate::audio::{Codec, Frame};

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
    /// No inbound audio for the timeout after the answer.
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
/// and audio share one stream, so a typed event keeps its place among
/// the audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegEvent {
    Rtp(RtpPacket),
    Frame(Frame),
    /// One typed press from a frame carrier. It contains no audio.
    Dtmf(char),
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

/// The half of a frame leg the hub writes to. Frames have no RTP header.
#[async_trait]
pub trait FrameSink: Send + Sync {
    async fn send(&self, frame: Frame);

    /// End the Call. The leg answers with [`LegEvent::Ended`].
    async fn hangup(&self);
}

/// Where the hub sends audio. Only a packet leg has RTP metadata.
pub enum LegSink {
    Packet {
        sink: Arc<dyn PacketSink>,
        /// The SSRC the transport advertises for Pagis's stream.
        ssrc: u32,
        /// The accepted telephone-event payload type, or none.
        dtmf_payload_type: Option<u8>,
    },
    Frame(Arc<dyn FrameSink>),
    /// A frame carrier that supplies typed inbound keypad events.
    TypedFrame(Arc<dyn FrameSink>),
}

impl LegSink {
    pub fn dtmf_payload_type(&self) -> Option<u8> {
        match self {
            Self::Packet {
                dtmf_payload_type, ..
            } => *dtmf_payload_type,
            Self::Frame(_) | Self::TypedFrame(_) => None,
        }
    }

    pub async fn hangup(&self) {
        match self {
            Self::Packet { sink, .. } => sink.hangup().await,
            Self::Frame(sink) | Self::TypedFrame(sink) => sink.hangup().await,
        }
    }
}

/// One call's leg, ready for a hub.
pub struct MediaLeg {
    /// The codec the answer settled on.
    pub codec: Codec,
    pub inbound: mpsc::Receiver<LegEvent>,
    pub sink: LegSink,
}

/// The carrier side of a Call with frames and typed inbound keypad events.
/// Dropping `inbound` ends the Call with `TransportLost`.
pub struct FrameLeg {
    pub inbound: mpsc::Sender<LegEvent>,
    pub outbound: mpsc::Receiver<Frame>,
}

impl FrameLeg {
    /// Create media for the hub and bounded channels for the carrier.
    /// The carrier sends `Answered` before it sends audio or keypad events.
    pub fn pair(codec: Codec, capacity: usize) -> (MediaLeg, Self) {
        let (inbound, events) = mpsc::channel(capacity);
        let (answer, outbound) = mpsc::channel(capacity);
        let (hangup, mut hung_up) = tokio::sync::oneshot::channel();
        let (feed, mut frames) = mpsc::channel(capacity);
        // Keep no inbound sender in the sink, so a lost carrier closes
        // the stream. A separate signal ends a local hang-up.
        pagis_rt::spawn(async move {
            loop {
                tokio::select! {
                    event = frames.recv() => {
                        let Some(event) = event else { return };
                        if inbound.send(event).await.is_err() { return; }
                    }
                    _ = &mut hung_up => {
                        let _ = inbound.send(LegEvent::Ended(EndedReason::LocalHangup)).await;
                        return;
                    }
                }
            }
        });
        (
            MediaLeg {
                codec,
                inbound: events,
                sink: LegSink::TypedFrame(Arc::new(ChannelFrames {
                    answer,
                    hangup: std::sync::Mutex::new(Some(hangup)),
                })),
            },
            Self {
                inbound: feed,
                outbound,
            },
        )
    }
}

struct ChannelFrames {
    answer: mpsc::Sender<Frame>,
    hangup: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[async_trait]
impl FrameSink for ChannelFrames {
    async fn send(&self, frame: Frame) {
        // A slow carrier drops a frame instead of stopping the pacer.
        let _ = self.answer.try_send(frame);
    }

    async fn hangup(&self) {
        if let Some(hangup) = self.hangup.lock().expect("lock").take() {
            let _ = hangup.send(());
        }
    }
}
