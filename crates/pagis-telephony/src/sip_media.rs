//! The RTP and SRTP leg of a SIP call (ADR-0020). This is the one place
//! `rustrtc` appears. A `PeerConnection` in its SDES-SRTP mode writes the
//! offer and the answer: PCMU and PCMA only, `ptime=20`, RTCP mux,
//! `RTP/SAVP` with `a=crypto`, no ICE and no DTLS. There is no
//! cleartext path.
//!
//! The leg hands the hub packets as they arrive and sends the hub's
//! packets on, converting between the laws only when the answer picked
//! the other one.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rsipstack::dialog::dialog::{DialogState, DialogStateReceiver, TerminatedReason};
use rsipstack::dialog::dialog_layer::DialogLayer;
use rsipstack::dialog::invite_dialog::InviteDialog;
use rsipstack::sip::StatusCode;
use rsipstack::transaction::key::TransactionRole;
use rustrtc::media::sample_track;
use rustrtc::peer_connection::RtpObserver;
use rustrtc::sdp::CryptoAttribute;
use rustrtc::{
    AudioCapability, MediaCapabilities, PeerConnection, PeerConnectionState,
    RtcConfigurationBuilder, RtcpMuxPolicy, RtpCodecParameters, SdpType, SessionDescription,
    TransportMode,
};
use tokio::sync::mpsc;

use crate::audio::{Codec, FRAME_DURATION, Frame};
use crate::leg::{EndedReason, LegEvent, MediaLeg, PacketSink, RtpPacket};
use crate::transport::{TransportError, TransportErrorCode};

/// The telephone-event payload type the offer carries. The far side may
/// answer with another number; the sink maps it.
pub const DTMF_PAYLOAD_TYPE: u8 = 101;
/// The codec the hub speaks when the answer has not settled one yet.
const PREFERRED: Codec = Codec::Pcmu;
/// How many inbound events may wait for the hub before packets drop.
const INBOUND_DEPTH: usize = 64;

/// What the far side's SDP settled: the codec Pagis sends, and the
/// payload type of a telephone event, when it accepted one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiated {
    pub codec: Codec,
    pub dtmf_payload_type: Option<u8>,
}

/// Whether the remote SDP keys its audio with `a=crypto`, so SRTP can
/// come up. A carrier with encrypted media off answers without it, and
/// then no packet in either direction can be read (ADR-0020).
pub fn carries_crypto(sdp: &SessionDescription) -> bool {
    sdp.first_audio_section()
        .is_some_and(|audio| !audio.get_crypto_attributes().is_empty())
}

/// What the far side must do for Pagis to enable SRTP. The line reaches
/// the log when a call fails for the lack of it.
pub const CLEARTEXT_ADVICE: &str = "the far side offered cleartext media, which Pagis never \
     speaks; the carrier's SIP Connection must have encrypted media (SRTP) on";

/// Read what a remote SDP accepts. The first of PCMU and PCMA in the
/// audio line is the codec; a `telephone-event` rtpmap gives the event
/// payload type. `None` when the audio line has neither law.
pub fn negotiated_from_sdp(sdp: &SessionDescription) -> Option<Negotiated> {
    let audio = sdp.first_audio_section()?;
    let codec = audio
        .formats
        .iter()
        .filter_map(|format| format.parse::<u8>().ok())
        .find_map(Codec::from_payload_type)?;
    let dtmf_payload_type = audio.attributes.iter().find_map(|attribute| {
        if attribute.key != "rtpmap" {
            return None;
        }
        let value = attribute.value.as_deref()?;
        let (payload_type, encoding) = value.split_once(' ')?;
        encoding
            .to_ascii_lowercase()
            .starts_with("telephone-event/8000")
            .then(|| payload_type.parse().ok())
            .flatten()
    });
    Some(Negotiated {
        codec,
        dtmf_payload_type,
    })
}

/// The media half of one SIP call.
pub struct SipMedia {
    pc: PeerConnection,
    ssrc: u32,
}

