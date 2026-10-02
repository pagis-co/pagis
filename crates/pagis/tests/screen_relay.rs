//! The Media Relay end to end (ADR-0014).
//!
//! A local installation configures nothing, so the default `[screen]`
//! section has to carry a screen on its own. This runs one real WebRTC
//! session over the relay that section builds: a browser of the shape
//! `LiveScreen` makes, a media pipeline of the shape screend runs, and
//! the relay between them. Neither peer holds the other's address —
//! the browser knows the relay's advertised address and the pipeline
//! knows only the relay path it registers with — which is the
//! Computer's position on its Tenant Network.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use pagis::Config;
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::format::Codec;
use str0m::media::{Direction, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};
use tokio::net::UdpSocket;

/// One WebRTC peer on a real UDP socket.
pub(crate) struct Peer {
    pub(crate) rtc: Rtc,
    pub(crate) socket: UdpSocket,
    /// The address this peer advertised, which is where it believes the
    /// packets it reads arrived. The pipeline advertises the relay's
    /// address, exactly as screend does.
    pub(crate) local: SocketAddr,
}

impl Peer {
    /// Send what the session produced, then take in one datagram or one
    /// timeout. Returns the events of this step.
    pub(crate) async fn step(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let timeout = loop {
            match self.rtc.poll_output().expect("the session polls") {
                Output::Timeout(timeout) => break timeout,
                Output::Transmit(transmit) => {
                    self.socket
                        .send_to(&transmit.contents, transmit.destination)
                        .await
                        .expect("the datagram goes out");
                }
                Output::Event(event) => events.push(event),
            }
        };
        let wait = timeout
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(20));
        let mut buffer = [0u8; 2000];
        match tokio::time::timeout(wait, self.socket.recv_from(&mut buffer)).await {
            Ok(Ok((read, source))) => {
                let input = Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: self.local,
                        contents: buffer[..read].try_into().expect("a UDP payload"),
                    },
                );
                self.rtc.handle_input(input).expect("the session reads");
            }
            _ => {
                self.rtc
                    .handle_input(Input::Timeout(Instant::now()))
                    .expect("the session ticks");
            }
        }
        events
    }
}

/// A local installation views a screen with no configuration.
/// The default `[screen]` section builds the `daemon` relay on
/// loopback, and a real session between a browser and a pipeline
/// establishes through it and carries a video frame.
#[tokio::test]
async fn the_default_configuration_carries_a_screen_session_through_the_relay() {
    let relay = Config::default()
        .screen
        .media_relay()
        .expect("the default screen section builds a relay");

    // The relay opens the path and says which address the browser sends
    // media to. The pipeline listens where a Computer's screend does: on
    // its own socket, published nowhere, and it registers with the path
    // from that socket. screend sends it to the Docker host; here
    // the relay is on this machine.
    let path = relay
        .open(tokio_util::sync::CancellationToken::new())
        .await
        .expect("a media path");
    let advertised: SocketAddr = path.candidate.parse().expect("an ip:port candidate");
    assert_eq!(advertised.ip().to_string(), "127.0.0.1");
    let pipeline_socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    pipeline_socket
        .send_to(&path.registration(), ("127.0.0.1", path.port))
        .await
        .expect("the registration goes out");
    // A local installation needs no ICE server: the browser reaches the
    // advertised address itself.
    assert!(relay.ice_servers().is_empty());

    // The browser: one recvonly video transceiver and the input data
    // channel, which is the offer `LiveScreen` makes.
    let browser_socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    let browser_addr = browser_socket.local_addr().expect("address");
    let mut browser = Rtc::new();
    browser.add_local_candidate(
        Candidate::host(browser_addr, Protocol::Udp).expect("a host candidate"),
    );
    let mut api = browser.sdp_api();
    api.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None);
    api.add_channel("input".to_string());
    let (offer, pending) = api.apply().expect("the offer builds");

    // The pipeline answers the way screend does: ice-lite, H264 only,
    // and one host candidate, which is the relay's address.
    let mut pipeline = Rtc::builder()
        .clear_codecs()
        .enable_h264(true)
        .set_ice_lite(true)
        .build();
    pipeline
        .add_local_candidate(Candidate::host(advertised, Protocol::Udp).expect("a host candidate"));
    let answer = pipeline
        .sdp_api()
        .accept_offer(SdpOffer::from_sdp_string(&offer.to_sdp_string()).expect("the offer parses"))
        .expect("the pipeline answers");
    // The daemon gives the path the ICE credentials of the answer, so
    // the relay takes the checks that the browser signs with them.
    path.authenticate(
        pagis_computer::IceCredentials::of_answer(&answer.to_sdp_string())
            .expect("the answer carries ICE credentials"),
    );
    browser
        .sdp_api()
        .accept_answer(
            pending,
            SdpAnswer::from_sdp_string(&answer.to_sdp_string()).expect("the answer parses"),
        )
        .expect("the browser takes the answer");

    let mut browser = Peer {
        rtc: browser,
        socket: browser_socket,
        local: browser_addr,
    };
    let mut pipeline = Peer {
        rtc: pipeline,
        socket: pipeline_socket,
        local: advertised,
    };

    // Both sides connect, which means ICE and DTLS crossed the relay in
    // both directions.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut browser_connected = false;
    let mut pipeline_connected = false;
    let mut mid: Option<Mid> = None;
    let mut frames = 0usize;
    loop {
        assert!(
            Instant::now() < deadline,
            "the session never carried a frame through the relay \
             (browser connected: {browser_connected}, pipeline connected: \
             {pipeline_connected}, video: {mid:?})"
        );
        for event in browser.step().await {
            match event {
                Event::Connected => browser_connected = true,
                Event::MediaData(_) => frames += 1,
                _ => {}
            }
        }
        for event in pipeline.step().await {
            match event {
                Event::Connected => pipeline_connected = true,
                Event::MediaAdded(media) if media.kind == MediaKind::Video => {
                    mid = Some(media.mid);
                }
                _ => {}
            }
        }
        if frames > 0 {
            break;
        }
        // One H264 intra frame, once the video is negotiated.
        if let (true, Some(mid)) = (pipeline_connected, mid) {
            write_frame(&mut pipeline.rtc, mid);
        }
    }

    assert!(browser_connected, "the browser never connected");
    assert!(pipeline_connected, "the pipeline never connected");
}

/// One Annex-B H264 frame out of the pipeline, the way screend writes
/// an encoded picture.
fn write_frame(rtc: &mut Rtc, mid: Mid) {
    let Some(writer) = rtc.writer(mid) else {
        return;
    };
    let Some(pt) = writer
        .payload_params()
        .find(|params| params.spec().codec == Codec::H264)
        .map(|params| params.pt())
    else {
        return;
    };
    let payload = vec![
        0, 0, 0, 1, 0x65, 0x88, 0x84, 0x00, 0x33, 0xff, 0xf0, 0x12, 0x34, 0x56,
    ];
    let _ = writer.write(pt, Instant::now(), MediaTime::from_micros(0), payload);
}
