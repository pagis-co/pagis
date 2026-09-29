//! The line of a carrier Connection (ADR-0020): one endpoint task for
//! each carrier Connection owns the registration and the socket, and it
//! carries every Agent Phone Number of that carrier.
//!
//! Registration is a property of the carrier credential, not of a
//! number or a call. The line starts when the daemon starts and
//! restarts when the SIP credential changes; assigning or unassigning a
//! number does not touch it. It refreshes at half the expiry the
//! registrar granted, and after a failure it backs off, doubling to a
//! 30 s cap. Each number shows the state of its carrier's line, because
//! an unregistered line drops inbound calls in silence.
//!
//! The line routes each inbound call by the number that was dialed. The
//! stored Agent Phone Number of that number names the Workspace and the
//! Agent; the socket that took the `INVITE` names neither. The line
//! refuses a call before it answers when the dialed number is missing,
//! is not held by the installation, is held by no Agent, or matches
//! more than one record.
//!
//! One active call for each number. The line places the calls of each
//! number and answers the calls to each number; a second inbound call to
//! a number that is on a call gets `486 Busy Here`, and a call to
//! another number of the line is not affected. Each call gets one
//! [`MediaHub`].
//!
//! [`Endpoints`] is the registry above the lines: it resolves the SIP
//! credential of each carrier Connection, starts and restarts one line
//! for each, reports each state, and hands over the calls its lines
//! answered.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use pagis_core::{
    Connection, ConnectionId, ConnectionStore, EventBus, NewEvent, PhoneNumber, PhoneNumberId,
    PhoneNumberStatus, PhoneNumberStore, SecretStore, StoreError, WorkspaceId,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::hub::MediaHub;
use crate::sip_password_secret_name;
use crate::transport::{
    CallTransport, IncomingCall, Opened, Refusal, SipCredential, TransportCapabilities,
    TransportError, TransportErrorCode,
};

/// The expiry the task asks for. Telnyx recommends a refresh of about
/// 180 s, and the refresh is at half the expiry.
pub const REQUESTED_EXPIRY: Duration = Duration::from_secs(360);
/// The wait after the first failure. It doubles on each failure that
/// follows, up to [`MAX_BACKOFF`].
pub const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// How long one `REGISTER` may take before it counts as unreachable. A
/// registrar that never answers must not hold the task.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(15);
/// A registrar that grants a tiny expiry must not make the task spin.
const MIN_REFRESH: Duration = Duration::from_secs(1);
/// How many answered calls may wait for the daemon to take them.
const HANDOVER_DEPTH: usize = 8;

/// The connection `config` keys the SIP credential's public half lives
/// under. The password lives in the secret store.
pub const SIP_USERNAME_KEY: &str = "sip_username";
pub const SIP_DOMAIN_KEY: &str = "sip_domain";

/// Why a line is not registered. The codes are stable: they reach the
/// user's Settings page and the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationFailure {
    /// The carrier Connection has no SIP credential yet.
    NoCredential,
    Unauthorized,
    Unreachable,
    Refused,
}

impl RegistrationFailure {
    pub fn as_str(self) -> &'static str {
        match self {
            RegistrationFailure::NoCredential => "no_credential",
            RegistrationFailure::Unauthorized => "unauthorized",
            RegistrationFailure::Unreachable => "unreachable",
            RegistrationFailure::Refused => "refused",
        }
    }
}

impl From<TransportErrorCode> for RegistrationFailure {
    fn from(code: TransportErrorCode) -> Self {
        match code {
            TransportErrorCode::Unauthorized => RegistrationFailure::Unauthorized,
            TransportErrorCode::Unreachable => RegistrationFailure::Unreachable,
            TransportErrorCode::Refused => RegistrationFailure::Refused,
        }
    }
}

/// Where one line stands with the registrar (ADR-0020).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationState {
    /// The task is not running, or it stopped and dropped the binding.
    Unregistered,
    /// A fresh `REGISTER` is in flight.
    Registering,
    /// The registrar holds the binding. A refresh in flight keeps this
    /// state, because the binding stands until the expiry.
    Registered,
    /// The last attempt failed. The task retries after the backoff.
    Failed(RegistrationFailure),
}

