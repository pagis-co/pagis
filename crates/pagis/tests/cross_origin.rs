//! A browser request from an origin that a listener does not serve.
//!
//! The Session cookie is `SameSite=Strict`, which stops a cross-site
//! request only. A page on a sibling subdomain of the Public Origin, with
//! the same scheme, is same-site: the browser sends the cookie with its
//! WebSocket handshake and with its form post. The CORS answer stops
//! only the read of a response. So each listener refuses a browser
//! request from an origin it does not serve, on every socket upgrade and
//! on every unsafe method, before the Session check (ADR-0024).
//!
//! The walk over every write route reads the route tables of both
//! listeners, so a new route is in it without a change here.

use std::sync::Arc;

use pagis_server::{ADMINISTRATION_ROUTES, ROUTES, Route};
use pagis_testkit::{Script, ScriptedBrain, TestDaemon, TestDaemonOptions};
use reqwest::StatusCode;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};

/// The origin the browser reaches the installation at.
const PUBLIC_ORIGIN: &str = "https://pagis.example.com";

/// A page on a sibling subdomain of the Public Origin, with the same
/// scheme. It is same-site, so the browser sends the Session cookie with
/// its requests.
const SIBLING_ORIGIN: &str = "https://files.example.com";

/// A page on another port of this machine, such as a development server.
const OTHER_LOOPBACK_ORIGIN: &str = "http://127.0.0.1:5173";

fn options() -> TestDaemonOptions {
    TestDaemonOptions {
        public_origin: PUBLIC_ORIGIN.to_string(),
        ..TestDaemonOptions::default()
    }
}

/// A request with the given headers and no body.
fn request(method: &str, url: &str, headers: &[(&str, &str)]) -> reqwest::RequestBuilder {
    let method = reqwest::Method::from_bytes(method.to_uppercase().as_bytes())
        .unwrap_or_else(|_| panic!("{method} is not an HTTP method"));
    let mut builder = reqwest::Client::new().request(method, url);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder
}

/// Answer the status of a request and whether its answer is the refusal
/// of the origin check: `403` with an error that names the origin, which
/// no handler writes.
async fn send(builder: reqwest::RequestBuilder) -> (StatusCode, bool) {
    let response = builder.send().await.expect("the daemon answers");
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    let refused_for_origin = status == StatusCode::FORBIDDEN
        && body["error"]["code"] == "forbidden"
        && body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("origin"));
    (status, refused_for_origin)
}

/// The status of a WebSocket handshake to `path` on the product port:
/// `101` when the daemon upgrades. `cookie` is the Session or none.
async fn handshake(
    daemon: &TestDaemon,
    path: &str,
    cookie: Option<&str>,
    headers: &[(&str, &str)],
) -> u16 {
    let url = format!("ws://{}{path}", daemon.addr);
    let mut request = daemon.ws_request(&url);
    match cookie {
        Some(cookie) => {
            request.headers_mut().insert(
                "cookie",
                HeaderValue::from_str(cookie).expect("a cookie header"),
            );
        }
        None => {
            request.headers_mut().remove("cookie");
        }
    }
    for (name, value) in headers {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
            HeaderValue::from_str(value).expect("a header value"),
        );
    }
    match connect_async(request).await {
        Ok((_socket, response)) => response.status().as_u16(),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => response.status().as_u16(),
        Err(error) => panic!("the handshake to {path} failed: {error}"),
    }
}

/// The three sockets of the product port.
fn socket_paths(daemon: &TestDaemon) -> [String; 3] {
    [
        "/api/v1/ws".to_string(),
        format!("/api/v1/channels/{}/dictate", daemon.dm_channel_id),
        "/api/v1/calls/call_none/listen".to_string(),
    ]
}

/// A path of a route table with each parameter filled, so the request
/// reaches the route that the row names.
fn filled(route: &Route) -> String {
    let mut filled = String::new();
    let mut rest = route.path;
    while let Some(open) = rest.find('{') {
        let close = rest[open..].find('}').expect("a parameter closes") + open;
        filled.push_str(&rest[..open]);
        filled.push_str("none");
        rest = &rest[close + 1..];
    }
    filled.push_str(rest);
    filled
}

/// Whether anything wrote to the daemon's database. SQLite changes
/// `PRAGMA data_version` on one connection each time another connection
/// commits, so the probe holds one connection of the harness's own pool
/// and sees every write of the daemon.
struct WriteProbe {
    connection: sqlx::pool::PoolConnection<sqlx::Sqlite>,
    version: i64,
}

