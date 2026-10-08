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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::io::AsyncReadExt;
use pagis_broker::{HarnessSignIn, HostDispatchError, HostPresence};
use pagis_core::harness::{self, HarnessEntry, SignInAction, SignInMethod};
use pagis_core::{CodingSessionId, HarnessSignInId, Host};

use crate::{AcpSession, OpenFailureCode, OpenRequest, SessionPlace};

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
            SignInFailure::NotConnected => "host_not_connected",
            SignInFailure::Unavailable { .. } => "sign_in_unavailable",
            SignInFailure::ProbeFailed(_) => "probe_failed",
        }
    }
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
}

impl SignIns {
    /// `presence` carries the frame to the Host, and `place` opens the
    /// stream of a probe.
    pub fn new(presence: Arc<HostPresence>, place: Arc<dyn SessionPlace>) -> Self {
        Self { presence, place }
    }

    /// Start one sign-in of `method` to the harness `harness_id` on
    /// `host`, and answer its id when the frame is sent.
    ///
    /// The result comes later, when the Person closes the terminal
    /// window, and the daemon logs it. A Host that is not connected fails
    /// at once.
    pub async fn start(
        &self,
        host: &Host,
        harness_id: &str,
        method: SignInMethod,
    ) -> Result<HarnessSignInId, SignInFailure> {
        let entry = harness::entry(harness_id)
            .filter(|entry| host.can(&harness::capability(entry.id)))
            .ok_or_else(|| SignInFailure::UndeclaredHarness(harness_id.to_string()))?;
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
        let (host_id, harness) = (host.id.clone(), entry.id);
        let sign_in_id = id.clone();
        tokio::spawn(async move {
            match answer.await {
                Ok(outcome) => tracing::info!(
                    host_id = %host_id,
                    harness,
                    sign_in_id = %sign_in_id,
                    exit_code = ?outcome.exit_code,
                    error = outcome.error.as_deref(),
                    "a Harness Sign-In ended"
                ),
                Err(_) => tracing::info!(
                    host_id = %host_id,
                    harness,
                    sign_in_id = %sign_in_id,
                    "a Harness Sign-In has no result: the Host connection that received it closed"
                ),
            }
        });
        Ok(id)
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
            .open(&host.workspace_id, &host.id, request)
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
    use pagis_broker::{HostConnection, HostFrame, SignInOutcome};
    use pagis_core::{HostId, WorkspaceId};
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
            _host_id: &HostId,
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
            HostFrame::Dispatch(command) => panic!("a command arrived: {command:?}"),
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_vendor_command_is_the_invocation_and_opens_no_probe() {
        let host = host(&["shell", "harness:codex"]);
        let (presence, mut connection) = present(&host);
        let place = DuplexPlace::new(Script::default());
        let sign_ins = SignIns::new(Arc::clone(&presence), place.clone());

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
        let sign_ins = SignIns::new(presence, place.clone());

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
        let sign_ins = SignIns::new(presence, place.clone());

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
        let sign_ins = SignIns::new(presence, place);

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
        let sign_ins = SignIns::new(presence, DuplexPlace::new(Script::default()));

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
        let sign_ins = SignIns::new(presence, DuplexPlace::new(Script::default()));

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
        let sign_ins = SignIns::new(Arc::new(HostPresence::new()), place.clone());

        let failure = sign_ins
            .start(&host, "claude", SignInMethod::Subscription)
            .await
            .expect_err("the Host is not connected");

        assert_eq!(failure, SignInFailure::NotConnected);
        assert!(place.requests().is_empty());
    }
}
