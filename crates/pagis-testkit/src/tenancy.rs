//! The two-tenant harness.
//!
//! One Org, two people, one Workspace each. Person A holds a record of
//! every kind the REST surface reads by id, and person A's Call is live;
//! person B holds nothing. The harness then drives every authenticated
//! route of `crates/pagis-server` as person B against person A's ids
//! and requires a refusal.
//!
//! Two people in one Org is the shape a server actually runs, and it is
//! the harder case: they share the Org's provider keys, its Plugins and
//! its Software, and they differ only by Workspace.
//!
//! The route list comes from [`pagis_server::ROUTES`], which a test in
//! that crate proves holds every route the router builds. So a route
//! added without a tenant fails here instead of passing unnoticed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pagis_audit::AuditEventBus;
use pagis_core::{
    AgentMailbox, AgentMailboxState, Call, CallDirection, CallState, Connection, ConnectionId,
    Contribution, ContributionStatus, CreatorKind, Credential, CredentialId, CredentialProvenance,
    EventSubscription, EventSubscriptionId, EventSubscriptionState, PhoneNumber, PhoneNumberId,
    PluginId, PurchaseIntent, Schedule, ScheduleId, ScheduleKind, ScheduleState, SealedSecret,
    TrustEntry, TrustEntryId, TrustSubject, TrustTier, User, UserRole, Workspace, WorkspaceId,
    now_ms,
};
use pagis_server::ROUTES;
use pagis_telephony::fake::{FakeCallTransport, RemoteParty, TokioClock};
use pagis_telephony::hub::MediaHub;
use pagis_telephony::{
    CallArguments, CallBrief, CallLog, CallTransport, PlacedCall, SipCredential, TierGate,
    VoicemailPolicy,
};

use crate::TestDaemon;
use crate::fixture;

/// One person of the Org, with the Workspace they own and the `Cookie`
/// header of their Session.
pub struct Person {
    pub user_id: pagis_core::UserId,
    pub workspace_id: WorkspaceId,
    pub cookie: String,
}

/// A path parameter that names no record, with the reason it names
/// none. Every parameter of every route is either one of these or an id
/// the harness plants in person A's Workspace, and [`TwoTenants::start`]
/// refuses to run while one is neither.
const OPAQUE: &[(&str, &str)] = &[
    ("provider", "a provider name, the same for every Workspace"),
    (
        "alias",
        "a model alias name, read inside the tenant's own row",
    ),
    ("kind", "an Artifact class name from a fixed vocabulary"),
    (
        "sha",
        "a memory commit id; every read of it goes through the \
         MemoryStore seam, which takes the Workspace in its signature",
    ),
    (
        "package",
        "a Software package name on the Widget page routes. A Widget \
         page needs a materialized Version in the package's own bare \
         repository, which this harness does not build; the package \
         read itself is covered by the `name` routes",
    ),
    ("version", "a Software version name, as `package`"),
    ("widget", "a `[[widget]]` entry name, as `package`"),
    (
        "tool_call_id",
        "a live Widget view the broker holds in process, which nothing \
         outside a Run can plant",
    ),
    (
        "field",
        "a Plugin manifest field name; the route names the Plugin too, \
         and that id is planted",
    ),
    (
        "code",
        "a sign-in link secret, on a route that needs no Session",
    ),
    (
        "workspace_id",
        "the tenant itself, on the sandbox proxy route, which needs no \
         Session because a sandboxed frame sends no cookie. The \
         answer is a fixed document under the Content-Security-Policy \
         the named Workspace declares for that Widget, and a Workspace, \
         package, Version or Widget that names nothing installed answers \
         the same way under the closed default: the route holds no \
         record of any tenant. `software::the_sandbox_policy_is_the_named_tenants` \
         proves the two halves of that",
    ),
    (
        "message_id",
        "a Message id on the channel routes, and a mail `Message-ID` on \
         the mail route; the channel routes name the Channel too, and \
         the mail route names the mailbox",
    ),
];

