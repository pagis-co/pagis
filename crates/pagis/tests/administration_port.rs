//! The Administration Interface on its own port.
//!
//! The port is the installation's own. Every route on it requires a
//! signed-in Administrator by construction: the guard is a layer of the
//! router, so this file drives every row of
//! `pagis_server::ADMINISTRATION_ROUTES` rather than trusting each
//! handler to take the right extractor. A Member gets `403` and a
//! stranger `401`, on every route and every method.
//!
//! The documented exceptions are the first-run setup and the password
//! sign-in, and both states of the setup are tested: it answers on a
//! server nobody can sign in to, and `410 Gone` once somebody can.

use pagis_core::UserRole;
use pagis_server::{ADMINISTRATION_PUBLIC_ROUTES, ADMINISTRATION_ROUTES, ROUTES, SHARED_ROUTES};
use pagis_testkit::TestDaemon;
use reqwest::StatusCode;

const PASSWORD: &str = "correct horse battery";

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// One path of the table with its parameters filled in, so a request
/// reaches the route the row names.
fn path_of(route: &pagis_server::Route, user_id: &str) -> String {
    route
        .path
        .replace("{user_id}", user_id)
        .replace("{provider}", "anthropic")
        .replace("{plugin_id}", "plg_none")
        .replace("{field}", "none")
        .replace("{part}", "key")
}

fn request(method: &str, url: &str) -> reqwest::RequestBuilder {
    let client = client();
    match method {
        "get" => client.get(url),
        "post" => client.post(url),
        "put" => client.put(url),
        "delete" => client.delete(url),
        other => panic!("the table names no {other} route"),
    }
}

/// A Member of the installation, made through the route an
/// Administrator uses, and the `Cookie` header of their Session.
async fn member(daemon: &TestDaemon) -> (String, String) {
    let response = client()
        .post(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "name": "Grace",
            "password": PASSWORD,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let person: serde_json::Value = response.json().await.unwrap();
    let id = person["id"].as_str().unwrap().to_string();
    let cookie = sign_in(&daemon.base_url, "grace@example.com", PASSWORD)
        .await
        .expect("the member signs in");
    (id, cookie)
}

/// Sign in with a password at one origin and answer the `Cookie` header
/// value of the Session.
async fn sign_in(base_url: &str, email: &str, password: &str) -> Option<String> {
    let response = client()
        .post(format!("{base_url}/api/v1/sessions"))
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()
        .await
        .unwrap();
    if response.status() != StatusCode::OK {
        return None;
    }
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("pagis_session="))
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
}

/// Every guarded route of the administration port answers `401` without
/// a Session and `403` to a Member. The guard is the router's, so the
/// table is the whole list and a new route cannot escape it.
#[tokio::test]
async fn every_guarded_route_refuses_a_stranger_and_a_member() {
    let daemon = TestDaemon::start().await;
    let (member_id, member_cookie) = member(&daemon).await;

    let guarded: Vec<_> = ADMINISTRATION_ROUTES
        .iter()
        .filter(|route| route.authenticated)
        .collect();
    assert!(guarded.len() > 10, "the table looks empty");

    for route in guarded {
        let url = format!(
            "{}{}",
            daemon.administration_base_url,
            path_of(route, &member_id)
        );
        for method in route.methods {
            let stranger = request(method, &url)
                .json(&serde_json::json!({}))
                .send()
                .await
                .unwrap();
            assert_eq!(
                stranger.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {} answered a request with no session",
                route.path
            );
            let forbidden = request(method, &url)
                .header("cookie", &member_cookie)
                .json(&serde_json::json!({}))
                .send()
                .await
                .unwrap();
            assert_eq!(
                forbidden.status(),
                StatusCode::FORBIDDEN,
                "{method} {} answered a member",
                route.path
            );
            // A member gets nothing, not a partial view: the answer is
            // the error shape and holds no record of anybody.
            let body: serde_json::Value = forbidden.json().await.unwrap();
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }
}

/// The port serves the installation and not one person's content: a
/// product route of the same daemon is not there at all.
#[tokio::test]
async fn the_product_surface_is_not_on_the_administration_port() {
    let daemon = TestDaemon::start().await;

    for path in [
        "/api/v1/channels",
        "/api/v1/agents",
        "/api/v1/runs",
        "/api/v1/memory/feed",
    ] {
        let response = client()
            .get(format!("{}{path}", daemon.administration_base_url))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} answered on the administration port"
        );
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "not_found");
    }
}

