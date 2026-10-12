use super::*;
use std::sync::Arc;
use turn::auth::{AuthHandler, generate_auth_key};
use turn::relay::relay_static::RelayAddressGeneratorStatic;
use turn::server::{
    Server,
    config::{ConnConfig, ServerConfig},
};
use webrtc_util::vnet::net::Net;

struct Auth;
impl AuthHandler for Auth {
    fn auth_handle(
        &self,
        username: &str,
        realm: &str,
        _: SocketAddr,
    ) -> Result<Vec<u8>, turn::Error> {
        if username != "viewer" {
            return Err(turn::Error::ErrFakeErr);
        }
        Ok(generate_auth_key(username, realm, "secret"))
    }
}

fn pipeline() -> Pipeline {
    Pipeline::with_socket(UdpSocket::bind("127.0.0.1:0").unwrap())
}

fn request(sdp: String, servers: Vec<IceServer>) -> OfferRequest {
    OfferRequest {
        sdp,
        candidate: "127.0.0.1:7901".into(),
        relay: String::new(),
        token: "session".into(),
        ice_servers: servers,
        reply: mpsc::channel().0,
    }
}

#[test]
fn a_session_gathers_host_server_reflexive_and_relay_candidates() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (server, address) = runtime.block_on(async {
        let conn = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let address = conn.local_addr().unwrap();
        let server = Server::new(ServerConfig {
            conn_configs: vec![ConnConfig {
                conn,
                relay_addr_generator: Box::new(RelayAddressGeneratorStatic {
                    relay_address: "127.0.0.1".parse().unwrap(),
                    address: "127.0.0.1".into(),
                    net: Arc::new(Net::new(None)),
                }),
            }],
            realm: "pagis".into(),
            auth_handler: Arc::new(Auth),
            channel_bind_timeout: Duration::from_secs(600),
            alloc_close_notify: None,
        })
        .await
        .unwrap();
        (server, address)
    });
    // The STUN server models an address mapping outside the host interface.
    let stun_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let stun_address = stun_socket.local_addr().unwrap();
    let binding = std::thread::spawn(move || {
        let mut request = [0; 2048];
        let (_, peer) = stun_socket.recv_from(&mut request).unwrap();
        let mut response = request[..20].to_vec();
        response[..4].copy_from_slice(&[1, 1, 0, 12]);
        response.extend_from_slice(&[0, 32, 0, 8, 0, 1]);
        response.extend_from_slice(&(peer.port() ^ 0x2112).to_be_bytes());
        response
            .extend_from_slice(&(u32::from_be_bytes([127, 0, 0, 2]) ^ 0x2112a442).to_be_bytes());
        stun_socket.send_to(&response, peer).unwrap();
    });
    let mut pipeline = pipeline();
    let offer = pipeline
        .make_offer(&request(
            String::new(),
            vec![IceServer {
                urls: vec![
                    format!("stun:{stun_address}"),
                    format!("turn:{address}?transport=udp"),
                ],
                username: "viewer".into(),
                credential: "secret".into(),
            }],
        ))
        .unwrap();
    assert!(!offer.contains("a=ice-lite"));
    for kind in ["host", "srflx", "relay"] {
        assert!(
            offer.contains(&format!(" typ {kind}")),
            "missing {kind}: {offer}"
        );
    }
    binding.join().unwrap();
    runtime.block_on(server.close()).unwrap();
}

