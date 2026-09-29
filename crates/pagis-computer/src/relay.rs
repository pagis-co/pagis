//! The Media Relay (ADR-0014): how a browser reaches one
//! Computer's screen.
//!
//! A Computer publishes no media port, so no browser addresses it.
//! [`MediaRelay`] is the seam between the two: it opens a path and
//! answers with the address the browser sends media to, plus the ICE
//! servers the browser configures first. The screen path calls this
//! seam and never a concrete relay, so the deployment picks the
//! implementation and the screen code does not change.
//!
//! Two implementations ship. [`DaemonRelay`] is the default: the daemon
//! owns one advertised address and one UDP port range, and forwards the
//! packets itself, which is what a local installation and a small
//! server need and costs no second service. [`TurnRelay`] puts an
//! external TURN server (coturn) in front of the browser leg with
//! credentials the daemon mints for each session, which is the path for
//! a deployment whose firewall or scale makes a daemon-owned public
//! port range wrong.
//!
//! Both are built on [`MediaForwarder`]. The forwarder is a TURN
//! allocation without the protocol: one UDP socket for each viewer
//! session, the Computer's address learned from its registration,
//! the browser's addresses learned from the ICE checks it signs with the
//! pipeline's credentials, and the packets copied between the browser
//! and the container in both directions. One socket for each session
//! and not one for each Computer, because the pipeline demultiplexes
//! DTLS and RTP by source address: two viewers behind one socket would
//! reach it as one peer.

use std::collections::VecDeque;
use std::io::{IoSlice, IoSliceMut};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::ops::{Deref, RangeInclusive};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use nix::libc;
use nix::sys::socket::{
    ControlMessage, ControlMessageOwned, MsgFlags, SockaddrIn, recvmsg, sendmsg, setsockopt,
    sockopt,
};
use str0m::ice::StunMessage;
use tokio::io::Interest;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio_util::sync::{CancellationToken, DropGuard};

/// How long a media path may stay silent before the relay frees its
/// port. Only the registered Computer and the checks the browser signs
/// break the silence: a browser sends a consent check every few seconds
/// (RFC 7675), and the pipeline repeats its registration while its
/// session lives. Silence for this long means the viewer is gone,
/// whatever a stranger sends to the port.
pub const PATH_IDLE: Duration = Duration::from_secs(30);

/// The largest datagram the relay copies. The pipeline writes RTP
/// inside one MTU, and a STUN or DTLS record is smaller still.
const DATAGRAM_MAX: usize = 2048;

/// One ICE server a browser configures before it makes its offer.
/// The `daemon` relay needs none; the `turn` relay names the
/// TURN server and the credentials it minted for this session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceServer {
    /// The TURN or STUN URLs of one server, such as
    /// `turn:relay.example.net:3478?transport=udp`.
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
}

/// The start of the datagram a Computer's pipeline registers a path
/// with; the path's token follows it. Its first byte is in the
/// range RFC 7983 leaves to no protocol, so no STUN, DTLS or RTP packet
/// reads as a registration. screend holds the same bytes.
pub const REGISTRATION_PREFIX: &[u8] = b"pagis-register:";

/// The media path of one viewer session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPath {
    /// The `ip:port` the pipeline advertises as its ICE candidate, and
    /// the address the browser sends media to.
    pub candidate: String,
    /// The relay port the Computer's pipeline registers with.
    /// The daemon names the host the container reaches it at.
    pub port: u16,
    /// The secret of this path. The pipeline sends it in its
    /// registration, and the relay takes the container leg from nobody
    /// who does not hold it.
    pub token: String,
}

impl MediaPath {
    /// The datagram the pipeline registers this path with.
    pub fn registration(&self) -> Vec<u8> {
        [REGISTRATION_PREFIX, self.token.as_bytes()].concat()
    }
}

/// The ICE credentials of the pipeline's answer: its `a=ice-ufrag` and
/// `a=ice-pwd`. The browser signs each of its checks with them
/// (RFC 8445 section 7.2.2), so a check that carries them comes from
/// the browser that holds the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceCredentials {
    pub ufrag: String,
    pub pwd: String,
}

impl IceCredentials {
    /// The credentials of an SDP answer, or `None` when it has none.
    /// A bundled answer gives every media section the same pair, so the
    /// first of each is the pair of the whole session.
    pub fn of_answer(sdp: &str) -> Option<Self> {
        let value = |name: &str| {
            sdp.lines()
                .find_map(|line| line.trim_end().strip_prefix(name))
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        Some(Self {
            ufrag: value("a=ice-ufrag:")?,
            pwd: value("a=ice-pwd:")?,
        })
    }
}

/// One open media path, and the handle that controls it. The pipeline
/// reads the [`MediaPath`] through it. Dropping the handle closes the
/// path.
#[derive(Debug)]
pub struct OpenPath {
    path: MediaPath,
    credentials: Arc<OnceLock<IceCredentials>>,
    closed: DropGuard,
    task: JoinHandle<()>,
}

impl OpenPath {
    /// Accept the browser's checks under the pipeline's `credentials`.
    /// Before this, the path accepts no browser address. A path takes
    /// the credentials of one answer, so a second call changes nothing.
    pub fn authenticate(&self, credentials: IceCredentials) {
        let _ = self.credentials.set(credentials);
    }

