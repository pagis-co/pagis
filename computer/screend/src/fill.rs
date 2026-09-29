//! The daemon's tab and the verified write of a Vault fill (ADR-0013).
//!
//! The daemon drives this over the control endpoint, and only while it
//! holds the input switch. A password fill opens the Credential's login
//! address in the daemon's own tab and waits for the load event of that
//! page, then reads the top-level address the tab shows after every
//! redirect. The daemon checks that address against the Credential, and
//! then asks for the write with the origin it verified.
//!
//! The write runs `fill.js` in an isolated world of the top frame, in
//! one evaluation: the check of the origin and of the focused field and
//! the write of the value have no gap between them in which the focus
//! can move. The text goes into the field through the DevTools pipe and
//! never through the compositor, so the window that has the focus gets
//! nothing.

use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cdp::{Arrival, Cdp};

/// The function that checks and writes, in one evaluation.
const FILL_SCRIPT: &str = include_str!("fill.js");
/// The name of the isolated world the write runs in.
const WORLD: &str = "pagis-vault";
/// How long one command other than a navigation waits for its answer.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// How often a fill asks again while no field has the focus.
const FOCUS_POLL: Duration = Duration::from_millis(100);
/// The reason a request finds no tab of the daemon.
const NO_TAB: &str = "the daemon has no tab open: a password fill opens it";

/// The kind of field one value goes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Text,
    Password,
}

/// One value of a fill and the kind of field it goes into.
#[derive(Clone, Deserialize, Serialize)]
pub struct Field {
    pub kind: Kind,
    pub text: String,
}

/// The text is a secret or a code, so a debug print never shows it.
impl std::fmt::Debug for Field {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Field")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// The daemon's own tab of the browser. One request runs at a time.
pub struct Tab {
    /// The target id of the tab, once the daemon opened it.
    target: Mutex<Option<String>>,
    /// How long a page gets to load.
    load_timeout: Duration,
    /// How long a fill waits for the page to give the focus to a field.
    /// Chromium applies `autofocus` at the first rendering update after
    /// the load event, and a page can also build its form after it. When
    /// no field has the focus at the end of the wait, the fill finds the
    /// login fields of the page itself.
    focus_wait: Duration,
}

/// What one evaluation of the fill script answered.
enum Answer {
    Written,
    /// Nothing has the focus yet, and nothing was written.
    Wait(String),
    Stopped(String),
}

impl Tab {
    pub fn new(load_timeout: Duration, focus_wait: Duration) -> Self {
        Self {
            target: Mutex::new(None),
            load_timeout,
            focus_wait,
        }
    }

    /// Open `url` in the daemon's tab and wait for the load event of the
    /// page. The answer is the top-level address after every redirect.
    pub fn open(&self, cdp: &Cdp, url: &str) -> Result<String, String> {
        let mut target = self.target.lock().expect("tab lock");
        let open = target.as_deref().is_some_and(|id| {
            cdp.call(
                None,
                "Target.getTargetInfo",
                json!({ "targetId": id }),
                COMMAND_TIMEOUT,
            )
            .is_ok()
        });
        if !open {
            let created = cdp.call(
                None,
                "Target.createTarget",
                json!({ "url": "about:blank" }),
                COMMAND_TIMEOUT,
            )?;
            let id = created.message["targetId"]
                .as_str()
                .ok_or("the browser opened no tab")?;
            *target = Some(id.to_string());
        }
        let id = target.as_deref().ok_or(NO_TAB)?;
        cdp.call(
            None,
            "Target.activateTarget",
            json!({ "targetId": id }),
            COMMAND_TIMEOUT,
        )?;
        let session = Session::attach(cdp, id)?;
        session.call("Page.enable", json!({}))?;
        session.call("Page.setLifecycleEventsEnabled", json!({ "enabled": true }))?;
        // The page acts as a focused page whichever window the
        // compositor focused, so its autofocus runs as it does for a
        // person.
        session.call(
            "Emulation.setFocusEmulationEnabled",
            json!({ "enabled": true }),
        )?;
        let deadline = Instant::now() + self.load_timeout;
        let navigated = match cdp.call(
            Some(&session.id),
            "Page.navigate",
            json!({ "url": url }),
            self.load_timeout,
        ) {
            Ok(navigated) => navigated,
            Err(error) => return Err(self.not_loaded(&session, deadline, &error)),
        };
        if let Some(error) = navigated.message["errorText"].as_str() {
            return Err(format!("the page did not load: {error}"));
        }
        // A navigation inside the same document has no loader, and its
        // document has loaded already.
        if let Some(loader) = navigated.message["loaderId"].as_str() {
            let frame = navigated.message["frameId"].as_str().unwrap_or_default();
            if let Err(error) = wait_for_load(&session, frame, loader, navigated.order, deadline) {
                return Err(self.not_loaded(&session, deadline, &error));
            }
        }
        address(&session.call("Page.getFrameTree", json!({}))?)
    }