impl RegistrationState {
    pub fn as_str(self) -> &'static str {
        match self {
            RegistrationState::Unregistered => "unregistered",
            RegistrationState::Registering => "registering",
            RegistrationState::Registered => "registered",
            RegistrationState::Failed(_) => "failed",
        }
    }

    pub fn failure(self) -> Option<RegistrationFailure> {
        match self {
            RegistrationState::Failed(failure) => Some(failure),
            _ => None,
        }
    }
}

/// Why a call was not placed. The codes are stable: they reach the
/// tool result and the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    /// The number is on a call.
    #[error("the number is on a call")]
    Busy,
    /// The line of the number's carrier is not registered, so no
    /// `INVITE` can go out.
    #[error("the number is not registered")]
    Unregistered,
    /// The carrier did not take the `INVITE`.
    #[error("the carrier did not take the call: {}", .0.as_str())]
    Transport(TransportErrorCode),
    /// The number has no line: no Agent holds it now, its carrier
    /// Connection has no line, or its record could not be read.
    #[error("the number has no endpoint")]
    NoEndpoint,
}

impl CallError {
    pub fn as_str(self) -> &'static str {
        match self {
            CallError::Busy => "busy",
            CallError::Unregistered => "unregistered",
            CallError::Transport(_) => "transport",
            CallError::NoEndpoint => "no_endpoint",
        }
    }
}

/// A call the line answered, ready for whoever runs it. The carrier
/// names only the dialed number, so the line says whose number it is:
/// the stored record of the number names the Workspace that holds it.
pub struct IncomingHub {
    pub workspace_id: WorkspaceId,
    pub phone_number_id: PhoneNumberId,
    /// The number the Remote Party dialed.
    pub e164: String,
    pub from_e164: String,
    pub hub: Arc<MediaHub>,
}

/// Where a line reads the stored Agent Phone Numbers of a dialed
/// number. The daemon reads its store over every Workspace; a test
/// gives a list.
#[async_trait]
pub trait NumberDirectory: Send + Sync {
    /// Every record of the E.164 number that is not released.
    async fn lookup(&self, e164: &str) -> Result<Vec<PhoneNumber>, StoreError>;
}

/// The daemon's directory: the stored Agent Phone Numbers.
struct StoredNumbers(Arc<dyn PhoneNumberStore>);

#[async_trait]
impl NumberDirectory for StoredNumbers {
    async fn lookup(&self, e164: &str) -> Result<Vec<PhoneNumber>, StoreError> {
        self.0.live_for_e164(e164).await
    }
}

enum Command {
    Dial {
        number_id: PhoneNumberId,
        from_e164: String,
        to_e164: String,
        reply: oneshot::Sender<Result<Arc<MediaHub>, CallError>>,
    },
}

/// The one call slot of each number on the line. A hub that ended
/// frees its slot.
#[derive(Default)]
struct CallSlots(Mutex<HashMap<PhoneNumberId, Arc<MediaHub>>>);

impl CallSlots {
    fn active(&self, number_id: &PhoneNumberId) -> Option<Arc<MediaHub>> {
        self.0
            .lock()
            .expect("lock")
            .get(number_id)
            .filter(|hub| !hub.is_ended())
            .cloned()
    }

    fn take_free(&self, number_id: &PhoneNumberId) -> bool {
        let mut slots = self.0.lock().expect("lock");
        if slots.get(number_id).is_some_and(|hub| !hub.is_ended()) {
            return false;
        }
        slots.remove(number_id);
        true
    }

    fn fill(&self, number_id: PhoneNumberId, hub: Arc<MediaHub>) {
        self.0.lock().expect("lock").insert(number_id, hub);
    }

    fn all_active(&self) -> Vec<Arc<MediaHub>> {
        self.0
            .lock()
            .expect("lock")
            .values()
            .filter(|hub| !hub.is_ended())
            .cloned()
            .collect()
    }
}

