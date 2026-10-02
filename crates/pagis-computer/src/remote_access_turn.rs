//! The TURN server of Remote Access (ADR-0028): how a browser on another
//! machine reaches the Media Relay through the owner's Tailscale Funnel.
//!
//! The Funnel carries TCP alone, and the Media Relay takes UDP
//! (ADR-0014). With Remote Access on, the daemon runs this TURN server on
//! a loopback TCP port, and Funnel publishes that port on port 8443 of the
//! public name, where `tailscaled` ends TLS and forwards plain TCP. A
//! browser that gets the server from the ICE route makes a UDP allocation
//! over that connection (RFC 8656), and the server relays its datagrams to
//! the Media Relay on this machine.
//!
//! The TURN code is the `turn` crate of webrtc-rs: the long-term
//! credential check, allocations, permissions and channels. That code
//! reads whole messages, so this module frames the TCP stream of each
//! client as RFC 8656 section 12.5 says ([`frame`]) and gives each
//! connection to the crate as one `Conn`. A connection that closes takes
//! its allocation with it.
//!
//! The server relays to the Media Relay and to nothing else on this
//! machine: each relay socket binds the Media Relay's own address, sends
//! only to that address on a port of the Media Relay's range, and drops
//! every datagram from anywhere else ([`MediaRelayPeers`]). Its
//! credentials follow the scheme of the `turn` relay, under a secret that
//! the daemon makes at each start and writes nowhere.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::time::Instant;
use tokio_util::sync::{CancellationToken, DropGuard};
use turn::allocation::allocation_manager::{Manager, ManagerConfig};
use turn::allocation::five_tuple::FiveTuple;
use turn::auth::{AuthHandler, generate_auth_key};
use turn::relay::RelayAddressGenerator;
use turn::server::request::Request;
use webrtc_util::Conn;

use crate::IceServer;
use crate::relay::{mint_turn_credential, turn_password};

/// The realm of the server. A client makes the key that signs its
/// requests from the realm that the server names, so any fixed name works.
pub const REALM: &str = "pagis";

/// How long a credential lives. A browser signs each Refresh of its
/// allocation with the credential it got, so a credential that ends ends
/// the live screen of that browser, and twelve hours hold a working day.
/// A credential that leaks reaches the Media Relay alone, where a viewer
/// path takes nothing from a sender that does not hold its ICE password.
pub const CREDENTIAL_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// How long a connection may stay open with no allocation. A browser
/// allocates one or two round trips after it connects, and anybody else
/// who connects through the public name holds no connection longer.
pub const ALLOCATE_WITHIN: Duration = Duration::from_secs(10);

/// How many client connections the server holds at once. Each viewer
/// holds one, and the Funnel carries a few viewers (ADR-0028).
pub const CONNECTIONS_MAX: usize = 64;

/// The largest message the server reads: a STUN message, or a ChannelData
/// message that carries one datagram for the Media Relay.
const MESSAGE_MAX: usize = 2048;

/// How many messages wait for the TCP connection of one client. Media
/// that finds the queue full is dropped, as a congested UDP path drops it.
const WRITE_QUEUE: usize = 256;

/// How many nonces the server keeps. The crate keeps each nonce it gives
/// out and forgets it only when a client presents it after it expired,
/// and each request without credentials makes one, so [`bound_nonces`]
/// holds the map to this size. A client whose nonce the bound drops gets
/// 438 (Stale Nonce) and a new nonce, and goes on (RFC 8489 section 9.2.4).
const NONCES_MAX: usize = 1024;

/// How long a nonce is good for: the lifetime that the crate gives one.
const NONCE_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// How many sockets one allocation binds before it finds a port outside
/// the Media Relay's range.
const RELAY_BIND_ATTEMPTS: usize = 8;

/// The peers that the server relays to: the Media Relay's own address, on
/// a port of its UDP range. The Media Relay advertises this address in the
/// candidate of each path, so it is the peer that a browser names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRelayPeers {
    address: IpAddr,
    ports: RangeInclusive<u16>,
}

impl MediaRelayPeers {
    /// The Media Relay at `address`, with the UDP range `ports`.
    pub fn new(address: IpAddr, ports: RangeInclusive<u16>) -> Self {
        Self { address, ports }
    }

    /// Whether the server relays to `peer`. Every other address of this
    /// machine, and every other port of its loopback, is refused.
    pub fn allows(&self, peer: SocketAddr) -> bool {
        peer.ip() == self.address && self.ports.contains(&peer.port())
    }
}

