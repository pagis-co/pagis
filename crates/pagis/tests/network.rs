//! The daemon on a network.
//!
//! A server binds a named interface and a reverse proxy in front of it
//! terminates TLS. These tests bind one of this machine's own non-
//! loopback addresses and drive sign-in through a small forwarding proxy
//! that sets `X-Forwarded-For` and `X-Forwarded-Proto`, the way Caddy and
//! nginx do (https://docs.pagis.co/server/proxy).
//!
//! The machine needs a non-loopback address for that. A machine with no
//! default route has none, and each test says why it did not run instead
//! of passing on nothing.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::response::Response;
use pagis_core::{User, UserRole, Workspace, WorkspaceId, now_ms};
use pagis_server::{FunnelPort, SESSION_COOKIE};
use pagis_testkit::{FakeTailscale, TestDaemon, TestDaemonOptions};

/// The origin the proxy answers on, as the deployment configures it.
const PUBLIC_ORIGIN: &str = "https://pagis.example.net";

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build a client")
}

/// One `Set-Cookie` header of the answer, by cookie name.
fn set_cookie(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(&format!("{name}=")))
        .map(str::to_string)
}

/// A non-loopback address of this machine: the source address of the
/// default route. `None` on a machine with no route out, where nothing
/// but loopback exists to bind.
fn own_network_address() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // TEST-NET-1 is never routed, and a connected UDP socket sends
    // nothing: the kernel only picks the source address it would use.
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}

/// The source address a connection to `destination` leaves this machine
/// with. The daemon trusts the proxy by address, so the test learns the
/// address before it configures the daemon.
async fn source_address_to(destination: IpAddr) -> IpAddr {
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(destination, 0))
        .await
        .expect("bind a probe listener");
    let address = listener.local_addr().expect("the probe address");
    let (accepted, _) = tokio::try_join!(
        async { listener.accept().await.map(|(_, peer)| peer) },
        tokio::net::TcpStream::connect(address),
    )
    .expect("probe the source address");
    accepted.ip()
}

/// A person with a password, their own Workspace, and no Agent.
async fn person(daemon: &TestDaemon, email: &str, password: &str) -> User {
    let now = now_ms();
    let stores = daemon.stores();
    let org = stores
        .orgs
        .list()
        .await
        .expect("list the orgs")
        .into_iter()
        .next()
        .expect("the seeded org");
    let person = User {
        email: Some(email.to_string()),
        name: Some("Grace".to_string()),
        password_hash: Some(pagis_server::hash_password(password).expect("hash the password")),
        ..User::new(org.id, UserRole::Member, now)
    };
    stores
        .users
        .create(&person)
        .await
        .expect("create the person");
    stores
        .workspaces
        .create(&Workspace {
            id: WorkspaceId::generate(),
            user_id: person.id.clone(),
            name: "Grace".to_string(),
            timezone: "UTC".to_string(),
            created_at: now,
            onboarded_at: Some(now),
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
            home_exit_host_id: None,
        })
        .await
        .expect("create the workspace");
    person
}