    /// Close the path now. Its port is free when this returns.
    pub async fn close(self) {
        let Self { closed, task, .. } = self;
        drop(closed);
        if let Err(error) = task.await {
            tracing::error!(%error, "the media path ended with an error");
        }
    }
}

impl Deref for OpenPath {
    type Target = MediaPath;

    fn deref(&self) -> &MediaPath {
        &self.path
    }
}

/// How media reaches a browser (ADR-0014). The screen path holds
/// one of these and knows nothing else about the media path.
#[async_trait]
pub trait MediaRelay: Send + Sync {
    /// Open a media path between one browser and one Computer. The
    /// Computer's pipeline registers with the path from inside its
    /// Tenant Network, so the relay learns its address from the
    /// container and no host port is published for media. The path
    /// closes when `closed` is cancelled, when its handle closes it or
    /// drops, and when it falls silent. The daemon cancels `closed`
    /// when the Session that asked for the path ends.
    async fn open(&self, closed: CancellationToken) -> Result<OpenPath, String>;

    /// The ICE servers a browser configures before it offers. The
    /// answer is minted for each call, because a relay that needs
    /// credentials mints them for one session.
    fn ice_servers(&self) -> Vec<IceServer>;
}

/// The UDP forwarder both relays put between a browser and a Computer:
/// one advertised address, one port range, and one socket for
/// each viewer session.
pub struct MediaForwarder {
    /// The address browsers reach this daemon at. A local installation
    /// leaves it at loopback; a server names the address its viewers
    /// resolve, and a `turn` deployment names the address its TURN
    /// server reaches the daemon at.
    advertise: String,
    first: u16,
    last: u16,
    idle: Duration,
    /// Where the next search for a free port starts, so two sessions in
    /// a row take two ports and a freed port is not reused at once.
    next: AtomicU32,
}

impl MediaForwarder {
    /// The forwarder of one deployment. `ports` is the whole UDP range
    /// the daemon may bind for media; one port serves one viewer
    /// session, so the range bounds how many viewers watch at once.
    pub fn new(advertise: String, ports: RangeInclusive<u16>) -> Self {
        Self {
            advertise,
            first: *ports.start(),
            last: *ports.end(),
            idle: PATH_IDLE,
            next: AtomicU32::new(0),
        }
    }

    /// The same, with a shorter silence before a path frees its port.
    /// The tests use it; production keeps [`PATH_IDLE`].
    pub fn with_idle(mut self, idle: Duration) -> Self {
        self.idle = idle;
        self
    }

    /// Open one path and start copying packets on it until it closes
    /// or falls silent.
    async fn open(&self, closed: CancellationToken) -> Result<OpenPath, String> {
        let socket = self.bind_in_range().await?;
        let port = socket
            .local_addr()
            .map_err(|error| format!("the relay socket has no address: {error}"))?
            .port();
        let path = MediaPath {
            candidate: format!("{}:{port}", self.advertise),
            port,
            token: new_token(),
        };
        // The handle closes this path alone, and never the Session
        // whose end also closes it.
        let closed = closed.child_token();
        let credentials = Arc::new(OnceLock::new());
        let task = tokio::spawn(copy_both_ways(
            socket,
            path.registration(),
            Arc::clone(&credentials),
            self.idle,
            closed.clone(),
        ));
        Ok(OpenPath {
            path,
            credentials,
            closed: closed.drop_guard(),
            task,
        })
    }

    /// One free UDP port of the range. The operating system is the
    /// pool: a port a live session holds refuses the bind, and a port
    /// whose session ended is free again. The search starts where the
    /// last one did not, so a port that just closed is the last one
    /// tried and not the first.
    async fn bind_in_range(&self) -> Result<UdpSocket, String> {
        let span = u32::from(self.last - self.first) + 1;
        let start = self.next.fetch_add(1, Ordering::Relaxed) % span;
        for step in 0..span {
            let port = self.first + ((start + step) % span) as u16;
            if let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).await {
                return Ok(socket);
            }
        }
        Err(format!(
            "every media port of {}-{} is in use; widen screen.media_port_first..last \
             or wait for a viewer to leave",
            self.first, self.last
        ))
    }
}

/// A fresh path token: sixteen random bytes, hex-encoded.
fn new_token() -> String {
    let mut bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut bytes);
    hex::encode(bytes)
}

