//! A scripted computer runtime for tests: the daemon and manager
//! tests run Docker-free; the real bollard runtime is covered by
//! `#[ignore]`-tagged tests.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{
    AgentId, ScheduleId, StoreError, UnixMillis, Workspace, WorkspaceId, WorkspaceStore, now_ms,
};

use crate::{
    ComputerRuntime, ExecOutcome, ExecRequest, IMAGE, IMAGE_VERSION, ImageRemoval, InputHolder,
    OtherImage, RunningComputer, StartedComputer,
};

/// What every image call answers while Docker does not answer.
const NO_DOCKER: &str = "cannot reach Docker: no endpoint answered";

/// A Media Relay for tests: the daemon relay on loopback, with
/// the operating system picking each session's port, so parallel test
/// binaries never meet on one.
pub fn loopback_relay() -> std::sync::Arc<dyn crate::MediaRelay> {
    std::sync::Arc::new(crate::DaemonRelay::new(crate::MediaForwarder::new(
        "127.0.0.1".to_string(),
        0..=0,
    )))
}

/// The SDP answer of the fake pipeline. It carries ICE credentials as
/// a real answer does, so a test that plays the browser signs its checks
/// with them ([`ice_check`]).
pub const ANSWER: &str = "v=0\r\na=ice-ufrag:fakeufrag\r\na=ice-pwd:fakepasswordfakepassword\r\n";

/// An ICE connectivity check as a browser sends it (RFC 8445 section
/// 7.2.2): a STUN Binding request whose USERNAME starts with the
/// pipeline's ufrag and whose MESSAGE-INTEGRITY is keyed with the
/// pipeline's password. `nominates` adds USE-CANDIDATE. The transaction
/// id is random, and it is bytes 8 to 20 of the check.
pub fn ice_check(credentials: &crate::IceCredentials, nominates: bool) -> Vec<u8> {
    use str0m::ice::{StunMessageBuilder, TransId};

    let username = format!("{}:browser", credentials.ufrag);
    let mut check = StunMessageBuilder::new()
        .binding()
        .request()
        .username(&username)
        .prio(0x6e00_1eff)
        .ice_controlling(1);
    if nominates {
        check = check.use_candidate();
    }
    let mut buffer = [0u8; 256];
    let length = check
        .build(TransId::new())
        .to_bytes(Some(credentials.pwd.as_bytes()), &mut buffer)
        .expect("a check fits in 256 bytes");
    buffer[..length].to_vec()
}

/// A TURN client over TCP, as a browser runs one through the Funnel of
/// Remote Access (ADR-0028): the client of the `turn` crate over one TCP
/// connection, framed as RFC 8656 section 12.5 says. A test connects it to
/// the loopback port of the TURN server, as `tailscaled` does once it has
/// ended TLS.
pub struct TurnClient {
    connection: std::sync::Arc<FramedTcp>,
    relay: Box<dyn webrtc_util::Conn + Send + Sync>,
    _client: turn::client::Client,
}

impl TurnClient {
    /// Connect to the TURN server at `server`, and allocate a UDP relay
    /// with `username` and `password`.
    pub async fn allocate(
        server: std::net::SocketAddr,
        username: &str,
        password: &str,
    ) -> Result<Self, String> {
        let stream = tokio::net::TcpStream::connect(server)
            .await
            .map_err(|error| format!("cannot connect to the TURN server at {server}: {error}"))?;
        let local = stream.local_addr().map_err(|error| error.to_string())?;
        let (reader, writer) = stream.into_split();
        let connection = std::sync::Arc::new(FramedTcp {
            server,
            local,
            reader: tokio::sync::Mutex::new((reader, bytes::BytesMut::new())),
            writer: tokio::sync::Mutex::new(Some(writer)),
            closed: tokio_util::sync::CancellationToken::new(),
        });
        let client = turn::client::Client::new(turn::client::ClientConfig {
            stun_serv_addr: String::new(),
            turn_serv_addr: server.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            realm: String::new(),
            software: String::new(),
            // TCP carries each request once; a short timer would send
            // it again before the answer comes.
            rto_in_ms: 2000,
            conn: std::sync::Arc::clone(&connection) as _,
            vnet: None,
        })
        .await
        .map_err(|error| error.to_string())?;
        client.listen().await.map_err(|error| error.to_string())?;
        let relay = client
            .allocate()
            .await
            .map_err(|error| format!("the TURN server allocated nothing: {error}"))?;
        Ok(Self {
            connection,
            relay: Box::new(relay),
            _client: client,
        })
    }

    /// The relay address that the server allocated.
    pub fn relayed_address(&self) -> std::net::SocketAddr {
        self.relay
            .local_addr()
            .expect("an allocation has a relay address")
    }

