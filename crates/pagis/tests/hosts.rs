//! Full-daemon Host tests.
//!
//! A client registers as a Host over the socket it already
//! authenticated, and it is present while that socket lives. These tests
//! drive the whole path: the registration, the two surfaces that read
//! presence, the rule that one person never sees another person's
//! machine, and the rule that only the connection that received a
//! command answers it.

use pagis_testkit::{HostAnswer, HostClient, TestDaemon, TwoTenants};

async fn hosts_of(daemon: &TestDaemon, cookie: &str) -> Vec<serde_json::Value> {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/hosts", daemon.base_url))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json::<serde_json::Value>().await.unwrap()["items"]
        .as_array()
        .unwrap()
        .clone()
}

/// The client registers and the person's own list shows the machine as
/// present. The record carries what the client declared, because the
/// client is the authority for all of it.
#[tokio::test]
async fn a_client_registers_and_the_person_sees_it_as_present() {
    let daemon = TestDaemon::start().await;

    let client = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::ok()).await;

    let hosts = hosts_of(&daemon, daemon.cookie()).await;
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0]["id"], client.host_id());
    assert_eq!(hosts[0]["name"], "Air");
    assert_eq!(hosts[0]["platform"], "macos");
    assert_eq!(hosts[0]["capabilities"], serde_json::json!(["shell"]));
    assert_eq!(hosts[0]["present"], true);
}

/// The answer to a registration names the Harness Catalog with the
/// programs each harness needs on the `PATH` of the machine. The client
/// looks for them and registers again with a `harness:<id>` capability
/// for each harness it found, and the record holds what it declared.
#[tokio::test]
async fn the_registration_answer_names_the_catalog_and_a_second_registration_declares_a_harness() {
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let daemon = TestDaemon::start().await;
    let mut socket = daemon.event_socket(daemon.cookie()).await;
    let register = |capabilities: &[&str]| {
        Message::text(
            serde_json::json!({
                "type": "register_host",
                "name": "Air",
                "platform": "macos",
                "capabilities": capabilities,
            })
            .to_string(),
        )
    };

    socket.send(register(&["shell"])).await.unwrap();
    let first = next_frame_of(&mut socket, "host.registered").await;

    assert_eq!(
        first["payload"]["harnesses"],
        serde_json::json!([
            { "id": "claude", "launchers": ["npx"] },
            { "id": "codex", "launchers": ["npx"] },
            { "id": "opencode", "launchers": ["opencode"] },
            { "id": "pi", "launchers": ["npx", "pi"] },
            { "id": "gemini", "launchers": ["npx"] },
            { "id": "copilot", "launchers": ["npx"] },
            { "id": "cursor", "launchers": ["cursor-agent"] },
        ])
    );
    assert_eq!(
        first["payload"]["capabilities"],
        serde_json::json!(["shell"])
    );

    socket
        .send(register(&["shell", "harness:claude"]))
        .await
        .unwrap();
    let second = next_frame_of(&mut socket, "host.registered").await;

    assert_eq!(second["payload"]["host_id"], first["payload"]["host_id"]);
    assert_eq!(
        second["payload"]["harnesses"],
        first["payload"]["harnesses"]
    );
    let declared = serde_json::json!(["shell", "harness:claude"]);
    assert_eq!(second["payload"]["capabilities"], declared);
    let hosts = hosts_of(&daemon, daemon.cookie()).await;
    assert_eq!(hosts.len(), 1, "one machine is one record: {hosts:?}");
    assert_eq!(hosts[0]["capabilities"], declared);
}

/// The machine is absent the moment its socket closes, and the record
/// says when it was last here. The same machine that comes back is the
/// same Host, so a Grant that names it still reaches it.
#[tokio::test]
async fn a_closed_client_is_absent_and_keeps_its_identity() {
    let daemon = TestDaemon::start().await;
    let client = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::ok()).await;
    let host_id = client.host_id().to_string();

    client.disconnect();
    // The daemon writes the last-seen time as the socket closes, so the
    // read waits for the close to land rather than racing it.
    let absent = loop {
        let hosts = hosts_of(&daemon, daemon.cookie()).await;
        if hosts[0]["present"] == false {
            break hosts[0].clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(absent["id"], host_id);
    assert!(absent["last_seen_at"].as_i64().unwrap() > 0);

    let again = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::ok()).await;

    assert_eq!(again.host_id(), host_id);
    let hosts = hosts_of(&daemon, daemon.cookie()).await;
    assert_eq!(hosts.len(), 1, "one machine is one record: {hosts:?}");
    assert_eq!(hosts[0]["present"], true);
}

