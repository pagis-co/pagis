//! A Pagis client that acts as a Host of the test daemon.
//!
//! It speaks the real protocol: it opens the authenticated WebSocket,
//! registers as a Host, and answers every command the daemon dispatches
//! and every Harness Sign-In the daemon sends.
//! A test that exercises a host action holds one of these, because a host
//! action runs on a present client and never in the daemon.
//!
//! The answer is scripted, so a test never depends on what a shell of the
//! machine running the test would print. The Client App's own
//! executor has its own tests.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_broker::HarnessSignIn;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::TestDaemon;

/// What the client answers a dispatched command with.
#[derive(Debug, Clone)]
pub enum HostAnswer {
    /// Answer every command with this outcome.
    Exit {
        code: i64,
        stdout: String,
        stderr: String,
    },
    /// Answer with the command itself on standard output, so a test can
    /// tell which command reached the machine.
    Echo,
    /// Answer no command and no sign-in. The test answers each command
    /// with [`HostClient::answer`], so it decides when the answer reaches
    /// the daemon.
    Manual,
}

impl HostAnswer {
    /// The plain success a test uses when it cares about the flow and not
    /// about the output.
    pub fn ok() -> Self {
        HostAnswer::Exit {
            code: 0,
            stdout: "ok\n".to_string(),
            stderr: String::new(),
        }
    }
}

/// One command the daemon dispatched, as the client read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostDispatch {
    /// The call id the daemon made for the command. A result names it.
    pub id: String,
    pub command: String,
    /// Whether the dispatch said that an Allow Rule approved the command.
    /// The Client App then runs it under `/bin/sh` and not in the
    /// person's own shell.
    pub approved_by_rule: bool,
}

/// A frame the test sends, and the signal that the daemon read it.
type Outgoing = (serde_json::Value, oneshot::Sender<()>);

/// One connected Host of the test daemon. Dropping it closes the socket,
/// which is how a machine becomes absent.
pub struct HostClient {
    host_id: String,
    dispatched: Arc<Mutex<Vec<HostDispatch>>>,
    sign_ins: Arc<Mutex<Vec<HarnessSignIn>>>,
    results: mpsc::UnboundedSender<Outgoing>,
    task: tokio::task::JoinHandle<()>,
}

impl HostClient {
    /// Connect as the daemon's own person, register the machine, and
    /// answer every dispatched command. It returns once the daemon has
    /// acknowledged the registration, so the machine is present when this
    /// returns.
    pub async fn connect(
        daemon: &TestDaemon,
        name: &str,
        platform: &str,
        capabilities: &[&str],
        answer: HostAnswer,
    ) -> Self {
        Self::connect_as(
            daemon,
            daemon.cookie(),
            name,
            platform,
            capabilities,
            answer,
        )
        .await
    }

