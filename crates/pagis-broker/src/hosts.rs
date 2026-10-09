//! Presence and dispatch for the machines a person's clients run on.
//!
//! A Host is present while the daemon holds its connection. That is
//! memory of the running process and never a record: a daemon that
//! restarts holds no connection, so nothing is present until the clients
//! come back. The record keeps the last time each machine was seen.
//!
//! [`HostPresence`] is the one place that knows which machines are here
//! now. The connection handler registers one while it holds the socket
//! and answers the commands it receives; the broker asks whether a
//! machine is present, and the host tool sends a command through it.
//!
//! Absence is an answer and not a wait: [`HostPresence::run`] fails at
//! once for a machine that is not connected, and a machine that is
//! connected but silent fails after the caller's own deadline.
//!
//! The same connection carries each Harness Sign-In that the Person
//! starts on the machine ([`HostPresence::sign_in`]). The person signs in
//! in a terminal window, which can take minutes, so a sign-in has no
//! deadline here: the Client App answers it when the window ends, and
//! the end of the connection ends the wait.
//!
//! [`HostPresence::departures`] tells each listener when a machine goes
//! away, so the Coding Sessions on it are interrupted.

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pagis_core::{HarnessSignInId, HostId};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};

/// One command the daemon sends to a machine, as the client reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostCommand {
    /// What the answer comes back under. The daemon mints it and
    /// correlates the result with it.
    pub id: String,
    pub host_id: String,
    pub command: String,
    /// How long the client may take. The client stops the command at the
    /// deadline, so a machine that is present answers within it.
    pub timeout_ms: u64,
    /// True when an Allow Rule approved the command, and false when the
    /// person approved it on its card. The client runs a rule-approved
    /// command under `/bin/sh`, the dialect that the rule check parsed,
    /// and a card-approved command in the person's own shell (ADR-0015).
    pub approved_by_rule: bool,
}

/// What one machine answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostOutcome {
    /// The exit code, or `None` when a signal stopped the command.
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
}

/// One Harness Sign-In that the Client App runs in a terminal window on
/// the machine (ADR-0033): the vendor's own program, with `args` and
/// `env`. It carries no credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessSignIn {
    /// What the result comes back under.
    pub id: HarnessSignInId,
    /// The id of the harness in the Harness Catalog.
    pub harness: String,
    /// The name of the harness that the person reads.
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// How one Harness Sign-In ended on the machine. The exit code tells how
/// the vendor's program ended, not that the person signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInOutcome {
    /// `None` when the vendor's program gave no exit code. `error` then
    /// tells why.
    pub exit_code: Option<i64>,
    pub error: Option<String>,
}