/// Copy one viewer session's packets until `closed` is cancelled or the
/// session falls silent. The socket goes with the task, so no packet
/// crosses a closed path and its port is free again.
///
/// The Computer's address is the source of the last datagram that
/// carried this path's registration. The pipeline sends it from
/// its media socket, so the relay reaches the pipeline through the NAT
/// of the Docker host and no host port is published for media. A
/// registration is never forwarded, and one with another token is
/// ignored.
///
/// Everything else comes from a browser or from a stranger, and
/// [`BrowserLeg`] tells them apart with the pipeline's ICE `credentials`.
/// It also learns where each packet of the Computer goes back to. The
/// silence ends only for the registration, the Computer's packets and
/// the browser's signed checks, which is the consent rule of ICE
/// (RFC 7675) and the refresh rule of TURN (RFC 8656).
/// The UDP socket of one path. Each read says which address of this
/// machine the datagram was sent to, and each send names the address it
/// leaves from.
///
/// The socket listens on every address, so the system would pick the
/// source of a reply by its route to the receiver. A browser on this
/// machine can check the relay's loopback candidate from an interface
/// address; the route back to that address starts at the interface, so
/// the reply would come from an address the browser never sent to, and
/// the browser drops it (RFC 8445 section 7.2.5.2.1). The relay sends
/// each datagram from the address its receiver last sent to.
struct PathSocket {
    io: UdpSocket,
}

impl PathSocket {
    fn new(io: UdpSocket) -> std::io::Result<Self> {
        setsockopt(&io, sockopt::Ipv4PacketInfo, &true)?;
        Ok(Self { io })
    }

    /// Read one datagram into `buffer`: its length, its sender, and the
    /// address of this machine it was sent to.
    async fn recv(&self, buffer: &mut [u8]) -> std::io::Result<(usize, Peer)> {
        loop {
            self.io.readable().await?;
            let read = self.io.try_io(Interest::READABLE, || {
                let mut control = nix::cmsg_space!(libc::in_pktinfo);
                let mut data = [IoSliceMut::new(buffer)];
                let message = recvmsg::<SockaddrIn>(
                    std::os::fd::AsRawFd::as_raw_fd(&self.io),
                    &mut data,
                    Some(&mut control),
                    MsgFlags::empty(),
                )?;
                let from = message.address.ok_or(std::io::ErrorKind::InvalidData)?;
                let local = message.cmsgs()?.find_map(|control| match control {
                    ControlMessageOwned::Ipv4PacketInfo(info) => {
                        Some(Ipv4Addr::from(u32::from_be(info.ipi_addr.s_addr)))
                    }
                    _ => None,
                });
                Ok((
                    message.bytes,
                    Peer {
                        address: SocketAddr::V4(SocketAddrV4::from(from)),
                        local,
                    },
                ))
            });
            match read {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                read => return read,
            }
        }
    }

    /// Send `datagram` to `to`, from the address of this machine that
    /// `to` sends to.
    async fn send(&self, datagram: &[u8], to: Peer) -> std::io::Result<()> {
        let SocketAddr::V4(address) = to.address else {
            return Err(std::io::ErrorKind::Unsupported.into());
        };
        let destination = SockaddrIn::from(address);
        loop {
            self.io.writable().await?;
            let sent = self.io.try_io(Interest::WRITABLE, || {
                let source = to.local.map(|local| libc::in_pktinfo {
                    ipi_ifindex: 0,
                    ipi_spec_dst: libc::in_addr {
                        s_addr: u32::from(local).to_be(),
                    },
                    ipi_addr: libc::in_addr { s_addr: 0 },
                });
                let control: Vec<ControlMessage<'_>> =
                    source.iter().map(ControlMessage::Ipv4PacketInfo).collect();
                sendmsg(
                    std::os::fd::AsRawFd::as_raw_fd(&self.io),
                    &[IoSlice::new(datagram)],
                    &control,
                    MsgFlags::empty(),
                    Some(&destination),
                )?;
                Ok(())
            });
            match sent {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                sent => return sent,
            }
        }
    }
}

/// One peer of a path: where it sends from, and the address of this
/// machine it sends to, which the relay answers it from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Peer {
    address: SocketAddr,
    local: Option<Ipv4Addr>,
}

