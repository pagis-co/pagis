//! Remote Access (ADR-0028): the switch in the Administration Interface
//! over a fake Tailscale, who reaches a Local Installation, the password
//! that another machine does not send, and the TURN server that carries
//! the live screen to another machine.
//!
//! A request "through the Funnel" is what `tailscaled` on the owner's
//! machine sends to the daemon: it connects from loopback, passes on the
//! public name in `Host`, and writes `X-Forwarded-For`,
//! `X-Forwarded-Proto` and `Tailscale-Funnel-Request`.

use std::sync::Arc;
use std::time::Duration;

use pagis_core::{User, UserRole, now_ms};
use pagis_server::{FunnelPort, SESSION_COOKIE};
use pagis_testkit::tailscale::{ENABLE_URL, TAILNET_NAME};
use pagis_testkit::{FakeTailscale, TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;

/// The Public Origin that Remote Access writes for the fake tailnet.
pub(crate) const TAILNET_ORIGIN: &str = "https://owner-mac.tail1234.ts.net";

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build a client")
}

fn switch_url(daemon: &TestDaemon) -> String {
    format!(
        "{}/api/v1/settings/system/remote-access",
        daemon.administration_base_url
    )
}

async fn read(daemon: &TestDaemon) -> serde_json::Value {
    let response = client()
        .get(switch_url(daemon))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

async fn turn_on(daemon: &TestDaemon) -> reqwest::Response {
    client()
        .put(switch_url(daemon))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
}

async fn turn_off(daemon: &TestDaemon) -> reqwest::Response {
    client()
        .delete(switch_url(daemon))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
}

/// Read the switch until no turn-on waits.
async fn settled(daemon: &TestDaemon) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let read = read(daemon).await;
        if read["turning_on"].is_null() {
            return read;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the turn-on did not end: {read}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Read the switch until the turn-on shows the page that Tailscale names.
async fn waiting_at_the_page(daemon: &TestDaemon) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let read = read(daemon).await;
        if read["turning_on"]["enable_url"].is_string() {
            return read;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the turn-on named no page: {read}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn config_file(daemon: &TestDaemon) -> pagis::Config {
    pagis::Config::read_file(&daemon.booted.home.join("config.toml")).unwrap()
}

fn raw_config(daemon: &TestDaemon) -> String {
    std::fs::read_to_string(daemon.booted.home.join("config.toml")).unwrap()
}

/// A Local Installation that runs in Remote Access, as the switch and
/// the restart leave it.
pub(crate) fn in_remote_access(tailscale: Arc<FakeTailscale>) -> TestDaemonOptions {
    TestDaemonOptions {
        remote_access: true,
        public_origin: TAILNET_ORIGIN.to_string(),
        trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
        tailscale,
        ..TestDaemonOptions::default()
    }
}

/// A request that `tailscaled` forwards from a browser on the internet.
pub(crate) fn through_the_funnel(method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
    client()
        .request(method, url)
        .header("host", TAILNET_NAME)
        .header("x-forwarded-for", "203.0.113.9")
        .header("x-forwarded-proto", "https")
        .header("tailscale-funnel-request", "?1")
}

/// A Member of the Org with a password and their own Workspace.
pub(crate) async fn member(daemon: &TestDaemon, email: &str, password: &str) -> User {
    let now = now_ms();
    let org = daemon.stores().orgs.list().await.unwrap().remove(0);
    let person = User {
        email: Some(email.to_string()),
        name: Some("Grace".to_string()),
        password_hash: Some(pagis_server::hash_password(password).unwrap()),
        ..User::new(org.id, UserRole::Member, now)
    };
    daemon.stores().users.create(&person).await.unwrap();
    daemon
        .stores()
        .workspaces
        .create(&pagis_core::Workspace {
            id: pagis_core::WorkspaceId::generate(),
            user_id: person.id.clone(),
            name: "Grace".to_string(),
            timezone: "UTC".to_string(),
            created_at: now,
            onboarded_at: Some(now),
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
        })
        .await
        .unwrap();
    person
}

/// The change that the switch asks the fake Tailscale for: Funnel on or
/// off for the product port and the TURN port of `daemon`.
fn funnel_change(on_or_off: &str, daemon: &TestDaemon) -> String {
    format!(
        "funnel {on_or_off} {} {}",
        daemon.addr.port(),
        daemon.remote_access_turn_addr.port()
    )
}

fn sets_a_session(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| {
            value.starts_with(&format!("{SESSION_COOKIE}=")) && !value.contains("Max-Age=0")
        })
}

// --- the switch ---------------------------------------------------------

/// The switch reads each state of Tailscale, with Remote Access off.
#[tokio::test]
async fn the_switch_reads_each_state_of_tailscale() {
    for (tailscale, state) in [
        (FakeTailscale::not_installed(), "not_installed"),
        (FakeTailscale::not_running(), "not_running"),
        (FakeTailscale::funnel_off(), "funnel_off"),
        (
            FakeTailscale::ready(FunnelPort::Nothing, FunnelPort::Nothing),
            "ready",
        ),
    ] {
        let daemon = TestDaemon::start_with(TestDaemonOptions {
            tailscale: Arc::new(tailscale),
            ..TestDaemonOptions::default()
        })
        .await;

        let read = read(&daemon).await;

        assert_eq!(read["tailscale"]["state"], state, "{read}");
        assert_eq!(read["enabled"], false, "{read}");
        assert_eq!(read["switchable"], true, "{read}");
        assert_eq!(read["public_origin"], serde_json::Value::Null);
        assert_eq!(read["turning_on"], serde_json::Value::Null);
    }
}

/// Turning on runs Funnel to the product port and to the TURN server of
/// the live screen, then writes the Public Origin of the machine on the
/// tailnet and the Trusted Proxy, keeps the Bind Address on loopback, and
/// says that a restart puts it in effect. The daemon does not restart
/// until the page asks.
#[tokio::test]
async fn turning_on_runs_funnel_and_writes_the_origin_and_the_proxy() {
    let tailscale = Arc::new(FakeTailscale::ready(
        FunnelPort::Nothing,
        FunnelPort::Nothing,
    ));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        tailscale: Arc::clone(&tailscale) as _,
        ..TestDaemonOptions::default()
    })
    .await;

    let response = turn_on(&daemon).await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let read = settled(&daemon).await;
    assert_eq!(read["enabled"], true, "{read}");
    assert_eq!(read["public_origin"], TAILNET_ORIGIN);
    assert_eq!(read["restart_required"], true);
    assert_eq!(read["failure"], serde_json::Value::Null);
    assert_eq!(read["tailscale"]["port_443"]["serves"], "pagis");
    assert_eq!(read["tailscale"]["port_8443"]["serves"], "pagis");
    assert_eq!(tailscale.changes(), [funnel_change("on", &daemon)]);
    let config = config_file(&daemon);
    assert!(config.remote_access.enabled);
    assert_eq!(config.public_origin, TAILNET_ORIGIN);
    assert_eq!(config.trusted_proxy, "127.0.0.1");
    assert!(config.bind_address().unwrap().is_loopback());
    assert!(!daemon.restart.is_asked());
}

