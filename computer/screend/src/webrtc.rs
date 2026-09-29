//! The in-container WebRTC pipeline: per-viewer str0m sessions
//! (ice-lite, one advertised host candidate each) sharing one
//! damage-gated openh264 encoder over the capture session's frames.
//! The encoder exists only while a viewer is connected; a joining
//! viewer forces an intra frame; a gone viewer's session is dropped.
//!
//! No host port reaches the media socket (ADR-0014). Each viewer
//! session registers outbound with the daemon's Media Relay instead:
//! the pipeline sends the session's registration from its media socket
//! to the relay port the offer names, and repeats it while the session
//! lives. The relay learns the pipeline's address from the source of
//! that datagram, and the Docker NAT carries the relay's packets back
//! to this socket, on Linux and on a macOS Docker host alike.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use openh264::OpenH264API;
use openh264::encoder::{Encoder, EncoderConfig};
use openh264::formats::{RgbSliceU8, YUVBuffer};
use str0m::change::SdpOffer;
use str0m::format::Codec;
use str0m::media::{MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};

/// The in-container media UDP port. No host port is published for it:
/// every viewer session registers outbound with the relay.
pub const MEDIA_PORT: u16 = 7901;

/// The start of a registration datagram, which is the daemon's
/// `pagis_computer::REGISTRATION_PREFIX`. Its first byte is in the range
/// RFC 7983 leaves to no protocol, so the relay never reads STUN, DTLS
/// or RTP as a registration.
const REGISTRATION_PREFIX: &[u8] = b"pagis-register:";

/// How often a session repeats its registration. The first one can be
/// lost like any datagram, and the repeats hold the NAT mapping of the
/// Docker host open while the session lives.
const REGISTRATION_INTERVAL: Duration = Duration::from_secs(1);

/// One offer handed over from the control endpoint. `candidate`
/// is the externally advertised `ip:port` for the ICE host candidate;
/// `relay` is the `host:port` of the relay path this session registers
/// with, and `token` is the path's secret.
pub struct OfferRequest {
    pub sdp: String,
    pub candidate: String,
    pub relay: String,
    pub token: String,
    pub reply: mpsc::Sender<Result<String, String>>,
}

struct Viewer {
    rtc: Rtc,
    mid: Option<Mid>,
    advertised: SocketAddr,
    connected: bool,
    /// Where this session registers, and what it sends there.
    relay: SocketAddr,
    registration: Vec<u8>,
    registered_at: Option<Instant>,
}

pub struct Pipeline {
    socket: UdpSocket,
    viewers: Vec<Viewer>,
    encoder: Option<Encoder>,
    force_intra: bool,
    /// A frame must go out without waiting for damage: a viewer just
    /// connected to a possibly static screen.
    refresh: bool,
    t0: Instant,
}

impl Pipeline {
    pub fn new() -> Self {
        let socket =
            UdpSocket::bind(("0.0.0.0", MEDIA_PORT)).expect("bind media port");
        socket.set_nonblocking(true).expect("nonblocking media socket");
        Pipeline {
            socket,
            viewers: Vec::new(),
            encoder: None,
            force_intra: false,
            refresh: false,
            t0: Instant::now(),
        }
    }

    pub fn socket(&self) -> &UdpSocket {
        &self.socket
    }