/// The TURN server of Remote Access. It serves until it drops or until
/// the token it started with is cancelled, and each open allocation
/// closes with it.
pub struct RemoteAccessTurn {
    address: SocketAddr,
    credentials: Arc<Credentials>,
    _stop: DropGuard,
}

impl RemoteAccessTurn {
    /// Serve TURN on `listener`, a loopback port, and relay to `peers`
    /// alone. The server stops when `closed` is cancelled.
    pub fn start(
        listener: TcpListener,
        peers: MediaRelayPeers,
        closed: CancellationToken,
    ) -> std::io::Result<Self> {
        Self::start_waiting(listener, peers, closed, ALLOCATE_WITHIN)
    }

    /// The same, with another wait for the allocation of a connection.
    /// The tests use it; production keeps [`ALLOCATE_WITHIN`].
    pub fn start_waiting(
        listener: TcpListener,
        peers: MediaRelayPeers,
        closed: CancellationToken,
        allocate_within: Duration,
    ) -> std::io::Result<Self> {
        let address = listener.local_addr()?;
        let credentials = Arc::new(Credentials::generate());
        let shared = Arc::new(Shared {
            manager: Arc::new(Manager::new(ManagerConfig {
                relay_addr_generator: Box::new(RelaySockets {
                    peers: Arc::new(peers),
                }),
                alloc_close_notify: None,
            })),
            nonces: Arc::default(),
            credentials: Arc::clone(&credentials) as _,
            allocate_within,
        });
        let closed = closed.child_token();
        tokio::spawn(accept(listener, shared, closed.clone()));
        Ok(Self {
            address,
            credentials,
            _stop: closed.drop_guard(),
        })
    }

    /// The loopback address that the server listens on.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The ICE server that a browser configures to reach this server at
    /// `url`, with a credential of its own that ends after
    /// [`CREDENTIAL_TTL`].
    pub fn ice_server(&self, url: String) -> IceServer {
        let (username, credential) = self.credentials.mint();
        IceServer {
            urls: vec![url],
            username,
            credential,
        }
    }
}

/// The credentials of one server: the TURN REST API scheme of the `turn`
/// relay, under a secret that the daemon makes at each start. No
/// credential outlives the process that minted it.
struct Credentials {
    secret: String,
}

impl Credentials {
    /// A new secret: 32 random bytes, hex-encoded.
    fn generate() -> Self {
        let mut secret = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut secret);
        Self {
            secret: hex::encode(secret),
        }
    }

    /// A new username and its password.
    fn mint(&self) -> (String, String) {
        mint_turn_credential(&self.secret, CREDENTIAL_TTL)
    }

    /// The password of `username` at `now`, in Unix seconds, or why the
    /// username has none: it does not start with an expiry, or the expiry
    /// has passed.
    fn password(&self, username: &str, now: i64) -> Result<String, String> {
        let expiry = username
            .split_once(':')
            .and_then(|(expiry, _)| expiry.parse::<i64>().ok())
            .ok_or_else(|| {
                format!("the TURN username {username:?} does not start with an expiry")
            })?;
        if expiry <= now {
            return Err(format!("the TURN username {username:?} expired"));
        }
        Ok(turn_password(&self.secret, username))
    }
}

impl AuthHandler for Credentials {
    fn auth_handle(
        &self,
        username: &str,
        realm: &str,
        _client: SocketAddr,
    ) -> Result<Vec<u8>, turn::Error> {
        let password = self
            .password(username, pagis_core::now_ms() / 1000)
            .map_err(turn::Error::Other)?;
        Ok(generate_auth_key(username, realm, &password))
    }
}

/// Where the first message of a TCP stream of TURN ends.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    /// The length of the message.
    pub(crate) message: usize,
    /// The length that it takes in the stream: over TCP a ChannelData
    /// message is padded to four bytes.
    pub(crate) taken: usize,
}

/// The magic cookie that follows the type and the length of every STUN
/// message (RFC 8489 section 5).
const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];