/// The routes whose record belongs to the Org and not to one person
/// (ADR-0017).
///
/// A Plugin is installed once by the administrator for everybody: the
/// rows, the Bindings and the checkout are the Org's, and every Workspace
/// runs them in its own Plugin Computer. So person B reading the Plugin
/// person A installed is not person B reading person A's record, and the
/// sweep's two rules do not apply. What B may not reach is A's Plugin
/// Computer, A's Grants and A's Connections, which the entries below say
/// how each route keeps.
const ORG_OWNED: &[(&str, &str)] = &[
    (
        "/api/v1/plugins/{plugin_id}",
        "the Org's installed Plugin, which everybody in the Org reads. \
         The running state and the frozen catalogue in the answer come \
         from the asking person's own Plugin Computer.",
    ),
    (
        "/api/v1/plugins/{plugin_id}/log",
        "the Plugin log of the asking person's own Workspace. Person A's \
         Plugin server wrote [`A_PLUGIN_STDERR`] to A's log. Person B \
         reads B's own log, which is empty until B runs the Plugin, and \
         the sweep requires that B's answer does not hold A's line.",
    ),
];

/// The line person A's Plugin server wrote to stderr, in the Plugin log
/// of A's Workspace. A server can write Person data or a bound value to
/// stderr, so no answer to person B may hold it (ADR-0023).
pub const A_PLUGIN_STDERR: &str = "the stderr of person A's Plugin server";

/// The query string one route needs before its handler runs. Without
/// it `axum` refuses the request at the extractor, and the answer would
/// say nothing about the tenant.
const QUERIES: &[(&str, &str)] = &[(
    "/api/v1/channels/{channel_id}/messages/{message_id}/speech",
    "block=0",
)];

/// The routes that answer only a WebSocket handshake. `axum` refuses a
/// plain request at the upgrade extractor, before the handler reads
/// anything, so their answer says nothing about the tenant and the
/// harness holds them to the one rule that covers every method: they do
/// not succeed. Each one resolves the tenant from the same Session
/// cookie once the socket is up.
const UPGRADE_ONLY: &[&str] = &[
    "/api/v1/ws",
    "/api/v1/hosts/{host_id}/exit",
    "/api/v1/hosts/{host_id}/sessions",
    "/api/v1/channels/{channel_id}/dictate",
    "/api/v1/calls/{call_id}/listen",
];

/// What one route answered person B.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub path: String,
    pub method: String,
    pub status: u16,
}

/// What the sweep found.
#[derive(Debug, Default)]
pub struct Report {
    /// The routes the harness drove, with what each one answered.
    pub driven: Vec<Answer>,
    /// The authenticated routes with no planted id in their path: a
    /// cross-tenant read is not expressible through them.
    pub not_crossable: Vec<String>,
}

pub struct TwoTenants {
    pub daemon: TestDaemon,
    /// Person A: the person the boot seeded. Their Workspace holds one
    /// record of every kind the routes read by id, and their Call is
    /// live.
    pub a: Person,
    /// Person B: a second person of the same Org, with an empty
    /// Workspace of their own.
    pub b: Person,
    /// Person A's ids, by the path parameter each one fills.
    owned: BTreeMap<&'static str, String>,
    /// The Remote Party of person A's live Call. The Call stays live
    /// while the harness holds it.
    _a_remote_party: Arc<RemoteParty>,
}

impl TwoTenants {
    /// Boot the daemon, seed person A's records and person B's empty
    /// Workspace, and check that every path parameter of every route is
    /// accounted for.
    pub async fn start() -> Self {
        Self::on(TestDaemon::start().await).await
    }

    /// The same harness against Postgres, or `None` when Docker is not
    /// reachable. The tenant rule is a property of the store
    /// traits, so it holds on both backends or one of them is wrong.
    pub async fn start_on_postgres() -> Option<Self> {
        Some(Self::on(TestDaemon::start_on_postgres().await?).await)
    }