/// Where the tailnet has HTTPS and Funnel off, the switch shows the page
/// that Tailscale names and writes nothing until the owner approves.
#[tokio::test]
async fn with_funnel_off_the_switch_shows_the_page_and_waits_for_the_approval() {
    let tailscale = Arc::new(FakeTailscale::funnel_off());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        tailscale: Arc::clone(&tailscale) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let before = raw_config(&daemon);

    assert_eq!(turn_on(&daemon).await.status(), StatusCode::ACCEPTED);
    let waiting = waiting_at_the_page(&daemon).await;

    assert_eq!(waiting["turning_on"]["enable_url"], ENABLE_URL);
    assert_eq!(waiting["enabled"], false);
    assert_eq!(raw_config(&daemon), before);
    // A second turn-on joins the one that waits.
    assert_eq!(turn_on(&daemon).await.status(), StatusCode::ACCEPTED);

    tailscale.approve();

    let read = settled(&daemon).await;
    assert_eq!(read["enabled"], true, "{read}");
    assert_eq!(read["public_origin"], TAILNET_ORIGIN);
    assert_eq!(tailscale.changes().len(), 1, "{:?}", tailscale.changes());
}

/// A read of the switch that a turn-on overtakes shows the end of the
/// turn-on whole: the Public Origin it wrote, and no turn-on that waits.
/// The read waits on `tailscale status`, and the turn-on ends meanwhile.
#[tokio::test]
async fn a_read_that_a_turn_on_overtakes_shows_its_end_whole() {
    let tailscale = Arc::new(FakeTailscale::funnel_off());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        tailscale: Arc::clone(&tailscale) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    assert_eq!(turn_on(&daemon).await.status(), StatusCode::ACCEPTED);
    waiting_at_the_page(&daemon).await;

    tailscale.hold_next_state_read();
    let overtaken = read(&daemon);
    let turn_on_ends = async {
        tailscale.read_is_held().await;
        tailscale.approve();
        settled(&daemon).await;
        tailscale.release_held_read();
    };
    let (overtaken, ()) = tokio::join!(overtaken, turn_on_ends);

    assert!(overtaken["turning_on"].is_null(), "{overtaken}");
    assert_eq!(overtaken["enabled"], true, "{overtaken}");
    assert_eq!(overtaken["public_origin"], TAILNET_ORIGIN);
}

