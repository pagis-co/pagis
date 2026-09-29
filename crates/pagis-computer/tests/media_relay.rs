//! The Media Relay suite: one set of test bodies, both relays.
//!
//! [`MediaRelay`] is the contract between the screen path and the media
//! path. Two implementations satisfy it, so one suite proves both: a
//! body written here runs against the `daemon` relay and against the
//! `turn` relay, and an implementation that behaves differently fails
//! the suite. The store suite of the storage crates is the same
//! arrangement.
//!
//! A body takes an [`Implementation`], which builds one relay over a
//! port range and a silence limit the body chooses. It never names a
//! concrete relay type, so a body cannot be written for one
//! implementation only. [`relay_suite`] turns every body into a test of
//! each implementation, and the guard test at the bottom fails while a
//! body is missing from the list.

use std::net::SocketAddr;
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;

use pagis_computer::fake::ice_check;
use pagis_computer::{
    DaemonRelay, IceCredentials, MediaForwarder, MediaPath, MediaRelay, OpenPath, TurnRelay,
    TurnServer,
};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

/// What the relays of the suite advertise. The browser and the computer
/// of a body are both sockets on this machine.
const ADVERTISE: &str = "127.0.0.1";

/// One Media Relay implementation under the suite.
trait Implementation {
    fn relay(&self, ports: RangeInclusive<u16>, idle: Duration) -> Arc<dyn MediaRelay>;
}

struct Daemon;

impl Implementation for Daemon {
    fn relay(&self, ports: RangeInclusive<u16>, idle: Duration) -> Arc<dyn MediaRelay> {
        Arc::new(DaemonRelay::new(forwarder(ports, idle)))
    }
}

struct Turn;

impl Implementation for Turn {
    fn relay(&self, ports: RangeInclusive<u16>, idle: Duration) -> Arc<dyn MediaRelay> {
        Arc::new(TurnRelay::new(
            forwarder(ports, idle),
            TurnServer {
                urls: vec!["turn:relay.example.net:3478?transport=udp".to_string()],
                secret: "shared".to_string(),
                ttl: Duration::from_secs(600),
            },
        ))
    }
}

fn forwarder(ports: RangeInclusive<u16>, idle: Duration) -> MediaForwarder {
    MediaForwarder::new(ADVERTISE.to_string(), ports).with_idle(idle)
}

/// A free UDP range of `count` ports, found by holding them and letting
/// them go. The bodies run in parallel processes, so a range written
/// down here would be a range another body already holds; the base is
/// spread by the process id and the clock and the ports are probed the
/// way the relay binds them.
async fn ports(count: u16) -> RangeInclusive<u16> {
    let spread = std::process::id()
        ^ std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after 1970")
            .subsec_nanos();
    for attempt in 0..200u32 {
        let first =
            20000 + ((spread.wrapping_add(attempt.wrapping_mul(count.into()))) % 8000) as u16;
        let mut held = Vec::new();
        for port in first..(first + count) {
            match UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, port)).await {
                Ok(socket) => held.push(socket),
                Err(_) => break,
            }
        }
        if held.len() == usize::from(count) {
            return first..=(first + count - 1);
        }
    }
    panic!("no free UDP range of {count} ports on this machine");
}

/// The ICE credentials of the pipeline's answer. The browser of a body
/// signs its checks with them.
fn credentials() -> IceCredentials {
    IceCredentials {
        ufrag: "pipe".to_string(),
        pwd: "pipelinepasswordpipeline".to_string(),
    }
}

/// A path that has the pipeline's ICE credentials, as the daemon gives
/// them to it after the pipeline answers.
async fn live_path(relay: &dyn MediaRelay) -> OpenPath {
    let path = relay
        .open(CancellationToken::new())
        .await
        .expect("the relay opened a path");
    path.authenticate(credentials());
    path
}

/// A check the browser signs with the pipeline's credentials.
fn check(nominates: bool) -> Vec<u8> {
    ice_check(&credentials(), nominates)
}

/// The STUN transaction id of a message.
fn transaction(message: &[u8]) -> [u8; 12] {
    message[8..20].try_into().expect("a STUN header")
}

/// The address a browser sends media to.
fn relay_addr(path: &MediaPath) -> SocketAddr {
    path.candidate.parse().expect("the candidate is an ip:port")
}

