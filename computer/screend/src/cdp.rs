//! A Chrome DevTools Protocol client over a pipe: the transport of
//! `--remote-debugging-pipe`, which Playwright also uses. Each message is
//! one JSON object and one NUL byte, in both directions. A command
//! carries an `id`, and its answer carries the same `id`. An event
//! carries a `method`, and an event of one attached target carries the
//! `sessionId` of that attachment.
//!
//! No crate speaks this transport in a blocking program: the Rust CDP
//! crates are asynchronous and connect over a WebSocket to a debugging
//! port, and the browser here has no port. The client is one reader
//! thread and a map of callers that wait for an answer.
//!
//! A message can carry a secret, so the client never logs one.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::Value;

/// One message from the browser, with its place in the order the
/// messages arrived.
#[derive(Debug, Clone)]
pub struct Arrival {
    pub order: u64,
    /// The `result` of an answer, or the whole message of an event.
    pub message: Value,
}

/// The answer to one command: the result, or the browser's error.
type Answer = Result<Arrival, String>;

pub struct Cdp {
    writer: Mutex<Box<dyn Write + Send>>,
    next_id: AtomicU64,
    shared: Arc<Shared>,
}

/// What the reader thread and the callers share.
#[derive(Default)]
struct Shared {
    /// The callers that wait for an answer, by command id.
    waiting: Mutex<HashMap<u64, mpsc::Sender<Answer>>>,
    /// The event queue of each attached session.
    sessions: Mutex<HashMap<String, mpsc::Sender<Arrival>>>,
    /// Set when the browser closed its end of the pipe.
    closed: AtomicBool,
    /// How many messages arrived so far.
    arrivals: AtomicU64,
}

impl Cdp {
    /// A client over one pipe. `reader` is the end the browser writes
    /// to, and `writer` is the end the browser reads from.
    pub fn new(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
    ) -> Arc<Self> {
        let shared = Arc::new(Shared::default());
        let reading = Arc::clone(&shared);
        std::thread::spawn(move || read(reader, &reading));
        Arc::new(Self {
            writer: Mutex::new(Box::new(writer)),
            next_id: AtomicU64::new(1),
            shared,
        })
    }

    /// Send one command, to a session or to the browser itself, and
    /// wait at most `timeout` for its answer.
    pub fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Arrival, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (answer, answered) = mpsc::channel();
        self.shared
            .waiting
            .lock()
            .expect("waiting lock")
            .insert(id, answer);
        // The reader sets `closed` before it drops the waiting callers,
        // so a caller that registers after that sees the flag here.
        if self.shared.closed.load(Ordering::SeqCst) {
            self.forget(id);
            return Err(CLOSED.to_string());
        }
        let mut command = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            command["sessionId"] = Value::from(session);
        }
        let mut bytes = serde_json::to_vec(&command).expect("a command is JSON");
        bytes.push(0);
        let written = {
            let mut writer = self.writer.lock().expect("writer lock");
            writer.write_all(&bytes).and_then(|()| writer.flush())
        };
        if written.is_err() {
            self.forget(id);
            return Err(CLOSED.to_string());
        }
        match answered.recv_timeout(timeout) {
            Ok(answer) => answer,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.forget(id);
                Err(format!(
                    "{method} got no answer in {} ms",
                    timeout.as_millis()
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(CLOSED.to_string()),
        }
    }

    /// The events of one session, from now on, in the order they
    /// arrive.
    pub fn subscribe(&self, session: &str) -> mpsc::Receiver<Arrival> {
        let (events, receiver) = mpsc::channel();
        self.shared
            .sessions
            .lock()
            .expect("sessions lock")
            .insert(session.to_string(), events);
        receiver
    }

    /// Stop the events of one session.
    pub fn unsubscribe(&self, session: &str) {
        self.shared
            .sessions
            .lock()
            .expect("sessions lock")
            .remove(session);
    }

    fn forget(&self, id: u64) {
        self.shared
            .waiting
            .lock()
            .expect("waiting lock")
            .remove(&id);
    }
}

