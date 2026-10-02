//! The route tables of the daemon.
//!
//! [`ROUTES`] is the product port, and the two-tenant harness drives
//! every row of it, so a route that reaches a workspace-owned record
//! without naming the tenant fails loudly. [`ADMINISTRATION_ROUTES`] is
//! the administration port. Both tables are hand-written, and the tests
//! below prove each one holds every route its router builds, with the
//! same methods and the same side of the session middleware.

/// One route of the daemon: the path as `axum` matches it, the methods
/// it answers, and whether it sits behind the session middleware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub path: &'static str,
    pub methods: &'static [&'static str],
    /// `false` for the ways in and the reads that hold no person's
    /// data: they answer without a Session. [`PUBLIC_ROUTES`] names
    /// every one of them and says why each holds no workspace content.
    pub authenticated: bool,
}

/// Every route outside the session middleware, with why it returns no
/// workspace content.
///
/// A route that reads a record of a Workspace belongs behind the
/// middleware. The test below holds the public set of [`ROUTES`] to
/// exactly this list, so a route that leaves the authenticated layer
/// has to be argued for here first.
pub const PUBLIC_ROUTES: &[(&str, &str)] = &[
    (
        "/api/v1/widgets/{workspace_id}/{package}/{version}/{widget}/sandbox",
        "The sandbox proxy document. The page loads it from the \
         other loopback host, so the browser sends no Session cookie \
         with it: the cookie is SameSite=Strict, and the separate origin \
         is what keeps a Widget out of the page that frames it. The \
         body is a fixed HTML document. The one thing it reads is the \
         Content-Security-Policy a Version declares for that Widget, \
         which the browser has to enforce on content the page cannot \
         rewrite. The Workspace is in the path because no cookie can \
         reach here, so a Widget of one tenant gets that \
         tenant's own declaration; a Workspace, package, Version or \
         Widget that names nothing installed answers under the closed \
         default and says nothing about any tenant either way. It \
         carries no message, no memory, no Credential and no file.",
    ),
    (
        "/api/v1/connections/google/start",
        "The start route of a brokered Google authorization, which the \
         authorize request answers. It cannot sit behind the session \
         middleware: a browser with no Session, such as the system \
         browser that the Client App opens, must sign in and come back, \
         not get 401. It reads the Session itself. With no Session it \
         redirects to the same address in the Product App, which shows \
         the sign-in page. A Session of a Person other than the one who \
         started the authorization gets a fixed refusal page. Only for \
         that Person does it set the transaction cookie, which holds the \
         hash of the `state`, and redirect to Google. A local \
         installation with Remote Access off has one Person and \
         answers only programs of its own machine, so there it asks for \
         no Session, and the callback keeps every check. It returns no \
         workspace content.",
    ),
    (
        "/api/v1/connections/google/callback",
        "Where Google redirects a person's browser after they consent. \
         It cannot sit behind the session middleware: the \
         Session cookie is SameSite=Strict, so a browser sends none on a \
         cross-site navigation, and Google's redirect is one. The \
         request carries the `state` the daemon minted, which is 256 \
         bits of randomness, spent on first use and good for ten \
         minutes; a value this daemon did not mint reaches no record at \
         all. The route finishes only for the browser that holds the \
         transaction cookie of that `state`, while the Session that \
         started the authorization is live, and only for the Google \
         account of the Connection. The answer is a fixed page with one \
         sentence on it: it names no account, no Workspace and no \
         Connection, so a browser that arrives with somebody else's \
         `state` learns nothing from it. It returns no workspace \
         content.",
    ),
    (
        "/api/v1/setup",
        "The read of the server's own first run, from which the \
         sign-in page names the Administration Port. It cannot sit \
         behind the session middleware: it exists because nobody can \
         sign in yet. It answers only while the installation holds no \
         Client Credential, which says it is a server and not a local \
         installation, and no administrator holds a password, which \
         says nobody can sign in; the first password closes it for \
         good and it answers `410 Gone` from then on. It says which \
         model providers the installation may take a key for and which \
         already hold one, by provider name alone. It names no person, \
         reads no Workspace record and returns no key. Server Setup \
         posts on the Administration Port alone, so this port makes no \
         Administrator.",
    ),
    (
        "/api/v1/health",
        "The liveness answer the desktop shell reads before it attaches \
         (ADR-0025): a status and the daemon version. The sign-in page \
         reads from it how its browser signs in, a password or a \
         Sign-In Link (ADR-0028), which follows the setting and the \
         address of the request alone.",
    ),
    (
        "/api/v1/runtime/identity",
        "The proof the Client App reads before it attaches \
         (ADR-0025). It answers only where the daemon holds a Client \
         Credential, which a server does not, and the caller must hold \
         the same file to check the proof. The one Workspace id of a \
         local installation is in it because the client compares it \
         with the installation it started; no record of that Workspace \
         is.",
    ),
    (
        "/api/v1/openapi.json",
        "The API contract. It describes the shapes of the routes and \
         holds no data of anybody.",
    ),
    (
        "/api/v1/sessions",
        "A way in: it takes a password and hands out the Session \
         the other routes need.",
    ),
    (
        "/api/v1/sessions/client",
        "A way in: the Client App proves the Client \
         Credential and gets a Session.",
    ),
    (
        "/api/v1/sessions/link",
        "A way in: the secret of a Sign-In Link of the Public Origin \
         is the credential, and the route hands out a Session. It counts \
         each refusal against the source address, as a password does.",
    ),
    (
        "/api/v1/sessions/link/{code}",
        "A way in: the secret of the start link is the credential, and \
         the route hands out a Session to a browser on this machine.",
    ),
];

