//! The daemon process stops on the signals that a supervisor sends:
//! SIGTERM from Docker, systemd and a compose stop, and SIGINT from
//! Ctrl-C and the Client App.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Docker sends SIGKILL ten seconds after SIGTERM, so a graceful stop
/// ends well before that.
const STOP_LIMIT: Duration = Duration::from_secs(8);

/// Start a local daemon in a new state directory on free ports, and
/// wait until it prints the last line of its start banner.
fn started_daemon(home: &Path) -> Child {
    started(Command::new(env!("CARGO_BIN_EXE_pagis")), home).0
}

/// A port that no process holds now, on every interface. The model port
/// is never 0, because the egress rules name it, so each daemon of a
/// test takes one of these.
fn free_port() -> u16 {
    std::net::TcpListener::bind("0.0.0.0:0")
        .expect("a free port")
        .local_addr()
        .expect("the bound address")
        .port()
}

/// Start a local daemon from `command` in the state directory `home` on
/// free ports, and wait until it prints the last line of its start
/// banner. Answers the process and the local origin that the banner
/// names.
///
/// The state directory holds a Key File before the start, so the daemon
/// does not ask the keychain or the Secret Service for the Installation
/// Key. Each build of the binary has a new ad-hoc code signature, and
/// the keychain stops the start on an approval prompt for it.
pub(crate) fn started(mut command: Command, home: &Path) -> (Child, String) {
    write_key_file(home);
    let mut child = command
        .args(["--local", "--no-open", "--port", "0"])
        .env("PAGIS_HOME", home)
        .env("PAGIS_ADMINISTRATION_PORT", "0")
        .env("PAGIS_COMPUTER_MODEL_PORT", free_port().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the pagis binary starts");
    let stdout = child.stdout.take().expect("the daemon's stdout");
    let (lines, banner) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                return;
            }
        }
    });
    match local_origin(&banner) {
        Ok(origin) => (child, origin),
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the daemon printed no start banner: {error}");
        }
    }
}

/// The local origin in the last line of the start banner: the origin of
/// the one-time sign-in link.
fn local_origin(banner: &mpsc::Receiver<String>) -> Result<String, mpsc::RecvTimeoutError> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let line = banner.recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
        if let Some(link) = line.strip_prefix("sign in within one minute: ") {
            let origin = link.split("/api/").next().unwrap_or(link);
            return Ok(origin.to_string());
        }
    }
}

/// Write the Key File of a Local Installation: 64 hexadecimal characters,
/// mode 600, in the file `installation-key` of the state directory.
fn write_key_file(home: &Path) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(home.join("installation-key"))
        .expect("the Key File is written");
    file.write_all("0123456789abcdef".repeat(4).as_bytes())
        .expect("the Key File holds the key");
}

/// Send one signal to the daemon, and wait for it to exit.
pub(crate) fn stop(mut child: Child, signal: &str) -> std::process::ExitStatus {
    let sent = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(sent.success(), "kill {signal} failed");
    let deadline = Instant::now() + STOP_LIMIT;
    loop {
        if let Some(status) = child.try_wait().expect("the daemon's status") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the daemon did not stop within {STOP_LIMIT:?} of {signal}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sigterm_stops_the_daemon_gracefully() {
    let home = tempfile::tempdir().unwrap();
    let daemon = started_daemon(home.path());

    let status = stop(daemon, "-TERM");

    assert_eq!(status.code(), Some(0), "{status}");
}

#[test]
fn sigint_stops_the_daemon_gracefully() {
    let home = tempfile::tempdir().unwrap();
    let daemon = started_daemon(home.path());

    let status = stop(daemon, "-INT");

    assert_eq!(status.code(), Some(0), "{status}");
}