/// A turn-on that `tailscale funnel` refuses says why, and writes
/// nothing.
#[tokio::test]
async fn a_turn_on_that_tailscale_refuses_says_why_and_writes_nothing() {
    let tailscale = Arc::new(
        FakeTailscale::ready(FunnelPort::Nothing, FunnelPort::Nothing)
            .refusing("Funnel not available on this build"),
    );
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        tailscale: Arc::clone(&tailscale) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    let before = raw_config(&daemon);

    assert_eq!(turn_on(&daemon).await.status(), StatusCode::ACCEPTED);
    let read = settled(&daemon).await;

    assert_eq!(read["failure"], "Funnel not available on this build");
    assert_eq!(read["enabled"], false);
    assert_eq!(raw_config(&daemon), before);
}

/// With no Tailscale, a Tailscale that does not run, or a port 443 or a
/// port 8443 that serves something else, a turn-on is refused at once
/// with what to do, and Funnel is not touched.
#[tokio::test]
async fn turning_on_is_refused_where_tailscale_cannot_serve_pagis() {
    for (tailscale, says) in [
        (FakeTailscale::not_installed(), "install it from"),
        (FakeTailscale::not_running(), "open Tailscale, sign in"),
        (
            FakeTailscale::ready(
                FunnelPort::Other {
                    target: "/ http://127.0.0.1:3000".to_string(),
                },
                FunnelPort::Nothing,
            ),
            "http://127.0.0.1:3000",
        ),
        (
            FakeTailscale::ready(
                FunnelPort::Nothing,
                FunnelPort::Other {
                    target: "127.0.0.1:5432".to_string(),
                },
            ),
            "--tls-terminated-tcp=8443 off",
        ),
    ] {
        let tailscale = Arc::new(tailscale);
        let daemon = TestDaemon::start_with(TestDaemonOptions {
            tailscale: Arc::clone(&tailscale) as _,
            ..TestDaemonOptions::default()
        })
        .await;
        let before = raw_config(&daemon);

        let response = turn_on(&daemon).await;

        assert_eq!(response.status(), StatusCode::CONFLICT, "{says}");
        let refused: serde_json::Value = response.json().await.unwrap();
        let message = refused["error"]["message"].as_str().unwrap();
        assert!(message.contains(says), "{message}");
        assert!(tailscale.changes().is_empty(), "{:?}", tailscale.changes());
        assert_eq!(raw_config(&daemon), before);
    }
}

/// Turning off removes both Funnel ports of Pagis and clears Remote
/// Access, the Public Origin and the Trusted Proxy. A daemon that runs in
/// Remote Access needs a restart to leave it.
#[tokio::test]
async fn turning_off_removes_the_funnel_and_clears_the_three_settings() {
    let tailscale = Arc::new(FakeTailscale::ready(FunnelPort::Pagis, FunnelPort::Pagis));
    let daemon = TestDaemon::start_with(in_remote_access(Arc::clone(&tailscale))).await;
    let read = read(&daemon).await;
    assert_eq!(read["enabled"], true, "{read}");
    assert_eq!(read["restart_required"], false, "{read}");

    let response = turn_off(&daemon).await;

    assert_eq!(response.status(), StatusCode::OK);
    let read: serde_json::Value = response.json().await.unwrap();
    assert_eq!(read["enabled"], false, "{read}");
    assert_eq!(read["public_origin"], serde_json::Value::Null);
    assert_eq!(read["restart_required"], true);
    assert_eq!(read["tailscale"]["port_443"]["serves"], "nothing");
    assert_eq!(read["tailscale"]["port_8443"]["serves"], "nothing");
    assert_eq!(tailscale.changes(), [funnel_change("off", &daemon)]);
    let config = config_file(&daemon);
    assert!(!config.remote_access.enabled);
    assert_eq!(config.public_origin, "");
    assert_eq!(config.trusted_proxy, "");
    assert!(config.bind_address().unwrap().is_loopback());
    assert!(!daemon.restart.is_asked());
}

