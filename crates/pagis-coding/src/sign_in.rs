//! The daemon side of a Harness Sign-In (ADR-0033).
//!
//! The Person starts a sign-in to one Coding Harness on one of their
//! Hosts. [`SignIns`] names what the Client App runs: the vendor's own
//! command from the Harness Catalog, or the ACP terminal method that the
//! harness gives in `initialize`. It sends that as a `harness_sign_in`
//! frame on the Host's connection, and the Client App runs it in a
//! terminal window. The Client App stays a pipe: it gets a command, opens
//! a terminal, and reports the exit.
//!
//! Pagis never reads, copies, stores or relays the credential. The
//! sign-in runs in the vendor's own program on the Person's machine, and
//! the frames carry no credential.
//!
//! [`SignIns::check`] asks the Client App to run the vendor's own status
//! command of a harness. The Client App reads only its exit code and
//! whether its output holds fixed words, and reports a sign-in state.
//!
//! [`SignInReports`] tells, for each harness on each Host, the state of
//! the last check, whether the harness refused a session for a sign-in,
//! and how the last sign-in ended.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use futures::io::AsyncReadExt;
use pagis_broker::{HarnessSignIn, HostDispatchError, HostPresence, SignInOutcome};
use pagis_core::harness::{self, HarnessEntry, SignInAction, SignInMethod, SignInState};
use pagis_core::{
    Clock, CodingSessionId, EventBus, HarnessSignInId, Host, HostId, NewEvent, UnixMillis,
    WorkspaceId,
};
use serde_json::json;
use tokio::sync::Mutex;

use crate::{AcpSession, OpenFailureCode, OpenRequest, Place, SessionPlace};

/// The longest wait for the answer to `initialize` of a probe. The first
/// start of an npx harness on a machine downloads the package.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

/// The directory of a probe: the Person's home directory, which the
/// Client App resolves.
const PROBE_DIRECTORY: &str = "~";

/// Why a Harness Sign-In did not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignInFailure {
    /// The harness is not in the Harness Catalog, or the Host does not
    /// declare `harness:<id>`.
    #[error("the host does not declare the harness {0}")]
    UndeclaredHarness(String),
    /// The Harness Catalog holds no sign-in of this method for the
    /// harness.
    #[error("{harness} has no {method} sign-in")]
    MethodNotOffered {
        harness: &'static str,
        method: &'static str,
    },
    /// The Harness Catalog holds no status command for the harness.
    #[error("{0} has no status command, so Pagis cannot check its sign-in")]
    NoCheck(&'static str),
    /// The Host has no connection, or its session socket is not open for
    /// the probe.
    #[error("the host is not connected")]
    NotConnected,
    /// The harness's `initialize` on the Host offers no terminal method
    /// with the id that the Harness Catalog names.
    #[error("{harness} on this host offers no sign-in method {method_id}")]
    Unavailable {
        harness: &'static str,
        method_id: &'static str,
    },
    /// The probe did not give the harness's sign-in methods.
    #[error("the harness did not give its sign-in methods: {0}")]
    ProbeFailed(String),
}

impl SignInFailure {
    /// The stable code of the failure.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            SignInFailure::UndeclaredHarness(_) => "undeclared_harness",
            SignInFailure::MethodNotOffered { .. } => "method_not_offered",
            SignInFailure::NoCheck(_) => "no_sign_in_check",
            SignInFailure::NotConnected => "host_not_connected",
            SignInFailure::Unavailable { .. } => "sign_in_unavailable",
            SignInFailure::ProbeFailed(_) => "probe_failed",
        }
    }
}

/// The domain event of a change of the sign-in report of one harness on
/// one Host.
pub const SIGN_IN_CHANGED_EVENT: &str = "harness.sign_in_changed";

/// The error of a sign-in whose Host connection closed before the Client
/// App reported how it ended.
const DISCONNECTED: &str = "the machine disconnected before the sign-in ended";

/// One Harness Sign-In that the Person started, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInAttempt {
    pub id: HarnessSignInId,
    /// `None` while the terminal window is open.
    pub outcome: Option<SignInOutcome>,
}

/// What Pagis knows about the sign-in to one harness on one Host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignInReport {
    /// When the harness refused a session for a sign-in, until a session
    /// opens or a sign-in exits with code 0.
    pub needs_sign_in_since: Option<UnixMillis>,
    /// What the last run of the vendor's status command showed.
    pub state: SignInState,
    /// The last Harness Sign-In that the Person started.
    pub last_sign_in: Option<SignInAttempt>,
}

