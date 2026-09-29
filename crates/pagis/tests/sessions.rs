//! Full-daemon identity tests: the two ways in, the way out, the
//! rate limit, the bounded password checks, the expiry, and two people
//! in one Org who see only their own roster.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_core::{
    Agent, AgentStatus, AgentStore, ClientKind, DEFAULT_MODEL_ALIAS, Org, OrgStore, Session,
    SessionId, SessionStore, User, UserRole, UserStore, Workspace, WorkspaceId, WorkspaceStore,
    now_ms,
};
use pagis_server::SESSION_COOKIE;
use pagis_storage_sqlite::{
    SqliteAgentStore, SqliteOrgStore, SqliteSessionStore, SqliteUserStore, SqliteWorkspaceStore,
};
use pagis_testkit::{Socket, TestDaemon, TestDaemonOptions};
use sqlx::SqlitePool;
use tokio_tungstenite::tungstenite::Message;

/// A client that follows no redirect, so a test reads the 303 of the
/// sign-in link itself.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build a client")
}

/// The `pagis_session` value of a `Set-Cookie` header, or `None` when the
/// answer sets none.
fn session_cookie(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            let (pair, attributes) = value.split_once(';')?;
            let (name, secret) = pair.split_once('=')?;
            (name.trim() == SESSION_COOKIE && attributes.contains("HttpOnly"))
                .then(|| secret.trim().to_string())
        })
}

/// The one Org of the installation.
async fn org(pool: &SqlitePool) -> Org {
    SqliteOrgStore::new(pool.clone())
        .list()
        .await
        .expect("list the orgs")
        .into_iter()
        .next()
        .expect("the seeded org")
}

/// A second person in the same Org, with a password, their own Workspace
/// and one Agent of their own.
async fn second_person(pool: &SqlitePool, email: &str, password: &str) -> (User, Agent) {
    let now = now_ms();
    let person = User {
        email: Some(email.to_string()),
        name: Some("Grace".to_string()),
        password_hash: Some(pagis_server::hash_password(password).expect("hash the password")),
        ..User::new(org(pool).await.id, UserRole::Member, now)
    };
    SqliteUserStore::new(pool.clone())
        .create(&person)
        .await
        .expect("create the person");
    let workspace = Workspace {
        id: WorkspaceId::generate(),
        user_id: person.id.clone(),
        name: "Grace".to_string(),
        timezone: "UTC".to_string(),
        created_at: now,
        onboarded_at: Some(now),
        chief_of_staff_agent_id: None,
        report_schedule_id: None,
    };
    SqliteWorkspaceStore::new(pool.clone())
        .create(&workspace)
        .await
        .expect("create the workspace");
    let agent = Agent {
        id: pagis_core::AgentId::generate(),
        workspace_id: workspace.id.clone(),
        name: "Ada".to_string(),
        job: "second sprite".to_string(),
        description: "The other person's own sprite.".to_string(),
        personality: "Quiet.".to_string(),
        model_alias: DEFAULT_MODEL_ALIAS.to_string(),
        avatar: Default::default(),
        voice: None,
        standing_brief: None,
        status: AgentStatus::Active,
        created_at: now,
        updated_at: now,
    };
    SqliteAgentStore::new(pool.clone())
        .create(&agent)
        .await
        .expect("create the agent");
    (person, agent)
}

