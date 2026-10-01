//! The Sign-In Link of the Public Origin (ADR-0028): a signed-in Person,
//! an Administrator and `pagis pair` each make one, and a client on any
//! machine trades its secret for a Session, once.

use pagis_core::{CLIENT_LINK_LIFETIME_MS, INVITE_LINK_LIFETIME_MS, User, UserRole, now_ms};
use pagis_server::SESSION_COOKIE;
use pagis_testkit::TestDaemon;
use reqwest::StatusCode;

const SAFARI_ON_MACOS: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15";
const CHROME_ON_ANDROID: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/139.0.0.0 Mobile Safari/537.36";

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build a client")
}

/// The `Cookie` header of the Session that an answer sets, or `None`
/// when it sets none.
fn session_cookie(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            let pair = value.split(';').next()?;
            let (name, secret) = pair.split_once('=')?;
            (name.trim() == SESSION_COOKIE && !secret.is_empty()).then(|| pair.trim().to_string())
        })
}

/// The secret of a link: the fragment of its URL.
fn secret_of(url: &str) -> &str {
    url.split_once('#')
        .unwrap_or_else(|| panic!("{url} holds no fragment"))
        .1
}

/// Trade a secret at the route the `/sign-in` page posts to, as the
/// browser with `user_agent`.
async fn trade(daemon: &TestDaemon, secret: &str, user_agent: &str) -> reqwest::Response {
    client()
        .post(format!("{}/api/v1/sessions/link", daemon.base_url))
        .header(reqwest::header::USER_AGENT, user_agent)
        .json(&serde_json::json!({ "secret": secret, "timezone": "Europe/Berlin" }))
        .send()
        .await
        .expect("the daemon answers the trade")
}

/// Ask for a link for one more client, as the Session of `cookie`.
async fn client_link(daemon: &TestDaemon, cookie: &str) -> serde_json::Value {
    let response = client()
        .post(format!("{}/api/v1/settings/sign-in-links", daemon.base_url))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json().await.unwrap()
}

/// Who the Session of `cookie` signs in, or the status of the refusal.
async fn user_of(daemon: &TestDaemon, cookie: &str) -> Result<serde_json::Value, StatusCode> {
    let response = client()
        .get(format!("{}/api/v1/user", daemon.base_url))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    match response.status() {
        StatusCode::OK => Ok(response.json().await.unwrap()),
        status => Err(status),
    }
}

/// A second Person of the Org, with no password and no Session.
async fn member(daemon: &TestDaemon, email: &str) -> User {
    let org = daemon.stores().orgs.list().await.unwrap().remove(0);
    let person = User {
        email: Some(email.to_string()),
        name: Some("Grace".to_string()),
        ..User::new(org.id, UserRole::Member, now_ms())
    };
    daemon.stores().users.create(&person).await.unwrap();
    person
}

/// A signed-in Person makes a link for one more client. It starts at the
/// Public Origin, it lives five minutes, and its QR code travels beside
/// it. The client that trades it gets a Session of the same Person,
/// named for its browser, and the link signs nobody in a second time.
#[tokio::test]
async fn a_person_signs_one_more_client_in_with_a_link_once() {
    let daemon = TestDaemon::start().await;
    let before = now_ms();

    let link = client_link(&daemon, daemon.cookie()).await;

    let url = link["url"].as_str().unwrap();
    assert!(
        url.starts_with(&format!("{}/sign-in#", daemon.public_origin)),
        "{url}"
    );
    let expires_at = link["expires_at"].as_i64().unwrap();
    assert!(
        (before + CLIENT_LINK_LIFETIME_MS..=now_ms() + CLIENT_LINK_LIFETIME_MS)
            .contains(&expires_at),
        "the link lives five minutes: {link}"
    );
    assert!(link["qr_svg"].as_str().unwrap().contains("<svg"), "{link}");

    // Opening the address spends nothing: the secret is in the fragment,
    // which a browser never sends, and the page is the Product App.
    client()
        .get(url.split('#').next().unwrap())
        .send()
        .await
        .unwrap();

    let first = trade(&daemon, secret_of(url), SAFARI_ON_MACOS).await;
    assert_eq!(first.status(), StatusCode::OK);
    let cookie = session_cookie(&first).expect("the trade sets the session cookie");
    let signed_in: serde_json::Value = first.json().await.unwrap();
    assert_eq!(signed_in["id"], daemon.user_id.to_string());
    assert_eq!(
        user_of(&daemon, &cookie).await.unwrap()["id"],
        daemon.user_id.to_string()
    );

    let second = trade(&daemon, secret_of(url), SAFARI_ON_MACOS).await;
    assert_eq!(
        second.status(),
        StatusCode::UNAUTHORIZED,
        "a link is good for one use"
    );
    assert_eq!(session_cookie(&second), None);
}

