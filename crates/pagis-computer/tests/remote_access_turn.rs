//! The TURN server of Remote Access (ADR-0028) over real sockets: a TURN
//! client over TCP, as `tailscaled` forwards a browser to it once it has
//! ended TLS, the Media Relay's UDP range, and the rest of this machine.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use pagis_computer::fake::{TurnClient, ice_check};
use pagis_computer::{
    DaemonRelay, IceCredentials, MediaForwarder, MediaRelay, MediaRelayPeers, RemoteAccessTurn,
};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_util::sync::CancellationToken;

/// The URL that the ICE route names for a browser through the Funnel.
const URL: &str = "turns:owner-mac.tail1234.ts.net:8443?transport=tcp";

/// A TURN server on a loopback port of its own, which relays to `peers`.
async fn turn_server(peers: MediaRelayPeers) -> RemoteAccessTurn {
    turn_server_waiting(peers, Duration::from_secs(10)).await
}

async fn turn_server_waiting(
    peers: MediaRelayPeers,
    allocate_within: Duration,
) -> RemoteAccessTurn {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("a loopback TCP port");
    RemoteAccessTurn::start_waiting(listener, peers, CancellationToken::new(), allocate_within)
        .expect("the TURN server starts")
}

/// A socket that stands in for the Media Relay, and the peers of a server
/// that relays to it: its address, and a range of its one port.
async fn media_relay() -> (UdpSocket, MediaRelayPeers) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("a loopback UDP socket");
    let port = socket.local_addr().unwrap().port();
    (
        socket,
        MediaRelayPeers::new(Ipv4Addr::LOCALHOST.into(), port..=port),
    )
}

/// A client that allocated with a credential the server minted, as the
/// ICE route gives one to a browser.
async fn client_of(turn: &RemoteAccessTurn) -> TurnClient {
    let ice = turn.ice_server(URL.to_string());
    assert_eq!(ice.urls, [URL]);
    TurnClient::allocate(turn.address(), &ice.username, &ice.credential)
        .await
        .expect("the client allocates")
}

/// Read one datagram, or fail the test rather than hang it.
async fn read(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0u8; 2048];
    let (read, from) = tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buffer))
        .await
        .expect("a datagram arrived before the deadline")
        .expect("the socket read succeeded");
    (buffer[..read].to_vec(), from)
}

async fn received(client: &TurnClient) -> (Vec<u8>, SocketAddr) {
    tokio::time::timeout(Duration::from_secs(5), client.recv_from())
        .await
        .expect("a datagram arrived before the deadline")
        .expect("the relay read succeeded")
}

/// Whether a datagram arrives within a short wait.
async fn reads_nothing(socket: &UdpSocket) -> bool {
    let mut buffer = [0u8; 2048];
    tokio::time::timeout(Duration::from_millis(300), socket.recv_from(&mut buffer))
        .await
        .is_err()
}