/// The installation answers on this port alone: a request to one of its
/// routes on the product port finds nothing there, even from a signed-in
/// Administrator. A path the product port does not hold answers `404`;
/// a path it holds for a Person's own read answers `405` to the method
/// that belongs to the installation.
#[tokio::test]
async fn the_administration_routes_are_not_on_the_product_port() {
    let daemon = TestDaemon::start().await;
    let (member_id, _) = member(&daemon).await;

    let mut not_found = 0;
    for route in ADMINISTRATION_ROUTES {
        let url = format!("{}{}", daemon.base_url, path_of(route, &member_id));
        let held = ROUTES.iter().find(|product| product.path == route.path);
        for method in route.methods {
            if SHARED_ROUTES
                .iter()
                .any(|(path, methods, _)| *path == route.path && methods.contains(method))
            {
                continue;
            }
            let response = request(method, &url)
                .header("cookie", daemon.cookie())
                .json(&serde_json::json!({}))
                .send()
                .await
                .unwrap();
            let expected = match held {
                None => StatusCode::NOT_FOUND,
                Some(_) => StatusCode::METHOD_NOT_ALLOWED,
            };
            assert_eq!(
                response.status(),
                expected,
                "{method} {} answered on the product port",
                route.path
            );
            if held.is_none() {
                not_found += 1;
            }
        }
    }
    assert!(
        not_found > 10,
        "the product port sweep reached too few routes"
    );
}

/// The product tells an Administrator where this port answers, so the
/// page can link to it, and tells a Member nothing of it.
#[tokio::test]
async fn the_product_names_this_port_to_an_administrator_alone() {
    let daemon = TestDaemon::start().await;
    let (_, member_cookie) = member(&daemon).await;
    let user = |cookie: String| {
        let url = format!("{}/api/v1/user", daemon.base_url);
        async move {
            client()
                .get(url)
                .header("cookie", cookie)
                .send()
                .await
                .unwrap()
                .json::<serde_json::Value>()
                .await
                .unwrap()
        }
    };

    let administrator = user(daemon.cookie().to_string()).await;
    assert_eq!(
        administrator["administration"]["origin"],
        daemon.administration_base_url
    );
    assert_eq!(administrator["administration"]["loopback"], true);

    let member = user(member_cookie).await;
    assert_eq!(member["administration"], serde_json::Value::Null);
}