    /// The harness on a daemon the test built itself, for a test that
    /// needs a scripted brain or another option of its own.
    pub async fn on(daemon: TestDaemon) -> Self {
        let stores = daemon.stores().clone();
        let a_workspace = daemon.workspace_id.clone();

        // Person B joins the one Org that the boot seeded, with a
        // Workspace of their own and nothing in it.
        let users = &stores.users;
        let a_user = users
            .get(&daemon.user_id)
            .await
            .expect("read the seeded person")
            .expect("the boot seeds one person");
        let now = now_ms();
        let b_user = User {
            email: Some("b@example.com".to_string()),
            name: Some("B".to_string()),
            ..User::new(a_user.org_id.clone(), UserRole::Member, now)
        };
        users.create(&b_user).await.expect("write person B");
        let b_workspace = Workspace {
            id: WorkspaceId::generate(),
            user_id: b_user.id.clone(),
            name: "B's Workspace".to_string(),
            timezone: "UTC".to_string(),
            created_at: now,
            onboarded_at: None,
            chief_of_staff_agent_id: None,
            report_schedule_id: None,
            home_exit_host_id: None,
        };
        stores
            .workspaces
            .create(&b_workspace)
            .await
            .expect("write person B's Workspace");

        let (owned, a_remote_party) = seed_a(&daemon, &a_workspace).await;
        let b_cookie = daemon.cookie_for(&b_user.id).await;
        let harness = Self {
            a: Person {
                user_id: daemon.user_id.clone(),
                workspace_id: a_workspace,
                cookie: daemon.cookie().to_string(),
            },
            b: Person {
                user_id: b_user.id,
                workspace_id: b_workspace.id,
                cookie: b_cookie,
            },
            daemon,
            owned,
            _a_remote_party: a_remote_party,
        };
        harness.check_every_parameter_is_accounted_for();
        harness
    }

    /// One of person A's ids, by the path parameter it fills.
    pub fn a_id(&self, parameter: &str) -> &str {
        self.owned
            .get(parameter)
            .unwrap_or_else(|| panic!("no seeded id for {{{parameter}}}"))
    }

    /// Every path parameter of every route is either an id the harness
    /// plants or a parameter [`OPAQUE`] explains. A new parameter is
    /// neither, so this fails until somebody decides which it is.
    fn check_every_parameter_is_accounted_for(&self) {
        let opaque: BTreeSet<&str> = OPAQUE.iter().map(|(name, _)| *name).collect();
        let mut unaccounted = BTreeSet::new();
        for route in ROUTES {
            for parameter in parameters(route.path) {
                if !self.owned.contains_key(parameter) && !opaque.contains(parameter) {
                    unaccounted.insert(parameter.to_string());
                }
            }
        }
        assert!(
            unaccounted.is_empty(),
            "the two-tenant harness plants no id for {unaccounted:?}; seed one in \
             `seed_a`, or say in OPAQUE why the parameter names no record"
        );
        // An exception that names no route is an exception nothing needs.
        for (path, _) in ORG_OWNED {
            assert!(
                ROUTES.iter().any(|route| route.path == *path),
                "ORG_OWNED names {path}, which is not a route"
            );
        }
    }