/// The reason every call fails once the browser's end is gone.
const CLOSED: &str = "the browser closed the DevTools pipe";

/// Read messages until the browser closes its end, and hand each one to
/// the caller that waits for it or to the queue of its session.
fn read(reader: impl Read, shared: &Shared) {
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(0, &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if buffer.pop() != Some(0) {
            // The browser ended in the middle of a message.
            break;
        }
        let Ok(message) = serde_json::from_slice::<Value>(&buffer) else {
            eprintln!("[screend] a DevTools message is not JSON");
            continue;
        };
        let order = shared.arrivals.fetch_add(1, Ordering::SeqCst);
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            let waiter = shared.waiting.lock().expect("waiting lock").remove(&id);
            if let Some(waiter) = waiter {
                let answer = match message.get("error") {
                    Some(error) => Err(error["message"]
                        .as_str()
                        .unwrap_or("the browser refused the command")
                        .to_string()),
                    None => Ok(Arrival {
                        order,
                        message: message.get("result").cloned().unwrap_or(Value::Null),
                    }),
                };
                let _ = waiter.send(answer);
            }
        } else if let Some(session) = message.get("sessionId").and_then(Value::as_str) {
            let sessions = shared.sessions.lock().expect("sessions lock");
            if let Some(events) = sessions.get(session) {
                let _ = events.send(Arrival { order, message });
            }
        }
    }
    shared.closed.store(true, Ordering::SeqCst);
    shared.waiting.lock().expect("waiting lock").clear();
    shared.sessions.lock().expect("sessions lock").clear();
}

/// A browser end of the pipe for tests.
#[cfg(test)]
pub mod fake {
    use super::*;

    /// Every command the fake browser received, in order.
    pub type Commands = Arc<Mutex<Vec<Value>>>;