/// The handle of one running line.
pub struct EndpointTask {
    state: watch::Receiver<RegistrationState>,
    commands: mpsc::Sender<Command>,
    slots: Arc<CallSlots>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl EndpointTask {
    /// Start the line of one SIP credential. With no credential the
    /// line reports `no_credential` and asks the carrier nothing; the
    /// registry restarts it once an Administrator enters one. The line
    /// finds the number of each inbound call in `numbers`, and the
    /// calls it answers go to `answered`.
    pub fn spawn(
        transport: Arc<dyn CallTransport>,
        credential: Option<SipCredential>,
        numbers: Arc<dyn NumberDirectory>,
        answered: mpsc::Sender<IncomingHub>,
    ) -> Self {
        // The handle is truthful from the start: the first `REGISTER`
        // is in flight as soon as the task exists, or it never will be.
        let (tx, state) = watch::channel(match credential {
            Some(_) => RegistrationState::Registering,
            None => RegistrationState::Failed(RegistrationFailure::NoCredential),
        });
        let (commands, command_rx) = mpsc::channel(1);
        let slots = Arc::new(CallSlots::default());
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(
            Runner {
                transport,
                numbers,
                state: tx,
                commands: command_rx,
                answered,
                slots: Arc::clone(&slots),
                cancel: cancel.clone(),
            },
            credential,
        ));
        Self {
            state,
            commands,
            slots,
            cancel,
            task,
        }
    }

    pub fn state(&self) -> RegistrationState {
        *self.state.borrow()
    }

    /// A receiver that sees every state change, for the registry's
    /// event watcher and for tests.
    pub fn watch(&self) -> watch::Receiver<RegistrationState> {
        self.state.clone()
    }

    /// The call on one number now, or `None` when the number is free.
    pub fn active_call(&self, number_id: &PhoneNumberId) -> Option<Arc<MediaHub>> {
        self.slots.active(number_id)
    }

    /// Send `INVITE` from one number of the line to one E.164 number.
    /// The hub carries the call from here: ringing, the answer and the
    /// end arrive as its events.
    pub async fn place_call(
        &self,
        number: &PhoneNumber,
        to_e164: &str,
    ) -> Result<Arc<MediaHub>, CallError> {
        dial_on(&self.commands, number, to_e164).await
    }

    /// Stop the line. It hangs up every call that runs and unregisters,
    /// when it can, before it ends.
    pub async fn stop(self) {
        self.cancel.cancel();
        if let Err(error) = self.task.await {
            tracing::error!(%error, "an endpoint task ended badly");
        }
    }
}

async fn dial_on(
    commands: &mpsc::Sender<Command>,
    number: &PhoneNumber,
    to_e164: &str,
) -> Result<Arc<MediaHub>, CallError> {
    let (reply, answer) = oneshot::channel();
    commands
        .send(Command::Dial {
            number_id: number.id.clone(),
            from_e164: number.e164.clone(),
            to_e164: to_e164.to_string(),
            reply,
        })
        .await
        .map_err(|_| CallError::NoEndpoint)?;
    answer.await.map_err(|_| CallError::NoEndpoint)?
}

/// What the task loop holds.
struct Runner {
    transport: Arc<dyn CallTransport>,
    numbers: Arc<dyn NumberDirectory>,
    state: watch::Sender<RegistrationState>,
    commands: mpsc::Receiver<Command>,
    answered: mpsc::Sender<IncomingHub>,
    slots: Arc<CallSlots>,
    cancel: CancellationToken,
}

async fn run(mut runner: Runner, credential: Option<SipCredential>) {
    let Some(credential) = credential else {
        runner.cancel.cancelled().await;
        runner.state.send_replace(RegistrationState::Unregistered);
        return;
    };
    let mut line: Option<Opened> = None;
    let mut backoff = INITIAL_BACKOFF;
    let mut next_attempt = Instant::now();
    loop {
        tokio::select! {
            () = runner.cancel.cancelled() => break,
            () = tokio::time::sleep_until(next_attempt) => {
                let wait = attempt(&mut runner, &credential, &mut line, &mut backoff).await;
                next_attempt = Instant::now() + wait;
            }
            Some(command) = runner.commands.recv() => match command {
                Command::Dial { number_id, from_e164, to_e164, reply } => {
                    let placed = dial(&runner, &mut line, number_id, &from_e164, &to_e164).await;
                    // A caller that gave up is not an error.
                    let _ = reply.send(placed);
                }
            },
            Some(call) = next_incoming(&mut line) => {
                answer(&runner, call).await;
            }
        }
    }
    for hub in runner.slots.all_active() {
        hub.hangup().await;
    }
    if let Some(mut opened) = line {
        // Best effort: the registrar drops the binding at expiry anyway.
        if let Ok(Err(error)) =
            tokio::time::timeout(REGISTER_TIMEOUT, opened.line.unregister()).await
        {
            tracing::warn!(
                username = credential.username(),
                code = error.0.as_str(),
                "unregistering a line failed"
            );
        }
    }
    runner.state.send_replace(RegistrationState::Unregistered);
}