impl SipMedia {
    /// A fresh media session with nothing negotiated. The socket binds
    /// on the first description.
    pub fn new() -> Self {
        let config = RtcConfigurationBuilder::new()
            .transport_mode(TransportMode::Srtp)
            .rtcp_mux_policy(RtcpMuxPolicy::Require)
            // Telnyx corrects the RTP address when the SDP carries a
            // private one; the leg follows the first packets it gets.
            .enable_latching(true)
            .media_capabilities(MediaCapabilities {
                audio: vec![
                    AudioCapability::pcmu(),
                    AudioCapability::pcma(),
                    AudioCapability::telephone_event(),
                ],
                video: Vec::new(),
                application: None,
                image: Vec::new(),
            })
            .build();
        let pc = PeerConnection::new(config);
        // The track is never fed: the hub writes whole packets. It
        // exists so the description carries one sending audio line.
        let (_source, track, _feedback) = sample_track(rustrtc::media::MediaKind::Audio, 1);
        let sender = pc
            .add_track(
                track,
                RtpCodecParameters {
                    payload_type: PREFERRED.payload_type(),
                    name: PREFERRED.name().to_string(),
                    clock_rate: crate::audio::SAMPLE_RATE,
                    channels: 1,
                },
            )
            .expect("a fresh connection takes a track");
        Self {
            pc,
            ssrc: sender.ssrc(),
        }
    }

    /// The offer for a call Pagis places.
    pub async fn offer(&self) -> Result<String, TransportError> {
        let mut offer = self.pc.create_offer().await.map_err(media_failed)?;
        strip_crypto_session_params(&mut offer);
        add_ptime(&mut offer);
        let sdp = offer.to_sdp_string();
        self.pc.set_local_description(offer).map_err(media_failed)?;
        Ok(sdp)
    }

    /// The answer to a call that arrived, or `None` when the offer has
    /// no law Pagis speaks, or no `a=crypto` to key SRTP with.
    pub async fn answer(
        &self,
        offer: &str,
    ) -> Result<Option<(String, Negotiated)>, TransportError> {
        let mut offer = SessionDescription::parse(SdpType::Offer, offer).map_err(|error| {
            tracing::warn!(%error, "the offer did not parse");
            TransportError(TransportErrorCode::Refused)
        })?;
        let Some(negotiated) = negotiated_from_sdp(&offer) else {
            tracing::warn!("the offer carries no law Pagis speaks");
            return Ok(None);
        };
        let Some(tag) = keep_one_crypto(&mut offer) else {
            tracing::warn!("{CLEARTEXT_ADVICE}");
            return Ok(None);
        };
        // rustrtc keys SDES on the transport start that the remote
        // description begins, and reads both descriptions to do it. On
        // the answered side the local answer does not exist yet, and on
        // a multi-thread runtime the start wins the race: the session
        // fails with "Missing crypto attributes for SDES" and no packet
        // passes in either direction. So the offer goes in without its
        // media address first, which starts nothing, the answer is set,
        // and then the offer goes in whole and starts the transport with
        // both descriptions in place.
        let mut unaddressed = offer.clone();
        unaddressed.session.connection = None;
        for section in &mut unaddressed.media_sections {
            section.connection = None;
        }
        self.pc
            .set_remote_description(unaddressed)
            .await
            .map_err(media_failed)?;
        let mut answer = self.pc.create_answer().await.map_err(media_failed)?;
        strip_crypto_session_params(&mut answer);
        echo_crypto_tag(&mut answer, tag);
        add_ptime(&mut answer);
        let sdp = answer.to_sdp_string();
        self.pc
            .set_local_description(answer)
            .map_err(media_failed)?;
        self.pc
            .set_remote_description(offer)
            .await
            .map_err(media_failed)?;
        Ok(Some((sdp, negotiated)))
    }

    /// The far side's answer to Pagis's offer.
    pub async fn apply_answer(&self, sdp: &str) -> Result<Negotiated, TransportError> {
        let answer = SessionDescription::parse(SdpType::Answer, sdp).map_err(|error| {
            tracing::warn!(%error, "the answer did not parse");
            TransportError(TransportErrorCode::Refused)
        })?;
        let negotiated = negotiated_from_sdp(&answer).ok_or_else(|| {
            tracing::warn!("the answer carries no law Pagis speaks");
            TransportError(TransportErrorCode::Refused)
        })?;
        if !carries_crypto(&answer) {
            tracing::warn!("{CLEARTEXT_ADVICE}");
            return Err(TransportError(TransportErrorCode::Refused));
        }
        self.pc
            .set_remote_description(answer)
            .await
            .map_err(media_failed)?;
        Ok(negotiated)
    }

    /// Why the media session failed, once it has: the far side's
    /// packets cannot be read and Pagis's cannot be sent.
    pub fn failure(&self) -> Option<String> {
        (*self.pc.subscribe_peer_state().borrow() == PeerConnectionState::Failed).then(|| {
            self.pc
                .disconnect_reason()
                .map_or_else(|| "unknown".to_string(), |reason| format!("{reason:?}"))
        })
    }

