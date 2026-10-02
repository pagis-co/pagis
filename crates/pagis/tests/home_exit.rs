//! Full-daemon Home Exit tests (ADR-0029).
//!
//! A Client App connected to a Server opens the exit socket of its Host,
//! and the Host is present as a Home Exit while the socket lives. On a
//! Server the exit listener takes the `CONNECT` of a Computer's Exit Proxy
//! and carries it through the Person's Home Exit. These tests drive the
//! whole path over real sockets: the socket route, its Person rule, the
//! end of the Session, the exit listener of a Server, the start
//! environment of a Computer, the Person's choice in Settings with the
//! live switch of their Computers, the exit in use, and the System
//! Setting of the Administrator. The Client App's end of the socket is the
//! fake Home Exit of `pagis_computer`, which speaks the protocol of the
//! Client App; the interop test at the end drives the Client App's own
//! code.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_computer::ExitMode;
use pagis_computer::fake::{FakeComputerRuntime, FakeHomeExit};
use pagis_core::HostId;
use pagis_testkit::{ExitClient, HostAnswer, HostClient, TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

const WAIT: Duration = Duration::from_secs(5);

/// A second person of the same installation, with a Workspace of their
/// own, and the `Cookie` header of their Session.
async fn another_person(daemon: &TestDaemon) -> String {
    let org_id = pagis_core::UserStore::get(daemon.stores().users.as_ref(), &daemon.user_id)
        .await
        .expect("read the seeded person")
        .expect("the boot seeds one person")
        .org_id;
    let person = pagis_core::User {
        email: Some("bo@example.com".to_string()),
        name: Some("Bo".to_string()),
        ..pagis_core::User::new(org_id, pagis_core::UserRole::Member, pagis_core::now_ms())
    };
    daemon
        .stores()
        .users
        .create(&person)
        .await
        .expect("write the second person");
    pagis_server::provisioning::WorkspaceSeed::from(daemon.stores())
        .run(
            &person.id,
            "Bo's Workspace",
            "UTC",
            pagis_server::provisioning::Onboarding::Done,
            pagis_core::now_ms(),
        )
        .await
        .expect("seed the second person's Workspace");
    daemon.cookie_for(&person.id).await
}

/// A TCP target on loopback that greets each connection, then sends back
/// what it reads.
async fn echo_target() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                stream.write_all(b"hello from the target\n").await.ok();
                let (mut reader, mut writer) = stream.split();
                tokio::io::copy(&mut reader, &mut writer).await.ok();
            });
        }
    });
    address
}

