//! The live screen in a real browser on another machine, through the TURN
//! server of Remote Access (ADR-0028).
//!
//! Headless Chrome opens a screen session as `LiveScreen` does, with the
//! ICE server that the screen route gives a browser through the Funnel and
//! with relay candidates alone, so each packet of the session crosses the
//! TURN server. Chrome reaches the TURN port of the daemon over plain TCP,
//! as `tailscaled` forwards it once it has ended TLS on port 8443. A path
//! of the Media Relay carries the session on to a pipeline of the shape
//! screend runs: ice-lite, with the relay's address as its one candidate.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pagis_computer::{DaemonRelay, IceCredentials, MediaForwarder, MediaRelay};
use pagis_server::FunnelPort;
use pagis_testkit::browser::Browser;
use pagis_testkit::{FakeTailscale, TestDaemon, TestDaemonOptions};
use str0m::change::SdpOffer;
use str0m::channel::ChannelId;
use str0m::net::Protocol;
use str0m::{Candidate, Event, Rtc};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::remote_access::{
    TAILNET_ORIGIN, ice_servers, in_remote_access, member, through_the_funnel,
};
use crate::screen_relay::Peer;

/// A UDP port of loopback that nothing holds now, for the one-port range
/// of the Media Relay of the test.
async fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("a loopback UDP socket")
        .local_addr()
        .expect("an address")
        .port()
}

#[tokio::test]
async fn a_browser_on_another_machine_opens_a_screen_session_through_the_turn_server() {
    // The Media Relay of the session, and the daemon that names its
    // address and its range to the TURN server.
    let port = free_udp_port().await;
    let relay = DaemonRelay::new(MediaForwarder::new("127.0.0.1".to_string(), port..=port));
    let path = relay
        .open(CancellationToken::new())
        .await
        .expect("the Media Relay opens a path");
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        screen: pagis_server::ScreenRelay {
            relay: pagis_server::MediaRelayKind::Daemon,
            advertise_ip: "127.0.0.1".to_string(),
            media_ports: port..=port,
        },
        ..in_remote_access(Arc::new(FakeTailscale::ready(
            FunnelPort::Pagis,
            FunnelPort::Pagis,
        )))
    })
    .await;
    assert_eq!(daemon.public_origin, TAILNET_ORIGIN);

    // The ICE server of a browser through the Funnel. The Funnel ends
    // TLS on port 8443 and forwards plain TCP to the TURN port, so Chrome
    // opens that port with `turn:` over TCP.
    let grace = member(&daemon, "grace@example.com", "a good password").await;
    let cookie = daemon.cookie_for(&grace.id).await;
    let ice = ice_servers(
        through_the_funnel(
            reqwest::Method::GET,
            format!("{}/api/v1/screen/ice", daemon.base_url),
        )
        .header("cookie", &cookie),
    )
    .await;
    assert_eq!(
        ice[0]["urls"][0],
        "turns:owner-mac.tail1234.ts.net:8443?transport=tcp"
    );
    let server = serde_json::json!({
        "urls": format!("turn:{}?transport=tcp", daemon.remote_access_turn_addr),
        "username": ice[0]["username"],
        "credential": ice[0]["credential"],
    });

    let browser = Browser::launch().await;
    let tab = browser.signed_in(&daemon).await;
    let offer: String = tab
        .evaluate(&format!(
            "async () => {{
                const pc = new RTCPeerConnection({{ iceServers: [{server}], iceTransportPolicy: 'relay' }})
                const channel = pc.createDataChannel('input')
                window.screenSession = {{ pc, channel, received: [] }}
                channel.onmessage = (event) => window.screenSession.received.push(event.data)
                await pc.setLocalDescription(await pc.createOffer())
                return pc.localDescription.sdp
            }}"
        ))
        .await;

    // The pipeline answers as screend does, from its own socket, and
    // registers with the path of the Media Relay.
    let advertised: SocketAddr = path.candidate.parse().expect("an ip:port candidate");
    let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
    socket
        .send_to(&path.registration(), ("127.0.0.1", path.port))
        .await
        .expect("the registration goes out");
    let mut rtc = Rtc::builder().set_ice_lite(true).build();
    rtc.add_local_candidate(Candidate::host(advertised, Protocol::Udp).expect("a candidate"));
    let answer = rtc
        .sdp_api()
        .accept_offer(SdpOffer::from_sdp_string(&offer).expect("Chrome's offer parses"))
        .expect("the pipeline answers")
        .to_sdp_string();
    path.authenticate(IceCredentials::of_answer(&answer).expect("the answer has credentials"));
    let mut pipeline = Peer {
        rtc,
        socket,
        local: advertised,
    };
    let answered: bool = tab
        .evaluate(&format!(
            "async () => {{
                await window.screenSession.pc.setRemoteDescription({{ type: 'answer', sdp: {} }})
                return true
            }}",
            serde_json::to_string(&answer).expect("the answer as a string")
        ))
        .await;
    assert!(answered);

    // ICE, DTLS and SCTP cross the TURN server, and the input channel
    // opens on both sides.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut channel: Option<ChannelId> = None;
    let mut from_the_viewer: Vec<Vec<u8>> = Vec::new();
    let mut sent = false;
    loop {
        for event in pipeline.step().await {
            match event {
                Event::ChannelOpen(id, _) => channel = Some(id),
                Event::ChannelData(data) => from_the_viewer.push(data.data),
                _ => {}
            }
        }
        let state: serde_json::Value = tab
            .evaluate(
                "() => ({ ice: window.screenSession.pc.iceConnectionState, \
                   channel: window.screenSession.channel.readyState, \
                   received: window.screenSession.received })",
            )
            .await;
        if let (Some(id), "open") = (channel, state["channel"].as_str().unwrap_or_default()) {
            if !sent {
                let _: bool = tab
                    .evaluate(
                        "() => { window.screenSession.channel.send('a click of the viewer'); \
                           return true }",
                    )
                    .await;
                pipeline
                    .rtc
                    .channel(id)
                    .expect("the open channel")
                    .write(false, b"a frame of the pipeline")
                    .expect("the pipeline writes");
                sent = true;
            }
            let viewer_got = state["received"]
                .as_array()
                .is_some_and(|received| received.iter().any(|m| m == "a frame of the pipeline"));
            if viewer_got
                && from_the_viewer
                    .iter()
                    .any(|m| m == b"a click of the viewer")
            {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the session never opened through the TURN server: {state}, pipeline channel \
             {channel:?}, from the viewer {from_the_viewer:?}"
        );
    }

    // The pair that carries the session is on Chrome's TURN port, which
    // it allocated over TCP. The pipeline answers each check with the
    // address that it sees, the Media Relay's, so Chrome can learn its
    // side of the pair as peer-reflexive; it is still a candidate of the
    // TURN port.
    let pair: serde_json::Value = tab
        .evaluate(
            "async () => {
                const stats = [...(await window.screenSession.pc.getStats()).values()]
                const transport = stats.find((report) => report.type === 'transport')
                const pair = stats.find((report) => report.id === transport.selectedCandidatePairId)
                const local = stats.find((report) => report.id === pair.localCandidateId)
                return { type: local.candidateType, relayProtocol: local.relayProtocol }
            }",
        )
        .await;
    assert!(pair["type"] == "relay" || pair["type"] == "prflx", "{pair}");
    assert_eq!(pair["relayProtocol"], "tcp", "{pair}");
}