/// The Session that a link opens is named for the browser that traded
/// it, so the Person tells it from their other Sessions.
#[tokio::test]
async fn the_session_of_a_link_is_named_for_its_browser() {
    let daemon = TestDaemon::start().await;
    let link = client_link(&daemon, daemon.cookie()).await;

    let traded = trade(
        &daemon,
        secret_of(link["url"].as_str().unwrap()),
        CHROME_ON_ANDROID,
    )
    .await;
    let cookie = session_cookie(&traded).expect("the trade signs in");

    let sessions: serde_json::Value = client()
        .get(format!("{}/api/v1/settings/sessions", daemon.base_url))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let current = sessions["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["current"] == true)
        .unwrap_or_else(|| panic!("the list marks the current Session: {sessions}"));
    assert_eq!(current["client_name"], "Chrome on Android");
    assert_eq!(current["client_kind"], "browser");
}

/// A link past its expiry, and a secret that names no link, answer the
/// same refusal and set no cookie.
#[tokio::test]
async fn an_expired_link_and_an_unknown_secret_are_refused() {
    let daemon = TestDaemon::start().await;
    let expired = pagis_server::mint_public_origin_link(
        daemon.stores().sign_in_links.as_ref(),
        &daemon.user_id,
        &daemon.public_origin,
        CLIENT_LINK_LIFETIME_MS,
        now_ms() - CLIENT_LINK_LIFETIME_MS - 1_000,
    )
    .await
    .unwrap();

    let late = trade(&daemon, secret_of(&expired.url), SAFARI_ON_MACOS).await;
    assert_eq!(late.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(session_cookie(&late), None);
    let late: serde_json::Value = late.json().await.unwrap();

    let unknown = trade(&daemon, &"0".repeat(64), SAFARI_ON_MACOS).await;
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
    let unknown: serde_json::Value = unknown.json().await.unwrap();
    assert_eq!(late, unknown, "the refusal says nothing about the link");
}

/// The start link opens through its own route alone, and a link of the
/// Public Origin through its own route alone. A link offered at the
/// other route is refused and stays good for the route it was made for.
#[tokio::test]
async fn each_kind_of_link_opens_through_its_own_route_alone() {
    let daemon = TestDaemon::start().await;

    let start = pagis::start_link(daemon.stores(), &daemon.base_url)
        .await
        .unwrap();
    let start_secret = start.rsplit('/').next().unwrap();
    let refused = trade(&daemon, start_secret, SAFARI_ON_MACOS).await;
    assert_eq!(
        refused.status(),
        StatusCode::UNAUTHORIZED,
        "a start link is not a link of the Public Origin"
    );
    let opened = client().get(&start).send().await.unwrap();
    assert_eq!(opened.status(), StatusCode::SEE_OTHER);
    assert!(session_cookie(&opened).is_some());

    let public = client_link(&daemon, daemon.cookie()).await;
    let public_secret = secret_of(public["url"].as_str().unwrap());
    let refused = client()
        .get(format!(
            "{}/api/v1/sessions/link/{public_secret}",
            daemon.base_url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        refused.status(),
        StatusCode::UNAUTHORIZED,
        "a link of the Public Origin is not a start link"
    );
    assert_eq!(session_cookie(&refused), None);
    let traded = trade(&daemon, public_secret, SAFARI_ON_MACOS).await;
    assert_eq!(traded.status(), StatusCode::OK);
}

/// A disabled Person signs in to nothing, also with a link made for them
/// before the Administrator disabled the account.
#[tokio::test]
async fn the_link_of_a_disabled_person_is_refused() {
    let daemon = TestDaemon::start().await;
    let grace = member(&daemon, "grace@example.com").await;
    let link = pagis_server::mint_public_origin_link(
        daemon.stores().sign_in_links.as_ref(),
        &grace.id,
        &daemon.public_origin,
        INVITE_LINK_LIFETIME_MS,
        now_ms(),
    )
    .await
    .unwrap();
    let now = now_ms();
    daemon
        .stores()
        .users
        .set_disabled(&grace.id, Some(now), now)
        .await
        .unwrap();

    let refused = trade(&daemon, secret_of(&link.url), SAFARI_ON_MACOS).await;

    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(session_cookie(&refused), None);
}

/// The trade counts each refusal against the source address with the
/// limits of a password sign-in. Five wrong secrets lock the address,
/// for a good link and for a password as well.
#[tokio::test]
async fn refused_links_lock_the_address_as_wrong_passwords_do() {
    let daemon = TestDaemon::start().await;
    let good = client_link(&daemon, daemon.cookie()).await;

    for attempt in 0..5 {
        let refused = trade(&daemon, &format!("{attempt:064}"), SAFARI_ON_MACOS).await;
        assert_eq!(
            refused.status(),
            StatusCode::UNAUTHORIZED,
            "attempt {attempt} is answered"
        );
    }

    let locked = trade(
        &daemon,
        secret_of(good["url"].as_str().unwrap()),
        SAFARI_ON_MACOS,
    )
    .await;
    assert_eq!(locked.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(session_cookie(&locked), None);
    let locked: serde_json::Value = locked.json().await.unwrap();
    assert_eq!(locked["error"]["code"], "rate_limited");

    let password = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({ "email": "nobody@example.com", "password": "no" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        password.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the address is locked for a password too"
    );
}

/// An Administrator who creates a Person gets the Person's invite: a
/// link good for seven days, which signs the Person in with no password.
#[tokio::test]
async fn creating_a_person_answers_their_invite() {
    let daemon = TestDaemon::start().await;
    let before = now_ms();

    let response = client()
        .post(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({ "email": "grace@example.com", "name": "Grace" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: serde_json::Value = response.json().await.unwrap();

    let invite = &created["invite"];
    let url = invite["url"].as_str().unwrap();
    assert!(
        url.starts_with(&format!("{}/sign-in#", daemon.public_origin)),
        "the invite starts at the Public Origin, not at this port: {url}"
    );
    let expires_at = invite["expires_at"].as_i64().unwrap();
    assert!(
        (before + INVITE_LINK_LIFETIME_MS..=now_ms() + INVITE_LINK_LIFETIME_MS)
            .contains(&expires_at),
        "the invite lives seven days: {invite}"
    );
    assert!(invite["qr_svg"].as_str().unwrap().contains("<svg"));

    let traded = trade(&daemon, secret_of(url), SAFARI_ON_MACOS).await;
    assert_eq!(traded.status(), StatusCode::OK);
    let grace: serde_json::Value = traded.json().await.unwrap();
    assert_eq!(grace["id"], created["person"]["id"]);
    assert_eq!(grace["email"], "grace@example.com");

    // The first sign-in takes the timezone of the browser.
    let workspace = daemon
        .stores()
        .workspaces
        .get(&pagis_core::WorkspaceId::from(
            created["person"]["workspace_id"]
                .as_str()
                .unwrap()
                .to_string(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(workspace.timezone, "Europe/Berlin");

    // With no password given, there is no password to sign in with.
    let password = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({ "email": "grace@example.com", "password": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(password.status(), StatusCode::UNAUTHORIZED);
}

/// A password at creation stays a way in beside the invite.
#[tokio::test]
async fn a_person_created_with_a_password_signs_in_with_it_too() {
    let daemon = TestDaemon::start().await;

    let response = client()
        .post(format!(
            "{}/api/v1/administration/people",
            daemon.administration_base_url
        ))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "name": "Grace",
            "password": "correct horse battery",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: serde_json::Value = response.json().await.unwrap();
    assert!(created["invite"]["url"].is_string(), "{created}");

    let password = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "password": "correct horse battery",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(password.status(), StatusCode::OK);
}

/// An Administrator makes a new invite for a Person whose invite
/// expired. A disabled account gets none, and a Person who is not there
/// reads as absent.
#[tokio::test]
async fn an_administrator_makes_a_new_invite_for_a_person() {
    let daemon = TestDaemon::start().await;
    let grace = member(&daemon, "grace@example.com").await;
    let invite = |user_id: String| {
        client()
            .post(format!(
                "{}/api/v1/administration/people/{user_id}/sign-in-links",
                daemon.administration_base_url
            ))
            .header("cookie", daemon.cookie())
            .send()
    };

    let response = invite(grace.id.to_string()).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let link: serde_json::Value = response.json().await.unwrap();
    assert!(
        link["expires_at"].as_i64().unwrap() > now_ms() + INVITE_LINK_LIFETIME_MS - 60_000,
        "{link}"
    );
    let traded = trade(
        &daemon,
        secret_of(link["url"].as_str().unwrap()),
        SAFARI_ON_MACOS,
    )
    .await;
    assert_eq!(traded.status(), StatusCode::OK);
    let signed_in: serde_json::Value = traded.json().await.unwrap();
    assert_eq!(signed_in["id"], grace.id.to_string());

    let now = now_ms();
    daemon
        .stores()
        .users
        .set_disabled(&grace.id, Some(now), now)
        .await
        .unwrap();
    assert_eq!(
        invite(grace.id.to_string()).await.unwrap().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        invite("nobody".to_string()).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

/// `pagis pair` writes a link into the records of the installation,
/// with no API of the running daemon, at the configured Public Origin.
/// The running daemon spends it, for the first Administrator or for the
/// Person whose address the command names.
async fn pair_signs_a_client_in(daemon: TestDaemon) {
    let home = daemon.booted.home.clone();
    let config_file = home.join("config.toml");
    let mut config = pagis::Config::read_file(&config_file).unwrap();
    config.public_origin = daemon.base_url.clone();
    config.save(&config_file).unwrap();
    let grace = member(&daemon, "grace@example.com").await;
    let before = now_ms();

    let pairing = pagis::pair(&home, None, now_ms()).await.unwrap();

    assert!(
        pairing
            .url
            .starts_with(&format!("{}/sign-in#", daemon.base_url)),
        "{}",
        pairing.url
    );
    assert!(pairing.expires_at >= before + CLIENT_LINK_LIFETIME_MS);
    assert_eq!(pairing.person.id, daemon.user_id);
    let traded = trade(&daemon, secret_of(&pairing.url), SAFARI_ON_MACOS).await;
    assert_eq!(traded.status(), StatusCode::OK);
    let signed_in: serde_json::Value = traded.json().await.unwrap();
    assert_eq!(signed_in["id"], daemon.user_id.to_string());

    let for_grace = pagis::pair(&home, Some("Grace@Example.com"), now_ms())
        .await
        .unwrap();
    assert_eq!(for_grace.person.id, grace.id);
    let traded = trade(&daemon, secret_of(&for_grace.url), SAFARI_ON_MACOS).await;
    let signed_in: serde_json::Value = traded.json().await.unwrap();
    assert_eq!(signed_in["id"], grace.id.to_string());

    let unknown = pagis::pair(&home, Some("nobody@example.com"), now_ms())
        .await
        .expect_err("no Person has the address");
    assert!(
        unknown.to_string().contains("nobody@example.com"),
        "{unknown}"
    );
}

#[tokio::test]
async fn pair_signs_a_client_in_on_sqlite() {
    pair_signs_a_client_in(TestDaemon::start().await).await;
}

#[tokio::test]
async fn pair_signs_a_client_in_on_postgres() {
    let Some(daemon) = TestDaemon::start_on_postgres().await else {
        return;
    };
    pair_signs_a_client_in(daemon).await;
}

/// `pagis pair` refuses a directory that holds no installation, and
/// writes nothing there.
#[tokio::test]
async fn pair_refuses_a_directory_with_no_installation() {
    let home = tempfile::tempdir().unwrap();

    let error = pagis::pair(home.path(), None, now_ms())
        .await
        .expect_err("there is no installation");

    assert!(
        error.to_string().contains("no Pagis installation"),
        "{error}"
    );
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}

/// The binary prints the QR code and the link, which the daemon spends.
#[tokio::test]
async fn the_pair_command_prints_the_link_and_its_qr_code() {
    let daemon = TestDaemon::start().await;
    let home = daemon.booted.home.clone();
    let config_file = home.join("config.toml");
    let mut config = pagis::Config::read_file(&config_file).unwrap();
    config.public_origin = daemon.base_url.clone();
    config.save(&config_file).unwrap();

    // The daemon answers on this runtime, so the command waits on the
    // blocking pool.
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_pagis"))
            .arg("pair")
            .env("PAGIS_HOME", &home)
            .env_remove("PAGIS_PUBLIC_ORIGIN")
            .env_remove("PAGIS_DATABASE_URL")
            .output()
    })
    .await
    .unwrap()
    .expect("run pagis pair");

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains('\u{2588}'),
        "the QR code is printed:\n{stdout}"
    );
    assert!(stdout.contains("five minutes"), "{stdout}");
    let url = stdout
        .lines()
        .find(|line| line.starts_with(&format!("{}/sign-in#", daemon.base_url)))
        .unwrap_or_else(|| panic!("the link is printed:\n{stdout}"));
    let traded = trade(&daemon, secret_of(url), SAFARI_ON_MACOS).await;
    assert_eq!(traded.status(), StatusCode::OK);
}