/// The relay port the pipeline registers with. The container reaches the
/// daemon at the Docker host; here the pipeline is on this machine.
fn register_addr(path: &MediaPath) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], path.port))
}

/// A socket standing in for one Computer's media pipeline, or for the
/// browser that watches it.
async fn socket() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("a loopback UDP socket")
}

/// A pipeline that registered with `path` the way screend does: from
/// its own media socket, with the path's token.
async fn registered_computer(path: &MediaPath) -> UdpSocket {
    let computer = socket().await;
    computer
        .send_to(&path.registration(), register_addr(path))
        .await
        .expect("the registration goes out");
    // The registration and the browser's first packet are two sockets,
    // so let the registration land first.
    tokio::time::sleep(Duration::from_millis(50)).await;
    computer
}

/// Read one datagram, or fail the body rather than hang it.
async fn read(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0u8; 2048];
    let (read, from) = tokio::time::timeout(Duration::from_secs(5), socket.recv_from(&mut buffer))
        .await
        .expect("a datagram arrived before the deadline")
        .expect("the socket read succeeded");
    (buffer[..read].to_vec(), from)
}

/// Whether a datagram arrives within a short wait.
async fn reads_nothing(socket: &UdpSocket) -> bool {
    let mut buffer = [0u8; 2048];
    tokio::time::timeout(Duration::from_millis(300), socket.recv_from(&mut buffer))
        .await
        .is_err()
}

/// The candidate is the relay's advertised address and one port of its
/// range, never the Computer's own address. A browser has no
/// route to the Tenant Network, so the candidate has to be the relay's.
async fn the_candidate_names_the_relay_and_one_port_of_the_range(
    implementation: &dyn Implementation,
) {
    let range = ports(4).await;
    let relay = implementation.relay(range.clone(), Duration::from_secs(5));

    let path = relay
        .open(CancellationToken::new())
        .await
        .expect("the relay opened a path");

    let address = relay_addr(&path);
    assert_eq!(address.ip().to_string(), ADVERTISE);
    assert!(range.contains(&address.port()), "{path:?} vs {range:?}");
    // The pipeline registers with the same port the browser reaches.
    assert_eq!(path.port, address.port());
}

/// Every path takes a token of its own, so a pipeline that holds
/// the token of one session cannot take the container leg of another.
async fn two_paths_take_two_tokens(implementation: &dyn Implementation) {
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));

    let first = relay.open(CancellationToken::new()).await.unwrap();
    let second = relay.open(CancellationToken::new()).await.unwrap();

    assert!(!first.token.is_empty());
    assert_ne!(first.token, second.token);
    assert_ne!(first.registration(), second.registration());
}

/// What the browser sends reaches the pipeline that registered.
/// The relay accepts the browser's address from this first packet: a
/// browser sends its own ICE check, signed with the pipeline's
/// credentials, before the pipeline says anything.
async fn a_packet_from_the_browser_reaches_the_registered_computer(
    implementation: &dyn Implementation,
) {
    let browser = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    let check = check(false);

    browser.send_to(&check, relay_addr(&path)).await.unwrap();

    let (datagram, from) = read(&computer).await;
    assert_eq!(datagram, check);
    // It arrives from the relay's own socket, not from the browser, so
    // the pipeline sees one peer for one viewer.
    assert_eq!(from, register_addr(&path));
}

/// What the registered pipeline sends reaches the browser.
async fn a_packet_from_the_registered_computer_reaches_the_browser(
    implementation: &dyn Implementation,
) {
    let browser = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    browser
        .send_to(&check(false), relay_addr(&path))
        .await
        .unwrap();
    let (_, relay_side) = read(&computer).await;

    computer
        .send_to(b"a video frame", relay_side)
        .await
        .unwrap();

    let (datagram, from) = read(&browser).await;
    assert_eq!(datagram, b"a video frame");
    assert_eq!(from, relay_addr(&path));
}

/// A registration is not media: the relay keeps it and sends it
/// to nobody, so the browser never reads the path's token.
async fn a_registration_reaches_no_browser(implementation: &dyn Implementation) {
    let browser = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    browser
        .send_to(&check(false), relay_addr(&path))
        .await
        .unwrap();
    read(&computer).await;

    computer
        .send_to(&path.registration(), register_addr(&path))
        .await
        .unwrap();

    assert!(
        reads_nothing(&browser).await,
        "the registration was forwarded"
    );
}