/// One `REGISTER`, and how long to wait before the next one.
async fn attempt(
    runner: &mut Runner,
    credential: &SipCredential,
    line: &mut Option<Opened>,
    backoff: &mut Duration,
) -> Duration {
    if line.is_none() {
        // A retry after a failure is a fresh registration; a refresh
        // keeps `registered`, because the binding stands until the
        // expiry.
        runner.state.send_replace(RegistrationState::Registering);
    }
    let outcome = tokio::time::timeout(
        REGISTER_TIMEOUT,
        register(&*runner.transport, credential, line),
    )
    .await
    .unwrap_or(Err(TransportError(TransportErrorCode::Unreachable)));
    match outcome {
        Ok(granted) => {
            *backoff = INITIAL_BACKOFF;
            runner.state.send_replace(RegistrationState::Registered);
            (granted / 2).max(MIN_REFRESH)
        }
        Err(error) => {
            // The socket is not trusted after a failure; the next
            // attempt opens a fresh one.
            *line = None;
            tracing::warn!(
                username = credential.username(),
                code = error.0.as_str(),
                "registering a line failed"
            );
            runner
                .state
                .send_replace(RegistrationState::Failed(error.0.into()));
            let wait = *backoff;
            *backoff = (*backoff * 2).min(MAX_BACKOFF);
            wait
        }
    }
}

/// One attempt: open the socket when there is none, then `REGISTER`.
async fn register(
    transport: &dyn CallTransport,
    credential: &SipCredential,
    line: &mut Option<Opened>,
) -> Result<Duration, TransportError> {
    if line.is_none() {
        *line = Some(transport.open(credential).await?);
    }
    line.as_mut()
        .expect("the line was opened above")
        .line
        .register(REQUESTED_EXPIRY)
        .await
}

/// The next call that arrives on the line, or never while there is no
/// line.
async fn next_incoming(line: &mut Option<Opened>) -> Option<IncomingCall> {
    match line {
        Some(opened) => opened.incoming.recv().await,
        None => std::future::pending().await,
    }
}

async fn dial(
    runner: &Runner,
    line: &mut Option<Opened>,
    number_id: PhoneNumberId,
    from_e164: &str,
    to_e164: &str,
) -> Result<Arc<MediaHub>, CallError> {
    if *runner.state.borrow() != RegistrationState::Registered {
        return Err(CallError::Unregistered);
    }
    let Some(opened) = line.as_mut() else {
        return Err(CallError::Unregistered);
    };
    if !runner.slots.take_free(&number_id) {
        return Err(CallError::Busy);
    }
    let leg = opened
        .line
        .dial(from_e164, to_e164)
        .await
        .map_err(|error| CallError::Transport(error.0))?;
    let hub = MediaHub::start(leg);
    runner.slots.fill(number_id, Arc::clone(&hub));
    Ok(hub)
}

/// Answer a call that arrived for a number an Agent holds, or turn it
/// away before it is answered.
async fn answer(runner: &Runner, call: IncomingCall) {
    let number = match route(runner.numbers.as_ref(), call.dialed_e164.as_deref()).await {
        Ok(number) => number,
        Err(refusal) => {
            call.answer.reject(refusal).await;
            return;
        }
    };
    if !runner.slots.take_free(&number.id) {
        tracing::info!(
            e164 = number.e164,
            from = call.from_e164,
            "busy: a call runs already"
        );
        call.answer.reject(Refusal::Busy).await;
        return;
    }
    let leg = match call.answer.accept().await {
        Ok(leg) => leg,
        Err(error) => {
            tracing::warn!(
                e164 = number.e164,
                code = error.0.as_str(),
                "answering a call failed"
            );
            return;
        }
    };
    let hub = MediaHub::start(leg);
    runner.slots.fill(number.id.clone(), Arc::clone(&hub));
    let handed = runner.answered.try_send(IncomingHub {
        workspace_id: number.workspace_id,
        phone_number_id: number.id,
        e164: number.e164.clone(),
        from_e164: call.from_e164,
        hub: Arc::clone(&hub),
    });
    if handed.is_err() {
        tracing::error!(
            e164 = number.e164,
            "nobody takes answered calls; hanging up"
        );
        hub.hangup().await;
    }
}