/// The first message of `stream`, `None` while it is not all there, or
/// why the stream is not TURN (RFC 8656 section 12.5).
///
/// The first two bits tell the two kinds apart. A STUN message is a
/// 20-byte header with the magic cookie, then its attributes, whose
/// length the header gives in whole words (RFC 8489 section 5). A
/// ChannelData message is a 4-byte header and its data, and over TCP the
/// sender pads it to four bytes; the length field does not count the
/// padding.
pub(crate) fn frame(stream: &[u8]) -> Result<Option<Frame>, String> {
    let Some(header) = stream.get(..4) else {
        return Ok(None);
    };
    let length = usize::from(u16::from_be_bytes([header[2], header[3]]));
    let (message, taken) = match header[0] >> 6 {
        0b00 if length % 4 != 0 => {
            return Err(format!(
                "a STUN message of {length} bytes is not in whole words"
            ));
        }
        0b00 if stream
            .get(4..8)
            .is_some_and(|cookie| cookie != MAGIC_COOKIE) =>
        {
            return Err("a STUN message has no magic cookie".to_string());
        }
        0b00 => (20 + length, 20 + length),
        0b01 => (4 + length, (4 + length).next_multiple_of(4)),
        _ => {
            return Err(format!(
                "a message that starts with {:#04x} is neither STUN nor ChannelData",
                header[0]
            ));
        }
    };
    if taken > MESSAGE_MAX {
        return Err(format!(
            "a message of {taken} bytes is longer than {MESSAGE_MAX} bytes"
        ));
    }
    Ok((stream.len() >= taken).then_some(Frame { message, taken }))
}

/// What every connection of one server reads.
struct Shared {
    manager: Arc<Manager>,
    nonces: Arc<Mutex<HashMap<String, Instant>>>,
    credentials: Arc<dyn AuthHandler + Send + Sync>,
    allocate_within: Duration,
}

impl Shared {
    /// Give one message of `client` to the TURN code of the crate. Only a
    /// STUN message can make a nonce, so a ChannelData message, the media
    /// of a viewer, takes no lock of the nonces.
    async fn handle(
        &self,
        connection: &Arc<dyn Conn + Send + Sync>,
        client: SocketAddr,
        message: Vec<u8>,
    ) {
        if message.first().is_some_and(|first| first >> 6 == 0b00) {
            bound_nonces(&mut *self.nonces.lock().await, Instant::now());
        }
        let mut request = Request {
            conn: Arc::clone(connection),
            src_addr: client,
            buff: message,
            allocation_manager: Arc::clone(&self.manager),
            nonces: Arc::clone(&self.nonces),
            auth_handler: Arc::clone(&self.credentials),
            realm: REALM.to_string(),
            channel_bind_timeout: turn::proto::lifetime::DEFAULT_LIFETIME,
        };
        if let Err(error) = request.handle_request().await {
            tracing::debug!(%error, %client, "the TURN server of Remote Access refused a message");
        }
    }
}

/// Hold the nonces to [`NONCES_MAX`]: drop the expired ones, then keep
/// the newest half. A client uses its nonce in the next round trip, so the
/// newest half holds the nonce of each client that signs in now.
fn bound_nonces(nonces: &mut HashMap<String, Instant>, now: Instant) {
    if nonces.len() < NONCES_MAX {
        return;
    }
    nonces.retain(|_, given| now.duration_since(*given) < NONCE_LIFETIME);
    if nonces.len() < NONCES_MAX {
        return;
    }
    let mut given: Vec<Instant> = nonces.values().copied().collect();
    given.sort_unstable();
    let oldest_kept = given[given.len() - NONCES_MAX / 2];
    nonces.retain(|_, given| *given >= oldest_kept);
}

/// Accept clients until the server stops. A client over the limit is
/// closed at once.
async fn accept(listener: TcpListener, shared: Arc<Shared>, closed: CancellationToken) {
    let slots = Arc::new(Semaphore::new(CONNECTIONS_MAX));
    loop {
        let accepted = tokio::select! {
            () = closed.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let (stream, client) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                // Out of file descriptors, for example. The pause keeps
                // the loop from spinning until one is free.
                tracing::warn!(%error, "the TURN server of Remote Access cannot accept a client");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
            tracing::debug!(%client, "the TURN server of Remote Access holds {CONNECTIONS_MAX} clients and refuses one more");
            continue;
        };
        tokio::spawn(serve(
            stream,
            client,
            Arc::clone(&shared),
            closed.child_token(),
            slot,
        ));
    }
}