async fn wait_until(what: &str, ready: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !ready() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A Client App that declares `exit` opens the exit socket of its Host
/// with its Session, and the Host is a present Home Exit while the
/// socket lives.
#[tokio::test]
async fn a_host_that_declares_exit_opens_its_exit_socket() {
    let daemon = TestDaemon::start().await;
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;
    let host_id = HostId::from(host.host_id().to_string());

    let exit = ExitClient::connect(
        &daemon,
        daemon.cookie(),
        host.host_id(),
        FakeHomeExit::to(echo_target().await),
    )
    .await
    .expect("the exit socket opens");
    wait_until("the Host is no present Home Exit", || {
        daemon.home_exits.is_open(&host_id)
    })
    .await;

    exit.disconnect();
    wait_until("the Host stays a present Home Exit", || {
        !daemon.home_exits.is_open(&host_id)
    })
    .await;
}

/// The exit socket of another Person's Host does not open: the Host read
/// names the Session's Workspace, so that Host is absent. A Host that
/// declared no `exit` carries no exit traffic.
#[tokio::test]
async fn the_exit_socket_of_another_persons_host_or_of_a_host_without_exit_is_refused() {
    let daemon = TestDaemon::start().await;
    let theirs = another_person(&daemon).await;
    let their_host = HostClient::connect_as(
        &daemon,
        &theirs,
        "Their Air",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;
    let phone = HostClient::connect(&daemon, "Phone", "ios", &[], HostAnswer::ok()).await;
    let exit = FakeHomeExit::to(echo_target().await);

    let refused = ExitClient::connect(
        &daemon,
        daemon.cookie(),
        their_host.host_id(),
        Arc::clone(&exit),
    )
    .await
    .err();
    assert_eq!(refused, Some(404));
    let refused = ExitClient::connect(&daemon, daemon.cookie(), phone.host_id(), exit)
        .await
        .err();
    assert_eq!(refused, Some(409));
    assert!(
        !daemon
            .home_exits
            .is_open(&HostId::from(their_host.host_id().to_string()))
    );
}

/// The exit socket lives no longer than its Session: a sign-out closes
/// it with 1008, and the Host is absent as a Home Exit.
#[tokio::test]
async fn the_exit_socket_closes_with_1008_when_the_session_ends() {
    let daemon = TestDaemon::start().await;
    let cookie = daemon.cookie_for(&daemon.user_id).await;
    let host = HostClient::connect_as(
        &daemon,
        &cookie,
        "Air",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;
    let host_id = HostId::from(host.host_id().to_string());
    let mut exit = ExitClient::connect(
        &daemon,
        &cookie,
        host.host_id(),
        FakeHomeExit::to(echo_target().await),
    )
    .await
    .expect("the exit socket opens");
    wait_until("the Host is no present Home Exit", || {
        daemon.home_exits.is_open(&host_id)
    })
    .await;

    daemon.sign_out(&cookie).await;

    assert_eq!(exit.closed().await, Some(1008));
    assert!(!daemon.home_exits.is_open(&host_id));
}

/// A Local Installation opens no exit listener, and its Computers name
/// none and start in Direct mode, also with a Home Exit chosen.
#[tokio::test]
async fn a_local_installation_opens_no_exit_listener() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        computer: Arc::clone(&runtime) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let host = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::ok()).await;
    daemon
        .stores()
        .workspaces
        .set_home_exit(
            &daemon.workspace_id,
            Some(&HostId::from(host.host_id().to_string())),
        )
        .await
        .expect("the Home Exit is written");

    wake(&daemon).await;

    assert_eq!(daemon.exit_addr, None);
    let env = runtime.start_envs().pop().expect("one start");
    assert!(
        !env.iter().any(|entry| entry.starts_with("PAGIS_EXIT_")),
        "{env:?}"
    );
}

/// Wake the seeded Agent's Computer and wait until it is awake.
async fn wake(daemon: &TestDaemon) {
    let client = reqwest::Client::new();
    let woke = client
        .post(format!(
            "{}/api/v1/agents/{}/computer/wake",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("the wake answers");
    assert!(woke.status().is_success(), "{}", woke.status());
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let state: serde_json::Value = client
            .get(format!(
                "{}/api/v1/agents/{}/computer",
                daemon.base_url, daemon.agent_id
            ))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .expect("the state answers")
            .json()
            .await
            .expect("the state is JSON");
        if state["state"] == "awake" {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never awake: {state}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// One `CONNECT` as the Exit Proxy sends it, and the head of the answer.
async fn connect(listener: SocketAddr, token: &str, target: &str) -> (TcpStream, String) {
    let mut stream = TcpStream::connect(listener)
        .await
        .expect("reach the exit listener");
    stream
        .write_all(
            format!(
                "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\
                 Proxy-Authorization: Bearer {token}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .expect("send the CONNECT");
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        match tokio::time::timeout(WAIT, stream.read_u8()).await {
            Ok(Ok(byte)) => head.push(byte),
            Ok(Err(_)) => break,
            Err(_) => panic!("no answer in time"),
        }
    }
    (stream, String::from_utf8_lossy(&head).into_owned())
}

/// A Server opens the exit listener. An Agent's Computer of a Person who
/// chose a Home Exit starts in Home mode with the address of the
/// listener, and the listener carries the `CONNECT` of its Exit Proxy
/// through the exit socket of that Host. When the Client App goes away,
/// the connection closes.
#[tokio::test]
async fn a_server_carries_a_computers_connection_through_its_persons_home_exit() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let Some(daemon) = TestDaemon::start_on_postgres_with(TestDaemonOptions {
        computer: Arc::clone(&runtime) as _,
        ..TestDaemonOptions::default()
    })
    .await
    else {
        return;
    };
    let listener = daemon.exit_addr.expect("a Server opens the exit listener");
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;
    let host_id = HostId::from(host.host_id().to_string());
    assert!(
        daemon
            .stores()
            .workspaces
            .set_home_exit(&daemon.workspace_id, Some(&host_id))
            .await
            .expect("the Home Exit is written")
    );
    let exit = FakeHomeExit::to(echo_target().await);
    let client_app =
        ExitClient::connect(&daemon, daemon.cookie(), host.host_id(), Arc::clone(&exit))
            .await
            .expect("the exit socket opens");
    wait_until("the Host is no present Home Exit", || {
        daemon.home_exits.is_open(&host_id)
    })
    .await;

    wake(&daemon).await;
    let env = runtime.start_envs().pop().expect("one start");
    for entry in [
        format!("PAGIS_EXIT_DAEMON=host.docker.internal:{}", listener.port()),
        "PAGIS_EXIT_MODE=home".to_string(),
    ] {
        assert!(env.contains(&entry), "missing {entry}: {env:?}");
    }

    // The fake runtime gives each Computer this token.
    let token = format!("fake-token-{}", daemon.agent_id);
    let (_refused, head) = connect(listener, "no-computer-has-this", "shop.example.com:443").await;
    assert!(head.starts_with("HTTP/1.1 407"), "{head}");
    let (mut stream, head) = connect(listener, &token, "shop.example.com:443").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut greeting = [0; 22];
    tokio::time::timeout(WAIT, stream.read_exact(&mut greeting))
        .await
        .expect("the greeting arrives in time")
        .expect("the greeting is readable");
    assert_eq!(&greeting, b"hello from the target\n");
    stream.write_all(b"ping\n").await.expect("send");
    let mut pong = [0; 5];
    tokio::time::timeout(WAIT, stream.read_exact(&mut pong))
        .await
        .expect("the echo arrives in time")
        .expect("the echo is readable");
    assert_eq!(&pong, b"ping\n");
    assert_eq!(exit.destinations(), ["shop.example.com:443"]);
    assert_eq!(daemon.home_exits.bytes(&daemon.workspace_id).sent, 5);

    client_app.disconnect();

    let mut byte = [0; 1];
    assert!(
        matches!(
            tokio::time::timeout(WAIT, stream.read(&mut byte)).await,
            Ok(Ok(0) | Err(_))
        ),
        "the connection stays open after the Home Exit went away"
    );
}

/// One request of the signed-in Person's own Home Exit route.
async fn home_exit(
    daemon: &TestDaemon,
    cookie: &str,
    method: reqwest::Method,
    body: Option<serde_json::Value>,
) -> reqwest::Response {
    let request = reqwest::Client::new()
        .request(
            method,
            format!("{}/api/v1/settings/home-exit", daemon.base_url),
        )
        .header("cookie", cookie);
    match body {
        Some(body) => request.json(&body),
        None => request,
    }
    .send()
    .await
    .expect("the Home Exit route answers")
}

/// Choose `host_id` as the Person's Home Exit through the route, and
/// answer the response.
async fn choose(daemon: &TestDaemon, host_id: &str) -> reqwest::Response {
    home_exit(
        daemon,
        daemon.cookie(),
        reqwest::Method::PUT,
        Some(serde_json::json!({ "host_id": host_id })),
    )
    .await
}

/// The Home Exit System Setting through the Administration Port.
async fn switch_the_system_setting(daemon: &TestDaemon, enabled: bool) -> reqwest::Response {
    reqwest::Client::new()
        .put(format!(
            "{}/api/v1/settings/system/home-exit",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "enabled": enabled }))
        .send()
        .await
        .expect("the System Setting route answers")
}

/// The Computer of the seeded Agent, as its view reads it.
async fn computer(daemon: &TestDaemon) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!(
            "{}/api/v1/agents/{}/computer",
            daemon.base_url, daemon.agent_id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("the state answers")
        .json()
        .await
        .expect("the state is JSON")
}

/// Wait until the Computer's view shows `exit`.
async fn wait_for_exit(daemon: &TestDaemon, exit: serde_json::Value) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let shown = computer(daemon).await;
        if shown["exit"] == exit {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the view shows {shown}, not {exit}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The event stream of the Product App, signed in as the seeded Person.
async fn firehose(daemon: &TestDaemon) -> Socket {
    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("the event socket opens");
    socket
        .send(Message::text(
            serde_json::json!({ "type": "auth" }).to_string(),
        ))
        .await
        .expect("send the auth frame");
    next_frame_of(&mut socket, "ready").await;
    socket
}

/// The next JSON frame of `frame_type` on the event stream.
async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(WAIT, socket.next())
            .await
            .expect("a frame in time")
            .expect("the socket is open")
            .expect("the frame is readable");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).expect("the frame is JSON");
        if frame["type"] == frame_type {
            return frame;
        }
    }
}

/// A Server over the fake runtime, or `None` where Docker does not run
/// the Postgres of the test.
async fn server(runtime: &Arc<FakeComputerRuntime>) -> Option<TestDaemon> {
    TestDaemon::start_on_postgres_with(TestDaemonOptions {
        computer: Arc::clone(runtime) as _,
        ..TestDaemonOptions::default()
    })
    .await
}

/// The Person chooses their own Host as the Home Exit in Settings, and
/// the Exit Proxy of their awake Computer switches to Home mode at once,
/// with no restart. Its view says which exit is in use, and each change
/// shows on the event stream: the server while the Host is absent, the
/// Host by its name while its exit socket is open. Turned off, the
/// Computer switches back and shows no exit.
#[tokio::test]
async fn a_person_chooses_their_own_host_and_their_awake_computer_switches() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let Some(daemon) = server(&runtime).await else {
        return;
    };
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;
    HostClient::connect(&daemon, "Phone", "ios", &[], HostAnswer::ok()).await;
    wake(&daemon).await;
    let agent_id = pagis_core::AgentId::from(daemon.agent_id.clone());
    assert_eq!(runtime.exit_mode(&agent_id), Some(ExitMode::Direct));
    let starts = runtime.starts();

    let read = home_exit(&daemon, daemon.cookie(), reqwest::Method::GET, None).await;
    assert_eq!(read.status(), StatusCode::OK);
    let read: serde_json::Value = read.json().await.expect("JSON");
    assert_eq!(read["available"], true);
    assert_eq!(read["administrator_turned_off"], false);
    assert_eq!(read["chosen"], serde_json::Value::Null);
    let hosts = read["hosts"].as_array().expect("the Hosts");
    assert_eq!(hosts.len(), 1, "only a Host that declares exit: {read}");
    assert_eq!(hosts[0]["name"], "Air");
    assert_eq!(hosts[0]["present"], false);

    let chose = choose(&daemon, host.host_id()).await;
    assert_eq!(chose.status(), StatusCode::OK);
    let chose: serde_json::Value = chose.json().await.expect("JSON");
    assert_eq!(chose["home_exit"]["chosen"]["id"], host.host_id());
    assert_eq!(chose["not_switched"], serde_json::json!([]));
    assert_eq!(runtime.exit_mode(&agent_id), Some(ExitMode::Home));
    assert_eq!(
        runtime.starts(),
        starts,
        "the switch restarted the Computer"
    );
    assert_eq!(computer(&daemon).await["exit"], "exit: server");

    let mut events = firehose(&daemon).await;
    let _client_app = ExitClient::connect(
        &daemon,
        daemon.cookie(),
        host.host_id(),
        FakeHomeExit::to(echo_target().await),
    )
    .await
    .expect("the exit socket opens");
    wait_for_exit(&daemon, serde_json::json!("exit: Air")).await;
    let change = next_frame_of(&mut events, "computer.exit_changed").await;
    assert_eq!(change["payload"]["agent_id"], daemon.agent_id);
    assert_eq!(change["payload"]["payload"]["exit"], "exit: Air");
    let read: serde_json::Value = home_exit(&daemon, daemon.cookie(), reqwest::Method::GET, None)
        .await
        .json()
        .await
        .expect("JSON");
    assert_eq!(read["chosen"]["present"], true);

    let cleared = home_exit(&daemon, daemon.cookie(), reqwest::Method::DELETE, None).await;
    assert_eq!(cleared.status(), StatusCode::OK);
    let cleared: serde_json::Value = cleared.json().await.expect("JSON");
    assert_eq!(cleared["home_exit"]["chosen"], serde_json::Value::Null);
    assert_eq!(runtime.exit_mode(&agent_id), Some(ExitMode::Direct));
    assert_eq!(computer(&daemon).await["exit"], serde_json::Value::Null);
}

/// Only a Host of the Person that declared `exit` can be their Home
/// Exit, and the route says why it refuses another: the Host of another
/// Person is not one of theirs, and a Host without `exit` carries no
/// exit traffic. The choice does not change.
#[tokio::test]
async fn the_home_exit_is_a_host_of_the_person_that_declared_exit() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let Some(daemon) = server(&runtime).await else {
        return;
    };
    let theirs = another_person(&daemon).await;
    let their_host = HostClient::connect_as(
        &daemon,
        &theirs,
        "Their Air",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;
    let phone = HostClient::connect(&daemon, "Phone", "ios", &["shell"], HostAnswer::ok()).await;

    let refused = choose(&daemon, their_host.host_id()).await;
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = refused.json().await.expect("JSON");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("host")),
        "{body}"
    );

    let refused = choose(&daemon, phone.host_id()).await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = refused.json().await.expect("JSON");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Phone") && message.contains("`exit`")),
        "{body}"
    );

    let read: serde_json::Value = home_exit(&daemon, daemon.cookie(), reqwest::Method::GET, None)
        .await
        .json()
        .await
        .expect("JSON");
    assert_eq!(read["chosen"], serde_json::Value::Null);
    let workspace = daemon
        .stores()
        .workspaces
        .get(&daemon.workspace_id)
        .await
        .expect("read")
        .expect("the Workspace");
    assert_eq!(workspace.home_exit_host_id, None);
}