/// The administration page is served at every address of the port, so a
/// bookmark of one of its views loads the page and not a 404.
#[tokio::test]
async fn the_administration_page_answers_at_every_address() {
    let daemon = TestDaemon::start().await;

    for path in ["/", "/people", "/resources"] {
        let response = client()
            .get(format!("{}{path}", daemon.administration_base_url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path} did not answer");
        let page = response.text().await.unwrap().to_lowercase();
        assert!(
            page.contains("pagis"),
            "{path} answered with something other than the page"
        );
    }
}

/// A Client App names its machine when it signs in, so the Sessions
/// view tells its Session from a browser's.
#[tokio::test]
async fn a_client_app_session_carries_its_machine_name() {
    let daemon = TestDaemon::start().await;
    let (member_id, _) = member(&daemon).await;
    let response = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "password": PASSWORD,
            "client_name": "grace-macbook",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let sessions: serde_json::Value = client()
        .get(format!(
            "{}/api/v1/administration/sessions",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let grace: Vec<&serde_json::Value> = sessions["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|session| session["person"]["id"] == member_id.as_str())
        .collect();
    let kinds: Vec<(&str, Option<&str>)> = grace
        .iter()
        .map(|session| {
            (
                session["client_kind"].as_str().unwrap(),
                session["client_name"].as_str(),
            )
        })
        .collect();
    assert!(
        kinds.contains(&("desktop", Some("grace-macbook"))),
        "{kinds:?}"
    );
    assert!(kinds.contains(&("browser", None)), "{kinds:?}");
}

/// What the interface shows, from the records of a running daemon: the
/// roster, the spend, the live Sessions, the resources per person and
/// the health of the daemon.
#[tokio::test]
async fn the_administrator_reads_the_installation_on_its_own_port() {
    let daemon = TestDaemon::start().await;
    let (member_id, _) = member(&daemon).await;
    let at = |path: &str| {
        client()
            .get(format!("{}{path}", daemon.administration_base_url))
            .header("cookie", daemon.cookie())
            .send()
    };

    let roster: serde_json::Value = at("/api/v1/administration/people")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = roster["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|person| person["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&member_id.as_str()));

    // The spend is per person and per period, and a period with no model
    // call is zero rather than absent.
    let usage: serde_json::Value = at("/api/v1/administration/usage")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(usage["items"].as_array().unwrap().len(), ids.len());
    assert!(usage["from"].as_i64().unwrap() < usage["to"].as_i64().unwrap());
    assert_eq!(usage["total"]["cost_usd"].as_f64().unwrap(), 0.0);

    // The Sessions are the ones open now: the Administrator's own, which
    // the list says is the one they are reading with, and the Member's.
    let sessions: serde_json::Value = at("/api/v1/administration/sessions")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = sessions["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{items:?}");
    let current: Vec<&serde_json::Value> = items
        .iter()
        .filter(|session| session["current"].as_bool().unwrap())
        .collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0]["person"]["id"], daemon.user_id.to_string());
    assert_eq!(current[0]["client_kind"], "browser");
    assert!(current[0]["created_at"].as_i64().unwrap() > 0);

    // The resources are per person, by the owner label of the Docker
    // objects, and the caps of the installation are beside them.
    let resources: serde_json::Value = at("/api/v1/administration/resources")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resources["items"].as_array().unwrap().len(), ids.len());
    assert_eq!(resources["awake_on_server"].as_u64().unwrap(), 0);
    assert!(resources["awake_cap_per_tenant"].as_u64().unwrap() > 0);
    let mine = resources["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["person"]["id"] == daemon.user_id.to_string())
        .expect("the administrator is on the list");
    assert_eq!(mine["containers"].as_u64().unwrap(), 0);
    assert_eq!(mine["volume_bytes"].as_u64().unwrap(), 0);

    let health: serde_json::Value = at("/api/v1/administration/health")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["version"], pagis_server::VERSION);
    assert_eq!(health["database"], "sqlite");
    assert_eq!(health["queued_runs"].as_u64().unwrap(), 0);
    assert_eq!(health["unfinished_runs"].as_u64().unwrap(), 0);
}

/// The Health view reports whether the machine holds the writable
/// container layer of a Computer to its size, next to the volume quota:
/// `unknown` until a Computer wakes, and then the answer of the runtime.
#[tokio::test]
async fn the_health_view_reports_the_container_quota() {
    let runtime = std::sync::Arc::new(pagis_computer::fake::FakeComputerRuntime::default());
    let daemon = TestDaemon::start_with(pagis_testkit::TestDaemonOptions {
        computer: std::sync::Arc::clone(&runtime) as _,
        ..Default::default()
    })
    .await;
    let health = || async {
        client()
            .get(format!(
                "{}/api/v1/administration/health",
                daemon.administration_base_url
            ))
            .header("cookie", daemon.cookie())
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap()
    };

    assert_eq!(health().await["container_quota"], "unknown");
    for (answer, word) in [
        (pagis_computer::Quota::Unsupported, "unsupported"),
        (pagis_computer::Quota::Supported, "supported"),
    ] {
        runtime.set_container_quota(answer);
        let health = health().await;
        assert_eq!(health["container_quota"], word, "{health}");
        assert_eq!(health["volume_quota"], "supported", "{health}");
    }
}