    /// The top-level address the daemon's tab shows now. Nothing
    /// navigates.
    pub fn page(&self, cdp: &Cdp) -> Result<String, String> {
        let target = self.target.lock().expect("tab lock");
        let id = target.as_deref().ok_or(NO_TAB)?;
        let session = Session::attach(cdp, id).map_err(|_| NO_TAB.to_string())?;
        address(&session.call("Page.getFrameTree", json!({}))?)
    }

    /// Write `fields` into the page of the daemon's tab while its
    /// top-level origin is `origin`.
    pub fn fill(&self, cdp: &Cdp, origin: &str, fields: &[Field]) -> Result<(), String> {
        if fields.is_empty() {
            return Err("a fill needs at least one value".to_string());
        }
        let target = self.target.lock().expect("tab lock");
        let id = target.as_deref().ok_or(NO_TAB)?;
        cdp.call(
            None,
            "Target.activateTarget",
            json!({ "targetId": id }),
            COMMAND_TIMEOUT,
        )
        .map_err(|_| NO_TAB.to_string())?;
        let session = Session::attach(cdp, id).map_err(|_| NO_TAB.to_string())?;
        session.call(
            "Emulation.setFocusEmulationEnabled",
            json!({ "enabled": true }),
        )?;
        let tree = session.call("Page.getFrameTree", json!({}))?;
        let frame = tree.message["frameTree"]["frame"]["id"]
            .as_str()
            .ok_or("the tab reports no top frame")?;
        let world = session.call(
            "Page.createIsolatedWorld",
            json!({ "frameId": frame, "worldName": WORLD }),
        )?;
        let context = world.message["executionContextId"]
            .as_i64()
            .ok_or("the browser made no isolated world")?;
        let deadline = Instant::now() + self.focus_wait;
        let mut pick = false;
        loop {
            let answer = session.call(
                "Runtime.callFunctionOn",
                json!({
                    "functionDeclaration": FILL_SCRIPT,
                    "executionContextId": context,
                    "arguments": [{ "value": origin }, { "value": fields }, { "value": pick }],
                    "returnByValue": true,
                }),
            )?;
            match answer_of(&answer) {
                Answer::Written => return Ok(()),
                Answer::Wait(_) if Instant::now() < deadline => std::thread::sleep(FOCUS_POLL),
                // No field took the focus in time, so the last evaluation
                // finds the login fields of the page, as a password
                // manager does.
                Answer::Wait(_) if !pick => pick = true,
                Answer::Wait(reason) | Answer::Stopped(reason) => return Err(reason),
            }
        }
    }

    /// Stop a navigation that did not load, so the tab does not go on to
    /// the page later, and say why it stopped.
    fn not_loaded(&self, session: &Session, deadline: Instant, error: &str) -> String {
        let _ = session.call("Page.stopLoading", json!({}));
        if Instant::now() >= deadline {
            format!(
                "the page did not load in {} seconds",
                self.load_timeout.as_secs_f64()
            )
        } else {
            format!("the page did not load: {error}")
        }
    }
}

/// Wait for the load event of the top frame's document. That is the
/// document of `loader`, or a later document of the top frame when the
/// page itself moved on to another address after the navigation.
fn wait_for_load(
    session: &Session,
    frame: &str,
    loader: &str,
    after: u64,
    deadline: Instant,
) -> Result<(), String> {
    let mut loader = loader.to_string();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let arrival = match session.events.recv_timeout(remaining) {
            Ok(arrival) => arrival,
            Err(mpsc::RecvTimeoutError::Timeout) => return Err("no load event".to_string()),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the browser closed the DevTools pipe".to_string());
            }
        };
        let event = &arrival.message;
        if event["method"] != "Page.lifecycleEvent" || event["params"]["frameId"] != frame {
            continue;
        }
        let document = event["params"]["loaderId"].as_str().unwrap_or_default();
        match event["params"]["name"].as_str() {
            Some("init") if arrival.order > after => loader = document.to_string(),
            Some("load") if document == loader => return Ok(()),
            _ => {}
        }
    }
}