/// A phone registers without the shell capability, and the record says
/// so. What it can do is the client's word, and the broker reads it when
/// it decides which machines a shell command may reach.
#[tokio::test]
async fn a_phone_registers_without_a_shell() {
    let daemon = TestDaemon::start().await;

    let _phone = HostClient::connect(&daemon, "Phone", "ios", &[], HostAnswer::ok()).await;

    let hosts = hosts_of(&daemon, daemon.cookie()).await;
    assert_eq!(hosts[0]["capabilities"], serde_json::json!([]));
    assert_eq!(hosts[0]["present"], true);
}

/// One person never sees another person's machine: the read names the
/// Workspace of the Session that made it.
#[tokio::test]
async fn one_person_never_sees_another_persons_host() {
    let tenants = TwoTenants::start().await;

    let mine_client = HostClient::connect_as(
        &tenants.daemon,
        &tenants.a.cookie,
        "A's Air",
        "macos",
        &["shell"],
        HostAnswer::ok(),
    )
    .await;

    let mine = hosts_of(&tenants.daemon, &tenants.a.cookie).await;
    assert!(
        mine.iter().any(|host| host["id"] == mine_client.host_id()),
        "A sees their own machine: {mine:?}"
    );
    assert!(
        hosts_of(&tenants.daemon, &tenants.b.cookie)
            .await
            .is_empty(),
        "B sees none of A's machines"
    );
}

/// The administration port shows every machine of the installation
/// beside the Sessions, with the person who owns it and whether a host
/// action could run on it now.
#[tokio::test]
async fn the_administration_port_shows_host_presence() {
    let daemon = TestDaemon::start().await;
    let client = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::ok()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/administration/hosts",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body = response.json::<serde_json::Value>().await.unwrap();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], client.host_id());
    assert_eq!(items[0]["name"], "Air");
    assert_eq!(items[0]["present"], true);
    assert_eq!(items[0]["person"]["id"], daemon.user_id.to_string());
}