/// A client over TCP allocates with a minted credential and relays
/// datagrams to the Media Relay and back. The sizes are not whole words,
/// so each ChannelData message on the TCP connection carries padding in
/// both directions, once the client binds a channel (RFC 8656 section
/// 12.5).
#[tokio::test]
async fn a_client_over_tcp_relays_to_the_media_relay_and_back() {
    let (media, peers) = media_relay().await;
    let media_address = media.local_addr().unwrap();
    let turn = turn_server(peers).await;
    let client = client_of(&turn).await;
    assert_eq!(client.relayed_address().ip(), Ipv4Addr::LOCALHOST);

    for size in 1..=40usize {
        let datagram = vec![size as u8; size];
        client.send_to(&datagram, media_address).await.unwrap();

        let (arrived, from) = read(&media).await;
        assert_eq!(arrived, datagram, "to the Media Relay, {size} bytes");
        assert_eq!(from, client.relayed_address());

        let answer = vec![!(size as u8); size + 1];
        media.send_to(&answer, from).await.unwrap();

        let (answered, peer) = received(&client).await;
        assert_eq!(answered, answer, "from the Media Relay, {} bytes", size + 1);
        assert_eq!(peer, media_address);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The server reaches nothing but the Media Relay: a datagram to another
/// loopback port does not leave it, and a datagram from another loopback
/// port does not reach the client, although the client holds a
/// permission for the address of both.
#[tokio::test]
async fn the_relay_reaches_nothing_but_the_media_relay() {
    let (media, peers) = media_relay().await;
    let media_address = media.local_addr().unwrap();
    let other = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let other_address = other.local_addr().unwrap();
    let turn = turn_server(peers).await;
    let client = client_of(&turn).await;

    client
        .send_to(b"to another port of loopback", other_address)
        .await
        .unwrap();
    client
        .send_to(b"to the Media Relay", media_address)
        .await
        .unwrap();

    assert_eq!(read(&media).await.0, b"to the Media Relay");
    assert!(reads_nothing(&other).await);

    other
        .send_to(b"from another port", client.relayed_address())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    media
        .send_to(b"from the Media Relay", client.relayed_address())
        .await
        .unwrap();

    let (first, from) = received(&client).await;
    assert_eq!(first, b"from the Media Relay");
    assert_eq!(from, media_address);
}

/// A credential of another server, such as one minted before a restart,
/// allocates nothing.
#[tokio::test]
async fn a_credential_of_another_server_allocates_nothing() {
    let (_media, peers) = media_relay().await;
    let turn = turn_server(peers.clone()).await;
    let other = turn_server(peers).await;
    let theirs = other.ice_server(URL.to_string());

    let refused = TurnClient::allocate(turn.address(), &theirs.username, &theirs.credential).await;

    assert!(refused.is_err());
}

/// A browser on another machine watches a Computer through the server:
/// its signed ICE check crosses the TURN server and a path of the Media
/// Relay to the pipeline, and the pipeline's media comes back the same
/// way.
#[tokio::test]
async fn a_viewer_path_of_the_media_relay_crosses_the_turn_server() {
    let range = crate::media_relay::ports(2).await;
    let relay = DaemonRelay::new(MediaForwarder::new("127.0.0.1".to_string(), range.clone()));
    let path = relay.open(CancellationToken::new()).await.unwrap();
    let credentials = IceCredentials {
        ufrag: "pipe".to_string(),
        pwd: "pipelinepasswordpipeline".to_string(),
    };
    path.authenticate(credentials.clone());
    let pipeline = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    pipeline
        .send_to(&path.registration(), (Ipv4Addr::LOCALHOST, path.port))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let turn = turn_server(MediaRelayPeers::new(Ipv4Addr::LOCALHOST.into(), range)).await;
    let browser = client_of(&turn).await;
    let candidate: SocketAddr = path.candidate.parse().unwrap();
    let check = ice_check(&credentials, true);

    browser.send_to(&check, candidate).await.unwrap();

    let (arrived, relay_side) = read(&pipeline).await;
    assert_eq!(arrived, check);
    pipeline
        .send_to(b"\x80\x60a video frame", relay_side)
        .await
        .unwrap();
    let (frame, from) = received(&browser).await;
    assert_eq!(frame, b"\x80\x60a video frame");
    assert_eq!(from, candidate);
}

/// A client that allocates nothing in time is closed, so a stranger who
/// connects through the public name holds no connection.
#[tokio::test]
async fn a_client_that_allocates_nothing_is_closed() {
    let (_media, peers) = media_relay().await;
    let turn = turn_server_waiting(peers, Duration::from_millis(200)).await;
    let mut stranger = TcpStream::connect(turn.address()).await.unwrap();

    let read = tokio::time::timeout(Duration::from_secs(5), stranger.read(&mut [0u8; 64]))
        .await
        .expect("the server closed the connection");

    assert_eq!(read.unwrap(), 0);
}

/// A stream that is not TURN closes the connection at once.
#[tokio::test]
async fn a_stream_that_is_not_turn_is_closed() {
    let (_media, peers) = media_relay().await;
    let turn = turn_server(peers).await;
    let mut stranger = TcpStream::connect(turn.address()).await.unwrap();

    tokio::io::AsyncWriteExt::write_all(&mut stranger, b"GET / HTTP/1.1\r\n\r\n")
        .await
        .unwrap();

    let read = tokio::time::timeout(Duration::from_secs(5), stranger.read(&mut [0u8; 64]))
        .await
        .expect("the server closed the connection");
    assert_eq!(read.unwrap(), 0);
}

/// A client that goes away with no Refresh takes its allocation with it:
/// the relay port is free again.
#[tokio::test]
async fn a_client_that_hangs_up_frees_its_relay_port() {
    let (_media, peers) = media_relay().await;
    let turn = turn_server(peers).await;
    let client = client_of(&turn).await;
    let relayed = client.relayed_address();
    assert!(UdpSocket::bind(relayed).await.is_err());

    client.hang_up().await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while UdpSocket::bind(relayed).await.is_err() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the relay port {relayed} is still held"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The server holds a bounded number of clients and closes the next one
/// at once.
#[tokio::test]
async fn a_client_over_the_limit_is_closed() {
    let (_media, peers) = media_relay().await;
    let turn = turn_server(peers).await;
    let mut held = Vec::new();
    for _ in 0..pagis_computer::remote_access_turn::CONNECTIONS_MAX {
        held.push(TcpStream::connect(turn.address()).await.unwrap());
    }
    // Each held client is accepted before the next one connects.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut over = TcpStream::connect(turn.address()).await.unwrap();

    let read = tokio::time::timeout(Duration::from_secs(5), over.read(&mut [0u8; 64]))
        .await
        .expect("the server closed the connection");
    assert_eq!(read.unwrap(), 0);
}

/// A server that stops closes its clients and its port.
#[tokio::test]
async fn a_server_that_stops_closes_its_clients_and_its_port() {
    let (_media, peers) = media_relay().await;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let stop = CancellationToken::new();
    let turn = RemoteAccessTurn::start(listener, peers, stop.clone()).unwrap();
    let mut client = TcpStream::connect(turn.address()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    stop.cancel();

    let read = tokio::time::timeout(Duration::from_secs(5), client.read(&mut [0u8; 64]))
        .await
        .expect("the server closed the connection");
    assert_eq!(read.unwrap(), 0);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while TcpStream::connect(turn.address()).await.is_ok() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the server still listens"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