/// One frame that the daemon sends to a machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostFrame {
    Dispatch(HostCommand),
    HarnessSignIn(HarnessSignIn),
    /// Run the status command of the harness `harness` of the Harness
    /// Catalog and report the sign-in state (ADR-0033). No answer waits.
    SignInCheck {
        harness: String,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HostDispatchError {
    #[error("the host is not connected")]
    NotConnected,
    #[error("the host did not answer within {0:?}")]
    Timeout(Duration),
}

/// The live connection of one machine. The handler that holds the socket
/// holds this, and dropping it makes the machine absent, whatever ended
/// the connection. Only this connection answers the commands it
/// receives.
pub struct HostConnection {
    host_id: HostId,
    presence: Arc<HostPresence>,
    frames: mpsc::UnboundedReceiver<HostFrame>,
    /// Which registration this is, so a connection that has already been
    /// replaced does not deregister its replacement, and does not answer
    /// a command that its replacement received.
    epoch: u64,
}

impl HostConnection {
    /// The next frame for this machine, or `None` once the presence
    /// registry has dropped the connection.
    pub async fn next(&mut self) -> Option<HostFrame> {
        self.frames.recv().await
    }

    pub fn host_id(&self) -> &HostId {
        &self.host_id
    }
}

impl Drop for HostConnection {
    fn drop(&mut self) {
        let mut state = self.presence.state.lock().expect("host presence lock");
        // A second connection of the same machine has already replaced
        // this one, so only the connection that is still registered
        // deregisters.
        if state
            .connected
            .get(&self.host_id)
            .is_some_and(|held| held.epoch == self.epoch)
        {
            state.connected.remove(&self.host_id);
            // Nobody listens when no Coding Session runtime exists.
            let _ = self.presence.departures.send(self.host_id.clone());
        }
        // Only this connection answers the commands and the sign-ins it
        // took, and nothing more comes from it, replaced or not. So they
        // answer now. Dropping the answer channel is that answer: the
        // caller reads it as an absent machine and the person is not left
        // waiting out the deadline.
        state
            .waiting
            .retain(|_, waiting| !waiting.received_by(self));
        state
            .signing_in
            .retain(|_, waiting| !waiting.received_by(self));
    }
}

struct Connected {
    frames: mpsc::UnboundedSender<HostFrame>,
    /// Which registration this is. A client that reconnects before the
    /// daemon noticed the old socket replaces it, and the old one must
    /// not deregister the new one on its way out.
    epoch: u64,
}

/// One command or sign-in in flight, and the connection that received
/// it: the machine and the registration.
struct Waiting<T> {
    host_id: HostId,
    epoch: u64,
    answer: oneshot::Sender<T>,
}

impl<T> Waiting<T> {
    fn received_by(&self, connection: &HostConnection) -> bool {
        self.host_id == connection.host_id && self.epoch == connection.epoch
    }
}

/// Take the waiting entry of `key` when `from` received it. An answer
/// from another connection, or for an entry nobody waits for, takes
/// nothing.
fn take_received<K, Q, T>(
    waiting: &mut HashMap<K, Waiting<T>>,
    key: &Q,
    from: &HostConnection,
) -> Option<Waiting<T>>
where
    K: Borrow<Q> + Hash + Eq,
    Q: Hash + Eq + ?Sized,
{
    if waiting
        .get(key)
        .is_some_and(|waiting| waiting.received_by(from))
    {
        waiting.remove(key)
    } else {
        None
    }
}

#[derive(Default)]
struct PresenceState {
    connected: HashMap<HostId, Connected>,
    waiting: HashMap<String, Waiting<HostOutcome>>,
    signing_in: HashMap<HarnessSignInId, Waiting<SignInOutcome>>,
    next_epoch: u64,
    next_call: u64,
}

/// How many departures wait for a slow listener before it misses one.
const DEPARTURES: usize = 256;

/// Which machines are here now, and the commands in flight on them.
pub struct HostPresence {
    state: Mutex<PresenceState>,
    departures: broadcast::Sender<HostId>,
}

impl Default for HostPresence {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            departures: broadcast::channel(DEPARTURES).0,
        }
    }
}

impl HostPresence {
    pub fn new() -> Self {
        Self::default()
    }

    /// The machines that go away: the id of each Host whose registered
    /// connection drops. A connection that a newer one of the same
    /// machine replaced sends nothing, because the machine stays.
    pub fn departures(&self) -> broadcast::Receiver<HostId> {
        self.departures.subscribe()
    }

    /// Make one machine present, and answer the connection that holds it
    /// so. A second registration of one machine replaces the first: the
    /// client that is connected now is the one that runs the commands.
    pub fn connect(self: &Arc<Self>, host_id: &HostId) -> HostConnection {
        let (sender, frames) = mpsc::unbounded_channel();
        let epoch = {
            let mut state = self.state.lock().expect("host presence lock");
            state.next_epoch += 1;
            let epoch = state.next_epoch;
            state.connected.insert(
                host_id.clone(),
                Connected {
                    frames: sender,
                    epoch,
                },
            );
            epoch
        };
        HostConnection {
            host_id: host_id.clone(),
            presence: Arc::clone(self),
            frames,
            epoch,
        }
    }