    /// Drive every authenticated route as found in [`ROUTES`] as person
    /// B, against person A's ids.
    ///
    /// No answer may be a success: person B reaches nothing of person
    /// A's through any method. A `GET` must answer exactly 404, because
    /// it carries no body and so nothing can refuse it before the
    /// handler reads the record; person A's record has to read as
    /// absent. A write may refuse its empty body first, so it is held
    /// only to the rule that it does not succeed.
    pub async fn b_reaches_nothing_of_a(&self) -> Report {
        let client = reqwest::Client::new();
        let mut report = Report::default();
        let org_owned: BTreeSet<&str> = ORG_OWNED.iter().map(|(path, _)| *path).collect();
        for route in ROUTES.iter().filter(|route| route.authenticated) {
            let Some(path) = self.fill(route.path) else {
                report.not_crossable.push(route.path.to_string());
                continue;
            };
            let query = QUERIES
                .iter()
                .find(|(named, _)| *named == route.path)
                .map(|(_, query)| format!("?{query}"))
                .unwrap_or_default();
            for method in route.methods {
                let url = format!("{}{path}{query}", self.daemon.base_url);
                let body = serde_json::json!({});
                let request = match *method {
                    "get" => client.get(&url),
                    "post" => client.post(&url).json(&body),
                    "put" => client.put(&url).json(&body),
                    "delete" => client.delete(&url).json(&body),
                    other => panic!("the route table names an unknown method {other}"),
                };
                let response = request
                    .header("cookie", &self.b.cookie)
                    .send()
                    .await
                    .unwrap_or_else(|error| panic!("{method} {path}: {error}"));
                let status = response.status().as_u16();
                // A record the Org owns is not person A's, so neither rule
                // applies to it; ORG_OWNED says what each such route keeps
                // instead. Person B reads the Org's record and B's own
                // Plugin Computer, and nothing of A's Plugin Computer.
                if org_owned.contains(&route.path) {
                    let body = response.text().await.unwrap_or_default();
                    assert!(
                        !body.contains(A_PLUGIN_STDERR),
                        "{method} {path} answered person B with the Plugin log of person \
                         A's Workspace"
                    );
                    if *method == "get" {
                        assert_eq!(
                            status, 200,
                            "GET {path} answered person B with {status}; B reads the Org's \
                             Plugin and B's own Plugin Computer"
                        );
                    }
                    report.driven.push(Answer {
                        path: path.clone(),
                        method: (*method).to_string(),
                        status,
                    });
                    continue;
                }
                assert!(
                    !response.status().is_success(),
                    "{method} {path} answered person B with {status}; it reads person \
                     A's record without naming the tenant"
                );
                if *method == "get" && !UPGRADE_ONLY.contains(&route.path) {
                    assert_eq!(
                        status, 404,
                        "GET {path} answered person B with {status}, not 404; person \
                         A's record must read as absent"
                    );
                }
                report.driven.push(Answer {
                    path: path.clone(),
                    method: (*method).to_string(),
                    status,
                });
            }
        }
        report
    }

    /// The path with every parameter filled from person A's records, or
    /// `None` when the path names no record of A's and a cross-tenant
    /// read is therefore not expressible through it.
    fn fill(&self, path: &'static str) -> Option<String> {
        let names: Vec<&str> = parameters(path).collect();
        if names.is_empty() || !names.iter().any(|name| self.owned.contains_key(name)) {
            return None;
        }
        let mut filled = path.to_string();
        for name in names {
            let value = match self.owned.get(name) {
                Some(value) => value.clone(),
                // An opaque parameter beside a planted id: any value
                // does, because the planted id decides the answer.
                None => "opaque".to_string(),
            };
            filled = filled.replace(&format!("{{{name}}}"), &value);
        }
        Some(filled)
    }
}

