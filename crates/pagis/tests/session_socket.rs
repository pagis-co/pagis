//! Full-daemon session socket tests (ADR-0033).
//!
//! A Client App that declares a `harness:<id>` capability opens the
//! session socket of its Host, and the daemon opens one stream on it for
//! each Coding Session. These tests drive the whole path over real
//! sockets: the socket route, its Person rule, the end of the Session, the
//! open request and its answer, the bytes of the process both ways, and
//! the `session_exit` frame of the Host socket. The Client App's end of
//! the socket is the fake Client App of `pagis_broker`, which speaks the
//! protocol of the Client App and writes back each byte that it reads.

use std::collections::BTreeMap;
use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt};
use pagis_broker::fake::{FakeAnswer, FakeClientApp};
use pagis_broker::{OpenFailure, OpenRequest, SessionExit, SessionOpenError};
use pagis_core::{CodingSessionId, HostId};
use pagis_testkit::{HostAnswer, HostClient, SessionClient, TestDaemon, TwoTenants};
use tokio::sync::oneshot::error::TryRecvError;

const WAIT: Duration = Duration::from_secs(5);

async fn wait_until(what: &str, ready: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !ready() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn request(session_id: &CodingSessionId) -> OpenRequest {
    OpenRequest {
        session_id: session_id.clone(),
        command: "npx".to_string(),
        args: vec![
            "--yes".to_string(),
            "@agentclientprotocol/claude-agent-acp@0.87.0".to_string(),
        ],
        cwd: "/Users/bo/code/app".to_string(),
        env: BTreeMap::new(),
        worktree: None,
    }
}

/// A Host of the daemon's own person that can start Claude Code, with
/// its session socket open and served by `client_app`.
async fn host_with_session_socket(
    daemon: &TestDaemon,
    cookie: &str,
    client_app: std::sync::Arc<FakeClientApp>,
) -> (HostClient, SessionClient, HostId) {
    let host = HostClient::connect_as(
        daemon,
        cookie,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let host_id = HostId::from(host.host_id().to_string());
    let sessions = SessionClient::connect(daemon, cookie, host.host_id(), client_app)
        .await
        .expect("the session socket opens");
    wait_until("the session socket is not open", || {
        daemon.host_sessions.is_open(&host_id)
    })
    .await;
    (host, sessions, host_id)
}

/// A Client App that declares `harness:claude` opens the session socket
/// of its Host with its Session, and the socket is open while it lives.
#[tokio::test]
async fn a_host_that_declares_a_harness_opens_its_session_socket() {
    let daemon = TestDaemon::start().await;
    let (_host, sessions, host_id) =
        host_with_session_socket(&daemon, daemon.cookie(), FakeClientApp::opening("/work")).await;

    sessions.disconnect();

    wait_until("the session socket stays open", || {
        !daemon.host_sessions.is_open(&host_id)
    })
    .await;
}

/// The session socket of another Person's Host does not open: the Host
/// read names the Session's Workspace, so that Host is absent. A Host
/// that declared no harness starts no Coding Session.
#[tokio::test]
async fn the_session_socket_of_another_persons_host_or_of_a_host_without_a_harness_is_refused() {
    let tenants = TwoTenants::start().await;
    let daemon = &tenants.daemon;
    let a_host = HostClient::connect_as(
        daemon,
        &tenants.a.cookie,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let a_shell_only = HostClient::connect_as(
        daemon,
        &tenants.a.cookie,
        "Mini",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;

    let refused = SessionClient::connect(
        daemon,
        &tenants.b.cookie,
        a_host.host_id(),
        FakeClientApp::opening("/work"),
    )
    .await
    .err();
    assert_eq!(refused, Some(404));
    let refused = SessionClient::connect(
        daemon,
        &tenants.a.cookie,
        a_shell_only.host_id(),
        FakeClientApp::opening("/work"),
    )
    .await
    .err();
    assert_eq!(refused, Some(409));
    assert!(
        !daemon
            .host_sessions
            .is_open(&HostId::from(a_host.host_id().to_string()))
    );
}

/// The session socket lives no longer than its Session: a sign-out
/// closes it with 1008, and the socket is no longer open.
#[tokio::test]
async fn the_session_socket_closes_with_1008_when_the_session_ends() {
    let daemon = TestDaemon::start().await;
    let cookie = daemon.cookie_for(&daemon.user_id).await;
    let (_host, mut sessions, host_id) =
        host_with_session_socket(&daemon, &cookie, FakeClientApp::opening("/work")).await;

    daemon.sign_out(&cookie).await;

    assert_eq!(sessions.closed().await, Some(1008));
    wait_until("the session socket stays open", || {
        !daemon.host_sessions.is_open(&host_id)
    })
    .await;
}

/// `open` carries the request to the Client App and the bytes of the
/// process both ways, and the `session_exit` frame of the Host socket
/// reaches the session's `exit`.
#[tokio::test]
async fn a_session_carries_its_request_its_bytes_and_its_exit() {
    let daemon = TestDaemon::start().await;
    let client_app = FakeClientApp::opening("/Users/bo/.pagis-worktrees/app/pagis/fix");
    let (host, _sessions, host_id) =
        host_with_session_socket(&daemon, daemon.cookie(), std::sync::Arc::clone(&client_app))
            .await;
    let session_id = CodingSessionId::generate();

    let mut opened = daemon
        .host_sessions
        .open(&daemon.workspace_id, &host_id, &request(&session_id))
        .await
        .expect("the session opens");

    assert_eq!(opened.cwd, "/Users/bo/.pagis-worktrees/app/pagis/fix");
    assert_eq!(client_app.requests(), [request(&session_id)]);
    let line = b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\"}\n";
    opened.stream.write_all(line).await.expect("write stdin");
    let mut echoed = vec![0; line.len()];
    tokio::time::timeout(WAIT, opened.stream.read_exact(&mut echoed))
        .await
        .expect("stdout in time")
        .expect("read stdout");
    assert_eq!(echoed, line);
    assert_eq!(opened.exit.try_recv(), Err(TryRecvError::Empty));

    host.session_exit(session_id.as_str(), Some(1), "error: not signed in\n")
        .await;

    let exit = tokio::time::timeout(WAIT, opened.exit)
        .await
        .expect("the exit in time")
        .expect("the exit arrives");
    assert_eq!(
        exit,
        SessionExit {
            exit_code: Some(1),
            stderr_tail: "error: not signed in\n".to_string(),
        }
    );
}

/// A `not_found` answer of the Client App is `Refused` with its code
/// and message.
#[tokio::test]
async fn a_not_found_answer_is_refused() {
    let daemon = TestDaemon::start().await;
    let client_app = FakeClientApp::answering(FakeAnswer::Refuse {
        code: OpenFailure::NotFound,
        message: "npx is not on PATH".to_string(),
    });
    let (_host, _sessions, host_id) =
        host_with_session_socket(&daemon, daemon.cookie(), client_app).await;

    let opened = daemon
        .host_sessions
        .open(
            &daemon.workspace_id,
            &host_id,
            &request(&CodingSessionId::generate()),
        )
        .await;

    assert_eq!(
        opened.err(),
        Some(SessionOpenError::Refused {
            code: OpenFailure::NotFound,
            message: "npx is not on PATH".to_string(),
        })
    );
}

/// `open` for a Host with no session socket answers `NotConnected` at
/// once, and does not wait for a socket to come.
#[tokio::test]
async fn open_for_a_host_with_no_session_socket_is_not_connected_at_once() {
    let daemon = TestDaemon::start().await;
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;

    let opened = tokio::time::timeout(
        Duration::from_millis(500),
        daemon.host_sessions.open(
            &daemon.workspace_id,
            &HostId::from(host.host_id().to_string()),
            &request(&CodingSessionId::generate()),
        ),
    )
    .await
    .expect("the answer comes at once");

    assert_eq!(opened.err(), Some(SessionOpenError::NotConnected));
}

/// Person B opens nothing on person A's Host, and a `session_exit` from
/// person B's Host socket with the session id of person A does not reach
/// person A's session.
#[tokio::test]
async fn person_b_neither_opens_a_session_on_person_as_host_nor_ends_one() {
    let tenants = TwoTenants::start().await;
    let daemon = &tenants.daemon;
    let (a_host, _a_sessions, a_host_id) =
        host_with_session_socket(daemon, &tenants.a.cookie, FakeClientApp::opening("/work")).await;
    let b_host = HostClient::connect_as(
        daemon,
        &tenants.b.cookie,
        "Air",
        "macos",
        &["shell", "harness:claude"],
        HostAnswer::ok(),
    )
    .await;
    let session_id = CodingSessionId::generate();

    let opened = daemon
        .host_sessions
        .open(&tenants.b.workspace_id, &a_host_id, &request(&session_id))
        .await;
    assert_eq!(opened.err(), Some(SessionOpenError::NotConnected));

    let mut opened = daemon
        .host_sessions
        .open(&tenants.a.workspace_id, &a_host_id, &request(&session_id))
        .await
        .expect("person A's session opens");
    b_host
        .session_exit(session_id.as_str(), Some(0), "from person B")
        .await;
    assert_eq!(opened.exit.try_recv(), Err(TryRecvError::Empty));

    a_host.session_exit(session_id.as_str(), Some(0), "").await;
    let exit = tokio::time::timeout(WAIT, opened.exit)
        .await
        .expect("the exit in time")
        .expect("the exit arrives");
    assert_eq!(exit.stderr_tail, "");
}