/// Every route of the administration port outside its two guard layers,
/// with why each one answers without a signed-in Administrator.
///
/// The port exists to be private, so this list is meant to stay two
/// entries long. A route that leaves the guard has to be argued for
/// here first, and the test below holds the set to exactly this list.
pub const ADMINISTRATION_PUBLIC_ROUTES: &[(&str, &str)] = &[
    (
        "/api/v1/setup",
        "The server's own first run. It cannot sit behind the \
         guard: it exists because nobody can sign in yet, so there is no \
         Administrator to require. It answers only while the \
         installation holds no Client Credential, which says it is a \
         server, and no administrator holds a password, which says \
         nobody can sign in; the first password closes it for good and \
         it answers `410 Gone` from then on. The read names the model \
         providers the installation may take a key for and which \
         already hold one, by provider name alone. The write answers \
         here alone: this port binds loopback by default, so the first \
         Administrator is made by a person who reaches the Server's own \
         machine.",
    ),
    (
        "/api/v1/sessions",
        "The way in on this port. The guard asks for a Session, \
         and this is what hands one out. It cannot be left to the \
         product port: an administrator who reaches this port through a \
         tunnel or a private interface may not reach that one at all, \
         and a private port whose only way in is the public port is not \
         private. It takes an address and a password and answers the \
         same refusal for an address nobody holds, a person with no \
         password and a wrong password.",
    ),
];

/// The routes both ports answer, with the methods each answers on both
/// and why the product port holds them too.
///
/// Every other route of [`ADMINISTRATION_ROUTES`] is the installation's
/// and answers on the administration port alone (ADR-0024): the people,
/// the spend, the Sessions, the Hosts, the resources, the installation
/// settings, the System Settings, the restart, the Plugins the Org
/// installs and the installation's setup of every provider. The test below holds the overlap to exactly this list, so
/// a route of the installation that comes back to the product port
/// fails until somebody argues for it here.
pub const SHARED_ROUTES: &[(&str, &[&str], &str)] = &[
    (
        "/api/v1/user",
        &["get"],
        "Who the page serves. Each port draws a page for the signed-in \
         person, and the product answer is the person's own record.",
    ),
    (
        "/api/v1/sessions",
        &["post"],
        "The password sign-in. Each port needs a way in of its own, so \
         that the private port does not depend on the public one.",
    ),
    (
        "/api/v1/sessions/current",
        &["delete"],
        "Sign out. A person ends their own Session on the port they \
         signed in on.",
    ),
    (
        "/api/v1/setup",
        &["get"],
        "The read of the server's own first run. The sign-in page of \
         the product port names the Administration Port from it. It \
         answers while nobody can sign in and `410 Gone` from the first \
         password onwards. The write is on the administration port \
         alone, because a write on the product port would give the \
         first Administrator to whoever reaches the public name first.",
    ),
    (
        "/api/v1/plugins",
        &["get"],
        "The Plugins the Org installed, which everybody in the Org reads \
         so they can grant one to their own Agent. The install is on the \
         administration port alone.",
    ),
    (
        "/api/v1/plugins/{plugin_id}",
        &["get"],
        "One installed Plugin, read by everybody in the Org. The \
         uninstall is on the administration port alone.",
    ),
];

