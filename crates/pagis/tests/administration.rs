//! What an Administrator does to the installation's people.
//!
//! The seeded person of a test daemon is the installation's
//! Administrator, so `daemon.cookie()` is the Administrator's. Every
//! test that needs a Member makes one through the route an
//! Administrator uses, which is also the route under test.

use futures::{SinkExt, StreamExt};
use pagis_testkit::TestDaemon;
use reqwest::StatusCode;
use tokio_tungstenite::tungstenite::Message;

const PASSWORD: &str = "correct horse battery";

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

async fn create_account(daemon: &TestDaemon, email: &str) -> serde_json::Value {
    let response = client()
        .post(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "email": email,
            "name": "Grace",
            "password": PASSWORD,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json().await.unwrap()
}

/// Sign in with a password and answer the `Cookie` header value.
async fn sign_in(daemon: &TestDaemon, email: &str, password: &str) -> Option<String> {
    let response = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
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

async fn roster(daemon: &TestDaemon) -> Vec<serde_json::Value> {
    let response = client()
        .get(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page: serde_json::Value = response.json().await.unwrap();
    page["items"].as_array().expect("items").clone()
}

async fn get_with(daemon: &TestDaemon, cookie: &str, path: &str) -> reqwest::Response {
    client()
        .get(format!("{}{path}", daemon.base_url))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
}

/// A read of the Administration Port, where the installation's
/// settings answer.
async fn administration_get_with(
    daemon: &TestDaemon,
    cookie: &str,
    path: &str,
) -> reqwest::Response {
    client()
        .get(format!("{}{path}", daemon.administration_base_url))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
}

/// An Administrator makes an account, and the person signs in to an
/// empty Workspace with a working model route and no key of their own.
#[tokio::test]
async fn an_administrator_creates_an_account_and_the_person_signs_in_to_their_own_workspace() {
    let daemon = TestDaemon::start().await;

    let person = create_account(&daemon, "grace@example.com").await;
    assert_eq!(person["role"], "member");
    assert_eq!(person["disabled"], false);
    assert!(!person["workspace_id"].is_null(), "{person}");

    let cookie = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("the new person signs in");

    // Their own Workspace: one sprite of their own, not the
    // administrator's.
    let agents: serde_json::Value = get_with(&daemon, &cookie, "/api/v1/agents")
        .await
        .json()
        .await
        .unwrap();
    let items = agents["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{agents}");
    assert_eq!(items[0]["name"], "Pixie");
    assert_ne!(items[0]["id"].as_str().unwrap(), daemon.agent_id);

    // A working model route, written by the same seed a local first run
    // uses.
    let aliases: serde_json::Value = get_with(&daemon, &cookie, "/api/v1/settings/model-aliases")
        .await
        .json()
        .await
        .unwrap();
    let default = aliases["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|alias| alias["alias"] == "default")
        .expect("the default alias");
    assert!(
        !default["candidates"]
            .as_array()
            .expect("candidates")
            .is_empty(),
        "{default}"
    );

    // And no key of their own: the installation's keys serve them.
    assert_eq!(
        administration_get_with(&daemon, &cookie, "/api/v1/administration/providers")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}

/// The default route of a Person, in the Workspace the cookie opens.
async fn default_candidates(daemon: &TestDaemon, cookie: &str) -> serde_json::Value {
    let aliases: serde_json::Value = get_with(daemon, cookie, "/api/v1/settings/model-aliases")
        .await
        .json()
        .await
        .unwrap();
    aliases["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|alias| alias["alias"] == "default")
        .expect("the default alias")["candidates"]
        .clone()
}

async fn set_default_candidates(daemon: &TestDaemon, candidates: serde_json::Value) {
    let response = client()
        .put(format!(
            "{}/api/v1/settings/model-aliases/default",
            daemon.base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "candidates": candidates }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// A new Person thinks on the model the Administrator chose, not on the
/// first provider of a fixed order that holds a key.
#[tokio::test]
async fn a_new_person_takes_the_route_the_administrator_chose() {
    let daemon = pagis_testkit::TestDaemon::start_with(pagis_testkit::TestDaemonOptions {
        keys: pagis_testkit::test_provider_keys(vec![
            ("ANTHROPIC_API_KEY", "sk-ant"),
            ("OPENAI_API_KEY", "sk-openai"),
        ]),
        ..pagis_testkit::TestDaemonOptions::default()
    })
    .await;
    set_default_candidates(&daemon, serde_json::json!(["openai/gpt-6-luna"])).await;

    create_account(&daemon, "grace@example.com").await;

    let cookie = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .unwrap();
    assert_eq!(
        default_candidates(&daemon, &cookie).await,
        serde_json::json!(["openai/gpt-6-luna"])
    );
}

/// A candidate of the Administrator's route whose provider holds no key
/// reaches nothing, so the new Person's route leaves it out.
#[tokio::test]
async fn a_new_person_drops_a_candidate_whose_provider_holds_no_key() {
    let daemon = pagis_testkit::TestDaemon::start_with(pagis_testkit::TestDaemonOptions {
        keys: pagis_testkit::test_provider_keys(vec![("OPENAI_API_KEY", "sk-openai")]),
        ..pagis_testkit::TestDaemonOptions::default()
    })
    .await;
    set_default_candidates(
        &daemon,
        serde_json::json!(["anthropic/claude-opus-5", "openai/gpt-6-luna"]),
    )
    .await;

    create_account(&daemon, "grace@example.com").await;

    let cookie = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .unwrap();
    assert_eq!(
        default_candidates(&daemon, &cookie).await,
        serde_json::json!(["openai/gpt-6-luna"])
    );
}

/// A Member reads and writes none of the installation's provider keys.
/// The Org holds one key per provider and every person thinks
/// on it, so rotating one is one write and never one per person.
#[tokio::test]
async fn a_member_reaches_none_of_the_installations_provider_keys() {
    let daemon = TestDaemon::start().await;
    create_account(&daemon, "grace@example.com").await;
    let cookie = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("sign in");

    for response in [
        administration_get_with(&daemon, &cookie, "/api/v1/administration/providers").await,
        client()
            .put(format!(
                "{}/api/v1/administration/providers/anthropic/key",
                daemon.administration_base_url
            ))
            .header("cookie", &cookie)
            .json(&serde_json::json!({ "fields": { "api_key": "sk-theirs" } }))
            .send()
            .await
            .unwrap(),
        client()
            .delete(format!(
                "{}/api/v1/administration/providers/anthropic/key",
                daemon.administration_base_url
            ))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap(),
        // The first-run write of the product port is the
        // Administrator's too, so a Member's onboarding stores no key.
        client()
            .put(format!(
                "{}/api/v1/settings/onboarding/providers/anthropic/key",
                daemon.base_url
            ))
            .header("cookie", &cookie)
            .json(&serde_json::json!({ "key": "sk-theirs" }))
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "forbidden");
    }

    // Their onboarding read names no provider key either, so the wizard
    // never shows them the installation's keys.
    let onboarding: serde_json::Value = get_with(&daemon, &cookie, "/api/v1/settings/onboarding")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        onboarding["providers"].as_array().expect("providers").len(),
        0,
        "{onboarding}"
    );

    // The administrator still reads and writes them.
    assert_eq!(
        administration_get_with(&daemon, daemon.cookie(), "/api/v1/administration/providers")
            .await
            .status(),
        StatusCode::OK
    );
}

/// The carrier connection and the mail domain belong to the Org.
/// A Member writes neither, on either port, so no person holds a copy
/// of the installation's carrier key.
#[tokio::test]
async fn a_member_writes_neither_the_carrier_connection_nor_the_mail_domain() {
    let daemon = TestDaemon::start().await;
    create_account(&daemon, "grace@example.com").await;
    let cookie = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("sign in");

    for (provider, fields) in [
        ("telnyx", serde_json::json!({ "api_key": "carrier-secret" })),
        (
            "migadu",
            serde_json::json!({
                "account": "ops@example.com",
                "api_key": "mail-secret",
                "domain": "example.com",
            }),
        ),
    ] {
        let response = client()
            .post(format!("{}/api/v1/settings/connections", daemon.base_url))
            .header("cookie", &cookie)
            .json(&serde_json::json!({
                "provider": provider,
                "alias": provider,
                "display_name": provider,
                "fields": fields,
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "a member created a {provider} connection"
        );
        let response = client()
            .put(format!(
                "{}/api/v1/administration/providers/{provider}/connection",
                daemon.administration_base_url
            ))
            .header("cookie", &cookie)
            .json(&serde_json::json!({ "fields": fields }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "a member set up {provider} on the administration port"
        );
    }

    // And their connection list holds none of the installation's.
    let connections: serde_json::Value = get_with(&daemon, &cookie, "/api/v1/settings/connections")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        connections["items"].as_array().expect("items").len(),
        0,
        "{connections}"
    );
}

/// A Member reaches no administration route, against the
/// administrator's own id.
#[tokio::test]
async fn a_member_reaches_no_administration_route() {
    let daemon = TestDaemon::start().await;
    create_account(&daemon, "grace@example.com").await;
    let cookie = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("sign in");
    let administrator = daemon.user_id.to_string();

    let refused = [
        client().get(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        )),
        client()
            .post(format!(
                "{}/api/v1/administration/people",
                daemon.administration_base_url
            ))
            .json(&serde_json::json!({
                "email": "mallory@example.com",
                "name": "Mallory",
                "password": PASSWORD,
            })),
        client().post(format!(
            "{}/api/v1/administration/people/{administrator}/disable",
            daemon.administration_base_url
        )),
        client().post(format!(
            "{}/api/v1/administration/people/{administrator}/enable",
            daemon.administration_base_url
        )),
        client()
            .post(format!(
                "{}/api/v1/administration/people/{administrator}/password",
                daemon.administration_base_url
            ))
            .json(&serde_json::json!({ "password": "a new password here" })),
        client()
            .put(format!(
                "{}/api/v1/administration/people/{administrator}/spend-cap",
                daemon.administration_base_url
            ))
            .json(&serde_json::json!({ "monthly_spend_cap_usd": 1.0 })),
        client().get(format!(
            "{}/api/v1/administration/usage",
            daemon.administration_base_url
        )),
    ];
    for request in refused {
        let response = request.header("cookie", &cookie).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    // The roster still holds both people, so nothing the Member sent
    // changed the installation.
    assert_eq!(roster(&daemon).await.len(), 2);
}

/// A disabled account reaches nothing and signs in to nothing, and
/// re-enabling gives the same person the same Workspace back.
#[tokio::test]
async fn disabling_an_account_ends_its_sessions_and_re_enabling_gives_it_back() {
    let daemon = TestDaemon::start().await;
    let person = create_account(&daemon, "grace@example.com").await;
    let id = person["id"].as_str().unwrap().to_string();
    let workspace = person["workspace_id"].as_str().unwrap().to_string();
    let cookie = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("sign in");
    assert_eq!(
        get_with(&daemon, &cookie, "/api/v1/user").await.status(),
        StatusCode::OK
    );

    let disabled: serde_json::Value = client()
        .post(format!(
            "{}/api/v1/administration/people/{id}/disable",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(disabled["disabled"], true);

    // The Session they held is gone, and the password opens nothing.
    assert_eq!(
        get_with(&daemon, &cookie, "/api/v1/user").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        sign_in(&daemon, "grace@example.com", PASSWORD)
            .await
            .is_none()
    );

    let enabled: serde_json::Value = client()
        .post(format!(
            "{}/api/v1/administration/people/{id}/enable",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(enabled["disabled"], false);
    // The same Workspace, so nothing of theirs was lost.
    assert_eq!(enabled["workspace_id"], workspace);
    assert!(
        sign_in(&daemon, "grace@example.com", PASSWORD)
            .await
            .is_some()
    );
}

/// An administrator does not disable the account they are signed in as:
/// an installation with no administrator has nobody to configure it.
#[tokio::test]
async fn an_administrator_does_not_disable_their_own_account() {
    let daemon = TestDaemon::start().await;

    let response = client()
        .post(format!(
            "{}/api/v1/administration/people/{}/disable",
            daemon.administration_base_url, daemon.user_id
        ))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// A reset replaces the password and ends the Sessions the old one
/// opened: a reset that left a stolen Session alive would reset nothing.
#[tokio::test]
async fn a_password_reset_ends_the_old_sessions_and_the_new_password_works() {
    let daemon = TestDaemon::start().await;
    let person = create_account(&daemon, "grace@example.com").await;
    let id = person["id"].as_str().unwrap().to_string();
    let cookie = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("sign in");

    let response = client()
        .post(format!(
            "{}/api/v1/administration/people/{id}/password",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "password": "another long password" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(
        get_with(&daemon, &cookie, "/api/v1/user").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        sign_in(&daemon, "grace@example.com", PASSWORD)
            .await
            .is_none()
    );
    assert!(
        sign_in(&daemon, "grace@example.com", "another long password")
            .await
            .is_some()
    );
}

/// The roster names every person with their role, their last sign-in
/// and their cap.
#[tokio::test]
async fn the_roster_names_every_person_with_the_last_sign_in_and_the_cap() {
    let daemon = TestDaemon::start().await;
    let person = create_account(&daemon, "grace@example.com").await;
    let id = person["id"].as_str().unwrap().to_string();
    assert!(person["last_signed_in_at"].is_null(), "{person}");

    sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("sign in");
    client()
        .put(format!(
            "{}/api/v1/administration/people/{id}/spend-cap",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "monthly_spend_cap_usd": 25.0 }))
        .send()
        .await
        .unwrap();

    let items = roster(&daemon).await;
    assert_eq!(items.len(), 2);
    let grace = items
        .iter()
        .find(|item| item["email"] == "grace@example.com")
        .expect("the new person is on the roster");
    assert_eq!(grace["role"], "member");
    assert_eq!(grace["monthly_spend_cap_usd"], 25.0);
    assert!(!grace["last_signed_in_at"].is_null(), "{grace}");
    let administrator = items
        .iter()
        .find(|item| item["id"] == daemon.user_id.to_string())
        .expect("the seeded person is on the roster");
    assert_eq!(administrator["role"], "administrator");
}

/// A cap is a positive number of dollars or nothing at all.
#[tokio::test]
async fn a_spend_cap_is_a_positive_amount_or_none() {
    let daemon = TestDaemon::start().await;
    let person = create_account(&daemon, "grace@example.com").await;
    let id = person["id"].as_str().unwrap().to_string();

    for bad in [serde_json::json!(0.0), serde_json::json!(-5.0)] {
        let response = client()
            .put(format!(
                "{}/api/v1/administration/people/{id}/spend-cap",
                daemon.administration_base_url
            ))
            .header("cookie", daemon.cookie())
            .json(&serde_json::json!({ "monthly_spend_cap_usd": bad }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    let cleared: serde_json::Value = client()
        .put(format!(
            "{}/api/v1/administration/people/{id}/spend-cap",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "monthly_spend_cap_usd": null }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(cleared["monthly_spend_cap_usd"].is_null(), "{cleared}");
}

/// One address is one account, whatever case a client typed it in.
#[tokio::test]
async fn one_address_is_one_account() {
    let daemon = TestDaemon::start().await;
    create_account(&daemon, "grace@example.com").await;

    let response = client()
        .post(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "email": "Grace@Example.COM",
            "name": "Grace again",
            "password": PASSWORD,
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(roster(&daemon).await.len(), 2);
}

/// An account an administrator creates lands in the product, not in the
/// local first-run wizard.
///
/// The wizard asks which model the installation thinks on, and the
/// administrator answered that for everybody before the account
/// existed. So the Workspace is onboarded when it is made: the person's
/// own onboarding read says `completed`, and nothing asks them to
/// complete onboarding.
#[tokio::test]
async fn a_created_account_is_onboarded_already() {
    let daemon = TestDaemon::start().await;
    let person = create_account(&daemon, "grace@example.com").await;
    let cookie = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("the created person signs in");

    let onboarding: serde_json::Value = client()
        .get(format!("{}/api/v1/settings/onboarding", daemon.base_url))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(onboarding["completed"], true, "{onboarding}");

    // And the record itself says so, so nothing derives it per request.
    let workspace = daemon
        .stores()
        .workspaces
        .for_user(&pagis_core::UserId::from(
            person["id"].as_str().expect("the person's id").to_string(),
        ))
        .await
        .expect("read the workspace")
        .expect("the created person owns a workspace");
    assert!(workspace.onboarded_at.is_some());
}

/// The administrator a server's own first run makes is onboarded too:
/// they typed the address, the password and the installation's
/// provider keys, which is the whole wizard.
#[tokio::test]
async fn the_server_setup_administrator_is_onboarded_already() {
    let Some(daemon) = TestDaemon::start_on_postgres_with(pagis_testkit::TestDaemonOptions {
        public_origin: "https://pagis.example".to_string(),
        ..pagis_testkit::TestDaemonOptions::default()
    })
    .await
    else {
        return;
    };
    assert_eq!(daemon.booted.client_credential, None);

    let response = client()
        .post(format!("{}/api/v1/setup", daemon.administration_base_url))
        .json(&serde_json::json!({
            "email": "ada@example.com",
            "password": PASSWORD,
            "provider_keys": { "anthropic": "sk-installation" },
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("pagis_session="))
        .and_then(|value| value.split(';').next())
        .expect("the answer signs the administrator in")
        .to_string();

    let onboarding: serde_json::Value = client()
        .get(format!("{}/api/v1/settings/onboarding", daemon.base_url))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(onboarding["completed"], true, "{onboarding}");
}

/// The close code of a socket whose Session ended.
const SESSION_ENDED: u16 = 1008;

/// What an Administrator does that ends every Session of a Person.
#[derive(Debug, Clone, Copy)]
enum EverySessionEnds {
    Disable,
    ResetPassword,
    SetWayIn,
}

/// Do `action` to the Person `id` on the administration port.
async fn end_every_session(daemon: &TestDaemon, id: &str, action: EverySessionEnds) {
    let base = &daemon.administration_base_url;
    let request = match action {
        EverySessionEnds::Disable => {
            client().post(format!("{base}/api/v1/administration/people/{id}/disable"))
        }
        EverySessionEnds::ResetPassword => client()
            .post(format!("{base}/api/v1/administration/people/{id}/password"))
            .json(&serde_json::json!({ "password": "another long password" })),
        EverySessionEnds::SetWayIn => client()
            .put(format!("{base}/api/v1/administration/people/{id}/sign-in"))
            .json(&serde_json::json!({
                "email": "grace@example.com",
                "password": "another long password",
            })),
    };
    let response = request
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{action:?} is refused");
}

/// Every socket of the Person closes with 1008, whichever of their
/// Sessions opened it, and the Administrator keeps their own.
async fn every_socket_of_the_person_closes(action: EverySessionEnds) {
    let daemon = TestDaemon::start().await;
    let person = create_account(&daemon, "grace@example.com").await;
    let id = person["id"].as_str().unwrap().to_string();
    let laptop = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("sign in on the laptop");
    let phone = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("sign in on the phone");
    let mut on_laptop = daemon.event_socket(&laptop).await;
    let mut on_phone = daemon.event_socket(&phone).await;
    let mut administrators = daemon.event_socket(daemon.cookie()).await;

    end_every_session(&daemon, &id, action).await;

    for socket in [&mut on_laptop, &mut on_phone] {
        assert_eq!(
            pagis_testkit::read_until_closed(socket).await.code,
            SESSION_ENDED,
            "{action:?}"
        );
    }
    administrators
        .send(Message::text(r#"{"type":"ping"}"#))
        .await
        .expect("the Administrator's socket is open");
    let pong = tokio::time::timeout(std::time::Duration::from_secs(5), administrators.next())
        .await
        .expect("an answer before the timeout")
        .expect("the Administrator's socket is open")
        .expect("a readable frame");
    assert_eq!(pong.into_text().unwrap().as_str(), r#"{"type":"pong"}"#);
}

#[tokio::test]
async fn disabling_a_person_closes_every_socket_of_theirs() {
    every_socket_of_the_person_closes(EverySessionEnds::Disable).await;
}

#[tokio::test]
async fn resetting_a_password_closes_every_socket_of_the_person() {
    every_socket_of_the_person_closes(EverySessionEnds::ResetPassword).await;
}

#[tokio::test]
async fn setting_a_way_in_closes_every_socket_of_the_person() {
    every_socket_of_the_person_closes(EverySessionEnds::SetWayIn).await;
}

/// A Takeover of the Person's Computer ends and the input goes back to
/// the Agent. No Session of the Person remains, so nobody could hand
/// the Computer back.
async fn the_takeover_of_the_person_ends(action: EverySessionEnds) {
    use std::sync::Arc;

    use pagis_computer::InputHolder;
    use pagis_computer::fake::FakeComputerRuntime;

    let runtime = Arc::new(FakeComputerRuntime::with_image());
    let daemon = TestDaemon::start_with(pagis_testkit::TestDaemonOptions {
        computer: Arc::clone(&runtime) as _,
        ..pagis_testkit::TestDaemonOptions::default()
    })
    .await;
    let person = create_account(&daemon, "grace@example.com").await;
    let id = person["id"].as_str().unwrap().to_string();
    let workspace_id =
        pagis_core::WorkspaceId::from(person["workspace_id"].as_str().unwrap().to_string());
    let cookie = sign_in(&daemon, "grace@example.com", PASSWORD)
        .await
        .expect("sign in");
    let agents: serde_json::Value = get_with(&daemon, &cookie, "/api/v1/agents")
        .await
        .json()
        .await
        .unwrap();
    let agent_id = agents["items"][0]["id"].as_str().unwrap().to_string();
    let post = |path: String| {
        let cookie = cookie.clone();
        let url = format!("{}{path}", daemon.base_url);
        async move {
            client()
                .post(url)
                .header("cookie", cookie)
                .send()
                .await
                .unwrap()
        }
    };
    post(format!("/api/v1/agents/{agent_id}/computer/wake")).await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let state: serde_json::Value = get_with(
            &daemon,
            &cookie,
            &format!("/api/v1/agents/{agent_id}/computer"),
        )
        .await
        .json()
        .await
        .unwrap();
        if state["state"] == "awake" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never awake: {state}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let takeover = post(format!("/api/v1/agents/{agent_id}/screen/takeover")).await;
    assert_eq!(takeover.status(), StatusCode::OK);
    let agent = pagis_core::AgentId::from(agent_id.clone());
    assert_eq!(runtime.holder(&agent), InputHolder::User);

    end_every_session(&daemon, &id, action).await;

    assert_eq!(
        runtime.holder(&agent),
        InputHolder::Agent,
        "{action:?} left the Person holding the input"
    );
    let ended = daemon
        .stores()
        .events
        .list_by_types(&workspace_id, &["screen.takeover_ended"], None, 10)
        .await
        .unwrap();
    assert_eq!(ended.len(), 1, "{action:?}: {ended:?}");
    assert_eq!(ended[0].agent_id.as_ref(), Some(&agent));
}

#[tokio::test]
async fn disabling_a_person_ends_their_takeover() {
    the_takeover_of_the_person_ends(EverySessionEnds::Disable).await;
}

#[tokio::test]
async fn resetting_a_password_ends_the_takeover_of_the_person() {
    the_takeover_of_the_person_ends(EverySessionEnds::ResetPassword).await;
}

#[tokio::test]
async fn setting_a_way_in_ends_the_takeover_of_the_person() {
    the_takeover_of_the_person_ends(EverySessionEnds::SetWayIn).await;
}