    /// Send `datagram` to `peer` through the relay. The first datagram to
    /// a peer asks for its permission first.
    pub async fn send_to(&self, datagram: &[u8], peer: std::net::SocketAddr) -> Result<(), String> {
        self.relay
            .send_to(datagram, peer)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// The next datagram that a peer sent to the relay, and that peer.
    pub async fn recv_from(&self) -> Result<(Vec<u8>, std::net::SocketAddr), String> {
        let mut buffer = vec![0u8; 2048];
        let (read, from) = self
            .relay
            .recv_from(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        buffer.truncate(read);
        Ok((buffer, from))
    }

    /// Close the TCP connection, as a browser that goes away does, with no
    /// Refresh that ends the allocation first.
    pub async fn hang_up(&self) {
        self.connection.closed.cancel();
        self.connection.writer.lock().await.take();
    }
}

/// The TCP connection of a [`TurnClient`], one TURN message at a time.
struct FramedTcp {
    server: std::net::SocketAddr,
    local: std::net::SocketAddr,
    reader: tokio::sync::Mutex<(tokio::net::tcp::OwnedReadHalf, bytes::BytesMut)>,
    writer: tokio::sync::Mutex<Option<tokio::net::tcp::OwnedWriteHalf>>,
    closed: tokio_util::sync::CancellationToken,
}

#[async_trait]
impl webrtc_util::Conn for FramedTcp {
    async fn connect(&self, _address: std::net::SocketAddr) -> webrtc_util::Result<()> {
        Ok(())
    }

    async fn recv(&self, buffer: &mut [u8]) -> webrtc_util::Result<usize> {
        Ok(self.recv_from(buffer).await?.0)
    }

    async fn recv_from(
        &self,
        buffer: &mut [u8],
    ) -> webrtc_util::Result<(usize, std::net::SocketAddr)> {
        use tokio::io::AsyncReadExt;

        let mut reader = self.reader.lock().await;
        let (reader, stream) = &mut *reader;
        loop {
            let frame =
                crate::remote_access_turn::frame(stream).map_err(webrtc_util::Error::Other)?;
            if let Some(frame) = frame {
                let message = stream.split_to(frame.taken);
                let into = buffer
                    .get_mut(..frame.message)
                    .ok_or(webrtc_util::Error::ErrBufferShort)?;
                into.copy_from_slice(&message[..frame.message]);
                return Ok((frame.message, self.server));
            }
            stream.reserve(2048);
            let read = tokio::select! {
                () = self.closed.cancelled() => 0,
                read = reader.read_buf(stream) => read?,
            };
            if read == 0 {
                return Err(webrtc_util::Error::ErrUseClosedNetworkConn);
            }
        }
    }

    async fn send(&self, message: &[u8]) -> webrtc_util::Result<usize> {
        self.send_to(message, self.server).await
    }

    async fn send_to(
        &self,
        message: &[u8],
        _target: std::net::SocketAddr,
    ) -> webrtc_util::Result<usize> {
        use tokio::io::AsyncWriteExt;

        let mut writer = self.writer.lock().await;
        let writer = writer
            .as_mut()
            .ok_or(webrtc_util::Error::ErrUseClosedNetworkConn)?;
        writer.write_all(message).await?;
        Ok(message.len())
    }

    fn local_addr(&self) -> webrtc_util::Result<std::net::SocketAddr> {
        Ok(self.local)
    }

    fn remote_addr(&self) -> Option<std::net::SocketAddr> {
        Some(self.server)
    }

    async fn close(&self) -> webrtc_util::Result<()> {
        Ok(())
    }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

/// The fake's world: which image is present, and what a live frame
/// returns. Containers "run" in memory; volumes are a name set that
/// survives stop, mirroring Docker's named-volume behavior.
pub struct FakeComputerRuntime {
    state: Mutex<FakeState>,
}

/// What one running container booted with: the image version and the
/// fingerprint of its mount set.
#[derive(Clone)]
struct Booted {
    version: Option<String>,
    mounts: Option<String>,
    ready: bool,
    image_matches: bool,
}

struct FakeState {
    /// Whether Docker answers the image calls.
    docker_answers: bool,
    image_version: Option<String>,
    /// The version label of the image that a pull installs.
    pulled_version: String,
    /// The other images of the Computer Image repository, each with
    /// whether a container uses it.
    old_images: Vec<(OtherImage, bool)>,
    /// Every image the daemon removed, in order.
    removed_images: Vec<String>,
    /// Every image that a pull fetched, in order.
    pulled_images: Vec<String>,
    /// Running containers and what each one booted with.
    running: HashMap<AgentId, Booted>,
    /// The mount set of every start, in order.
    mounts: Vec<Vec<crate::BindMount>>,
    /// The environment of every start, in order.
    start_envs: Vec<Vec<String>>,
    volumes: HashSet<String>,
    /// The owner of every start, in order.
    owners: Vec<crate::ComputerOwner>,
    /// Every streaming exec the daemon asked for, in order.
    exec_streams: Vec<ExecRequest>,
    /// The server side of each streaming exec, for a test to drive.
    server_ends: Vec<tokio::io::DuplexStream>,
    /// The stderr sender of each streaming exec.
    stderr_senders: Vec<tokio::sync::mpsc::Sender<Vec<u8>>>,
    /// Scripted bytes per volume, for the disk figure.
    volume_bytes: u64,
    /// Scripted answer of the container quota.
    container_quota: crate::Quota,
    frame: Vec<u8>,
    /// Frames the next fetches return in order, before `frame`.
    queued_frames: std::collections::VecDeque<Vec<u8>>,
    starts: u32,
    pulls: u32,
    pull_error: Option<String>,
    pull_delay: Duration,
    /// Every pull waits here until a test releases it.
    pull_gate: std::sync::Arc<tokio::sync::Semaphore>,
    inputs: Vec<(InputHolder, crate::exec::InputOp)>,
    fail_input: Option<String>,
    /// The error every live frame fetch answers with, when set.
    fail_frame: Option<String>,
    frames_served: u32,
    /// Relayed offers: the offer sdp and the Media Relay path it came
    /// with.
    offers: Vec<(String, crate::MediaPath)>,
    /// What every offer answers: an SDP answer, or the reason the
    /// pipeline refuses it.
    answer: Result<String, String>,
    /// The pipeline input switch per agent; absent means agent.
    holders: HashMap<AgentId, InputHolder>,
    /// Scripted milliseconds since the last user input.
    user_idle_ms: u64,
    /// Scripted exec answers, returned in order.
    exec_outcomes: VecDeque<ExecOutcome>,
    /// Every exec request the manager sent, in order.
    execs: Vec<ExecRequest>,
    /// How long each exec takes, so a test can race the idle-stop.
    exec_delay: Duration,
    /// Every archive the daemon uploaded: (path, tar bytes).
    uploads: Vec<(String, Vec<u8>)>,
    /// Makes the next upload fail with this message.
    fail_upload: Option<String>,
    /// What the container answers a download with, by path: a new
    /// stream for each download.
    downloads: HashMap<String, DownloadSource>,
    /// Every path the daemon downloaded, in order.
    downloaded: Vec<String>,
    /// The daemon's browser tab.
    browser: FakeBrowser,
    /// The mode of each Exit Proxy: the one its start environment named,
    /// then the one of the last switch.
    exit_modes: HashMap<AgentId, crate::ExitMode>,
    /// Every switch of an Exit Proxy that took effect, in order.
    exit_switches: Vec<(AgentId, crate::ExitMode)>,
    /// The Computers whose Exit Proxy refuses every switch, with the
    /// reason.
    failing_exit_switches: HashMap<AgentId, String>,
}

/// Makes the stream of one download.
type DownloadSource = std::sync::Arc<dyn Fn() -> crate::ArchiveStream + Send + Sync>;

/// The daemon's own tab of the browser, as the fake plays it.
#[derive(Default)]
struct FakeBrowser {
    /// Where an open of one address lands after its redirects. An
    /// address with no entry lands on itself.
    redirects: HashMap<String, String>,
    /// The top-level address the tab shows; `None` while the daemon
    /// has no tab.
    page: Option<String>,
    /// The error every open answers with, when set.
    fail_open: Option<String>,
    /// The error every fill answers with, when set: the reason a page
    /// check gives.
    fail_fill: Option<String>,
    /// Every address the daemon opened, in order.
    opens: Vec<String>,
    /// Every fill the browser took: the origin and the fields.
    fills: Vec<(String, Vec<crate::FillField>)>,
}

impl Default for FakeComputerRuntime {
    fn default() -> Self {
        Self {
            state: Mutex::new(FakeState {
                docker_answers: true,
                image_version: None,
                pulled_version: IMAGE_VERSION.to_string(),
                old_images: Vec::new(),
                removed_images: Vec::new(),
                pulled_images: Vec::new(),
                running: HashMap::new(),
                mounts: Vec::new(),
                start_envs: Vec::new(),
                volumes: HashSet::new(),
                owners: Vec::new(),
                exec_streams: Vec::new(),
                server_ends: Vec::new(),
                stderr_senders: Vec::new(),
                volume_bytes: 0,
                container_quota: crate::Quota::Unknown,
                frame: b"png-frame".to_vec(),
                queued_frames: std::collections::VecDeque::new(),
                starts: 0,
                pulls: 0,
                pull_error: None,
                pull_delay: Duration::ZERO,
                pull_gate: open_gate(),
                inputs: Vec::new(),
                fail_input: None,
                fail_frame: None,
                frames_served: 0,
                offers: Vec::new(),
                answer: Ok(ANSWER.to_string()),
                holders: HashMap::new(),
                user_idle_ms: 0,
                exec_outcomes: VecDeque::new(),
                execs: Vec::new(),
                exec_delay: Duration::ZERO,
                uploads: Vec::new(),
                fail_upload: None,
                downloads: HashMap::new(),
                downloaded: Vec::new(),
                browser: FakeBrowser::default(),
                exit_modes: HashMap::new(),
                exit_switches: Vec::new(),
                failing_exit_switches: HashMap::new(),
            }),
        }
    }
}

impl FakeComputerRuntime {
    /// A fake whose image is already present at the pinned version.
    pub fn with_image() -> Self {
        let fake = Self::default();
        fake.set_image_version(Some(IMAGE_VERSION));
        fake
    }

    pub fn set_image_version(&self, version: Option<&str>) {
        self.state.lock().expect("fake state").image_version = version.map(str::to_string);
    }

    /// Whether Docker answers the image calls from now on. A machine
    /// where Docker is not installed or does not run answers none.
    pub fn set_docker_answers(&self, answers: bool) {
        self.state.lock().expect("fake state").docker_answers = answers;
    }

    /// The version label of the image that the next pull installs, as a
    /// registry that serves another image under the pinned reference.
    pub fn pull_installs(&self, version: &str) {
        self.state.lock().expect("fake state").pulled_version = version.to_string();
    }

    /// One more image of the Computer Image repository beside the pinned
    /// one, of an older version. `in_use` says whether a container uses
    /// it.
    pub fn add_old_image(&self, id: &str, in_use: bool) {
        self.add_other_image(id, Some("0.0.1"), in_use);
    }

    /// One more image of the Computer Image repository beside the pinned
    /// one, with its version label.
    pub fn add_other_image(&self, id: &str, version: Option<&str>, in_use: bool) {
        let image = OtherImage {
            id: id.to_string(),
            version: version.map(str::to_string),
        };
        self.state
            .lock()
            .expect("fake state")
            .old_images
            .push((image, in_use));
    }

    /// The other images of the repository that are still present.
    pub fn old_images(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("fake state")
            .old_images
            .iter()
            .map(|(image, _)| image.id.clone())
            .collect()
    }

    /// Every image that a pull fetched, in order.
    pub fn pulled_images(&self) -> Vec<String> {
        self.state.lock().expect("fake state").pulled_images.clone()
    }

    /// Every image the daemon removed, in order.
    pub fn removed_images(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("fake state")
            .removed_images
            .clone()
    }

    /// How many bytes each Agent volume holds.
    pub fn set_volume_bytes(&self, bytes: u64) {
        self.state.lock().expect("fake state").volume_bytes = bytes;
    }

    /// What the container quota answers from now on. It is
    /// [`crate::Quota::Unknown`] until a test sets it, as it is for a
    /// real runtime before the first wake.
    pub fn set_container_quota(&self, answer: crate::Quota) {
        self.state.lock().expect("fake state").container_quota = answer;
    }

    pub fn set_frame(&self, frame: &[u8]) {
        self.state.lock().expect("fake state").frame = frame.to_vec();
    }

    /// A screen that changes: each next fetch returns the next of
    /// `frames`, and the last one stays as the current frame.
    pub fn play_frames(&self, frames: &[&[u8]]) {
        let mut state = self.state.lock().expect("fake state");
        state.queued_frames = frames.iter().map(|frame| frame.to_vec()).collect();
        if let Some(last) = state.queued_frames.pop_back() {
            state.frame = last;
        }
    }

    pub fn pulls(&self) -> u32 {
        self.state.lock().expect("fake state").pulls
    }

    pub fn fail_pull(&self, message: impl Into<String>) {
        self.state.lock().expect("fake state").pull_error = Some(message.into());
    }

    pub fn set_pull_delay(&self, delay: Duration) {
        self.state.lock().expect("fake state").pull_delay = delay;
    }

    /// Hold every pull after it starts and before it reports progress,
    /// until [`Self::release_pulls`]. A test that must act while a pull
    /// runs holds it, instead of racing a delay.
    pub fn hold_pulls(&self) {
        self.state.lock().expect("fake state").pull_gate =
            std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    }

    /// Let every held pull, and every pull after it, go on.
    pub fn release_pulls(&self) {
        self.state.lock().expect("fake state").pull_gate.close();
    }

    pub fn starts(&self) -> u32 {
        self.state.lock().expect("fake state").starts
    }

    pub fn is_running(&self, agent_id: &AgentId) -> bool {
        self.state
            .lock()
            .expect("fake state")
            .running
            .contains_key(agent_id)
    }

    /// The image version the agent's running container booted from.
    pub fn version(&self, agent_id: &AgentId) -> Option<String> {
        self.state
            .lock()
            .expect("fake state")
            .running
            .get(agent_id)
            .and_then(|booted| booted.version.clone())
    }

    /// The agent's volume survived every stop so far.
    pub fn has_volume(&self, agent_id: &AgentId) -> bool {
        self.state
            .lock()
            .expect("fake state")
            .volumes
            .iter()
            .any(|name| name.ends_with(agent_id.as_str()))
    }

    /// How many live frames were fetched.
    pub fn frames_served(&self) -> u32 {
        self.state.lock().expect("fake state").frames_served
    }

    /// Every input op sent so far, in order.
    pub fn inputs(&self) -> Vec<crate::exec::InputOp> {
        self.state
            .lock()
            .expect("fake state")
            .inputs
            .iter()
            .map(|(_, op)| op.clone())
            .collect()
    }

    /// Every input op sent so far with the holder each batch declared.
    pub fn inputs_with_holder(&self) -> Vec<(InputHolder, crate::exec::InputOp)> {
        self.state.lock().expect("fake state").inputs.clone()
    }

    /// Make the next input batches fail with this message.
    pub fn fail_input(&self, message: &str) {
        self.state.lock().expect("fake state").fail_input = Some(message.to_string());
    }

    /// Make every live frame fetch fail with this message.
    pub fn fail_frame(&self, message: &str) {
        self.state.lock().expect("fake state").fail_frame = Some(message.to_string());
    }

    /// Script what every next offer answers: an SDP answer, or the
    /// reason the pipeline refuses the offer. The pipeline records a
    /// refused offer too.
    pub fn answer_offers_with(&self, answer: Result<&str, &str>) {
        self.state.lock().expect("fake state").answer =
            answer.map(str::to_string).map_err(str::to_string);
    }

    /// Every relayed offer so far: the offer sdp and the Media Relay
    /// path the pipeline registers with. A test that plays the pipeline
    /// registers with the path's token.
    pub fn offers(&self) -> Vec<(String, crate::MediaPath)> {
        self.state.lock().expect("fake state").offers.clone()
    }

    /// The pipeline's input switch for one agent.
    pub fn holder(&self, agent_id: &AgentId) -> InputHolder {
        self.state
            .lock()
            .expect("fake state")
            .holders
            .get(agent_id)
            .copied()
            .unwrap_or(InputHolder::Agent)
    }

    /// Script how long ago the pipeline last applied user input.
    pub fn set_user_idle_ms(&self, idle_ms: u64) {
        self.state.lock().expect("fake state").user_idle_ms = idle_ms;
    }

    /// Script the answer to the next exec. Answers come back in
    /// the order they were pushed; an unscripted exec answers with
    /// exit 0 and no output.
    pub fn push_exec_outcome(&self, outcome: ExecOutcome) {
        self.state
            .lock()
            .expect("fake state")
            .exec_outcomes
            .push_back(outcome);
    }

    /// Make every exec take this long, so a test can run the idle-stop
    /// sweep while a command is in flight.
    pub fn set_exec_delay(&self, delay: Duration) {
        self.state.lock().expect("fake state").exec_delay = delay;
    }

    /// Every exec request so far, in order.
    pub fn execs(&self) -> Vec<ExecRequest> {
        self.state.lock().expect("fake state").execs.clone()
    }

    /// Every uploaded archive so far: (path, tar bytes).
    pub fn uploads(&self) -> Vec<(String, Vec<u8>)> {
        self.state.lock().expect("fake state").uploads.clone()
    }

    /// Make every upload fail with this message.
    pub fn fail_upload(&self, message: impl Into<String>) {
        self.state.lock().expect("fake state").fail_upload = Some(message.into());
    }

    /// What a download of `path` answers with: the whole tar in one
    /// chunk.
    pub fn set_download(&self, path: impl Into<String>, tar: Vec<u8>) {
        let tar = bytes::Bytes::from(tar);
        self.set_download_stream(path, move || {
            Box::pin(futures::stream::iter([Ok(tar.clone())]))
        });
    }

    /// What a download of `path` answers with: the stream that `make`
    /// returns, a new one for each download. A test sends a tar it never
    /// holds whole this way, or holds a download open.
    pub fn set_download_stream(
        &self,
        path: impl Into<String>,
        make: impl Fn() -> crate::ArchiveStream + Send + Sync + 'static,
    ) {
        self.state
            .lock()
            .expect("fake state")
            .downloads
            .insert(path.into(), std::sync::Arc::new(make));
    }

    /// Every path the daemon downloaded so far, in order.
    pub fn downloaded(&self) -> Vec<String> {
        self.state.lock().expect("fake state").downloaded.clone()
    }

    /// An open of `from` lands on `to`, as a redirect does.
    pub fn redirect(&self, from: &str, to: &str) {
        self.state
            .lock()
            .expect("fake state")
            .browser
            .redirects
            .insert(from.to_string(), to.to_string());
    }

    /// The daemon's tab shows `url`, as after the Agent went on from a
    /// fill.
    pub fn set_browser_page(&self, url: &str) {
        self.state.lock().expect("fake state").browser.page = Some(url.to_string());
    }

    /// Make every open fail with this reason, as a page that does not
    /// load does.
    pub fn fail_browser_open(&self, reason: &str) {
        self.state.lock().expect("fake state").browser.fail_open = Some(reason.to_string());
    }

    /// Make every fill fail with this reason, as a page check does.
    pub fn fail_browser_fill(&self, reason: &str) {
        self.state.lock().expect("fake state").browser.fail_fill = Some(reason.to_string());
    }

    /// Every address the daemon opened in its tab, in order.
    pub fn browser_opens(&self) -> Vec<String> {
        self.state.lock().expect("fake state").browser.opens.clone()
    }

    /// Every fill the browser took, in order: the origin and the
    /// fields.
    pub fn browser_fills(&self) -> Vec<(String, Vec<crate::FillField>)> {
        self.state.lock().expect("fake state").browser.fills.clone()
    }

    /// Kill every running container, as an idle-stop or a Docker
    /// restart would.
    pub fn stop_all(&self) {
        self.state.lock().expect("fake state").running.clear();
    }

    /// Simulate a container started outside this daemon process, on
    /// the pinned image.
    pub fn boot_externally(&self, agent_id: &AgentId) {
        self.boot_externally_with_version(agent_id, IMAGE_VERSION);
    }

    pub fn boot_externally_not_ready(&self, agent_id: &AgentId) {
        let mut state = self.state.lock().expect("fake state");
        state.running.insert(
            agent_id.clone(),
            Booted {
                version: Some(IMAGE_VERSION.to_string()),
                mounts: Some(crate::mounts_fingerprint(&[])),
                ready: false,
                image_matches: true,
            },
        );
    }

    /// The same, on a scripted image version.
    pub fn boot_externally_with_version(&self, agent_id: &AgentId, version: &str) {
        self.boot_externally_with(agent_id, version, &[]);
    }

    /// The same, on a scripted image version and mount set.
    pub fn boot_externally_with(
        &self,
        agent_id: &AgentId,
        version: &str,
        mounts: &[crate::BindMount],
    ) {
        let mut state = self.state.lock().expect("fake state");
        state.running.insert(
            agent_id.clone(),
            Booted {
                version: Some(version.to_string()),
                mounts: Some(crate::mounts_fingerprint(mounts)),
                ready: true,
                image_matches: true,
            },
        );
        state.volumes.insert(format!("fake-volume-{agent_id}"));
    }

    /// Simulate the same version label on bytes outside the release's
    /// immutable image reference.
    pub fn set_running_image_matches(&self, agent_id: &AgentId, matches: bool) {
        if let Some(booted) = self
            .state
            .lock()
            .expect("fake state")
            .running
            .get_mut(agent_id)
        {
            booted.image_matches = matches;
        }
    }

    /// The mount set of every start, in order.
    pub fn mounts(&self) -> Vec<Vec<crate::BindMount>> {
        self.state.lock().expect("fake state").mounts.clone()
    }

    /// The environment of every start, in order.
    pub fn start_envs(&self) -> Vec<Vec<String>> {
        self.state.lock().expect("fake state").start_envs.clone()
    }

    /// The owner of every start, in order: which tenant and
    /// which Agent each container belongs to.
    pub fn started_owners(&self) -> Vec<crate::ComputerOwner> {
        self.state.lock().expect("fake state").owners.clone()
    }

    /// Every streaming exec the daemon asked for, in order: the
    /// argv, the uid, the working directory and the environment one
    /// plugin server started with.
    pub fn exec_streams(&self) -> Vec<ExecRequest> {
        self.state.lock().expect("fake state").exec_streams.clone()
    }

    /// The server side of the streaming exec started last, so a test
    /// answers the daemon the way a real MCP server does.
    /// The mode of the Exit Proxy of `agent_id`, as its start environment
    /// or its last switch set it, or `None` before its first start.
    pub fn exit_mode(&self, agent_id: &AgentId) -> Option<crate::ExitMode> {
        self.state
            .lock()
            .expect("fake state")
            .exit_modes
            .get(agent_id)
            .copied()
    }

    /// Every switch of an Exit Proxy that took effect, in order.
    pub fn exit_switches(&self) -> Vec<(AgentId, crate::ExitMode)> {
        self.state.lock().expect("fake state").exit_switches.clone()
    }

    /// Make the Exit Proxy of `agent_id` refuse every switch with
    /// `reason`.
    pub fn fail_exit_switch(&self, agent_id: &AgentId, reason: &str) {
        self.state
            .lock()
            .expect("fake state")
            .failing_exit_switches
            .insert(agent_id.clone(), reason.to_string());
    }

    pub fn take_server_end(&self) -> Option<tokio::io::DuplexStream> {
        self.state.lock().expect("fake state").server_ends.pop()
    }
}

/// A gate that every pull passes.
fn open_gate() -> std::sync::Arc<tokio::sync::Semaphore> {
    let gate = tokio::sync::Semaphore::new(0);
    gate.close();
    std::sync::Arc::new(gate)
}

impl FakeState {
    /// `Err` while Docker does not answer.
    fn answering(&self) -> Result<(), String> {
        match self.docker_answers {
            true => Ok(()),
            false => Err(NO_DOCKER.to_string()),
        }
    }
}

/// The agent a fake computer belongs to: its control address is
/// `fake:<agent>`.
fn fake_agent(computer: &StartedComputer) -> AgentId {
    AgentId::from(
        computer
            .control_addr
            .strip_prefix("fake:")
            .unwrap_or_default()
            .to_string(),
    )
}

/// screend answers the browser channel only while the daemon holds the
/// switch; the fake enforces the same rule.
fn daemon_holds(state: &FakeState, computer: &StartedComputer) -> Result<(), String> {
    let current = state
        .holders
        .get(&fake_agent(computer))
        .copied()
        .unwrap_or(InputHolder::Agent);
    if current != InputHolder::Daemon {
        return Err(format!(
            "the browser channel refused: {} holds the switch",
            current.as_str()
        ));
    }
    Ok(())
}

#[async_trait]
impl ComputerRuntime for FakeComputerRuntime {
    async fn image_version(&self) -> Result<Option<String>, String> {
        let state = self.state.lock().expect("fake state");
        state.answering()?;
        Ok(state.image_version.clone())
    }

    async fn pull_image(
        &self,
        image: &str,
        progress: tokio::sync::mpsc::UnboundedSender<u8>,
    ) -> Result<(), String> {
        let (delay, error, gate) = {
            let mut state = self.state.lock().expect("fake state");
            state.answering()?;
            state.pulls += 1;
            (
                state.pull_delay,
                state.pull_error.take(),
                std::sync::Arc::clone(&state.pull_gate),
            )
        };
        // A closed gate lets the pull go on.
        let _ = gate.acquire().await;
        tokio::time::sleep(delay).await;
        for percent in [10u8, 50, 100] {
            let _ = progress.send(percent);
        }
        if let Some(error) = error {
            return Err(error);
        }
        let mut state = self.state.lock().expect("fake state");
        if image == IMAGE {
            state.image_version = Some(state.pulled_version.clone());
        }
        state.pulled_images.push(image.to_string());
        Ok(())
    }

    async fn other_images(&self) -> Result<Vec<OtherImage>, String> {
        let state = self.state.lock().expect("fake state");
        state.answering()?;
        if state.image_version.is_none() {
            return Err("the pinned Computer Image is absent".to_string());
        }
        Ok(state
            .old_images
            .iter()
            .map(|(image, _)| image.clone())
            .collect())
    }

    async fn remove_image(&self, id: &str) -> Result<ImageRemoval, String> {
        let mut state = self.state.lock().expect("fake state");
        state.answering()?;
        let Some(at) = state.old_images.iter().position(|(old, _)| old.id == id) else {
            return Err(format!("no such image: {id}"));
        };
        if state.old_images[at].1 {
            return Ok(ImageRemoval::InUse);
        }
        state.old_images.remove(at);
        state.removed_images.push(id.to_string());
        Ok(ImageRemoval::Removed)
    }

    async fn running(
        &self,
        owner: &crate::ComputerOwner,
    ) -> Result<Option<RunningComputer>, String> {
        let agent_id = &owner.agent_id;
        let state = self.state.lock().expect("fake state");
        Ok(state
            .running
            .get(agent_id)
            .filter(|booted| booted.ready)
            .map(|booted| RunningComputer {
                computer: StartedComputer {
                    container: owner.container_name(),
                    control_addr: format!("fake:{agent_id}"),
                    token: format!("fake-token-{agent_id}"),
                },
                version: booted.version.clone(),
                mounts: booted.mounts.clone(),
                image_matches: booted.image_matches,
            }))
    }

    /// The fake keeps one entry per Agent, and the owner of each start
    /// says which tenant the Agent belongs to, so a container booted
    /// outside the daemon belongs to no tenant here.
    async fn running_agents(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
    ) -> Result<Vec<AgentId>, String> {
        let state = self.state.lock().expect("fake state");
        Ok(state
            .owners
            .iter()
            .filter(|owner| &owner.workspace_id == workspace_id)
            .map(|owner| owner.agent_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|agent_id| state.running.contains_key(agent_id))
            .collect())
    }

    async fn start(
        &self,
        owner: &crate::ComputerOwner,
        mounts: &[crate::BindMount],
        env: &[String],
    ) -> Result<StartedComputer, String> {
        let agent_id = &owner.agent_id;
        let mut state = self.state.lock().expect("fake state");
        state.starts += 1;
        let version = state.image_version.clone();
        state.running.insert(
            agent_id.clone(),
            Booted {
                version,
                mounts: Some(crate::mounts_fingerprint(mounts)),
                ready: true,
                image_matches: true,
            },
        );
        state.mounts.push(mounts.to_vec());
        state.start_envs.push(env.to_vec());
        state.volumes.insert(owner.volume_name());
        state.owners.push(owner.clone());
        let mode = match env.iter().any(|entry| entry == "PAGIS_EXIT_MODE=home") {
            true => crate::ExitMode::Home,
            false => crate::ExitMode::Direct,
        };
        state.exit_modes.insert(agent_id.clone(), mode);
        Ok(StartedComputer {
            container: owner.container_name(),
            control_addr: format!("fake:{agent_id}"),
            token: format!("fake-token-{agent_id}"),
        })
    }

    /// One scripted server process. The fake answers with the
    /// two halves of a duplex pipe, so a test drives the server side by
    /// hand and nothing starts on the host.
    async fn exec_stream(
        &self,
        _computer: &StartedComputer,
        request: ExecRequest,
    ) -> Result<crate::ExecStream, String> {
        let (daemon, server) = tokio::io::duplex(64 * 1024);
        let (stderr_tx, stderr_rx) = tokio::sync::mpsc::channel(crate::EXEC_STREAM_CAPACITY);
        let (read, write) = tokio::io::split(daemon);
        let mut state = self.state.lock().expect("fake state");
        state.exec_streams.push(request);
        state.server_ends.push(server);
        state.stderr_senders.push(stderr_tx);
        Ok(crate::ExecStream {
            stdin: Box::pin(write),
            stdout: Box::pin(read),
            stderr: stderr_rx,
        })
    }

    async fn stop(&self, owner: &crate::ComputerOwner) -> Result<(), String> {
        let mut state = self.state.lock().expect("fake state");
        state.running.remove(&owner.agent_id);
        Ok(())
    }

    async fn volume_quota(&self) -> crate::Quota {
        // The fake makes no Docker volume, so it holds none to a size.
        crate::Quota::Supported
    }

    async fn container_quota(&self) -> crate::Quota {
        self.state.lock().expect("fake state").container_quota
    }

    /// The bytes of the volumes of one tenant: the fake names
    /// every volume after its owner, so the filter is the same one the
    /// real runtime applies to the owner label.
    async fn resources(
        &self,
        workspace_id: &pagis_core::WorkspaceId,
    ) -> Result<crate::TenantResources, String> {
        let state = self.state.lock().expect("fake state");
        let prefix = crate::volume_prefix(workspace_id);
        let volumes = state
            .volumes
            .iter()
            .filter(|name| name.starts_with(&prefix))
            .count() as u32;
        // A container of this tenant that still runs. The fake keeps one
        // entry per Agent, and the owners of every start say which
        // tenant each Agent belongs to.
        let containers = state
            .owners
            .iter()
            .filter(|owner| &owner.workspace_id == workspace_id)
            .map(|owner| owner.agent_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .filter(|agent_id| state.running.contains_key(agent_id))
            .count() as u32;
        Ok(crate::TenantResources {
            containers,
            volumes,
            volume_bytes: u64::from(volumes) * state.volume_bytes,
        })
    }

    async fn send_input(
        &self,
        computer: &StartedComputer,
        holder: InputHolder,
        ops: &[crate::exec::InputOp],
    ) -> Result<(), String> {
        let agent = fake_agent(computer);
        let mut state = self.state.lock().expect("fake state");
        if let Some(message) = &state.fail_input {
            return Err(message.clone());
        }
        // screend refuses every batch of the daemon, which writes
        // through the browser channel, and a batch whose declared holder
        // is not the one that holds the switch; the fake enforces the
        // same rules.
        if holder == InputHolder::Daemon {
            return Err("input refused: the daemon writes through the browser channel".to_string());
        }
        let current = state
            .holders
            .get(&agent)
            .copied()
            .unwrap_or(InputHolder::Agent);
        if current != holder {
            return Err(format!(
                "input refused: {} holds the switch",
                current.as_str()
            ));
        }
        state
            .inputs
            .extend(ops.iter().cloned().map(|op| (holder, op)));
        Ok(())
    }

    async fn browser_open(&self, computer: &StartedComputer, url: &str) -> Result<String, String> {
        let mut state = self.state.lock().expect("fake state");
        daemon_holds(&state, computer)?;
        state.browser.opens.push(url.to_string());
        if let Some(reason) = &state.browser.fail_open {
            return Err(reason.clone());
        }
        let landed = state
            .browser
            .redirects
            .get(url)
            .cloned()
            .unwrap_or_else(|| url.to_string());
        state.browser.page = Some(landed.clone());
        Ok(landed)
    }

    async fn browser_page(&self, computer: &StartedComputer) -> Result<String, String> {
        let state = self.state.lock().expect("fake state");
        daemon_holds(&state, computer)?;
        state
            .browser
            .page
            .clone()
            .ok_or_else(|| "the daemon has no tab open".to_string())
    }

    async fn browser_fill(
        &self,
        computer: &StartedComputer,
        origin: &str,
        fields: &[crate::FillField],
    ) -> Result<(), String> {
        let mut state = self.state.lock().expect("fake state");
        daemon_holds(&state, computer)?;
        if let Some(reason) = &state.browser.fail_fill {
            return Err(reason.clone());
        }
        state
            .browser
            .fills
            .push((origin.to_string(), fields.to_vec()));
        Ok(())
    }

    async fn relay_offer(
        &self,
        _computer: &StartedComputer,
        offer: &str,
        path: &crate::MediaPath,
    ) -> Result<String, String> {
        let mut state = self.state.lock().expect("fake state");
        state.offers.push((offer.to_string(), path.clone()));
        state.answer.clone()
    }

    async fn set_holder(
        &self,
        computer: &StartedComputer,
        holder: InputHolder,
    ) -> Result<(), String> {
        self.state
            .lock()
            .expect("fake state")
            .holders
            .insert(fake_agent(computer), holder);
        Ok(())
    }

    async fn user_input_idle_ms(&self, _computer: &StartedComputer) -> Result<u64, String> {
        Ok(self.state.lock().expect("fake state").user_idle_ms)
    }

    /// A fake Computer runs no browser and no shell, so its Exit Proxy
    /// holds no connection. Its mode is the one that its start
    /// environment named, or the one of the last switch.
    async fn exit_status(&self, computer: &StartedComputer) -> Result<crate::ExitStatus, String> {
        let state = self.state.lock().expect("fake state");
        Ok(crate::ExitStatus {
            mode: state
                .exit_modes
                .get(&fake_agent(computer))
                .copied()
                .unwrap_or(crate::ExitMode::Direct),
            connections: 0,
        })
    }

    async fn set_exit_mode(
        &self,
        computer: &StartedComputer,
        mode: crate::ExitMode,
    ) -> Result<u64, String> {
        let agent_id = fake_agent(computer);
        let mut state = self.state.lock().expect("fake state");
        if let Some(reason) = state.failing_exit_switches.get(&agent_id) {
            return Err(format!("the Exit Proxy switch was refused: {reason}"));
        }
        state.exit_modes.insert(agent_id.clone(), mode);
        state.exit_switches.push((agent_id, mode));
        Ok(0)
    }

    async fn exec(
        &self,
        computer: &StartedComputer,
        request: ExecRequest,
    ) -> Result<ExecOutcome, String> {
        let agent = AgentId::from(
            computer
                .control_addr
                .strip_prefix("fake:")
                .unwrap_or_default()
                .to_string(),
        );
        let (delay, outcome) = {
            let mut state = self.state.lock().expect("fake state");
            state.execs.push(request);
            (state.exec_delay, state.exec_outcomes.pop_front())
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        // A stopped container kills every exec on it, with code 137.
        if !self
            .state
            .lock()
            .expect("fake state")
            .running
            .contains_key(&agent)
        {
            return Ok(ExecOutcome {
                exit_code: 137,
                ..ExecOutcome::default()
            });
        }
        Ok(outcome.unwrap_or_default())
    }

    async fn upload_archive(
        &self,
        _computer: &StartedComputer,
        path: &str,
        tar: Vec<u8>,
    ) -> Result<(), String> {
        let mut state = self.state.lock().expect("fake state");
        if let Some(message) = state.fail_upload.clone() {
            return Err(message);
        }
        state.uploads.push((path.to_string(), tar));
        Ok(())
    }

    async fn download_archive(
        &self,
        _computer: &StartedComputer,
        path: &str,
    ) -> Result<crate::ArchiveStream, String> {
        let mut state = self.state.lock().expect("fake state");
        state.downloaded.push(path.to_string());
        state
            .downloads
            .get(path)
            .map(|make| make())
            .ok_or_else(|| format!("no such file or directory: {path}"))
    }

    async fn fetch_frame(&self, computer: &StartedComputer) -> Result<Vec<u8>, String> {
        let mut state = self.state.lock().expect("fake state");
        state.frames_served += 1;
        let queued = state.queued_frames.pop_front();
        let state = &*state;
        if let Some(message) = &state.fail_frame {
            return Err(message.clone());
        }
        let agent = computer
            .control_addr
            .strip_prefix("fake:")
            .unwrap_or_default();
        if state
            .running
            .contains_key(&AgentId::from(agent.to_string()))
        {
            Ok(queued.unwrap_or_else(|| state.frame.clone()))
        } else {
            Err("container is not running".to_string())
        }
    }
}

/// Which Agent belongs to which Workspace, in memory. A manager
/// reads it to refuse an Agent that is not its tenant's.
pub struct FakeAgents {
    /// True when every Agent id belongs to every Workspace. A test that
    /// measures something other than the tenant boundary wants this: it
    /// wakes Agents it never wrote a row for.
    open: bool,
    rows: Mutex<Vec<(WorkspaceId, pagis_core::AgentId)>>,
}

impl FakeAgents {
    /// Every Agent belongs to every Workspace.
    pub fn open() -> Self {
        Self {
            open: true,
            rows: Mutex::new(Vec::new()),
        }
    }

    /// Only the Agents this fake was told about exist, each in its own
    /// Workspace.
    pub fn strict() -> Self {
        Self {
            open: false,
            rows: Mutex::new(Vec::new()),
        }
    }

    /// Record one Agent of one Workspace.
    pub fn add(&self, workspace_id: &WorkspaceId, agent_id: &pagis_core::AgentId) {
        self.rows
            .lock()
            .expect("fake agents")
            .push((workspace_id.clone(), agent_id.clone()));
    }
}

#[async_trait]
impl pagis_core::AgentStore for FakeAgents {
    async fn create(&self, agent: &pagis_core::Agent) -> Result<(), StoreError> {
        self.add(&agent.workspace_id, &agent.id);
        Ok(())
    }

    async fn get(
        &self,
        workspace_id: &WorkspaceId,
        id: &pagis_core::AgentId,
    ) -> Result<Option<pagis_core::Agent>, StoreError> {
        let held = self.open
            || self
                .rows
                .lock()
                .expect("fake agents")
                .iter()
                .any(|(workspace, agent)| workspace == workspace_id && agent == id);
        Ok(held.then(|| agent_row(workspace_id, id)))
    }

    async fn list_by_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Vec<pagis_core::Agent>, StoreError> {
        Ok(self
            .rows
            .lock()
            .expect("fake agents")
            .iter()
            .filter(|(workspace, _)| workspace == workspace_id)
            .map(|(workspace, agent)| agent_row(workspace, agent))
            .collect())
    }

    async fn update(&self, _agent: &pagis_core::Agent) -> Result<(), StoreError> {
        Ok(())
    }

    async fn update_avatar(
        &self,
        _workspace_id: &WorkspaceId,
        _id: &pagis_core::AgentId,
        _avatar: &pagis_core::AvatarAppearance,
        _updated_at: i64,
    ) -> Result<bool, StoreError> {
        Ok(true)
    }
}

/// The Agent row the fake answers with. Only the two ids matter to the
/// manager, so the rest is filler.
fn agent_row(workspace_id: &WorkspaceId, id: &pagis_core::AgentId) -> pagis_core::Agent {
    pagis_core::Agent {
        id: id.clone(),
        workspace_id: workspace_id.clone(),
        name: "test".to_string(),
        job: String::new(),
        description: String::new(),
        personality: String::new(),
        model_alias: "default".to_string(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: pagis_core::AgentStatus::Active,
        created_at: now_ms(),
        updated_at: now_ms(),
    }
}

/// Workspaces in memory, so a test can say which timezone the
/// container boots with, and which Home Exit a Person chose.
pub struct FakeWorkspaces {
    workspaces: Mutex<Vec<Workspace>>,
}

impl FakeWorkspaces {
    /// One Workspace with `timezone`.
    pub fn with_timezone(id: &WorkspaceId, timezone: &str) -> Self {
        Self {
            workspaces: Mutex::new(vec![Self::workspace(id, timezone)]),
        }
    }

    /// One Workspace of a Person of its own, in UTC, with no Home Exit.
    pub fn workspace(id: &WorkspaceId, timezone: &str) -> Workspace {
        Workspace {
            id: id.clone(),
            user_id: pagis_core::UserId::generate(),
            name: "test".to_string(),
            timezone: timezone.to_string(),
            created_at: now_ms(),
            onboarded_at: None,
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
            home_exit_host_id: None,
        }
    }

    /// Change the Workspace `id` with `change`, when it is here.
    fn change(&self, id: &WorkspaceId, change: impl FnOnce(&mut Workspace)) -> bool {
        let mut workspaces = self.workspaces.lock().expect("fake workspaces");
        match workspaces.iter_mut().find(|workspace| &workspace.id == id) {
            Some(workspace) => {
                change(workspace);
                true
            }
            None => false,
        }
    }
}

#[async_trait]
impl WorkspaceStore for FakeWorkspaces {
    /// A Workspace with the id of one that is here replaces it.
    async fn create(&self, workspace: &Workspace) -> Result<(), StoreError> {
        let mut workspaces = self.workspaces.lock().expect("fake workspaces");
        workspaces.retain(|held| held.id != workspace.id);
        workspaces.push(workspace.clone());
        Ok(())
    }

    async fn get(&self, id: &WorkspaceId) -> Result<Option<Workspace>, StoreError> {
        let workspaces = self.workspaces.lock().expect("fake workspaces");
        Ok(workspaces
            .iter()
            .find(|workspace| &workspace.id == id)
            .cloned())
    }

    async fn for_user(
        &self,
        user_id: &pagis_core::UserId,
    ) -> Result<Option<Workspace>, StoreError> {
        let workspaces = self.workspaces.lock().expect("fake workspaces");
        Ok(workspaces
            .iter()
            .find(|workspace| &workspace.user_id == user_id)
            .cloned())
    }

    async fn list(&self) -> Result<Vec<Workspace>, StoreError> {
        Ok(self.workspaces.lock().expect("fake workspaces").clone())
    }

    async fn set_onboarded(&self, id: &WorkspaceId, at: UnixMillis) -> Result<(), StoreError> {
        self.change(id, |workspace| {
            workspace.onboarded_at.get_or_insert(at);
        });
        Ok(())
    }

    async fn set_timezone(&self, id: &WorkspaceId, timezone: &str) -> Result<(), StoreError> {
        self.change(id, |workspace| workspace.timezone = timezone.to_string());
        Ok(())
    }

    async fn set_chief_of_staff(
        &self,
        id: &WorkspaceId,
        agent_id: Option<&AgentId>,
    ) -> Result<(), StoreError> {
        self.change(id, |workspace| {
            workspace.chief_of_staff_agent_id = agent_id.cloned()
        });
        Ok(())
    }

    async fn set_report_schedule(
        &self,
        id: &WorkspaceId,
        schedule_id: Option<&ScheduleId>,
    ) -> Result<(), StoreError> {
        self.change(id, |workspace| {
            workspace.report_schedule_id = schedule_id.cloned()
        });
        Ok(())
    }

    /// The fake holds no Host, so it takes any Host id. The store suite
    /// proves on both backends that a Host of another Workspace is
    /// refused.
    async fn set_home_exit(
        &self,
        id: &WorkspaceId,
        host_id: Option<&pagis_core::HostId>,
    ) -> Result<bool, StoreError> {
        Ok(self.change(id, |workspace| {
            workspace.home_exit_host_id = host_id.cloned()
        }))
    }
}

/// The two ends of one exit socket in memory: the daemon's end, which
/// [`crate::HomeExits::serve`] takes, and the Client App's end, which
/// [`FakeHomeExit::serve`] takes. They carry the bytes that the binary
/// frames of the WebSocket carry.
pub fn exit_socket_pair() -> (
    tokio_util::compat::Compat<tokio::io::DuplexStream>,
    tokio_util::compat::Compat<tokio::io::DuplexStream>,
) {
    use tokio_util::compat::TokioAsyncReadCompatExt;

    let (daemon, client_app) = tokio::io::duplex(64 * 1024);
    (daemon.compat(), client_app.compat())
}

/// What the fake Home Exit does with one destination.
#[derive(Debug, Clone)]
pub enum FakeExitAnswer {
    /// Connect to this address and carry the bytes.
    Connect(std::net::SocketAddr),
    /// Answer `refused` with this reason.
    Refuse(String),
    /// Answer `failed` with this reason.
    Fail(String),
}

/// The Client App's end of an exit socket, in this process. It speaks the
/// protocol of the Client App: it accepts the yamux streams of the
/// daemon, reads each preamble, answers one status line and copies the
/// bytes both ways. A test reaches no site on the internet, so `route`
/// maps each destination to an answer; the address check of the real
/// Client App has tests of its own.
pub struct FakeHomeExit {
    route: Box<ExitRoute>,
    destinations: Mutex<Vec<String>>,
}

/// What a fake Home Exit answers for each destination.
type ExitRoute = dyn Fn(&str, u16) -> FakeExitAnswer + Send + Sync;

impl FakeHomeExit {
    pub fn new(
        route: impl Fn(&str, u16) -> FakeExitAnswer + Send + Sync + 'static,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            route: Box::new(route),
            destinations: Mutex::new(Vec::new()),
        })
    }

    /// A Home Exit that connects every destination to `target`.
    pub fn to(target: std::net::SocketAddr) -> std::sync::Arc<Self> {
        Self::new(move |_, _| FakeExitAnswer::Connect(target))
    }

    /// The preambles of the streams it took, in order.
    pub fn destinations(&self) -> Vec<String> {
        self.destinations.lock().expect("fake destinations").clone()
    }

    /// Serve the Client App's end of one exit socket until it ends.
    pub async fn serve<T>(self: std::sync::Arc<Self>, socket: T)
    where
        T: futures::AsyncRead + futures::AsyncWrite + Unpin + Send + 'static,
    {
        let mut connection = yamux::Connection::new(
            socket,
            crate::home_exit::yamux_config(),
            yamux::Mode::Server,
        );
        while let Some(Ok(stream)) =
            futures::future::poll_fn(|cx| connection.poll_next_inbound(cx)).await
        {
            tokio::spawn(std::sync::Arc::clone(&self).carry(stream));
        }
    }

    /// One stream: the preamble, the answer, then the bytes.
    async fn carry(self: std::sync::Arc<Self>, stream: yamux::Stream) {
        use tokio::io::AsyncWriteExt;
        use tokio_util::compat::FuturesAsyncReadCompatExt;

        let mut stream = stream.compat();
        let Ok(line) =
            crate::home_exit::read_line(&mut stream, crate::home_exit::PREAMBLE_LIMIT).await
        else {
            return;
        };
        self.destinations
            .lock()
            .expect("fake destinations")
            .push(line.clone());
        let answer = match crate::home_exit::parse_preamble(&line) {
            Some((host, port)) => (self.route)(&host, port),
            None => FakeExitAnswer::Fail(format!("{line:?} is no destination")),
        };
        let status = |status: crate::home_exit::Status| status.line();
        match answer {
            FakeExitAnswer::Connect(target) => match tokio::net::TcpStream::connect(target).await {
                Ok(mut upstream) => {
                    if stream
                        .write_all(status(crate::home_exit::Status::Ok).as_bytes())
                        .await
                        .is_ok()
                    {
                        let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
                    }
                }
                Err(error) => {
                    let _ = stream
                        .write_all(
                            status(crate::home_exit::Status::Failed(error.to_string())).as_bytes(),
                        )
                        .await;
                }
            },
            FakeExitAnswer::Refuse(reason) => {
                let _ = stream
                    .write_all(status(crate::home_exit::Status::Refused(reason)).as_bytes())
                    .await;
            }
            FakeExitAnswer::Fail(reason) => {
                let _ = stream
                    .write_all(status(crate::home_exit::Status::Failed(reason)).as_bytes())
                    .await;
            }
        }
        let _ = stream.shutdown().await;
    }
}