#[tokio::test]
async fn a_password_sign_in_answers_the_person_and_an_http_only_cookie() {
    let daemon = TestDaemon::start().await;
    let (person, _) = second_person(daemon.pool(), "grace@example.com", "a good password").await;

    let response = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({
            "email": "Grace@Example.com",
            "password": "a good password",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let secret = session_cookie(&response).expect("the answer sets the session cookie");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["id"], person.id.to_string());
    assert_eq!(body["name"], "Grace");
    assert_eq!(body["role"], "member");

    // The cookie is what the daemon reads on every later request, and the
    // secret is nowhere in a URL.
    let user: serde_json::Value = client()
        .get(format!("{}/api/v1/user", daemon.base_url))
        .header("cookie", format!("{SESSION_COOKIE}={secret}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(user["id"], person.id.to_string());
    assert_eq!(user["email"], "grace@example.com");
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_address_answer_the_same_way() {
    let daemon = TestDaemon::start().await;
    second_person(daemon.pool(), "grace@example.com", "a good password").await;

    let wrong = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({ "email": "grace@example.com", "password": "no" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);
    let wrong: serde_json::Value = wrong.json().await.unwrap();

    let unknown = client()
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({ "email": "nobody@example.com", "password": "no" }))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 401);
    let unknown: serde_json::Value = unknown.json().await.unwrap();

    assert_eq!(wrong, unknown, "the answer says nothing about the address");
}

#[tokio::test]
async fn sign_in_is_rate_limited_and_the_lock_says_nothing_about_the_address() {
    let daemon = TestDaemon::start().await;
    second_person(daemon.pool(), "grace@example.com", "a good password").await;
    let client = client();

    for attempt in 0..5 {
        let response = client
            .post(format!("{}/api/v1/sessions", daemon.base_url))
            .json(&serde_json::json!({ "email": "grace@example.com", "password": "no" }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401, "attempt {attempt} is answered");
    }

    // The account is locked, and so is the address: the right password no
    // longer opens a Session, and an address that belongs to nobody gets
    // the same answer.
    let locked = client
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "password": "a good password",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(locked.status(), 429);
    let locked: serde_json::Value = locked.json().await.unwrap();
    assert_eq!(locked["error"]["code"], "rate_limited");
    assert!(
        !locked["error"]["message"]
            .as_str()
            .unwrap()
            .contains("grace"),
        "the message names no address: {locked}"
    );

    let other = client
        .post(format!("{}/api/v1/sessions", daemon.base_url))
        .json(&serde_json::json!({ "email": "nobody@example.com", "password": "no" }))
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), 429, "the source address is locked too");
}

/// Wrong passwords that arrive at the same time pass the limit no better
/// than attempts one after the other. The limiter counts each attempt
/// before the first await, so no more than five attempts reach the
/// password and every other attempt gets `429`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrent_burst_of_wrong_passwords_gets_no_more_refusals_than_the_limit() {
    const BURST: usize = 24;
    let daemon = TestDaemon::start().await;
    second_person(daemon.pool(), "grace@example.com", "a good password").await;
    let client = client();
    let url = format!("{}/api/v1/sessions", daemon.base_url);
    let wrong = serde_json::json!({ "email": "grace@example.com", "password": "no" });

    let statuses: Vec<u16> =
        futures::future::join_all((0..BURST).map(|_| client.post(&url).json(&wrong).send()))
            .await
            .into_iter()
            .map(|response| response.expect("the daemon answers").status().as_u16())
            .collect();

    let refused = statuses.iter().filter(|status| **status == 401).count();
    let limited = statuses.iter().filter(|status| **status == 429).count();
    assert_eq!(
        refused + limited,
        BURST,
        "every attempt is refused or limited: {statuses:?}"
    );
    assert!(
        (1..=5).contains(&refused),
        "no more than five attempts reach the password: {statuses:?}"
    );
}

/// A right password does not lift the bound. One address opens twenty
/// Sessions in the window, and its next sign-in gets `429` and no
/// cookie.
#[tokio::test]
async fn repeated_sign_ins_from_one_address_get_429_after_the_success_limit() {
    let daemon = TestDaemon::start().await;
    second_person(daemon.pool(), "grace@example.com", "a good password").await;
    let client = client();
    let sign_in = || {
        client
            .post(format!("{}/api/v1/sessions", daemon.base_url))
            .json(&serde_json::json!({
                "email": "grace@example.com",
                "password": "a good password",
            }))
            .send()
    };

    for attempt in 0..20 {
        let response = sign_in().await.unwrap();
        assert_eq!(response.status(), 200, "sign-in {attempt} opens a Session");
    }

    let limited = sign_in().await.unwrap();
    assert_eq!(limited.status(), 429);
    assert_eq!(
        session_cookie(&limited),
        None,
        "a limited sign-in sets no cookie"
    );
    let limited: serde_json::Value = limited.json().await.unwrap();
    assert_eq!(limited["error"]["code"], "rate_limited");
}

/// A daemon whose password verifier runs `check` with `permits` permits,
/// in place of argon2id with one permit for each core.
async fn daemon_verifying_with(
    permits: usize,
    check: impl Fn(&str, &str) -> bool + Send + Sync + 'static,
) -> TestDaemon {
    TestDaemon::start_with(TestDaemonOptions {
        password_verifier: Arc::new(pagis_server::PasswordVerifier::new(permits, check)),
        ..TestDaemonOptions::default()
    })
    .await
}

/// A password check that holds its verification permit until the test
/// opens the gate. It refuses every password.
#[derive(Clone, Default)]
struct Gate {
    open: Arc<(Mutex<bool>, Condvar)>,
    running: Arc<AtomicUsize>,
}

impl Gate {
    /// The check for the verifier. A check that stays closed for thirty
    /// seconds ends anyway, so a check that blocks the async worker of
    /// the test cannot stop the test forever.
    fn check(&self) -> impl Fn(&str, &str) -> bool + Send + Sync + 'static {
        let gate = self.clone();
        move |_password: &str, _hash: &str| {
            gate.running.fetch_add(1, Ordering::SeqCst);
            let (open, opened) = &*gate.open;
            let closed = open.lock().expect("gate lock");
            let _open = opened
                .wait_timeout_while(closed, Duration::from_secs(30), |open| !*open)
                .expect("gate lock");
            gate.running.fetch_sub(1, Ordering::SeqCst);
            false
        }
    }

    /// How many checks run at this moment.
    fn running(&self) -> usize {
        self.running.load(Ordering::SeqCst)
    }

    /// Wait until `count` checks run at the same time.
    async fn wait_until_running(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.running() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{count} checks do not run at the same time"));
    }

    /// Let every check end.
    fn open(&self) {
        let (open, opened) = &*self.open;
        *open.lock().expect("gate lock") = true;
        opened.notify_all();
    }
}