/// The installation settings answer on this port.
#[tokio::test]
async fn the_installation_settings_answer_on_the_administration_port() {
    let daemon = TestDaemon::start().await;
    let at = |path: &str| {
        client()
            .get(format!("{}{path}", daemon.administration_base_url))
            .header("cookie", daemon.cookie())
            .send()
    };

    let system: serde_json::Value = at("/api/v1/settings/system")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(system["version"], pagis_server::VERSION);
    assert!(system["data_directory"].as_str().unwrap().len() > 1);

    for path in [
        "/api/v1/administration/providers",
        "/api/v1/administration/people",
    ] {
        let response = at(path).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path} did not answer");
    }
}

/// The seeded person of a local installation holds no password:
/// the client trades the Client Credential. This port is where they set
/// a way in, and the password then signs a browser in on this port.
#[tokio::test]
async fn a_local_person_sets_their_own_way_in_on_this_port() {
    let daemon = TestDaemon::start().await;
    assert!(
        sign_in(&daemon.administration_base_url, "ada@example.com", PASSWORD)
            .await
            .is_none(),
        "the seeded person signs in with no password set"
    );

    let response = client()
        .put(format!(
            "{}/api/v1/administration/people/{}/sign-in",
            daemon.administration_base_url, daemon.user_id
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "email": "ada@example.com", "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let person: serde_json::Value = response.json().await.unwrap();
    assert_eq!(person["email"], "ada@example.com");

    // The Session they set it with stays: signing yourself out of the
    // interface you are using tells nobody anything.
    let still_signed_in = client()
        .get(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(still_signed_in.status(), StatusCode::OK);
    // And a browser on this port now signs in with the password.
    let cookie = sign_in(&daemon.administration_base_url, "ada@example.com", PASSWORD)
        .await
        .expect("the password signs a browser in on the administration port");
    assert!(cookie.starts_with("pagis_session="));
}

/// The first-run setup answers on this port while nobody can sign in,
/// and it is gone from the moment somebody can.
#[tokio::test]
async fn the_first_run_setup_answers_here_and_then_is_gone() {
    // A server holds no Client Credential, so its first-run setup
    // answers.
    let Some(daemon) = TestDaemon::start_on_postgres().await else {
        return;
    };
    let setup_url = format!("{}/api/v1/setup", daemon.administration_base_url);

    let open = client().get(&setup_url).send().await.unwrap();
    assert_eq!(open.status(), StatusCode::OK);
    let state: serde_json::Value = open.json().await.unwrap();
    assert!(!state["providers"].as_array().unwrap().is_empty());

    let done = client()
        .post(&setup_url)
        .json(&serde_json::json!({
            "email": "ada@example.net",
            "password": PASSWORD,
            "name": "Ada",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(done.status(), StatusCode::OK);

    // From the first password onwards the flow is spent, on this port as
    // on the product port.
    let spent = client().get(&setup_url).send().await.unwrap();
    assert_eq!(spent.status(), StatusCode::GONE);
    let body: serde_json::Value = spent.json().await.unwrap();
    assert_eq!(body["error"]["code"], "setup_complete");
    // And the administrator the flow made reads the interface.
    let cookie = sign_in(&daemon.administration_base_url, "ada@example.net", PASSWORD)
        .await
        .expect("the new administrator signs in");
    let roster = client()
        .get(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(roster.status(), StatusCode::OK);
    let people: serde_json::Value = roster.json().await.unwrap();
    assert_eq!(
        people["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|person| person["role"] == UserRole::Administrator.as_str())
            .count(),
        1
    );
}

/// The setup flow is spent on a local installation from its first boot:
/// the seed made the administrator, and the Client Credential is how
/// that person signs in.
#[tokio::test]
async fn the_setup_page_is_gone_on_a_local_installation() {
    let daemon = TestDaemon::start().await;

    let response = client()
        .get(format!("{}/api/v1/setup", daemon.administration_base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::GONE);
}

/// The two routes outside the guard are the documented two and no
/// others, so what answers without an Administrator is what the record
/// says answers without one.
#[test]
fn the_exceptions_are_the_documented_two() {
    let paths: Vec<&str> = ADMINISTRATION_PUBLIC_ROUTES
        .iter()
        .map(|(path, _)| *path)
        .collect();
    assert_eq!(paths, vec!["/api/v1/setup", "/api/v1/sessions"]);
}