/// A Computer whose Exit Proxy does not switch keeps its mode, and the
/// answer names it. The choice is saved, and the Computer takes it at
/// its next wake.
#[tokio::test]
async fn the_answer_names_a_computer_that_did_not_switch() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let Some(daemon) = server(&runtime).await else {
        return;
    };
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;
    wake(&daemon).await;
    let agent_id = pagis_core::AgentId::from(daemon.agent_id.clone());
    runtime.fail_exit_switch(&agent_id, "screend does not answer");

    let chose = choose(&daemon, host.host_id()).await;

    assert_eq!(chose.status(), StatusCode::OK);
    let chose: serde_json::Value = chose.json().await.expect("JSON");
    assert_eq!(chose["home_exit"]["chosen"]["id"], host.host_id());
    let failed = chose["not_switched"].as_array().expect("a list");
    assert_eq!(failed.len(), 1, "{chose}");
    assert_eq!(failed[0]["agent_id"], daemon.agent_id);
    assert!(
        failed[0]["agent_name"]
            .as_str()
            .is_some_and(|name| !name.is_empty())
    );
    assert!(
        failed[0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("screend does not answer")),
        "{chose}"
    );
    assert_eq!(runtime.exit_mode(&agent_id), Some(ExitMode::Direct));
    assert_eq!(computer(&daemon).await["exit"], serde_json::Value::Null);
}