async fn copy_both_ways(
    socket: UdpSocket,
    registration: Vec<u8>,
    credentials: Arc<OnceLock<IceCredentials>>,
    idle: Duration,
    closed: CancellationToken,
) {
    let socket = match PathSocket::new(socket) {
        Ok(socket) => socket,
        Err(error) => {
            tracing::error!(%error, "the media path cannot read the addresses of its datagrams");
            return;
        }
    };
    let mut buffer = vec![0u8; DATAGRAM_MAX];
    let mut computer: Option<Peer> = None;
    let mut browser = BrowserLeg::default();
    let mut silent_at = tokio::time::Instant::now() + idle;
    loop {
        let received = tokio::select! {
            // A datagram that arrived at the same time as the close is
            // not copied.
            biased;
            () = closed.cancelled() => return,
            received = tokio::time::timeout_at(silent_at, socket.recv(&mut buffer)) => received,
        };
        let Ok(received) = received else {
            // Silence: the viewer left, and the port goes back.
            return;
        };
        let (read, from) = match received {
            Ok(received) => received,
            Err(error) => {
                tracing::debug!(%error, "the media path closed");
                return;
            }
        };
        let datagram = &buffer[..read];
        if datagram.starts_with(REGISTRATION_PREFIX) {
            if datagram == registration.as_slice() {
                computer = Some(from);
                silent_at = tokio::time::Instant::now() + idle;
            }
            continue;
        }
        let to = if computer.is_some_and(|computer| computer.address == from.address) {
            silent_at = tokio::time::Instant::now() + idle;
            match browser.destination(datagram) {
                Some(browser) => browser,
                // The pipeline spoke before any viewer did; nothing
                // to send it to yet.
                None => continue,
            }
        } else {
            match browser.learn(datagram, from, credentials.get()) {
                Sender::Checked => silent_at = tokio::time::Instant::now() + idle,
                Sender::Accepted => {}
                Sender::Stranger => continue,
            }
            match computer {
                Some(computer) => computer,
                // No registration yet. The browser repeats its ICE
                // checks, so a check that finds no Computer is lost
                // and the next one goes through.
                None => continue,
            }
        };
        if let Err(error) = socket.send(datagram, to).await {
            tracing::debug!(%error, to = %to.address, "a media packet did not go out");
            return;
        }
    }
}

/// How many unanswered checks the relay remembers for one path. A
/// browser checks each of its pairs every few seconds, and the
/// pipeline answers at once, so a handful are open at any time.
const OPEN_CHECKS_MAX: usize = 64;

/// How many addresses of one browser the relay accepts at once. A
/// browser checks one pair for each of its candidates, and it keeps
/// checking the pairs it uses, so the address that checked last is the
/// one the relay forgets last.
const BROWSER_ADDRESSES_MAX: usize = 16;

/// What one datagram that is not the Computer's says about its sender.
#[derive(Debug, PartialEq, Eq)]
enum Sender {
    /// It is a check signed with the pipeline's credentials.
    Checked,
    /// Its sender sent a signed check before.
    Accepted,
    /// Anybody else. The relay drops the datagram.
    Stranger,
}

/// The browser side of one path: which addresses are the browser's,
/// and where each packet of the Computer goes back to.
///
/// An address is the browser's after it sends a check signed with the
/// pipeline's ICE credentials: a STUN Binding request whose USERNAME
/// starts with the pipeline's ice-ufrag and whose MESSAGE-INTEGRITY is
/// correct for the pipeline's ice-pwd (RFC 8445 section 7.2.2). Only
/// the browser that holds the answer has the password. The relay drops
/// every datagram of every other address, so a stranger who finds the
/// port moves no media, reaches no pipeline, and keeps no path open.
///
/// A browser can reach the relay from several addresses at once. It
/// gathers a host candidate for each network interface when the page
/// holds a camera or microphone grant, and it checks every pair it
/// forms until consent ends. Among its own addresses, the relay routes
/// the way a symmetric RTP relay does, by what the browser says and not
/// by who spoke last:
///
/// - An answer to an ICE check goes to the address that sent that
///   check, found by its STUN transaction id. The browser keeps
///   consent on its selected pair only while those answers arrive.
/// - Media goes to the selected pair: the address of the last signed
///   check that nominated its pair (USE-CANDIDATE), or the last of the
///   browser's addresses that sent media, whichever came later. Before
///   either, it goes to the address of the last signed check.
#[derive(Default)]
struct BrowserLeg {
    /// The addresses that sent a signed check, the most recent last.
    accepted: VecDeque<Peer>,
    selected: Option<Peer>,
    last_check: Option<Peer>,
    open_checks: VecDeque<([u8; 12], Peer)>,
}

impl BrowserLeg {
    /// Learn from one datagram that `from` sent. `credentials` are the
    /// pipeline's, from the time the daemon gives them to the path.
    fn learn(
        &mut self,
        datagram: &[u8],
        from: Peer,
        credentials: Option<&IceCredentials>,
    ) -> Sender {
        if let Some(check) = credentials.and_then(|credentials| credentials.check(datagram)) {
            self.accepted.retain(|peer| peer.address != from.address);
            if self.accepted.len() == BROWSER_ADDRESSES_MAX {
                self.accepted.pop_front();
            }
            self.accepted.push_back(from);
            if self.open_checks.len() == OPEN_CHECKS_MAX {
                self.open_checks.pop_front();
            }
            self.open_checks.push_back((check.transaction, from));
            self.last_check = Some(from);
            if check.nominates {
                self.selected = Some(from);
            }
            return Sender::Checked;
        }
        if !self
            .accepted
            .iter()
            .any(|peer| peer.address == from.address)
        {
            return Sender::Stranger;
        }
        if Stun::parse(datagram).is_none() {
            self.selected = Some(from);
        }
        Sender::Accepted
    }