/// A registration with another token takes nothing. A stranger
/// who finds a relay port cannot draw the browser's packets to itself.
async fn a_registration_with_another_token_takes_nothing(implementation: &dyn Implementation) {
    let browser = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    let stranger = socket().await;
    let forged = MediaPath {
        token: "another-token".to_string(),
        ..path.clone()
    };
    stranger
        .send_to(&forged.registration(), register_addr(&path))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let check = check(false);

    browser.send_to(&check, relay_addr(&path)).await.unwrap();

    let (datagram, _) = read(&computer).await;
    assert_eq!(datagram, check);
    assert!(reads_nothing(&stranger).await, "the stranger took the path");
}

/// Before any pipeline registers, the browser's packets reach nobody.
/// The relay has no Computer address of its own to guess.
async fn a_path_with_no_registration_forwards_nothing(implementation: &dyn Implementation) {
    let browser = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = socket().await;

    browser
        .send_to(&check(false), relay_addr(&path))
        .await
        .unwrap();

    assert!(reads_nothing(&computer).await);
}

/// A path accepts no browser before the daemon gives it the pipeline's
/// credentials. Until then, no check can show who sent it.
async fn a_path_takes_no_browser_before_it_has_the_credentials(
    implementation: &dyn Implementation,
) {
    let browser = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = relay.open(CancellationToken::new()).await.unwrap();
    let computer = registered_computer(&path).await;

    browser
        .send_to(&check(true), relay_addr(&path))
        .await
        .unwrap();

    assert!(
        reads_nothing(&computer).await,
        "a check crossed a path that has no credentials"
    );
}

/// Two viewers take two ports. The pipeline tells DTLS and RTP
/// sessions apart by source address, so two viewers behind one port
/// would reach it as one peer and neither would see the screen.
async fn two_viewers_take_two_ports(implementation: &dyn Implementation) {
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));

    let first = relay.open(CancellationToken::new()).await.unwrap();
    let second = relay.open(CancellationToken::new()).await.unwrap();

    assert_ne!(relay_addr(&first).port(), relay_addr(&second).port());
}

/// A range with no free port refuses the path and says what to do.
/// The range bounds how many people watch at once, so the
/// refusal has to name it.
async fn a_full_range_refuses_the_path(implementation: &dyn Implementation) {
    let range = ports(1).await;
    let relay = implementation.relay(range.clone(), Duration::from_secs(5));
    let _held = relay.open(CancellationToken::new()).await.unwrap();

    let error = relay
        .open(CancellationToken::new())
        .await
        .expect_err("the only port of the range is taken");

    assert!(error.contains(&range.start().to_string()), "{error}");
    assert!(error.contains("media_port_first"), "{error}");
}

/// A path that falls silent gives its port back. Nothing tells
/// the daemon that a viewer closed the tab, so silence is the signal; a
/// live session holds its port with ICE consent checks and the
/// pipeline's repeated registration.
async fn an_idle_path_gives_its_port_back(implementation: &dyn Implementation) {
    let range = ports(1).await;
    let relay = implementation.relay(range.clone(), Duration::from_millis(100));
    let first = relay.open(CancellationToken::new()).await.unwrap();

    tokio::time::sleep(Duration::from_millis(400)).await;
    let second = relay
        .open(CancellationToken::new())
        .await
        .expect("the port of the silent viewer came back");

    assert_eq!(relay_addr(&first).port(), relay_addr(&second).port());
}

/// The pipeline's repeated registration holds a live path open. The
/// pipeline registers again and again while its session lives, so the
/// path outlives the silence limit and keeps its port.
async fn the_registration_of_the_computer_holds_a_live_path_open(
    implementation: &dyn Implementation,
) {
    let idle = Duration::from_millis(300);
    let relay = implementation.relay(ports(1).await, idle);
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;

    for _ in 0..12 {
        tokio::time::sleep(idle / 3).await;
        computer
            .send_to(&path.registration(), register_addr(&path))
            .await
            .unwrap();
    }

    let browser = socket().await;
    let check = check(false);
    browser.send_to(&check, relay_addr(&path)).await.unwrap();
    let (datagram, _) = read(&computer).await;
    assert_eq!(datagram, check);
    assert!(
        relay.open(CancellationToken::new()).await.is_err(),
        "the live path gave its port away"
    );
}