/// The stored record the dialed number reaches, or why the call is
/// refused. The record, not the socket, names the Workspace and the
/// Agent the call runs under.
async fn route(
    numbers: &dyn NumberDirectory,
    dialed: Option<&str>,
) -> Result<PhoneNumber, Refusal> {
    let Some(dialed) = dialed else {
        tracing::info!("refused: the INVITE names no E.164 dialed number");
        return Err(Refusal::NotFound);
    };
    let mut held = match numbers.lookup(dialed).await {
        Ok(held) => held,
        Err(error) => {
            tracing::error!(%error, dialed, "refused: the dialed number could not be read");
            return Err(Refusal::Unavailable);
        }
    };
    if held.len() > 1 {
        tracing::error!(
            dialed,
            records = held.len(),
            "refused: the dialed number matches more than one record"
        );
        return Err(Refusal::Unavailable);
    }
    let Some(number) = held.pop() else {
        tracing::info!(
            dialed,
            "refused: the installation does not hold the dialed number"
        );
        return Err(Refusal::NotFound);
    };
    if number.agent_id.is_none() {
        tracing::info!(dialed, "refused: no Agent holds the dialed number");
        return Err(Refusal::Unavailable);
    }
    Ok(number)
}

/// Everything the registry reaches: the transport, the stores that hold
/// the numbers and the carrier, the secret store that holds the SIP
/// password, and the bus the state changes go to.
pub struct EndpointsDeps {
    pub transport: Arc<dyn CallTransport>,
    pub numbers: Arc<dyn PhoneNumberStore>,
    pub connections: Arc<dyn ConnectionStore>,
    /// The Org's Workspace, which holds the carrier Connection and
    /// with it the SIP sign-in of its line.
    pub org_workspace_id: WorkspaceId,
    pub secrets: Arc<dyn SecretStore>,
    pub bus: Arc<dyn EventBus>,
}

struct Running {
    task: EndpointTask,
    watcher: tokio::task::JoinHandle<()>,
}

/// The lines of the daemon, one for each carrier Connection.
pub struct Endpoints {
    transport: Arc<dyn CallTransport>,
    numbers: Arc<dyn PhoneNumberStore>,
    connections: Arc<dyn ConnectionStore>,
    org_workspace_id: WorkspaceId,
    secrets: Arc<dyn SecretStore>,
    bus: Arc<dyn EventBus>,
    running: Mutex<HashMap<ConnectionId, Running>>,
    /// One line starts or stops at a time. The start-up pass and a
    /// credential change both give a carrier a line, and two starts for
    /// one carrier would interleave their `REGISTER` and `UNREGISTER`.
    lifecycle: tokio::sync::Mutex<()>,
    answered: mpsc::Sender<IncomingHub>,
    /// Taken once, by whoever runs the answered calls.
    handover: Mutex<Option<mpsc::Receiver<IncomingHub>>>,
}

impl Endpoints {
    pub fn new(deps: EndpointsDeps) -> Self {
        let (answered, handover) = mpsc::channel(HANDOVER_DEPTH);
        Self {
            transport: deps.transport,
            numbers: deps.numbers,
            connections: deps.connections,
            org_workspace_id: deps.org_workspace_id,
            secrets: deps.secrets,
            bus: deps.bus,
            running: Mutex::new(HashMap::new()),
            lifecycle: tokio::sync::Mutex::new(()),
            answered,
            handover: Mutex::new(Some(handover)),
        }
    }

    pub fn capabilities(&self) -> TransportCapabilities {
        self.transport.capabilities()
    }

    /// The calls the lines answer, across every number. There is one
    /// receiver; the second call returns `None`.
    pub fn take_answered_calls(&self) -> Option<mpsc::Receiver<IncomingHub>> {
        self.handover.lock().expect("lock").take()
    }

    /// Start the line of one carrier Connection, unless one runs. The
    /// daemon calls it once at start, off the boot path, so a credential
    /// an Administrator enters meanwhile is not undone: a line that runs
    /// already keeps its registration and its calls.
    pub async fn start(&self, connection_id: &ConnectionId) {
        let _lifecycle = self.lifecycle.lock().await;
        if self.state(connection_id).is_none() {
            self.start_line(connection_id).await;
        }
    }

    /// Restart the line of one carrier Connection, so a SIP credential
    /// that was entered, replaced or removed is used now and not at the
    /// next start. A Connection that is gone has no line.
    pub async fn restart_for_connection(&self, connection_id: &ConnectionId) {
        let _lifecycle = self.lifecycle.lock().await;
        self.stop_line(connection_id).await;
        self.start_line(connection_id).await;
    }