/// Turning off while a turn-on waits for the approval stops it: nothing
/// is written, also when the approval comes later, and a daemon that runs
/// with Remote Access off needs no restart.
#[tokio::test]
async fn turning_off_stops_a_turn_on_that_waits() {
    let tailscale = Arc::new(FakeTailscale::funnel_off());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        tailscale: Arc::clone(&tailscale) as _,
        ..TestDaemonOptions::default()
    })
    .await;
    turn_on(&daemon).await;
    waiting_at_the_page(&daemon).await;

    let response = turn_off(&daemon).await;

    assert_eq!(response.status(), StatusCode::OK);
    let stopped: serde_json::Value = response.json().await.unwrap();
    assert_eq!(stopped["turning_on"], serde_json::Value::Null, "{stopped}");
    assert_eq!(stopped["enabled"], false);
    assert_eq!(stopped["restart_required"], false);
    tailscale.approve();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(read(&daemon).await["enabled"], false);
    assert!(!config_file(&daemon).remote_access.enabled);
}

/// A Server is not switched in the Administration Interface: its
/// deployment sets `PAGIS_REMOTE_ACCESS`. The view shows what it runs
/// with and no Tailscale, and both routes refuse.
#[tokio::test]
async fn a_server_shows_remote_access_and_refuses_the_switch() {
    let tailscale = Arc::new(FakeTailscale::ready(
        FunnelPort::Nothing,
        FunnelPort::Nothing,
    ));
    let Some(daemon) = TestDaemon::start_on_postgres_with(TestDaemonOptions {
        public_origin: "https://pagis.example.net".to_string(),
        trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
        tailscale: Arc::clone(&tailscale) as _,
        ..TestDaemonOptions::default()
    })
    .await
    else {
        return;
    };

    assert_eq!(
        read(&daemon).await,
        serde_json::json!({
            "enabled": false,
            "restart_required": false,
            "public_origin": null,
            "switchable": false,
            "tailscale": null,
            "turning_on": null,
            "failure": null,
        })
    );
    let before = raw_config(&daemon);

    for response in [turn_on(&daemon).await, turn_off(&daemon).await] {
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let refused: serde_json::Value = response.json().await.unwrap();
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .contains("PAGIS_REMOTE_ACCESS"),
            "{refused}"
        );
    }
    assert_eq!(raw_config(&daemon), before);
    assert!(tailscale.changes().is_empty());
}

// --- who reaches a Local Installation -----------------------------------