/// The browser's ICE consent checks (RFC 7675) hold a live path open,
/// as they hold the browser's own session.
async fn the_checks_of_the_browser_hold_a_live_path_open(implementation: &dyn Implementation) {
    let idle = Duration::from_millis(300);
    let relay = implementation.relay(ports(1).await, idle);
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    let browser = socket().await;

    for _ in 0..12 {
        browser
            .send_to(&check(false), relay_addr(&path))
            .await
            .unwrap();
        read(&computer).await;
        tokio::time::sleep(idle / 3).await;
    }

    assert!(
        relay.open(CancellationToken::new()).await.is_err(),
        "the live path gave its port away"
    );
}

/// A stranger who sends to a path more often than the silence limit
/// does not hold it open. When the viewer and the pipeline stop, the
/// path frees its port, whatever the stranger sends: media, a
/// registration with another token, or a nomination it cannot sign.
async fn a_stranger_does_not_hold_a_path_open_after_its_viewer_and_its_computer_stop(
    implementation: &dyn Implementation,
) {
    let idle = Duration::from_millis(300);
    let relay = implementation.relay(ports(1).await, idle);
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    let browser = socket().await;
    browser
        .send_to(&check(true), relay_addr(&path))
        .await
        .unwrap();
    read(&computer).await;
    // The viewer and the pipeline stop.
    drop((browser, computer));

    let stranger = socket().await;
    let forged = MediaPath {
        token: "another-token".to_string(),
        ..path.clone()
    };
    let datagrams = [
        MEDIA.to_vec(),
        forged.registration(),
        binding_request([9; 12], true),
    ];
    let until = tokio::time::Instant::now() + idle * 4;
    while tokio::time::Instant::now() < until {
        for datagram in &datagrams {
            // A closed port can refuse a datagram; the stranger goes on.
            let _ = stranger.send_to(datagram, relay_addr(&path)).await;
        }
        tokio::time::sleep(idle / 6).await;
    }

    let again = relay
        .open(CancellationToken::new())
        .await
        .expect("the port of the abandoned path came back");
    assert_eq!(relay_addr(&again).port(), relay_addr(&path).port());
}

/// A path the daemon closes forwards nothing more and gives its port
/// back at once. The daemon closes the paths of a Session when the
/// Session ends, so video and input stop for that viewer then, and not
/// after the silence of an idle path. The browser keeps its ICE consent
/// checks going, so silence would never come.
async fn a_closed_path_forwards_nothing_and_gives_its_port_back(
    implementation: &dyn Implementation,
) {
    let range = ports(1).await;
    let relay = implementation.relay(range, Duration::from_secs(5));
    let closed = CancellationToken::new();
    let path = relay.open(closed.clone()).await.unwrap();
    path.authenticate(credentials());
    let computer = registered_computer(&path).await;
    let browser = socket().await;
    browser
        .send_to(&check(false), relay_addr(&path))
        .await
        .unwrap();
    read(&computer).await;

    closed.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;

    browser
        .send_to(&check(false), relay_addr(&path))
        .await
        .unwrap();
    assert!(
        reads_nothing(&computer).await,
        "a closed path forwarded a packet"
    );
    let again = relay
        .open(CancellationToken::new())
        .await
        .expect("the port of the closed path came back");
    assert_eq!(relay_addr(&again).port(), relay_addr(&path).port());
}

/// A STUN Binding request (RFC 8489) with no MESSAGE-INTEGRITY: the
/// header, the transaction id, and USE-CANDIDATE when the request
/// nominates its pair. It has the shape of a browser's check, and
/// nobody signed it.
fn binding_request(transaction: [u8; 12], nominates: bool) -> Vec<u8> {
    // USE-CANDIDATE is an attribute with no value.
    let attributes: &[u8] = if nominates {
        &[0x00, 0x25, 0x00, 0x00]
    } else {
        &[]
    };
    stun(0x0001, transaction, attributes)
}

/// The pipeline's success answer to the check with `transaction`.
fn binding_response(transaction: [u8; 12]) -> Vec<u8> {
    stun(0x0101, transaction, &[])
}