/// A reverse proxy in front of the daemon, as small as one can be and
/// still be one: it forwards every request and tells the daemon which
/// address the browser came from and that the browser spoke TLS.
struct ForwardingProxy {
    base_url: String,
    /// The browser the proxy reports. A test moves it between calls to
    /// act as a second browser on the same proxy.
    client_address: Arc<Mutex<IpAddr>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for ForwardingProxy {
    fn drop(&mut self) {
        self.server.abort();
    }
}

struct ProxyState {
    daemon_base_url: String,
    client_address: Arc<Mutex<IpAddr>>,
    outgoing: reqwest::Client,
}

impl ForwardingProxy {
    /// Start the proxy on loopback in front of `daemon_base_url`.
    async fn start(daemon_base_url: String, client_address: IpAddr) -> Self {
        let client_address = Arc::new(Mutex::new(client_address));
        let state = Arc::new(ProxyState {
            daemon_base_url,
            client_address: Arc::clone(&client_address),
            outgoing: client(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy address");
        let app = axum::Router::new()
            .fallback(axum::routing::any(forward))
            .with_state(state);
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve the proxy");
        });
        Self {
            base_url: format!("http://{addr}"),
            client_address,
            server,
        }
    }

    fn set_client_address(&self, address: IpAddr) {
        *self.client_address.lock().expect("the client address") = address;
    }
}

/// Forward one request, with the two headers a reverse proxy adds.
async fn forward(State(state): State<Arc<ProxyState>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map(ToString::to_string)
        .unwrap_or_else(|| "/".to_string());
    let body = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .expect("read the request body");
    let mut outgoing = state
        .outgoing
        .request(parts.method, format!("{}{path}", state.daemon_base_url))
        .body(body);
    for (name, value) in parts.headers.iter() {
        // The daemon is reached at its own address, and the body is sent
        // again, so the hop-by-hop values are the proxy's to set.
        if name == axum::http::header::HOST || name == axum::http::header::CONTENT_LENGTH {
            continue;
        }
        outgoing = outgoing.header(name, value);
    }
    let address = *state.client_address.lock().expect("the client address");
    let answer = outgoing
        .header("x-forwarded-for", address.to_string())
        .header("x-forwarded-proto", "https")
        .send()
        .await
        .expect("forward the request");

    let mut response = Response::builder().status(answer.status());
    for (name, value) in answer.headers().iter() {
        if name == reqwest::header::CONTENT_LENGTH || name == reqwest::header::TRANSFER_ENCODING {
            continue;
        }
        response = response.header(name, value);
    }
    let bytes = answer.bytes().await.expect("read the answer");
    response
        .body(axum::body::Body::from(bytes))
        .expect("build the answer")
}

/// A server bound to this machine's own network address, behind a proxy
/// whose forwarded headers it believes. `None` when the machine has no
/// non-loopback address, or when Docker is not reachable for the
/// Postgres a server runs on.
async fn daemon_behind_a_trusted_proxy() -> Option<(TestDaemon, ForwardingProxy)> {
    let Some(bind) = own_network_address() else {
        eprintln!("skipped: this machine has no non-loopback address to bind");
        return None;
    };
    let proxy_address = source_address_to(bind).await;
    let daemon = TestDaemon::start_on_postgres_with(TestDaemonOptions {
        bind,
        public_origin: PUBLIC_ORIGIN.to_string(),
        trusted_proxy: Some(proxy_address),
        ..TestDaemonOptions::default()
    })
    .await?;
    assert!(
        !daemon.addr.ip().is_loopback(),
        "the daemon binds a named interface: {}",
        daemon.addr
    );
    let proxy =
        ForwardingProxy::start(daemon.base_url.clone(), "203.0.113.7".parse().unwrap()).await;
    Some((daemon, proxy))
}

/// The same proxy in front of a server that trusts no address: the
/// forwarded scheme is not believed, so the cookie is not `Secure`. A
/// page that claims TLS cannot make the daemon mark a cookie for it.
#[tokio::test]
async fn an_untrusted_forwarded_scheme_does_not_make_the_cookie_secure() {
    let Some(bind) = own_network_address() else {
        eprintln!("skipped: this machine has no non-loopback address to bind");
        return;
    };
    let Some(daemon) = TestDaemon::start_on_postgres_with(TestDaemonOptions {
        bind,
        public_origin: PUBLIC_ORIGIN.to_string(),
        ..TestDaemonOptions::default()
    })
    .await
    else {
        return;
    };
    let proxy =
        ForwardingProxy::start(daemon.base_url.clone(), "203.0.113.7".parse().unwrap()).await;
    person(&daemon, "grace@example.com", "a good password").await;

    let response = client()
        .post(format!("{}/api/v1/sessions", proxy.base_url))
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "password": "a good password",
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let cookie = set_cookie(&response, SESSION_COOKIE).expect("the answer sets the cookie");
    assert!(!cookie.contains("Secure"), "{cookie}");
}

/// The sign-in rate limit counts against the browser's own address, not
/// the proxy's: one person who forgets their password does not lock out
/// everybody else behind the same proxy.
#[tokio::test]
async fn the_sign_in_rate_limit_holds_through_the_proxy_per_browser() {
    let Some((daemon, proxy)) = daemon_behind_a_trusted_proxy().await else {
        return;
    };
    person(&daemon, "grace@example.com", "a good password").await;
    person(&daemon, "ada@example.com", "another good password").await;
    let client = client();
    let attempt = |email: &'static str, password: &'static str| {
        let client = client.clone();
        let url = format!("{}/api/v1/sessions", proxy.base_url);
        async move {
            client
                .post(url)
                .json(&serde_json::json!({ "email": email, "password": password }))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };

    for _ in 0..5 {
        assert_eq!(attempt("grace@example.com", "no").await, 401);
    }
    assert_eq!(
        attempt("grace@example.com", "a good password").await,
        429,
        "the browser that failed five times is locked"
    );

    // A second browser behind the same proxy is a different address, so
    // it signs in. The proxy's own address is not the key.
    proxy.set_client_address("203.0.113.8".parse().unwrap());
    assert_eq!(
        attempt("ada@example.com", "another good password").await,
        200
    );
}

/// The CORS answer names the Public Origin and no other, so a page
/// somewhere else cannot read this API with the browser's cookie.
#[tokio::test]
async fn cors_names_the_public_origin_and_refuses_another() {
    let Some((daemon, proxy)) = daemon_behind_a_trusted_proxy().await else {
        return;
    };
    assert_eq!(daemon.public_origin, PUBLIC_ORIGIN);

    let preflight = client()
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/api/v1/sessions", proxy.base_url),
        )
        .header("origin", PUBLIC_ORIGIN)
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type")
        .send()
        .await
        .unwrap();

    assert_eq!(
        preflight
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some(PUBLIC_ORIGIN)
    );
    assert_eq!(
        preflight
            .headers()
            .get("access-control-allow-credentials")
            .and_then(|value| value.to_str().ok()),
        Some("true")
    );

    let stranger = client()
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/api/v1/sessions", proxy.base_url),
        )
        .header("origin", "https://pagis.example.net.evil.test")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert!(
        stranger
            .headers()
            .get("access-control-allow-origin")
            .is_none(),
        "another origin is named in no answer: {:?}",
        stranger.headers()
    );
}

/// A local installation configures nothing and is reached from its own
/// machine alone: the listener is on loopback and the origin is that
/// address.
#[tokio::test]
async fn a_local_installation_binds_loopback_with_no_configuration() {
    let daemon = TestDaemon::start().await;

    assert!(daemon.addr.ip().is_loopback(), "{}", daemon.addr);
    assert!(daemon.public_origin.starts_with("http://127.0.0.1:"));

    // Nothing is forwarded, so the cookie carries no `Secure`: the page
    // is plain HTTP on this machine and would drop one.
    person(&daemon, "grace@example.com", "a good password").await;
    let sign_in = |forwarded: bool| {
        let request = client()
            .post(format!("{}/api/v1/sessions", daemon.base_url))
            .json(&serde_json::json!({
                "email": "grace@example.com",
                "password": "a good password",
            }));
        match forwarded {
            true => request.header("x-forwarded-proto", "https"),
            false => request,
        }
        .send()
    };
    let response = sign_in(false).await.unwrap();
    assert_eq!(response.status(), 200);
    let cookie = set_cookie(&response, SESSION_COOKIE).expect("the answer sets the cookie");
    assert!(!cookie.contains("Secure"), "{cookie}");

    // A request that says a proxy forwarded it did not come from this
    // machine, so the installation refuses it.
    let forwarded = sign_in(true).await.unwrap();
    assert_eq!(forwarded.status(), 403);
    assert_eq!(set_cookie(&forwarded, SESSION_COOKIE), None);
}

// --- the Client Credential works from this machine alone --------------

/// The origin the Funnel of the owner's machine answers on.
const OWNERS_ORIGIN: &str = "https://owner-mac.tail1234.ts.net";

/// A local installation that other People reach through Remote Access:
/// the daemon binds loopback, the Public Origin is the name the Funnel
/// answers on, and the Funnel at `127.0.0.1` is the Trusted Proxy. The
/// proxy of the test stands in for the Funnel.
async fn local_installation_in_remote_access() -> (TestDaemon, ForwardingProxy) {
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        public_origin: OWNERS_ORIGIN.to_string(),
        trusted_proxy: Some(std::net::Ipv4Addr::LOCALHOST.into()),
        remote_access: true,
        ..TestDaemonOptions::default()
    })
    .await;
    let proxy =
        ForwardingProxy::start(daemon.base_url.clone(), "203.0.113.7".parse().unwrap()).await;
    (daemon, proxy)
}