    pub fn present(&self, host_id: &HostId) -> bool {
        self.state
            .lock()
            .expect("host presence lock")
            .connected
            .contains_key(host_id)
    }

    /// Run one command on a machine and wait for its answer.
    /// `approved_by_rule` tells the client whether an Allow Rule approved
    /// the command, which decides the shell it runs in.
    ///
    /// A machine that is not connected fails at once, so the tool answers
    /// the person instead of hanging. A machine that took the command and
    /// said nothing fails at `timeout`.
    pub async fn run(
        &self,
        host_id: &HostId,
        command: &str,
        approved_by_rule: bool,
        timeout: Duration,
    ) -> Result<HostOutcome, HostDispatchError> {
        let (call_id, answer) = {
            let mut state = self.state.lock().expect("host presence lock");
            let Some((frames, epoch)) = state
                .connected
                .get(host_id)
                .map(|connected| (connected.frames.clone(), connected.epoch))
            else {
                return Err(HostDispatchError::NotConnected);
            };
            state.next_call += 1;
            let call_id = format!("{host_id}:{}", state.next_call);
            let (sender, answer) = oneshot::channel();
            let sent = frames.send(HostFrame::Dispatch(HostCommand {
                id: call_id.clone(),
                host_id: host_id.to_string(),
                command: command.to_string(),
                timeout_ms: timeout.as_millis() as u64,
                approved_by_rule,
            }));
            if sent.is_err() {
                // The handler is gone and has not deregistered yet.
                state.connected.remove(host_id);
                return Err(HostDispatchError::NotConnected);
            }
            state.waiting.insert(
                call_id.clone(),
                Waiting {
                    host_id: host_id.clone(),
                    epoch,
                    answer: sender,
                },
            );
            (call_id, answer)
        };
        match tokio::time::timeout(timeout, answer).await {
            Ok(Ok(outcome)) => Ok(outcome),
            // The connection closed under the command, or the deadline
            // passed. Either way nothing is coming, and the waiting slot
            // goes with it.
            Ok(Err(_)) => {
                self.forget(&call_id);
                Err(HostDispatchError::NotConnected)
            }
            Err(_) => {
                self.forget(&call_id);
                Err(HostDispatchError::Timeout(timeout))
            }
        }
    }

    /// Hand the answer that came in on `from` to the call that waits for
    /// it. A call completes only from the connection that received its
    /// command: the same machine and the same registration. A call id is
    /// not a secret, so an answer from another machine, or from an
    /// earlier registration of the same machine, is dropped, and the call
    /// keeps waiting for its own connection and its deadline.
    ///
    /// An answer for a call nobody waits for is dropped too: the caller
    /// already gave up, and a client cannot make the daemon hold state by
    /// answering twice.
    pub fn complete(&self, from: &HostConnection, call_id: &str, outcome: HostOutcome) {
        let waiting = {
            let mut state = self.state.lock().expect("host presence lock");
            take_received(&mut state.waiting, call_id, from)
        };
        if let Some(waiting) = waiting {
            let _ = waiting.answer.send(outcome);
        }
    }

    /// Send one Harness Sign-In to a machine. The answer comes on the
    /// receiver when the Client App reports how the sign-in ended.
    ///
    /// A machine that is not connected fails at once. The receiver fails
    /// when the connection that received the sign-in closes first, since
    /// no other connection can answer it.
    pub fn sign_in(
        &self,
        host_id: &HostId,
        sign_in: HarnessSignIn,
    ) -> Result<oneshot::Receiver<SignInOutcome>, HostDispatchError> {
        let mut state = self.state.lock().expect("host presence lock");
        let Some((frames, epoch)) = state
            .connected
            .get(host_id)
            .map(|connected| (connected.frames.clone(), connected.epoch))
        else {
            return Err(HostDispatchError::NotConnected);
        };
        // A caller that stopped waiting leaves its entry until the
        // connection answers or closes. It goes here, so the map holds
        // only the sign-ins that somebody still waits for.
        state
            .signing_in
            .retain(|_, waiting| !waiting.answer.is_closed());
        let id = sign_in.id.clone();
        if frames.send(HostFrame::HarnessSignIn(sign_in)).is_err() {
            // The handler is gone and has not deregistered yet.
            state.connected.remove(host_id);
            return Err(HostDispatchError::NotConnected);
        }
        let (sender, answer) = oneshot::channel();
        state.signing_in.insert(
            id,
            Waiting {
                host_id: host_id.clone(),
                epoch,
                answer: sender,
            },
        );
        Ok(answer)
    }