fn stun(kind: u16, transaction: [u8; 12], attributes: &[u8]) -> Vec<u8> {
    let mut message = Vec::new();
    message.extend_from_slice(&kind.to_be_bytes());
    message.extend_from_slice(&(attributes.len() as u16).to_be_bytes());
    message.extend_from_slice(&0x2112_A442u32.to_be_bytes());
    message.extend_from_slice(&transaction);
    message.extend_from_slice(attributes);
    message
}

/// The first bytes of an SRTP packet: version 2, as RFC 7983 sorts it.
const MEDIA: &[u8] = b"\x80\x60a video frame";

/// A datagram from a stranger does not move the media of a live path.
/// The stranger found the port and sends media and DTLS without pause,
/// but it never sent a check signed with the pipeline's credentials.
/// So the relay gives the pipeline nothing of it and sends the stranger
/// nothing.
async fn a_datagram_from_a_stranger_moves_no_media(implementation: &dyn Implementation) {
    let browser = socket().await;
    let stranger = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    browser
        .send_to(&check(true), relay_addr(&path))
        .await
        .unwrap();
    let (_, relay_side) = read(&computer).await;

    for _ in 0..3 {
        stranger.send_to(MEDIA, relay_addr(&path)).await.unwrap();
        stranger
            .send_to(b"\x16\xfe\xfda DTLS record", relay_addr(&path))
            .await
            .unwrap();
    }
    assert!(
        reads_nothing(&computer).await,
        "a datagram of the stranger reached the pipeline"
    );
    computer.send_to(MEDIA, relay_side).await.unwrap();

    let (datagram, _) = read(&browser).await;
    assert_eq!(datagram, MEDIA);
    assert!(
        reads_nothing(&stranger).await,
        "the media went to the stranger"
    );
}

/// A check that nominates its pair (USE-CANDIDATE) moves nothing when
/// the pipeline's password did not sign it. A stranger can copy the
/// shape of a browser's check, but not its MESSAGE-INTEGRITY.
async fn a_nominating_check_without_valid_integrity_moves_nothing(
    implementation: &dyn Implementation,
) {
    let browser = socket().await;
    let stranger = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    browser
        .send_to(&check(true), relay_addr(&path))
        .await
        .unwrap();
    let (_, relay_side) = read(&computer).await;
    let another_password = IceCredentials {
        pwd: "anotherpasswordanother12".to_string(),
        ..credentials()
    };

    for nomination in [
        binding_request([9; 12], true),
        ice_check(&another_password, true),
    ] {
        stranger
            .send_to(&nomination, relay_addr(&path))
            .await
            .unwrap();
    }
    assert!(
        reads_nothing(&computer).await,
        "a check without valid integrity reached the pipeline"
    );
    computer.send_to(MEDIA, relay_side).await.unwrap();

    let (datagram, _) = read(&browser).await;
    assert_eq!(datagram, MEDIA);
    assert!(
        reads_nothing(&stranger).await,
        "a nomination without valid integrity moved the media"
    );
}

/// A malformed STUN message from a stranger ends nothing. The relay
/// reads datagrams from anybody, and one that is not a valid check is
/// dropped while the path carries on.
async fn a_malformed_message_from_a_stranger_leaves_the_path_open(
    implementation: &dyn Implementation,
) {
    let browser = socket().await;
    let stranger = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    // A Binding request whose last attribute, a FINGERPRINT, has no
    // value.
    let malformed = stun(0x0001, [9; 12], &[0x80, 0x28, 0x00, 0x00]);

    stranger
        .send_to(&malformed, relay_addr(&path))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let check = check(false);
    browser.send_to(&check, relay_addr(&path)).await.unwrap();
    let (datagram, _) = read(&computer).await;
    assert_eq!(datagram, check);
}

/// A browser with a microphone grant gathers a host candidate on every
/// interface, so its checks reach the relay from several addresses at
/// once. Each answer goes back to the address that sent its check. An
/// answer sent to whichever address spoke last never reaches the pair
/// that asked, and the browser drops the session when consent expires.
async fn each_check_is_answered_at_the_address_that_sent_it(implementation: &dyn Implementation) {
    let first = socket().await;
    let second = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    let first_check = check(false);

    first
        .send_to(&first_check, relay_addr(&path))
        .await
        .unwrap();
    let (_, relay_side) = read(&computer).await;
    second
        .send_to(&check(false), relay_addr(&path))
        .await
        .unwrap();
    read(&computer).await;

    let answer = binding_response(transaction(&first_check));
    computer.send_to(&answer, relay_side).await.unwrap();

    let (datagram, _) = read(&first).await;
    assert_eq!(datagram, answer);
    assert!(
        reads_nothing(&second).await,
        "the answer went to the wrong address"
    );
}