/// The owner's Client App signs in with no prompt on a local
/// installation whose Public Origin is not loopback, and the first-run
/// route stays closed, because the installation holds a Client
/// Credential.
#[tokio::test]
async fn the_owners_client_app_signs_in_on_a_local_installation_with_a_public_origin() {
    let (daemon, _proxy) = local_installation_in_remote_access().await;

    let response = client()
        .post(format!("{}/api/v1/sessions/client", daemon.base_url))
        .json(&serde_json::json!({ "credential": daemon.client_credential() }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(set_cookie(&response, SESSION_COOKIE).is_some());

    let setup = client()
        .get(format!("{}/api/v1/setup", daemon.administration_base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(setup.status(), 410);
}

/// An Administrator turns on Remote Access from the Administration
/// Interface alone. After the restart, a second person on another machine
/// signs in through the Funnel with a Sign-In Link and not with a
/// password, and the owner's Client App stays signed in by the credential.
#[tokio::test]
async fn an_administrator_turns_on_remote_access_for_a_local_installation() {
    let before = TestDaemon::start_with(TestDaemonOptions {
        tailscale: Arc::new(FakeTailscale::ready(
            FunnelPort::Nothing,
            FunnelPort::Nothing,
        )),
        ..TestDaemonOptions::default()
    })
    .await;
    let switch = format!(
        "{}/api/v1/settings/system/remote-access",
        before.administration_base_url
    );
    let response = client()
        .put(&switch)
        .header("cookie", before.cookie())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let read: serde_json::Value = client()
            .get(&switch)
            .header("cookie", before.cookie())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if read["enabled"] == true {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "{read}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // The next start reads the file the switch wrote, as the boot does.
    let config = pagis::Config::read_file(&before.booted.home.join("config.toml")).unwrap();
    let after = TestDaemon::start_with(TestDaemonOptions {
        bind: config.bind_address().unwrap(),
        public_origin: config.public_origin(config.port),
        trusted_proxy: config.trusted_proxy().unwrap().address(),
        remote_access: config.remote_access.enabled,
        ..TestDaemonOptions::default()
    })
    .await;
    assert_eq!(after.public_origin, OWNERS_ORIGIN);
    let funnel =
        ForwardingProxy::start(after.base_url.clone(), "203.0.113.7".parse().unwrap()).await;
    let grace = person(&after, "grace@example.com", "a good password").await;

    let password = client()
        .post(format!("{}/api/v1/sessions", funnel.base_url))
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "password": "a good password",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(password.status(), 403);
    assert_eq!(set_cookie(&password, SESSION_COOKIE), None);

    let link = pagis_server::mint_public_origin_link(
        after.stores().sign_in_links.as_ref(),
        &grace.id,
        &after.public_origin,
        pagis_core::CLIENT_LINK_LIFETIME_MS,
        now_ms(),
    )
    .await
    .unwrap();
    let secret = link.url.split_once('#').expect("a fragment").1;
    let second = client()
        .post(format!("{}/api/v1/sessions/link", funnel.base_url))
        .json(&serde_json::json!({ "secret": secret }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 200);
    let cookie = set_cookie(&second, SESSION_COOKIE).expect("the answer sets the cookie");
    assert!(cookie.contains("; Secure"), "{cookie}");

    let owner = client()
        .post(format!("{}/api/v1/sessions/client", after.base_url))
        .json(&serde_json::json!({ "credential": after.client_credential() }))
        .send()
        .await
        .unwrap();
    assert_eq!(owner.status(), 200);
    assert!(set_cookie(&owner, SESSION_COOKIE).is_some());
}

/// A trade that arrives through the Trusted Proxy is refused, even with
/// the right credential and a proxy on the same machine, whose socket
/// peer is loopback.
#[tokio::test]
async fn a_credential_trade_through_the_trusted_proxy_is_refused() {
    let (daemon, proxy) = local_installation_in_remote_access().await;

    let response = client()
        .post(format!("{}/api/v1/sessions/client", proxy.base_url))
        .json(&serde_json::json!({ "credential": daemon.client_credential() }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(set_cookie(&response, SESSION_COOKIE), None);

    // The runtime identity handshake is keyed by the same credential and
    // follows the same rule.
    let identity = client()
        .get(format!(
            "{}/api/v1/runtime/identity?challenge={}",
            proxy.base_url,
            "0".repeat(64)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(identity.status(), 403);
}

/// A trade whose socket peer is not loopback is refused: the daemon
/// reads the socket, not a header.
#[tokio::test]
async fn a_credential_trade_from_another_address_is_refused() {
    let Some(bind) = own_network_address() else {
        eprintln!("skipped: this machine has no non-loopback address to bind");
        return;
    };
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        bind,
        ..TestDaemonOptions::default()
    })
    .await;

    let response = client()
        .post(format!("{}/api/v1/sessions/client", daemon.base_url))
        .header("host", "127.0.0.1")
        .json(&serde_json::json!({ "credential": daemon.client_credential() }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(set_cookie(&response, SESSION_COOKIE), None);
}

/// The sign-in link follows the same rule. It starts at the loopback
/// origin of the daemon, not at the Public Origin, and it signs a
/// browser in on this machine alone.
#[tokio::test]
async fn the_sign_in_link_works_from_this_machine_alone() {
    let (daemon, proxy) = local_installation_in_remote_access().await;

    let through_the_proxy = pagis::start_link(daemon.stores(), &daemon.base_url)
        .await
        .expect("mint a sign-in link");
    let path = through_the_proxy.trim_start_matches(&daemon.base_url);
    let refused = client()
        .get(format!("{}{path}", proxy.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(set_cookie(&refused, SESSION_COOKIE), None);

    let here = pagis::start_link(daemon.stores(), &daemon.base_url)
        .await
        .expect("mint a sign-in link");
    let signed_in = client().get(&here).send().await.unwrap();
    assert_eq!(signed_in.status(), 303);
    assert!(set_cookie(&signed_in, SESSION_COOKIE).is_some());

    // The link the start banner prints starts at the local origin, which
    // is loopback, where this rule lets it in, whatever the Public Origin
    // names.
    let printed = pagis::start_link(
        daemon.stores(),
        &daemon.booted.config.local_origin(daemon.addr.port()),
    )
    .await
    .expect("a local installation mints a link");
    assert!(printed.starts_with("http://127.0.0.1:"), "{printed}");
}

// --- real TLS, through the documented proxy ---------------------------

/// The Caddy image the deployment runs (`deploy/compose.yaml`).
const CADDY_IMAGE: &str = "caddy:2-alpine";
/// The name the certificate is for. It resolves to nothing, so the test
/// points its own client at the published port.
const TLS_HOST: &str = "pagis.test";

/// A Caddy in a container with `tls internal` in front of the daemon,
/// and its own root certificate, so a client verifies the chain instead
/// of being told to trust anything.
struct TlsProxy {
    container: String,
    port: u16,
    root: Vec<u8>,
}

impl TlsProxy {
    /// Start Caddy in front of `daemon_port` on this machine.
    ///
    /// The container writes its own configuration and then runs: a bind
    /// mount of a file is not portable across Docker runtimes, and the
    /// configuration is four lines.
    ///
    /// Docker chooses the host port of the TLS endpoint, and the test
    /// reads it back. A port that the test takes from the system first
    /// is free only until the test lets it go, so another process can
    /// bind it before Docker does.
    async fn start(daemon_port: u16) -> Self {
        let container = format!("pagis-tls-{}", std::process::id());
        let write_and_run = format!(
            "printf '%s\\n' '{{' 'admin off' '}}' '{TLS_HOST} {{' 'tls internal' \
             'reverse_proxy host.docker.internal:{daemon_port}' '}}' > /etc/caddy/Caddyfile && \
             exec caddy run --config /etc/caddy/Caddyfile"
        );
        let status = std::process::Command::new("docker")
            .args([
                "run",
                "-d",
                "--rm",
                "--name",
                &container,
                "--add-host=host.docker.internal:host-gateway",
                "-p",
                "127.0.0.1::443",
                CADDY_IMAGE,
                "sh",
                "-c",
                &write_and_run,
            ])
            .status()
            .expect("docker run caddy");
        assert!(status.success(), "caddy did not start");
        let port = published_port(&container);
        let root = read_caddy_root(&container).await;
        let proxy = Self {
            container,
            port,
            root,
        };
        proxy.wait_for_tls().await;
        proxy
    }

    /// Wait until the TLS endpoint answers. The local authority's root is
    /// written before the leaf certificate of the site is, so a client
    /// that reaches the port too early gets a handshake that ends.
    async fn wait_for_tls(&self) {
        let client = self.client();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let reached = client
                .get(format!("{}/api/v1/health", self.base_url()))
                .send()
                .await
                .is_ok();
            if reached {
                return;
            }
            if std::time::Instant::now() >= deadline {
                self.stop();
                panic!("caddy never answered over TLS");
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    /// A client that verifies Caddy's own chain and resolves the
    /// certificate's name to the published port.
    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .add_root_certificate(
                reqwest::Certificate::from_pem(&self.root).expect("caddy's root is a certificate"),
            )
            .resolve(
                TLS_HOST,
                SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), self.port),
            )
            .build()
            .expect("build the TLS client")
    }

    fn base_url(&self) -> String {
        format!("https://{TLS_HOST}:{}", self.port)
    }

    fn stop(&self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.container])
            .status();
    }
}

/// Caddy's local certificate authority, which `tls internal` signs with.
/// It is written on the first handshake, so this waits for it.
async fn read_caddy_root(container: &str) -> Vec<u8> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let output = std::process::Command::new("docker")
            .args([
                "exec",
                container,
                "cat",
                "/data/caddy/pki/authorities/local/root.crt",
            ])
            .output()
            .expect("docker exec cat");
        if output.status.success() && !output.stdout.is_empty() {
            return output.stdout;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "caddy wrote no local certificate authority: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// The host port that Docker published the container's port 443 on.
/// `docker port` answers with one `127.0.0.1:<port>` line for the one
/// loopback binding.
fn published_port(container: &str) -> u16 {
    let output = std::process::Command::new("docker")
        .args(["port", container, "443/tcp"])
        .output()
        .expect("docker port");
    let answer = String::from_utf8_lossy(&output.stdout);
    let port = answer
        .lines()
        .find_map(|line| line.trim().rsplit_once(':'))
        .and_then(|(_, port)| port.parse().ok());
    port.unwrap_or_else(|| {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", container])
            .status();
        panic!(
            "docker published no host port for 443: {answer:?} {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// The address a container's request arrives at this machine from, which
/// is the address the daemon must trust the forwarded headers of. It is
/// not guessed: a container connects to a listener of this test, and the
/// peer address is the answer.
async fn container_source_address() -> IpAddr {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0")
        .await
        .expect("bind a probe listener");
    let port = listener.local_addr().expect("the probe address").port();
    let probe = tokio::task::spawn_blocking(move || {
        std::process::Command::new("docker")
            .args([
                "run",
                "--rm",
                "--add-host=host.docker.internal:host-gateway",
                CADDY_IMAGE,
                "sh",
                "-c",
                &format!("nc -z host.docker.internal {port} || true"),
            ])
            .status()
    });
    let accepted =
        tokio::time::timeout(std::time::Duration::from_secs(60), listener.accept()).await;
    let _ = probe.await;
    accepted
        .expect("a container reached this machine")
        .expect("accept the probe")
        .1
        .ip()
}

fn require_docker() {
    let reachable = std::process::Command::new("docker")
        .args(["version"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    assert!(
        reachable,
        "Docker is not reachable. Start it and set DOCKER_HOST, for example \
         `export DOCKER_HOST=unix:///Users/<you>/.colima/default/docker.sock`."
    );
}

/// A browser signs in over real TLS through the proxy the deployment
/// documents, and the Session cookie it gets is `Secure` and host-only.
///
/// The other tests here send `X-Forwarded-Proto: https` over plain HTTP.
/// This test runs the documented shape: Caddy in front, a certificate, a
/// WebSocket-capable reverse proxy, and a daemon that terminates no
/// TLS. The certificate is Caddy's own `tls internal`, and the client
/// verifies its chain rather than being told to accept anything.
#[tokio::test]
#[ignore = "needs Docker; run via cargo test -- --ignored"]
async fn a_browser_signs_in_over_real_tls_through_caddy() {
    require_docker();
    // Caddy reaches the daemon from a NAT address of the Docker host, and
    // that is the one address whose forwarded headers the daemon believes.
    let proxy_address = container_source_address().await;
    let daemon = TestDaemon::start_on_postgres_with(TestDaemonOptions {
        bind: std::net::Ipv4Addr::UNSPECIFIED.into(),
        public_origin: format!("https://{TLS_HOST}"),
        trusted_proxy: Some(proxy_address),
        ..TestDaemonOptions::default()
    })
    .await
    .expect("a server runs on Postgres, and this test needs Docker");
    assert_eq!(
        daemon.booted.client_credential, None,
        "a server holds no Client Credential (ADR-0025)"
    );
    let person = person(&daemon, "grace@example.com", "a good password").await;
    let proxy = TlsProxy::start(daemon.addr.port()).await;

    let client = proxy.client();
    let response = client
        .post(format!("{}/api/v1/sessions", proxy.base_url()))
        .json(&serde_json::json!({
            "email": "grace@example.com",
            "password": "a good password",
        }))
        .send()
        .await
        .expect("sign in over TLS");

    let status = response.status();
    let cookie = set_cookie(&response, SESSION_COOKIE);
    if status != 200 || cookie.is_none() {
        proxy.stop();
        panic!("the sign-in answered {status} with cookie {cookie:?}");
    }
    let cookie = cookie.expect("the answer sets the cookie");
    let outcome = std::panic::catch_unwind(|| {
        assert!(cookie.contains("; Secure"), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Strict"), "{cookie}");
        assert!(!cookie.to_lowercase().contains("domain"), "{cookie}");
    });

    // The cookie carries the person's own read back through the proxy,
    // over the same TLS connection.
    let secret = cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, secret)| secret.to_string())
        .expect("the cookie value");
    let user = client
        .get(format!("{}/api/v1/user", proxy.base_url()))
        .header("cookie", format!("{SESSION_COOKIE}={secret}"))
        .send()
        .await
        .expect("read the person over TLS")
        .json::<serde_json::Value>()
        .await
        .expect("JSON");

    proxy.stop();
    outcome.expect("the cookie is Secure, HttpOnly, SameSite=Strict and host-only");
    assert_eq!(user["id"], person.id.to_string());
}