/// An Administrator turns the Home Exit off for the installation: the
/// awake Computer switches to Direct mode at once, the Person's view
/// says that the Administrator turned it off, their choice stays, and
/// they cannot choose another. On again, the choice is back in effect.
#[tokio::test]
async fn the_administrator_turns_the_home_exit_off_and_on_for_the_installation() {
    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let Some(daemon) = server(&runtime).await else {
        return;
    };
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;
    assert_eq!(
        choose(&daemon, host.host_id()).await.status(),
        StatusCode::OK
    );
    wake(&daemon).await;
    let agent_id = pagis_core::AgentId::from(daemon.agent_id.clone());
    assert_eq!(runtime.exit_mode(&agent_id), Some(ExitMode::Home));

    let off = switch_the_system_setting(&daemon, false).await;

    assert_eq!(off.status(), StatusCode::OK);
    let off: serde_json::Value = off.json().await.expect("JSON");
    assert_eq!(off["settings"]["home_exit"]["enabled"], false);
    assert_eq!(off["not_switched"], 0);
    let config = pagis::Config::read_file(&daemon.booted.home.join("config.toml")).unwrap();
    assert!(!config.computer.home_exit);
    assert_eq!(runtime.exit_mode(&agent_id), Some(ExitMode::Direct));
    assert_eq!(computer(&daemon).await["exit"], serde_json::Value::Null);
    let read: serde_json::Value = home_exit(&daemon, daemon.cookie(), reqwest::Method::GET, None)
        .await
        .json()
        .await
        .expect("JSON");
    assert_eq!(read["administrator_turned_off"], true);
    assert_eq!(read["chosen"]["id"], host.host_id());
    let refused = choose(&daemon, host.host_id()).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = refused.json().await.expect("JSON");
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Administrator")),
        "{body}"
    );

    let on = switch_the_system_setting(&daemon, true).await;

    assert_eq!(on.status(), StatusCode::OK);
    assert_eq!(runtime.exit_mode(&agent_id), Some(ExitMode::Home));
    assert_eq!(computer(&daemon).await["exit"], "exit: server");
}