impl WriteProbe {
    async fn start(daemon: &TestDaemon) -> Self {
        let mut connection = daemon.pool().acquire().await.expect("a connection");
        let version = Self::data_version(&mut connection).await;
        Self {
            connection,
            version,
        }
    }

    async fn wrote(&mut self) -> bool {
        Self::data_version(&mut self.connection).await != self.version
    }

    async fn data_version(connection: &mut sqlx::SqliteConnection) -> i64 {
        sqlx::query_scalar("PRAGMA data_version")
            .fetch_one(connection)
            .await
            .expect("read the data version")
    }
}

/// With a valid Session and the origin of a sibling subdomain, none of
/// the three sockets upgrades. From the Public Origin, each one still
/// does.
#[tokio::test]
async fn the_sockets_refuse_a_sibling_origin_and_upgrade_for_the_public_origin() {
    let daemon = TestDaemon::start_with(options()).await;

    for path in socket_paths(&daemon) {
        assert_eq!(
            handshake(
                &daemon,
                &path,
                Some(daemon.cookie()),
                &[("origin", SIBLING_ORIGIN)]
            )
            .await,
            403,
            "{path} upgraded for a sibling origin"
        );
        assert_eq!(
            handshake(
                &daemon,
                &path,
                Some(daemon.cookie()),
                &[("origin", PUBLIC_ORIGIN)]
            )
            .await,
            101,
            "{path} did not upgrade for the Public Origin"
        );
    }
}