/// Start `count` wrong-password sign-ins for Grace, each in a task of
/// its own.
fn start_sign_ins(
    daemon: &TestDaemon,
    count: usize,
) -> Vec<tokio::task::JoinHandle<reqwest::Result<reqwest::Response>>> {
    let client = client();
    (0..count)
        .map(|_| {
            tokio::spawn(
                client
                    .post(format!("{}/api/v1/sessions", daemon.base_url))
                    .json(&serde_json::json!({ "email": "grace@example.com", "password": "no" }))
                    .send(),
            )
        })
        .collect()
}

/// A password check runs on the blocking pool and not on an async
/// worker. While checks hold every verification permit, an unrelated
/// authenticated request still answers.
#[tokio::test]
async fn an_authenticated_request_answers_while_every_verification_permit_is_held() {
    const PERMITS: usize = 2;
    let gate = Gate::default();
    let daemon = daemon_verifying_with(PERMITS, gate.check()).await;
    second_person(daemon.pool(), "grace@example.com", "a good password").await;

    let held = start_sign_ins(&daemon, PERMITS);
    gate.wait_until_running(PERMITS).await;

    let user = client()
        .get(format!("{}/api/v1/user", daemon.base_url))
        .header("cookie", daemon.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(user.status(), 200);
    assert_eq!(
        gate.running(),
        PERMITS,
        "the request answers while the checks still hold every permit"
    );

    gate.open();
    for sign_in in held {
        assert_eq!(sign_in.await.unwrap().unwrap().status(), 401);
    }
}

/// A sign-in that finds no free verification permit within the short
/// wait gets `429` and does not queue. Its password was not checked, so
/// it does not count against the attempt limit.
#[tokio::test]
async fn a_sign_in_gets_429_when_no_verification_permit_is_free() {
    const PERMITS: usize = 2;
    let gate = Gate::default();
    let daemon = daemon_verifying_with(PERMITS, gate.check()).await;
    second_person(daemon.pool(), "grace@example.com", "a good password").await;
    let client = client();
    let attempt = || {
        client
            .post(format!("{}/api/v1/sessions", daemon.base_url))
            .json(&serde_json::json!({ "email": "nobody@example.com", "password": "no" }))
            .send()
    };

    let held = start_sign_ins(&daemon, PERMITS);
    gate.wait_until_running(PERMITS).await;

    // The address has two attempts in flight, so the limiter lets a third
    // one through. The permits turn it away.
    let busy = attempt().await.unwrap();
    assert_eq!(busy.status(), 429);
    let busy: serde_json::Value = busy.json().await.unwrap();
    assert_eq!(busy["error"]["code"], "rate_limited");

    gate.open();
    for sign_in in held {
        assert_eq!(sign_in.await.unwrap().unwrap().status(), 401);
    }

    // Two refusals count against the address, and the attempt the permits
    // turned away does not. Three more reach the password.
    for attempt_number in 0..3 {
        let refused = attempt().await.unwrap();
        assert_eq!(
            refused.status(),
            401,
            "attempt {attempt_number} is answered"
        );
    }
    assert_eq!(attempt().await.unwrap().status(), 429);
}

/// An unknown address, a disabled Person and a wrong password each go
/// through one password check, with an argon2id hash of the same
/// parameters. No refusal is faster than another, and a disabled
/// Person's own hash is never checked.
#[tokio::test]
async fn an_unknown_address_a_disabled_person_and_a_wrong_password_each_check_one_hash() {
    let checked = Arc::new(Mutex::new(Vec::<String>::new()));
    let daemon = daemon_verifying_with(1, {
        let checked = Arc::clone(&checked);
        move |_password: &str, hash: &str| {
            checked.lock().expect("check lock").push(hash.to_string());
            false
        }
    })
    .await;
    let (grace, _) = second_person(daemon.pool(), "grace@example.com", "a good password").await;
    let now = now_ms();
    let ada = User {
        email: Some("ada@example.com".to_string()),
        password_hash: Some(pagis_server::hash_password("her password").unwrap()),
        disabled_at: Some(now),
        ..User::new(org(daemon.pool()).await.id, UserRole::Member, now)
    };
    SqliteUserStore::new(daemon.pool().clone())
        .create(&ada)
        .await
        .expect("create the disabled person");

    let mut answers = Vec::new();
    for (email, password) in [
        ("nobody@example.com", "no"),
        ("ada@example.com", "her password"),
        ("grace@example.com", "no"),
    ] {
        let response = client()
            .post(format!("{}/api/v1/sessions", daemon.base_url))
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401, "{email} is refused");
        answers.push(response.json::<serde_json::Value>().await.unwrap());
    }
    assert!(
        answers.iter().all(|answer| *answer == answers[0]),
        "each refusal answers the same: {answers:?}"
    );

    let checked = checked.lock().expect("check lock").clone();
    assert_eq!(
        checked.len(),
        3,
        "each attempt checks one hash: {checked:?}"
    );
    let grace_hash = grace.password_hash.expect("grace has a password");
    assert_eq!(
        checked[2], grace_hash,
        "a wrong password checks the Person's hash"
    );
    assert_ne!(
        Some(&checked[1]),
        ada.password_hash.as_ref(),
        "a disabled Person's own hash is never checked"
    );
    // `$argon2id$v=19$m=…,t=…,p=…`: the algorithm, the version and the
    // parameters, before the salt and the hash.
    let parameters = |hash: &str| hash.rsplitn(3, '$').nth(2).map(str::to_string);
    for hash in &checked[..2] {
        assert_eq!(
            parameters(hash),
            parameters(&grace_hash),
            "the dummy hash does the work of a real one"
        );
    }
}