/// Read one answer of the fill script.
fn answer_of(answer: &Arrival) -> Answer {
    if answer.message.get("exceptionDetails").is_some() {
        return Answer::Stopped("the check of the page stopped with an error".to_string());
    }
    let result = &answer.message["result"];
    match (result["type"].as_str(), result["subtype"].as_str()) {
        (Some("object"), Some("null")) => Answer::Written,
        (Some("string"), _) => {
            Answer::Stopped(result["value"].as_str().unwrap_or_default().to_string())
        }
        (Some("object"), _) => match result["value"]["wait"].as_str() {
            Some(reason) => Answer::Wait(reason.to_string()),
            None => Answer::Stopped("the check of the page gave no answer".to_string()),
        },
        _ => Answer::Stopped("the check of the page gave no answer".to_string()),
    }
}

/// The address of the top frame in a `Page.getFrameTree` answer.
fn address(tree: &Arrival) -> Result<String, String> {
    tree.message["frameTree"]["frame"]["url"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the tab reports no address".to_string())
}

/// One attachment to the daemon's tab, with the queue of its events. It
/// detaches when it drops, so no DevTools session stays on the page.
struct Session<'a> {
    cdp: &'a Cdp,
    id: String,
    events: mpsc::Receiver<Arrival>,
}

impl<'a> Session<'a> {
    fn attach(cdp: &'a Cdp, target: &str) -> Result<Self, String> {
        let attached = cdp.call(
            None,
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
            COMMAND_TIMEOUT,
        )?;
        let id = attached.message["sessionId"]
            .as_str()
            .ok_or("the browser gave no session")?
            .to_string();
        let events = cdp.subscribe(&id);
        Ok(Self { cdp, id, events })
    }