    async fn start_line(&self, connection_id: &ConnectionId) {
        let connection = match self
            .connections
            .get(&self.org_workspace_id, connection_id)
            .await
        {
            Ok(Some(connection)) => connection,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(%error, "reading the carrier Connection failed; it has no line");
                return;
            }
        };
        let task = EndpointTask::spawn(
            Arc::clone(&self.transport),
            self.credential_for(&connection),
            Arc::new(StoredNumbers(Arc::clone(&self.numbers))),
            self.answered.clone(),
        );
        let watcher = tokio::spawn(publish_changes(
            Arc::clone(&self.bus),
            self.org_workspace_id.clone(),
            connection.id.clone(),
            task.watch(),
        ));
        self.running
            .lock()
            .expect("lock")
            .insert(connection.id, Running { task, watcher });
    }

    async fn stop_line(&self, connection_id: &ConnectionId) {
        let running = self.running.lock().expect("lock").remove(connection_id);
        if let Some(running) = running {
            running.task.stop().await;
            if let Err(error) = running.watcher.await {
                tracing::error!(%error, "an endpoint watcher ended badly");
            }
        }
    }

    /// Where the line of one carrier Connection stands, or `None` when
    /// the Connection has no line.
    pub fn state(&self, connection_id: &ConnectionId) -> Option<RegistrationState> {
        self.running
            .lock()
            .expect("lock")
            .get(connection_id)
            .map(|running| running.task.state())
    }

    /// Place a call from one number, on the line of its carrier
    /// Connection. The number must be held by an Agent now.
    pub async fn place_call(
        &self,
        workspace_id: &WorkspaceId,
        number_id: &PhoneNumberId,
        to_e164: &str,
    ) -> Result<Arc<MediaHub>, CallError> {
        let number = match self.numbers.get(workspace_id, number_id).await {
            Ok(Some(number)) if number.status == PhoneNumberStatus::Assigned => number,
            Ok(_) => return Err(CallError::NoEndpoint),
            Err(error) => {
                tracing::error!(%error, "reading the number that places a call failed");
                return Err(CallError::NoEndpoint);
            }
        };
        let commands = self
            .running
            .lock()
            .expect("lock")
            .get(&number.connection_id)
            .map(|running| running.task.commands.clone())
            .ok_or(CallError::NoEndpoint)?;
        dial_on(&commands, &number, to_e164).await
    }

    fn credential_for(&self, connection: &Connection) -> Option<SipCredential> {
        let password = match self.secrets.get(&sip_password_secret_name(
            &connection.provider,
            &connection.alias,
        )) {
            Ok(password) => password?,
            Err(error) => {
                tracing::error!(%error, "reading the SIP password failed");
                return None;
            }
        };
        sip_identity(connection)
            .map(|(username, domain)| SipCredential::new(username, password, domain))
    }
}

/// The public half of the SIP credential on a carrier Connection: the
/// username and the registrar, when both are set.
pub fn sip_identity(connection: &Connection) -> Option<(String, String)> {
    let username = connection.config[SIP_USERNAME_KEY].as_str()?;
    let domain = connection.config[SIP_DOMAIN_KEY].as_str()?;
    Some((username.to_string(), domain.to_string()))
}

/// One audit line for each change of state that matters: `registering`
/// is a step of every retry and is not published, and a retry that
/// fails the way the last one did is not published either.
async fn publish_changes(
    bus: Arc<dyn EventBus>,
    workspace_id: WorkspaceId,
    connection_id: ConnectionId,
    mut states: watch::Receiver<RegistrationState>,
) {
    let mut published = None;
    loop {
        let state = *states.borrow_and_update();
        if state != RegistrationState::Registering && published != Some(state) {
            published = Some(state);
            let event = NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: "connection.registration_changed".to_string(),
                agent_id: None,
                run_id: None,
                channel_id: None,
                payload: serde_json::json!({
                    "connection_id": connection_id.as_str(),
                    "registration": state.as_str(),
                    "registration_failure": state.failure().map(RegistrationFailure::as_str),
                }),
            };
            if let Err(error) = bus.publish(event).await {
                tracing::error!(%error, "publishing a registration change failed");
            }
        }
        if states.changed().await.is_err() {
            return;
        }
    }
}