/// With Remote Access off, a Local Installation answers only programs of
/// its own machine: a Funnel that still runs reaches nothing, on both
/// ports and for the pages too, also with the Session of a Member. The
/// owner at the machine still reaches everything.
#[tokio::test]
async fn with_remote_access_off_a_local_installation_refuses_another_machine() {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        public_origin: TAILNET_ORIGIN.to_string(),
        trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
        ..TestDaemonOptions::default()
    })
    .await;
    let member = member(&daemon, "grace@example.com", "a good password").await;
    let cookie = daemon.cookie_for(&member.id).await;

    for path in ["/api/v1/channels", "/api/v1/user", "/"] {
        let response =
            through_the_funnel(reqwest::Method::GET, format!("{}{path}", daemon.base_url))
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
    let administration = through_the_funnel(
        reqwest::Method::GET,
        format!("{}/api/v1/user", daemon.administration_base_url),
    )
    .header("cookie", daemon.cookie())
    .send()
    .await
    .unwrap();
    assert_eq!(administration.status(), StatusCode::FORBIDDEN);

    let owner = client()
        .get(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(owner.status(), StatusCode::OK);
}

/// With Remote Access on, the same request through the Funnel reaches
/// the Product App.
#[tokio::test]
async fn with_remote_access_on_a_local_installation_answers_another_machine() {
    let daemon = TestDaemon::start_with(in_remote_access(Arc::new(FakeTailscale::ready(
        FunnelPort::Pagis,
        FunnelPort::Pagis,
    ))))
    .await;
    let member = member(&daemon, "grace@example.com", "a good password").await;
    let cookie = daemon.cookie_for(&member.id).await;

    let response = through_the_funnel(
        reqwest::Method::GET,
        format!("{}/api/v1/channels", daemon.base_url),
    )
    .header("cookie", &cookie)
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

// --- no password from another machine ------------------------------------

/// In Remote Access a password from another machine is refused before it
/// is checked, and the refusal names the Sign-In Link. The same password
/// from this machine signs in, and a Sign-In Link signs another machine
/// in through the Funnel.
#[tokio::test]
async fn in_remote_access_a_password_from_another_machine_is_refused() {
    let daemon = TestDaemon::start_with(in_remote_access(Arc::new(FakeTailscale::ready(
        FunnelPort::Pagis,
        FunnelPort::Pagis,
    ))))
    .await;
    let member = member(&daemon, "grace@example.com", "a good password").await;
    let credentials = serde_json::json!({
        "email": "grace@example.com",
        "password": "a good password",
    });

    let refused = through_the_funnel(
        reqwest::Method::POST,
        format!("{}/api/v1/sessions", daemon.base_url),
    )
    .json(&credentials)
    .send()
    .await
    .unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert!(!sets_a_session(&refused));
    let refused: serde_json::Value = refused.json().await.unwrap();
    let message = refused["error"]["message"].as_str().unwrap();
    assert!(message.contains("Sign-In Link"), "{message}");
    assert!(message.contains("pagis pair"), "{message}");

    let here = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&credentials)
        .send()
        .await
        .unwrap();
    assert_eq!(here.status(), StatusCode::OK);
    assert!(sets_a_session(&here));

    let link = pagis_server::mint_public_origin_link(
        daemon.stores().sign_in_links.as_ref(),
        &member.id,
        &daemon.public_origin,
        pagis_core::CLIENT_LINK_LIFETIME_MS,
        now_ms(),
    )
    .await
    .unwrap();
    assert!(
        link.url.starts_with(&format!("{TAILNET_ORIGIN}/sign-in#")),
        "{}",
        link.url
    );
    let secret = link.url.split_once('#').unwrap().1;
    let traded = through_the_funnel(
        reqwest::Method::POST,
        format!("{}/api/v1/sessions/link", daemon.base_url),
    )
    .json(&serde_json::json!({ "secret": secret }))
    .send()
    .await
    .unwrap();
    assert_eq!(traded.status(), StatusCode::OK);
    let cookie = traded
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(cookie.contains("; Secure"), "{cookie}");
}

/// The sign-in page reads from the health answer which form to show: a
/// browser on another machine in Remote Access pastes a Sign-In Link, and
/// a browser on this machine keeps the password.
#[tokio::test]
async fn the_health_answer_names_how_the_browser_that_asks_signs_in() {
    let daemon = TestDaemon::start_with(in_remote_access(Arc::new(FakeTailscale::ready(
        FunnelPort::Pagis,
        FunnelPort::Pagis,
    ))))
    .await;
    let health = |request: reqwest::RequestBuilder| async move {
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.json::<serde_json::Value>().await.unwrap()["sign_in"].clone()
    };
    let url = format!("{}/api/v1/health", daemon.base_url);

    assert_eq!(
        health(through_the_funnel(reqwest::Method::GET, url.clone())).await,
        "link"
    );
    assert_eq!(health(client().get(&url)).await, "password");

    let off = TestDaemon::start().await;
    assert_eq!(
        health(client().get(format!("{}/api/v1/health", off.base_url))).await,
        "password"
    );
}

/// With Remote Access off, a Local Installation keeps the password for
/// the People of its own machine.
#[tokio::test]
async fn with_remote_access_off_a_password_from_this_machine_signs_in() {
    let daemon = TestDaemon::start().await;
    member(&daemon, "grace@example.com", "a good password").await;

    let response = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "password": "a good password",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

// --- the live screen through the Funnel ----------------------------------

/// The URL of the TURN server of the fake tailnet: `turns:` the name of
/// the machine, port 8443 of the Funnel, over TCP.
const TURN_URL: &str = "turns:owner-mac.tail1234.ts.net:8443?transport=tcp";

/// The ICE servers that the screen route gives to `request`.
pub(crate) async fn ice_servers(request: reqwest::RequestBuilder) -> Vec<serde_json::Value> {
    let response = request.send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    body["ice_servers"].as_array().expect("a list").clone()
}

/// In Remote Access a browser on another machine gets the TURN server of
/// the live screen, with a credential of its own for each answer. A
/// browser on this machine gets none, and keeps its direct path to the
/// Media Relay.
#[tokio::test]
async fn in_remote_access_a_browser_on_another_machine_gets_the_turn_server() {
    let daemon = TestDaemon::start_with(in_remote_access(Arc::new(FakeTailscale::ready(
        FunnelPort::Pagis,
        FunnelPort::Pagis,
    ))))
    .await;
    let member = member(&daemon, "grace@example.com", "a good password").await;
    let cookie = daemon.cookie_for(&member.id).await;
    let url = format!("{}/api/v1/screen/ice", daemon.base_url);

    let first = ice_servers(
        through_the_funnel(reqwest::Method::GET, url.clone()).header("cookie", &cookie),
    )
    .await;
    let second = ice_servers(
        through_the_funnel(reqwest::Method::GET, url.clone()).header("cookie", &cookie),
    )
    .await;

    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0]["urls"], serde_json::json!([TURN_URL]));
    let username = first[0]["username"].as_str().unwrap();
    let expiry: i64 = username
        .split_once(':')
        .expect("an expiry and a session")
        .0
        .parse()
        .expect("the expiry is Unix seconds");
    assert!(expiry > now_ms() / 1000 + 3600, "{username}");
    assert!(!first[0]["credential"].as_str().unwrap().is_empty());
    assert_ne!(first[0]["username"], second[0]["username"]);

    let here = ice_servers(client().get(&url).header("cookie", daemon.cookie())).await;
    assert!(here.is_empty(), "{here:?}");
}