    /// Where a packet of the Computer goes.
    fn destination(&mut self, datagram: &[u8]) -> Option<Peer> {
        if let Some(Stun::Other { transaction }) = Stun::parse(datagram) {
            let asked = self
                .open_checks
                .iter()
                .position(|(open, _)| *open == transaction);
            if let Some(index) = asked {
                return self.open_checks.remove(index).map(|(_, from)| from);
            }
        }
        self.selected.or(self.last_check)
    }
}

/// One ICE check signed with the pipeline's credentials.
#[derive(Debug, PartialEq, Eq)]
struct Check {
    transaction: [u8; 12],
    /// The check carries USE-CANDIDATE and nominates its pair.
    nominates: bool,
}

impl IceCredentials {
    /// The check `datagram` holds, when it is a STUN Binding request
    /// whose USERNAME starts with this ufrag and whose MESSAGE-INTEGRITY
    /// is correct for this password. str0m's STUN code reads the message
    /// and verifies the integrity.
    fn check(&self, datagram: &[u8]) -> Option<Check> {
        let Some(Stun::Request { transaction }) = Stun::parse(datagram) else {
            return None;
        };
        // The relay reads datagrams from anybody, and str0m's parser
        // reads a fixed number of bytes for some attributes whatever
        // their length says. A malformed message never reaches it.
        if !Stun::well_formed(datagram) {
            return None;
        }
        // A panic of the parser on a message that is well formed would
        // end the path. The message is then not a check, and the path
        // carries on.
        let message = std::panic::catch_unwind(|| StunMessage::parse(datagram))
            .ok()?
            .ok()?;
        let (ufrag, _) = message.split_username()?;
        let signed = message.is_binding_request()
            && ufrag == self.ufrag
            && message.verify(self.pwd.as_bytes());
        signed.then(|| Check {
            transaction,
            nominates: message.use_candidate(),
        })
    }
}

/// The part of a STUN message (RFC 8489) the relay routes on: the
/// class and the transaction id of its header.
#[derive(Debug, PartialEq, Eq)]
enum Stun {
    /// A request, such as an ICE connectivity or consent check.
    Request { transaction: [u8; 12] },
    /// A response or an indication.
    Other { transaction: [u8; 12] },
}