/// Serve one client until it leaves, the server stops, or it makes no
/// allocation in time. Its allocation closes with the connection.
async fn serve(
    stream: TcpStream,
    client: SocketAddr,
    shared: Arc<Shared>,
    ended: CancellationToken,
    _slot: OwnedSemaphorePermit,
) {
    let local = match stream.local_addr() {
        Ok(local) => local,
        Err(error) => {
            tracing::debug!(%error, %client, "a client of the TURN server of Remote Access left at once");
            return;
        }
    };
    let (reader, writer) = stream.into_split();
    let (queue, outgoing) = mpsc::channel(WRITE_QUEUE);
    let connection: Arc<dyn Conn + Send + Sync> = Arc::new(ClientConnection {
        local,
        client,
        queue,
    });
    let writing = tokio::spawn(write(writer, outgoing, ended.clone()));
    // The 5-tuple under which the crate files the allocation of this
    // client: it names UDP for every allocation it makes.
    let five_tuple = FiveTuple {
        protocol: turn::proto::PROTO_UDP,
        src_addr: client,
        dst_addr: local,
    };
    let why = read(reader, client, &connection, &shared, five_tuple, &ended).await;
    tracing::debug!(%client, %why, "a client of the TURN server of Remote Access left");
    shared.manager.delete_allocation(&five_tuple).await;
    ended.cancel();
    let _ = writing.await;
}

/// Read the messages of one client and handle each in turn. Answer why
/// the reading stops.
async fn read(
    mut reader: OwnedReadHalf,
    client: SocketAddr,
    connection: &Arc<dyn Conn + Send + Sync>,
    shared: &Shared,
    five_tuple: FiveTuple,
    ended: &CancellationToken,
) -> String {
    let mut stream = BytesMut::with_capacity(MESSAGE_MAX);
    let allocate_by = tokio::time::sleep(shared.allocate_within);
    tokio::pin!(allocate_by);
    let mut allocation_checked = false;
    loop {
        loop {
            match frame(&stream) {
                Ok(Some(Frame { message, taken })) => {
                    let mut message_bytes = stream.split_to(taken);
                    message_bytes.truncate(message);
                    shared
                        .handle(connection, client, message_bytes.to_vec())
                        .await;
                }
                Ok(None) => break,
                Err(why) => return why,
            }
        }
        stream.reserve(MESSAGE_MAX);
        tokio::select! {
            () = ended.cancelled() => return "the server stopped".to_string(),
            () = &mut allocate_by, if !allocation_checked => {
                if shared.manager.get_allocation(&five_tuple).await.is_none() {
                    return format!(
                        "it made no allocation in {} seconds",
                        shared.allocate_within.as_secs_f32()
                    );
                }
                allocation_checked = true;
            }
            read = reader.read_buf(&mut stream) => match read {
                Ok(0) => return "it closed the connection".to_string(),
                Ok(_) => {}
                Err(error) => return error.to_string(),
            },
        }
    }
}

/// Write the queued messages of one client until the connection ends.
/// A write that fails ends the connection, and so does the end of the
/// connection while a client that reads nothing holds a write.
async fn write(
    mut writer: OwnedWriteHalf,
    mut outgoing: mpsc::Receiver<Vec<u8>>,
    ended: CancellationToken,
) {
    loop {
        let message = tokio::select! {
            () = ended.cancelled() => return,
            message = outgoing.recv() => message,
        };
        let Some(message) = message else {
            return;
        };
        let written = tokio::select! {
            () = ended.cancelled() => return,
            written = writer.write_all(&message) => written,
        };
        if written.is_err() {
            ended.cancel();
            return;
        }
    }
}

/// The TCP connection of one client, as the TURN code of the crate writes
/// to it. The crate pads each ChannelData message it makes to four bytes,
/// as TCP needs.
struct ClientConnection {
    local: SocketAddr,
    client: SocketAddr,
    queue: mpsc::Sender<Vec<u8>>,
}

#[async_trait]
impl Conn for ClientConnection {
    async fn connect(&self, _address: SocketAddr) -> webrtc_util::Result<()> {
        Err(not_read_here())
    }

    async fn recv(&self, _buffer: &mut [u8]) -> webrtc_util::Result<usize> {
        Err(not_read_here())
    }

    async fn recv_from(&self, _buffer: &mut [u8]) -> webrtc_util::Result<(usize, SocketAddr)> {
        Err(not_read_here())
    }

    async fn send(&self, message: &[u8]) -> webrtc_util::Result<usize> {
        self.send_to(message, self.client).await
    }