#[tokio::test]
async fn a_client_credential_opens_a_session_and_a_sign_out_closes_it() {
    let daemon = TestDaemon::start().await;
    let client = client();

    let response = client
        .post(format!("{}/api/v1/sessions/client", daemon.base_url))
        .json(&serde_json::json!({ "credential": daemon.client_credential() }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let secret = session_cookie(&response).expect("the answer sets the session cookie");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body["role"], "administrator",
        "the seeded local person is the administrator"
    );
    let cookie = format!("{SESSION_COOKIE}={secret}");

    let signed_in = client
        .get(format!("{}/api/v1/user", daemon.base_url))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(signed_in.status(), 200);

    let signed_out = client
        .delete(format!("{}/api/v1/sessions/current", daemon.base_url))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(signed_out.status(), 204);

    let after = client
        .get(format!("{}/api/v1/user", daemon.base_url))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 401, "a signed-out session opens nothing");
}

#[tokio::test]
async fn a_wrong_client_credential_is_refused() {
    let daemon = TestDaemon::start().await;

    let response = client()
        .post(format!("{}/api/v1/sessions/client", daemon.base_url))
        .json(&serde_json::json!({ "credential": "0".repeat(64) }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
    assert_eq!(session_cookie(&response), None);
}

#[tokio::test]
async fn a_session_that_expired_opens_nothing() {
    let daemon = TestDaemon::start().await;
    let secret = pagis_server::random_secret();
    let now = now_ms();
    SqliteSessionStore::new(daemon.pool().clone())
        .create(&Session {
            id: SessionId::generate(),
            user_id: daemon.user_id.clone(),
            token_hash: pagis_server::hash_secret(&secret),
            client_kind: ClientKind::Browser,
            client_name: None,
            created_at: now - 31 * 24 * 60 * 60 * 1_000,
            last_used_at: now - 1_000,
            expires_at: now - 1,
        })
        .await
        .unwrap();

    let response = client()
        .get(format!("{}/api/v1/user", daemon.base_url))
        .header("cookie", format!("{SESSION_COOKIE}={secret}"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn a_sign_in_link_works_once_and_hands_the_browser_a_cookie() {
    let daemon = TestDaemon::start().await;
    let url = pagis::sign_in_link(daemon.stores(), &daemon.public_origin)
        .await
        .expect("mint a sign-in link");
    // The secret is a path segment and the URL carries no query at all.
    assert!(!url.contains("token="), "{url}");
    assert!(!url.contains('?'), "{url}");

    let client = client();
    let first = client.get(&url).send().await.unwrap();
    assert_eq!(first.status(), 303);
    assert_eq!(first.headers()["location"], "/");
    let secret = session_cookie(&first).expect("the link sets the session cookie");

    let signed_in = client
        .get(format!("{}/api/v1/user", daemon.base_url))
        .header("cookie", format!("{SESSION_COOKIE}={secret}"))
        .send()
        .await
        .unwrap();
    assert_eq!(signed_in.status(), 200);

    let second = client.get(&url).send().await.unwrap();
    assert_eq!(second.status(), 401, "a link is good for one use");
}

#[tokio::test]
async fn a_request_without_a_session_reads_nothing() {
    let daemon = TestDaemon::start().await;

    let response = client()
        .get(format!("{}/api/v1/agents", daemon.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn two_people_in_one_org_each_see_only_their_own_roster() {
    let daemon = TestDaemon::start().await;
    let (person, agent) =
        second_person(daemon.pool(), "grace@example.com", "a good password").await;
    let client = client();

    let roster = |cookie: String| {
        let client = client.clone();
        let url = format!("{}/api/v1/agents", daemon.base_url);
        async move {
            let response = client
                .get(url)
                .header("cookie", cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let page: serde_json::Value = response.json().await.unwrap();
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["name"].as_str().unwrap().to_string())
                .collect::<Vec<String>>()
        }
    };

    // The seeded person keeps the sprite the seed made.
    assert_eq!(
        roster(daemon.cookie().to_string()).await,
        vec!["Pixie".to_string()]
    );
    // The second person sees their own sprite and nothing else.
    assert_eq!(
        roster(daemon.cookie_for(&person.id).await).await,
        vec![agent.name.clone()]
    );

    // One Org holds them both, and each owns one Workspace.
    let people = SqliteUserStore::new(daemon.pool().clone())
        .list_by_org(&org(daemon.pool()).await.id)
        .await
        .unwrap();
    assert_eq!(people.len(), 2);
    let workspaces = SqliteWorkspaceStore::new(daemon.pool().clone())
        .list()
        .await
        .unwrap();
    assert_eq!(workspaces.len(), 2);
    assert_ne!(workspaces[0].user_id, workspaces[1].user_id);
}

/// The close code of a socket whose Session ended: 1008, policy
/// violation. A client tells it from a network fault and shows the
/// sign-in page instead of reconnecting.
const SESSION_ENDED: u16 = 1008;

/// Whether the socket still answers a ping with a pong.
async fn answers(socket: &mut Socket) -> bool {
    if socket
        .send(Message::text(
            serde_json::json!({ "type": "ping" }).to_string(),
        ))
        .await
        .is_err()
    {
        return false;
    }
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) if text.as_str() == r#"{"type":"pong"}"# => {
                return true;
            }
            Ok(Some(Ok(Message::Text(_)))) => {}
            _ => return false,
        }
    }
}

/// Create a Channel in the Workspace of `cookie`. The daemon publishes
/// `channel.created`, which every open socket of that Workspace reads.
async fn publish_an_event(daemon: &TestDaemon, cookie: &str) {
    let response = client()
        .post(format!("{}/api/v1/channels", daemon.base_url))
        .header("cookie", cookie)
        .json(&serde_json::json!({ "title": "after the sign-out" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
}

/// A sign-out ends the Session, and the socket it opened goes with it:
/// the daemon closes it with 1008 and sends nothing after that, not
/// even the events of the Workspace that the Person's other Session
/// makes.
#[tokio::test]
async fn a_sign_out_closes_the_socket_of_that_session_with_1008_and_sends_nothing_after_it() {
    let daemon = TestDaemon::start().await;
    let signing_out = daemon.cookie_for(&daemon.user_id).await;
    let mut socket = daemon.event_socket(&signing_out).await;

    daemon.sign_out(&signing_out).await;
    publish_an_event(&daemon, daemon.cookie()).await;

    let closed = pagis_testkit::read_until_closed(&mut socket).await;
    assert_eq!(closed.code, SESSION_ENDED);
    assert!(
        closed.frames.is_empty(),
        "the socket sent frames after the sign-out: {:?}",
        closed.frames
    );
    let after = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("the connection ends after the Close");
    assert!(
        !matches!(after, Some(Ok(_))),
        "the socket sent a frame after its Close: {after:?}"
    );
}

/// A sign-out ends one Session. Another Session of the same Person and
/// a Session of another Person keep their sockets.
#[tokio::test]
async fn a_sign_out_leaves_the_sockets_of_every_other_session_open() {
    let daemon = TestDaemon::start().await;
    let (grace, _) = second_person(daemon.pool(), "grace@example.com", "a good password").await;
    let signing_out = daemon.cookie_for(&daemon.user_id).await;
    let mut signed_out = daemon.event_socket(&signing_out).await;
    let mut same_person = daemon.event_socket(daemon.cookie()).await;
    let mut other_person = daemon
        .event_socket(&daemon.cookie_for(&grace.id).await)
        .await;

    daemon.sign_out(&signing_out).await;

    assert_eq!(
        pagis_testkit::read_until_closed(&mut signed_out).await.code,
        SESSION_ENDED
    );
    assert!(
        answers(&mut same_person).await,
        "the other Session of the same Person lost its socket"
    );
    assert!(
        answers(&mut other_person).await,
        "the Session of another Person lost its socket"
    );
}

/// A clock a test moves by hand. Every wait on its change wakes when it
/// moves, so a timer at a far instant fires with no real wait.
struct ManualClock {
    now: std::sync::atomic::AtomicI64,
    changed: tokio::sync::Notify,
}

impl ManualClock {
    fn at(millis: i64) -> Self {
        Self {
            now: std::sync::atomic::AtomicI64::new(millis),
            changed: tokio::sync::Notify::new(),
        }
    }

    fn advance_to(&self, millis: i64) {
        self.now.store(millis, std::sync::atomic::Ordering::SeqCst);
        self.changed.notify_waiters();
    }
}

impl pagis_core::Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn changed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(self.changed.notified())
    }
}

/// `auth::resolve` refuses a Session past its expiry on a request, and a
/// socket obeys the same limit: it closes with 1008 when its Session
/// reaches `expires_at`, although nobody signed out.
#[tokio::test]
async fn a_socket_closes_when_its_session_reaches_its_expiry() {
    let start = now_ms();
    let clock = std::sync::Arc::new(ManualClock::at(start));
    let daemon = TestDaemon::start_with(pagis_testkit::TestDaemonOptions {
        clock: clock.clone(),
        ..pagis_testkit::TestDaemonOptions::default()
    })
    .await;
    // One hour of life left. The socket must not wait an hour of real
    // time: the fake clock moves past the expiry at once.
    let expires_at = start + 60 * 60 * 1_000;
    let secret = pagis_server::random_secret();
    SqliteSessionStore::new(daemon.pool().clone())
        .create(&Session {
            id: SessionId::generate(),
            user_id: daemon.user_id.clone(),
            token_hash: pagis_server::hash_secret(&secret),
            client_kind: ClientKind::Browser,
            client_name: None,
            created_at: start - 1_000,
            last_used_at: start - 1_000,
            expires_at,
        })
        .await
        .unwrap();
    let mut socket = daemon
        .event_socket(&format!("{SESSION_COOKIE}={secret}"))
        .await;
    assert!(answers(&mut socket).await, "the live Session has a socket");

    clock.advance_to(expires_at);

    assert_eq!(
        pagis_testkit::read_until_closed(&mut socket).await.code,
        SESSION_ENDED
    );
}