/// The parameter names of one route path, in order.
fn parameters(path: &'static str) -> impl Iterator<Item = &'static str> {
    path.split('/').filter_map(|segment| {
        segment
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
    })
}

/// Install one Plugin for the Org through the Administration Interface,
/// as the Administrator does: a package with one Skill and no server,
/// uploaded as an Artifact. The answer is the Plugin's id.
async fn install_org_plugin(daemon: &TestDaemon) -> PluginId {
    let directory = tempfile::tempdir().expect("a scratch directory");
    std::fs::write(
        directory.path().join("plugin.json"),
        serde_json::json!({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name": "weather",
            "description": "The weather plugin.",
        })
        .to_string(),
    )
    .expect("the manifest");
    std::fs::create_dir_all(directory.path().join("skills/forecast")).expect("the skill");
    std::fs::write(
        directory.path().join("skills/forecast/SKILL.md"),
        "# Forecast\n",
    )
    .expect("the skill");
    let mut builder = tar::Builder::new(Vec::new());
    builder
        .append_dir_all(".", directory.path())
        .expect("the upload tar");
    let tar = builder.into_inner().expect("the upload tar");

    let client = reqwest::Client::new();
    let part = reqwest::multipart::Part::bytes(tar)
        .file_name("weather.tar".to_string())
        .mime_str("application/x-tar")
        .expect("the part");
    let artifact: serde_json::Value = client
        .post(format!("{}/api/v1/artifacts", daemon.base_url))
        .header("cookie", daemon.cookie())
        .multipart(reqwest::multipart::Form::new().part("file", part))
        .send()
        .await
        .expect("upload the package")
        .json()
        .await
        .expect("the artifact");
    let installed: serde_json::Value = client
        .post(format!("{}/api/v1/plugins", daemon.administration_base_url))
        .header("cookie", daemon.cookie())
        .json(&serde_json::json!({
            "source": {"kind": "upload", "artifact_id": artifact["id"]},
            "bindings": [],
        }))
        .send()
        .await
        .expect("install the plugin")
        .json()
        .await
        .expect("the installed plugin");
    PluginId::from(
        installed["id"]
            .as_str()
            .unwrap_or_else(|| panic!("the install answered {installed}"))
            .to_string(),
    )
}

/// Write one stderr line of a Plugin server into the Plugin log of one
/// Workspace, where the daemon keeps it.
fn write_plugin_stderr(
    daemon: &TestDaemon,
    workspace_id: &WorkspaceId,
    plugin_id: &PluginId,
    line: &str,
) {
    let path = pagis_plugins::PluginLogs::new(pagis::plugin_logs_directory(&daemon.booted.home))
        .path(workspace_id, plugin_id);
    std::fs::create_dir_all(path.parent().expect("the log directory")).expect("the log directory");
    std::fs::write(&path, format!("[weather] {line}\n")).expect("the plugin log");
}

/// One record of every kind the routes read by id, in person A's
/// Workspace. The values come back keyed by the path parameter each one
/// fills, with the Remote Party of person A's live Call.
async fn seed_a(
    daemon: &TestDaemon,
    workspace_id: &WorkspaceId,
) -> (BTreeMap<&'static str, String>, Arc<RemoteParty>) {
    let stores = daemon.stores().clone();
    let now = now_ms();
    let agent_id = pagis_core::AgentId::from(daemon.agent_id.clone());
    let channel_id = pagis_core::ChannelId::from(daemon.dm_channel_id.clone());
    let mut owned: BTreeMap<&'static str, String> = BTreeMap::new();
    owned.insert("agent_id", daemon.agent_id.clone());
    owned.insert("channel_id", daemon.dm_channel_id.clone());
    // Person A is the installation's administrator. Person B
    // drives the administration routes against A's own id, so a Member
    // who disabled, reset or capped the administrator would fail here.
    owned.insert("user_id", daemon.user_id.to_string());

    // A Session of A's own, beside the one the harness signs A in with,
    // so a route that ended it would not end the harness.
    let session = pagis_core::Session {
        id: pagis_core::SessionId::generate(),
        user_id: daemon.user_id.clone(),
        token_hash: "the hash of A's other Session".to_string(),
        client_kind: pagis_core::ClientKind::Browser,
        client_name: Some("Safari on iPhone".to_string()),
        created_at: now,
        last_used_at: now,
        expires_at: now + pagis_core::SESSION_LIFETIME_MS,
    };
    stores
        .sessions
        .create(&session)
        .await
        .expect("write A's other Session");
    owned.insert("session_id", session.id.to_string());

    // The phone of that other Session subscribed to Web Push (ADR-0030).
    // No route reads the keys back, so they need not be real keys.
    let push_subscription = stores
        .push_subscriptions
        .upsert(&pagis_core::PushSubscription {
            id: pagis_core::PushSubscriptionId::generate(),
            workspace_id: workspace_id.clone(),
            session_id: session.id.clone(),
            endpoint: "https://push.example.com/send/a".to_string(),
            p256dh: "the public key of A's phone".to_string(),
            auth: "the auth secret of A's phone".to_string(),
            created_at: now,
            last_sent_at: None,
        })
        .await
        .expect("write A's push subscription");
    owned.insert("push_subscription_id", push_subscription.id.to_string());

    let messages = &stores.messages;
    let message = fixture::user_message(workspace_id, &channel_id, "A's own words");
    messages.insert(&message).await.expect("write A's message");
    owned.insert("root_message_id", message.id.to_string());

    let runs = &stores.runs;
    let run = fixture::queued_run(workspace_id, &agent_id, &channel_id);
    runs.create(&run).await.expect("write A's run");
    owned.insert("run_id", run.id.to_string());

    let requests = &stores.requests;
    let request = fixture::pending_request(workspace_id, &agent_id, &run.id, "echo hi");
    requests.create(&request).await.expect("write A's request");
    owned.insert("request_id", request.id.to_string());

    // A's own machine, and the grant that names it.
    let host = stores
        .hosts
        .register(workspace_id, "A's laptop", "macos", &[], now)
        .await
        .expect("write A's host");
    owned.insert("host_id", host.id.to_string());

    let grants = &stores.grants;
    let grant = fixture::host_grant(workspace_id, &agent_id, &host.id, &["echo"]);
    grants.create(&grant).await.expect("write A's grant");
    owned.insert("grant_id", grant.id.to_string());

    let artifacts = &stores.artifacts;
    let artifact = fixture::artifact(workspace_id, "aa11");
    artifacts
        .insert(&artifact)
        .await
        .expect("write A's artifact");
    owned.insert("artifact_id", artifact.id.to_string());

    let connections = &stores.connections;
    let connection = Connection {
        id: ConnectionId::generate(),
        workspace_id: workspace_id.clone(),
        provider: "google".to_string(),
        alias: "work".to_string(),
        display_name: "Work".to_string(),
        status: Connection::CONNECTED.to_string(),
        auth_mode: "brokered".to_string(),
        authorized_capabilities: vec!["gmail_read".to_string()],
        config: serde_json::json!({"account": "a@example.com", "client": "c"}),
        created_at: now,
    };
    connections
        .create(&connection)
        .await
        .expect("write A's connection");
    owned.insert("connection_id", connection.id.to_string());

    let credentials = &stores.credentials;
    let credential = Credential {
        id: CredentialId::generate(),
        workspace_id: workspace_id.clone(),
        domain: "example.com".to_string(),
        username: "ada".to_string(),
        login_url: "https://example.com/login".to_string(),
        secret: SealedSecret(b"sealed".to_vec()),
        totp_seed: None,
        recipe: String::new(),
        owner_agent_id: None,
        provenance: CredentialProvenance::UserSupplied,
        created_run_id: None,
        created_at: now,
    };
    credentials
        .create(&credential)
        .await
        .expect("write A's credential");
    owned.insert("credential_id", credential.id.to_string());

    let schedules = &stores.schedules;
    let schedule = Schedule {
        id: ScheduleId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        name: "Morning plan".to_string(),
        instruction: "Write the plan".to_string(),
        subject_page_path: None,
        channel_id: channel_id.clone(),
        root_message_id: None,
        kind: ScheduleKind::Cron,
        cron_expression: Some("0 7 * * *".to_string()),
        interval_ms: None,
        anchor_at: None,
        timezone: "UTC".to_string(),
        scheduled_at: now,
        next_due_at: Some(now + 60_000),
        last_result: None,
        state: ScheduleState::Active,
        revision: 1,
        approved_revision: Some(1),
        creator: CreatorKind::User,
        creating_run_id: None,
        created_at: now,
        updated_at: now,
        archived_at: None,
    };
    schedules
        .create(&schedule)
        .await
        .expect("write A's schedule");
    owned.insert("schedule_id", schedule.id.to_string());

    let subscriptions = &stores.subscriptions;
    let subscription = EventSubscription {
        id: EventSubscriptionId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        source: pagis_core::EventSource::connection(connection.id.clone()),
        event_kind: "google.mail.received".to_string(),
        source_version: "v1".to_string(),
        name: "New mail".to_string(),
        instruction: "Read it".to_string(),
        channel_id: channel_id.clone(),
        root_message_id: None,
        filter: serde_json::json!({}),
        creator: CreatorKind::User,
        state: EventSubscriptionState::Active,
        revision: 1,
        approved_revision: Some(1),
        watermark_at: None,
        blocked_reason: None,
        created_at: now,
        updated_at: now,
        archived_at: None,
    };
    subscriptions
        .create(&subscription)
        .await
        .expect("write A's subscription");
    owned.insert("subscription_id", subscription.id.to_string());

    let numbers = &stores.phone_numbers;
    let intent = PurchaseIntent {
        id: pagis_core::PurchaseIntentId::generate(),
        workspace_id: workspace_id.clone(),
        connection_id: connection.id.clone(),
        e164: "+14155550123".to_string(),
        agent_id: None,
        state: pagis_core::PurchaseIntentState::Pending,
        created_at: now,
        settled_at: None,
    };
    numbers
        .create_intent(&intent)
        .await
        .expect("write A's purchase intent");
    let number = PhoneNumber::new(
        PhoneNumberId::generate(),
        workspace_id.clone(),
        connection.id.clone(),
        intent.e164.clone(),
        "pn-1".to_string(),
        None,
        now,
    );
    numbers
        .confirm_purchase(workspace_id, &intent.id, &number, now)
        .await
        .expect("write A's number");
    owned.insert("phone_number_id", number.id.to_string());

    let calls = &stores.calls;
    let call = Call {
        id: pagis_core::CallId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        run_id: run.id.clone(),
        phone_number_id: number.id.clone(),
        direction: CallDirection::Outbound,
        remote_e164: "+14155550999".to_string(),
        agent_name: "Sage".to_string(),
        own_e164: number.e164.clone(),
        purpose: "Book the room".to_string(),
        tools: Vec::new(),
        tier: TrustTier::Trusted,
        state: CallState::Live,
        outcome: None,
        ended_reason: None,
        classification: None,
        message_left: false,
        transcript: Vec::new(),
        recording_artifact_id: None,
        created_at: now,
        ringing_at: Some(now),
        answered_at: Some(now),
        ended_at: None,
        dismissed_at: None,
    };
    calls.insert(&call).await.expect("write A's call");
    owned.insert("call_id", call.id.to_string());
    let remote_party = make_live(daemon, &call, &number).await;

    // A Plugin is the Org's: the Administrator installs it once, and
    // every Workspace runs it in its own Plugin Computer (ADR-0017).
    // Person A's Plugin Computer has run it, and its server wrote one
    // line to stderr.
    let plugin_id = install_org_plugin(daemon).await;
    write_plugin_stderr(daemon, workspace_id, &plugin_id, A_PLUGIN_STDERR);
    owned.insert("plugin_id", plugin_id.to_string());

    let trust = &stores.trust_list;
    let entry = TrustEntry {
        id: TrustEntryId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: None,
        subject: TrustSubject::Number,
        value: "+14155550999".to_string(),
        tier: TrustTier::Trusted,
        label: "Home".to_string(),
        created_at: now,
    };
    trust.upsert(&entry).await.expect("write A's trust entry");
    owned.insert("trust_entry_id", entry.id.to_string());

    let packages = &stores.software;
    let package = fixture::software_package(workspace_id, &agent_id, "almanac");
    packages
        .create_package(&package)
        .await
        .expect("write A's package");
    owned.insert("name", package.name.clone());

    let contributions = &stores.contributions;
    let fork = fixture::software_package(workspace_id, &agent_id, "almanac-fork");
    packages
        .create_package(&fork)
        .await
        .expect("write A's fork");
    let contribution = Contribution {
        id: pagis_core::ContributionId::generate(),
        workspace_id: workspace_id.clone(),
        package_id: package.id.clone(),
        base_version: "v1".to_string(),
        latest_at_open: "v1".to_string(),
        fork_package_id: fork.id.clone(),
        fork_version: "v1".to_string(),
        patch: "--- a\n+++ b\n".to_string(),
        summary: "one change".to_string(),
        status: ContributionStatus::Open,
        outcome_reason: None,
        created_at: now,
        closed_at: None,
        run_id: run.id.clone(),
    };
    contributions
        .create(&contribution)
        .await
        .expect("write A's contribution");
    owned.insert("contribution_id", contribution.id.to_string());

    let mailboxes = &stores.agent_mailboxes;
    let mailbox = AgentMailbox {
        id: pagis_core::AgentMailboxId::generate(),
        workspace_id: workspace_id.clone(),
        agent_id: agent_id.clone(),
        connection_id: connection.id.clone(),
        address: "sage@example.com".to_string(),
        state: AgentMailboxState::Active,
        reason: None,
        outgoing_cap: 20,
        allow_rules: Vec::new(),
        cursor: None,
        sends_day: None,
        sends_today: 0,
        created_at: now,
        deleted_at: None,
    };
    mailboxes.create(&mailbox).await.expect("write A's mailbox");
    owned.insert("mailbox", mailbox.address.clone());

    // The Forget operation has no store method that writes one without
    // a memory preview, so the row goes in directly. It is A's row and
    // that is all the harness needs of it.
    daemon
        .rows()
        .execute(
            "INSERT INTO forget_operations (id, workspace_id, target, phase, created_at) \
             VALUES (?, ?, ?, 'structured', ?)",
            &[
                crate::sql::Bind::from("forget-a"),
                crate::sql::Bind::from(workspace_id.as_str()),
                crate::sql::Bind::from(
                    serde_json::json!({"connection_id": connection.id.as_str()}).to_string(),
                ),
                crate::sql::Bind::from(now),
            ],
        )
        .await
        .expect("write A's forget operation");
    owned.insert("id", "forget-a".to_string());

    (owned, remote_party)
}

/// Make person A's Call live, as the bridge does when a call starts:
/// its Media Hub goes into the daemon's live calls, and its tier gate
/// goes into the live tiers. A Call that is not live answers 404 to
/// every Person, so only a live Call proves that a route names the
/// Workspace.
///
/// The Remote Party never answers, so the hub waits for no media and
/// the Call stays live for as long as a test runs.
async fn make_live(daemon: &TestDaemon, call: &Call, number: &PhoneNumber) -> Arc<RemoteParty> {
    let transport = FakeCallTransport::new(Arc::new(TokioClock));
    let mut opened = transport
        .open(&SipCredential::new("a", "secret", "sip.example.test"))
        .await
        .expect("open A's line");
    let leg = opened
        .line
        .dial(&call.own_e164, &call.remote_e164)
        .await
        .expect("dial the Remote Party of A's call");
    let remote_party = transport.dials().pop().expect("one dialed party");
    daemon
        .live_calls
        .attach_hub(&call.workspace_id, &call.id, MediaHub::start(leg));

    let agent = daemon
        .stores()
        .agents
        .get(&call.workspace_id, &call.agent_id)
        .await
        .expect("read A's Agent")
        .expect("A's Agent");
    let arguments = CallArguments {
        to: call.remote_e164.clone(),
        brief: call.purpose.clone(),
        success_criteria: "The room is booked".to_string(),
        voicemail: VoicemailPolicy::default(),
        max_duration_s: None,
        tools: None,
    };
    let placed = PlacedCall {
        id: call.id.clone(),
        workspace_id: call.workspace_id.clone(),
        run_id: call.run_id.clone(),
        brief: CallBrief::outbound(&agent, number, &arguments, call.tier, Vec::new()),
        tools: Vec::new(),
    };
    let bus = Arc::new(AuditEventBus::new(daemon.stores().events.clone()));
    daemon.live_tiers.bind(
        &call.workspace_id,
        &call.id,
        Arc::new(TierGate::outbound(call.tier)),
        Arc::new(CallLog::new(bus, &placed)),
    );
    remote_party
}