/// The whole path, with two machines connected: the sprite calls the
/// tool, the person is asked which computer, taps one, approves the
/// command on that machine, and the command runs there and nowhere else.
/// The grant the approval writes names that machine, and the audit fact
/// carries its id.
#[tokio::test]
async fn a_person_with_two_machines_is_asked_which_one_and_the_command_runs_there() {
    use futures::{SinkExt, StreamExt};
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;

    let brain = std::sync::Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "git status" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: std::sync::Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let air = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::Echo).await;
    let studio =
        HostClient::connect(&daemon, "Studio", "linux", &["shell"], HostAnswer::Echo).await;

    let (mut socket, _) = connect_async(daemon.ws_request(&daemon.ws_url()))
        .await
        .expect("the firehose connects");
    socket
        .send(Message::text(
            serde_json::json!({"type": "auth"}).to_string(),
        ))
        .await
        .unwrap();
    let client = reqwest::Client::new();
    client
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"pending_id": "p-1", "text": "check the repository"}))
        .send()
        .await
        .unwrap();

    /// The next `request.created` of the firehose, with its Request row.
    async fn next_request(
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        daemon: &TestDaemon,
    ) -> serde_json::Value {
        loop {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(10), socket.next())
                .await
                .expect("a frame before the timeout")
                .expect("the socket stays open")
                .expect("a readable frame");
            let Message::Text(text) = frame else { continue };
            let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
            if frame["type"] != "request.created" {
                continue;
            }
            let id = frame["payload"]["payload"]["request_id"].as_str().unwrap();
            return reqwest::Client::new()
                .get(format!("{}/api/v1/requests/{id}", daemon.base_url))
                .header("cookie", daemon.cookie())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
        }
    }

    // The person is asked which computer, and both of theirs are offered.
    let question = next_request(&mut socket, &daemon).await;
    assert_eq!(question["kind"], "choice");
    assert_eq!(
        question["payload"]["title"],
        "Which computer should this run on?"
    );
    let options = question["payload"]["options"].as_array().unwrap();
    assert_eq!(
        options
            .iter()
            .map(|option| option["label"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["Air (macos)", "Studio (linux)"]
    );

    // They tap the Studio.
    let decided = client
        .post(format!(
            "{}/api/v1/requests/{}/decision",
            daemon.base_url,
            question["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "decision": "approved",
            "values": {"value": studio.host_id()},
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(decided.status(), 200);

    // The approval card that follows names the Studio.
    let approval = next_request(&mut socket, &daemon).await;
    assert_eq!(approval["kind"], "tool_action");
    assert_eq!(
        approval["payload"]["action_title"],
        "Run a command on your Studio"
    );
    assert_eq!(approval["payload"]["host_id"], studio.host_id());

    client
        .post(format!(
            "{}/api/v1/requests/{}/decision",
            daemon.base_url,
            approval["id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"decision": "approved", "scope": "always"}))
        .send()
        .await
        .unwrap();

    // The command ran on the Studio and on nothing else.
    let ran = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if !studio.commands().is_empty() {
                return studio.commands();
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the Studio ran the command");
    assert_eq!(ran, vec!["git status".to_string()]);
    assert!(air.commands().is_empty(), "the Air ran nothing");

    // The grant the approval wrote names the Studio, and its rule
    // belongs to that machine alone.
    let grants = client
        .get(format!("{}/api/v1/grants", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let items = grants["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["resource_kind"], "host");
    assert_eq!(items[0]["resource_id"], studio.host_id());
    assert_eq!(items[0]["allow"], serde_json::json!(["git status"]));

    // The audit fact of the action names the machine it ran on. The
    // daemon writes it after the command answers.
    let fact = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let events = daemon
                .stores()
                .events
                .list_by_types(&daemon.workspace_id, &["tool.completed"], None, 20)
                .await
                .unwrap();
            if let Some(fact) = events
                .into_iter()
                .find(|event| event.payload["name"] == "host_shell")
            {
                return fact;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the host action is in the audit log");
    assert_eq!(fact.payload["host_id"], studio.host_id());
}

/// A Host lives as long as its socket, and the socket lives no longer
/// than its Session. A sign-out closes the socket with 1008, the machine
/// is absent before the client reads the Close, and a host action that
/// follows is answered at once as absent: nothing runs on a machine
/// whose Session ended.
#[tokio::test]
async fn a_host_on_a_revoked_socket_is_absent_and_a_host_action_answers_so() {
    use futures::{SinkExt, StreamExt};
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};
    use tokio_tungstenite::tungstenite::Message;

    let brain = std::sync::Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "git status" }),
    ));
    brain.push(Script::reply(&["Noted."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: std::sync::Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let signing_out = daemon.cookie_for(&daemon.user_id).await;
    let mut socket = daemon.event_socket(&signing_out).await;
    socket
        .send(Message::text(
            serde_json::json!({
                "type": "register_host",
                "name": "Air",
                "platform": "macos",
                "capabilities": ["shell"],
            })
            .to_string(),
        ))
        .await
        .unwrap();
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
            .await
            .expect("a frame before the timeout")
            .expect("the socket stays open")
            .expect("a readable frame");
        let frame: serde_json::Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
        if frame["type"] == "host.registered" {
            break;
        }
    }
    assert_eq!(hosts_of(&daemon, daemon.cookie()).await[0]["present"], true);

    daemon.sign_out(&signing_out).await;

    assert_eq!(
        pagis_testkit::read_until_closed(&mut socket).await.code,
        1008
    );
    let hosts = hosts_of(&daemon, daemon.cookie()).await;
    assert_eq!(hosts[0]["name"], "Air");
    assert_eq!(hosts[0]["present"], false, "the machine is still present");

    // A host action for the machine, from the Person's other Session.
    let mut firehose = daemon.event_socket(daemon.cookie()).await;
    let sent = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"pending_id": "p-1", "text": "check the repository"}))
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), 201);
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(10), firehose.next())
            .await
            .expect("a frame before the timeout")
            .expect("the socket stays open")
            .expect("a readable frame");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_ne!(
            frame["type"], "request.created",
            "the action asked the Person to approve a command for an absent machine"
        );
        if frame["type"] == "run.state_changed" && frame["payload"]["payload"]["to"] == "completed"
        {
            break;
        }
    }
    let requests = brain.requests();
    let result = &requests
        .last()
        .expect("the model read the tool result")
        .messages;
    let result = &result.last().expect("the tool result").text;
    assert!(
        result.contains("Air is not connected"),
        "the host action was not answered as absent: {result}"
    );
}

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

/// A daemon whose Agent runs `git status` on a Host once, and then
/// replies.
async fn a_daemon_with_one_host_action()
-> (TestDaemon, std::sync::Arc<pagis_testkit::ScriptedBrain>) {
    use pagis_testkit::{Script, ScriptedBrain, TestDaemonOptions};

    let brain = std::sync::Arc::new(ScriptedBrain::default());
    brain.push(Script::tool_call(
        &[],
        "host_shell",
        serde_json::json!({ "command": "git status" }),
    ));
    brain.push(Script::reply(&["Done."]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: std::sync::Arc::clone(&brain) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    (daemon, brain)
}

/// The next text frame of `socket` with the type `frame_type`.
async fn next_frame_of(socket: &mut pagis_testkit::Socket, frame_type: &str) -> serde_json::Value {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(10), socket.next())
            .await
            .expect("a frame before the timeout")
            .expect("the socket stays open")
            .expect("a readable frame");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        if frame["type"] == frame_type {
            return frame;
        }
    }
}

/// Ask the Agent for its host action, approve the card once, and answer
/// the call id of the command when it reaches `host`. The host action
/// then waits for its result. `firehose` is an event socket of the
/// daemon's own person.
async fn a_waiting_host_action(
    daemon: &TestDaemon,
    firehose: &mut pagis_testkit::Socket,
    host: &HostClient,
) -> String {
    let client = reqwest::Client::new();
    let sent = client
        .post(format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"pending_id": "p-1", "text": "check the repository"}))
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), 201);
    let card = next_frame_of(firehose, "request.created").await;
    let decided = client
        .post(format!(
            "{}/api/v1/requests/{}/decision",
            daemon.base_url,
            card["payload"]["payload"]["request_id"].as_str().unwrap()
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({"decision": "approved"}))
        .send()
        .await
        .unwrap();
    assert_eq!(decided.status(), 200);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(dispatch) = host.dispatched().first() {
                return dispatch.id.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the command reaches the machine")
}

/// The tool result the model read, once the Run completes.
async fn the_tool_result(
    firehose: &mut pagis_testkit::Socket,
    brain: &pagis_testkit::ScriptedBrain,
) -> String {
    loop {
        let changed = next_frame_of(firehose, "run.state_changed").await;
        if changed["payload"]["payload"]["to"] == "completed" {
            break;
        }
    }
    let requests = brain.requests();
    requests
        .last()
        .expect("the model read the tool result")
        .messages
        .last()
        .expect("the tool result")
        .text
        .clone()
}

/// A Host id names a machine and proves nothing. The machine of another
/// person sends a result with the exact call id of this person's host
/// action, and the host action does not complete: the Agent reads what
/// this person's own machine answered.
#[tokio::test]
async fn a_result_from_another_persons_host_does_not_answer_a_host_action() {
    let (daemon, brain) = a_daemon_with_one_host_action().await;
    let air = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::Manual).await;
    let other = another_person(&daemon).await;
    let intruder = HostClient::connect_as(
        &daemon,
        &other,
        "Air",
        "macos",
        &["shell"],
        HostAnswer::ok(),
    )
    .await;
    let mut firehose = daemon.event_socket(daemon.cookie()).await;

    let call_id = a_waiting_host_action(&daemon, &mut firehose, &air).await;
    intruder
        .answer(&call_id, "forged by another person\n")
        .await;
    air.answer(&call_id, "the real output\n").await;

    let result = the_tool_result(&mut firehose, &brain).await;
    assert!(
        result.contains("the real output") && !result.contains("forged"),
        "the Agent did not read the answer of its own machine: {result}"
    );
}

