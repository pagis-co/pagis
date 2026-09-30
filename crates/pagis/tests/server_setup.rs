//! The server's own first run.
//!
//! A server has an Org and a Workspace from its boot, and nobody who can
//! sign in. Two things close that gap and both write the same rows: the
//! deployment's environment, and the first-run route set.
//!
//! The first-run routes answer only where the installation holds no
//! Client Credential, which is a server: a daemon started without the
//! local flag (ADR-0025). The write answers on the Administration Port
//! alone, and the product port keeps the read. A server binds loopback
//! behind its proxy here, so every body runs on any machine.

use std::sync::Arc;

use pagis_core::{Provider, SecretStore, UserRole};
use pagis_testkit::{TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;

/// The origin the documented deployment's reverse proxy answers on. The
/// daemon behind it binds loopback, as `deploy/compose.yaml` does.
const PUBLIC_ORIGIN: &str = "https://pagis.example";

const PASSWORD: &str = "correct horse battery";

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// A server: a daemon started without the local flag, which holds no
/// Client Credential and runs on Postgres. It binds loopback behind its
/// proxy, which is what the documented compose deployment does. `None`
/// when Docker is not reachable.
async fn server() -> Option<(TestDaemon, Arc<pagis_core::MemorySecretStore>)> {
    let secrets = Arc::new(pagis_core::MemorySecretStore::default());
    let daemon = TestDaemon::start_on_postgres_with(TestDaemonOptions {
        public_origin: PUBLIC_ORIGIN.to_string(),
        trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
        keys: Arc::new(pagis_core::ProviderKeys::with_env(
            |_| None,
            Default::default(),
            Arc::clone(&secrets) as _,
        )),
        secrets: Arc::clone(&secrets) as _,
        ..TestDaemonOptions::default()
    })
    .await?;
    assert!(
        daemon.booted.client_credential.is_none(),
        "a server holds no Client Credential"
    );
    Some((daemon, secrets))
}

/// The administrator of the one Org.
async fn administrator(daemon: &TestDaemon) -> pagis_core::User {
    let org = daemon
        .stores()
        .orgs
        .list()
        .await
        .expect("list the orgs")
        .into_iter()
        .next()
        .expect("the seeded org");
    daemon
        .stores()
        .users
        .list_by_org(&org.id)
        .await
        .expect("list the people")
        .into_iter()
        .find(|person| person.role == UserRole::Administrator)
        .expect("the seeded administrator")
}

/// Post Server Setup to the listener at `origin`.
async fn post_setup(origin: &str, body: serde_json::Value) -> reqwest::Response {
    client()
        .post(format!("{origin}/api/v1/setup"))
        .json(&body)
        .send()
        .await
        .unwrap()
}

/// The `Cookie` header of the Session an answer opens, or `None` when it
/// opens none.
fn session_cookie(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(&format!("{}=", pagis_server::SESSION_COOKIE)))
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
}

/// Every Session of the installation that has not expired.
async fn live_sessions(daemon: &TestDaemon) -> Vec<pagis_core::Session> {
    daemon
        .stores()
        .sessions
        .list_live(pagis_core::now_ms())
        .await
        .expect("list the Sessions")
}

/// Every provider key the secret store holds, in the provider order.
fn stored_keys(secrets: &pagis_core::MemorySecretStore) -> Vec<(Provider, String)> {
    pagis_core::PROVIDERS
        .into_iter()
        .filter_map(|provider| {
            secrets
                .get(provider.secret_name())
                .expect("read the secret store")
                .map(|key| (provider, key))
        })
        .collect()
}

/// A local installation never answers the first-run routes. Its seeded
/// person has no password on purpose and signs in with the Client
/// Credential, so a route that made an administrator would stay open for
/// ever.
#[tokio::test]
async fn a_local_installation_answers_no_first_run_route() {
    let daemon = TestDaemon::start().await;

    for response in [
        client()
            .get(format!("{}/api/v1/setup", daemon.base_url))
            .send()
            .await
            .unwrap(),
        client()
            .get(format!("{}/api/v1/setup", daemon.administration_base_url))
            .send()
            .await
            .unwrap(),
        post_setup(
            &daemon.administration_base_url,
            serde_json::json!({
                "email": "mallory@example.com",
                "password": PASSWORD,
            }),
        )
        .await,
    ] {
        assert_eq!(response.status(), StatusCode::GONE);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "setup_complete");
    }

    // And the seeded person is still the administrator with no password,
    // so the local installation is unchanged.
    let person = administrator(&daemon).await;
    assert_eq!(person.password_hash, None);
    assert_eq!(person.email, None);
}

