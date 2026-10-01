//! The installation's setup of each provider, on the administration
//! port.
//!
//! One set of routes serves every provider: the model keys, the
//! Installation OAuth Client, the carrier accounts with their SIP
//! sign-in, and the mail domains. What a person does with a provider
//! stays on the product port, and the product port refuses every
//! installation part of every provider. The carrier and the mail host
//! are fakes, so no network is reached.

use std::sync::Arc;

use pagis_core::UserRole;
use pagis_mail::fake::FakeMailboxHost;
use pagis_telephony::fake::FakeNumberCatalog;
use pagis_testkit::{TestDaemon, TestDaemonOptions};

const PASSWORD: &str = "correct horse battery";

async fn boot() -> TestDaemon {
    TestDaemon::start_with(TestDaemonOptions {
        number_catalog: Arc::new(FakeNumberCatalog::offering(&["+14155550123"])) as _,
        mail_host: Arc::new(FakeMailboxHost::default()) as _,
        ..TestDaemonOptions::default()
    })
    .await
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// One request with the seeded Administrator's Session, and its status
/// and body.
async fn send(
    daemon: &TestDaemon,
    method: reqwest::Method,
    url: String,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    send_as(daemon.cookie(), method, url, body).await
}

async fn send_as(
    cookie: &str,
    method: reqwest::Method,
    url: String,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let mut request = client().request(method, url).header("cookie", cookie);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (
        status,
        response.json().await.unwrap_or(serde_json::Value::Null),
    )
}

fn administration(daemon: &TestDaemon, path: &str) -> String {
    format!(
        "{}/api/v1/administration/providers{path}",
        daemon.administration_base_url
    )
}

fn product(daemon: &TestDaemon, path: &str) -> String {
    format!("{}{path}", daemon.base_url)
}

async fn setups(daemon: &TestDaemon) -> Vec<serde_json::Value> {
    let (status, page) = send(
        daemon,
        reqwest::Method::GET,
        administration(daemon, ""),
        None,
    )
    .await;
    assert_eq!(status, 200, "{page}");
    page["items"].as_array().unwrap().clone()
}

/// One part of one provider's setup, from the list.
async fn part(daemon: &TestDaemon, provider: &str, part: &str) -> serde_json::Value {
    part_of(
        &setups(daemon)
            .await
            .into_iter()
            .find(|setup| setup["provider"] == provider)
            .unwrap_or_else(|| panic!("{provider} is not on the list")),
        part,
    )
}

fn part_of(setup: &serde_json::Value, part: &str) -> serde_json::Value {
    setup["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == part)
        .unwrap_or_else(|| panic!("no part {part} in {setup}"))
        .clone()
}

/// The value of one fact a part states, such as the account or the
/// redirect URI.
fn fact(part: &serde_json::Value, label: &str) -> Option<String> {
    part["facts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|fact| fact["label"] == label)
        .map(|fact| fact["value"].as_str().unwrap().to_string())
}

async fn test_part(daemon: &TestDaemon, provider: &str, part: &str) -> (u16, serde_json::Value) {
    send(
        daemon,
        reqwest::Method::POST,
        administration(daemon, &format!("/{provider}/{part}/test")),
        None,
    )
    .await
}

async fn remove_part(daemon: &TestDaemon, provider: &str, part: &str) -> (u16, serde_json::Value) {
    send(
        daemon,
        reqwest::Method::DELETE,
        administration(daemon, &format!("/{provider}/{part}")),
        None,
    )
    .await
}

/// A Member of the installation and the `Cookie` header of their
/// Session.
async fn member(daemon: &TestDaemon) -> String {
    let (status, person) = send(
        daemon,
        reqwest::Method::POST,
        format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ),
        Some(serde_json::json!({
            "email": "grace@example.com",
            "name": "Grace",
            "password": PASSWORD,
        })),
    )
    .await;
    assert_eq!(status, 201, "{person}");
    assert_eq!(person["person"]["role"], UserRole::Member.as_str());
    let response = client()
        .post(product(daemon, "/api/v1/sessions"))
        .json(&serde_json::json!({ "email": "grace@example.com", "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("pagis_session="))
        .and_then(|value| value.split(';').next())
        .expect("the member signs in")
        .to_string()
}

/// The list names every provider the installation sets up, grouped,
/// with the fields of each part and whether a test proves anything.
#[tokio::test]
async fn the_administration_port_lists_what_the_installation_sets_up_for_every_provider() {
    let daemon = boot().await;

    let items = setups(&daemon).await;

    let providers: Vec<&str> = items
        .iter()
        .map(|setup| setup["provider"].as_str().unwrap())
        .collect();
    assert_eq!(
        providers,
        vec![
            "anthropic",
            "openai",
            "openrouter",
            "deepgram",
            "elevenlabs",
            "google",
            "telnyx",
            "twilio",
            "plivo",
            "migadu",
            "manual"
        ]
    );
    let telnyx = items
        .iter()
        .find(|setup| setup["provider"] == "telnyx")
        .unwrap();
    assert_eq!(telnyx["group"], "telephony");
    let account = part_of(telnyx, "connection");
    assert_eq!(account["kind"], "connection");
    assert_eq!(account["configured"], false);
    assert_eq!(account["testable"], true);
    assert_eq!(account["fields"][0]["key"], "api_key");
    assert_eq!(account["fields"][0]["secret"], true);
    let sip = part_of(telnyx, "sip");
    assert_eq!(sip["testable"], false);
    assert_eq!(sip["fields"][2]["default"], "sip.telnyx.com");

    // The redirect URI is what the administrator pastes into the Google
    // console, so the part states it before a client exists.
    let google = part(&daemon, "google", "oauth-client").await;
    assert_eq!(google["configured"], false);
    assert_eq!(
        fact(&google, "Redirect URI").unwrap(),
        format!(
            "{}/api/v1/connections/google/callback",
            daemon.public_origin
        )
    );
}

/// A key from the environment names its variable, which is where an
/// Administrator changes or removes it.
#[tokio::test]
async fn a_model_key_from_the_environment_names_the_variable() {
    let keys = pagis_testkit::test_provider_keys(vec![("OPENAI_API_KEY", "sk-env")]);
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        keys,
        ..TestDaemonOptions::default()
    })
    .await;

    let key = part(&daemon, "openai", "key").await;

    assert_eq!(key["configured"], true);
    assert_eq!(
        fact(&key, "Source").as_deref(),
        Some("the OPENAI_API_KEY environment variable")
    );
}

/// A model provider's key is one part like any other: set, then
/// removed, through the same routes.
#[tokio::test]
async fn a_model_key_is_set_and_removed_through_the_provider_routes() {
    let keys = pagis_testkit::test_provider_keys(Vec::new());
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        keys: Arc::clone(&keys),
        ..TestDaemonOptions::default()
    })
    .await;

    let (status, setup) = daemon
        .set_up_provider(
            "anthropic",
            "key",
            serde_json::json!({ "api_key": " sk-ant " }),
        )
        .await;
    assert_eq!(status, 200, "{setup}");
    let key = part_of(&setup, "key");
    assert_eq!(key["configured"], true);
    assert_eq!(fact(&key, "Source").as_deref(), Some("secrets.enc"));
    assert_eq!(
        keys.resolve(pagis_core::Provider::Anthropic)
            .unwrap()
            .unwrap()
            .0,
        "sk-ant"
    );
    // Nothing reads a key back.
    assert!(!setup.to_string().contains("sk-ant"));

    let (status, refused) = daemon
        .set_up_provider("anthropic", "key", serde_json::json!({ "api_key": " " }))
        .await;
    assert_eq!(status, 422, "{refused}");

    let (status, setup) = remove_part(&daemon, "anthropic", "key").await;
    assert_eq!(status, 200, "{setup}");
    assert_eq!(part_of(&setup, "key")["configured"], false);

    // A key proves itself on a model check, not here.
    let (status, refused) = test_part(&daemon, "anthropic", "key").await;
    assert_eq!(status, 422, "{refused}");
}

/// The Installation OAuth Client makes a person's Google connection
/// brokered; the person then types the account alone.
#[tokio::test]
async fn the_google_oauth_client_is_set_up_for_every_person_and_removed() {
    let daemon = boot().await;
    let google_fields = || async {
        let (status, page) = send(
            &daemon,
            reqwest::Method::GET,
            product(&daemon, "/api/v1/settings/connections/providers"),
            None,
        )
        .await;
        assert_eq!(status, 200);
        page["items"][0]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field["key"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert!(google_fields().await.contains(&"client_id".to_string()));

    let (status, setup) = daemon
        .set_up_provider(
            "google",
            "oauth-client",
            serde_json::json!({
                "client_id": "installation.apps.googleusercontent.com",
                "client_secret": "GOCSPX-installation",
            }),
        )
        .await;
    assert_eq!(status, 200, "{setup}");
    let client_part = part_of(&setup, "oauth-client");
    assert_eq!(client_part["configured"], true);
    assert_eq!(
        fact(&client_part, "Client ID").as_deref(),
        Some("installation.apps.googleusercontent.com")
    );
    assert!(!setup.to_string().contains("GOCSPX"));
    assert_eq!(google_fields().await, vec!["account".to_string()]);

    let (status, setup) = remove_part(&daemon, "google", "oauth-client").await;
    assert_eq!(status, 200, "{setup}");
    assert_eq!(part_of(&setup, "oauth-client")["configured"], false);
    assert!(google_fields().await.contains(&"client_id".to_string()));
}

/// The carrier account and its SIP sign-in are the installation's. The
/// person buys numbers on top of them on the product port.
#[tokio::test]
async fn a_carrier_account_and_its_sip_sign_in_are_set_up_tested_and_removed() {
    let daemon = boot().await;

    // The SIP sign-in needs the account it signs in to.
    let (status, refused) = daemon
        .set_up_provider(
            "telnyx",
            "sip",
            serde_json::json!({ "username": "robin", "password": "x", "domain": "sip.telnyx.com" }),
        )
        .await;
    assert_eq!(status, 409, "{refused}");

    let connection_id = daemon
        .connect_installation("telnyx", serde_json::json!({ "api_key": "telnyx-key" }))
        .await;
    let account = part(&daemon, "telnyx", "connection").await;
    assert_eq!(account["configured"], true);
    assert_eq!(account["status"], "connected");
    assert!(!account.to_string().contains("telnyx-key"));

    // The person's side reads the carrier the installation set up.
    let (status, page) = send(
        &daemon,
        reqwest::Method::GET,
        product(&daemon, "/api/v1/settings/phone-numbers"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(page["carrier"]["connection_id"], connection_id.as_str());

    let (status, setup) = daemon
        .set_up_provider(
            "telnyx",
            "sip",
            serde_json::json!({
                "username": "robin",
                "password": "sip-secret",
                "domain": "SIP.Telnyx.com",
            }),
        )
        .await;
    assert_eq!(status, 200, "{setup}");
    let sip = part_of(&setup, "sip");
    assert_eq!(sip["configured"], true);
    assert_eq!(
        fact(&sip, "Sign-in").as_deref(),
        Some("robin@sip.telnyx.com")
    );
    assert!(!setup.to_string().contains("sip-secret"));

    let (status, tested) = test_part(&daemon, "telnyx", "connection").await;
    assert_eq!(status, 200, "{tested}");
    assert_eq!(part_of(&tested, "connection")["status"], "connected");
    let (status, refused) = test_part(&daemon, "telnyx", "sip").await;
    assert_eq!(status, 422, "{refused}");

    // A replacement key is proved before it is kept.
    let (status, setup) = daemon
        .set_up_provider(
            "telnyx",
            "connection",
            serde_json::json!({ "api_key": "new-key" }),
        )
        .await;
    assert_eq!(status, 200, "{setup}");
    assert_eq!(
        part_of(&setup, "connection")["connection_id"],
        connection_id.as_str()
    );

    let (status, setup) = remove_part(&daemon, "telnyx", "sip").await;
    assert_eq!(status, 200, "{setup}");
    assert_eq!(part_of(&setup, "sip")["configured"], false);

    let (status, setup) = remove_part(&daemon, "telnyx", "connection").await;
    assert_eq!(status, 200, "{setup}");
    assert_eq!(part_of(&setup, "connection")["configured"], false);
    let (status, page) = send(
        &daemon,
        reqwest::Method::GET,
        product(&daemon, "/api/v1/settings/phone-numbers"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(page["carrier"], serde_json::Value::Null);
}

/// A carrier that still carries a number stays (ADR-0018). The
/// refusal says what to do first.
#[tokio::test]
async fn a_carrier_that_carries_a_number_is_not_removed() {
    let daemon = boot().await;
    daemon
        .connect_installation("telnyx", serde_json::json!({ "api_key": "telnyx-key" }))
        .await;
    let (status, number) = send(
        &daemon,
        reqwest::Method::POST,
        product(&daemon, "/api/v1/settings/phone-numbers"),
        Some(serde_json::json!({ "e164": "+14155550123" })),
    )
    .await;
    assert_eq!(status, 201, "{number}");

    let (status, refused) = remove_part(&daemon, "telnyx", "connection").await;

    assert_eq!(status, 409, "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Release the numbers first"),
        "{refused}"
    );
}

/// A mail domain is set up once, tested, and removed while no mailbox
/// is on it.
#[tokio::test]
async fn a_mail_domain_is_set_up_tested_and_removed() {
    let daemon = boot().await;

    let (status, refused) = daemon
        .set_up_provider(
            "migadu",
            "connection",
            serde_json::json!({ "domain": "example.com" }),
        )
        .await;
    assert_eq!(status, 422, "{refused}");

    let connection_id = daemon
        .connect_installation(
            "migadu",
            serde_json::json!({
                "account": "owner@example.com",
                "api_key": "migadu-key",
                "domain": "example.com",
            }),
        )
        .await;
    let domain = part(&daemon, "migadu", "connection").await;
    assert_eq!(domain["connection_id"], connection_id.as_str());
    assert_eq!(fact(&domain, "Domain").as_deref(), Some("example.com"));
    assert_eq!(
        fact(&domain, "Account").as_deref(),
        Some("owner@example.com")
    );

    let (status, tested) = test_part(&daemon, "migadu", "connection").await;
    assert_eq!(status, 200, "{tested}");

    let (status, removed) = remove_part(&daemon, "migadu", "connection").await;
    assert_eq!(status, 200, "{removed}");
    assert_eq!(part_of(&removed, "connection")["configured"], false);
}

/// A provider or a part the installation does not set up is not found,
/// and a part that is not set up has nothing to test or remove.
#[tokio::test]
async fn an_unknown_provider_or_part_is_not_found() {
    let daemon = boot().await;

    for (provider, part) in [
        ("fax", "key"),
        ("telnyx", "key"),
        ("anthropic", "connection"),
    ] {
        let (status, body) = daemon
            .set_up_provider(provider, part, serde_json::json!({ "api_key": "x" }))
            .await;
        assert_eq!(status, 404, "{provider}/{part}: {body}");
    }
    let (status, body) = test_part(&daemon, "telnyx", "connection").await;
    assert_eq!(status, 404, "{body}");
    let (status, body) = remove_part(&daemon, "migadu", "connection").await;
    assert_eq!(status, 404, "{body}");
}

/// The product port refuses every installation part of every provider,
/// for an Administrator as much as for a Member: the Administration
/// Interface is the one place that sets one up.
#[tokio::test]
async fn the_product_port_refuses_every_installation_part_of_every_provider() {
    let daemon = boot().await;
    let member_cookie = member(&daemon).await;

    // A person picks from their own providers alone.
    let (status, page) = send(
        &daemon,
        reqwest::Method::GET,
        product(&daemon, "/api/v1/settings/connections/providers"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    for entry in page["items"].as_array().unwrap() {
        let id = entry["id"].as_str().unwrap();
        assert!(
            !pagis_connect::is_installation_provider(id),
            "{id} is on the picker"
        );
    }

    // Creating an Installation Connection, for every provider that
    // declares one.
    for setup in pagis_connect::installation_setups() {
        let Some(connection) = setup
            .parts
            .iter()
            .find(|part| part.kind == pagis_connect::SetupKind::Connection)
        else {
            continue;
        };
        let fields: serde_json::Map<String, serde_json::Value> = connection
            .fields
            .iter()
            .map(|field| (field.key.to_string(), serde_json::json!("993")))
            .collect();
        for cookie in [daemon.cookie(), member_cookie.as_str()] {
            let (status, refused) = send_as(
                cookie,
                reqwest::Method::POST,
                product(&daemon, "/api/v1/settings/connections"),
                Some(serde_json::json!({
                    "provider": setup.provider,
                    "alias": "installation",
                    "display_name": setup.label,
                    "fields": fields,
                })),
            )
            .await;
            assert_eq!(status, 403, "{}: {refused}", setup.provider);
            assert!(
                refused["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("Administration Interface"),
                "{refused}"
            );
        }
    }

    // Authorizing or deleting one the installation set up.
    let connection_id = daemon
        .connect_installation("telnyx", serde_json::json!({ "api_key": "telnyx-key" }))
        .await;
    let (status, refused) = send(
        &daemon,
        reqwest::Method::POST,
        product(
            &daemon,
            &format!("/api/v1/settings/connections/{connection_id}/authorize"),
        ),
        Some(serde_json::json!({ "api_key": "other" })),
    )
    .await;
    assert_eq!(status, 403, "{refused}");
    let (status, refused) = send(
        &daemon,
        reqwest::Method::DELETE,
        product(
            &daemon,
            &format!("/api/v1/settings/connections/{connection_id}"),
        ),
        None,
    )
    .await;
    assert_eq!(status, 403, "{refused}");

    // The SIP sign-in, the OAuth client and the model keys have no route
    // on the product port at all.
    for (method, path) in [
        (
            reqwest::Method::PUT,
            format!("/api/v1/settings/connections/{connection_id}/sip-credential"),
        ),
        (
            reqwest::Method::GET,
            "/api/v1/settings/google-client".to_string(),
        ),
        (
            reqwest::Method::PUT,
            "/api/v1/settings/google-client".to_string(),
        ),
        (
            reqwest::Method::GET,
            "/api/v1/settings/provider-keys".to_string(),
        ),
        (
            reqwest::Method::PUT,
            "/api/v1/settings/providers/anthropic/key".to_string(),
        ),
        (
            reqwest::Method::DELETE,
            "/api/v1/settings/providers/anthropic/key".to_string(),
        ),
        (
            reqwest::Method::GET,
            "/api/v1/administration/providers".to_string(),
        ),
        (
            reqwest::Method::PUT,
            "/api/v1/administration/providers/telnyx/connection".to_string(),
        ),
    ] {
        let (status, body) = send(
            &daemon,
            method.clone(),
            product(&daemon, &path),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, 404, "{method} {path}: {body}");
    }

    // The installation's records are not a person's Connections, and the
    // list says which is which.
    let (status, page) = send(
        &daemon,
        reqwest::Method::GET,
        product(&daemon, "/api/v1/settings/connections"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let carrier = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|connection| connection["id"] == connection_id.as_str())
        .expect("the list names the Org's carrier");
    assert_eq!(carrier["installation"], true);
    assert_eq!(carrier["absent_capabilities"], serde_json::json!([]));
}

/// The default route of the Person whose Session `cookie` is.
async fn default_route_of(daemon: &TestDaemon, cookie: &str) -> serde_json::Value {
    let (status, aliases) = send_as(
        cookie,
        reqwest::Method::GET,
        product(daemon, "/api/v1/settings/model-aliases"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{aliases}");
    aliases["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|alias| alias["alias"] == "default")
        .expect("the default alias")["candidates"]
        .clone()
}

/// A server that gets its one key, an OpenAI key, in the Providers view
/// after its setup answers the first message of every Person. Nobody on
/// a server answers a model question: each default route starts on the
/// first preferred model, which the OpenAI key reaches.
#[tokio::test]
async fn a_server_with_only_an_openai_key_answers_the_first_message() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer sk-openai"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{ "id": "gpt-newest", "created": 2 }, { "id": "gpt-6-luna", "created": 1 }],
            "has_more": false,
        })))
        .mount(&provider)
        .await;
    let sse = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"ready\"}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer sk-openai"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&provider)
        .await;
    let keys = pagis_testkit::test_provider_keys(Vec::new());
    let models = Arc::new(
        pagis_agent::ModelCatalog::new(Arc::clone(&keys))
            .with_base_url(pagis_core::Provider::OpenAi, provider.uri()),
    );
    let brain = Arc::new(
        pagis_agent::RouterBrain::new(Arc::clone(&keys), models)
            .with_base_url(pagis_core::Provider::OpenAi, provider.uri()),
    );
    let Some(daemon) = TestDaemon::start_on_postgres_with(TestDaemonOptions {
        brain,
        keys,
        model_list_base_url: Some(provider.uri()),
        ..TestDaemonOptions::default()
    })
    .await
    else {
        return;
    };
    let setup = client()
        .post(format!("{}/api/v1/setup", daemon.administration_base_url))
        .json(&serde_json::json!({ "email": "ada@example.com", "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(setup.status(), 200);
    let lin = member(&daemon).await;

    let (status, stored) = daemon
        .set_up_provider(
            "openai",
            "key",
            serde_json::json!({ "api_key": "sk-openai" }),
        )
        .await;
    assert_eq!(status, 200, "{stored}");

    for cookie in [daemon.cookie(), lin.as_str()] {
        assert_eq!(
            default_route_of(&daemon, cookie).await,
            serde_json::json!(["openai/gpt-6-luna"])
        );
    }
    let (status, sent) = send(
        &daemon,
        reqwest::Method::POST,
        product(
            &daemon,
            &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
        ),
        Some(serde_json::json!({ "pending_id": "first", "text": "Hello Pixie" })),
    )
    .await;
    assert_eq!(status, 201, "{sent}");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (_, timeline) = send(
            &daemon,
            reqwest::Method::GET,
            product(
                &daemon,
                &format!("/api/v1/channels/{}/messages", daemon.dm_channel_id),
            ),
            None,
        )
        .await;
        let answered =
            timeline["items"].as_array().unwrap().iter().any(|message| {
                message["author_kind"] == "agent" && message["text_content"] == "ready"
            });
        if answered {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Pixie did not answer: {timeline}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