    fn call(&self, method: &str, params: Value) -> Result<Arrival, String> {
        self.cdp
            .call(Some(&self.id), method, params, COMMAND_TIMEOUT)
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        self.cdp.unsubscribe(&self.id);
        let _ = self.cdp.call(
            None,
            "Target.detachFromTarget",
            json!({ "sessionId": self.id }),
            COMMAND_TIMEOUT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdp::fake::{Commands, connect, event, methods, refuse, reply};

    const LOAD: Duration = Duration::from_millis(300);
    const FOCUS: Duration = Duration::from_millis(300);
    const PAGE: &str = "https://login.example.com/signin";

    /// What the fake browser does for one test.
    #[derive(Clone)]
    struct Script {
        /// The messages that answer `Page.navigate`, with the reply
        /// among them.
        navigate: fn(&Value) -> Vec<Value>,
        /// The results of the fill evaluations that wait for the focus
        /// of the page, in order. The last one answers every later one.
        evaluations: Vec<Value>,
        /// The result of the evaluation that picks the login fields.
        picked: Value,
        /// Whether `Target.getTargetInfo` finds a tab.
        tab_alive: bool,
    }

    impl Default for Script {
        fn default() -> Self {
            Self {
                navigate: loads,
                evaluations: vec![written()],
                picked: written(),
                tab_alive: true,
            }
        }
    }

    /// A navigation that commits `L1` and loads it.
    fn loads(command: &Value) -> Vec<Value> {
        vec![
            reply(command, json!({"frameId": "T1", "loaderId": "L1"})),
            lifecycle("init", "T1", "L1"),
            lifecycle("load", "T1", "L1"),
        ]
    }

    fn written() -> Value {
        json!({"result": {"type": "object", "subtype": "null", "value": null}})
    }

    fn waiting() -> Value {
        json!({"result": {"type": "object", "value": {"wait": "no text field has the focus on the page"}}})
    }

    fn lifecycle(name: &str, frame: &str, loader: &str) -> Value {
        event(
            "S1",
            "Page.lifecycleEvent",
            json!({"name": name, "frameId": frame, "loaderId": loader}),
        )
    }

    fn browser(script: Script) -> (std::sync::Arc<Cdp>, Commands) {
        let mut evaluations = script.evaluations.clone().into_iter().peekable();
        let mut last = Value::Null;
        connect(move |command| {
            let answer = match command["method"].as_str().unwrap_or_default() {
                "Target.createTarget" => reply(command, json!({"targetId": "T1"})),
                "Target.getTargetInfo" if !script.tab_alive => {
                    refuse(command, "No target with given id found")
                }
                "Target.getTargetInfo" => reply(
                    command,
                    json!({"targetInfo": {"targetId": "T1", "url": PAGE}}),
                ),
                "Target.attachToTarget" => reply(command, json!({"sessionId": "S1"})),
                "Page.navigate" => return Some((script.navigate)(command)),
                "Page.getFrameTree" => reply(
                    command,
                    json!({"frameTree": {"frame": {"id": "T1", "url": PAGE}}}),
                ),
                "Page.createIsolatedWorld" => reply(command, json!({"executionContextId": 7})),
                "Runtime.callFunctionOn" if command["params"]["arguments"][2]["value"] == true => {
                    reply(command, script.picked.clone())
                }
                "Runtime.callFunctionOn" => {
                    if let Some(next) = evaluations.next() {
                        last = next;
                    }
                    reply(command, last.clone())
                }
                _ => reply(command, json!({})),
            };
            Some(vec![answer])
        })
    }

    fn login() -> Vec<Field> {
        vec![
            Field {
                kind: Kind::Text,
                text: "alice".to_string(),
            },
            Field {
                kind: Kind::Password,
                text: "s3cret".to_string(),
            },
        ]
    }

    fn sent(commands: &Commands, method: &str) -> Vec<Value> {
        commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| command["method"] == method)
            .cloned()
            .collect()
    }

    #[test]
    fn an_open_answers_the_top_level_address_after_the_load_event() {
        let (cdp, commands) = browser(Script::default());
        let tab = Tab::new(LOAD, FOCUS);

        let page = tab.open(&cdp, "https://login.example.com/in").unwrap();

        assert_eq!(page, PAGE);
        let navigated = sent(&commands, "Page.navigate");
        assert_eq!(
            navigated[0]["params"]["url"],
            "https://login.example.com/in"
        );
        assert_eq!(navigated[0]["sessionId"], "S1");
        let order = methods(&commands);
        let position = |method: &str| order.iter().position(|sent| sent == method).unwrap();
        assert!(position("Target.activateTarget") < position("Page.navigate"));
        assert!(position("Page.setLifecycleEventsEnabled") < position("Page.navigate"));
        assert!(position("Page.navigate") < position("Page.getFrameTree"));
    }

    #[test]
    fn a_load_event_of_the_page_before_or_of_a_frame_does_not_end_the_wait() {
        let (cdp, commands) = browser(Script {
            navigate: |command| {
                vec![
                    lifecycle("load", "T1", "L0"),
                    reply(command, json!({"frameId": "T1", "loaderId": "L1"})),
                    lifecycle("init", "F2", "L2"),
                    lifecycle("load", "F2", "L2"),
                ]
            },
            ..Script::default()
        });
        let tab = Tab::new(LOAD, FOCUS);

        let failed = tab.open(&cdp, "https://login.example.com/in").unwrap_err();

        assert!(failed.contains("did not load"), "{failed}");
        assert!(sent(&commands, "Page.getFrameTree").is_empty());
        // The tab does not go on to a page that loads later.
        assert_eq!(sent(&commands, "Page.stopLoading").len(), 1);
    }

    #[test]
    fn an_open_follows_a_new_document_of_the_top_frame() {
        let (cdp, _) = browser(Script {
            navigate: |command| {
                vec![
                    reply(command, json!({"frameId": "T1", "loaderId": "L1"})),
                    lifecycle("init", "T1", "L1"),
                    lifecycle("init", "T1", "L2"),
                    lifecycle("load", "T1", "L2"),
                ]
            },
            ..Script::default()
        });
        let tab = Tab::new(LOAD, FOCUS);

        assert_eq!(
            tab.open(&cdp, "https://login.example.com/in").unwrap(),
            PAGE
        );
    }

    #[test]
    fn an_open_fails_when_the_navigation_fails() {
        let (cdp, commands) = browser(Script {
            navigate: |command| {
                vec![reply(
                    command,
                    json!({"frameId": "T1", "loaderId": "L1", "errorText": "net::ERR_NAME_NOT_RESOLVED"}),
                )]
            },
            ..Script::default()
        });
        let tab = Tab::new(LOAD, FOCUS);

        let failed = tab.open(&cdp, "https://login.example.com/in").unwrap_err();

        assert!(failed.contains("net::ERR_NAME_NOT_RESOLVED"), "{failed}");
        assert!(sent(&commands, "Page.getFrameTree").is_empty());
    }

    #[test]
    fn an_open_fails_when_the_navigation_never_commits() {
        let (cdp, commands) = browser(Script {
            navigate: |_| Vec::new(),
            ..Script::default()
        });
        let tab = Tab::new(LOAD, FOCUS);
        let started = Instant::now();

        let failed = tab
            .open(&cdp, "https://login.example.com:8443/")
            .unwrap_err();

        assert!(failed.contains("did not load"), "{failed}");
        assert!(started.elapsed() < LOAD + Duration::from_secs(2));
        assert_eq!(sent(&commands, "Page.stopLoading").len(), 1);
    }

    #[test]
    fn the_daemon_keeps_its_tab_while_the_tab_is_open() {
        let (cdp, commands) = browser(Script::default());
        let tab = Tab::new(LOAD, FOCUS);

        tab.open(&cdp, "https://login.example.com/a").unwrap();
        tab.open(&cdp, "https://login.example.com/b").unwrap();

        assert_eq!(sent(&commands, "Target.createTarget").len(), 1);
    }

    #[test]
    fn the_daemon_opens_a_new_tab_when_its_tab_is_closed() {
        let (cdp, commands) = browser(Script {
            tab_alive: false,
            ..Script::default()
        });
        let tab = Tab::new(LOAD, FOCUS);

        tab.open(&cdp, "https://login.example.com/a").unwrap();
        tab.open(&cdp, "https://login.example.com/b").unwrap();

        assert_eq!(sent(&commands, "Target.createTarget").len(), 2);
    }

    #[test]
    fn a_page_read_answers_the_address_of_the_daemons_tab_and_does_not_navigate() {
        let (cdp, commands) = browser(Script::default());
        let tab = Tab::new(LOAD, FOCUS);
        tab.open(&cdp, "https://login.example.com/a").unwrap();

        assert_eq!(tab.page(&cdp).unwrap(), PAGE);
        assert_eq!(sent(&commands, "Page.navigate").len(), 1);
    }

    #[test]
    fn a_page_read_and_a_fill_with_no_tab_fail_and_send_nothing() {
        let (cdp, commands) = browser(Script::default());
        let tab = Tab::new(LOAD, FOCUS);

        assert!(tab.page(&cdp).unwrap_err().contains("no tab"));
        assert!(
            tab.fill(&cdp, "https://login.example.com", &login())
                .unwrap_err()
                .contains("no tab")
        );
        assert!(commands.lock().unwrap().is_empty());
    }

    #[test]
    fn a_fill_checks_and_writes_in_one_evaluation_in_an_isolated_world_of_the_top_frame() {
        let (cdp, commands) = browser(Script::default());
        let tab = Tab::new(LOAD, FOCUS);
        tab.open(&cdp, "https://login.example.com/a").unwrap();

        tab.fill(&cdp, "https://login.example.com", &login())
            .unwrap();

        let worlds = sent(&commands, "Page.createIsolatedWorld");
        assert_eq!(worlds.len(), 1);
        assert_eq!(worlds[0]["params"]["frameId"], "T1");
        let evaluations = sent(&commands, "Runtime.callFunctionOn");
        assert_eq!(
            evaluations.len(),
            1,
            "the check and the write are one evaluation"
        );
        let params = &evaluations[0]["params"];
        assert_eq!(params["executionContextId"], 7);
        assert_eq!(params["functionDeclaration"], FILL_SCRIPT);
        assert_eq!(params["arguments"][0]["value"], "https://login.example.com");
        assert_eq!(
            params["arguments"][1]["value"],
            json!([{"kind": "text", "text": "alice"}, {"kind": "password", "text": "s3cret"}])
        );
        // The page gave the focus to its field, so the fill does not pick.
        assert_eq!(params["arguments"][2]["value"], false);
        // No text goes as input events: those go wherever the focus is.
        assert!(
            methods(&commands)
                .iter()
                .all(|method| !method.starts_with("Input.")),
            "{:?}",
            methods(&commands)
        );
    }

    #[test]
    fn a_fill_reports_the_reason_the_page_check_gives() {
        let (cdp, _) = browser(Script {
            evaluations: vec![
                json!({"result": {"type": "string", "value": "the password field did not keep the focus"}}),
            ],
            ..Script::default()
        });
        let tab = Tab::new(LOAD, FOCUS);
        tab.open(&cdp, "https://login.example.com/a").unwrap();

        let refused = tab
            .fill(&cdp, "https://login.example.com", &login())
            .unwrap_err();

        assert_eq!(refused, "the password field did not keep the focus");
    }

    #[test]
    fn a_fill_whose_evaluation_throws_or_answers_nothing_fails() {
        for evaluation in [
            json!({"result": {"type": "object"}, "exceptionDetails": {"text": "Uncaught"}}),
            json!({"result": {"type": "undefined"}}),
        ] {
            let (cdp, _) = browser(Script {
                evaluations: vec![evaluation],
                ..Script::default()
            });
            let tab = Tab::new(LOAD, FOCUS);
            tab.open(&cdp, "https://login.example.com/a").unwrap();

            assert!(
                tab.fill(&cdp, "https://login.example.com", &login())
                    .is_err()
            );
        }
    }

    #[test]
    fn a_fill_asks_again_while_no_field_has_the_focus() {
        let (cdp, commands) = browser(Script {
            evaluations: vec![waiting(), waiting(), written()],
            ..Script::default()
        });
        let tab = Tab::new(LOAD, Duration::from_secs(5));
        tab.open(&cdp, "https://login.example.com/a").unwrap();

        tab.fill(&cdp, "https://login.example.com", &login())
            .unwrap();

        let evaluations = sent(&commands, "Runtime.callFunctionOn");
        assert_eq!(evaluations.len(), 3);
        assert!(
            evaluations
                .iter()
                .all(|evaluation| evaluation["params"]["arguments"][2]["value"] == false)
        );
    }

    #[test]
    fn a_fill_picks_the_login_fields_when_no_field_takes_the_focus_in_time() {
        let (cdp, commands) = browser(Script {
            evaluations: vec![waiting()],
            ..Script::default()
        });
        let tab = Tab::new(LOAD, FOCUS);
        tab.open(&cdp, "https://login.example.com/a").unwrap();
        let started = Instant::now();

        tab.fill(&cdp, "https://login.example.com", &login())
            .unwrap();

        assert!(started.elapsed() >= FOCUS);
        assert!(started.elapsed() < FOCUS + Duration::from_secs(2));
        let evaluations = sent(&commands, "Runtime.callFunctionOn");
        let (last, waits) = evaluations.split_last().unwrap();
        assert!(!waits.is_empty());
        assert!(
            waits
                .iter()
                .all(|evaluation| evaluation["params"]["arguments"][2]["value"] == false)
        );
        // One last evaluation picks the fields, in the same isolated
        // world as the others.
        assert_eq!(last["params"]["arguments"][2]["value"], true);
        assert_eq!(last["params"]["executionContextId"], 7);
    }

    #[test]
    fn a_fill_reports_why_it_found_no_login_fields() {
        let (cdp, _) = browser(Script {
            evaluations: vec![waiting()],
            picked: json!({"result": {"type": "string", "value": "the page has no password field"}}),
            ..Script::default()
        });
        let tab = Tab::new(LOAD, FOCUS);
        tab.open(&cdp, "https://login.example.com/a").unwrap();

        let refused = tab
            .fill(&cdp, "https://login.example.com", &login())
            .unwrap_err();

        assert_eq!(refused, "the page has no password field");
    }

    #[test]
    fn a_debug_print_of_a_field_never_shows_the_text() {
        let printed = format!("{:?}", login());

        assert!(!printed.contains("s3cret"), "{printed}");
    }
}
