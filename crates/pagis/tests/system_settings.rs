//! System Settings, Docker discovery and the restart switch
//! (ADR-0024). The user changes a System Setting here, the daemon
//! validates it and writes the config file, and nobody edits a file.

use std::sync::Arc;

use pagis_computer::{DockerDiscovery, DockerSearch};
use pagis_testkit::{ScriptedDockerPing, TestDaemon, TestDaemonOptions, empty_docker_search};
use reqwest::StatusCode;

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn settings(daemon: &TestDaemon) -> serde_json::Value {
    let response = client()
        .get(format!(
            "{}/api/v1/settings/system",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

async fn save(daemon: &TestDaemon, body: serde_json::Value) -> reqwest::Response {
    client()
        .put(format!(
            "{}/api/v1/settings/system",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap()
}

/// The daemon says what it runs with, beside what the file holds: the
/// port it listens on, whether a supervisor starts it again after a
/// restart, and when this process started, which a restart changes.
#[tokio::test]
async fn the_system_settings_say_how_the_daemon_runs() {
    let daemon = TestDaemon::start().await;

    let read = settings(&daemon).await;

    let listening: u16 = daemon.base_url.rsplit(':').next().unwrap().parse().unwrap();
    assert_eq!(read["listening_port"], listening);
    // No flag and no variable sets the port of the test daemon.
    assert_eq!(read["port_override"], serde_json::Value::Null);
    assert_eq!(read["supervised"], false);
    assert!(read["started_at"].as_i64().unwrap() > 0, "{read}");
}

#[tokio::test]
async fn a_supervised_daemon_says_so() {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        supervised: true,
        ..TestDaemonOptions::default()
    })
    .await;

    assert_eq!(settings(&daemon).await["supervised"], true);
}

/// A home with one Colima socket per profile, which a ping never
/// opens.
fn fake_home(profiles: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for profile in profiles {
        let path = dir.path().join(".colima").join(profile);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("docker.sock"), "").unwrap();
    }
    dir
}

fn colima_endpoint(dir: &tempfile::TempDir, profile: &str) -> String {
    format!(
        "unix://{}/.colima/{profile}/docker.sock",
        dir.path().display()
    )
}

/// A daemon whose Docker discovery searches `home` and answers for the
/// endpoints given.
async fn daemon_searching(home: &tempfile::TempDir, answering: &[String]) -> TestDaemon {
    let search = DockerSearch {
        home: home.path().to_path_buf(),
        ..empty_docker_search()
    };
    TestDaemon::start_with(TestDaemonOptions {
        docker_discovery: Arc::new(DockerDiscovery::new(
            search,
            Arc::new(ScriptedDockerPing(answering.to_vec())),
            None,
        )),
        ..TestDaemonOptions::default()
    })
    .await
}

#[tokio::test]
async fn the_system_settings_read_back_with_the_data_directory_and_the_version() {
    let daemon = TestDaemon::start().await;

    let settings = settings(&daemon).await;

    assert_eq!(settings["port"], 4400);
    assert_eq!(settings["log_level"], "info");
    assert_eq!(settings["docker_endpoint"], serde_json::Value::Null);
    assert_eq!(
        settings["data_directory"],
        daemon.booted.home.display().to_string()
    );
    assert_eq!(settings["version"], env!("CARGO_PKG_VERSION"));
    // No Docker in a test, so no endpoint and no candidate.
    assert_eq!(settings["docker"]["endpoint"], serde_json::Value::Null);
    assert_eq!(settings["docker"]["candidates"], serde_json::json!([]));
}

#[tokio::test]
async fn a_saved_setting_reaches_the_config_file_and_says_a_restart_is_needed() {
    let daemon = TestDaemon::start().await;

    let response = save(
        &daemon,
        serde_json::json!({
            "port": 4500,
            "docker_endpoint": null,
            "log_level": "debug",
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let saved: serde_json::Value = response.json().await.unwrap();
    assert_eq!(saved["restart_required"], true);
    assert_eq!(saved["settings"]["port"], 4500);

    let file = std::fs::read_to_string(daemon.booted.home.join("config.toml")).unwrap();
    assert!(file.contains("port = 4500"));
    assert!(file.contains("log_level = \"debug\""));
    // The read is the same after the write.
    let settings = settings(&daemon).await;
    assert_eq!(settings["port"], 4500);
    assert_eq!(settings["log_level"], "debug");
}

#[tokio::test]
async fn a_setting_that_is_not_usable_is_refused_and_nothing_is_written() {
    let daemon = TestDaemon::start().await;
    let before = settings(&daemon).await;

    for (body, reason) in [
        (
            serde_json::json!({"port": 80, "docker_endpoint": null, "log_level": "info"}),
            "port",
        ),
        (
            serde_json::json!({"port": 4400, "docker_endpoint": null, "log_level": "chatty"}),
            "log level",
        ),
        (
            serde_json::json!({"port": 4400, "docker_endpoint": "docker.sock", "log_level": "info"}),
            "endpoint",
        ),
    ] {
        let response = save(&daemon, body).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{reason} must be refused"
        );
    }

    assert_eq!(settings(&daemon).await, before);
}

#[tokio::test]
async fn a_docker_override_is_pinged_before_it_is_saved() {
    let home = fake_home(&["default"]);
    let daemon = daemon_searching(&home, &[]).await;
    let endpoint = colima_endpoint(&home, "default");

    let response = save(
        &daemon,
        serde_json::json!({
            "port": 4400,
            "docker_endpoint": endpoint,
            "log_level": "info",
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Docker did not answer"),
        "{body}"
    );
    assert_eq!(
        settings(&daemon).await["docker_endpoint"],
        serde_json::Value::Null
    );
}

#[tokio::test]
async fn an_override_that_answers_becomes_the_endpoint_in_use() {
    let home = fake_home(&["default", "work"]);
    let work = colima_endpoint(&home, "work");
    // Only the second profile answers, so the override is the reason
    // the daemon uses it.
    let daemon = daemon_searching(&home, std::slice::from_ref(&work)).await;

    let saved: serde_json::Value = save(
        &daemon,
        serde_json::json!({
            "port": 4400,
            "docker_endpoint": work,
            "log_level": "info",
        }),
    )
    .await
    .json()
    .await
    .unwrap();

    assert_eq!(saved["restart_required"], false);
    assert_eq!(saved["settings"]["docker_endpoint"], work);
    assert_eq!(saved["settings"]["docker"]["endpoint"], work);
    assert_eq!(
        saved["settings"]["docker"]["candidates"][0]["source"],
        "override"
    );
    assert_eq!(
        saved["settings"]["docker"]["candidates"][0]["reachable"],
        true
    );
}

#[tokio::test]
async fn probe_again_lists_every_candidate_with_its_result() {
    let home = fake_home(&["default", "work"]);
    let daemon = daemon_searching(&home, &[colima_endpoint(&home, "work")]).await;

    let report: serde_json::Value = client()
        .post(format!(
            "{}/api/v1/settings/system/docker/probe",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(report["endpoint"], colima_endpoint(&home, "work"));
    let candidates = report["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0]["endpoint"], colima_endpoint(&home, "default"));
    assert_eq!(candidates[0]["reachable"], false);
    assert_eq!(candidates[0]["error"], "connection refused");
    assert_eq!(candidates[1]["reachable"], true);
}

#[tokio::test]
async fn the_restart_endpoint_names_the_reserved_code_and_asks_the_daemon() {
    let daemon = TestDaemon::start().await;
    assert!(!daemon.restart.is_asked());

    let body: serde_json::Value = client()
        .post(format!(
            "{}/api/v1/system/restart",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["exit_code"], pagis_server::RESTART_EXIT_CODE);
    assert!(daemon.restart.is_asked());
}

#[tokio::test]
async fn the_system_settings_need_a_session() {
    let daemon = TestDaemon::start().await;

    for response in [
        client()
            .get(format!(
                "{}/api/v1/settings/system",
                daemon.administration_base_url
            ))
            .send()
            .await
            .unwrap(),
        client()
            .post(format!(
                "{}/api/v1/system/restart",
                daemon.administration_base_url
            ))
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert!(!daemon.restart.is_asked());
}

/// A Member in the same Org, with their own Workspace.
async fn member(pool: &sqlx::SqlitePool) -> pagis_core::User {
    use pagis_core::{OrgStore, UserStore, WorkspaceStore, now_ms};

    let now = now_ms();
    let org = pagis_storage_sqlite::SqliteOrgStore::new(pool.clone())
        .list()
        .await
        .expect("list the orgs")
        .into_iter()
        .next()
        .expect("the seeded org");
    let person = pagis_core::User {
        email: Some("member@example.com".to_string()),
        name: Some("Mabel".to_string()),
        ..pagis_core::User::new(org.id, pagis_core::UserRole::Member, now)
    };
    pagis_storage_sqlite::SqliteUserStore::new(pool.clone())
        .create(&person)
        .await
        .expect("create the person");
    let workspace = pagis_core::Workspace {
        id: pagis_core::WorkspaceId::generate(),
        user_id: person.id.clone(),
        name: "Mabel".to_string(),
        timezone: "UTC".to_string(),
        created_at: now,
        onboarded_at: Some(now),
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
    };
    pagis_storage_sqlite::SqliteWorkspaceStore::new(pool.clone())
        .create(&workspace)
        .await
        .expect("create the workspace");
    person
}

/// A System Setting belongs to the installation, so only an
/// administrator reads or writes one. The Docker endpoint is the sharpest
/// case: a Member who could repoint it would repoint it for everybody.
#[tokio::test]
async fn a_member_reaches_no_system_setting_and_an_administrator_does() {
    let daemon = TestDaemon::start().await;
    let member = member(daemon.pool()).await;
    let cookie = daemon.cookie_for(&member.id).await;

    // The seeded person is the administrator of the installation.
    let current = settings(&daemon).await;
    assert_eq!(
        save(
            &daemon,
            serde_json::json!({
                "port": current["port"],
                "docker_endpoint": serde_json::Value::Null,
                "log_level": "info",
            }),
        )
        .await
        .status(),
        StatusCode::OK
    );

    // The Member reads nothing and writes nothing.
    let read = client()
        .get(format!(
            "{}/api/v1/settings/system",
            daemon.administration_base_url
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(read.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = read.json().await.unwrap();
    assert_eq!(body["error"]["code"], "forbidden");

    let written = client()
        .put(format!(
            "{}/api/v1/settings/system",
            daemon.administration_base_url
        ))
        .header("cookie", &cookie)
        .json(&serde_json::json!({
            "port": 4400,
            "docker_endpoint": "unix:///tmp/theirs.sock",
            "log_level": "info",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(written.status(), StatusCode::FORBIDDEN);

    for path in [
        "/api/v1/settings/system/docker/probe",
        "/api/v1/system/restart",
    ] {
        let response = client()
            .post(format!("{}{path}", daemon.administration_base_url))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

// --- the multi-user mode ------------------------------------------------

/// The origin the owner's proxy or tunnel answers on.
const OWNERS_ORIGIN: &str = "https://pagis.owner.example";

async fn enable_multi_user(daemon: &TestDaemon, body: serde_json::Value) -> reqwest::Response {
    client()
        .put(format!(
            "{}/api/v1/settings/system/multi-user",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&body)
        .send()
        .await
        .unwrap()
}

async fn disable_multi_user(daemon: &TestDaemon) -> reqwest::Response {
    client()
        .delete(format!(
            "{}/api/v1/settings/system/multi-user",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
}

fn config_file(daemon: &TestDaemon) -> pagis::Config {
    pagis::Config::read_file(&daemon.booted.home.join("config.toml")).unwrap()
}

/// A local installation serves its own machine until an Administrator
/// turns the mode on, and the Settings view says it can.
#[tokio::test]
async fn a_local_installation_starts_off_and_can_switch() {
    let daemon = TestDaemon::start().await;

    let settings = settings(&daemon).await;

    assert_eq!(
        settings["multi_user"],
        serde_json::json!({
            "enabled": false,
            "public_origin": null,
            "trusted_proxy": null,
            "switchable": true,
        })
    );
}

/// Turning the mode on writes the Public Origin and the Trusted Proxy,
/// keeps the Bind Address on loopback, and offers the restart that puts
/// them in effect.
#[tokio::test]
async fn switching_on_writes_the_origin_and_the_proxy_and_offers_a_restart() {
    let daemon = TestDaemon::start().await;

    let response = enable_multi_user(
        &daemon,
        serde_json::json!({
            "public_origin": "https://Pagis.Owner.example/",
            "trusted_proxy": "127.0.0.1",
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let saved: serde_json::Value = response.json().await.unwrap();
    assert_eq!(saved["restart_required"], true);
    assert_eq!(
        saved["settings"]["multi_user"],
        serde_json::json!({
            "enabled": true,
            "public_origin": OWNERS_ORIGIN,
            "trusted_proxy": "127.0.0.1",
            "switchable": true,
        })
    );
    let config = config_file(&daemon);
    assert_eq!(config.public_origin, OWNERS_ORIGIN);
    assert_eq!(config.trusted_proxy, "127.0.0.1");
    assert!(config.bind_address().unwrap().is_loopback());
    // The read is the same after the write.
    assert_eq!(settings(&daemon).await["multi_user"]["enabled"], true);
    // Nothing is restarted until the Administrator asks.
    assert!(!daemon.restart.is_asked());
}

/// The Trusted Proxy is optional.
#[tokio::test]
async fn switching_on_with_no_proxy_believes_no_forwarded_header() {
    let daemon = TestDaemon::start().await;

    let response = enable_multi_user(
        &daemon,
        serde_json::json!({ "public_origin": OWNERS_ORIGIN, "trusted_proxy": "" }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let saved: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        saved["settings"]["multi_user"]["trusted_proxy"],
        serde_json::Value::Null
    );
    assert_eq!(config_file(&daemon).trusted_proxy, "");
}

/// An origin people on other machines cannot open, or a proxy that is
/// not an address, is refused with a sentence that says what to type,
/// and the file does not change.
#[tokio::test]
async fn switching_on_refuses_an_origin_or_a_proxy_that_does_not_work() {
    let daemon = TestDaemon::start().await;
    let before = std::fs::read_to_string(daemon.booted.home.join("config.toml")).unwrap();

    for body in [
        serde_json::json!({ "public_origin": "pagis.owner.example" }),
        serde_json::json!({ "public_origin": "ftp://pagis.owner.example" }),
        serde_json::json!({ "public_origin": "https://pagis.owner.example/pagis" }),
        serde_json::json!({ "public_origin": "http://localhost:4400" }),
        serde_json::json!({ "public_origin": "http://127.0.0.1:4400" }),
        serde_json::json!({ "public_origin": OWNERS_ORIGIN, "trusted_proxy": "the proxy" }),
    ] {
        let response = enable_multi_user(&daemon, body.clone()).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}"
        );
        let refused: serde_json::Value = response.json().await.unwrap();
        assert_eq!(refused["error"]["code"], "validation", "{body}");
    }

    let after = std::fs::read_to_string(daemon.booted.home.join("config.toml")).unwrap();
    assert_eq!(after, before);
    assert_eq!(settings(&daemon).await["multi_user"]["enabled"], false);
}

/// Turning the mode off clears the Public Origin and the Trusted Proxy
/// and binds loopback. A daemon that runs in the mode needs a restart
/// to leave it.
#[tokio::test]
async fn switching_off_clears_the_origin_and_the_proxy() {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        public_origin: OWNERS_ORIGIN.to_string(),
        trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
        ..TestDaemonOptions::default()
    })
    .await;
    assert_eq!(settings(&daemon).await["multi_user"]["enabled"], true);

    let response = disable_multi_user(&daemon).await;

    assert_eq!(response.status(), StatusCode::OK);
    let saved: serde_json::Value = response.json().await.unwrap();
    assert_eq!(saved["restart_required"], true);
    assert_eq!(saved["settings"]["multi_user"]["enabled"], false);
    assert_eq!(
        saved["settings"]["multi_user"]["public_origin"],
        serde_json::Value::Null
    );
    let config = config_file(&daemon);
    assert_eq!(config.public_origin, "");
    assert_eq!(config.trusted_proxy, "");
    assert!(config.bind_address().unwrap().is_loopback());
}

/// A request that the owner's proxy forwards, as Caddy writes it.
async fn through_the_proxy(daemon: &TestDaemon, cookie: &str, path: &str) -> reqwest::Response {
    client()
        .get(format!("{}{path}", daemon.base_url))
        .header("host", "pagis.owner.example")
        .header("x-forwarded-for", "203.0.113.9")
        .header("x-forwarded-proto", "https")
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
}

/// With the mode off, a Member's Session reaches nothing through a
/// proxy that still runs: the daemon refuses every request that a proxy
/// forwarded, on both ports and for the pages too. The owner at the
/// machine still reaches everything.
#[tokio::test]
async fn with_the_mode_off_a_proxy_that_still_runs_reaches_nothing() {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        public_origin: OWNERS_ORIGIN.to_string(),
        trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
        ..TestDaemonOptions::default()
    })
    .await;
    let member = member(daemon.pool()).await;
    let cookie = daemon.cookie_for(&member.id).await;
    let before = through_the_proxy(&daemon, &cookie, "/api/v1/channels").await;
    assert_eq!(before.status(), StatusCode::OK);

    assert_eq!(disable_multi_user(&daemon).await.status(), StatusCode::OK);
    let daemon = daemon.restart(TestDaemonOptions::default()).await;

    for path in ["/api/v1/channels", "/api/v1/user", "/"] {
        let after = through_the_proxy(&daemon, &cookie, path).await;
        assert_eq!(after.status(), StatusCode::FORBIDDEN, "{path}");
    }
    let administration = client()
        .get(format!("{}/api/v1/system", daemon.administration_base_url))
        .header("x-forwarded-for", "203.0.113.9")
        .header("cookie", &cookie)
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

/// A switch back to what the daemon runs with needs no restart.
#[tokio::test]
async fn switching_back_to_the_running_mode_needs_no_restart() {
    let daemon = TestDaemon::start().await;
    enable_multi_user(
        &daemon,
        serde_json::json!({ "public_origin": OWNERS_ORIGIN }),
    )
    .await;

    let saved: serde_json::Value = disable_multi_user(&daemon).await.json().await.unwrap();

    assert_eq!(saved["restart_required"], false);
}

/// A server always serves a network. The Settings view shows the mode
/// it runs with and no switch, and the routes refuse a switch.
#[tokio::test]
async fn a_server_is_always_multi_user_and_refuses_the_switch() {
    let Some(daemon) = TestDaemon::start_on_postgres_with(TestDaemonOptions {
        public_origin: "https://pagis.example.net".to_string(),
        trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
        ..TestDaemonOptions::default()
    })
    .await
    else {
        return;
    };

    assert_eq!(
        settings(&daemon).await["multi_user"],
        serde_json::json!({
            "enabled": true,
            "public_origin": "https://pagis.example.net",
            "trusted_proxy": "127.0.0.1",
            "switchable": false,
        })
    );
    let before = std::fs::read_to_string(daemon.booted.home.join("config.toml")).unwrap();

    let on = enable_multi_user(
        &daemon,
        serde_json::json!({ "public_origin": "https://other.example.net" }),
    )
    .await;
    assert_eq!(on.status(), StatusCode::CONFLICT);
    let off = disable_multi_user(&daemon).await;
    assert_eq!(off.status(), StatusCode::CONFLICT);
    let refused: serde_json::Value = off.json().await.unwrap();
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("PAGIS_PUBLIC_ORIGIN"),
        "{refused}"
    );

    let after = std::fs::read_to_string(daemon.booted.home.join("config.toml")).unwrap();
    assert_eq!(after, before);
}

// --- the live screen ----------------------------------------------------

/// The Media Relay of a daemon on its default `[screen]` section.
fn default_screen() -> pagis_server::ScreenRelay {
    pagis::Screen::default()
        .screen_relay()
        .expect("the default screen section")
}

/// The live screen does not go through the proxy or tunnel, so the
/// Settings view names where the Media Relay of the running daemon
/// answers. A local installation that configures nothing advertises
/// loopback.
#[tokio::test]
async fn a_local_installation_names_the_media_relay_on_loopback() {
    let daemon = TestDaemon::start().await;

    assert_eq!(
        settings(&daemon).await["screen"],
        serde_json::json!({
            "relay": "daemon",
            "advertise_ip": "127.0.0.1",
            "loopback": true,
            "media_port_first": 50000,
            "media_port_last": 50099,
        })
    );
}

/// An address on the LAN is not loopback, and the view names it with
/// the UDP range that the firewall lets through.
#[tokio::test]
async fn a_local_installation_names_the_lan_address_it_advertises() {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        screen: pagis_server::ScreenRelay {
            advertise_ip: "192.168.86.55".to_string(),
            ..default_screen()
        },
        ..TestDaemonOptions::default()
    })
    .await;

    assert_eq!(
        settings(&daemon).await["screen"],
        serde_json::json!({
            "relay": "daemon",
            "advertise_ip": "192.168.86.55",
            "loopback": false,
            "media_port_first": 50000,
            "media_port_last": 50099,
        })
    );
}

/// A server names the Media Relay it runs with, which its deployment
/// sets in the environment and not in the file.
#[tokio::test]
async fn a_server_names_the_media_relay_it_runs_with() {
    let Some(daemon) = TestDaemon::start_on_postgres_with(TestDaemonOptions {
        public_origin: "https://pagis.example.net".to_string(),
        screen: pagis_server::ScreenRelay {
            relay: pagis_server::MediaRelayKind::Turn,
            advertise_ip: "10.0.1.7".to_string(),
            media_ports: 50000..=50019,
        },
        ..TestDaemonOptions::default()
    })
    .await
    else {
        return;
    };

    assert_eq!(
        settings(&daemon).await["screen"],
        serde_json::json!({
            "relay": "turn",
            "advertise_ip": "10.0.1.7",
            "loopback": false,
            "media_port_first": 50000,
            "media_port_last": 50019,
        })
    );
}