    /// Answer one browser offer with a fresh ice-lite session.
    pub fn accept_offer(&mut self, request: &OfferRequest) -> Result<String, String> {
        let advertised: SocketAddr = request
            .candidate
            .parse()
            .map_err(|err| format!("bad candidate {:?}: {err}", request.candidate))?;
        let relay = request
            .relay
            .to_socket_addrs()
            .map_err(|err| format!("bad relay {:?}: {err}", request.relay))?
            .find(SocketAddr::is_ipv4)
            .ok_or_else(|| format!("the relay {:?} has no IPv4 address", request.relay))?;
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_h264(true)
            .set_ice_lite(true)
            .build();
        rtc.add_local_candidate(
            Candidate::host(advertised, Protocol::Udp)
                .map_err(|err| format!("bad host candidate: {err}"))?,
        );
        let offer = SdpOffer::from_sdp_string(&request.sdp)
            .map_err(|err| format!("bad offer: {err}"))?;
        let answer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|err| format!("offer refused: {err}"))?;
        self.viewers.push(Viewer {
            rtc,
            mid: None,
            advertised,
            connected: false,
            relay,
            registration: [REGISTRATION_PREFIX, request.token.as_bytes()].concat(),
            registered_at: None,
        });
        eprintln!("[screend] viewer joined ({} total)", self.viewers.len());
        Ok(answer.to_sdp_string())
    }

    /// Feed received UDP and timers to every session, transmit what
    /// they produce, and drop the dead ones. Call often. Returns the
    /// user input events viewers sent over their data channels,
    /// in arrival order; the caller enforces the input switch.
    pub fn drive(&mut self) -> Vec<crate::UserInput> {
        let mut inputs = Vec::new();
        let mut buf = [0u8; 2000];
        loop {
            let (n, source) = match self.socket.recv_from(&mut buf) {
                Ok(received) => received,
                Err(_) => break,
            };
            let datagram = &buf[..n];
            // str0m 0.9 parses STUN here, and its parser reads a fixed
            // number of bytes for a FINGERPRINT, an ERROR-CODE or an XOR
            // address attribute whatever the attribute length says, so a
            // short one at the end of a message panics it and ends
            // screend. The daemon's Media Relay drops datagrams from
            // strangers, but a viewer's own accepted address can still
            // send such a datagram. Drop a STUN message that is not well
            // formed before str0m reads it.
            if stun::is_message(datagram) && !stun::well_formed(datagram) {
                continue;
            }
            for viewer in &mut self.viewers {
                let Ok(contents) = datagram.try_into() else {
                    break;
                };
                let input = Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: viewer.advertised,
                        contents,
                    },
                );
                if viewer.rtc.accepts(&input) {
                    let _ = viewer.rtc.handle_input(input);
                    break;
                }
            }
        }

        let socket = &self.socket;
        for viewer in &mut self.viewers {
            let now = Instant::now();
            let due = viewer
                .registered_at
                .is_none_or(|at| now.duration_since(at) >= REGISTRATION_INTERVAL);
            if due {
                let _ = socket.send_to(&viewer.registration, viewer.relay);
                viewer.registered_at = Some(now);
            }
            let _ = viewer.rtc.handle_input(Input::Timeout(now));
            loop {
                match viewer.rtc.poll_output() {
                    Err(_) => break,
                    Ok(Output::Timeout(_)) => break,
                    Ok(Output::Transmit(transmit)) => {
                        let _ = socket.send_to(&transmit.contents, transmit.destination);
                    }
                    Ok(Output::Event(event)) => match event {
                        Event::Connected => {
                            viewer.connected = true;
                            self.force_intra = true;
                            self.refresh = true;
                        }
                        Event::MediaAdded(media) if media.kind == MediaKind::Video => {
                            viewer.mid = Some(media.mid);
                            self.refresh = true;
                        }
                        Event::ChannelData(data) => {
                            match serde_json::from_slice::<crate::UserInput>(&data.data) {
                                Ok(input) => inputs.push(input),
                                Err(err) => {
                                    eprintln!("[screend] bad data-channel input: {err}");
                                }
                            }
                        }
                        Event::KeyframeRequest(_) => self.force_intra = true,
                        Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                            viewer.rtc.disconnect();
                        }
                        _ => {}
                    },
                }
            }
        }

        let before = self.viewers.len();
        self.viewers.retain(|viewer| viewer.rtc.is_alive());
        if self.viewers.len() < before {
            eprintln!("[screend] viewer left ({} total)", self.viewers.len());
        }
        if self.viewers.is_empty() {
            // Encoder idle when no viewer.
            self.encoder = None;
            self.refresh = false;
        }
        inputs
    }

    /// A connected viewer with negotiated video exists: frames are
    /// worth encoding.
    fn wants_frame(&self) -> bool {
        self.viewers
            .iter()
            .any(|viewer| viewer.connected && viewer.mid.is_some())
    }

    /// A frame must be pushed from the stored capture, damage or not.
    pub fn needs_refresh(&self) -> bool {
        self.refresh && self.wants_frame()
    }

    /// Encode one captured frame (tightly packed RGB) and send it to
    /// every connected viewer.
    pub fn send_frame(&mut self, rgb: &[u8], width: u32, height: u32) {
        self.refresh = false;
        if !self.wants_frame() {
            return;
        }
        if self.encoder.is_none() {
            match Encoder::with_api_config(OpenH264API::from_source(), EncoderConfig::new()) {
                Ok(encoder) => {
                    self.encoder = Some(encoder);
                    self.force_intra = true;
                }
                Err(err) => {
                    eprintln!("[screend] encoder create failed: {err}");
                    return;
                }
            }
        }
        let encoder = self.encoder.as_mut().expect("encoder exists");
        if self.force_intra {
            let _ = encoder.force_intra_frame();
            self.force_intra = false;
        }
        let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(
            rgb,
            (width as usize, height as usize),
        ));
        let payload = match encoder.encode(&yuv) {
            Ok(bitstream) => bitstream.to_vec(),
            Err(err) => {
                eprintln!("[screend] encode failed: {err}");
                return;
            }
        };
        if payload.is_empty() {
            return;
        }
        let now = Instant::now();
        let media_time = MediaTime::from_micros(self.t0.elapsed().as_micros() as u64);
        for viewer in &mut self.viewers {
            if !viewer.connected {
                continue;
            }
            let Some(mid) = viewer.mid else { continue };
            let Some(writer) = viewer.rtc.writer(mid) else { continue };
            let Some(pt) = writer
                .payload_params()
                .find(|params| params.spec().codec == Codec::H264)
                .map(|params| params.pt())
            else {
                continue;
            };
            if let Err(err) = writer.write(pt, now, media_time, payload.clone()) {
                eprintln!("[screend] frame write failed: {err:?}");
            }
        }
    }
}

