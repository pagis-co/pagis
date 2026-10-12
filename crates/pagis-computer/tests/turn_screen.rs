//! The real Computer sends its screen through coturn with no daemon forwarder.

use pagis_computer::{ComputerRuntime, MediaRelay, TEST_LABEL, TurnRelay, TurnServer};
use std::net::{Ipv4Addr, SocketAddr};
use std::process::Command;
use std::time::{Duration, Instant};
use str0m::{
    Candidate, Event, Input, Output, Rtc,
    change::{SdpAnswer, SdpOffer},
    media::{Direction, MediaKind},
    net::{Protocol, Receive},
};
use tokio_util::sync::CancellationToken;

/// The official coturn image, pinned for both test architectures.
const COTURN: &str = "ghcr.io/coturn/coturn:4.6.3@sha256:71c3c990283385567f11794ee692e3a47b66fd9b0bb39e42afbe776e331dd888";

#[tokio::test]
#[ignore = "needs Docker; run with cargo nextest run --run-ignored only"]
async fn coturn_carries_the_screen_without_a_daemon_packet() {
    let real = crate::docker_real::Real::new();
    let owner = real.owner(&pagis_core::AgentId::generate());
    let computer = real
        .runtime
        .start(&owner, &[], &pagis_computer::container_env("UTC", None))
        .await
        .unwrap();
    let network = pagis_computer::network_name(&real.workspace_id);
    let range = crate::media_relay::ports(12).await;
    let port = *range.start();
    let first = port + 1;
    let last = *range.end();
    let started = Command::new("docker")
        .args(["run", "--detach", "--label"])
        .arg(format!("{TEST_LABEL}={}", real.docker.mark()))
        .args(["--network", &network])
        .args(["--entrypoint", "turnserver"])
        .arg("--publish")
        .arg(format!("{port}:{port}/tcp"))
        .args([
            COTURN,
            "-n",
            "--verbose",
            "--no-cli",
            "--no-tls",
            "--no-dtls",
            "--no-multicast-peers",
            "--allow-loopback-peers",
            "--realm=pagis",
            "--use-auth-secret",
            "--static-auth-secret=screen-test",
            "--log-file=stdout",
        ])
        .arg(format!("--listening-port={port}"))
        .arg("--listening-ip=0.0.0.0")
        .arg(format!("--min-port={first}"))
        .arg(format!("--max-port={last}"))
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let container = String::from_utf8_lossy(&started.stdout).trim().to_string();
    let inspected = Command::new("docker")
        .args(["inspect", "--format"])
        .arg(format!(
            "{{{{ (index .NetworkSettings.Networks \"{network}\").IPAddress }}}}"
        ))
        .arg(&container)
        .output()
        .unwrap();
    assert!(
        inspected.status.success(),
        "{}",
        String::from_utf8_lossy(&inspected.stderr)
    );
    let host: Ipv4Addr = String::from_utf8_lossy(&inspected.stdout)
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let logs = Command::new("docker")
            .args(["logs", &container])
            .output()
            .unwrap();
        let text = format!(
            "{} {}",
            String::from_utf8_lossy(&logs.stdout),
            String::from_utf8_lossy(&logs.stderr)
        );
        if text.contains("UDP listener opened") && text.contains("TCP listener opened") {
            break;
        }
        assert!(Instant::now() < deadline, "coturn does not listen: {text}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // A daemon media socket observes every packet sent to the old path.
    let daemon = tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .unwrap();
    let relay = TurnRelay::new(TurnServer {
        urls: vec![
            format!("turn:{host}:{port}?transport=udp"),
            format!("turn:{host}:{port}?transport=tcp"),
        ],
        secret: "screen-test".into(),
        ttl: Duration::from_secs(600),
    });
    let path = relay.open(CancellationToken::new()).await.unwrap();
    assert_eq!(path.port, 0);
    let credential = &path.ice_servers[0];
    let browser = pagis_computer::fake::TurnClient::allocate(
        SocketAddr::from(([127, 0, 0, 1], port)),
        &credential.username,
        &credential.credential,
    )
    .await
    .unwrap_or_else(|error| {
        let logs = Command::new("docker")
            .args(["logs", &container])
            .output()
            .unwrap();
        panic!(
            "{error}: {} {}",
            String::from_utf8_lossy(&logs.stdout),
            String::from_utf8_lossy(&logs.stderr)
        );
    });
    let browser_address = browser.relayed_address();
    let mut peer = Rtc::builder().clear_codecs().enable_h264(true).build();
    peer.add_local_candidate(Candidate::host(browser_address, Protocol::Udp).unwrap());
    let mut api = peer.sdp_api();
    api.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None);
    api.add_channel("input".into());
    let (offer, pending) = api.apply().unwrap();
    let answer = real
        .runtime
        .relay_offer(&computer, &offer.to_sdp_string(), &path)
        .await
        .unwrap();
    assert!(!answer.contains("a=ice-lite"));
    assert!(
        answer
            .lines()
            .filter(|line| line.starts_with("a=candidate:") && line.contains(" typ relay "))
            .count()
            >= 2,
        "UDP and TCP allocations: {answer}"
    );
    // Only the Computer's relay candidate reaches this viewer.
    let answer = answer
        .lines()
        .filter(|line| !line.starts_with("a=candidate:") || line.contains(" typ relay "))
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n";
    let pipeline_address: SocketAddr = answer
        .lines()
        .find_map(|line| {
            if !line.starts_with("a=candidate:") {
                return None;
            }
            let fields: Vec<_> = line.split_whitespace().collect();
            Some(format!("{}:{}", fields[4], fields[5]).parse().unwrap())
        })
        .unwrap();
    peer.sdp_api()
        .accept_answer(pending, SdpAnswer::from_sdp_string(&answer).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut frame = false;
    while Instant::now() < deadline && !frame {
        peer.handle_input(Input::Timeout(Instant::now())).unwrap();
        loop {
            match peer.poll_output().unwrap() {
                Output::Transmit(packet) => {
                    browser
                        .send_to(&packet.contents, packet.destination)
                        .await
                        .unwrap();
                }
                Output::Event(Event::MediaData(data)) => frame |= !data.data.is_empty(),
                Output::Timeout(_) => break,
                _ => {}
            }
        }
        if let Ok(Ok((data, source))) =
            tokio::time::timeout(Duration::from_millis(5), browser.recv_from()).await
            && source == pipeline_address
        {
            peer.handle_input(Input::Receive(
                Instant::now(),
                Receive {
                    proto: Protocol::Udp,
                    source,
                    destination: browser_address,
                    contents: data.as_slice().try_into().unwrap(),
                },
            ))
            .unwrap();
        }
    }
    assert!(frame, "the browser receives no screen frame through coturn");
    let mut buffer = [0; 2048];
    assert!(
        tokio::time::timeout(Duration::from_millis(100), daemon.recv_from(&mut buffer))
            .await
            .is_err(),
        "the daemon carries a screen packet"
    );
    real.runtime
        .close_media(&computer, &path.token)
        .await
        .unwrap();
    browser.hang_up().await;
    real.runtime.stop(&owner).await.unwrap();
    path.close().await;
}

/// Full ICE also carries the real screen through the default daemon relay.
#[tokio::test]
#[ignore = "needs Docker; run with cargo nextest run --run-ignored only"]
async fn the_full_ice_pipeline_keeps_the_daemon_relay() {
    daemon_screen(false).await;
}

/// The control port makes an offer and accepts the viewer's answer with a token.
#[tokio::test]
#[ignore = "needs Docker; run with cargo nextest run --run-ignored only"]
async fn the_control_port_offers_to_a_viewer_that_answers() {
    daemon_screen(true).await;
}

async fn daemon_screen(pipeline_offers: bool) {
    let real = crate::docker_real::Real::new();
    let owner = real.owner(&pagis_core::AgentId::generate());
    let computer = real
        .runtime
        .start(&owner, &[], &pagis_computer::container_env("UTC", None))
        .await
        .unwrap();
    let relay = pagis_computer::DaemonRelay::new(pagis_computer::MediaForwarder::new(
        "127.0.0.1".into(),
        0..=0,
    ));
    let path = relay.open(CancellationToken::new()).await.unwrap();
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let local = socket.local_addr().unwrap();
    let mut peer = Rtc::builder().clear_codecs().enable_h264(true).build();
    peer.add_local_candidate(Candidate::host(local, Protocol::Udp).unwrap());
    if pipeline_offers {
        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{}/offer", computer.control_addr))
            .bearer_auth(&computer.token)
            .json(&serde_json::json!({
                "candidate": path.candidate, "relay": format!("host.docker.internal:{}", path.port),
                "token": path.token, "ice_servers": [],
            }))
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap()
        );
        let body: serde_json::Value = response.json().await.unwrap();
        let offer = body["sdp"].as_str().unwrap();
        assert!(!offer.contains("a=ice-lite"));
        path.authenticate(pagis_computer::IceCredentials::of_answer(offer).unwrap());
        let answer = peer
            .sdp_api()
            .accept_offer(SdpOffer::from_sdp_string(offer).unwrap())
            .unwrap();
        let body = serde_json::json!({ "token": path.token, "sdp": answer.to_sdp_string() });
        let url = format!("http://{}/answer", computer.control_addr);
        assert_eq!(
            client.post(&url).json(&body).send().await.unwrap().status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
        let response = client
            .post(&url)
            .bearer_auth(&computer.token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap()
        );
    } else {
        let mut api = peer.sdp_api();
        api.add_media(MediaKind::Video, Direction::RecvOnly, None, None, None);
        api.add_channel("input".into());
        let (offer, pending) = api.apply().unwrap();
        let answer = real
            .runtime
            .relay_offer(&computer, &offer.to_sdp_string(), &path)
            .await
            .unwrap();
        assert!(!answer.contains("a=ice-lite"));
        path.authenticate(pagis_computer::IceCredentials::of_answer(&answer).unwrap());
        peer.sdp_api()
            .accept_answer(pending, SdpAnswer::from_sdp_string(&answer).unwrap())
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut frame = false;
    let mut buffer = [0; 2048];
    while Instant::now() < deadline && !frame {
        peer.handle_input(Input::Timeout(Instant::now())).unwrap();
        loop {
            match peer.poll_output().unwrap() {
                Output::Transmit(packet) => {
                    socket
                        .send_to(&packet.contents, packet.destination)
                        .await
                        .unwrap();
                }
                Output::Event(Event::MediaData(data)) => frame |= !data.data.is_empty(),
                Output::Timeout(_) => break,
                _ => {}
            }
        }
        if let Ok(Ok((n, source))) =
            tokio::time::timeout(Duration::from_millis(5), socket.recv_from(&mut buffer)).await
        {
            peer.handle_input(Input::Receive(
                Instant::now(),
                Receive {
                    proto: Protocol::Udp,
                    source,
                    destination: local,
                    contents: (&buffer[..n]).try_into().unwrap(),
                },
            ))
            .unwrap();
        }
    }
    assert!(
        frame,
        "the full ICE pipeline sends no screen frame through the daemon relay"
    );
    real.runtime
        .close_media(&computer, &path.token)
        .await
        .unwrap();
    real.runtime.stop(&owner).await.unwrap();
    path.close().await;
}