/// The first-run flow makes the installation's administrator, keeps the
/// provider keys, signs the person in, and answers nothing from then on.
#[tokio::test]
async fn the_first_run_flow_makes_one_administrator_and_then_answers_nothing() {
    let Some((daemon, secrets)) = server().await else {
        return;
    };

    // While it is open the read says what the installation may take.
    let state: serde_json::Value = client()
        .get(format!("{}/api/v1/setup", daemon.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        state["providers"].as_array().expect("providers").len(),
        3,
        "{state}"
    );
    // The product port's sign-in page names where the first
    // Administrator is made.
    assert_eq!(
        state["administration_origin"],
        daemon.administration_base_url.as_str(),
        "{state}"
    );
    assert!(
        state["configured_providers"]
            .as_array()
            .expect("configured")
            .is_empty(),
        "{state}"
    );

    let response = post_setup(
        &daemon.administration_base_url,
        serde_json::json!({
            "email": "Ada@Example.COM",
            "password": PASSWORD,
            "name": "Ada",
            "provider_keys": { "anthropic": "sk-installation" },
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = session_cookie(&response).expect("the answer signs the administrator in");
    let person: serde_json::Value = response.json().await.unwrap();
    assert_eq!(person["role"], "administrator");
    assert_eq!(person["email"], "ada@example.com");
    assert_eq!(person["name"], "Ada");

    // A working model route: the key is the installation's, and the
    // administrator's own read names it.
    assert_eq!(
        secrets.get(Provider::Anthropic.secret_name()).unwrap(),
        Some("sk-installation".to_string())
    );
    let setups: serde_json::Value = client()
        .get(format!(
            "{}/api/v1/administration/providers",
            daemon.administration_base_url
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let anthropic = setups["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["provider"] == "anthropic")
        .expect("anthropic");
    assert_eq!(anthropic["parts"][0]["configured"], true);
    // The flow makes the seeded person the administrator, and no second
    // one.
    let roster: serde_json::Value = client()
        .get(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        roster["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter(|person| person["role"] == UserRole::Administrator.as_str())
            .count(),
        1,
        "{roster}"
    );

    // The flow is spent, on both methods.
    for response in [
        client()
            .get(format!("{}/api/v1/setup", daemon.base_url))
            .send()
            .await
            .unwrap(),
        post_setup(
            &daemon.administration_base_url,
            serde_json::json!({
                "email": "mallory@example.com",
                "password": "another long password",
            }),
        )
        .await,
    ] {
        assert_eq!(response.status(), StatusCode::GONE);
    }

    // The password the flow wrote is the way in from now on.
    let signed_in = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({ "email": "ada@example.com", "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(signed_in.status(), StatusCode::OK);
}

/// The product port takes no Server Setup. It faces the internet, and on
/// a Server that nobody can sign in to yet, a write there would give the
/// first Administrator and the installation's provider keys to the first
/// person who posts. The same write on the Administration Port, which
/// binds loopback, makes the Administrator and signs them in.
#[tokio::test]
async fn only_the_administration_port_makes_the_first_administrator() {
    let Some((daemon, secrets)) = server().await else {
        return;
    };

    let refused = post_setup(
        &daemon.base_url,
        serde_json::json!({
            "email": "mallory@example.com",
            "password": PASSWORD,
            "provider_keys": { "anthropic": "sk-mallory" },
        }),
    )
    .await;

    assert_ne!(refused.status(), StatusCode::OK);
    assert_eq!(session_cookie(&refused), None);
    let person = administrator(&daemon).await;
    assert_eq!(person.password_hash, None);
    assert_eq!(person.email, None);
    assert_eq!(stored_keys(&secrets), Vec::new());

    let made = post_setup(
        &daemon.administration_base_url,
        serde_json::json!({
            "email": "ada@example.com",
            "password": PASSWORD,
            "provider_keys": { "anthropic": "sk-installation" },
        }),
    )
    .await;

    assert_eq!(made.status(), StatusCode::OK);
    let cookie = session_cookie(&made).expect("the answer signs the administrator in");
    let me = client()
        .get(format!("{}/api/v1/user", daemon.base_url))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let me: serde_json::Value = me.json().await.unwrap();
    assert_eq!(me["email"], "ada@example.com");
    assert_eq!(me["role"], "administrator");
    assert!(administrator(&daemon).await.password_hash.is_some());
    assert_eq!(
        stored_keys(&secrets),
        vec![(Provider::Anthropic, "sk-installation".to_string())]
    );
}

/// Two Server Setups at once make one Administrator. Both can find
/// nobody who can sign in, and the claim in the store lets exactly one of
/// them write. The other answers `410 Gone`, opens no Session and keeps
/// none of its provider keys.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_setups_at_once_make_one_administrator() {
    let Some((daemon, secrets)) = server().await else {
        return;
    };
    // The harness signs the seeded person in before the test starts.
    let before: Vec<pagis_core::SessionId> = live_sessions(&daemon)
        .await
        .into_iter()
        .map(|session| session.id)
        .collect();
    let requests = [
        (
            "ada@example.com",
            vec![
                (Provider::Anthropic, "sk-ada"),
                (Provider::OpenAi, "sk-ada-openai"),
            ],
        ),
        (
            "grace@example.com",
            vec![
                (Provider::Anthropic, "sk-grace"),
                (Provider::OpenRouter, "sk-grace-openrouter"),
            ],
        ),
    ];

    let barrier = Arc::new(tokio::sync::Barrier::new(requests.len()));
    let sent: Vec<_> = requests
        .iter()
        .map(|(email, keys)| {
            let barrier = Arc::clone(&barrier);
            let request = client()
                .post(format!("{}/api/v1/setup", daemon.administration_base_url))
                .json(&serde_json::json!({
                    "email": email,
                    "password": PASSWORD,
                    "provider_keys": keys
                        .iter()
                        .map(|(provider, key)| (provider.id(), *key))
                        .collect::<std::collections::BTreeMap<_, _>>(),
                }));
            tokio::spawn(async move {
                barrier.wait().await;
                let response = request.send().await.expect("the setup answers");
                (response.status(), session_cookie(&response))
            })
        })
        .collect();
    let mut answers = Vec::new();
    for request in sent {
        answers.push(request.await.expect("the request task ends"));
    }

    let winners: Vec<usize> = (0..answers.len())
        .filter(|at| answers[*at].0 == StatusCode::OK)
        .collect();
    assert_eq!(winners.len(), 1, "exactly one setup wins: {answers:?}");
    let (winner, loser) = (winners[0], 1 - winners[0]);
    let cookie = answers[winner].1.clone().expect("the winner is signed in");
    assert_eq!(answers[loser], (StatusCode::GONE, None), "{answers:?}");

    // One new Session, and it is the winner's.
    let opened: Vec<pagis_core::Session> = live_sessions(&daemon)
        .await
        .into_iter()
        .filter(|session| !before.contains(&session.id))
        .collect();
    assert_eq!(opened.len(), 1, "{opened:?}");
    let secret = cookie
        .split_once('=')
        .map(|(_, secret)| secret)
        .expect("a cookie has a value");
    assert_eq!(opened[0].token_hash, pagis_server::hash_secret(secret));

    // The address and the provider keys are the winner's, and no key of
    // the loser is kept.
    let (email, keys) = &requests[winner];
    assert_eq!(administrator(&daemon).await.email.as_deref(), Some(*email));
    assert_eq!(
        stored_keys(&secrets),
        keys.iter()
            .map(|(provider, key)| (*provider, (*key).to_string()))
            .collect::<Vec<_>>()
    );
}

/// A Server Setup that finds an Administrator with a password answers
/// `410 Gone` on the Administration Port. It opens no Session and keeps
/// none of its provider keys: somebody can sign in already, and the
/// installation's keys are theirs to change.
#[tokio::test]
async fn a_setup_that_finds_an_administrator_with_a_password_changes_nothing() {
    let Some((daemon, secrets)) = server().await else {
        return;
    };
    secrets
        .set(Provider::Anthropic.secret_name(), "sk-installation")
        .expect("keep the installation's key");
    let person = administrator(&daemon).await;
    daemon
        .stores()
        .users
        .set_email_and_password(
            &person.id,
            "ada@example.com",
            &pagis_server::hash_password(PASSWORD).expect("hash the password"),
            pagis_core::now_ms(),
        )
        .await
        .expect("give the administrator a password");

    let refused = post_setup(
        &daemon.administration_base_url,
        serde_json::json!({
            "email": "mallory@example.com",
            "password": "another long password",
            "provider_keys": { "anthropic": "sk-mallory", "openai": "sk-mallory" },
        }),
    )
    .await;

    assert_eq!(refused.status(), StatusCode::GONE);
    assert_eq!(session_cookie(&refused), None);
    let body: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(body["error"]["code"], "setup_complete");
    assert_eq!(
        stored_keys(&secrets),
        vec![(Provider::Anthropic, "sk-installation".to_string())]
    );
    assert_eq!(
        administrator(&daemon).await.email.as_deref(),
        Some("ada@example.com")
    );
}

/// No answer of the API goes into a browser cache, on either port. A
/// browser that kept the `410` of one installation's closed setup would
/// show it to a new server at the same address, which then offers only a
/// sign-in form that nobody can use.
#[tokio::test]
async fn the_setup_read_is_never_cached() {
    let Some((daemon, _secrets)) = server().await else {
        return;
    };
    let origins = [&daemon.base_url, &daemon.administration_base_url];
    for origin in origins {
        let open = client()
            .get(format!("{origin}/api/v1/setup"))
            .send()
            .await
            .unwrap();
        assert_eq!(open.status(), StatusCode::OK, "{origin}");
        assert_eq!(open.headers()["cache-control"], "no-store", "{origin}");
    }

    let made = post_setup(
        &daemon.administration_base_url,
        serde_json::json!({ "email": "ada@example.com", "password": PASSWORD }),
    )
    .await;
    assert_eq!(made.status(), StatusCode::OK);
    assert_eq!(made.headers()["cache-control"], "no-store");

    for origin in origins {
        let closed = client()
            .get(format!("{origin}/api/v1/setup"))
            .send()
            .await
            .unwrap();
        assert_eq!(closed.status(), StatusCode::GONE, "{origin}");
        assert_eq!(closed.headers()["cache-control"], "no-store", "{origin}");
    }
}

/// A server refuses an address that is not one and a password nobody
/// could have meant, and it stays open afterwards.
#[tokio::test]
async fn the_first_run_flow_refuses_a_bad_address_or_a_short_password() {
    let Some((daemon, _secrets)) = server().await else {
        return;
    };

    for body in [
        serde_json::json!({ "email": "not-an-address", "password": PASSWORD }),
        serde_json::json!({ "email": "ada@example.com", "password": "short" }),
        serde_json::json!({
            "email": "ada@example.com",
            "password": PASSWORD,
            "provider_keys": { "no-such": "sk" },
        }),
        serde_json::json!({
            "email": "ada@example.com",
            "password": PASSWORD,
            "provider_keys": { "anthropic": "  " },
        }),
    ] {
        let response = post_setup(&daemon.administration_base_url, body.clone()).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body} was accepted"
        );
    }

    // Still open, because nothing was written.
    assert_eq!(
        client()
            .get(format!("{}/api/v1/setup", daemon.base_url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

/// The deployment's own setup: the environment names the administrator
/// and the provider keys, and it is read once. A later start finds
/// somebody who can sign in and changes nothing, so the variables may
/// stay in the deployment's configuration for ever.
#[tokio::test]
async fn the_environment_makes_the_first_administrator_once() {
    let daemon = TestDaemon::start().await;
    let secrets = pagis_core::MemorySecretStore::default();
    let environment = |name: &str| match name {
        "PAGIS_ADMIN_EMAIL" => Some("Ada@Example.COM".to_string()),
        "PAGIS_ADMIN_PASSWORD" => Some(PASSWORD.to_string()),
        "ANTHROPIC_API_KEY" => Some("sk-deployment".to_string()),
        _ => None,
    };

    let made = pagis_server::setup::from_environment(daemon.stores(), &secrets, environment, 1_000)
        .await
        .expect("read the environment");

    assert!(made);
    let person = administrator(&daemon).await;
    assert_eq!(person.email.as_deref(), Some("ada@example.com"));
    let hash = person
        .password_hash
        .clone()
        .expect("a password was written");
    assert!(hash.starts_with("$argon2id$"), "{hash}");
    assert_eq!(
        secrets.get(Provider::Anthropic.secret_name()).unwrap(),
        Some("sk-deployment".to_string())
    );

    // A second start changes nothing, and a password somebody changed
    // meanwhile is not overwritten.
    daemon
        .stores()
        .users
        .set_password_hash(&person.id, Some("$argon2id$their-own"), 2_000)
        .await
        .expect("the person changes their password");
    let again =
        pagis_server::setup::from_environment(daemon.stores(), &secrets, environment, 3_000)
            .await
            .expect("read the environment again");

    assert!(!again);
    assert_eq!(
        administrator(&daemon).await.password_hash.as_deref(),
        Some("$argon2id$their-own")
    );
}

/// An environment that names no administrator makes none, and it is not
/// an error: that is every local installation.
#[tokio::test]
async fn an_environment_that_names_nobody_makes_nobody() {
    let daemon = TestDaemon::start().await;
    let secrets = pagis_core::MemorySecretStore::default();

    let made = pagis_server::setup::from_environment(daemon.stores(), &secrets, |_| None, 1_000)
        .await
        .expect("read the environment");

    assert!(!made);
    assert_eq!(administrator(&daemon).await.password_hash, None);
}

/// The documented compose deployment: the daemon binds loopback behind a
/// reverse proxy on the same host, and a browser reaches it at the
/// proxy's name. The daemon starts without the local flag, so it is a
/// server. It holds no Client Credential, so its first-run flow answers,
/// the credential trade and the sign-in link answer `404`, and the
/// binary prints no link.
#[tokio::test]
async fn a_server_behind_a_proxy_on_loopback_holds_no_credential() {
    let Some(daemon) = TestDaemon::start_on_postgres_with(TestDaemonOptions {
        bind: std::net::Ipv4Addr::LOCALHOST.into(),
        public_origin: PUBLIC_ORIGIN.to_string(),
        trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
        ..TestDaemonOptions::default()
    })
    .await
    else {
        return;
    };

    // No credential was written, and none is on disk to carry away.
    assert_eq!(daemon.booted.client_credential, None);
    assert!(
        !daemon
            .booted
            .home
            .join(pagis::CLIENT_CREDENTIAL_FILE)
            .exists()
    );

    // The first-run flow is open, because nobody holds a password yet.
    assert_eq!(
        client()
            .get(format!("{}/api/v1/setup", daemon.base_url))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // The credential trade is not a way in.
    assert_eq!(
        client()
            .post(format!("{}/api/v1/sessions/client", daemon.base_url))
            .json(&serde_json::json!({ "credential": "anything at all" }))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    // Neither is a link, even one this installation's own store minted.
    let administrator = administrator(&daemon).await;
    let url = pagis_server::mint_sign_in_link(
        daemon.stores().sign_in_links.as_ref(),
        &administrator.id,
        &daemon.base_url,
        pagis_core::now_ms(),
    )
    .await
    .expect("mint a link");
    assert_eq!(
        client().get(&url).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    // And the runtime identity handshake answers nothing.
    assert_eq!(
        client()
            .get(format!(
                "{}/api/v1/runtime/identity?challenge={}",
                daemon.base_url,
                "0".repeat(64)
            ))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    // The start banner prints no link, because a link needs the Client
    // Credential that a server does not hold.
    assert_eq!(daemon.booted.client_credential, None);
}

/// A secret store that refuses every write, which is a keychain that is
/// locked or a key file the daemon cannot write.
struct RefusingSecrets;

impl SecretStore for RefusingSecrets {
    fn get(&self, _name: &str) -> Result<Option<String>, pagis_core::SecretError> {
        Ok(None)
    }

    fn set(&self, _name: &str, _value: &str) -> Result<(), pagis_core::SecretError> {
        Err(pagis_core::SecretError(
            "the keychain is locked".to_string(),
        ))
    }

    fn get_or_insert(&self, _name: &str, _value: &str) -> Result<String, pagis_core::SecretError> {
        Err(pagis_core::SecretError(
            "the keychain is locked".to_string(),
        ))
    }

    fn delete(&self, _name: &str) -> Result<(), pagis_core::SecretError> {
        Ok(())
    }
}

/// A provider key the environment named and the daemon could not keep
/// stops the boot.
///
/// A boot that logged it and passed over it would leave a server with an
/// administrator who can sign in, no model route, and no sign that
/// anything went wrong. The person who reads the logs of a failed boot is
/// the person who can fix it, so the failure names the key and no
/// administrator is made.
#[tokio::test]
async fn a_provider_key_the_daemon_cannot_keep_stops_the_setup() {
    let daemon = TestDaemon::start().await;

    let error = pagis_server::setup::from_environment(
        daemon.stores(),
        &RefusingSecrets,
        |name| match name {
            "PAGIS_ADMIN_EMAIL" => Some("ada@example.com".to_string()),
            "PAGIS_ADMIN_PASSWORD" => Some(PASSWORD.to_string()),
            "ANTHROPIC_API_KEY" => Some("sk-deployment".to_string()),
            _ => None,
        },
        1_000,
    )
    .await
    .expect_err("a key that is not kept stops the setup");

    assert!(error.to_string().contains("anthropic"), "{error}");
    // Nothing was written, so the next start with a working store makes
    // the administrator.
    let person = administrator(&daemon).await;
    assert_eq!(person.password_hash, None);
    assert_eq!(person.email, None);
}