/// Every route of the administration port.
///
/// `authenticated` is `true` for the routes inside the guard layers,
/// which is every route but the two of
/// [`ADMINISTRATION_PUBLIC_ROUTES`]. A guarded route answers `401`
/// without a Session and `403` to a Member, whatever extractor its
/// handler takes: the guard is the router's.
pub const ADMINISTRATION_ROUTES: &[Route] = &[
    Route {
        path: "/api/v1/administration/people",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/people/{user_id}/disable",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/people/{user_id}/enable",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/people/{user_id}/password",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/people/{user_id}/sign-in",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/people/{user_id}/sign-in-links",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/people/{user_id}/spend-cap",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/usage",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/sessions",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/hosts",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/resources",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/health",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/computer-image/pull",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/user",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/sessions/current",
        methods: &["delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/system",
        methods: &["get", "put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/system/remote-access",
        methods: &["get", "put", "delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/system/analytics",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/system/docker/probe",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/providers",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/providers/{provider}/{part}",
        methods: &["put", "delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/administration/providers/{provider}/{part}/test",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/system/restart",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/setup",
        methods: &["get", "post"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/sessions",
        methods: &["post"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/plugins",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/plugins/{plugin_id}",
        methods: &["get", "delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/plugins/{plugin_id}/update",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/plugins/{plugin_id}/bindings/{field}",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/plugins/{plugin_id}/start",
        methods: &["post"],
        authenticated: true,
    },
];

/// Every route the daemon serves.
pub const ROUTES: &[Route] = &[
    Route {
        path: "/api/v1/knowledge/forget",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/knowledge/forget/preview",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/knowledge/forget/{id}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/knowledge/forget/{id}/retry",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/knowledge/forget/{id}/reopt",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/connections/{connection_id}/sync",
        methods: &["get", "put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/connections/{connection_id}/sync/catalogue",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/connections/{connection_id}/sync/filter/preview",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/sync-connections",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/appearance",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/archive",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/mailbox",
        methods: &["get", "post", "delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/mailboxes",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/mailbox-offers",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/mailbox-offers/name",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/mailbox/reset-password",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/mail/{mailbox}/{message_id}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/computer",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/computer/wake",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/computer/sleep",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/computers/disk",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/screen/ice",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/screen/preview.png",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/screen/offer",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/screen/takeover",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/agents/{agent_id}/screen/handback",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/channels",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/channels/{channel_id}/messages",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/channels/{channel_id}/messages/{message_id}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/channels/{channel_id}/messages/{message_id}/speech",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/channels/{channel_id}/threads/{root_message_id}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/voices",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/artifacts",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/artifacts/{artifact_id}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/calls/{call_id}/control",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/runs/{run_id}/cancel",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/runs/{run_id}/dismiss",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/runs/{run_id}/retry",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/runs/{run_id}/events",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/runs/{run_id}/steps",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/runs",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/schedules",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/schedules/{schedule_id}",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/schedules/{schedule_id}/archive",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/schedules/{schedule_id}/run",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/schedules/{schedule_id}/revisions",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/schedules/{schedule_id}/occurrences",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/schedules/{schedule_id}/wakeups",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/event-subscriptions",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/event-subscriptions/{subscription_id}",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/event-subscriptions/{subscription_id}/events",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/event-subscriptions/{subscription_id}/wakeups",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/requests",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/requests/{request_id}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/requests/{request_id}/decision",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/memory/feed",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/memory/commits/{sha}/revert",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/memory/file",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/memory/pages",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/memory/pages/counts",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/memory/commits/{sha}/diff",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/hosts",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/grants",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/grants/{grant_id}/rules",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/grants/{grant_id}/capabilities",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/grants/{grant_id}",
        methods: &["delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/user",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/workspace",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/workspace/chief-of-staff",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/workspace/timezone",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/workspace/report",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/onboarding",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/onboarding/complete",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/onboarding/providers/{provider}/key",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/onboarding/providers/{provider}/key/check",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/onboarding/docker-endpoint",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/onboarding/default-model",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/providers/{provider}/check",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/models",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/model-aliases/{alias}",
        methods: &["put", "delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/model-aliases",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/retention",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/retention/{kind}",
        methods: &["put"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/connections/providers",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/connections",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/connections/{connection_id}",
        methods: &["delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/connections/{connection_id}/authorize",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/trust-list",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/trust-list/{trust_entry_id}",
        methods: &["delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/keypad-code",
        methods: &["put", "delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/keypad-code/failures",
        methods: &["delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/calls",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/calls/{call_id}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/calls/{call_id}/dismiss",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/calls/{call_id}/hangup",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/calls/{call_id}/tier",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/phone-numbers",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/phone-numbers/adopt",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/phone-numbers/available",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/phone-numbers/{phone_number_id}/assign",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/phone-numbers/{phone_number_id}/unassign",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/phone-numbers/{phone_number_id}/release",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/credentials",
        methods: &["get", "post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/credentials/{credential_id}",
        methods: &["delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/plugins",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/plugins/{plugin_id}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/plugins/{plugin_id}/grants",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/plugins/{plugin_id}/log",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/widgets/{package}/{version}/{widget}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/widgets/{tool_call_id}/view",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/widgets/{tool_call_id}/rpc",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/software",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/software/{name}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/software/{name}/contributions/{contribution_id}",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/usage",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/sessions/current",
        methods: &["delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/sessions",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/sessions/{session_id}",
        methods: &["delete"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/settings/sign-in-links",
        methods: &["post"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/ws",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/hosts/{host_id}/exit",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/channels/{channel_id}/dictate",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/calls/{call_id}/listen",
        methods: &["get"],
        authenticated: true,
    },
    Route {
        path: "/api/v1/widgets/{workspace_id}/{package}/{version}/{widget}/sandbox",
        methods: &["get"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/connections/google/start",
        methods: &["get"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/connections/google/callback",
        methods: &["get"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/health",
        methods: &["get"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/runtime/identity",
        methods: &["get"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/setup",
        methods: &["get"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/openapi.json",
        methods: &["get"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/sessions",
        methods: &["post"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/sessions/client",
        methods: &["post"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/sessions/link",
        methods: &["post"],
        authenticated: false,
    },
    Route {
        path: "/api/v1/sessions/link/{code}",
        methods: &["get"],
        authenticated: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `.route(...)` of one router, read out of its source: the
    /// path, the methods, and whether it is inside the authenticated
    /// router. A route added to a router and not to its table fails the
    /// test that compares them.
    fn routes_in_source(source: &str) -> Vec<(String, Vec<String>, bool)> {
        let middleware = source
            .find(".layer(middleware::from_fn_with_state(")
            .expect("the session middleware closes the authenticated router");
        let mut found = Vec::new();
        let bytes = source.as_bytes();
        let mut at = 0;
        while let Some(offset) = source[at..].find(".route(") {
            let start = at + offset;
            let open = start + ".route(".len() - 1;
            let quote = source[open..]
                .find('"')
                .expect("a route names its path first")
                + open;
            let close_quote = source[quote + 1..]
                .find('"')
                .expect("the path is a complete literal")
                + quote
                + 1;
            let path = source[quote + 1..close_quote].to_string();
            let mut depth = 0_i32;
            let mut cursor = open;
            loop {
                match bytes[cursor] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                cursor += 1;
            }
            let body = &source[open..cursor];
            let methods = ["get", "post", "put", "delete", "patch"]
                .into_iter()
                .filter(|method| body.contains(&format!("{method}(")))
                .map(str::to_string)
                .collect();
            found.push((path, methods, start < middleware));
            at = cursor;
        }
        found
    }

    /// One table as the comparison wants it.
    fn table(routes: &[Route]) -> Vec<(String, Vec<String>, bool)> {
        let mut rows: Vec<(String, Vec<String>, bool)> = routes
            .iter()
            .map(|route| {
                (
                    route.path.to_string(),
                    route.methods.iter().map(|m| (*m).to_string()).collect(),
                    route.authenticated,
                )
            })
            .collect();
        rows.sort();
        rows
    }

    /// The public set is exactly [`PUBLIC_ROUTES`]. A route that moves
    /// out of the authenticated layer fails here until somebody writes
    /// down why it carries no workspace content.
    #[test]
    fn the_public_routes_are_exactly_the_documented_ones() {
        let mut public: Vec<&str> = ROUTES
            .iter()
            .filter(|route| !route.authenticated)
            .map(|route| route.path)
            .collect();
        public.sort_unstable();
        let mut documented: Vec<&str> = PUBLIC_ROUTES.iter().map(|(path, _)| *path).collect();
        documented.sort_unstable();
        assert_eq!(
            public, documented,
            "the routes outside the session middleware are not the documented set; \
             a route that returns workspace content belongs behind the middleware, \
             and one that does not belongs in PUBLIC_ROUTES with its reason"
        );
        for (path, reason) in PUBLIC_ROUTES {
            assert!(
                reason.len() > 40,
                "{path} is public and says too little about why"
            );
        }
    }

    #[test]
    fn the_route_table_holds_every_route_the_router_builds() {
        let mut from_source = routes_in_source(include_str!("lib.rs"));
        from_source.sort();
        assert_eq!(
            table(ROUTES),
            from_source,
            "ROUTES is out of step with the router; add the new route to it"
        );
    }

    /// The administration port's table holds every route its router
    /// builds.
    #[test]
    fn the_administration_table_holds_every_route_that_router_builds() {
        let mut from_source = routes_in_source(include_str!("administration.rs"));
        from_source.sort();
        assert_eq!(
            table(ADMINISTRATION_ROUTES),
            from_source,
            "ADMINISTRATION_ROUTES is out of step with the administration router; \
             add the new route to it"
        );
    }

    /// The routes outside the administration guard are exactly the
    /// documented ones. A route that leaves the guard fails here
    /// until somebody writes down why it answers without an
    /// Administrator.
    #[test]
    fn the_administration_public_routes_are_exactly_the_documented_ones() {
        let mut public: Vec<&str> = ADMINISTRATION_ROUTES
            .iter()
            .filter(|route| !route.authenticated)
            .map(|route| route.path)
            .collect();
        public.sort_unstable();
        let mut documented: Vec<&str> = ADMINISTRATION_PUBLIC_ROUTES
            .iter()
            .map(|(path, _)| *path)
            .collect();
        documented.sort_unstable();
        assert_eq!(
            public, documented,
            "the routes outside the administration guard are not the documented set; \
             a route of the administration port requires an administrator unless \
             ADMINISTRATION_PUBLIC_ROUTES says why it cannot"
        );
        for (path, reason) in ADMINISTRATION_PUBLIC_ROUTES {
            assert!(
                reason.len() > 40,
                "{path} answers without an administrator and says too little about why"
            );
        }
    }

    /// The product port holds no route of the installation (ADR-0024).
    /// Every route of the administration port answers there alone, but
    /// for the documented overlap of [`SHARED_ROUTES`].
    #[test]
    fn the_product_port_holds_no_administration_route() {
        for route in ROUTES {
            assert!(
                !route.path.starts_with("/api/v1/administration/"),
                "{} is an administration route on the product port",
                route.path
            );
        }
        let shared = |path: &str, method: &str| {
            SHARED_ROUTES
                .iter()
                .any(|(shared, methods, _)| *shared == path && methods.contains(&method))
        };
        for administration in ADMINISTRATION_ROUTES {
            let Some(product) = ROUTES
                .iter()
                .find(|route| route.path == administration.path)
            else {
                continue;
            };
            for method in administration.methods {
                assert!(
                    !product.methods.contains(method) || shared(administration.path, method),
                    "{method} {} belongs to the installation and answers on the \
                     administration port alone; SHARED_ROUTES says why a route \
                     answers on both",
                    administration.path
                );
            }
        }
        // The list names only what both ports hold, so it cannot grow
        // stale and hide a route that moved.
        for (path, methods, why) in SHARED_ROUTES {
            for method in *methods {
                for table in [ROUTES, ADMINISTRATION_ROUTES] {
                    assert!(
                        table
                            .iter()
                            .any(|route| route.path == *path && route.methods.contains(method)),
                        "SHARED_ROUTES names {method} {path}, which one port does not hold"
                    );
                }
            }
            assert!(
                why.len() > 40,
                "{path} is shared and says too little about why"
            );
        }
    }

    /// The write of Server Setup answers on the Administration Port
    /// alone. The product port faces the internet, and on a Server that
    /// nobody can sign in to yet, a write there gives the first
    /// Administrator to the first person who posts. The product port
    /// keeps the read, because the sign-in page names the
    /// Administration Port from it.
    #[test]
    fn the_server_setup_write_answers_on_the_administration_port_alone() {
        let methods = |table: &[Route]| {
            table
                .iter()
                .find(|route| route.path == "/api/v1/setup")
                .map(|route| route.methods.to_vec())
        };
        assert_eq!(methods(ROUTES), Some(vec!["get"]));
        assert_eq!(methods(ADMINISTRATION_ROUTES), Some(vec!["get", "post"]));
        assert_eq!(
            SHARED_ROUTES
                .iter()
                .find(|(path, _, _)| *path == "/api/v1/setup")
                .map(|(_, methods, _)| methods.to_vec()),
            Some(vec!["get"])
        );
    }

    /// The product port holds no installation part of any provider. The
    /// carrier account, its SIP sign-in, the mail domain, the
    /// Installation OAuth Client and the model keys are set up through
    /// the provider routes of the administration port alone. The two
    /// first-run writes of onboarding are the one documented exception:
    /// they answer `409` once onboarding is complete.
    #[test]
    fn the_product_port_holds_no_installation_provider_setup() {
        const INSTALLATION_SETUP: &[&str] =
            &["/api/v1/administration/providers", "/{provider}/key"];
        const ONBOARDING: &str = "/api/v1/settings/onboarding/";
        for route in ROUTES {
            for fragment in INSTALLATION_SETUP {
                assert!(
                    !route.path.contains(fragment) || route.path.starts_with(ONBOARDING),
                    "{} sets up an installation part of a provider on the product port",
                    route.path
                );
            }
        }
        // Each part a provider declares answers on the administration
        // port through the one generic route set.
        for path in [
            "/api/v1/administration/providers",
            "/api/v1/administration/providers/{provider}/{part}",
            "/api/v1/administration/providers/{provider}/{part}/test",
        ] {
            assert!(
                ADMINISTRATION_ROUTES
                    .iter()
                    .any(|route| route.path == path && route.authenticated),
                "{path} is not a guarded route of the administration port"
            );
        }
    }

    /// Every guarded route of the administration port is a route of the
    /// installation and not of one Workspace's content. The port
    /// serves the installation: a route that reads a person's messages,
    /// memory or files belongs to the product port.
    #[test]
    fn the_administration_port_serves_the_installation_alone() {
        const WORKSPACE_CONTENT: &[&str] = &[
            "/api/v1/channels",
            "/api/v1/agents",
            "/api/v1/runs",
            "/api/v1/memory",
            "/api/v1/artifacts",
            "/api/v1/calls",
            "/api/v1/requests",
            "/api/v1/schedules",
            "/api/v1/ws",
        ];
        for route in ADMINISTRATION_ROUTES {
            for prefix in WORKSPACE_CONTENT {
                assert!(
                    !route.path.starts_with(prefix),
                    "{} is workspace content and belongs to the product port",
                    route.path
                );
            }
        }
    }
}