    /// The local description as the wire sees it, for tests.
    pub fn local_sdp(&self) -> Option<String> {
        self.pc
            .local_description()
            .map(|description| description.to_sdp_string())
    }
}

impl Default for SipMedia {
    fn default() -> Self {
        Self::new()
    }
}

fn media_failed(error: rustrtc::RtcError) -> TransportError {
    tracing::warn!(%error, "the media session failed");
    TransportError(TransportErrorCode::Refused)
}

/// The suite Pagis keys with, and the one it offers.
const CRYPTO_SUITE: &str = "AES_CM_128_HMAC_SHA1_80";

/// Keep the one crypto line of the offer that names Pagis's suite, and
/// answer its tag. A carrier offers many suites, AEAD ones first, and
/// rustrtc keys from the first remote line it sees, so the offer it
/// sees carries only the line Pagis answers. `None` when no line names
/// the suite, which is the cleartext case for the caller.
fn keep_one_crypto(offer: &mut SessionDescription) -> Option<u16> {
    let audio = offer
        .media_sections
        .iter_mut()
        .find(|section| section.kind == rustrtc::MediaKind::Audio)?;
    let chosen = audio
        .get_crypto_attributes()
        .into_iter()
        .find(|crypto| crypto.crypto_suite == CRYPTO_SUITE)?;
    audio.attributes.retain(|attribute| {
        attribute.key != "crypto"
            || attribute
                .value
                .as_deref()
                .and_then(CryptoAttribute::parse)
                .is_some_and(|crypto| crypto.tag == chosen.tag)
    });
    Some(chosen.tag)
}

/// The answer's crypto line carries the tag of the offer line it
/// answers (RFC 4568). rustrtc writes tag 1 whatever the offer said.
fn echo_crypto_tag(answer: &mut SessionDescription, tag: u16) {
    for section in &mut answer.media_sections {
        for attribute in &mut section.attributes {
            if attribute.key != "crypto" {
                continue;
            }
            if let Some(value) = &attribute.value
                && let Some((_, rest)) = value.split_once(' ')
            {
                attribute.value = Some(format!("{tag} {rest}"));
            }
        }
    }
}