    /// A client connected to a fake browser. The browser gives each
    /// command to `answer` and writes the messages it returns, in
    /// order. `None` closes the browser's end of the pipe.
    pub fn connect(
        mut answer: impl FnMut(&Value) -> Option<Vec<Value>> + Send + 'static,
    ) -> (Arc<Cdp>, Commands) {
        let (commands_reader, commands_writer) = std::io::pipe().expect("a pipe");
        let (events_reader, mut events_writer) = std::io::pipe().expect("a pipe");
        let commands: Commands = Arc::default();
        let recorded = Arc::clone(&commands);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(commands_reader);
            let mut buffer = Vec::new();
            while reader.read_until(0, &mut buffer).unwrap_or(0) > 0 {
                buffer.pop();
                let command: Value = serde_json::from_slice(&buffer).expect("a JSON command");
                buffer.clear();
                recorded.lock().unwrap().push(command.clone());
                let Some(messages) = answer(&command) else {
                    return;
                };
                for message in messages {
                    let mut bytes = serde_json::to_vec(&message).unwrap();
                    bytes.push(0);
                    if events_writer.write_all(&bytes).is_err() {
                        return;
                    }
                }
            }
        });
        (Cdp::new(events_reader, commands_writer), commands)
    }

    /// The answer to `command` with this result.
    pub fn reply(command: &Value, result: Value) -> Value {
        let mut message = serde_json::json!({ "id": command["id"], "result": result });
        if let Some(session) = command.get("sessionId") {
            message["sessionId"] = session.clone();
        }
        message
    }

    /// The error answer to `command`.
    pub fn refuse(command: &Value, reason: &str) -> Value {
        serde_json::json!({
            "id": command["id"],
            "error": { "code": -32000, "message": reason },
        })
    }

    /// One event of a session.
    pub fn event(session: &str, method: &str, params: Value) -> Value {
        serde_json::json!({ "method": method, "params": params, "sessionId": session })
    }

    /// The methods of the commands, in order.
    pub fn methods(commands: &Commands) -> Vec<String> {
        commands
            .lock()
            .unwrap()
            .iter()
            .map(|command| command["method"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn a_command_is_one_json_object_with_its_id_method_params_and_session() {
        let (cdp, commands) = connect(|command| Some(vec![reply(command, serde_json::json!({}))]));

        cdp.call(Some("S1"), "Page.enable", serde_json::json!({"a": 1}), WAIT)
            .unwrap();

        let sent = commands.lock().unwrap()[0].clone();
        assert_eq!(sent["method"], "Page.enable");
        assert_eq!(sent["params"], serde_json::json!({"a": 1}));
        assert_eq!(sent["sessionId"], "S1");
        assert!(sent["id"].is_u64(), "{sent}");
    }

    #[test]
    fn a_call_gets_the_result_of_its_own_answer() {
        let (cdp, _) = connect(|command| {
            Some(vec![
                event(
                    "S1",
                    "Page.lifecycleEvent",
                    serde_json::json!({"name": "load"}),
                ),
                reply(command, serde_json::json!({"targetId": "T1"})),
            ])
        });

        let answer = cdp
            .call(None, "Target.createTarget", serde_json::json!({}), WAIT)
            .unwrap();

        assert_eq!(answer.message, serde_json::json!({"targetId": "T1"}));
    }

    #[test]
    fn an_error_answer_fails_the_call_with_the_browsers_reason() {
        let (cdp, _) =
            connect(|command| Some(vec![refuse(command, "No target with given id found")]));

        let refused = cdp
            .call(None, "Target.getTargetInfo", serde_json::json!({}), WAIT)
            .unwrap_err();

        assert!(
            refused.contains("No target with given id found"),
            "{refused}"
        );
    }

    #[test]
    fn a_session_gets_its_own_events_in_order_before_and_after_an_answer() {
        let (cdp, _) = connect(|command| {
            Some(vec![
                event(
                    "S1",
                    "Page.lifecycleEvent",
                    serde_json::json!({"name": "commit"}),
                ),
                event(
                    "S2",
                    "Page.lifecycleEvent",
                    serde_json::json!({"name": "other"}),
                ),
                reply(command, serde_json::json!({})),
                event(
                    "S1",
                    "Page.lifecycleEvent",
                    serde_json::json!({"name": "load"}),
                ),
            ])
        });
        let events = cdp.subscribe("S1");

        let answer = cdp
            .call(Some("S1"), "Page.navigate", serde_json::json!({}), WAIT)
            .unwrap();

        let first = events.recv_timeout(WAIT).unwrap();
        let second = events.recv_timeout(WAIT).unwrap();
        assert_eq!(first.message["params"]["name"], "commit");
        assert_eq!(second.message["params"]["name"], "load");
        assert!(first.order < answer.order && answer.order < second.order);
        assert!(events.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn a_closed_pipe_fails_a_waiting_call_at_once() {
        let (cdp, _) = connect(|_| None);
        let started = std::time::Instant::now();

        let closed = cdp
            .call(
                None,
                "Browser.getVersion",
                serde_json::json!({}),
                Duration::from_secs(30),
            )
            .unwrap_err();

        assert!(closed.contains("closed"), "{closed}");
        assert!(started.elapsed() < Duration::from_secs(5));
        // And every later call fails at once too.
        assert!(
            cdp.call(
                None,
                "Browser.getVersion",
                serde_json::json!({}),
                Duration::from_secs(30)
            )
            .is_err()
        );
    }

    #[test]
    fn a_command_with_no_answer_fails_when_its_time_ends() {
        let (cdp, _) = connect(|_| Some(Vec::new()));

        let silent = cdp
            .call(
                None,
                "Page.navigate",
                serde_json::json!({}),
                Duration::from_millis(100),
            )
            .unwrap_err();

        assert!(silent.contains("Page.navigate"), "{silent}");
    }
}