/// A Local Installation has no Home Exit: its Computers leave from the
/// owner's own connection. The Person's route says it is not available
/// and changes nothing, and the System Settings name no Home Exit.
#[tokio::test]
async fn a_local_installation_has_no_home_exit() {
    let daemon = TestDaemon::start().await;
    let host = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::ok()).await;

    let read = home_exit(&daemon, daemon.cookie(), reqwest::Method::GET, None).await;
    assert_eq!(read.status(), StatusCode::OK);
    let read: serde_json::Value = read.json().await.expect("JSON");
    assert_eq!(read["available"], false);
    assert_eq!(read["hosts"], serde_json::json!([]));

    assert_eq!(
        choose(&daemon, host.host_id()).await.status(),
        StatusCode::CONFLICT
    );
    let cleared = home_exit(&daemon, daemon.cookie(), reqwest::Method::DELETE, None).await;
    assert_eq!(cleared.status(), StatusCode::CONFLICT);
    assert_eq!(
        switch_the_system_setting(&daemon, false).await.status(),
        StatusCode::CONFLICT
    );
    let settings: serde_json::Value = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/settings/system",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .expect("the settings answer")
        .json()
        .await
        .expect("JSON");
    assert_eq!(settings["home_exit"], serde_json::Value::Null);
}