#[test]
fn a_pipeline_offer_connects_to_a_peer_that_answers() {
    let mut pipeline = pipeline();
    let peer_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    peer_socket.set_nonblocking(true).unwrap();
    let peer_address = peer_socket.local_addr().unwrap();
    let mut peer = Rtc::builder().clear_codecs().enable_h264(true).build();
    peer.add_local_candidate(Candidate::host(peer_address, Protocol::Udp).unwrap());
    let offer = pipeline
        .make_offer(&request(String::new(), vec![]))
        .unwrap();
    let answer = peer
        .sdp_api()
        .accept_offer(SdpOffer::from_sdp_string(&offer).unwrap())
        .unwrap();
    pipeline
        .accept_answer("session", &answer.to_sdp_string())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut connected = false;
    while Instant::now() < deadline {
        pipeline.drive();
        peer.handle_input(Input::Timeout(Instant::now())).unwrap();
        let mut buffer = [0; 2048];
        while let Ok((n, source)) = peer_socket.recv_from(&mut buffer) {
            if let Ok(contents) = (&buffer[..n]).try_into() {
                peer.handle_input(Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: peer_address,
                        contents,
                    },
                ))
                .unwrap();
            }
        }
        loop {
            match peer.poll_output().unwrap() {
                Output::Transmit(t) => {
                    peer_socket.send_to(&t.contents, t.destination).unwrap();
                }
                Output::Event(Event::Connected) => connected = true,
                Output::Timeout(_) => break,
                _ => {}
            }
        }
        if connected && pipeline.viewers[0].connected {
            assert!(
                pipeline.viewers[0].mid.is_some(),
                "the offer has no video writer"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the pipeline offer did not connect to its answering peer");
}

#[test]
fn a_turn_candidate_carries_the_screen_and_viewer_input() {
    use webrtc_util::Conn;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (server, address) = runtime.block_on(async {
        let conn = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let address = conn.local_addr().unwrap();
        let server = Server::new(ServerConfig {
            conn_configs: vec![ConnConfig {
                conn,
                relay_addr_generator: Box::new(RelayAddressGeneratorStatic {
                    relay_address: "127.0.0.1".parse().unwrap(),
                    address: "127.0.0.1".into(),
                    net: Arc::new(Net::new(None)),
                }),
            }],
            realm: "pagis".into(),
            auth_handler: Arc::new(Auth),
            channel_bind_timeout: Duration::from_secs(600),
            alloc_close_notify: None,
        })
        .await
        .unwrap();
        (server, address)
    });
    let (client, browser) = runtime.block_on(async {
        let client = turn::client::Client::new(turn::client::ClientConfig {
            stun_serv_addr: address.to_string(),
            turn_serv_addr: address.to_string(),
            username: "viewer".into(),
            password: "secret".into(),
            realm: String::new(),
            software: String::new(),
            rto_in_ms: 200,
            conn: Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap()),
            vnet: None,
        })
        .await
        .unwrap();
        client.listen().await.unwrap();
        let browser = client.allocate().await.unwrap();
        (client, browser)
    });
    let browser_address = browser.local_addr().unwrap();
    let mut peer = Rtc::builder().clear_codecs().enable_h264(true).build();
    peer.add_local_candidate(Candidate::host(browser_address, Protocol::Udp).unwrap());
    let mut api = peer.sdp_api();
    api.add_media(
        MediaKind::Video,
        str0m::media::Direction::RecvOnly,
        None,
        None,
        None,
    );
    api.add_channel("input".into());
    let (offer, pending) = api.apply().unwrap();
    let mut pipeline = pipeline();
    let answer = pipeline
        .accept_offer(&request(
            offer.to_sdp_string(),
            vec![IceServer {
                urls: vec![format!("turn:{address}")],
                username: "viewer".into(),
                credential: "secret".into(),
            }],
        ))
        .unwrap();
    let relayed = pipeline.viewers[0].transports[0].relayed.unwrap();
    // The browser accepts only the relay path. Direct checks do not reach it.
    let answer = answer
        .lines()
        .filter(|l| !l.starts_with("a=candidate:") || l.contains(" typ relay "))
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n";
    peer.sdp_api()
        .accept_answer(pending, SdpAnswer::from_sdp_string(&answer).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut frame = false;
    let mut input = false;
    let mut channel = None;
    while Instant::now() < deadline {
        input |= !pipeline.drive().is_empty();
        pipeline.send_frame(&[128; 32 * 32 * 3], 32, 32);
        peer.handle_input(Input::Timeout(Instant::now())).unwrap();
        let mut buffer = [0; 2048];
        if let Ok(Ok((n, source))) = runtime.block_on(async {
            tokio::time::timeout(Duration::from_millis(2), browser.recv_from(&mut buffer)).await
        }) {
            if source == relayed {
                peer.handle_input(Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: browser_address,
                        contents: (&buffer[..n]).try_into().unwrap(),
                    },
                ))
                .unwrap();
            }
        }
        loop {
            match peer.poll_output().unwrap() {
                Output::Transmit(t) => {
                    runtime
                        .block_on(browser.send_to(&t.contents, t.destination))
                        .unwrap();
                }
                Output::Event(Event::MediaData(data)) => frame |= !data.data.is_empty(),
                Output::Event(Event::ChannelOpen(id, _)) => channel = Some(id),
                Output::Timeout(_) => break,
                _ => {}
            }
        }
        if let Some(id) = channel {
            peer.channel(id)
                .unwrap()
                .write(true, br#"{"op":"move","x":1,"y":2}"#)
                .unwrap();
        }
        if frame && input {
            break;
        }
    }
    assert!(frame, "the screen does not reach the browser through TURN");
    assert!(
        input,
        "viewer input does not reach the pipeline through TURN"
    );
    pipeline.close("session");
    assert!(pipeline.viewers.is_empty());
    runtime.block_on(browser.close()).unwrap();
    runtime.block_on(client.close()).unwrap();
    runtime.block_on(server.close()).unwrap();
}