/// A STUN well-formedness check by RFC 8489. The Media Relay in the
/// daemon holds the same rule (`crates/pagis-computer/src/relay.rs`,
/// `Stun::well_formed` and `Stun::has_its_size`); screend is a separate
/// Cargo workspace that the image builds from `computer/screend` alone,
/// so it cannot use that crate and carries its own copy. The pipeline
/// checks a datagram before str0m reads it, because str0m 0.9 panics on
/// a short attribute at the end of a message.
mod stun {
    const HEADER: usize = 20;
    const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];

    /// Whether `datagram` is a STUN message (RFC 7983): its first byte is
    /// 0 to 3 and the magic cookie follows the length.
    pub fn is_message(datagram: &[u8]) -> bool {
        datagram.len() >= HEADER && datagram[0] <= 3 && datagram[4..8] == MAGIC_COOKIE
    }

    /// Whether a STUN message is well formed by RFC 8489: its length
    /// field gives the rest of the datagram in whole words (section 5),
    /// every attribute and its padding fit in the message (section 14),
    /// and every attribute of a fixed size has its size.
    pub fn well_formed(message: &[u8]) -> bool {
        let Some(mut attributes) = message.get(HEADER..) else {
            return false;
        };
        let length = usize::from(u16::from_be_bytes([message[2], message[3]]));
        if length != attributes.len() || length % 4 != 0 {
            return false;
        }
        while !attributes.is_empty() {
            let Some(header) = attributes.get(..4) else {
                return false;
            };
            let kind = u16::from_be_bytes([header[0], header[1]]);
            let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
            let Some(value) = attributes.get(4..4 + length) else {
                return false;
            };
            if !has_its_size(kind, value) {
                return false;
            }
            let Some(rest) = attributes.get(4 + length.next_multiple_of(4)..) else {
                return false;
            };
            attributes = rest;
        }
        true
    }

    /// Whether an attribute value has the size its RFC gives its type. An
    /// attribute of another type can have any size.
    fn has_its_size(kind: u16, value: &[u8]) -> bool {
        match kind {
            // MESSAGE-INTEGRITY (RFC 8489 section 14.5): an HMAC-SHA1.
            0x0008 => value.len() == 20,
            // ERROR-CODE (RFC 8489 section 14.8): four bytes of class and
            // number, then the reason phrase.
            0x0009 => value.len() >= 4,
            // XOR-PEER-ADDRESS and XOR-RELAYED-ADDRESS (RFC 8656 sections
            // 18.3 and 18.5), and XOR-MAPPED-ADDRESS (RFC 8489 section
            // 14.2): eight bytes for the IPv4 family (0x01), and twenty
            // for the IPv6 family (0x02).
            0x0012 | 0x0016 | 0x0020 => matches!(
                (value.len(), value.get(1)),
                (8, Some(0x01)) | (20, Some(0x02))
            ),
            // FINGERPRINT (RFC 8489 section 14.7): a CRC-32.
            0x8028 => value.len() == 4,
            _ => true,
        }
    }
}

#[cfg(test)]
mod stun_tests {
    use super::stun;
    use str0m::ice::StunMessage;

    /// The sample request of RFC 5769 section 2.1: a Binding request that
    /// another STUN agent signed, with SOFTWARE, PRIORITY, ICE-CONTROLLED,
    /// USERNAME "evtj:h6vY", MESSAGE-INTEGRITY and a FINGERPRINT after it.
    const RFC_5769_REQUEST: [u8; 108] = [
        0x00, 0x01, 0x00, 0x58, 0x21, 0x12, 0xa4, 0x42, 0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6,
        0x86, 0xfa, 0x87, 0xdf, 0xae, 0x80, 0x22, 0x00, 0x10, 0x53, 0x54, 0x55, 0x4e, 0x20, 0x74,
        0x65, 0x73, 0x74, 0x20, 0x63, 0x6c, 0x69, 0x65, 0x6e, 0x74, 0x00, 0x24, 0x00, 0x04, 0x6e,
        0x00, 0x01, 0xff, 0x80, 0x29, 0x00, 0x08, 0x93, 0x2f, 0xf9, 0xb1, 0x51, 0x26, 0x3b, 0x36,
        0x00, 0x06, 0x00, 0x09, 0x65, 0x76, 0x74, 0x6a, 0x3a, 0x68, 0x36, 0x76, 0x59, 0x20, 0x20,
        0x20, 0x00, 0x08, 0x00, 0x14, 0x9a, 0xea, 0xa7, 0x0c, 0xbf, 0xd8, 0xcb, 0x56, 0x78, 0x1e,
        0xf2, 0xb5, 0xb2, 0xd3, 0xf2, 0x49, 0xc1, 0xb5, 0x71, 0xa2, 0x80, 0x28, 0x00, 0x04, 0xe5,
        0x7a, 0x3b, 0xcf,
    ];

