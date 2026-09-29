# 0024: A server serves a network behind a proxy, with a separate Administration Port

Status: accepted.

## Context

A team runs Pagis on a VM and reaches it from other machines, and a person can
open a local installation to the other People of a household or team. The
daemon then faces a network: it must say where it listens and where a browser
goes, believe forwarded headers from one place only, keep installation
settings off the public port, and keep its records in a store the deployment
can operate and back up.

## Decision

### Bind Address, Public Origin and Trusted Proxy

An installation that serves a network names the **Bind Address** (`bind`), the
**Public Origin** a browser reaches (`public_origin`) and the **Trusted Proxy**
in front of it (`trusted_proxy`) in its config file; `PAGIS_BIND`,
`PAGIS_PUBLIC_ORIGIN` and `PAGIS_TRUSTED_PROXY` override them. A local
installation that serves its own machine names none: it binds loopback, and
its Public Origin derives from the Bind Address and the port.

The daemon terminates no TLS. A team on a VM already runs a reverse proxy, and
a certificate lifecycle is a product of its own. The proxy holds the
certificate, forwards WebSocket upgrades, and reports the browser's address in
`X-Forwarded-For` and scheme in `X-Forwarded-Proto`. The Proxy page of the
documentation site (https://docs.pagis.co/server/proxy) holds matching Caddy
and nginx configuration. The daemon believes both headers only from the
Trusted Proxy's address and reads the last `X-Forwarded-For` entry, the one the
request could not write. Without a Trusted Proxy it reads neither.

The Session cookie carries `Secure` where the browser spoke TLS and not where
it did not, because a browser refuses a `Secure` cookie from a plain-HTTP
page. The sign-in rate limit counts the browser's own address, so one person
cannot lock out everyone behind the proxy.

Nothing that comes through a proxy reaches the Client Credential: its routes
answer only a request from a program on this machine (ADR-0025).

The CORS answer names the Public Origin and refuses every other origin. CORS
stops only a read, so each listener also refuses a browser request from an
origin it does not serve, on socket upgrades and unsafe methods: it allows
`Sec-Fetch-Site` of `same-origin` or `none`, else requires an exact `Origin`
match, and a request with neither header goes on to the Session check. The
product port serves the Public Origin, and on a local installation also its
loopback origin, where the Client App and the Sign-In Link open the Product
App. The Administration Port serves its own origin.

### The Multi-User Mode follows from the Public Origin

A Public Origin whose host is loopback serves the People at the machine; any
other serves People on other machines. The Bind Address does not decide this,
because the compose deployment binds loopback with a proxy on the same host.

A local installation that serves several People runs on SQLite, and they reach
it through the owner's proxy or tunnel. https://docs.pagis.co/client-app/several-people holds
the Caddy, Tailscale Serve and Cloudflare Tunnel setups.

The Product App works at an `http://` Public Origin, and `https://` is the
recommended one. The Product App needs no secure-context API: it makes ids
with `crypto.getRandomValues` and copies through the copy command when the
Clipboard API is absent. The microphone has no substitute, so dictation says it
needs `https://`. Over `http://` the password crosses the network in clear and
the cookie has no `Secure`, so every documented setup gives an `https://`
address.

A Client App that connects to a server requires `https://` except for a
loopback host (`127.0.0.1`, `[::1]`, `localhost`), because it runs the commands
the server dispatches (ADR-0015), and on clear text an attacker on the path can
pose as the server. It refuses such an address before any request, and a
stored `http://` origin of another machine fails at start. The W3C Secure
Contexts specification trusts a loopback `http://` origin alone, and Docker
Engine deprecates its non-loopback TCP socket without TLS for the same reason.
The Client App follows no redirect before its product window opens, uses the
platform's TLS certificate check with no pinning, and on an `https://` origin
holds the Session cookie as `Secure`, also where a proxy that does not report
TLS makes the server leave it out.

An Administrator of a local installation switches the mode in the
Administration Interface. No flag is stored: the switch writes the settings the
mode follows from. `PUT /api/v1/settings/system/multi-user` takes the Public
Origin and an optional Trusted Proxy (an IP address; the form offers
`127.0.0.1`). It refuses an origin that is not an absolute `https://` or
`http://` URL of a scheme, a host and an optional port alone, and a loopback or
unspecified host. `DELETE /api/v1/settings/system/multi-user` clears both.
Both write `config.toml` through the System Settings seam, bind loopback, and
answer whether a restart is required (whenever the written mode differs from
the running one); the form then asks for the reserved restart. Binding loopback
keeps the plain-HTTP port off the network, so nobody reaches the daemon around
the proxy's TLS; a same-machine proxy (Caddy, `tailscale serve`, `cloudflared`)
reaches the daemon at `127.0.0.1`. Turning the mode off must bind loopback
too, because a network bind would derive a non-loopback Public Origin.

With the mode off, the daemon refuses on both ports every request that is not
from a program on its own machine, by the rule of the Client Credential trade
(ADR-0025), so a proxy the owner forgot to stop reaches nothing. People from
other machines keep their accounts. A server always serves a network: its
deployment names `PAGIS_PUBLIC_ORIGIN`, its Settings view shows the mode with
no switch, and both routes answer `409`.

The mode does not decide the Client Credential; the kind of installation does
(ADR-0025). A local installation in the Multi-User Mode keeps its credential,
and the owner's Client App reaches it at its loopback origin and stays signed
in, while other People sign in with an address and a password through the
proxy.

### Server setup

A server has an Org and a seeded Workspace from its first boot, and nobody who
can sign in. Two paths write the same rows, once:

- **The deployment.** `PAGIS_ADMIN_EMAIL`, `PAGIS_ADMIN_PASSWORD` and the
  provider key variables, read at the first start that finds nobody who can
  sign in.
- **The first-run route.** While the installation holds no Client Credential
  and no Administrator has a password, `POST /api/v1/setup` on the
  Administration Port takes the first address, password and provider keys. The
  store sets the first password only while no Administrator has one, so of two
  concurrent requests one wins and the other answers `410 Gone` and keeps no
  key. There is no setup token: as with Jenkins and GitLab, the protection is
  that only an operator reaches the loopback Administration Port, over SSH.

`/api/v1/setup` answers `410 Gone` from the first password on. A local
installation never runs it, also in the Multi-User Mode: its seeded
Administrator signs in with the Client Credential and sets a browser address
and password in the Administration Interface.

### The Administration Port

The daemon binds two listeners in one process over one set of records: the
product port and the **Administration Port** (`[administration] port` and
`[administration] bind`), which binds loopback by default whatever the product
port binds. A second port, unlike a tab, can stay on loopback or a private
interface while the product port faces the team; an Administrator reaches it
over an SSH tunnel. Prometheus and Grafana separate a management listener for
the same reason.

The session middleware and an administrator guard are router layers, so any
route added there answers `401` without a Session and `403` to a Member. Two
routes answer outside the guard, each with its reason beside it and a test that
holds the set to exactly those two: the first-run setup, and the password
sign-in. No CORS answer belongs on the port: the Administration Interface is
served from it, so every call is same-origin.

Administration answers on the Administration Port alone: people, usage,
sessions, hosts, resources, health, System Settings, provider setup, restart,
and the install, update, binding, start and uninstall of a Plugin. The product
router has no installation-wide route, and a test holds that.
`crate::routes::SHARED_ROUTES` names the routes both ports answer, each with
its reason: `/api/v1/user`, `/api/v1/sessions`, `/api/v1/sessions/current`,
`GET /api/v1/setup`, `GET /api/v1/plugins` and
`GET /api/v1/plugins/{plugin_id}`. The Product App shows an Administrator one
link to the Administration Interface, with the SSH tunnel command where the
port binds loopback.

Local onboarding has two product-port routes,
`PUT /api/v1/settings/onboarding/providers/{provider}/key` and
`PUT /api/v1/settings/onboarding/docker-endpoint`, which take an Administrator
and answer `409` once the Workspace finished onboarding. A local installation
also serves the Administration Port on loopback, and the Client App opens it
from its menu.

### Provider setup is the installation's, and provider use is the person's

The installation's provider setup is on the Administration Port: model keys,
the Installation OAuth Client, the carrier and its SIP sign-in, and the mail
domain. Each provider declares its parts in `installation_setups()` in
`crates/pagis-connect/src/catalog.rs` (ADR-0012), and one route set serves
every part:

- `GET /api/v1/administration/providers` lists providers, parts and their
  state, and reads back no secret.
- `PUT /api/v1/administration/providers/{provider}/{part}` configures or
  connects a part.
- `POST /api/v1/administration/providers/{provider}/{part}/test` proves a kept
  credential again; only a `connection` part is testable.
- `DELETE /api/v1/administration/providers/{provider}/{part}` removes a part.
  A carrier that carries a number, and a mail domain that holds a mailbox,
  answer `409` and say what to do first.

The route creates each Installation Connection in the Org's Workspace
(ADR-0023) under the entry's default alias, one for each provider. What a
person does with a provider (their Google account, their numbers and
mailboxes, their own key check) is on the product port, which refuses every
installation part with `403` and names the Administration Interface.

### System Settings and restarts

A System Setting is a setting of the installation: the port, the Docker
endpoint, the log level, the data directory, and the Multi-User Mode of a local
installation. An Administrator changes it in the Administration Interface; the
daemon is the one writer of the config file. The timezone is each Person's own
(ADR-0006). A change that needs a restart offers one: the server exits with the
reserved restart code, the Client App starts it again, and a daemon run from
source prints that it must be run again.

The server discovers the container runtime on every installation: it pings
candidate endpoints in order and keeps the first that answers. The settings
show the endpoint in use, each candidate's result, an override that a ping
validates before it is saved, and a probe-again control. A change applies at
the next connect.

A taken port gets a message that names the port and the flag that moves it,
not the process, because the Headless Server image has no tool that finds it.

A restart is the whole server's. Boot recovery fails every unfinished Run of
the installation, because one daemon holds each Run's mid-turn state in
memory. Each failed Run says that a restart ends every Run on the server, and
the deployment document says to restart at a quiet hour. One daemon owns one
state directory and one database.

### The Headless Server image

The Headless Server is the same daemon and Product App as the Client App's
Server Runtime, as a Linux container image. It is not notarized; its trust
root is the registry, the digest the deployment pins, and the provenance
attestation of that digest.
https://docs.pagis.co/server states the deployment.

The image starts the daemon without `--local`, so it is a server and holds no
Client Credential, and it sets `PAGIS_REQUIRE_PUBLIC_ORIGIN`: the daemon then
refuses to start with `--local` or with an empty or loopback Public Origin, and
the error names what to change. Every program the daemon starts is in the
image: `gog`, `git`, and the PostgreSQL client that `pagis backup` and
`pagis restore` run. A VM needs Docker and nothing else.

### A server runs on Postgres, and a local installation on SQLite

Two Storage Backends sit behind the store traits of `pagis-core`, and
`--local` decides which one runs:

- A server (no `--local`) runs on Postgres. `[database] url` names it and
  `PAGIS_DATABASE_URL` overrides it; with neither, the server stops at boot and
  the error names both.
- A local installation runs on SQLite at `pagis.db` in the state directory,
  for one person or several. With `--local` a set database URL stops the boot,
  and the error says to remove it.

SQLite is one file with no service, so the Client App installs one binary.
SQLite has one writer, so on a server every write would queue on one lock, and
a team expects point-in-time backup, replicas, a connection limit and a
slow-query log. The rule follows `--local`, not the container, and the tests
follow it: a test daemon that boots as a server runs on Postgres.

`pagis-core::Stores` holds every repository behind its trait. A storage crate
builds one from its own pool, and no source outside a storage crate names a
pool type. Postgres holds the records, not the files: memory repositories,
Artifacts, recordings, the sealed secret file, Plugins, Software and Computer
volumes stay on the server's disk. Two daemons do not share a database: one
VM, one daemon, one state directory.

The store-trait tests live in `pagis-testkit` and run on both backends from
one set of bodies, as does the two-Workspace harness. The Postgres suite shares
one `postgres:18-alpine` container, one schema for each test, started through
Docker, and skips with a message when Docker is not reachable. Each backend
holds one migration baseline, `0001_baseline.sql`, in the same order with the
same comments. Full-text relevance is defined by properties on both
(ADR-0008).

### Backup and restore

A Backup is a consistent copy of one installation: the state directory, the
records and the Computer volumes. `pagis backup` takes the daemon's instance
lock, so a running daemon fails the backup with a reason.
`deploy/backup.sh` captures the Computer volumes beside the archive, because
they belong to the Docker host.

The Installation Key never sits in the archive it seals. The Client Credential
stays with the machine that made it, and the next boot of a restored directory
writes a new one. `computer-tokens/` stays out, because Computers keep running
while the daemon is stopped; the daemon writes a new token at each Computer
start and does not adopt a running Computer with no token on disk, which it
starts again at the next wake.

`pagis restore` restores only onto a new state directory, an empty database,
and a server of the Backup's release or newer, because the release marker is
one-way (ADR-0025).

## Consequences

- A network deployment is a bind address, a reverse proxy and three settings.
- A deployment that exposes the Administration Port puts a proxy in front of
  it; the supported way in is a tunnel to loopback.
- Every store trait has two implementations kept in step by one suite, with
  the same module layout and file names in both storage crates, so a reviewer
  diffs a file against its sibling and sees only the SQL dialect.
- A search returns the same pages on both backends, possibly in another order.
- Moving an installation between backends is not a feature.
- The `sqlx` Postgres driver carries rustls, so a managed Postgres that
  requires TLS works.
- One daemon serves everyone, so a restart interrupts every person.
