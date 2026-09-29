//! The browser and its DevTools pipe (ADR-0013).
//!
//! screend starts the browser itself, so it holds the other end of the
//! pipe that the browser reads its DevTools commands from. screend gives
//! the two ends to `pagis-browser` as its standard input and output, and
//! the script hands them to Chromium as the descriptors 3 and 4 that
//! `--remote-debugging-pipe` reads and writes. The pipe has no name in
//! the file system and no port, and only screend and Chromium hold it.
//! Both run as `screen`. The Agent's shell runs as `agent`, and one uid
//! cannot open the file descriptors of another uid's process.
//!
//! screend also supervises the browser. Without supervision, a single
//! exit of the browser leaves the Computer with no browser for the rest
//! of the session, and the only symptom is an Agent that sees a terminal
//! and nothing else. So screend starts it again, with a new pipe each
//! time, and backs off:
//! only exits that come fast one after another earn the longer wait, and
//! each start and exit is in the session log.

use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cdp::Cdp;

/// The first wait before a start after an exit, and the longest one.
const FIRST_DELAY: Duration = Duration::from_secs(1);
const LONGEST_DELAY: Duration = Duration::from_secs(30);
/// A browser that ran this long before it exited is a fresh start, not
/// a crash loop.
const STEADY: Duration = Duration::from_secs(30);

/// The supervised browser and the DevTools connection of its current
/// run.
pub struct Browser {
    connection: Mutex<Option<Arc<Cdp>>>,
}

impl Browser {
    /// Start `program` with its arguments, and start it again each time
    /// it exits.
    pub fn start(program: &[&str]) -> Arc<Self> {
        let browser = Arc::new(Self {
            connection: Mutex::new(None),
        });
        let supervised = Arc::clone(&browser);
        let program: Vec<String> = program.iter().map(|part| part.to_string()).collect();
        std::thread::spawn(move || supervised.supervise(&program));
        browser
    }

    /// The DevTools connection of the browser that runs now, or `None`
    /// while no browser runs.
    pub fn connection(&self) -> Option<Arc<Cdp>> {
        self.connection.lock().expect("browser lock").clone()
    }

    fn supervise(&self, program: &[String]) {
        let mut delay = FIRST_DELAY;
        loop {
            let started = Instant::now();
            match launch(program) {
                Ok((mut child, cdp)) => {
                    *self.connection.lock().expect("browser lock") = Some(cdp);
                    let status = child.wait();
                    *self.connection.lock().expect("browser lock") = None;
                    if started.elapsed() >= STEADY {
                        delay = FIRST_DELAY;
                    }
                    eprintln!(
                        "[screend] the browser exited ({status:?}); starting it again in {}s",
                        delay.as_secs()
                    );
                }
                Err(error) => eprintln!(
                    "[screend] the browser did not start: {error}; trying again in {}s",
                    delay.as_secs()
                ),
            }
            std::thread::sleep(delay);
            delay = (delay * 2).min(LONGEST_DELAY);
        }
    }
}

/// Start the browser with a new pipe on its standard streams, and a
/// client on screend's ends of it. The command drops at the end of this
/// function, so screend keeps only its own two ends and sees the end of
/// the pipe when the browser exits.
fn launch(program: &[String]) -> std::io::Result<(Child, Arc<Cdp>)> {
    let (commands_reader, commands_writer) = std::io::pipe()?;
    let (events_reader, events_writer) = std::io::pipe()?;
    let (name, arguments) = program
        .split_first()
        .ok_or_else(|| std::io::Error::other("no browser program"))?;
    let child = Command::new(name)
        .args(arguments)
        .stdin(commands_reader)
        .stdout(events_writer)
        .spawn()?;
    Ok((child, Cdp::new(events_reader, commands_writer)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wait until the browser has a connection that is not `previous`.
    fn next_connection(browser: &Browser, previous: Option<&Arc<Cdp>>) -> Arc<Cdp> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(connection) = browser.connection()
                && !previous.is_some_and(|previous| Arc::ptr_eq(previous, &connection))
            {
                return connection;
            }
            assert!(Instant::now() < deadline, "no browser connection");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// `cat` stands in for the browser: it sends each command back on
    /// the pipe, and the client takes the copy for the answer.
    #[test]
    fn the_browser_gets_the_pipe_and_comes_back_after_it_exits() {
        let browser = Browser::start(&["sh", "-c", "timeout 2 cat"]);

        let first = next_connection(&browser, None);
        first
            .call(
                None,
                "Browser.getVersion",
                serde_json::json!({}),
                Duration::from_secs(5),
            )
            .expect("the program reads and writes the pipe");

        let second = next_connection(&browser, Some(&first));
        second
            .call(
                None,
                "Browser.getVersion",
                serde_json::json!({}),
                Duration::from_secs(5),
            )
            .expect("the program that came back has a pipe of its own");
        assert!(
            first
                .call(
                    None,
                    "Browser.getVersion",
                    serde_json::json!({}),
                    Duration::from_secs(5)
                )
                .is_err(),
            "the pipe of the program that exited still answers"
        );
    }
}