/// With Remote Access off the daemon runs no TURN server: nothing listens
/// on its port, and the screen route names none.
#[tokio::test]
async fn with_remote_access_off_the_daemon_runs_no_turn_server() {
    let daemon = TestDaemon::start().await;

    let here = ice_servers(
        client()
            .get(format!("{}/api/v1/screen/ice", daemon.base_url))
            .header("cookie", daemon.cookie()),
    )
    .await;

    assert!(here.is_empty(), "{here:?}");
    assert!(
        tokio::net::TcpStream::connect(daemon.remote_access_turn_addr)
            .await
            .is_err()
    );
}

/// A browser on another machine reaches the Media Relay through the TURN
/// port of the daemon, where `tailscaled` forwards it once it has ended
/// TLS: it allocates over TCP with the credential of the screen route, and
/// a datagram goes to the Media Relay's range and back. Another port of
/// loopback gets nothing.
#[tokio::test]
async fn a_browser_on_another_machine_relays_to_the_media_relay_through_the_turn_port() {
    let media = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let media_address = media.local_addr().unwrap();
    let other = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        screen: pagis_server::ScreenRelay {
            relay: pagis_server::MediaRelayKind::Daemon,
            advertise_ip: "127.0.0.1".to_string(),
            media_ports: media_address.port()..=media_address.port(),
        },
        ..in_remote_access(Arc::new(FakeTailscale::ready(
            FunnelPort::Pagis,
            FunnelPort::Pagis,
        )))
    })
    .await;
    let member = member(&daemon, "grace@example.com", "a good password").await;
    let cookie = daemon.cookie_for(&member.id).await;
    let ice = ice_servers(
        through_the_funnel(
            reqwest::Method::GET,
            format!("{}/api/v1/screen/ice", daemon.base_url),
        )
        .header("cookie", &cookie),
    )
    .await;
    let browser = pagis_computer::fake::TurnClient::allocate(
        daemon.remote_access_turn_addr,
        ice[0]["username"].as_str().unwrap(),
        ice[0]["credential"].as_str().unwrap(),
    )
    .await
    .expect("the browser allocates with the credential of the screen route");

    browser
        .send_to(b"to another port", other.local_addr().unwrap())
        .await
        .unwrap();
    browser
        .send_to(b"an ICE check of the browser", media_address)
        .await
        .unwrap();

    let mut buffer = [0u8; 2048];
    let (read, relay_side) =
        tokio::time::timeout(Duration::from_secs(5), media.recv_from(&mut buffer))
            .await
            .expect("the datagram reached the Media Relay")
            .unwrap();
    assert_eq!(&buffer[..read], b"an ICE check of the browser");
    assert_eq!(relay_side, browser.relayed_address());
    media
        .send_to(b"the answer of the pipeline", relay_side)
        .await
        .unwrap();
    let (answer, from) = tokio::time::timeout(Duration::from_secs(5), browser.recv_from())
        .await
        .expect("the answer came back")
        .unwrap();
    assert_eq!(answer, b"the answer of the pipeline");
    assert_eq!(from, media_address);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), other.recv_from(&mut buffer))
            .await
            .is_err(),
        "another port of loopback got a datagram"
    );
}