/// An address of this machine that is not loopback: the source address
/// of its default route. A browser on this machine that holds a
/// microphone grant gathers a candidate there, and its checks reach the
/// relay's loopback address from it.
async fn an_interface_address() -> std::net::IpAddr {
    let probe = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    // A connected UDP socket sends nothing; the system only picks the
    // source address of the route.
    probe
        .connect("192.0.2.1:9")
        .await
        .expect("this machine has a default route");
    probe.local_addr().unwrap().ip()
}

/// Every answer and every media packet leaves the relay from the address
/// the browser sent to, which is the candidate. A browser whose check
/// reached the loopback candidate from an interface address takes an
/// answer from any other source address as no answer, and the pair
/// never connects.
async fn the_browser_hears_the_relay_from_its_candidate(implementation: &dyn Implementation) {
    let browser = UdpSocket::bind((an_interface_address().await, 0))
        .await
        .expect("a UDP socket on an interface address");
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    let asked = check(true);

    browser.send_to(&asked, relay_addr(&path)).await.unwrap();
    let (_, relay_side) = read(&computer).await;
    computer
        .send_to(&binding_response(transaction(&asked)), relay_side)
        .await
        .unwrap();
    let (_, answered_from) = read(&browser).await;
    computer.send_to(MEDIA, relay_side).await.unwrap();
    let (_, media_from) = read(&browser).await;

    assert_eq!(
        answered_from,
        relay_addr(&path),
        "the answer came from another address"
    );
    assert_eq!(
        media_from,
        relay_addr(&path),
        "the media came from another address"
    );
}

/// Media goes to the pair the browser nominated, not to the address of
/// a check that arrived after the nomination.
async fn media_goes_to_the_nominated_address(implementation: &dyn Implementation) {
    let nominated = socket().await;
    let other = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    nominated
        .send_to(&check(true), relay_addr(&path))
        .await
        .unwrap();
    let (_, relay_side) = read(&computer).await;
    other
        .send_to(&check(false), relay_addr(&path))
        .await
        .unwrap();
    read(&computer).await;

    computer.send_to(MEDIA, relay_side).await.unwrap();

    let (datagram, _) = read(&nominated).await;
    assert_eq!(datagram, MEDIA);
    assert!(
        reads_nothing(&other).await,
        "media went to a pair nobody selected"
    );
}

/// A browser that moves its session to another pair it checked sends
/// media from there, and the media follows it.
async fn media_follows_the_address_the_browser_sends_media_from(
    implementation: &dyn Implementation,
) {
    let nominated = socket().await;
    let moved = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    nominated
        .send_to(&check(true), relay_addr(&path))
        .await
        .unwrap();
    let (_, relay_side) = read(&computer).await;
    moved
        .send_to(&check(false), relay_addr(&path))
        .await
        .unwrap();
    read(&computer).await;
    moved.send_to(MEDIA, relay_addr(&path)).await.unwrap();
    read(&computer).await;

    computer.send_to(MEDIA, relay_side).await.unwrap();

    let (datagram, _) = read(&moved).await;
    assert_eq!(datagram, MEDIA);
    assert!(reads_nothing(&nominated).await);
}

/// A browser that moves to another address signs a check from there,
/// and its media follows it. A browser moves when its network changes,
/// and when ICE nominates a better pair.
async fn a_browser_that_moves_with_an_authenticated_check_still_gets_its_media(
    implementation: &dyn Implementation,
) {
    let before = socket().await;
    let after = socket().await;
    let relay = implementation.relay(ports(4).await, Duration::from_secs(5));
    let path = live_path(relay.as_ref()).await;
    let computer = registered_computer(&path).await;
    before
        .send_to(&check(true), relay_addr(&path))
        .await
        .unwrap();
    let (_, relay_side) = read(&computer).await;

    after
        .send_to(&check(true), relay_addr(&path))
        .await
        .unwrap();
    read(&computer).await;
    computer.send_to(MEDIA, relay_side).await.unwrap();

    let (datagram, _) = read(&after).await;
    assert_eq!(datagram, MEDIA);
    assert!(
        reads_nothing(&before).await,
        "the media stayed at the address the browser left"
    );
}