    /// The same, for another person's Session, so a test can hold the
    /// machines of two people at once.
    pub async fn connect_as(
        daemon: &TestDaemon,
        cookie: &str,
        name: &str,
        platform: &str,
        capabilities: &[&str],
        answer: HostAnswer,
    ) -> Self {
        let (mut socket, _) = connect_async(daemon.ws_request_as(&daemon.ws_url(), cookie))
            .await
            .expect("the host client connects");
        let send = async |socket: &mut _, frame: serde_json::Value| {
            SinkExt::send(socket, Message::text(frame.to_string()))
                .await
                .expect("the host client sends a frame");
        };
        send(&mut socket, serde_json::json!({"type": "auth"})).await;
        send(
            &mut socket,
            serde_json::json!({
                "type": "register_host",
                "name": name,
                "platform": platform,
                "capabilities": capabilities,
            }),
        )
        .await;
        let host_id = loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .expect("a frame before the timeout")
                .expect("the socket stays open")
                .expect("a readable frame");
            let Message::Text(text) = frame else { continue };
            let frame: serde_json::Value = serde_json::from_str(&text).expect("the frame is JSON");
            if frame["type"] == "host.registered" {
                break frame["payload"]["host_id"]
                    .as_str()
                    .expect("the acknowledgement names the host")
                    .to_string();
            }
            assert_ne!(frame["type"], "error", "the registration failed: {frame}");
        };
        let dispatched = Arc::new(Mutex::new(Vec::new()));
        let sign_ins = Arc::new(Mutex::new(Vec::new()));
        let (results, mut outgoing) = mpsc::unbounded_channel::<Outgoing>();
        let task = {
            let dispatched = Arc::clone(&dispatched);
            let sign_ins = Arc::clone(&sign_ins);
            tokio::spawn(async move {
                // The results the test sent, oldest first. Each one waits
                // for the `pong` of the ping that follows it.
                let mut unread: VecDeque<oneshot::Sender<()>> = VecDeque::new();
                loop {
                    tokio::select! {
                        frame = socket.next() => {
                            let Some(Ok(frame)) = frame else { return };
                            let Message::Text(text) = frame else { continue };
                            let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text) else {
                                continue;
                            };
                            if frame["type"] == "pong" {
                                if let Some(read) = unread.pop_front() {
                                    let _ = read.send(());
                                }
                                continue;
                            }
                            if frame["type"] == "harness_sign_in" {
                                let sign_in: HarnessSignIn =
                                    serde_json::from_value(frame["payload"].clone())
                                        .expect("a harness_sign_in frame holds a sign-in");
                                let reply = serde_json::json!({
                                    "type": "harness_sign_in_result",
                                    "id": sign_in.id,
                                    "exit_code": 0,
                                });
                                sign_ins.lock().expect("sign-ins").push(sign_in);
                                if matches!(answer, HostAnswer::Manual) {
                                    continue;
                                }
                                if SinkExt::send(&mut socket, Message::text(reply.to_string()))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                                continue;
                            }
                            if frame["type"] != "dispatch" {
                                continue;
                            }
                            let dispatch = HostDispatch {
                                id: frame["payload"]["id"].as_str().unwrap_or_default().to_string(),
                                command: frame["payload"]["command"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                                approved_by_rule: frame["payload"]["approved_by_rule"] == true,
                            };
                            dispatched.lock().expect("dispatched").push(dispatch.clone());
                            let reply = match &answer {
                                HostAnswer::Exit {
                                    code,
                                    stdout,
                                    stderr,
                                } => result_frame(&dispatch.id, *code, stdout, stderr),
                                HostAnswer::Echo => result_frame(
                                    &dispatch.id,
                                    0,
                                    &format!("{}\n", dispatch.command),
                                    "",
                                ),
                                HostAnswer::Manual => continue,
                            };
                            if SinkExt::send(&mut socket, Message::text(reply.to_string()))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Some((result, read)) = outgoing.recv() => {
                            // The daemon reads the frames of one socket in
                            // order, so the `pong` of this ping says that it
                            // has read the result.
                            let ping = serde_json::json!({"type": "ping"});
                            for frame in [result, ping] {
                                if SinkExt::send(&mut socket, Message::text(frame.to_string()))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            unread.push_back(read);
                        }
                    }
                }
            })
        };
        Self {
            host_id,
            dispatched,
            sign_ins,
            results,
            task,
        }
    }

    /// The id the daemon knows this machine by.
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    /// The commands the daemon dispatched to this machine, in order.
    pub fn commands(&self) -> Vec<String> {
        self.dispatched()
            .into_iter()
            .map(|dispatch| dispatch.command)
            .collect()
    }

    /// The dispatches this machine received, in order.
    pub fn dispatched(&self) -> Vec<HostDispatch> {
        self.dispatched.lock().expect("dispatched").clone()
    }

    /// The Harness Sign-Ins the daemon sent to this machine, in order.
    /// The client answers each one with exit code 0, as the Client App
    /// does when the vendor's program ends well, unless it answers
    /// [`HostAnswer::Manual`].
    pub fn sign_ins(&self) -> Vec<HarnessSignIn> {
        self.sign_ins.lock().expect("sign-ins").clone()
    }

    /// Send a result with exit code 0 and `stdout` for the call `id` on
    /// this socket, and return once the daemon has read it. The id can
    /// name a command that this machine did not receive, so a test can
    /// send a result from the wrong connection.
    pub async fn answer(&self, id: &str, stdout: &str) {
        self.send(result_frame(id, 0, stdout, "")).await;
    }

    /// Send the `session_exit` of one Coding Session on this socket, as
    /// the Client App does when the process of the session exits, and
    /// return once the daemon has read it.
    pub async fn session_exit(&self, session_id: &str, exit_code: Option<i64>, stderr_tail: &str) {
        self.send(serde_json::json!({
            "type": "session_exit",
            "session_id": session_id,
            "exit_code": exit_code,
            "stderr_tail": stderr_tail,
        }))
        .await;
    }

    /// Send one frame and return once the daemon has read it.
    async fn send(&self, frame: serde_json::Value) {
        let (read, was_read) = oneshot::channel();
        self.results
            .send((frame, read))
            .expect("the host client is connected");
        tokio::time::timeout(Duration::from_secs(5), was_read)
            .await
            .expect("the daemon reads the frame before the timeout")
            .expect("the socket stays open");
    }

    /// Close the socket, which makes the machine absent.
    pub fn disconnect(self) {
        drop(self);
    }
}

impl Drop for HostClient {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The `result` frame of one call, as the Client App sends it.
fn result_frame(id: &str, code: i64, stdout: &str, stderr: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "result",
        "id": id,
        "exit_code": code,
        "stdout": stdout,
        "stderr": stderr,
    })
}
