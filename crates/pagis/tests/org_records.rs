//! The Org's records belong to the Org, not to a person's Workspace.
//!
//! An Administrator sets up the carrier and the mail domain once, on
//! the administration port. Then a Member, in a Workspace of their own,
//! buys a number and makes a mailbox on them, and a call to the number
//! reaches the Member's Agent. The first Administrator's Workspace
//! holds none of the Org's records. The carrier, the line, the mail
//! host and the bridge are fakes, so no network is reached.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use pagis_mail::fake::{FakeMailTransport, FakeMailboxHost};
use pagis_telephony::fake::{FakeCallTransport, FakeNumberCatalog, FakeTextTransport, TokioClock};
use pagis_telephony::{CallDirection, FakeCallBridge};
use pagis_testkit::{TestDaemon, TestDaemonOptions};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const OWN: &str = "+14155550123";
const REMOTE: &str = "+16505550100";
const PASSWORD: &str = "correct horse battery";

struct Installation {
    daemon: TestDaemon,
    line: Arc<FakeCallTransport>,
    bridge: Arc<FakeCallBridge>,
    texts: Arc<FakeTextTransport>,
    mail_host: Arc<FakeMailboxHost>,
}

/// The fakes of one installation, and the options that carry them.
struct Fakes {
    line: Arc<FakeCallTransport>,
    bridge: Arc<FakeCallBridge>,
    texts: Arc<FakeTextTransport>,
    mail_host: Arc<FakeMailboxHost>,
}

impl Fakes {
    fn new() -> Self {
        Self {
            line: Arc::new(FakeCallTransport::new(Arc::new(TokioClock))),
            bridge: Arc::new(FakeCallBridge::reporting(FakeCallBridge::answered_report(
                "I would like to book a table.",
            ))),
            texts: Arc::new(FakeTextTransport::default()),
            mail_host: Arc::new(FakeMailboxHost::default()),
        }
    }

    fn options(&self) -> TestDaemonOptions {
        TestDaemonOptions {
            call_transport: Arc::clone(&self.line) as _,
            call_bridge: Some(Arc::clone(&self.bridge) as _),
            number_catalog: Arc::new(FakeNumberCatalog::offering(&[OWN])),
            text_transport: Arc::clone(&self.texts),
            mail_host: Arc::clone(&self.mail_host) as _,
            mail_transport: Arc::new(FakeMailTransport::default()) as _,
            ..TestDaemonOptions::default()
        }
    }

    fn with(self, daemon: TestDaemon) -> Installation {
        Installation {
            daemon,
            line: self.line,
            bridge: self.bridge,
            texts: self.texts,
            mail_host: self.mail_host,
        }
    }
}

async fn boot() -> Installation {
    let fakes = Fakes::new();
    let daemon = TestDaemon::start_with(fakes.options()).await;
    fakes.with(daemon)
}

/// The same installation on Postgres, or `None` when Docker is not
/// reachable.
async fn boot_on_postgres() -> Option<Installation> {
    let fakes = Fakes::new();
    let daemon = TestDaemon::start_on_postgres_with(fakes.options()).await?;
    Some(fakes.with(daemon))
}

async fn send_as(
    cookie: &str,
    method: reqwest::Method,
    url: String,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let mut request = reqwest::Client::new()
        .request(method, url)
        .header("cookie", cookie);
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

async fn next_frame_of(socket: &mut Socket, frame_type: &str) -> serde_json::Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("a frame arrives")
            .expect("the socket is open")
            .expect("a text frame");
        let Message::Text(text) = frame else { continue };
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        if value["type"] == frame_type {
            return value;
        }
    }
}