impl Stun {
    const HEADER: usize = 20;
    const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xA4, 0x42];

    /// The STUN message `datagram` holds, or `None` when it is DTLS,
    /// RTP or anything else. RFC 7983 sorts the protocols by the first
    /// byte, and STUN has 0 to 3 there and the magic cookie after the
    /// length.
    fn parse(datagram: &[u8]) -> Option<Stun> {
        if datagram.len() < Self::HEADER || datagram[0] > 3 || datagram[4..8] != Self::MAGIC_COOKIE
        {
            return None;
        }
        let kind = u16::from_be_bytes([datagram[0], datagram[1]]);
        let transaction: [u8; 12] = datagram[8..20].try_into().ok()?;
        // The class is bits 4 and 8 of the message type; both clear is
        // a request.
        if kind & 0x0110 != 0 {
            return Some(Stun::Other { transaction });
        }
        Some(Stun::Request { transaction })
    }

    /// Whether a STUN message is well formed by RFC 8489: its length
    /// field gives the rest of the datagram in whole words (section 5),
    /// every attribute and its padding fit in the message (section 14),
    /// and every attribute of a fixed size has its size.
    fn well_formed(message: &[u8]) -> bool {
        let Some(mut attributes) = message.get(Self::HEADER..) else {
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
            if !Self::has_its_size(kind, value) {
                return false;
            }
            let Some(rest) = attributes.get(4 + length.next_multiple_of(4)..) else {
                return false;
            };
            attributes = rest;
        }
        true
    }

    /// Whether an attribute value has the size its RFC gives its type.
    /// An attribute of another type can have any size.
    fn has_its_size(kind: u16, value: &[u8]) -> bool {
        match kind {
            // MESSAGE-INTEGRITY (RFC 8489 section 14.5): an HMAC-SHA1.
            0x0008 => value.len() == 20,
            // ERROR-CODE (RFC 8489 section 14.8): four bytes of class
            // and number, then the reason phrase.
            0x0009 => value.len() >= 4,
            // XOR-PEER-ADDRESS and XOR-RELAYED-ADDRESS (RFC 8656
            // sections 18.3 and 18.5), and XOR-MAPPED-ADDRESS (RFC 8489
            // section 14.2): eight bytes for the IPv4 family (0x01), and
            // twenty for the IPv6 family (0x02).
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

/// The default Media Relay: the daemon forwards media itself.
///
/// One advertised address and one UDP port range for the whole
/// installation, which is one firewall rule and one compose entry
/// instead of a host port for every awake Computer. A local
/// installation needs no configuration at all: the advertised address
/// is loopback and the browser is on the same machine.
pub struct DaemonRelay {
    forwarder: MediaForwarder,
}

impl DaemonRelay {
    pub fn new(forwarder: MediaForwarder) -> Self {
        Self { forwarder }
    }
}

#[async_trait]
impl MediaRelay for DaemonRelay {
    async fn open(&self, closed: CancellationToken) -> Result<OpenPath, String> {
        self.forwarder.open(closed).await
    }

    /// None: the browser reaches the advertised address by itself.
    fn ice_servers(&self) -> Vec<IceServer> {
        Vec::new()
    }
}

/// An external TURN server, and how the daemon authenticates to it.
///
/// The credentials follow the TURN REST API long-term credential
/// scheme, which is what coturn's `use-auth-secret` expects and what
/// every hosted TURN service takes: the daemon and the TURN server
/// share one secret, and the daemon mints a username that carries the
/// expiry and a password that is the HMAC of that username. No account
/// exists on the TURN server, and a credential that leaks expires.
#[derive(Debug, Clone)]
pub struct TurnServer {
    /// The TURN URLs the browser tries, such as
    /// `turn:relay.example.net:3478?transport=udp`.
    pub urls: Vec<String>,
    /// The shared secret of the TURN server's `static-auth-secret`.
    pub secret: String,
    /// How long a minted credential lives.
    pub ttl: Duration,
}

/// The Media Relay for a deployment with a TURN server.
///
/// The browser reaches the daemon's forwarder through the TURN server,
/// so the public UDP address and the ports belong to that server and
/// the daemon needs no public port range of its own. The pipeline's own
/// leg stays with the forwarder: it is ice-lite and answers one
/// advertised candidate, so only a full ICE agent inside the container
/// could take the daemon out of the media path, and ADR-0014 records
/// that.
pub struct TurnRelay {
    forwarder: MediaForwarder,
    turn: TurnServer,
}

impl TurnRelay {
    pub fn new(forwarder: MediaForwarder, turn: TurnServer) -> Self {
        Self { forwarder, turn }
    }
}

#[async_trait]
impl MediaRelay for TurnRelay {
    async fn open(&self, closed: CancellationToken) -> Result<OpenPath, String> {
        self.forwarder.open(closed).await
    }

    fn ice_servers(&self) -> Vec<IceServer> {
        let (username, credential) = mint_turn_credential(&self.turn.secret, self.turn.ttl);
        vec![IceServer {
            urls: self.turn.urls.clone(),
            username,
            credential,
        }]
    }
}

/// One TURN REST API credential: the username is the expiry in Unix
/// seconds and a session name, and the password is the base64 HMAC-SHA1
/// of that username under the shared secret.
fn mint_turn_credential(secret: &str, ttl: Duration) -> (String, String) {
    use base64::Engine;
    use hmac::Mac;

    let expiry = pagis_core::now_ms() / 1000 + ttl.as_secs() as i64;
    let mut session = [0u8; 8];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut session);
    let username = format!("{expiry}:{}", hex::encode(session));
    let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(secret.as_bytes())
        .expect("HMAC takes a key of any length");
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    (username, credential)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The username carries the expiry and a session of its own, and
    /// the password is the HMAC of the username the browser sends.
    /// A TURN server that shares the secret recomputes it.
    #[test]
    fn a_minted_credential_is_the_hmac_of_its_own_username() {
        use base64::Engine;
        use hmac::Mac;

        let (username, credential) = mint_turn_credential("shared", Duration::from_secs(600));

        let (expiry, session) = username.split_once(':').expect("expiry and session");
        let expiry: i64 = expiry.parse().expect("the expiry is Unix seconds");
        let now = pagis_core::now_ms() / 1000;
        assert!(
            expiry > now + 500 && expiry <= now + 600,
            "{expiry} vs {now}"
        );
        assert_eq!(session.len(), 16);

        let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(b"shared").unwrap();
        mac.update(username.as_bytes());
        assert_eq!(
            credential,
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
        );
    }

    /// A STUN message with the given type and attributes.
    fn stun(kind: u16, transaction: [u8; 12], attributes: &[u8]) -> Vec<u8> {
        let mut message = Vec::new();
        message.extend_from_slice(&kind.to_be_bytes());
        message.extend_from_slice(&(attributes.len() as u16).to_be_bytes());
        message.extend_from_slice(&Stun::MAGIC_COOKIE);
        message.extend_from_slice(&transaction);
        message.extend_from_slice(attributes);
        message
    }

    /// The sample request of RFC 5769 section 2.1: a Binding request
    /// that another STUN agent signed, with SOFTWARE, PRIORITY,
    /// ICE-CONTROLLED, USERNAME "evtj:h6vY", MESSAGE-INTEGRITY and a
    /// FINGERPRINT after it. Its password is "VOkJxbRl1RmTxUk/WvJxBt".
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

    /// The credentials of the agent that receives the RFC 5769 request:
    /// the first part of its USERNAME, and the password of the RFC.
    fn rfc_5769_receiver() -> IceCredentials {
        IceCredentials {
            ufrag: "evtj".to_string(),
            pwd: "VOkJxbRl1RmTxUk/WvJxBt".to_string(),
        }
    }

    /// The relay verifies a check that another agent signed, and not
    /// only one that str0m wrote.
    #[test]
    fn a_check_that_another_agent_signed_is_verified() {
        assert_eq!(
            rfc_5769_receiver().check(&RFC_5769_REQUEST),
            Some(Check {
                transaction: [
                    0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae
                ],
                nominates: false,
            })
        );
    }

    /// A check is a check of this session only under both halves of the
    /// pipeline's credentials, and only as it was signed.
    #[test]
    fn a_check_under_other_credentials_or_changed_on_the_way_is_refused() {
        let another_password = IceCredentials {
            pwd: "VOkJxbRl1RmTxUk/WvJxBu".to_string(),
            ..rfc_5769_receiver()
        };
        assert_eq!(another_password.check(&RFC_5769_REQUEST), None);
        // The ufrag of the sender, not of the receiver.
        let another_ufrag = IceCredentials {
            ufrag: "h6vY".to_string(),
            ..rfc_5769_receiver()
        };
        assert_eq!(another_ufrag.check(&RFC_5769_REQUEST), None);
        let mut changed = RFC_5769_REQUEST;
        // The last byte of the PRIORITY value.
        changed[47] ^= 0x01;
        assert_eq!(rfc_5769_receiver().check(&changed), None);
    }

    /// The nomination of a signed check reaches the relay.
    #[test]
    fn a_signed_check_that_nominates_its_pair_is_a_nomination() {
        let credentials = rfc_5769_receiver();

        let nominating = crate::fake::ice_check(&credentials, true);
        let checking = crate::fake::ice_check(&credentials, false);

        assert_eq!(
            credentials.check(&nominating).map(|check| check.nominates),
            Some(true)
        );
        assert_eq!(
            credentials.check(&checking).map(|check| check.nominates),
            Some(false)
        );
    }

    /// A USE-CANDIDATE request that nobody signed, a response, and a
    /// malformed message are not checks. The relay reads datagrams from
    /// anybody, so a message that str0m's parser cannot read ends
    /// nothing.
    #[test]
    fn only_a_signed_binding_request_is_a_check() {
        let credentials = rfc_5769_receiver();
        let unsigned = stun(0x0001, [7; 12], &[0x00, 0x25, 0x00, 0x00]);
        let response = stun(0x0101, [7; 12], &[]);
        // A FINGERPRINT with no value, as the last attribute.
        let malformed = stun(0x0001, [7; 12], &[0x80, 0x28, 0x00, 0x00]);

        assert_eq!(credentials.check(&unsigned), None);
        assert_eq!(credentials.check(&response), None);
        assert_eq!(credentials.check(&malformed), None);
        assert_eq!(credentials.check(b"\x80\x60a video frame"), None);
    }

    thread_local! {
        /// The panics of this thread so far.
        static PANICS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// What `run` returns, and how many panics it raised on this thread,
    /// also a panic that a `catch_unwind` inside it stopped. The hook
    /// counts each panic on the thread that raises it and then prints it
    /// with the hook it replaces, so a test on another thread changes no
    /// count here.
    fn panics_during<T>(run: impl FnOnce() -> T) -> (T, usize) {
        static COUNTING: std::sync::Once = std::sync::Once::new();
        COUNTING.call_once(|| {
            let print = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                PANICS.with(|panics| panics.set(panics.get() + 1));
                print(info);
            }));
        });
        let before = PANICS.with(std::cell::Cell::get);
        let value = run();
        (value, PANICS.with(std::cell::Cell::get) - before)
    }

    /// Whether str0m's parser panics on `message`: the message is one
    /// that only the well-formedness check keeps away from it.
    fn str0m_panics_on(message: &[u8]) -> bool {
        let (_, panics) =
            panics_during(|| std::panic::catch_unwind(|| StunMessage::parse(message)).is_err());
        panics == 1
    }

    /// The RFC 5769 request cut before its MESSAGE-INTEGRITY, with
    /// `attribute` last: SOFTWARE, PRIORITY, ICE-CONTROLLED and USERNAME
    /// as a check has them, and the attribute where the signature
    /// starts.
    fn check_ending_with(attribute: &[u8]) -> Vec<u8> {
        let mut message = [&RFC_5769_REQUEST[..76], attribute].concat();
        let length = u16::try_from(message.len() - Stun::HEADER).expect("a short message");
        message[2..4].copy_from_slice(&length.to_be_bytes());
        message
    }

    /// A FINGERPRINT must be four bytes (RFC 8489 section 14.7). The
    /// signed RFC 5769 request with an empty FINGERPRINT in place of
    /// its own is no check, and it never reaches str0m.
    #[test]
    fn a_short_fingerprint_is_refused_before_str0m_reads_it() {
        let mut message = [&RFC_5769_REQUEST[..100], &[0x80, 0x28, 0x00, 0x00][..]].concat();
        message[2..4].copy_from_slice(&84u16.to_be_bytes());
        assert!(str0m_panics_on(&message));

        assert!(!Stun::well_formed(&message));
        assert_eq!(
            panics_during(|| rfc_5769_receiver().check(&message)),
            (None, 0)
        );
    }

    /// An ERROR-CODE holds at least its four bytes of class and number
    /// (RFC 8489 section 14.8).
    #[test]
    fn a_short_error_code_is_refused_before_str0m_reads_it() {
        let message = check_ending_with(&[0x00, 0x09, 0x00, 0x00]);
        assert!(str0m_panics_on(&message));

        assert!(!Stun::well_formed(&message));
        assert_eq!(
            panics_during(|| rfc_5769_receiver().check(&message)),
            (None, 0)
        );
    }

    /// An XOR-MAPPED-ADDRESS is eight bytes for an IPv4 family and
    /// twenty for IPv6 (RFC 8489 section 14.2).
    #[test]
    fn a_short_xor_mapped_address_is_refused_before_str0m_reads_it() {
        for message in [
            // Four bytes that name the IPv6 family.
            check_ending_with(&[0x00, 0x20, 0x00, 0x04, 0x00, 0x02, 0x12, 0x34]),
            // The eight bytes of IPv4 that name the IPv6 family.
            check_ending_with(&[
                0x00, 0x20, 0x00, 0x08, 0x00, 0x02, 0x12, 0x34, 0x01, 0x02, 0x03, 0x04,
            ]),
        ] {
            assert!(str0m_panics_on(&message));

            assert!(!Stun::well_formed(&message));
            assert_eq!(
                panics_during(|| rfc_5769_receiver().check(&message)),
                (None, 0)
            );
        }
    }

    /// Each attribute and its padding fit in the message (RFC 8489
    /// section 14). The signed RFC 5769 request whose USERNAME claims
    /// 200 bytes is no check.
    #[test]
    fn an_attribute_that_runs_past_the_message_is_refused() {
        let mut message = RFC_5769_REQUEST;
        // The length of the USERNAME attribute.
        message[62..64].copy_from_slice(&200u16.to_be_bytes());

        assert!(!Stun::well_formed(&message));
        assert_eq!(
            panics_during(|| rfc_5769_receiver().check(&message)),
            (None, 0)
        );
    }

    /// A check that a browser signs, and the RFC 5769 request, are well
    /// formed.
    #[test]
    fn a_signed_check_is_well_formed() {
        assert!(Stun::well_formed(&RFC_5769_REQUEST));
        assert!(Stun::well_formed(&crate::fake::ice_check(
            &rfc_5769_receiver(),
            true
        )));
    }

    /// The credentials of an answer are its first `a=ice-ufrag` and
    /// `a=ice-pwd`; an answer without both has none.
    #[test]
    fn the_credentials_of_an_answer_are_its_ice_ufrag_and_ice_pwd() {
        let answer = "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\na=ice-lite\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
                      a=ice-ufrag:ZoVx\r\na=ice-pwd:k2mB2iVg3mA3QwXx7yLp1Z\r\n\
                      m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
                      a=ice-ufrag:ZoVx\r\na=ice-pwd:k2mB2iVg3mA3QwXx7yLp1Z\r\n";

        assert_eq!(
            IceCredentials::of_answer(answer),
            Some(IceCredentials {
                ufrag: "ZoVx".to_string(),
                pwd: "k2mB2iVg3mA3QwXx7yLp1Z".to_string(),
            })
        );
        assert_eq!(
            IceCredentials::of_answer("v=0\r\na=ice-ufrag:ZoVx\r\n"),
            None
        );
        assert_eq!(
            IceCredentials::of_answer("v=0\r\na=ice-ufrag:\r\na=ice-pwd:\r\n"),
            None
        );
    }

    /// Answers of both kinds are not requests, and a DTLS record or an
    /// RTP packet is not STUN at all.
    #[test]
    fn only_stun_parses_as_stun() {
        assert_eq!(
            Stun::parse(&stun(0x0101, [1; 12], &[])),
            Some(Stun::Other {
                transaction: [1; 12]
            })
        );
        assert_eq!(
            Stun::parse(&stun(0x0111, [1; 12], &[])),
            Some(Stun::Other {
                transaction: [1; 12]
            })
        );
        let mut dtls = stun(0x0001, [1; 12], &[]);
        dtls[0] = 22;
        assert_eq!(Stun::parse(&dtls), None);
        let mut no_cookie = stun(0x0001, [1; 12], &[]);
        no_cookie[4] = 0;
        assert_eq!(Stun::parse(&no_cookie), None);
        assert_eq!(Stun::parse(b"\x80\x60a video frame"), None);
    }

    /// Two sessions never share a credential, so one that leaks names
    /// one session.
    #[test]
    fn two_sessions_take_two_credentials() {
        let first = mint_turn_credential("shared", Duration::from_secs(600));
        let second = mint_turn_credential("shared", Duration::from_secs(600));

        assert_ne!(first.0, second.0);
        assert_ne!(first.1, second.1);
    }
}