/// The crypto line down to its key. rustrtc writes a lifetime and an
/// MKI (`|2^31|1:1`) after the key, but no packet it sends carries an
/// MKI, and a carrier that trusts the line refuses the call.
fn strip_crypto_session_params(description: &mut SessionDescription) {
    for section in &mut description.media_sections {
        for attribute in &mut section.attributes {
            if attribute.key != "crypto" {
                continue;
            }
            let Some(value) = &attribute.value else {
                continue;
            };
            let mut parts = value.split_whitespace();
            let (Some(tag), Some(suite), Some(key)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let key = key.split('|').next().unwrap_or(key);
            attribute.value = Some(format!("{tag} {suite} {key}"));
        }
    }
}

/// `a=ptime:20` on the audio line: one frame per packet.
fn add_ptime(description: &mut SessionDescription) {
    for section in &mut description.media_sections {
        if section.kind == rustrtc::MediaKind::Audio {
            section.attributes.push(rustrtc::Attribute::new(
                "ptime",
                Some(FRAME_DURATION.as_millis().to_string()),
            ));
        }
    }
}

/// One SIP call's leg: the dialog, the media, and the stream the hub
/// reads.
pub struct SipLeg {
    media: Arc<SipMedia>,
    dialog: InviteDialog,
    dialog_layer: Arc<DialogLayer>,
    feed: mpsc::Sender<LegEvent>,
    negotiated: Mutex<Option<Negotiated>>,
    /// The first packet that was not sent is logged; the rest are 20 ms
    /// apart and say the same thing.
    send_failed: AtomicBool,
}

impl SipLeg {
    /// Start the leg. The dialog's states become the typed events; the
    /// packets that arrive become `Rtp` events; `invite` is the client
    /// transaction of a call Pagis placed, whose failure ends the leg.
    pub fn start(
        media: SipMedia,
        dialog: InviteDialog,
        dialog_layer: Arc<DialogLayer>,
        states: DialogStateReceiver,
        negotiated: Option<Negotiated>,
        invite: Option<tokio::task::JoinHandle<rsipstack::dialog::invitation::InviteAsyncResult>>,
    ) -> MediaLeg {
        let (feed, inbound) = mpsc::channel(INBOUND_DEPTH);
        let media = Arc::new(media);
        media
            .pc
            .add_observer(Arc::new(Ingress { feed: feed.clone() }));
        let ssrc = media.ssrc;
        let leg = Arc::new(Self {
            media,
            dialog,
            dialog_layer,
            feed,
            negotiated: Mutex::new(negotiated),
            send_failed: AtomicBool::new(false),
        });
        tokio::spawn(Arc::clone(&leg).follow(states, invite));
        MediaLeg {
            codec: negotiated.map_or(PREFERRED, |negotiated| negotiated.codec),
            dtmf_payload_type: Some(DTMF_PAYLOAD_TYPE),
            ssrc,
            inbound,
            sink: leg,
        }
    }

    async fn follow(
        self: Arc<Self>,
        mut states: DialogStateReceiver,
        invite: Option<tokio::task::JoinHandle<rsipstack::dialog::invitation::InviteAsyncResult>>,
    ) {
        let mut invite = invite;
        let mut applied: Option<Vec<u8>> = None;
        let mut peer_state = self.media.pc.subscribe_peer_state();
        loop {
            let state = tokio::select! {
                state = states.recv() => state,
                changed = peer_state.changed() => {
                    if changed.is_err() {
                        continue;
                    }
                    if *peer_state.borrow_and_update() != PeerConnectionState::Failed {
                        continue;
                    }
                    let reason = self
                        .media
                        .pc
                        .disconnect_reason()
                        .map_or_else(|| "unknown".to_string(), |reason| format!("{reason:?}"));
                    tracing::warn!(%reason, "the media session failed; ending the call");
                    self.end(EndedReason::MediaFailed).await;
                    self.hang_up_dialog().await;
                    return;
                }
                result = wait_invite(&mut invite) => {
                    invite = None;
                    match result {
                        Ok(()) => continue,
                        Err(error) => {
                            tracing::warn!(%error, "the INVITE did not complete");
                            self.end(EndedReason::TransportLost).await;
                            return;
                        }
                    }
                }
            };
            let Some(state) = state else {
                self.end(EndedReason::TransportLost).await;
                return;
            };
            match state {
                DialogState::Early(_, response) => {
                    if self.dials() && !self.apply_remote(&response.body, &mut applied).await {
                        return;
                    }
                    self.emit(LegEvent::Ringing).await;
                }
                DialogState::Confirmed(_, response) => {
                    if self.dials() && !self.apply_remote(&response.body, &mut applied).await {
                        return;
                    }
                    self.emit(LegEvent::Answered).await;
                }
                DialogState::Terminated(_, reason) => {
                    let ended = ended_reason(&reason);
                    if ended == EndedReason::Refused {
                        tracing::warn!(?reason, "the carrier refused the call");
                    }
                    self.end(ended).await;
                    return;
                }
                DialogState::Updated(_, _, handle)
                | DialogState::Info(_, _, handle)
                | DialogState::Options(_, _, handle)
                | DialogState::Notify(_, _, handle)
                | DialogState::Message(_, _, handle)
                | DialogState::Refer(_, _, handle) => {
                    // Pagis reads digits from the RTP leg and changes no
                    // session: an in-dialog request gets a plain OK.
                    let _ = handle.reply(StatusCode::OK).await;
                }
                DialogState::Calling(_)
                | DialogState::Trying(_)
                | DialogState::WaitAck(_, _)
                | DialogState::Publish(_, _, _) => {}
            }
        }
    }

    /// Whether Pagis placed this call. On a call it answered, the far
    /// side's SDP was the offer, applied at the answer; the body a
    /// later dialog state carries is Pagis's own `200 OK`, and it must
    /// never go back into the media session as a remote answer.
    fn dials(&self) -> bool {
        self.dialog.role() == TransactionRole::Client
    }

    /// The far side's SDP, once per distinct body: a `183` and the `200`
    /// after it usually carry the same one. An SDP Pagis cannot run
    /// media on ends the leg with `media_failed`, and returns `false`.
    async fn apply_remote(&self, body: &[u8], applied: &mut Option<Vec<u8>>) -> bool {
        if body.is_empty() || applied.as_deref() == Some(body) {
            return true;
        }
        let sdp = String::from_utf8_lossy(body);
        match self.media.apply_answer(&sdp).await {
            Ok(negotiated) => {
                *applied = Some(body.to_vec());
                *self.negotiated.lock().expect("lock") = Some(negotiated);
                true
            }
            Err(error) => {
                tracing::warn!(%error, "the far side's SDP was not applied; ending the call");
                self.end(EndedReason::MediaFailed).await;
                self.hang_up_dialog().await;
                false
            }
        }
    }

    /// End the dialog: `CANCEL` or `603` while it is early, `BYE` once
    /// it is confirmed.
    async fn hang_up_dialog(&self) {
        let early = matches!(
            self.dialog.state(),
            DialogState::Calling(_) | DialogState::Trying(_) | DialogState::Early(_, _)
        );
        let outcome = if early && self.dialog.role() == TransactionRole::Server {
            self.dialog.reject(Some(StatusCode::Decline), None)
        } else {
            self.dialog.hangup().await
        };
        if let Err(error) = outcome {
            tracing::warn!(%error, "hanging up failed");
        }
    }

    async fn emit(&self, event: LegEvent) {
        // The hub is gone when this fails, and then nothing listens.
        let _ = self.feed.send(event).await;
    }

    async fn end(&self, reason: EndedReason) {
        self.emit(LegEvent::Ended(reason)).await;
        self.media.pc.close();
        self.dialog_layer.remove_dialog(&self.dialog.id());
    }
}

async fn wait_invite(
    invite: &mut Option<tokio::task::JoinHandle<rsipstack::dialog::invitation::InviteAsyncResult>>,
) -> Result<(), String> {
    match invite {
        Some(handle) => match handle.await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(error.to_string()),
            Err(error) => Err(error.to_string()),
        },
        None => std::future::pending().await,
    }
}