impl Installation {
    /// The Administrator sets up the carrier with its SIP sign-in, and
    /// the mail domain, on the administration port.
    async fn administrator_sets_up_the_carrier_and_the_mail_domain(&self) -> String {
        self.daemon
            .connect_installation("telnyx", serde_json::json!({ "api_key": "telnyx-key" }))
            .await;
        let (status, body) = self
            .daemon
            .set_up_provider(
                "telnyx",
                "sip",
                serde_json::json!({
                    "username": "robin",
                    "password": "sip-secret",
                    "domain": "sip.telnyx.com",
                }),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        self.daemon
            .connect_installation(
                "migadu",
                serde_json::json!({
                    "account": "owner@example.com",
                    "api_key": "migadu-key",
                    "domain": "example.com",
                }),
            )
            .await
    }

    /// A Member of the Org, made by the Administrator, and the `Cookie`
    /// header of the Member's own Session.
    async fn member(&self) -> String {
        let (status, person) = send_as(
            self.daemon.cookie(),
            reqwest::Method::POST,
            format!(
                "{}/api/v1/administration/people",
                self.daemon.administration_base_url
            ),
            Some(serde_json::json!({
                "email": "grace@example.com",
                "name": "Grace",
                "password": PASSWORD,
            })),
        )
        .await;
        assert_eq!(status, 201, "{person}");
        assert_eq!(person["person"]["role"], "member");
        let response = reqwest::Client::new()
            .post(format!("{}/api/v1/sessions", self.daemon.base_url))
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

    async fn get_as(&self, cookie: &str, path: &str) -> (u16, serde_json::Value) {
        send_as(
            cookie,
            reqwest::Method::GET,
            format!("{}{path}", self.daemon.base_url),
            None,
        )
        .await
    }

    async fn post_as(
        &self,
        cookie: &str,
        path: &str,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        send_as(
            cookie,
            reqwest::Method::POST,
            format!("{}{path}", self.daemon.base_url),
            Some(body),
        )
        .await
    }

    async fn firehose_as(&self, cookie: &str) -> Socket {
        let (mut socket, _) =
            connect_async(self.daemon.ws_request_as(&self.daemon.ws_url(), cookie))
                .await
                .expect("ws connect");
        socket
            .send(Message::text(
                serde_json::json!({ "type": "auth" }).to_string(),
            ))
            .await
            .expect("ws send");
        next_frame_of(&mut socket, "ready").await;
        socket
    }
}

/// A Member uses the carrier and the mail domain the Administrator set
/// up: the Member buys a number and makes a mailbox for their own
/// Agent, the number's texting is prepared on the Org's carrier, and a
/// call to the number reaches the Member's Agent.
#[tokio::test]
async fn a_member_uses_the_carrier_and_the_mail_domain_an_administrator_set_up() {
    a_member_uses_the_org_records(boot().await).await;
}

/// The same on Postgres. The test skips when Docker is not reachable.
#[tokio::test]
async fn a_member_uses_the_carrier_and_the_mail_domain_an_administrator_set_up_on_postgres() {
    let Some(installation) = boot_on_postgres().await else {
        return;
    };
    a_member_uses_the_org_records(installation).await;
}

async fn a_member_uses_the_org_records(installation: Installation) {
    let mail_domain = installation
        .administrator_sets_up_the_carrier_and_the_mail_domain()
        .await;
    let member = installation.member().await;

    // The Member's own Agent, in the Member's own Workspace.
    let (status, roster) = installation.get_as(&member, "/api/v1/agents").await;
    assert_eq!(status, 200, "{roster}");
    let agent_id = roster["items"][0]["id"]
        .as_str()
        .expect("the Member's Workspace has an Agent")
        .to_string();
    assert_ne!(agent_id, installation.daemon.agent_id);

    // The Member's page names the Org's carrier and mail domain, and
    // says that neither is the Member's to change.
    let (status, numbers) = installation
        .get_as(&member, "/api/v1/settings/phone-numbers")
        .await;
    assert_eq!(status, 200, "{numbers}");
    assert_eq!(numbers["carrier"]["status"], "connected", "{numbers}");
    let (status, connections) = installation
        .get_as(&member, "/api/v1/settings/connections")
        .await;
    assert_eq!(status, 200, "{connections}");
    let mut installation_providers: Vec<&str> = connections["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|connection| connection["installation"] == true)
        .map(|connection| connection["provider"].as_str().unwrap())
        .collect();
    installation_providers.sort_unstable();
    assert_eq!(
        installation_providers,
        vec!["migadu", "telnyx"],
        "{connections}"
    );

    // The Member buys a number for their Agent, and the Org's line,
    // registered with the Org's SIP sign-in, carries it.
    let (status, number) = installation
        .post_as(
            &member,
            "/api/v1/settings/phone-numbers",
            serde_json::json!({ "e164": OWN, "agent_id": agent_id }),
        )
        .await;
    assert_eq!(status, 201, "{number}");
    assert_eq!(number["agent_id"], agent_id);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !installation.line.is_registered("robin") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the Org's line registers");

    // The number's texting is prepared on the Org's carrier, so the
    // carrier delivers the Agent's texts to it.
    let prepares = installation.texts.prepares();
    assert_eq!(prepares.len(), 1, "{prepares:?}");
    assert_eq!(prepares[0].e164, OWN);

    // The Member makes a mailbox on the Org's mail domain.
    let (status, mailbox) = installation
        .post_as(
            &member,
            &format!("/api/v1/agents/{agent_id}/mailbox"),
            serde_json::json!({ "connection_id": mail_domain, "local_part": "grace" }),
        )
        .await;
    assert_eq!(status, 201, "{mailbox}");
    assert_eq!(mailbox["address"], "grace@example.com");
    assert_eq!(
        installation.mail_host.addresses(),
        vec!["grace@example.com"]
    );

    // The Member's Agent wakes on the calls it answers and on the mail
    // it gets: both standing rules name the Org's Connections.
    let (status, page) = installation
        .get_as(
            &member,
            &format!("/api/v1/event-subscriptions?agent_id={agent_id}"),
        )
        .await;
    assert_eq!(status, 200, "{page}");
    let mut kinds: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|rule| rule["state"] == "active")
        .map(|rule| rule["event_kind"].as_str().unwrap())
        .collect();
    kinds.sort_unstable();
    assert_eq!(kinds, vec!["call.ended", "mail.message_received"], "{page}");

    // A call to the number reaches the Member's Agent, in the Member's
    // Workspace.
    let mut socket = installation.firehose_as(&member).await;
    installation.line.ring(OWN, REMOTE);
    let placed = next_frame_of(&mut socket, "call.placed").await;
    assert_eq!(placed["payload"]["payload"]["direction"], "inbound");
    assert_eq!(placed["payload"]["agent_id"], agent_id);
    let ended = next_frame_of(&mut socket, "call.ended").await;
    assert_eq!(
        ended["payload"]["payload"]["outcome"], "answered",
        "{ended}"
    );
    let answered = installation.bridge.answered();
    assert_eq!(answered.len(), 1);
    assert_eq!(answered[0].brief.direction, CallDirection::Inbound);
    assert_eq!(answered[0].brief.remote_e164, REMOTE);

    // The first Administrator's Workspace holds none of the Org's
    // records.
    let held = installation
        .daemon
        .stores()
        .connections
        .list(&installation.daemon.workspace_id)
        .await
        .unwrap();
    assert!(held.is_empty(), "{held:?}");
}

/// The Administrator's own page reads the Org's carrier the same way a
/// Member's does, from the Org and not from the Administrator's
/// Workspace.
#[tokio::test]
async fn the_administrators_workspace_holds_only_their_own_data() {
    let installation = boot().await;
    installation
        .administrator_sets_up_the_carrier_and_the_mail_domain()
        .await;

    let daemon = &installation.daemon;
    let org = daemon.stores().orgs.list().await.unwrap().remove(0);
    assert_ne!(org.workspace_id, daemon.workspace_id);
    let org_connections = daemon
        .stores()
        .connections
        .list(&org.workspace_id)
        .await
        .unwrap();
    assert_eq!(org_connections.len(), 2, "{org_connections:?}");
    assert!(
        daemon
            .stores()
            .connections
            .list(&daemon.workspace_id)
            .await
            .unwrap()
            .is_empty()
    );
    // The Org's Workspace is nobody's: no sweep of the people's
    // Workspaces reaches it.
    let people: Vec<_> = daemon
        .stores()
        .workspaces
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|workspace| workspace.id)
        .collect();
    assert_eq!(people, vec![daemon.workspace_id.clone()]);

    let (status, numbers) = installation
        .get_as(daemon.cookie(), "/api/v1/settings/phone-numbers")
        .await;
    assert_eq!(status, 200, "{numbers}");
    assert_eq!(numbers["carrier"]["status"], "connected", "{numbers}");
}