/// The sign-in report of each harness, by Host.
///
/// The report lives in daemon memory, as presence does, so it is empty
/// after a restart. It holds three facts:
///
/// - A start that the harness refuses for a sign-in sets the refusal of
///   its harness on its Host. A `session/new` that succeeds, or a
///   sign-in that exits with code 0, clears it.
/// - The Client App reports the state that the vendor's status command
///   gives. A state of "signed in" does not clear a refusal: the command
///   reads the stored credential and does not test it, so it does not
///   see a credential that expired or that the vendor revoked.
/// - The last Harness Sign-In runs until the Client App reports how it
///   ended, or until its Host connection closes.
///
/// Each change publishes [`SIGN_IN_CHANGED_EVENT`] for the Workspace of
/// the Host.
pub struct SignInReports {
    bus: Arc<dyn EventBus>,
    clock: Arc<dyn Clock>,
    /// The report by Host and harness id. The lock is held across the
    /// publish, so the events come in the order of the changes.
    reports: Mutex<HashMap<(HostId, String), SignInReport>>,
}

impl SignInReports {
    pub fn new(bus: Arc<dyn EventBus>, clock: Arc<dyn Clock>) -> Self {
        Self {
            bus,
            clock,
            reports: Mutex::default(),
        }
    }

    /// The report of `harness` on `host_id`. A harness with no report has
    /// the default: no refusal, an unknown state, and no sign-in.
    pub async fn report(&self, host_id: &HostId, harness: &str) -> SignInReport {
        self.reports
            .lock()
            .await
            .get(&(host_id.clone(), harness.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    /// The harness refused a session for a sign-in.
    pub async fn needs_sign_in(&self, workspace_id: &WorkspaceId, host_id: &HostId, harness: &str) {
        let now = self.clock.now_ms();
        self.change(workspace_id, host_id, harness, |report| {
            if report.needs_sign_in_since.is_some() {
                return false;
            }
            report.needs_sign_in_since = Some(now);
            true
        })
        .await;
    }

    /// The harness opened a session, so it is signed in.
    pub async fn signed_in(&self, workspace_id: &WorkspaceId, host_id: &HostId, harness: &str) {
        self.change(workspace_id, host_id, harness, |report| {
            report.needs_sign_in_since.take().is_some()
        })
        .await;
    }

    /// The Client App ran the vendor's status command of the harness.
    pub async fn checked(
        &self,
        workspace_id: &WorkspaceId,
        host_id: &HostId,
        harness: &str,
        state: SignInState,
    ) {
        self.change(workspace_id, host_id, harness, |report| {
            std::mem::replace(&mut report.state, state) != state
        })
        .await;
    }

    /// The Host received the sign-in `id`.
    pub async fn sign_in_started(
        &self,
        workspace_id: &WorkspaceId,
        host_id: &HostId,
        harness: &str,
        id: &HarnessSignInId,
    ) {
        self.change(workspace_id, host_id, harness, |report| {
            report.last_sign_in = Some(SignInAttempt {
                id: id.clone(),
                outcome: None,
            });
            true
        })
        .await;
    }

    /// The sign-in `id` ended. An exit code of 0 clears the refusal at
    /// once: the next start tells whether the sign-in worked. Any other
    /// end leaves it. The outcome stays with the last sign-in when `id`
    /// is that sign-in.
    pub async fn sign_in_ended(
        &self,
        workspace_id: &WorkspaceId,
        host_id: &HostId,
        harness: &str,
        id: &HarnessSignInId,
        outcome: &SignInOutcome,
    ) {
        self.change(workspace_id, host_id, harness, |report| {
            let mut changed = false;
            if outcome.exit_code == Some(0) {
                changed |= report.needs_sign_in_since.take().is_some();
            }
            if let Some(attempt) = report
                .last_sign_in
                .as_mut()
                .filter(|attempt| &attempt.id == id)
            {
                attempt.outcome = Some(outcome.clone());
                changed = true;
            }
            changed
        })
        .await;
    }

    /// Applies `apply` to the report of one harness, and publishes the
    /// report when `apply` answers that it changed. The report holds the
    /// change also when the event does not reach the clients: their next
    /// read of the Hosts shows it.
    async fn change(
        &self,
        workspace_id: &WorkspaceId,
        host_id: &HostId,
        harness: &str,
        apply: impl FnOnce(&mut SignInReport) -> bool,
    ) {
        let mut reports = self.reports.lock().await;
        let report = reports
            .entry((host_id.clone(), harness.to_string()))
            .or_default();
        if !apply(report) {
            return;
        }
        let payload = json!({
            "host_id": host_id.as_str(),
            "harness": harness,
            "needs_sign_in": report.needs_sign_in_since.is_some(),
            "sign_in_state": report.state,
        });
        let published = self
            .bus
            .publish(NewEvent {
                workspace_id: workspace_id.clone(),
                event_type: SIGN_IN_CHANGED_EVENT.to_string(),
                agent_id: None,
                run_id: None,
                channel_id: None,
                payload,
            })
            .await;
        if let Err(error) = published {
            tracing::warn!(host_id = %host_id, harness, %error, "a change of the sign-in report did not reach the clients");
        }
    }
}

/// The catalog entry of the harness `harness_id`, when `host` declares
/// it.
fn declared(host: &Host, harness_id: &str) -> Result<&'static HarnessEntry, SignInFailure> {
    harness::entry(harness_id)
        .filter(|entry| host.can(&harness::capability(entry.id)))
        .ok_or_else(|| SignInFailure::UndeclaredHarness(harness_id.to_string()))
}

/// The program, the arguments and the environment that the Client App
/// runs in the terminal window.
struct Invocation {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

/// Starts the Harness Sign-Ins that the Person asks for.
pub struct SignIns {
    presence: Arc<HostPresence>,
    place: Arc<dyn SessionPlace>,
    reports: Arc<SignInReports>,
}

impl SignIns {
    /// `presence` carries the frame to the Host, `place` opens the stream
    /// of a probe, and the result of each sign-in goes to `reports`.
    pub fn new(
        presence: Arc<HostPresence>,
        place: Arc<dyn SessionPlace>,
        reports: Arc<SignInReports>,
    ) -> Self {
        Self {
            presence,
            place,
            reports,
        }
    }

    /// Start one sign-in of `method` to the harness `harness_id` on
    /// `host`, and answer its id when the frame is sent.
    ///
    /// The result comes later, when the Person closes the terminal
    /// window. The daemon logs it and gives it to the sign-in report. A
    /// Host that is not connected fails at once.
    pub async fn start(
        &self,
        host: &Host,
        harness_id: &str,
        method: SignInMethod,
    ) -> Result<HarnessSignInId, SignInFailure> {
        let entry = declared(host, harness_id)?;
        let action = entry
            .sign_in
            .iter()
            .find(|sign_in| sign_in.method == method)
            .ok_or(SignInFailure::MethodNotOffered {
                harness: entry.id,
                method: method.as_str(),
            })?
            .how;
        if !self.presence.present(&host.id) {
            return Err(SignInFailure::NotConnected);
        }
        let invocation = match action {
            SignInAction::Command(argv) => {
                let (command, args) = argv
                    .split_first()
                    .expect("each sign-in command of the Harness Catalog names a program");
                Invocation {
                    command: (*command).to_string(),
                    args: args.iter().map(|arg| (*arg).to_string()).collect(),
                    env: BTreeMap::new(),
                }
            }
            SignInAction::TerminalAuth(method_id) => {
                self.terminal_auth(host, entry, method_id).await?
            }
        };

        let id = HarnessSignInId::generate();
        let answer = self
            .presence
            .sign_in(
                &host.id,
                HarnessSignIn {
                    id: id.clone(),
                    harness: entry.id.to_string(),
                    name: entry.label.to_string(),
                    command: invocation.command,
                    args: invocation.args,
                    env: invocation.env,
                },
            )
            // A sign-in has no deadline, so the one error is absence.
            .map_err(|_: HostDispatchError| SignInFailure::NotConnected)?;
        let (workspace_id, host_id, harness) =
            (host.workspace_id.clone(), host.id.clone(), entry.id);
        self.reports
            .sign_in_started(&workspace_id, &host_id, harness, &id)
            .await;
        let sign_in_id = id.clone();
        let reports = Arc::clone(&self.reports);
        tokio::spawn(async move {
            let outcome = match answer.await {
                Ok(outcome) => {
                    tracing::info!(
                        host_id = %host_id,
                        harness,
                        sign_in_id = %sign_in_id,
                        exit_code = ?outcome.exit_code,
                        error = outcome.error.as_deref(),
                        "a Harness Sign-In ended"
                    );
                    outcome
                }
                Err(_) => {
                    tracing::info!(
                        host_id = %host_id,
                        harness,
                        sign_in_id = %sign_in_id,
                        "a Harness Sign-In has no result: the Host connection that received it closed"
                    );
                    SignInOutcome {
                        exit_code: None,
                        error: Some(DISCONNECTED.to_string()),
                    }
                }
            };
            reports
                .sign_in_ended(&workspace_id, &host_id, harness, &sign_in_id, &outcome)
                .await;
        });
        Ok(id)
    }

    /// Ask `host` to run the vendor's status command of the harness
    /// `harness_id`. The Client App reports the state on its own frame,
    /// and the report publishes a change. A Host that is not connected
    /// fails at once.
    pub fn check(&self, host: &Host, harness_id: &str) -> Result<(), SignInFailure> {
        let entry = declared(host, harness_id)?;
        if entry.sign_in_check.is_none() {
            return Err(SignInFailure::NoCheck(entry.id));
        }
        self.presence
            .check_sign_in(&host.id, entry.id)
            .map_err(|_: HostDispatchError| SignInFailure::NotConnected)
    }

    /// The invocation of the ACP terminal method `method_id`: the launch
    /// command of the harness with the method's `args`, and its `env`.
    ///
    /// The probe runs on the same stream path as a session, so the
    /// harness gives its methods at its pinned version on that machine.
    async fn terminal_auth(
        &self,
        host: &Host,
        entry: &'static HarnessEntry,
        method_id: &'static str,
    ) -> Result<Invocation, SignInFailure> {
        let (command, launch_args) = harness::launch_command(entry);
        let request = OpenRequest {
            session_id: CodingSessionId::generate(),
            command: command.to_string(),
            args: launch_args.iter().map(|arg| (*arg).to_string()).collect(),
            cwd: PROBE_DIRECTORY.to_string(),
            env: BTreeMap::new(),
            worktree: None,
        };
        let opened = self
            .place
            .open(&host.workspace_id, &Place::Host(host.id.clone()), request)
            .await
            .map_err(|failure| match failure.code {
                OpenFailureCode::HostNotConnected => SignInFailure::NotConnected,
                _ => SignInFailure::ProbeFailed(failure.message),
            })?;
        let (incoming, outgoing) = opened.stream.split();
        let info = tokio::time::timeout(PROBE_TIMEOUT, AcpSession::probe(outgoing, incoming))
            .await
            .map_err(|_| {
                SignInFailure::ProbeFailed(format!(
                    "no answer to initialize in {} seconds",
                    PROBE_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|error| SignInFailure::ProbeFailed(error.to_string()))?;
        let method = info
            .sign_in_methods
            .into_iter()
            .find(|method| method.id == method_id)
            .ok_or(SignInFailure::Unavailable {
                harness: entry.id,
                method_id,
            })?;
        Ok(Invocation {
            command: command.to_string(),
            args: launch_args
                .into_iter()
                .map(str::to_string)
                .chain(method.args)
                .collect(),
            env: method.env,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use pagis_broker::{HostConnection, HostFrame};
    use pagis_core::{
        Event, EventId, EventScope, EventStream, HostId, StoreError, SystemClock, WorkspaceId,
    };
    use tokio::sync::oneshot;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    use super::*;
    use crate::fake::{FakeHarness, Script, acp};
    use crate::{OpenFailure, OpenedStream};

    const WAIT: Duration = Duration::from_secs(10);

    /// A place that runs a fake harness on each stream it opens, and
    /// keeps each request and each harness.
    struct DuplexPlace {
        script: Script,
        requests: Mutex<Vec<OpenRequest>>,
        harnesses: Mutex<Vec<FakeHarness>>,
    }

    impl DuplexPlace {
        fn new(script: Script) -> Arc<Self> {
            Arc::new(Self {
                script,
                requests: Mutex::default(),
                harnesses: Mutex::default(),
            })
        }

        fn requests(&self) -> Vec<OpenRequest> {
            self.requests.lock().unwrap().clone()
        }

        fn harness(&self) -> FakeHarness {
            self.harnesses.lock().unwrap()[0].clone()
        }
    }

    #[async_trait]
    impl SessionPlace for DuplexPlace {
        async fn open(
            &self,
            _workspace_id: &WorkspaceId,
            _place: &Place,
            request: OpenRequest,
        ) -> Result<OpenedStream, OpenFailure> {
            self.requests.lock().unwrap().push(request);
            let (daemon_end, harness_end) = tokio::io::duplex(64 * 1024);
            let (harness_read, harness_write) = tokio::io::split(harness_end);
            let harness = FakeHarness::serve(
                self.script.clone(),
                harness_write.compat_write(),
                harness_read.compat(),
            );
            self.harnesses.lock().unwrap().push(harness);
            let (_exit, exited) = oneshot::channel();
            Ok(OpenedStream {
                stream: Box::new(daemon_end.compat()),
                cwd: "/Users/bo".to_string(),
                exit: exited,
                fixed_model: false,
            })
        }
    }

    fn host(capabilities: &[&str]) -> Host {
        Host {
            id: HostId::generate(),
            workspace_id: WorkspaceId::generate(),
            name: "Air".to_string(),
            platform: "macos".to_string(),
            capabilities: capabilities
                .iter()
                .map(|held| (*held).to_string())
                .collect(),
            last_seen_at: 0,
            created_at: 0,
        }
    }

    /// The fake harness of Claude Code: its `initialize` gives the
    /// terminal method `claude-ai-login`.
    fn claude_script() -> Script {
        Script::default()
            .agent("claude-agent-acp", "0.87.0")
            .auth_method(acp::AuthMethod::Terminal(
                acp::AuthMethodTerminal::new("claude-ai-login", "Use Claude subscription")
                    .args(vec!["--cli".to_owned(), "auth".to_owned()])
                    .env(HashMap::from([(
                        "CLAUDE_CODE_LOGIN".to_owned(),
                        "subscription".to_owned(),
                    )])),
            ))
    }

    /// A present Host: the presence holds its connection.
    fn present(host: &Host) -> (Arc<HostPresence>, HostConnection) {
        let presence = Arc::new(HostPresence::new());
        let connection = presence.connect(&host.id);
        (presence, connection)
    }

    async fn next_sign_in(connection: &mut HostConnection) -> HarnessSignIn {
        match tokio::time::timeout(WAIT, connection.next())
            .await
            .expect("a frame before the timeout")
            .expect("the connection stays registered")
        {
            HostFrame::HarnessSignIn(sign_in) => sign_in,
            other => panic!("another frame arrived: {other:?}"),
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    /// A bus that keeps each event it gets.
    #[derive(Default)]
    struct KeptBus {
        events: Mutex<Vec<NewEvent>>,
    }

    impl KeptBus {
        fn payloads(&self) -> Vec<serde_json::Value> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .map(|event| {
                    assert_eq!(event.event_type, SIGN_IN_CHANGED_EVENT);
                    event.payload.clone()
                })
                .collect()
        }

        /// The `needs_sign_in` of each change, oldest first.
        fn refusals(&self) -> Vec<bool> {
            self.payloads()
                .iter()
                .map(|payload| payload["needs_sign_in"].as_bool().unwrap())
                .collect()
        }

        /// The `sign_in_state` of each change, oldest first.
        fn states(&self) -> Vec<String> {
            self.payloads()
                .iter()
                .map(|payload| payload["sign_in_state"].as_str().unwrap().to_string())
                .collect()
        }
    }

    #[async_trait]
    impl EventBus for KeptBus {
        async fn publish(&self, event: NewEvent) -> Result<Event, StoreError> {
            self.events.lock().unwrap().push(event.clone());
            Ok(Event {
                id: EventId::generate(),
                seq: 1,
                workspace_id: event.workspace_id,
                event_type: event.event_type,
                agent_id: event.agent_id,
                run_id: event.run_id,
                channel_id: event.channel_id,
                payload: event.payload,
                created_at: 0,
            })
        }

        async fn subscribe(&self, _: EventScope, _: Option<i64>) -> EventStream {
            Box::pin(futures::stream::empty())
        }
    }

    fn reports_on(bus: Arc<KeptBus>) -> Arc<SignInReports> {
        Arc::new(SignInReports::new(bus, Arc::new(SystemClock)))
    }

    fn reports() -> Arc<SignInReports> {
        reports_on(Arc::default())
    }

    /// The report of Codex on `host_id`, once `done` holds for it.
    async fn eventually(
        reports: &SignInReports,
        host_id: &HostId,
        done: impl Fn(&SignInReport) -> bool,
    ) -> SignInReport {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let report = reports.report(host_id, "codex").await;
            if done(&report) {
                return report;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the report did not change: {report:?}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn exited(exit_code: Option<i64>) -> SignInOutcome {
        SignInOutcome {
            exit_code,
            error: exit_code
                .is_none()
                .then(|| "the terminal did not open".to_string()),
        }
    }

    fn needs(report: &SignInReport) -> bool {
        report.needs_sign_in_since.is_some()
    }

    #[tokio::test]
    async fn a_sign_in_that_exits_0_clears_the_refusal_and_any_other_end_leaves_it() {
        let host = host(&["harness:codex"]);
        let bus = Arc::new(KeptBus::default());
        let reports = reports_on(Arc::clone(&bus));
        let (workspace_id, host_id) = (&host.workspace_id, &host.id);
        reports.needs_sign_in(workspace_id, host_id, "codex").await;
        assert!(needs(&reports.report(host_id, "codex").await));

        for other in [Some(1), None] {
            let id = HarnessSignInId::generate();
            reports
                .sign_in_started(workspace_id, host_id, "codex", &id)
                .await;
            reports
                .sign_in_ended(workspace_id, host_id, "codex", &id, &exited(other))
                .await;
            assert!(
                needs(&reports.report(host_id, "codex").await),
                "the end {other:?} cleared the refusal"
            );
        }
        let id = HarnessSignInId::generate();
        reports
            .sign_in_started(workspace_id, host_id, "codex", &id)
            .await;
        reports
            .sign_in_ended(workspace_id, host_id, "codex", &id, &exited(Some(0)))
            .await;

        assert!(!needs(&reports.report(host_id, "codex").await));
        assert_eq!(bus.refusals().first(), Some(&true));
        assert_eq!(bus.refusals().last(), Some(&false));
    }

    #[tokio::test]
    async fn a_report_changes_once_for_each_change_and_holds_each_harness_apart() {
        let host = host(&["harness:codex", "harness:claude"]);
        let bus = Arc::new(KeptBus::default());
        let reports = reports_on(Arc::clone(&bus));
        let (workspace_id, host_id) = (&host.workspace_id, &host.id);

        reports.signed_in(workspace_id, host_id, "codex").await;
        reports.needs_sign_in(workspace_id, host_id, "codex").await;
        reports.needs_sign_in(workspace_id, host_id, "codex").await;

        assert!(needs(&reports.report(host_id, "codex").await));
        assert_eq!(
            reports.report(host_id, "claude").await,
            SignInReport::default()
        );
        assert_eq!(
            reports.report(&HostId::generate(), "codex").await,
            SignInReport::default()
        );
        assert_eq!(bus.refusals(), [true]);
        let event = bus.events.lock().unwrap()[0].clone();
        assert_eq!(&event.workspace_id, workspace_id);
        assert_eq!(
            event.payload,
            json!({
                "host_id": host_id.as_str(),
                "harness": "codex",
                "needs_sign_in": true,
                "sign_in_state": "unknown",
            })
        );
    }

    #[tokio::test]
    async fn a_check_sets_the_state_and_publishes_only_a_change() {
        let host = host(&["harness:codex"]);
        let bus = Arc::new(KeptBus::default());
        let reports = reports_on(Arc::clone(&bus));
        let (workspace_id, host_id) = (&host.workspace_id, &host.id);

        reports
            .checked(workspace_id, host_id, "codex", SignInState::Unknown)
            .await;
        reports
            .checked(workspace_id, host_id, "codex", SignInState::NotSignedIn)
            .await;
        reports
            .checked(workspace_id, host_id, "codex", SignInState::NotSignedIn)
            .await;
        reports
            .checked(workspace_id, host_id, "codex", SignInState::SignedIn)
            .await;

        assert_eq!(
            reports.report(host_id, "codex").await.state,
            SignInState::SignedIn
        );
        assert_eq!(bus.states(), ["not_signed_in", "signed_in"]);
    }

    /// A status command reads the stored credential and does not test
    /// it, so a check that says "signed in" does not hide a session that
    /// the harness refused: the credential expired, or the vendor revoked
    /// it.
    #[tokio::test]
    async fn a_check_that_says_signed_in_leaves_the_refusal_of_a_session() {
        let host = host(&["harness:claude"]);
        let reports = reports();
        let (workspace_id, host_id) = (&host.workspace_id, &host.id);
        reports
            .checked(workspace_id, host_id, "claude", SignInState::SignedIn)
            .await;
        reports.needs_sign_in(workspace_id, host_id, "claude").await;

        reports
            .checked(workspace_id, host_id, "claude", SignInState::SignedIn)
            .await;

        let report = reports.report(host_id, "claude").await;
        assert!(needs(&report));
        assert_eq!(report.state, SignInState::SignedIn);
    }

    #[tokio::test]
    async fn the_last_sign_in_runs_until_its_result_and_then_keeps_how_it_ended() {
        let host = host(&["harness:codex"]);
        let bus = Arc::new(KeptBus::default());
        let reports = reports_on(Arc::clone(&bus));
        let (workspace_id, host_id) = (&host.workspace_id, &host.id);
        let id = HarnessSignInId::generate();

        reports
            .sign_in_started(workspace_id, host_id, "codex", &id)
            .await;
        assert_eq!(
            reports.report(host_id, "codex").await.last_sign_in,
            Some(SignInAttempt {
                id: id.clone(),
                outcome: None
            })
        );
        let timed_out = SignInOutcome {
            exit_code: None,
            error: Some("The sign-in did not end in 30 minutes".to_string()),
        };
        reports
            .sign_in_ended(workspace_id, host_id, "codex", &id, &timed_out)
            .await;

        assert_eq!(
            reports.report(host_id, "codex").await.last_sign_in,
            Some(SignInAttempt {
                id,
                outcome: Some(timed_out)
            })
        );
        assert_eq!(bus.events.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn the_end_of_an_older_sign_in_does_not_replace_the_newer_one() {
        let host = host(&["harness:codex"]);
        let reports = reports();
        let (workspace_id, host_id) = (&host.workspace_id, &host.id);
        let (older, newer) = (HarnessSignInId::generate(), HarnessSignInId::generate());
        reports
            .sign_in_started(workspace_id, host_id, "codex", &older)
            .await;
        reports
            .sign_in_started(workspace_id, host_id, "codex", &newer)
            .await;

        reports
            .sign_in_ended(workspace_id, host_id, "codex", &older, &exited(Some(1)))
            .await;

        assert_eq!(
            reports.report(host_id, "codex").await.last_sign_in,
            Some(SignInAttempt {
                id: newer,
                outcome: None
            })
        );
    }

    /// The `harness_sign_in_result` of the Client App reaches the report.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_result_of_the_client_app_ends_the_sign_in_and_clears_the_refusal() {
        let host = host(&["shell", "harness:codex"]);
        let (presence, mut connection) = present(&host);
        let reports = reports();
        reports
            .needs_sign_in(&host.workspace_id, &host.id, "codex")
            .await;
        let sign_ins = SignIns::new(
            Arc::clone(&presence),
            DuplexPlace::new(Script::default()),
            Arc::clone(&reports),
        );
        let id = sign_ins
            .start(&host, "codex", SignInMethod::Subscription)
            .await
            .expect("the sign-in starts");
        assert_eq!(
            reports.report(&host.id, "codex").await.last_sign_in,
            Some(SignInAttempt {
                id: id.clone(),
                outcome: None
            })
        );
        let sign_in = next_sign_in(&mut connection).await;

        presence.complete_sign_in(&connection, &sign_in.id, exited(Some(0)));

        let report = eventually(&reports, &host.id, |report| !needs(report)).await;
        assert_eq!(
            report.last_sign_in,
            Some(SignInAttempt {
                id,
                outcome: Some(exited(Some(0)))
            })
        );
    }

    /// A sign-in whose Host connection closes has no result, and the
    /// report says so.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_sign_in_whose_connection_closes_ends_with_no_exit_code() {
        let host = host(&["shell", "harness:codex"]);
        let (presence, mut connection) = present(&host);
        let reports = reports();
        let sign_ins = SignIns::new(
            Arc::clone(&presence),
            DuplexPlace::new(Script::default()),
            Arc::clone(&reports),
        );
        sign_ins
            .start(&host, "codex", SignInMethod::Subscription)
            .await
            .expect("the sign-in starts");
        next_sign_in(&mut connection).await;

        drop(connection);

        let report = eventually(&reports, &host.id, |report| {
            report
                .last_sign_in
                .as_ref()
                .is_some_and(|attempt| attempt.outcome.is_some())
        })
        .await;
        let outcome = report.last_sign_in.unwrap().outcome.unwrap();
        assert_eq!(outcome.exit_code, None);
        assert_eq!(
            outcome.error.as_deref(),
            Some("the machine disconnected before the sign-in ended")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_check_sends_the_harness_to_the_host() {
        let host = host(&["shell", "harness:codex"]);
        let (presence, mut connection) = present(&host);
        let sign_ins = SignIns::new(presence, DuplexPlace::new(Script::default()), reports());

        sign_ins.check(&host, "codex").expect("the check is sent");

        match tokio::time::timeout(WAIT, connection.next())
            .await
            .expect("a frame before the timeout")
            .expect("the connection stays registered")
        {
            HostFrame::SignInCheck { harness } => assert_eq!(harness, "codex"),
            other => panic!("another frame arrived: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_check_of_a_harness_with_no_status_command_or_on_an_absent_host_is_refused() {
        let host = host(&["shell", "harness:gemini", "harness:codex"]);
        let (presence, _connection) = present(&host);
        let sign_ins = SignIns::new(presence, DuplexPlace::new(Script::default()), reports());
        let absent = SignIns::new(
            Arc::new(HostPresence::new()),
            DuplexPlace::new(Script::default()),
            reports(),
        );

        assert_eq!(
            sign_ins
                .check(&host, "gemini")
                .map_err(|failure| failure.code()),
            Err("no_sign_in_check")
        );
        assert_eq!(
            sign_ins
                .check(&host, "claude")
                .map_err(|failure| failure.code()),
            Err("undeclared_harness")
        );
        assert_eq!(
            absent.check(&host, "codex"),
            Err(SignInFailure::NotConnected)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_vendor_command_is_the_invocation_and_opens_no_probe() {
        let host = host(&["shell", "harness:codex"]);
        let (presence, mut connection) = present(&host);
        let place = DuplexPlace::new(Script::default());
        let sign_ins = SignIns::new(Arc::clone(&presence), place.clone(), reports());

        let id = sign_ins
            .start(&host, "codex", SignInMethod::ApiKey)
            .await
            .expect("the sign-in starts");

        let sign_in = next_sign_in(&mut connection).await;
        assert_eq!(sign_in.id, id);
        assert_eq!(sign_in.harness, "codex");
        assert_eq!(sign_in.name, "Codex");
        assert_eq!(sign_in.command, "npx");
        assert_eq!(
            sign_in.args,
            strings(&["--yes", "@openai/codex@0.159.1", "login", "--with-api-key"])
        );
        assert!(sign_in.env.is_empty());
        assert!(
            place.requests().is_empty(),
            "a vendor command needs no probe"
        );
        // The result of the Client App reaches the daemon's wait.
        presence.complete_sign_in(
            &connection,
            &sign_in.id,
            SignInOutcome {
                exit_code: Some(0),
                error: None,
            },
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_terminal_method_is_the_launch_command_with_its_args_and_env() {
        let host = host(&["harness:claude"]);
        let (presence, mut connection) = present(&host);
        let place = DuplexPlace::new(claude_script());
        let sign_ins = SignIns::new(presence, place.clone(), reports());

        sign_ins
            .start(&host, "claude", SignInMethod::Subscription)
            .await
            .expect("the sign-in starts");

        let sign_in = next_sign_in(&mut connection).await;
        assert_eq!(sign_in.name, "Claude Code");
        assert_eq!(sign_in.command, "npx");
        assert_eq!(
            sign_in.args,
            strings(&[
                "--yes",
                "@agentclientprotocol/claude-agent-acp@0.87.0",
                "--cli",
                "auth"
            ])
        );
        assert_eq!(
            sign_in.env,
            BTreeMap::from([("CLAUDE_CODE_LOGIN".to_string(), "subscription".to_string())])
        );
        // The probe runs the launch command in the Person's home directory.
        let requests = place.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].command, "npx");
        assert_eq!(
            requests[0].args,
            strings(&["--yes", "@agentclientprotocol/claude-agent-acp@0.87.0"])
        );
        assert_eq!(requests[0].cwd, "~");
        assert_eq!(requests[0].worktree, None);
    }

    /// The probe asks `initialize` alone: it opens no session, so a
    /// harness that needs a sign-in still gives its methods.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_probe_sends_initialize_alone() {
        let host = host(&["harness:claude"]);
        let (presence, _connection) = present(&host);
        let place = DuplexPlace::new(claude_script().new_session_auth_required());
        let sign_ins = SignIns::new(presence, place.clone(), reports());

        sign_ins
            .start(&host, "claude", SignInMethod::Subscription)
            .await
            .expect("the sign-in starts");

        let methods: Vec<String> = place
            .harness()
            .received()
            .into_iter()
            .map(|received| received.method)
            .collect();
        assert_eq!(methods, ["initialize"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_method_the_harness_does_not_offer_is_unavailable_and_sends_nothing() {
        let host = host(&["harness:claude"]);
        let (presence, mut connection) = present(&host);
        // The harness offers `claude-ai-login`, and the API key sign-in
        // of the catalog is `console-login`.
        let place = DuplexPlace::new(claude_script());
        let sign_ins = SignIns::new(presence, place, reports());

        let failure = sign_ins
            .start(&host, "claude", SignInMethod::ApiKey)
            .await
            .expect_err("the harness offers no such method");

        assert_eq!(failure.code(), "sign_in_unavailable");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), connection.next())
                .await
                .is_err(),
            "the Host received a frame"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_harness_the_host_does_not_declare_is_refused() {
        let host = host(&["shell"]);
        let (presence, _connection) = present(&host);
        let sign_ins = SignIns::new(presence, DuplexPlace::new(Script::default()), reports());

        let failure = sign_ins
            .start(&host, "codex", SignInMethod::Subscription)
            .await
            .expect_err("the Host does not declare codex");

        assert_eq!(
            failure,
            SignInFailure::UndeclaredHarness("codex".to_string())
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_method_the_catalog_does_not_hold_is_refused() {
        let host = host(&["harness:copilot"]);
        let (presence, _connection) = present(&host);
        let sign_ins = SignIns::new(presence, DuplexPlace::new(Script::default()), reports());

        let failure = sign_ins
            .start(&host, "copilot", SignInMethod::ApiKey)
            .await
            .expect_err("Copilot has no API key sign-in");

        assert_eq!(failure.code(), "method_not_offered");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_absent_host_fails_at_once_without_a_probe() {
        let host = host(&["harness:claude"]);
        let place = DuplexPlace::new(claude_script());
        let sign_ins = SignIns::new(Arc::new(HostPresence::new()), place.clone(), reports());

        let failure = sign_ins
            .start(&host, "claude", SignInMethod::Subscription)
            .await
            .expect_err("the Host is not connected");

        assert_eq!(failure, SignInFailure::NotConnected);
        assert!(place.requests().is_empty());
    }
}