    /// Ask a machine to check the sign-in state of one harness. The Client
    /// App reports the state on its own frame, so nothing waits here. A
    /// machine that is not connected fails at once.
    pub fn check_sign_in(&self, host_id: &HostId, harness: &str) -> Result<(), HostDispatchError> {
        let mut state = self.state.lock().expect("host presence lock");
        let Some(frames) = state
            .connected
            .get(host_id)
            .map(|connected| connected.frames.clone())
        else {
            return Err(HostDispatchError::NotConnected);
        };
        let frame = HostFrame::SignInCheck {
            harness: harness.to_string(),
        };
        if frames.send(frame).is_err() {
            // The handler is gone and has not deregistered yet.
            state.connected.remove(host_id);
            return Err(HostDispatchError::NotConnected);
        }
        Ok(())
    }

    /// Hand the result of a Harness Sign-In that came in on `from` to the
    /// caller that waits for it, with the rule of [`Self::complete`]:
    /// only the connection that received the sign-in answers it, and a
    /// result that nobody waits for is dropped.
    pub fn complete_sign_in(
        &self,
        from: &HostConnection,
        id: &HarnessSignInId,
        outcome: SignInOutcome,
    ) {
        let waiting = {
            let mut state = self.state.lock().expect("host presence lock");
            take_received(&mut state.signing_in, id, from)
        };
        if let Some(waiting) = waiting {
            let _ = waiting.answer.send(outcome);
        }
    }

    fn forget(&self, call_id: &str) {
        self.state
            .lock()
            .expect("host presence lock")
            .waiting
            .remove(call_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(200);

    /// The next frame of a connection, which a test expects to be a
    /// command.
    async fn next_command(connection: &mut HostConnection) -> HostCommand {
        match connection.next().await.expect("the frame arrives") {
            HostFrame::Dispatch(command) => command,
            other => panic!("another frame arrived: {other:?}"),
        }
    }

    fn outcome() -> HostOutcome {
        HostOutcome {
            exit_code: Some(0),
            stdout: "ok\n".to_string(),
            stderr: String::new(),
        }
    }

    #[tokio::test]
    async fn an_absent_host_answers_at_once_and_does_not_wait() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();

        let started = std::time::Instant::now();
        let error = presence
            .run(&host, "echo hi", false, Duration::from_secs(30))
            .await
            .expect_err("an absent host does not run a command");

        assert_eq!(error, HostDispatchError::NotConnected);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!presence.present(&host));
    }

    #[tokio::test]
    async fn a_present_host_receives_the_command_and_its_answer_comes_back() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let mut connection = presence.connect(&host);
        assert!(presence.present(&host));

        let dispatching = {
            let presence = Arc::clone(&presence);
            let host = host.clone();
            tokio::spawn(async move {
                presence
                    .run(&host, "echo hi", true, Duration::from_secs(5))
                    .await
            })
        };
        let command = next_command(&mut connection).await;
        assert_eq!(command.command, "echo hi");
        assert_eq!(command.host_id, host.to_string());
        // The client reads which shell to run it in from the command.
        assert!(command.approved_by_rule);
        presence.complete(&connection, &command.id, outcome());