/// Every POST, PUT and DELETE route of both listeners refuses the
/// sibling origin before its handler runs, so nothing changes: no row,
/// no line of the config file and no restart.
#[tokio::test]
async fn every_write_route_of_both_listeners_refuses_a_sibling_origin() {
    let daemon = TestDaemon::start_with(options()).await;
    // The background loops write on their own clock. Stopped, they leave
    // the requests below as the only possible writer.
    daemon.cancel();
    let mut probe = WriteProbe::start(&daemon).await;
    let config = daemon.booted.home.join("config.toml");
    let config_before = std::fs::read(&config).expect("read the config");

    let mut walked = 0;
    for (base_url, table) in [
        (&daemon.base_url, ROUTES),
        (&daemon.administration_base_url, ADMINISTRATION_ROUTES),
    ] {
        for route in table {
            for method in route.methods.iter().filter(|method| **method != "get") {
                let url = format!("{base_url}{}", filled(route));
                let (status, refused_for_origin) = send(
                    request(
                        method,
                        &url,
                        &[("cookie", daemon.cookie()), ("origin", SIBLING_ORIGIN)],
                    )
                    .json(&serde_json::json!({})),
                )
                .await;
                assert!(
                    refused_for_origin,
                    "{method} {url} answered {status} to a sibling origin"
                );
                walked += 1;
            }
        }
    }

    assert!(walked > 60, "the walk reached {walked} routes only");
    assert!(!probe.wrote().await, "a refused request wrote a row");
    assert_eq!(
        std::fs::read(&config).expect("read the config"),
        config_before,
        "a refused request wrote the config file"
    );
    assert!(
        !daemon.restart.is_asked(),
        "a refused request asked for a restart"
    );

    // The probe sees a write: the same kind of request from the Public
    // Origin makes a row.
    let (status, _) = send(
        request(
            "post",
            &format!("{}/api/v1/channels", daemon.base_url),
            &[("cookie", daemon.cookie()), ("origin", PUBLIC_ORIGIN)],
        )
        .json(&serde_json::json!({ "title": "general" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(probe.wrote().await, "the probe missed a write");
}

/// A cancel has no body, so a form post reaches it with no preflight.
/// From a sibling origin it is refused and the Run goes on; from the
/// Public Origin it cancels the Run.
#[tokio::test]
async fn a_bodyless_cancel_from_a_sibling_origin_is_refused() {
    let brain = Arc::new(ScriptedBrain::default());
    brain.push(Script::hang(&["Still working"]));
    let daemon = TestDaemon::start_with(TestDaemonOptions {
        brain: Arc::clone(&brain) as _,
        ..options()
    })
    .await;
    let sent = request(
        "post",
        &format!(
            "{}/api/v1/channels/{}/messages",
            daemon.base_url, daemon.dm_channel_id
        ),
        &[("cookie", daemon.cookie())],
    )
    .json(&serde_json::json!({ "pending_id": "p1", "text": "hi" }))
    .send()
    .await
    .unwrap();
    assert!(sent.status().is_success(), "{}", sent.status());
    let run_id = running_run(&daemon).await;
    let cancel = format!("{}/api/v1/runs/{run_id}/cancel", daemon.base_url);

    let (status, refused_for_origin) = send(request(
        "post",
        &cancel,
        &[("cookie", daemon.cookie()), ("origin", SIBLING_ORIGIN)],
    ))
    .await;

    assert!(refused_for_origin, "the cancel answered {status}");
    assert_eq!(running_run(&daemon).await, run_id, "the Run stopped");

    let (status, _) = send(request(
        "post",
        &cancel,
        &[("cookie", daemon.cookie()), ("origin", PUBLIC_ORIGIN)],
    ))
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

/// The id of the one running Run, once the agent loop started it.
async fn running_run(daemon: &TestDaemon) -> String {
    for _ in 0..200 {
        let page: serde_json::Value = request(
            "get",
            &format!("{}/api/v1/runs?state=running", daemon.base_url),
            &[("cookie", daemon.cookie())],
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        if let Some(id) = page["items"][0]["id"].as_str() {
            return id.to_string();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no Run started");
}

/// `multipart/form-data` is a content type a form posts with no
/// preflight. From a sibling origin the upload is refused and stores
/// nothing: the same bytes from the Public Origin make a new Artifact.
#[tokio::test]
async fn a_multipart_upload_from_a_sibling_origin_is_refused() {
    let daemon = TestDaemon::start_with(options()).await;
    let upload = |origin: &'static str| {
        let part = reqwest::multipart::Part::bytes(b"a page's upload".to_vec())
            .file_name("note.txt")
            .mime_str("text/plain")
            .unwrap();
        request(
            "post",
            &format!("{}/api/v1/artifacts", daemon.base_url),
            &[("cookie", daemon.cookie()), ("origin", origin)],
        )
        .multipart(reqwest::multipart::Form::new().part("file", part))
    };

    let (status, refused_for_origin) = send(upload(SIBLING_ORIGIN)).await;
    assert!(refused_for_origin, "the upload answered {status}");

    let (status, _) = send(upload(PUBLIC_ORIGIN)).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the refused upload stored the bytes, or the Public Origin cannot upload"
    );
}

/// `Sec-Fetch-Site` decides when the browser sends it. `same-site` and
/// `cross-site` are refused on an unsafe method and on an upgrade, also
/// when the `Origin` header matches.
#[tokio::test]
async fn a_same_site_or_cross_site_request_is_refused_even_with_a_matching_origin() {
    let daemon = TestDaemon::start_with(options()).await;
    let administration_origin = daemon.administration_base_url.clone();

    for site in ["same-site", "cross-site"] {
        let (status, refused_for_origin) = send(
            request(
                "post",
                &format!("{}/api/v1/channels", daemon.base_url),
                &[
                    ("cookie", daemon.cookie()),
                    ("origin", PUBLIC_ORIGIN),
                    ("sec-fetch-site", site),
                ],
            )
            .json(&serde_json::json!({ "title": "general" })),
        )
        .await;
        assert!(refused_for_origin, "{site} POST answered {status}");

        let status = handshake(
            &daemon,
            "/api/v1/ws",
            Some(daemon.cookie()),
            &[("origin", PUBLIC_ORIGIN), ("sec-fetch-site", site)],
        )
        .await;
        assert_eq!(status, 403, "{site} upgrade answered {status}");

        let (status, refused_for_origin) = send(
            request(
                "post",
                &format!(
                    "{}/api/v1/administration/people",
                    daemon.administration_base_url
                ),
                &[
                    ("cookie", daemon.cookie()),
                    ("origin", &administration_origin),
                    ("sec-fetch-site", site),
                ],
            )
            .json(&serde_json::json!({})),
        )
        .await;
        assert!(
            refused_for_origin,
            "{site} POST on the administration port answered {status}"
        );
    }
}

/// `Sec-Fetch-Site: same-origin` and `none` pass to the Session check:
/// without a Session the answer is `401`, and with one the request works.
#[tokio::test]
async fn same_origin_and_none_pass_to_the_session_check() {
    let daemon = TestDaemon::start_with(options()).await;
    let channels = format!("{}/api/v1/channels", daemon.base_url);

    for site in ["same-origin", "none"] {
        let (status, _) = send(
            request("post", &channels, &[("sec-fetch-site", site)])
                .json(&serde_json::json!({ "title": "general" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{site} POST");

        let status = handshake(&daemon, "/api/v1/ws", None, &[("sec-fetch-site", site)]).await;
        assert_eq!(status, 401, "{site} upgrade");

        let (status, _) = send(
            request(
                "post",
                &channels,
                &[("cookie", daemon.cookie()), ("sec-fetch-site", site)],
            )
            .json(&serde_json::json!({ "title": "general" })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{site} POST with a Session");
    }
}

/// A POST from the Public Origin works, as the Product App makes it.
#[tokio::test]
async fn a_post_from_the_public_origin_works() {
    let daemon = TestDaemon::start_with(options()).await;

    let (status, _) = send(
        request(
            "post",
            &format!("{}/api/v1/channels", daemon.base_url),
            &[
                ("cookie", daemon.cookie()),
                ("origin", PUBLIC_ORIGIN),
                ("sec-fetch-site", "same-origin"),
            ],
        )
        .json(&serde_json::json!({ "title": "general" })),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
}

/// The Administration Port serves its own origin only. A page of the
/// product port cannot ask it for a restart; the Administration
/// Interface, on its own origin, can.
#[tokio::test]
async fn the_administration_port_refuses_the_origin_of_the_product_port() {
    let daemon = TestDaemon::start().await;
    let restart = format!("{}/api/v1/system/restart", daemon.administration_base_url);

    let (status, refused_for_origin) = send(request(
        "post",
        &restart,
        &[
            ("cookie", daemon.cookie()),
            ("origin", &daemon.public_origin),
        ],
    ))
    .await;

    assert!(refused_for_origin, "the restart answered {status}");
    assert!(
        !daemon.restart.is_asked(),
        "the product origin restarted the daemon"
    );

    let (status, _) = send(request(
        "post",
        &restart,
        &[
            ("cookie", daemon.cookie()),
            ("origin", &daemon.administration_base_url),
        ],
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(daemon.restart.is_asked());
}

/// The Administration Interface works from its own origin, with or
/// without `Sec-Fetch-Site`.
#[tokio::test]
async fn the_administration_interface_works_from_its_own_origin() {
    let daemon = TestDaemon::start().await;
    let people = format!(
        "{}/api/v1/administration/people",
        daemon.administration_base_url
    );

    for (email, headers) in [
        (
            "grace@example.com",
            vec![
                ("origin", daemon.administration_base_url.as_str()),
                ("sec-fetch-site", "same-origin"),
            ],
        ),
        (
            "ada@example.com",
            vec![("origin", daemon.administration_base_url.as_str())],
        ),
    ] {
        let mut headers = headers;
        headers.push(("cookie", daemon.cookie()));
        let (status, _) = send(request("post", &people, &headers).json(&serde_json::json!({
            "email": email,
            "name": "Grace",
            "password": "correct horse battery",
        })))
        .await;
        assert_eq!(status, StatusCode::CREATED, "{headers:?}");
    }
}

/// On a local installation, a page on another port of the same machine
/// is another origin, and both listeners refuse it.
#[tokio::test]
async fn a_loopback_origin_on_another_port_is_refused_on_both_listeners() {
    let daemon = TestDaemon::start().await;
    assert!(daemon.public_origin.starts_with("http://127.0.0.1:"));

    let (status, refused_for_origin) = send(
        request(
            "post",
            &format!("{}/api/v1/channels", daemon.base_url),
            &[
                ("cookie", daemon.cookie()),
                ("origin", OTHER_LOOPBACK_ORIGIN),
            ],
        )
        .json(&serde_json::json!({ "title": "general" })),
    )
    .await;
    assert!(refused_for_origin, "the product port answered {status}");

    let status = handshake(
        &daemon,
        "/api/v1/ws",
        Some(daemon.cookie()),
        &[("origin", OTHER_LOOPBACK_ORIGIN)],
    )
    .await;
    assert_eq!(status, 403, "the event socket answered {status}");

    let (status, refused_for_origin) = send(
        request(
            "post",
            &format!(
                "{}/api/v1/administration/people",
                daemon.administration_base_url
            ),
            &[
                ("cookie", daemon.cookie()),
                ("origin", OTHER_LOOPBACK_ORIGIN),
            ],
        )
        .json(&serde_json::json!({})),
    )
    .await;
    assert!(
        refused_for_origin,
        "the administration port answered {status}"
    );
}

/// A request with no `Origin` and no `Sec-Fetch-Site` is not a
/// browser's. It passes to the Session check, which still answers `401`
/// without a Session.
#[tokio::test]
async fn a_request_with_neither_header_passes_to_the_session_check() {
    let daemon = TestDaemon::start_with(options()).await;

    let (status, _) = send(
        request("post", &format!("{}/api/v1/channels", daemon.base_url), &[])
            .json(&serde_json::json!({ "title": "general" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    for path in socket_paths(&daemon) {
        assert_eq!(handshake(&daemon, &path, None, &[]).await, 401, "{path}");
    }

    let (status, _) = send(
        request(
            "post",
            &format!("{}/api/v1/system/restart", daemon.administration_base_url),
            &[],
        )
        .json(&serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The Host socket of the Client App registers a Host. The handshake
/// carries the headers that the Client App's WebSocket sends, as the
/// test "sends the Session cookie and no Origin or Sec-Fetch-Site in its
/// handshake" of `desktop/src/host.test.ts` records them: the cookie, and
/// no `Origin` and no `Sec-Fetch-Site`.
#[tokio::test]
async fn the_host_socket_of_the_client_app_registers_a_host() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let daemon = TestDaemon::start_with(options()).await;
    let mut handshake = daemon.ws_request(&daemon.ws_url());
    for (name, value) in [
        ("accept", "*/*"),
        ("accept-language", "*"),
        ("sec-fetch-mode", "websocket"),
        ("user-agent", "node"),
        ("pragma", "no-cache"),
        ("cache-control", "no-cache"),
        ("accept-encoding", "gzip, deflate"),
    ] {
        handshake
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    assert!(!handshake.headers().contains_key("origin"));
    assert!(!handshake.headers().contains_key("sec-fetch-site"));

    let (mut socket, _) = connect_async(handshake)
        .await
        .expect("the Host socket opens");
    for frame in [
        serde_json::json!({ "type": "auth" }),
        serde_json::json!({
            "type": "register_host",
            "name": "Air",
            "platform": "macos",
            "capabilities": ["shell"],
        }),
    ] {
        socket
            .send(Message::text(frame.to_string()))
            .await
            .expect("send a frame");
    }
    let registered = loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
            .await
            .expect("a frame before the timeout")
            .expect("the socket stays open")
            .expect("a readable frame");
        let Message::Text(text) = frame else { continue };
        let frame: serde_json::Value = serde_json::from_str(&text).expect("a JSON frame");
        assert_ne!(frame["type"], "error", "the registration failed: {frame}");
        if frame["type"] == "host.registered" {
            break frame;
        }
    };
    assert!(registered["payload"]["host_id"].is_string(), "{registered}");
}

/// On a local installation in the multi-user mode, the Public Origin is
/// the name that the owner's proxy answers on. The owner's Client App and
/// the Sign-In Link still open the Product App at the loopback origin of
/// the daemon (ADR-0024), and a browser sends no `Sec-Fetch-Site` on a
/// WebSocket handshake. So the product port of a local installation
/// serves that origin too, and the sockets of that page upgrade.
#[tokio::test]
async fn a_local_installation_serves_its_loopback_origin_too() {
    let daemon = TestDaemon::start_with(options()).await;
    let local_origin = daemon.booted.config.local_origin(daemon.addr.port());
    assert_ne!(local_origin, daemon.public_origin);

    for path in socket_paths(&daemon) {
        assert_eq!(
            handshake(
                &daemon,
                &path,
                Some(daemon.cookie()),
                &[("origin", &local_origin)]
            )
            .await,
            101,
            "{path} did not upgrade for the loopback origin"
        );
    }
    let (status, _) = send(
        request(
            "post",
            &format!("{}/api/v1/channels", daemon.base_url),
            &[("cookie", daemon.cookie()), ("origin", &local_origin)],
        )
        .json(&serde_json::json!({ "title": "general" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

/// A server holds no Client Credential, and no Client App opens it at
/// loopback: its product port serves the Public Origin alone.
#[tokio::test]
async fn a_server_serves_its_public_origin_alone() {
    let Some(daemon) = TestDaemon::start_on_postgres_with(options()).await else {
        return;
    };
    let loopback_origin = daemon.booted.config.local_origin(daemon.addr.port());

    let status = handshake(
        &daemon,
        "/api/v1/ws",
        Some(daemon.cookie()),
        &[("origin", &loopback_origin)],
    )
    .await;

    assert_eq!(status, 403);
}