    /// Queue `message` for the client. Media that finds the queue full is
    /// dropped. A STUN message waits for room, because a client over TCP
    /// does not send its request again (RFC 8489 section 6.2.2).
    async fn send_to(&self, message: &[u8], target: SocketAddr) -> webrtc_util::Result<usize> {
        if target != self.client {
            return Err(webrtc_util::Error::Other(format!(
                "the connection of {} carries no message for {target}",
                self.client
            )));
        }
        let channel_data = message.first().is_some_and(|first| first >> 6 == 0b01);
        let queued = if channel_data {
            match self.queue.try_send(message.to_vec()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
                Err(mpsc::error::TrySendError::Closed(_)) => Err(()),
            }
        } else {
            self.queue.send(message.to_vec()).await.map_err(|_| ())
        };
        queued
            .map(|()| message.len())
            .map_err(|()| webrtc_util::Error::ErrUseClosedNetworkConn)
    }

    fn local_addr(&self) -> webrtc_util::Result<SocketAddr> {
        Ok(self.local)
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        Some(self.client)
    }

    /// Nothing: the connection ends when its client or the server ends
    /// it, and not when an allocation on it closes.
    async fn close(&self) -> webrtc_util::Result<()> {
        Ok(())
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

/// Why a `Conn` of this module answers a call that the crate never makes
/// on it.
fn not_read_here() -> webrtc_util::Error {
    webrtc_util::Error::Other("the TURN server reads its connections itself".to_string())
}

/// The relay sockets of the server: each binds the Media Relay's address,
/// at a port outside the Media Relay's range.
struct RelaySockets {
    peers: Arc<MediaRelayPeers>,
}

#[async_trait]
impl RelayAddressGenerator for RelaySockets {
    fn validate(&self) -> Result<(), turn::Error> {
        Ok(())
    }

    async fn allocate_conn(
        &self,
        use_ipv4: bool,
        requested_port: u16,
    ) -> Result<(Arc<dyn Conn + Send + Sync>, SocketAddr), turn::Error> {
        let address = self.peers.address;
        if use_ipv4 != address.is_ipv4() {
            return Err(turn::Error::Other(format!(
                "the Media Relay has the address {address}, of the other family"
            )));
        }
        // A relay socket in the Media Relay's range would be a peer of
        // every other allocation, and a port that the Media Relay cannot
        // open for a viewer. The sockets that land there stay bound until
        // the search ends, so the system gives another port each time.
        let mut in_range = Vec::new();
        for _ in 0..RELAY_BIND_ATTEMPTS {
            let socket = UdpSocket::bind(SocketAddr::new(address, requested_port)).await?;
            let relayed = socket.local_addr()?;
            if !self.peers.ports.contains(&relayed.port()) {
                let socket = RelaySocket {
                    socket,
                    peers: Arc::clone(&self.peers),
                    closed: CancellationToken::new(),
                };
                return Ok((Arc::new(socket), relayed));
            }
            if requested_port != 0 {
                break;
            }
            in_range.push(socket);
        }
        Err(turn::Error::Other(format!(
            "no relay port of {address} outside the Media Relay's range is free"
        )))
    }
}

/// The relay socket of one allocation. It sends to the Media Relay alone,
/// and it drops each datagram that does not come from the Media Relay.
struct RelaySocket {
    socket: UdpSocket,
    peers: Arc<MediaRelayPeers>,
    closed: CancellationToken,
}

#[async_trait]
impl Conn for RelaySocket {
    async fn connect(&self, _address: SocketAddr) -> webrtc_util::Result<()> {
        Err(not_read_here())
    }

    async fn recv(&self, _buffer: &mut [u8]) -> webrtc_util::Result<usize> {
        Err(not_read_here())
    }

    async fn recv_from(&self, buffer: &mut [u8]) -> webrtc_util::Result<(usize, SocketAddr)> {
        loop {
            let (read, from) = tokio::select! {
                () = self.closed.cancelled() => {
                    return Err(webrtc_util::Error::ErrUseClosedNetworkConn);
                }
                received = self.socket.recv_from(buffer) => received?,
            };
            if self.peers.allows(from) {
                return Ok((read, from));
            }
        }
    }

    async fn send(&self, _message: &[u8]) -> webrtc_util::Result<usize> {
        Err(not_read_here())
    }

    async fn send_to(&self, datagram: &[u8], target: SocketAddr) -> webrtc_util::Result<usize> {
        if !self.peers.allows(target) {
            return Err(webrtc_util::Error::Other(format!(
                "the TURN server of Remote Access relays to the Media Relay alone, and not to \
                 {target}"
            )));
        }
        Ok(self.socket.send_to(datagram, target).await?)
    }

    fn local_addr(&self) -> webrtc_util::Result<SocketAddr> {
        Ok(self.socket.local_addr()?)
    }

    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }

    async fn close(&self) -> webrtc_util::Result<()> {
        self.closed.cancel();
        Ok(())
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    fn loopback(port: u16) -> SocketAddr {
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)
    }

    fn media_relay() -> MediaRelayPeers {
        MediaRelayPeers::new(Ipv4Addr::LOCALHOST.into(), 50000..=50099)
    }

    /// The Media Relay's address on each port of its range is a peer.
    #[test]
    fn the_media_relay_range_is_a_peer() {
        let peers = media_relay();

        for port in [50000, 50042, 50099] {
            assert!(peers.allows(loopback(port)), "{port}");
        }
    }

    /// Nothing else on this machine is a peer: another port of loopback,
    /// another loopback address, another address of the machine, and
    /// another family.
    #[test]
    fn every_other_address_and_port_is_refused() {
        let peers = media_relay();

        for refused in [
            loopback(4400),
            loopback(4401),
            loopback(49999),
            loopback(50100),
            "127.0.0.2:50000".parse().unwrap(),
            "192.168.1.20:50000".parse().unwrap(),
            "0.0.0.0:50000".parse().unwrap(),
            "[::1]:50000".parse().unwrap(),
            "[::ffff:127.0.0.1]:50000".parse().unwrap(),
        ] {
            assert!(!peers.allows(refused), "{refused}");
        }
    }

    /// A credential that the server mints has a password the server
    /// gives back for its username, and the key it signs with is the key
    /// of that password (RFC 8489 section 9.2.2).
    #[test]
    fn a_minted_credential_authenticates() {
        let credentials = Credentials::generate();
        let (username, password) = credentials.mint();

        let key = credentials
            .auth_handle(&username, REALM, loopback(1))
            .expect("the username is good");

        assert_eq!(key, generate_auth_key(&username, REALM, &password));
    }

    /// A username lives until its expiry and not after it.
    #[test]
    fn a_credential_ends_at_its_expiry() {
        let credentials = Credentials::generate();
        let (username, password) = credentials.mint();
        let expiry: i64 = username.split_once(':').unwrap().0.parse().unwrap();
        let now = pagis_core::now_ms() / 1000;
        let ttl = CREDENTIAL_TTL.as_secs() as i64;
        assert!(expiry > now + ttl - 60 && expiry <= now + ttl, "{expiry}");

        assert_eq!(credentials.password(&username, expiry - 1), Ok(password));
        assert!(credentials.password(&username, expiry).is_err());
        assert!(credentials.password(&username, expiry + 3600).is_err());
    }

    /// The secret is the server's own: a credential of another server, or
    /// of the external TURN server, gives another key, and a username that
    /// does not start with an expiry gives none.
    #[test]
    fn a_credential_of_another_secret_or_shape_does_not_authenticate() {
        let ours = Credentials::generate();
        let theirs = Credentials::generate();
        let (username, password) = theirs.mint();

        let key = ours.auth_handle(&username, REALM, loopback(1)).unwrap();

        assert_ne!(key, generate_auth_key(&username, REALM, &password));
        for username in ["", "owner", "later:session", "-:session"] {
            assert!(ours.password(username, 0).is_err(), "{username:?}");
        }
    }

    /// Each server makes a secret of its own, and each credential a
    /// username of its own.
    #[test]
    fn each_server_and_each_credential_is_its_own() {
        let first = Credentials::generate();
        let second = Credentials::generate();
        assert_ne!(first.secret, second.secret);
        assert_eq!(first.secret.len(), 64);

        assert_ne!(first.mint().0, first.mint().0);
    }

    fn stun(attributes: usize) -> Vec<u8> {
        let mut message = vec![0x00, 0x03];
        message.extend_from_slice(&(attributes as u16).to_be_bytes());
        message.extend_from_slice(&[0x21, 0x12, 0xa4, 0x42]);
        message.extend_from_slice(&[7; 12]);
        message.extend(std::iter::repeat_n(0, attributes));
        message
    }

    fn channel_data(data: usize, padding: usize) -> Vec<u8> {
        let mut message = vec![0x40, 0x00];
        message.extend_from_slice(&(data as u16).to_be_bytes());
        message.extend(std::iter::repeat_n(9, data));
        message.extend(std::iter::repeat_n(0, padding));
        message
    }

    /// A STUN message takes its header and its attributes.
    #[test]
    fn a_stun_message_is_its_header_and_its_attributes() {
        let message = stun(8);

        assert_eq!(
            frame(&message),
            Ok(Some(Frame {
                message: 28,
                taken: 28
            }))
        );
    }

    /// Over TCP a ChannelData message takes its padding, and the message
    /// that the crate reads ends before it (RFC 8656 section 12.5).
    #[test]
    fn a_channel_data_message_takes_its_padding() {
        let mut stream = channel_data(5, 3);
        stream.extend(stun(0));

        assert_eq!(
            frame(&stream),
            Ok(Some(Frame {
                message: 9,
                taken: 12
            }))
        );
        assert_eq!(
            frame(&channel_data(8, 0)),
            Ok(Some(Frame {
                message: 12,
                taken: 12
            }))
        );
    }

    /// A message waits until all of it is there, its padding too.
    #[test]
    fn a_message_waits_for_all_of_its_bytes() {
        assert_eq!(frame(&[]), Ok(None));
        assert_eq!(frame(&[0x00, 0x01, 0x00]), Ok(None));
        assert_eq!(frame(&stun(8)[..27]), Ok(None));
        assert_eq!(frame(&channel_data(5, 0)), Ok(None));
        assert_eq!(frame(&channel_data(5, 2)), Ok(None));
    }

    /// A stream that is not TURN ends the connection: a first byte of
    /// neither kind, a STUN header without the magic cookie, a STUN
    /// length that is not whole words, and a message longer than the
    /// server reads.
    #[test]
    fn a_stream_that_is_not_turn_is_refused() {
        assert!(frame(&[0x80, 0x60, 0x00, 0x04]).is_err());
        assert!(frame(&[0xc0, 0x00, 0x00, 0x04]).is_err());
        // The start of a TLS ClientHello: the Funnel ends TLS, so this
        // is a client that skipped it.
        assert!(frame(&[0x16, 0x03, 0x01, 0x00, 0xf8, 0x01, 0x00, 0x00, 0xf4]).is_err());
        assert!(frame(b"GET / HTTP/1.1\r\n").is_err());
        assert!(frame(&[0x00, 0x01, 0x00, 0x05]).is_err());
        assert!(frame(&[0x00, 0x01, 0x10, 0x00]).is_err());
        assert!(frame(&[0x40, 0x00, 0x10, 0x00]).is_err());
    }

    /// The nonces stay under the bound: the expired ones go first, then
    /// the oldest half.
    #[test]
    fn the_nonces_stay_under_the_bound() {
        let now = Instant::now() + NONCE_LIFETIME * 2;
        let mut nonces: HashMap<String, Instant> = (0..NONCES_MAX)
            .map(|index| {
                (
                    format!("nonce-{index}"),
                    now - Duration::from_secs(index as u64),
                )
            })
            .collect();

        bound_nonces(&mut nonces, now);

        assert_eq!(nonces.len(), NONCES_MAX / 2);
        assert!(nonces.contains_key("nonce-0"));
        assert!(!nonces.contains_key(&format!("nonce-{}", NONCES_MAX - 1)));

        let mut stale: HashMap<String, Instant> = (0..NONCES_MAX)
            .map(|index| {
                let given = if index < 10 {
                    now
                } else {
                    now - NONCE_LIFETIME
                };
                (format!("nonce-{index}"), given)
            })
            .collect();

        bound_nonces(&mut stale, now);

        assert_eq!(stale.len(), 10);
    }

    /// Under the bound, nothing goes.
    #[test]
    fn nonces_under_the_bound_stay() {
        let now = Instant::now() + NONCE_LIFETIME * 2;
        let mut nonces: HashMap<String, Instant> = (0..NONCES_MAX - 1)
            .map(|index| (format!("nonce-{index}"), now - NONCE_LIFETIME * 2))
            .collect();

        bound_nonces(&mut nonces, now);

        assert_eq!(nonces.len(), NONCES_MAX - 1);
    }
}