        assert_eq!(dispatching.await.unwrap(), Ok(outcome()));
    }

    /// A machine that took the command and said nothing fails at the
    /// deadline. The person waits that long and no longer.
    #[tokio::test]
    async fn a_silent_host_times_out() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let mut connection = presence.connect(&host);

        let dispatching = {
            let presence = Arc::clone(&presence);
            let host = host.clone();
            tokio::spawn(async move { presence.run(&host, "sleep 100", false, SHORT).await })
        };
        next_command(&mut connection).await;

        assert_eq!(
            dispatching.await.unwrap(),
            Err(HostDispatchError::Timeout(SHORT))
        );
    }

    #[tokio::test]
    async fn a_closed_connection_makes_the_host_absent() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let connection = presence.connect(&host);
        assert!(presence.present(&host));

        drop(connection);

        assert!(!presence.present(&host));
        assert_eq!(
            presence.run(&host, "echo hi", false, SHORT).await,
            Err(HostDispatchError::NotConnected)
        );
    }

    /// The departure of a machine is news: a Coding Session on it is
    /// interrupted. A connection that a newer one replaced is no
    /// departure, because the machine is still here.
    #[test]
    fn a_dropped_connection_sends_its_host_once_and_a_replaced_one_sends_nothing() {
        let presence = Arc::new(HostPresence::new());
        let mut departures = presence.departures();
        let host = HostId::generate();

        let first = presence.connect(&host);
        let second = presence.connect(&host);
        drop(first);
        assert_eq!(
            departures.try_recv(),
            Err(broadcast::error::TryRecvError::Empty),
            "the replaced connection sends nothing"
        );

        drop(second);
        assert_eq!(departures.try_recv(), Ok(host));
        assert_eq!(
            departures.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        );
    }

    /// The command in flight when the connection closes answers that the
    /// machine is gone, rather than waiting out the whole deadline.
    #[tokio::test]
    async fn a_connection_that_closes_under_a_command_answers_absent() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let mut connection = presence.connect(&host);

        let dispatching = {
            let presence = Arc::clone(&presence);
            let host = host.clone();
            tokio::spawn(async move {
                presence
                    .run(&host, "sleep 100", false, Duration::from_secs(30))
                    .await
            })
        };
        next_command(&mut connection).await;
        drop(connection);

        assert_eq!(
            dispatching.await.unwrap(),
            Err(HostDispatchError::NotConnected)
        );
    }

    /// The client reconnects before the daemon noticed the old socket.
    /// The machine stays present, and the connection that went away does
    /// not take the new one with it.
    #[tokio::test]
    async fn a_second_connection_replaces_the_first_and_the_host_stays_present() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let first = presence.connect(&host);
        let mut second = presence.connect(&host);

        drop(first);

        assert!(presence.present(&host));
        let dispatching = {
            let presence = Arc::clone(&presence);
            let host = host.clone();
            tokio::spawn(async move {
                presence
                    .run(&host, "echo hi", false, Duration::from_secs(5))
                    .await
            })
        };
        let command = next_command(&mut second).await;
        presence.complete(&second, &command.id, outcome());
        assert!(dispatching.await.unwrap().is_ok());
    }

    /// An answer nobody waits for is dropped. A client that answers
    /// twice, or answers a call that already timed out, changes nothing.
    #[tokio::test]
    async fn an_answer_for_no_call_is_dropped() {
        let presence = Arc::new(HostPresence::new());
        let connection = presence.connect(&HostId::generate());

        presence.complete(&connection, "nobody-waits", outcome());
    }

    fn forged() -> HostOutcome {
        HostOutcome {
            exit_code: Some(0),
            stdout: "forged\n".to_string(),
            stderr: String::new(),
        }
    }

    /// A host id is not a secret. Another machine, of the same person or
    /// of another person, that names the exact call id does not answer
    /// the command. The host action keeps waiting, and the connection
    /// that received the command still answers it.
    #[tokio::test]
    async fn a_result_from_another_host_does_not_answer_the_command() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let mut connection = presence.connect(&host);
        let intruder = presence.connect(&HostId::generate());

        let dispatching = {
            let presence = Arc::clone(&presence);
            let host = host.clone();
            tokio::spawn(async move {
                presence
                    .run(&host, "echo hi", false, Duration::from_secs(5))
                    .await
            })
        };
        let command = next_command(&mut connection).await;
        presence.complete(&intruder, &command.id, forged());
        presence.complete(&connection, &command.id, outcome());

        assert_eq!(dispatching.await.unwrap(), Ok(outcome()));
    }

    /// The machine connects again, and the new connection receives the
    /// command. The old connection names the same machine, but it is an
    /// earlier registration, so its result does not answer the command.
    #[tokio::test]
    async fn a_result_from_a_replaced_connection_does_not_answer_the_command() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let first = presence.connect(&host);
        let mut second = presence.connect(&host);

        let dispatching = {
            let presence = Arc::clone(&presence);
            let host = host.clone();
            tokio::spawn(async move {
                presence
                    .run(&host, "echo hi", false, Duration::from_secs(5))
                    .await
            })
        };
        let command = next_command(&mut second).await;
        presence.complete(&first, &command.id, forged());
        presence.complete(&second, &command.id, outcome());

        assert_eq!(dispatching.await.unwrap(), Ok(outcome()));
    }

    /// The first result of the connection that received the command is
    /// the answer. A second result for the same call changes nothing,
    /// and the daemon holds no state for it.
    #[tokio::test]
    async fn a_second_result_for_an_answered_command_has_no_effect() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let mut connection = presence.connect(&host);

        let dispatching = {
            let presence = Arc::clone(&presence);
            let host = host.clone();
            tokio::spawn(async move {
                presence
                    .run(&host, "echo hi", false, Duration::from_secs(5))
                    .await
            })
        };
        let command = next_command(&mut connection).await;
        presence.complete(&connection, &command.id, outcome());
        presence.complete(&connection, &command.id, forged());

        assert_eq!(dispatching.await.unwrap(), Ok(outcome()));
        assert!(
            presence.state.lock().unwrap().waiting.is_empty(),
            "the daemon holds a call that is already answered"
        );
    }

    /// Only the connection that received a command can answer it. When
    /// that connection closes after a new one replaced it, nothing can
    /// answer the command, so the host action answers absent at once and
    /// does not wait out the deadline.
    #[tokio::test]
    async fn a_replaced_connection_that_closes_under_a_command_answers_absent() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let mut first = presence.connect(&host);

        let dispatching = {
            let presence = Arc::clone(&presence);
            let host = host.clone();
            tokio::spawn(async move {
                presence
                    .run(&host, "sleep 100", false, Duration::from_secs(30))
                    .await
            })
        };
        next_command(&mut first).await;
        let _second = presence.connect(&host);
        drop(first);

        let answered = tokio::time::timeout(Duration::from_secs(5), dispatching)
            .await
            .expect("the host action answers before its deadline");
        assert_eq!(answered.unwrap(), Err(HostDispatchError::NotConnected));
        assert!(presence.present(&host), "the new connection stays present");
    }

    fn sign_in() -> HarnessSignIn {
        HarnessSignIn {
            id: HarnessSignInId::generate(),
            harness: "codex".to_string(),
            name: "Codex".to_string(),
            command: "npx".to_string(),
            args: vec![
                "--yes".to_string(),
                "@openai/codex".to_string(),
                "login".to_string(),
            ],
            env: BTreeMap::from([("CODEX_HOME".to_string(), "/home/bo/.codex".to_string())]),
        }
    }

    fn signed_in() -> SignInOutcome {
        SignInOutcome {
            exit_code: Some(0),
            error: None,
        }
    }

    /// The next frame of a connection, which a test expects to be a
    /// Harness Sign-In.
    async fn next_sign_in(connection: &mut HostConnection) -> HarnessSignIn {
        match connection.next().await.expect("the frame arrives") {
            HostFrame::HarnessSignIn(sign_in) => sign_in,
            other => panic!("another frame arrived: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_sign_in_check_reaches_the_registered_connection() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let mut connection = presence.connect(&host);

        presence
            .check_sign_in(&host, "codex")
            .expect("a present host takes the check");

        assert_eq!(
            connection.next().await,
            Some(HostFrame::SignInCheck {
                harness: "codex".to_string()
            })
        );
    }

    #[tokio::test]
    async fn a_sign_in_check_on_an_absent_host_fails_at_once() {
        let presence = Arc::new(HostPresence::new());

        assert_eq!(
            presence.check_sign_in(&HostId::generate(), "codex"),
            Err(HostDispatchError::NotConnected)
        );
    }

    #[tokio::test]
    async fn a_sign_in_reaches_the_registered_connection_and_its_result_comes_back() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let mut connection = presence.connect(&host);

        let answer = presence
            .sign_in(&host, sign_in())
            .expect("a present host takes the sign-in");
        let received = next_sign_in(&mut connection).await;
        assert_eq!(received.command, "npx");
        assert_eq!(received.env, sign_in().env);
        presence.complete_sign_in(&connection, &received.id, signed_in());

        assert_eq!(answer.await, Ok(signed_in()));
    }

    #[tokio::test]
    async fn a_sign_in_on_an_absent_host_fails_at_once() {
        let presence = Arc::new(HostPresence::new());

        assert_eq!(
            presence.sign_in(&HostId::generate(), sign_in()).err(),
            Some(HostDispatchError::NotConnected)
        );
    }

    /// A sign-in id is not a secret. A result from another machine, or
    /// from a connection that a new one replaced, does not answer the
    /// sign-in, and the connection that received it still answers it.
    #[tokio::test]
    async fn a_sign_in_result_from_another_connection_is_dropped() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let replaced = presence.connect(&host);
        let mut connection = presence.connect(&host);
        let intruder = presence.connect(&HostId::generate());

        let mut answer = presence.sign_in(&host, sign_in()).unwrap();
        let received = next_sign_in(&mut connection).await;
        let forged = SignInOutcome {
            exit_code: Some(1),
            error: Some("forged".to_string()),
        };
        presence.complete_sign_in(&intruder, &received.id, forged.clone());
        presence.complete_sign_in(&replaced, &received.id, forged);
        assert!(
            answer.try_recv().is_err(),
            "a result from another connection answered the sign-in"
        );

        presence.complete_sign_in(&connection, &received.id, signed_in());
        assert_eq!(answer.await, Ok(signed_in()));
    }

    /// The connection that received the sign-in is the only one that can
    /// answer it. When it closes, the result is lost, and the waiting
    /// side hears so at once.
    #[tokio::test]
    async fn a_connection_that_closes_under_a_sign_in_ends_its_wait() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let mut connection = presence.connect(&host);

        let answer = presence.sign_in(&host, sign_in()).unwrap();
        next_sign_in(&mut connection).await;
        drop(connection);

        assert!(answer.await.is_err());
        assert!(presence.state.lock().unwrap().signing_in.is_empty());
    }

    /// A command and a sign-in share the channel of the connection, in
    /// the order the daemon sent them.
    #[tokio::test]
    async fn a_command_and_a_sign_in_arrive_in_order_on_one_connection() {
        let presence = Arc::new(HostPresence::new());
        let host = HostId::generate();
        let mut connection = presence.connect(&host);

        let dispatching = {
            let presence = Arc::clone(&presence);
            let host = host.clone();
            tokio::spawn(async move {
                presence
                    .run(&host, "echo hi", false, Duration::from_secs(5))
                    .await
            })
        };
        let HostFrame::Dispatch(command) = connection.next().await.unwrap() else {
            panic!("the command arrives first");
        };
        let _answer = presence.sign_in(&host, sign_in()).unwrap();
        next_sign_in(&mut connection).await;
        presence.complete(&connection, &command.id, outcome());

        assert_eq!(dispatching.await.unwrap(), Ok(outcome()));
    }
}