#[async_trait]
impl PacketSink for SipLeg {
    async fn send(&self, packet: RtpPacket) {
        let negotiated = *self.negotiated.lock().expect("lock");
        let Some(negotiated) = negotiated else {
            // Nothing answered yet: there is nowhere to send to.
            return;
        };
        let (payload_type, payload) = if packet.payload_type == DTMF_PAYLOAD_TYPE {
            let Some(payload_type) = negotiated.dtmf_payload_type else {
                return;
            };
            (payload_type, packet.payload)
        } else {
            match Codec::from_payload_type(packet.payload_type) {
                Some(codec) if codec != negotiated.codec => (
                    negotiated.codec.payload_type(),
                    Frame::new(codec, packet.payload)
                        .into_codec(negotiated.codec)
                        .payload()
                        .clone(),
                ),
                _ => (negotiated.codec.payload_type(), packet.payload),
            }
        };
        let mut header = rustrtc::rtp::RtpHeader::new(
            payload_type,
            packet.sequence,
            packet.timestamp,
            packet.ssrc,
        );
        header.marker = packet.marker;
        let packet = rustrtc::rtp::RtpPacket {
            header,
            payload,
            padding_len: 0,
        };
        if let Err(error) = self.media.pc.send_raw_rtp(packet).await
            && !self.send_failed.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(%error, "an RTP packet was not sent; the far side hears nothing");
        }
    }

    async fn hangup(&self) {
        self.emit(LegEvent::Ended(EndedReason::LocalHangup)).await;
        self.hang_up_dialog().await;
    }
}

/// Every packet that arrives, straight to the hub. A hub that falls
/// more than [`INBOUND_DEPTH`] behind loses packets, not the socket.
struct Ingress {
    feed: mpsc::Sender<LegEvent>,
}

impl RtpObserver for Ingress {
    fn on_ingress(&self, packet: &rustrtc::rtp::RtpPacket, _source: SocketAddr) {
        let packet = RtpPacket {
            ssrc: packet.header.ssrc,
            payload_type: packet.header.payload_type,
            sequence: packet.header.sequence_number,
            timestamp: packet.header.timestamp,
            marker: packet.header.marker,
            payload: packet.payload.clone(),
        };
        if let Err(mpsc::error::TrySendError::Full(_)) = self.feed.try_send(LegEvent::Rtp(packet)) {
            tracing::trace!("the hub is behind; an inbound packet was dropped");
        }
    }
}

/// Why the dialog ended, as the Call record says it.
fn ended_reason(reason: &TerminatedReason) -> EndedReason {
    match reason {
        TerminatedReason::UacCancel | TerminatedReason::UasDecline => EndedReason::LocalHangup,
        TerminatedReason::UacBye | TerminatedReason::UasBye => EndedReason::RemoteHangup,
        TerminatedReason::UacBusy | TerminatedReason::UasBusy => EndedReason::Busy,
        TerminatedReason::Timeout => EndedReason::NoAnswer,
        TerminatedReason::UasOther(code) | TerminatedReason::UacOther(code) => match code {
            StatusCode::BusyHere | StatusCode::BusyEverywhere => EndedReason::Busy,
            StatusCode::RequestTimeout | StatusCode::TemporarilyUnavailable => {
                EndedReason::NoAnswer
            }
            _ => EndedReason::Refused,
        },
        TerminatedReason::ProxyError(_) | TerminatedReason::ProxyAuthRequired => {
            EndedReason::Refused
        }
    }
}