/// Write each body as a test of each implementation.
macro_rules! relay_suite {
    ($($name:ident),* $(,)?) => {
        mod on_daemon {
            $(
                #[tokio::test]
                async fn $name() {
                    super::$name(&super::Daemon).await;
                }
            )*
        }

        mod on_turn {
            $(
                #[tokio::test]
                async fn $name() {
                    super::$name(&super::Turn).await;
                }
            )*
        }
    };
}

relay_suite!(
    the_candidate_names_the_relay_and_one_port_of_the_range,
    two_paths_take_two_tokens,
    a_packet_from_the_browser_reaches_the_registered_computer,
    a_packet_from_the_registered_computer_reaches_the_browser,
    a_registration_reaches_no_browser,
    a_registration_with_another_token_takes_nothing,
    a_path_with_no_registration_forwards_nothing,
    a_path_takes_no_browser_before_it_has_the_credentials,
    two_viewers_take_two_ports,
    a_full_range_refuses_the_path,
    an_idle_path_gives_its_port_back,
    the_registration_of_the_computer_holds_a_live_path_open,
    the_checks_of_the_browser_hold_a_live_path_open,
    a_stranger_does_not_hold_a_path_open_after_its_viewer_and_its_computer_stop,
    a_closed_path_forwards_nothing_and_gives_its_port_back,
    a_datagram_from_a_stranger_moves_no_media,
    a_nominating_check_without_valid_integrity_moves_nothing,
    a_malformed_message_from_a_stranger_leaves_the_path_open,
    each_check_is_answered_at_the_address_that_sent_it,
    media_goes_to_the_nominated_address,
    media_follows_the_address_the_browser_sends_media_from,
    a_browser_that_moves_with_an_authenticated_check_still_gets_its_media,
    the_browser_hears_the_relay_from_its_candidate,
);

/// Every body of this file is in the suite list, so a body cannot be
/// added and left out of one implementation.
#[test]
fn the_suite_runs_every_body() {
    let file = include_str!("media_relay.rs");
    let listed = file
        .split_once("relay_suite!(")
        .expect("the suite list")
        .1
        .split_once(");")
        .expect("the end of the suite list")
        .0;
    for chunk in file.split("\nasync fn ").skip(1) {
        // Everything up to the body's own brace is its signature.
        let (signature, _) = chunk.split_once('{').expect("a body signature");
        // The helpers of this file take no implementation; the bodies do.
        if !signature.contains("dyn Implementation") {
            continue;
        }
        let name = signature.split('(').next().expect("a body name");
        assert!(
            listed.contains(name),
            "the body {name} is not in the relay_suite! list"
        );
    }
}

/// The `daemon` relay needs no ICE server: the browser reaches the
/// advertised address itself.
#[test]
fn the_daemon_relay_names_no_ice_server() {
    // No path opens here, so the range is the ephemeral one.
    let relay = Daemon.relay(0..=0, Duration::from_secs(5));

    assert!(relay.ice_servers().is_empty());
}

/// The `turn` relay names the TURN server and mints one credential for
/// each viewer session. The credentials are the TURN REST API
/// scheme, so the TURN server recomputes the password from the shared
/// secret and holds no account.
#[test]
fn the_turn_relay_mints_a_credential_for_each_session() {
    let relay = Turn.relay(0..=0, Duration::from_secs(5));

    let first = relay.ice_servers();
    let second = relay.ice_servers();

    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].urls,
        vec!["turn:relay.example.net:3478?transport=udp".to_string()]
    );
    // The username carries the expiry the TURN server checks.
    let expiry: i64 = first[0]
        .username
        .split_once(':')
        .expect("expiry and session")
        .0
        .parse()
        .expect("the expiry is Unix seconds");
    assert!(expiry > pagis_core::now_ms() / 1000);
    assert!(!first[0].credential.is_empty());
    assert_ne!(first[0].username, second[0].username);
}