/// The repository root, read at run time so a binary built in another
/// worktree reads this one.
fn repository() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the dir"))
        .join("../..")
}

/// A TCP target on loopback that sends back what it reads, and closes
/// its side when the other side closed its own.
async fn bulk_echo_target() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind a port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut reader, mut writer) = stream.split();
                tokio::io::copy(&mut reader, &mut writer).await.ok();
                writer.shutdown().await.ok();
            });
        }
    });
    address
}

/// More than a window of one yamux stream each way, so both ends wait
/// for window updates on the way.
const BULK: usize = 1024 * 1024;

/// The interop of the two ends of the exit socket: the Rust yamux of the
/// daemon and the Client App's own TypeScript (`desktop/src/yamux.ts`
/// and `desktop/src/exit.ts`), over the real WebSocket of the daemon.
/// Several streams carry more than a window each way at once, and a FIN
/// from the daemon ends the connection at the target, whose own end then
/// ends the stream. When the Client App goes away, the stream that it
/// carried ends, and the Home Exit is absent.
///
/// The Client App's end runs under `node`, 22.18 or later, which strips
/// the types. Its dial connects every stream to the target of the test,
/// which is on loopback, where the real address check refuses it.
#[tokio::test]
async fn the_daemon_and_the_client_app_carry_bytes_both_ways_over_the_exit_socket() {
    let daemon = TestDaemon::start().await;
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;
    let host_id = HostId::from(host.host_id().to_string());
    assert!(
        daemon
            .stores()
            .workspaces
            .set_home_exit(&daemon.workspace_id, Some(&host_id))
            .await
            .expect("the Home Exit is written")
    );
    let target = bulk_echo_target().await;
    let secret = daemon
        .cookie()
        .strip_prefix(&format!("{}=", pagis_server::SESSION_COOKIE))
        .expect("the cookie names the Session");
    let mut client_app = tokio::process::Command::new("node")
        .current_dir(repository())
        .args([
            "--disable-warning=MODULE_TYPELESS_PACKAGE_JSON",
            "desktop/test/exit-peer.ts",
            &daemon.base_url,
            secret,
            host.host_id(),
            &target.to_string(),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("node runs; the interop test needs Node.js 22.18 or later on the PATH");
    let mut ready = String::new();
    let mut stdout = tokio::io::BufReader::new(client_app.stdout.take().expect("stdout"));
    tokio::time::timeout(
        Duration::from_secs(30),
        tokio::io::AsyncBufReadExt::read_line(&mut stdout, &mut ready),
    )
    .await
    .expect("the Client App is ready in time")
    .expect("the Client App writes its stdout");
    assert_eq!(ready, "ready\n");
    wait_until("the Client App is no present Home Exit", || {
        daemon.home_exits.is_open(&host_id)
    })
    .await;

    let carried = futures::future::join_all((0..4).map(|index| {
        let home_exits = Arc::clone(&daemon.home_exits);
        let workspace_id = daemon.workspace_id.clone();
        async move {
            let mut stream = home_exits
                .open(&workspace_id, &format!("site-{index}.example.com"), 443)
                .await
                .expect("the Home Exit is present")
                .expect("the Home Exit carries the connection");
            let sent: Vec<u8> = (0..BULK).map(|byte| ((byte + index) % 251) as u8).collect();
            let (mut reader, mut writer) = tokio::io::split(&mut stream);
            let mut back = Vec::new();
            let (written, read) = tokio::join!(
                async {
                    writer.write_all(&sent).await?;
                    // The FIN of the daemon ends the target's input, so the
                    // target closes its side, and the stream ends.
                    writer.shutdown().await
                },
                tokio::time::timeout(Duration::from_secs(30), reader.read_to_end(&mut back)),
            );
            written.expect("the bytes go out");
            read.expect("the bytes come back in time")
                .expect("the bytes are readable");
            assert!(back == sent, "stream {index} came back changed");
        }
    }));
    carried.await;
    let bytes = daemon.home_exits.bytes(&daemon.workspace_id);
    assert_eq!(bytes.sent, 4 * BULK as u64);
    assert_eq!(bytes.received, 4 * BULK as u64);

    // The Client App goes away under an open connection.
    let mut open = daemon
        .home_exits
        .open(&daemon.workspace_id, "last.example.com", 443)
        .await
        .expect("the Home Exit is present")
        .expect("the Home Exit carries the connection");
    open.write_all(b"ping").await.expect("send");
    let mut pong = [0; 4];
    tokio::time::timeout(WAIT, open.read_exact(&mut pong))
        .await
        .expect("the echo arrives in time")
        .expect("the echo is readable");
    client_app.kill().await.expect("the Client App stops");
    let mut byte = [0; 1];
    assert!(
        matches!(
            tokio::time::timeout(WAIT, open.read(&mut byte)).await,
            Ok(Ok(0) | Err(_))
        ),
        "the connection stays open after the Client App went away"
    );
    wait_until("the Home Exit stays present", || {
        !daemon.home_exits.is_open(&host_id)
    })
    .await;
    assert!(
        daemon
            .home_exits
            .open(&daemon.workspace_id, "after.example.com", 443)
            .await
            .is_none()
    );
}

/// Bulk bytes both ways at once on several streams, as an upload while a
/// page loads, from a peer that pings. The yamux of the daemon reads no
/// frame while it holds the answer to a Ping, and it holds it until the
/// socket takes its bytes. The exit socket carries the two directions
/// apart, so the bytes to the Client App go out while the bytes from it
/// wait, and neither side waits for the other for ever. The pipe of the
/// daemon is 64 KiB, so each stream carries many times its size.
#[tokio::test]
async fn bulk_bytes_both_ways_on_several_streams_keep_the_exit_socket_moving() {
    const STREAMS: usize = 8;
    const BYTES: usize = 4 * 1024 * 1024;
    let daemon = TestDaemon::start().await;
    let host = HostClient::connect(
        &daemon,
        "Air",
        "macos",
        &["shell", "exit"],
        HostAnswer::ok(),
    )
    .await;
    let host_id = HostId::from(host.host_id().to_string());
    assert!(
        daemon
            .stores()
            .workspaces
            .set_home_exit(&daemon.workspace_id, Some(&host_id))
            .await
            .expect("the Home Exit is written")
    );
    let _client_app = ExitClient::connect_pinging(
        &daemon,
        daemon.cookie(),
        host.host_id(),
        FakeHomeExit::to(bulk_echo_target().await),
    )
    .await
    .expect("the exit socket opens");
    wait_until("the Host is no present Home Exit", || {
        daemon.home_exits.is_open(&host_id)
    })
    .await;

    let carried = futures::future::join_all((0..STREAMS).map(|index| {
        let home_exits = Arc::clone(&daemon.home_exits);
        let workspace_id = daemon.workspace_id.clone();
        async move {
            let mut stream = home_exits
                .open(&workspace_id, &format!("upload-{index}.example.com"), 443)
                .await
                .expect("the Home Exit is present")
                .expect("the Home Exit carries the connection");
            let sent: Vec<u8> = (0..BYTES)
                .map(|byte| ((byte + index) % 253) as u8)
                .collect();
            let (mut reader, mut writer) = tokio::io::split(&mut stream);
            let mut back = Vec::with_capacity(BYTES);
            let (written, read) = tokio::join!(
                async {
                    writer.write_all(&sent).await?;
                    writer.shutdown().await
                },
                reader.read_to_end(&mut back),
            );
            written.expect("the bytes go out");
            read.expect("the bytes come back");
            assert!(back == sent, "stream {index} came back changed");
        }
    }));
    tokio::time::timeout(Duration::from_secs(30), carried)
        .await
        .expect("the exit socket stopped: the bytes of one direction waited for the other");
    let bytes = daemon.home_exits.bytes(&daemon.workspace_id);
    assert_eq!(bytes.sent, (STREAMS * BYTES) as u64);
    assert_eq!(bytes.received, (STREAMS * BYTES) as u64);
}