/// A socket that registered no Host received no command. Its result with
/// the exact call id of a host action does not complete the host action.
#[tokio::test]
async fn a_result_from_a_socket_without_a_host_does_not_answer_a_host_action() {
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let (daemon, brain) = a_daemon_with_one_host_action().await;
    let air = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::Manual).await;
    let other = another_person(&daemon).await;
    let mut no_host = daemon.event_socket(&other).await;
    let mut firehose = daemon.event_socket(daemon.cookie()).await;

    let call_id = a_waiting_host_action(&daemon, &mut firehose, &air).await;
    for frame in [
        serde_json::json!({
            "type": "result",
            "id": call_id,
            "exit_code": 0,
            "stdout": "forged by a socket with no host\n",
        }),
        serde_json::json!({"type": "ping"}),
    ] {
        no_host
            .send(Message::text(frame.to_string()))
            .await
            .unwrap();
    }
    // The daemon reads the frames of one socket in order, so the pong
    // says that it has read the result.
    next_frame_of(&mut no_host, "pong").await;
    air.answer(&call_id, "the real output\n").await;

    let result = the_tool_result(&mut firehose, &brain).await;
    assert!(
        result.contains("the real output") && !result.contains("forged"),
        "the Agent did not read the answer of its own machine: {result}"
    );
}

/// The machine connects again and the new connection replaces the old
/// one. The command goes to the new connection, and a result from the
/// old connection does not complete it.
#[tokio::test]
async fn a_result_from_a_replaced_connection_does_not_answer_a_host_action() {
    let (daemon, brain) = a_daemon_with_one_host_action().await;
    let old = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::ok()).await;
    let new = HostClient::connect(&daemon, "Air", "macos", &["shell"], HostAnswer::Manual).await;
    assert_eq!(new.host_id(), old.host_id(), "one machine is one Host");
    let mut firehose = daemon.event_socket(daemon.cookie()).await;

    let call_id = a_waiting_host_action(&daemon, &mut firehose, &new).await;
    old.answer(&call_id, "forged by the old connection\n").await;
    new.answer(&call_id, "the real output\n").await;

    let result = the_tool_result(&mut firehose, &brain).await;
    assert!(
        result.contains("the real output") && !result.contains("forged"),
        "the Agent did not read the answer of the connection that ran the command: {result}"
    );
    assert!(old.commands().is_empty(), "the old connection ran nothing");
}