    /// The RFC 5769 request cut before its MESSAGE-INTEGRITY, with
    /// `attribute` in the place the signature starts. The length field is
    /// set to the new message length.
    fn check_ending_with(attribute: &[u8]) -> Vec<u8> {
        let mut message = [&RFC_5769_REQUEST[..76], attribute].concat();
        let length = u16::try_from(message.len() - 20).expect("a short message");
        message[2..4].copy_from_slice(&length.to_be_bytes());
        message
    }

    /// Whether str0m's parser panics on `message`. A malformed message is
    /// one that only the well-formedness check keeps away from str0m.
    fn str0m_panics_on(message: &[u8]) -> bool {
        std::panic::catch_unwind(|| StunMessage::parse(message)).is_err()
    }

    /// A FINGERPRINT must be four bytes (RFC 8489 section 14.7). The
    /// signed request with an empty FINGERPRINT in place of its own
    /// panics str0m, and the check drops it before str0m reads it.
    #[test]
    fn a_short_fingerprint_is_dropped_before_str0m_reads_it() {
        let mut message = [&RFC_5769_REQUEST[..100], &[0x80, 0x28, 0x00, 0x00][..]].concat();
        message[2..4].copy_from_slice(&84u16.to_be_bytes());

        assert!(str0m_panics_on(&message), "str0m reads this message without a panic");
        assert!(stun::is_message(&message));
        assert!(!stun::well_formed(&message));
    }

    /// An ERROR-CODE holds at least its four bytes of class and number
    /// (RFC 8489 section 14.8).
    #[test]
    fn a_short_error_code_is_dropped_before_str0m_reads_it() {
        let message = check_ending_with(&[0x00, 0x09, 0x00, 0x00]);

        assert!(str0m_panics_on(&message), "str0m reads this message without a panic");
        assert!(!stun::well_formed(&message));
    }

    /// An XOR-MAPPED-ADDRESS is eight bytes for the IPv4 family and
    /// twenty for IPv6 (RFC 8489 section 14.2).
    #[test]
    fn a_short_xor_mapped_address_is_dropped_before_str0m_reads_it() {
        for message in [
            // Four bytes that name the IPv6 family.
            check_ending_with(&[0x00, 0x20, 0x00, 0x04, 0x00, 0x02, 0x12, 0x34]),
            // The eight bytes of IPv4 that name the IPv6 family.
            check_ending_with(&[
                0x00, 0x20, 0x00, 0x08, 0x00, 0x02, 0x12, 0x34, 0x01, 0x02, 0x03, 0x04,
            ]),
        ] {
            assert!(str0m_panics_on(&message), "str0m reads this message without a panic");
            assert!(!stun::well_formed(&message));
        }
    }

    /// Each attribute and its padding fit in the message (RFC 8489
    /// section 14). The request whose USERNAME claims 200 bytes is not
    /// well formed.
    #[test]
    fn an_attribute_that_runs_past_the_message_is_dropped() {
        let mut message = RFC_5769_REQUEST;
        // The length of the USERNAME attribute.
        message[62..64].copy_from_slice(&200u16.to_be_bytes());

        assert!(!stun::well_formed(&message));
    }

    /// A signed check that another agent wrote is well formed, and str0m
    /// reads it without a panic.
    #[test]
    fn a_well_formed_signed_check_passes() {
        assert!(stun::is_message(&RFC_5769_REQUEST));
        assert!(stun::well_formed(&RFC_5769_REQUEST));
        assert!(!str0m_panics_on(&RFC_5769_REQUEST));
    }

    /// A media packet and a registration datagram are not STUN, so the
    /// check leaves them alone.
    #[test]
    fn a_media_or_registration_datagram_is_not_a_stun_message() {
        assert!(!stun::is_message(b"\x80\x60a video frame"));
        assert!(!stun::is_message(b"pagis-register:sometoken"));
    }
}
