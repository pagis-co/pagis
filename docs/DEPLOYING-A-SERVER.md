# Deploying a Pagis server

One team, one server. This document is the whole deployment: the shape, the
settings, the proxy, the first administrator, backup and restore, and
upgrades. The deployment is the Headless Server image with the compose files
in `deploy/`. ADR-0024 holds the decisions,
and `docs/RELEASING-SERVER.md` holds the four artifacts of a release and the
version rule between them.

## The shape

**One VM. One daemon. One state directory. Postgres for the records. No
second daemon.**

- **One VM.** The memory repositories, the Artifacts, the recordings, the
  Plugins, the Software, the sealed secrets and the Computer volumes are
  files on this machine's disk. A server does not scale horizontally, and a
  second machine is a second installation.
- **One daemon.** One process owns every Run of every Person and holds each
  Run's mid-turn state in its own memory. A restart therefore ends every Run
  on the server (see "A restart is the whole server's"). Two daemons against
  one database is not a supported shape.
- **One state directory**: `/var/lib/pagis`, at the same path on the VM and
  in the daemon's container.
- **Postgres for the records**, so the People of the team do not queue
  behind one writer. Postgres holds the records and nothing else: every file
  above stays on the disk.
- **The People on it trust each other.** A Computer is a container on a
  shared kernel. That is a reasonable boundary between colleagues, and it is
  weaker than it looks between strangers.
- **The Plugins of a Workspace are not isolated from each other.** Every
  stdio server of a Workspace runs as one uid in its Plugin Computer, so each
  one can read the secrets and the data of the others. The Administrator
  installs together only Plugins that they trust together
  (`docs/PLUGINS.md`, "Reach").
- **An HTTP Plugin sends requests from the daemon's network.** The daemon
  has the VM's network, so its loopback is the VM's loopback. An HTTP or SSE
  Plugin server can make the daemon send requests to the services that listen
  there, for example Postgres, the product port, the Administration Port and
  the control port of each Computer. It can also make the daemon send requests
  to every HTTPS host that the VM reaches. The egress policy of the Computers
  does not apply to these requests ("What a Computer reaches").
  `deploy/Caddyfile` turns off the admin API of Caddy, so the proxy listens on
  no port of that loopback.

## What makes an installation a server

**The process that starts the daemon decides it.** The Client App starts a
local installation with `--local`. The Headless Server image starts `pagis`
without it, and that daemon is a server. The
configuration cannot decide it: a local installation that other People reach
through the owner's proxy has the same Bind Address, Public Origin and
Trusted Proxy as the compose deployment.

A server holds no Client Credential. The daemon writes no credential file
and removes one that a local run left, refuses the trade at
`/api/v1/sessions/client`, answers nothing at `/api/v1/runtime/identity` and
mints no one-time sign-in link. The start banner in the logs therefore
carries no way in, and everybody signs in with an address and a password.

A local installation holds one, whatever its Public Origin. The daemon
accepts the trade, the sign-in link and the runtime identity handshake only
from a program on the same machine: the socket peer is loopback, the request
carries no header that a proxy writes (`Forwarded`, `Via`,
`X-Forwarded-For`, `X-Forwarded-Host`, `X-Forwarded-Proto`, `X-Real-IP`),
and the `Host` header names a loopback host. Anything else gets `403`. A
request through the Trusted Proxy is refused even when the proxy runs on the
same machine, because that proxy writes `X-Forwarded-For` or passes on the
public name (ADR-0025).

**The same flag decides the Storage Backend.** A server keeps its records in
Postgres, and it stops at boot when no database URL is set; the error names
`PAGIS_DATABASE_URL` and `[database] url`. A local installation keeps its
records in SQLite at `pagis.db` in the state directory, whether one person
uses it or several, and it stops at boot when a database URL is set. The
rule is the same for the Headless Server image and for a server without
containers (ADR-0024).

The Public Origin decides who can reach the installation, not the Bind
Address: its host is loopback for the People at the machine, and anything
else for People on other machines (ADR-0024).

The Headless Server image sets `PAGIS_REQUIRE_PUBLIC_ORIGIN`. The daemon
then refuses to start with `--local`, and while the Public Origin is empty
or has a loopback host, and the error names what to change. A bare
`docker run` of the image stops at once.

## The settings

A server names its settings in `config.toml` in the state directory, or in
`PAGIS_*` variables for a deployment that mounts no file. A variable wins
for the run and is never written back into the file.

| `config.toml` | Variable | Default | What it is |
| --- | --- | --- | --- |
| `port` | `PAGIS_PORT` | `4400` | The product port. |
| `bind` | `PAGIS_BIND` | `127.0.0.1` | The **Bind Address** of the product port. Name the private interface the proxy is on, or `0.0.0.0` for every interface. |
| `public_origin` | `PAGIS_PUBLIC_ORIGIN` | Derived from `bind` and `port` | The **Public Origin**: the scheme, host and port a browser reaches the installation at. The CORS answer names it. |
| `trusted_proxy` | `PAGIS_TRUSTED_PROXY` | None | The **Trusted Proxy**: the one address whose `X-Forwarded-For` and `X-Forwarded-Proto` the daemon believes. |
| `[administration] port` | `PAGIS_ADMINISTRATION_PORT` | `4401` | The Administration Port. |
| `[administration] bind` | `PAGIS_ADMINISTRATION_BIND` | `127.0.0.1` | Its Bind Address, loopback whatever the product port binds. |
| `[database] url` | `PAGIS_DATABASE_URL` | None | The `postgres://` URL of the database that holds the records. A server does not start without it. |
| `[secrets] key_file` | None | `/run/secrets/pagis-secrets-key` | The **Key File** (Linux). |
| `[screen] advertise_ip` | `PAGIS_SCREEN_ADVERTISE_IP` | `127.0.0.1` | The address browsers reach the Media Relay at. |
| `[screen] media_port_first`, `media_port_last` | `PAGIS_SCREEN_MEDIA_PORT_FIRST`, `PAGIS_SCREEN_MEDIA_PORT_LAST` | `50000`, `50099` | The Media Relay's UDP range. |

```toml
bind = "10.0.1.7"
public_origin = "https://pagis.example.net"
trusted_proxy = "10.0.1.6"

[administration]
port = 4401
bind = "127.0.0.1"

[database]
url = "postgres://pagis:<password>@127.0.0.1:5432/pagis"
```

Bind the private interface the proxy is on, not `0.0.0.0`, where the
network allows it. The daemon then answers the proxy alone, and a firewall
rule is a second line of defence rather than the only one.

An Administrator changes the port, the Docker endpoint and the log level in
the Settings view of the Administration Interface, and the daemon writes
`config.toml` itself. The timezone is each Person's own: their browser or
Client App reports it at their first sign-in, and they change it in
**Settings → Timezone** of the Product App. The clock of the server is
never a Person's timezone.

A change to the port, the log level or the multi-user mode takes effect on
a restart. The daemon then exits with code 75, and a supervisor starts it
again: the Client App, or the `restart` policy of the compose deployment.
Each of them sets `PAGIS_SUPERVISED=1`, and the Settings view then waits for
the new process. A daemon that a person started by hand has no supervisor, so
the Settings view says to run `pagis` again. A supervisor of your own, such as
a systemd unit with `Restart=on-failure`, sets `PAGIS_SUPERVISED=1` too.
`--port` and `PAGIS_PORT` override the port of `config.toml` for one run, and
the Settings view shows both ports while they differ.

The log level applies to the daemon's own lines. The libraries it uses log
warnings and errors alone. `PAGIS_LOG` replaces the whole filter with a
`tracing` directive such as `debug` or `info,tantivy=debug`.

## The Key File

On Linux the daemon reads the **Installation Key**, which seals
`secrets.enc`, from the Key File. The file holds 64 hexadecimal characters
and nothing else. The daemon refuses a file that a group or another user
can read, and it does not start without the file.

```bash
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > pagis-secrets-key
chmod 600 pagis-secrets-key
```

The key never sits in `config.toml` and never goes into a backup: an
archive that holds both is a lock beside its key. Keep it where the
deployment keeps its other secrets. **A restored server without it opens no
stored secret.**

## Compose

`deploy/` holds six files, and only `.env` is edited:

| File | What it is |
| --- | --- |
| `compose.yaml` | The four services. Nothing in it is edited for a deployment. |
| `.env.example` | Every setting, stated one time. Copy it to `.env` and fill it in. |
| `Caddyfile` | The proxy, which holds the certificate. |
| `egress.sh` | The egress rules of the Computers, which the `egress` service installs. |
| `backup.sh` | The backup below. |
| `restore.sh` | The restore below. |

The four services:

- **`db`**, `postgres:17-alpine`, pinned by digest, on the `pagis-database`
  volume. It listens on loopback, so nothing off the VM reaches it.
- **`egress`**, `rancher/klipper-lb`, pinned by digest, which is Alpine with
  `iptables`. It runs `egress.sh`, which writes the egress rules of the
  Computers into the VM's firewall (see "What a Computer reaches"). The
  `pagis` service starts only after the rules are in place. The service then
  stays up: Docker starts it again when the VM boots, and each start writes
  the rules again. It has `NET_ADMIN` and no Docker socket.
- **`pagis`**, the release's Headless Server image, on the VM's
  `/var/lib/pagis`. It is the one service that receives the Docker socket,
  because it is the one that starts Computers.
- **`proxy`**, `caddy:2-alpine`, pinned by digest, which holds the
  certificate and speaks TLS. It is the one service that binds a public
  interface.

`compose.yaml` names `postgres`, `klipper-lb` and `caddy` by tag and digest
(`postgres:17-alpine@sha256:...`), so a server runs the bytes that its
release was tested with, and a moved tag does not reach it. The `pagis`
image takes the release in `PAGIS_VERSION`.

### The `.env` names

`compose.yaml` maps the `.env` names to the daemon's variables and to the
`egress` service:

| `.env` | Where it goes |
| --- | --- |
| `PAGIS_DOMAIN` | `PAGIS_PUBLIC_ORIGIN`, as `https://<PAGIS_DOMAIN>` |
| `PAGIS_PUBLIC_IP` | `PAGIS_SCREEN_ADVERTISE_IP` |
| `PAGIS_MEDIA_PORT_FIRST` | `PAGIS_SCREEN_MEDIA_PORT_FIRST`, and the range that the egress rules keep open |
| `PAGIS_MEDIA_PORT_LAST` | `PAGIS_SCREEN_MEDIA_PORT_LAST`, and the range that the egress rules keep open |
| `PAGIS_COMPUTER_ALLOW` | The `egress` service: the private destinations that a Computer reaches. Empty by default. |
| `PAGIS_DATABASE_PASSWORD` | `PAGIS_DATABASE_URL`, and the `db` service's password |
| `PAGIS_VERSION` | The image tag |

`compose.yaml` sets the rest: `PAGIS_BIND=127.0.0.1`, `PAGIS_PORT=4400`,
`PAGIS_TRUSTED_PROXY=127.0.0.1`, and the Administration Port on
`127.0.0.1:4401`.

### Every service shares the VM's network namespace

Each service has `network_mode: host`, for one reason: **the daemon starts
each Agent's Computer as a container of its own and reaches it on the
Docker host's loopback.** A daemon with a network namespace of its own
reaches none of them, and every Computer stops at "did not become healthy
in time". Host networking also gives the Media Relay its UDP range
directly, with no forwarder for each port. It is a Linux facility, so this
image is for a Linux VM and not for Docker on a desktop. The `egress`
service has host networking for a second reason: it writes the firewall of
the VM, and the firewall is part of the VM's network namespace.

Privacy is then a Bind Address rather than a published port:

| Port | Binds | Who reaches it |
| --- | --- | --- |
| 443, 80 | Every interface | The team's browsers and Client Apps. The certificate lives here. |
| 4400, the product port | `127.0.0.1` | The proxy, on the same loopback, and nobody else. |
| 5432, Postgres | `127.0.0.1` | The daemon, and nobody else. |
| 4401, the Administration Port | `127.0.0.1` | The Administrator, over an SSH tunnel. |
| The Media Relay's UDP range | Every interface | The team's browsers, because media does not go through the proxy, and the Computers. Open exactly that range in the VM's firewall, and nothing else. |
| None, the `egress` service | Nothing | Nobody. The service writes the egress rules, which close every port of the VM to the Computers except the Media Relay's range. |

### The state directory has one path

The state directory is the VM's `/var/lib/pagis`, mounted at
`/var/lib/pagis` in the daemon's container, and not a named volume. **The
daemon gives each Computer files from its state directory as bind mounts**:
the screend token, and the checkout of each Plugin. Docker reads the source
of a bind mount on the Docker host, not in the container that asks for it.
The path is therefore the same in both places, or every Computer gets an
empty directory in place of its token and stops at "did not get its access
token".

Only the owner of the state directory can read it. The daemon sets the
directory to mode 700 at start, and the files that it writes give no
permission to a group or to other users. The Plugin checkouts are the one
exception, because a Computer reads them as a different user.

### The volumes

| Volume | What is in it | In a backup |
| --- | --- | --- |
| `/var/lib/pagis`, a directory on the VM | The state directory: `config.toml`, `secrets.enc`, `runtime-release`, `memory/<workspace_id>/`, `artifacts/`, `recordings/`, `screens/`, `gog/`, `plugins/`, `software/`, `computer-tokens/`, `logs/` | Yes, without the logs and `computer-tokens/` |
| `pagis-database` | The Postgres cluster | As a dump, not as files |
| `pagis-volume-<workspace_id>-<agent_id>` | One Agent's Computer home, made by the daemon | Yes, one tarball each |
| `caddy-data`, `caddy-config` | The certificate and Caddy's state | No. Caddy gets a certificate again on a new host. |

Pagis encrypts only the secrets in these volumes, for example `secrets.enc`.
The rest of their content has no encryption. Put `/var/lib/pagis` and
`/var/lib/docker`, which holds the Postgres cluster and the Computer
volumes, on an encrypted disk
([What Pagis encrypts](../docs/DATA-AND-PRIVACY.md#what-pagis-encrypts)).

### Bringing it up

```bash
cd deploy
mkdir -p secrets
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > secrets/pagis-secrets-key
chmod 600 secrets/pagis-secrets-key
cp .env.example .env         # fill in the domain, the address and the passwords
docker compose up -d
docker compose logs -f pagis
```

The compose file mounts the key at `/run/secrets/pagis-secrets-key`.
Compose outside Swarm mounts the file as it is on the host, so the daemon
sees the owner and the mode of `secrets/pagis-secrets-key`. The daemon
refuses a key that a group or another user can read: keep the file at
mode `600`.

## The proxy

The daemon terminates no TLS. A team that runs a server already runs a
proxy, and a certificate lifecycle inside the daemon is a product of its
own. The proxy holds the certificate, speaks TLS to the browser, and speaks
plain HTTP to the daemon on the private interface.

The proxy does three things:

1. **Forward the WebSocket upgrade** of `/api/v1/ws`,
   `/api/v1/channels/{id}/dictate` and `/api/v1/calls/{id}/listen`.
   Without it the Product App loads and then shows nothing live.
2. **Set `X-Forwarded-Proto`** to the scheme the browser used. The Session
   cookie carries `Secure` only where the daemon is told the browser spoke
   TLS.
3. **Set `X-Forwarded-For`** to the browser's address. The sign-in rate
   limit counts against it, so without it one person who forgets a password
   locks out everybody behind the proxy.

A browser can write both headers, so the daemon reads them from the Trusted
Proxy and from no other address. It takes the last entry of
`X-Forwarded-For`, which is the entry the proxy wrote, so a proxy that
appends and a proxy that replaces read the same. In the compose deployment
the Trusted Proxy is `127.0.0.1`, where the proxy is and where no browser
can be.

### Caddy

```caddyfile
pagis.example.net {
	reverse_proxy 10.0.1.7:4400
}
```

Caddy gets and renews the certificate, sets `X-Forwarded-Proto`, appends to
`X-Forwarded-For`, and passes a WebSocket upgrade through with no
configuration. `deploy/Caddyfile` is the same block for the compose
deployment, with the admin API of Caddy off (see "The shape").

### nginx

```nginx
server {
    listen 443 ssl;
    server_name pagis.example.net;

    ssl_certificate     /etc/letsencrypt/live/pagis.example.net/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/pagis.example.net/privkey.pem;

    location / {
        proxy_pass http://10.0.1.7:4400;
        proxy_set_header Host              $host;
        proxy_set_header X-Forwarded-For   $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # The WebSocket upgrade of /api/v1/ws and of the two media sockets.
        proxy_http_version 1.1;
        proxy_set_header Upgrade    $http_upgrade;
        proxy_set_header Connection $connection_upgrade;

        # A run streams for minutes and a socket stays open for hours.
        proxy_read_timeout  1h;
        proxy_send_timeout  1h;
        proxy_buffering     off;
    }

    client_max_body_size 50m;  # the artifact upload cap
}

# Connection: upgrade only where the request asked for one.
map $http_upgrade $connection_upgrade {
    default upgrade;
    ''      close;
}
```

## The Administration Port

The daemon serves the Administration Interface on a second listener of the
same process: the spend of each Person, the People, who is signed in, the
Hosts, the containers and disk of each Person, the provider setup, the
System Settings, the Plugins, the restart and the health of the daemon.
These answer on the Administration Port alone. The product port serves each
Person's own settings, and an Administrator reads one link to the
Administration Interface in the Product App's Settings.

Every route on the port needs a signed-in Administrator, and a Member gets
nothing. Two routes answer without one: the first-run setup and the password
sign-in.

**Keep it private.** The port binds loopback by default, so nothing off the
machine reaches it. Reach it over an SSH tunnel:

```bash
ssh -L 4401:127.0.0.1:4401 pagis.example.net
```

Then open `http://127.0.0.1:4401/` and sign in with an address and a
password.

To reach it from an Administrator's own network, name that interface in
`[administration] bind` and firewall the port to that network. Do not put
the Administration Port behind the public name of the product port: the
reason for the second listener is that the internet does not reach it. The
daemon terminates no TLS on this port either, so a deployment that exposes
it puts a proxy in front of it.

A daemon whose Administration Port is taken names the port and the setting
that moves it. `pagis --port` moves the product port alone.

## The first administrator

A server boots with an Org and a seeded Workspace and nobody who can sign
in. Until the first Administrator exists, the start banner and the sign-in
page of the product port say so and name the Administration Port. Two paths
close that gap, and both write the same rows:

- **From the environment.** Set `PAGIS_ADMIN_EMAIL`, `PAGIS_ADMIN_PASSWORD`
  (at least 12 characters) and the provider key variables
  (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`) before the
  first start. The daemon reads them at the first start that finds nobody
  who can sign in, and keeps the keys in its sealed secret file. The
  variables can stay in the configuration: a later start never overwrites a
  password that somebody changed.
- **From a browser.** Leave them empty, open the tunnel to 4401 and finish
  setup there. Server Setup makes the first Administrator on the
  Administration Port only: `POST /api/v1/setup` takes the address, the
  password and the keys and signs the new Administrator in, and it answers
  on no other port. A person who reaches the public name cannot make the
  first Administrator. `GET /api/v1/setup` names the model providers the
  installation takes a key for. The product port answers it too, because its
  sign-in page names the Administration Port from it. The routes answer only
  while the installation holds no Client Credential and no Administrator
  holds a password. They answer `410 Gone` from the first password onwards.
  The first password is one claim: of two requests at once, one makes the
  Administrator, and the other answers `410 Gone` and keeps no provider key.

Either way no Person and no client takes a model key: the Administrator
configures the installation one time, for everybody. The providers of these
keys therefore get the model requests of every Person: the messages, the
memory and the tool results, with the screenshots of the Computers
([What the model provider receives](../docs/DATA-AND-PRIVACY.md#what-the-model-provider-receives)).
There is no sign-up.
The Administrator creates each Person in the Administration Interface, and
each new Person gets a seeded Workspace: one Agent, its direct Channel, the
Model Aliases and the Report Schedule. The new Workspace takes the timezone
of the Administrator's own Workspace until the Person's first sign-in, which
takes the timezone of their browser or Client App.

A Schedule that comes due while no provider holds a key starts no Run. It
waits, and it runs when an Administrator stores a key.

Nobody on a server answers a model question. The `default` Model Alias of a
new Person names the models of the Administrator's own route whose provider
holds a key. A key that the Administrator stores or removes in **Providers**
gives every `default` alias that names no provider with a key the newest
listed model of the first provider that holds one, in the order Anthropic,
OpenAI, OpenRouter. A route that still reaches a provider stays as it is.

The Administrator also sets up the providers in the Administration
Interface: the Google OAuth client, the carrier account and its SIP
sign-in, and the mail domain. A Person then connects their own Google
account, buys an Agent Phone Number or makes an Agent Mailbox in the
Product App.

## The way in

A Person signs in at `https://<the domain>/` with an address and a password.
The answer is an HTTP-only, host-only, `SameSite=Strict` Session cookie, and
it carries `Secure` where the proxy reported TLS. No token appears in a URL.
The CORS answer names the Public Origin and no other origin, so a page
elsewhere cannot read the API with a Person's cookie. Each port also refuses a
socket upgrade or a write from a browser page at another origin, so the Public
Origin must be the exact address that people open.

### A Client App connected to this server

A Person can use the Client App instead of a browser. At setup they
choose to connect to a Pagis server and enter `https://<the domain>`. The
client then opens the server's own sign-in page, where they sign in with
their address and password. The client installs nothing and keeps no key.
It refuses a server whose release is outside its Compatibility Range (its
own version and every later version that promises the same API) and says
which end to update. When the server has no Administrator yet, the client
says so and names the Administration Interface.

**Connecting gives this server's Administrator a shell on the Person's
machine.** The client registers as a Host, and it runs every command the
server dispatches, as the OS user that started the client. The approval
lives on the server (ADR-0015). Tell each Person this before they connect.

### What does not work on a server

- **Widgets.** A Widget needs two loopback origins, `localhost` and
  `127.0.0.1`, to isolate its frame from the Product App. A Product App on
  a public name has no second origin, so it shows the projection of a
  Widget and never the Widget (`docs/WIDGETS.md`).
- **A `byo` Google Connection.** The loopback flow of bring your own needs a
  Person at the daemon's machine. The daemon refuses it for a request that
  comes through the proxy or from another machine, and the refusal says that
  an Administrator sets up the Google OAuth client.

## The screen view

Media does not travel through the proxy. A browser sends it to the Media
Relay over UDP, at `[screen] advertise_ip` and one port of the range.
`docs/SCREEN-RELAY.md` holds that deployment, and also the variant that
puts an external TURN server in front of the relay where the daemon cannot
own a public UDP range.

## A Computer on this VM

An Agent's Computer is a container the daemon starts on this machine's
Docker host, from the Computer image of the release. The daemon pulls the
image the first time an Agent wakes, so the VM needs a route to the
registry. It compares the image's version label with its own pin and boots
nothing on a mismatch.

Each Computer gets a container, a volume and the Tenant Network of its
Workspace, and the daemon reaches it on the VM's loopback. A Computer that
stays in `starting` until it fails is the first thing to check against the
network section above.

### What a Computer reaches

An Agent reads web pages, mail and documents that other people write, and
text in them can steer it. So a Computer reaches the public internet and
nothing private. The `egress` service writes these rules into the VM's
firewall before the daemon starts:

- **The public internet**: yes. An Agent browses and downloads.
- **The Media Relay**: yes, its UDP range on the VM, which the pipeline
  of each screen registers with.
- **Name resolution**: yes, UDP and TCP port 53 to the resolvers that Docker
  gives to containers, also when a resolver has a private or link-local
  address.
- **Its own Workspace's containers**: yes. The containers of another
  Workspace: no, because Docker keeps two Tenant Networks apart.
- **Any other address of the VM**: no. This includes SSH and every port
  that a service of the VM binds on a network interface. When the public
  address of the VM is on one of its own interfaces, it also includes the
  proxy, so a Computer does not open this installation's public name.
- **Link-local addresses** (`169.254.0.0/16`), which include the metadata
  service of the cloud and with it the VM's cloud credentials: no.
- **Private addresses** (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`,
  `100.64.0.0/10`), which are the LAN or the VPC: no, except the blocks in
  `PAGIS_COMPUTER_ALLOW`.

`PAGIS_COMPUTER_ALLOW` in `.env` names the private destinations that a
Computer reaches, as IPv4 addresses and CIDR blocks separated by commas:

```bash
PAGIS_COMPUTER_ALLOW=10.0.5.0/24,192.168.1.40
```

The list opens a destination on the LAN or in the VPC, on every port. It
opens no address of the VM itself. After a change, run
`docker compose up -d`, which starts the `egress` service again with the
new list.

The rules are in the VM's `DOCKER-USER` chain, which holds the traffic that
the VM forwards, and in its `INPUT` chain, which holds the traffic to the
VM itself. They match the traffic that comes from a user-defined Docker
bridge. On this VM each such container is a Computer or the Plugin
Computer, because every service of `compose.yaml` has host networking. The
rules therefore hold the stdio servers of a Plugin, which run in the Plugin
Computer. They do not hold an HTTP or SSE Plugin server: the daemon sends
its requests from the VM's network ("The shape"). Root
inside a Computer cannot remove them: they are rules of the VM, not of the
container. They take reach away and give none: the traffic that they let
through goes on to Docker's rules and to the rest of the VM's firewall. A
Tenant Network has IPv6 off, so the IPv4 rules hold every address of a
Computer.

`egress.sh` writes its rules with the backend of `iptables` that holds
Docker's `DOCKER-USER` chain, `nf_tables` or legacy. It stops with an error
when the VM has no such chain, and the daemon does not start. Each run
replaces the Pagis chains, `PAGIS-FORWARD` and `PAGIS-INPUT`, and their
jumps, so a second run leaves one copy of each rule. To read what is in
place:

```bash
docker compose logs egress
sudo iptables -S PAGIS-FORWARD
sudo iptables -S PAGIS-INPUT
```

A stop of the `egress` service leaves the rules in place. A boot of the VM
clears them, and Docker then starts the service, which writes them again.

### Give Docker a subnet for each Workspace

The Tenant Network of a Workspace stays after its last Computer stops
(ADR-0014), and each network takes one subnet from the address pools of the
Docker daemon. The default pools hold about thirty bridge networks. When they
are full, Docker refuses a new network with `all predefined address pools
have been fully subnetted`, and the next Workspace that wakes a Computer
stays in `starting` until it fails. A server with more Workspaces sets larger
pools in `/etc/docker/daemon.json` and restarts Docker:

```json
{
  "default-address-pools": [{ "base": "10.200.0.0/16", "size": 24 }]
}
```

This pool holds 256 networks. Select a base that no network of the VM uses.

### Bound a Computer's disk

A Computer writes to two places on the VM's disk, and the daemon asks
Docker to hold each one to a size. Each size is a setting of `config.toml`
in gibibytes, and zero asks for no size:

| Part | What it holds | Setting | Default |
| --- | --- | --- | --- |
| The volume at `/data` | The Agent's home and the browser profile. It stays when the Computer stops. | `[computer] volume_gb` | `10` |
| The writable container layer | Every write outside `/data`, such as `/tmp` and the packages that `pagis-apt` installs. A stop removes it with the container. | `[computer] layer_gb` | `10` |

Docker holds each part to its size only on this storage:

- **The volume**: Docker's `local` volume driver holds a volume to a size
  only where `/var/lib/docker` is on XFS mounted with `pquota`.
- **The writable layer**: the `overlay2` storage driver holds the layer to a
  size only where `/var/lib/docker` is on XFS mounted with `pquota`. Docker
  documents the same option for the `btrfs` and `zfs` storage drivers. The
  containerd image store holds the layer to no size. Docker Engine 29 and
  later uses that store on a new installation.

So one setup gives both bounds: `/var/lib/docker` on its own XFS filesystem
with `pquota`, and the `overlay2` storage driver.

```
# /etc/fstab: the Docker data directory on its own XFS filesystem
/dev/disk/by-label/docker  /var/lib/docker  xfs  defaults,pquota  0 2
```

When `docker info` shows `driver-type: io.containerd.snapshotter.v1` under
`Storage Driver`, the containerd image store is on. Turn it off in
`/etc/docker/daemon.json`, beside the other keys of that file, and restart
Docker:

```json
{
  "features": { "containerd-snapshotter": false }
}
```

`docker info` then shows `Storage Driver: overlay2` and
`Backing Filesystem: xfs`. Docker does not show the images of the other
store, so the daemon pulls the Computer image again at the next wake.

On every other host, the daemon makes the volume or the container without
the size, logs one warning, and the Computer wakes. **Nothing then bounds
that part of the disk, and one Person's Computer can fill the VM's disk for
everybody.** Nothing bounds the volume on ext4, or on XFS without `pquota`.
Nothing bounds the writable layer on ext4, on XFS without `pquota`, or under
the containerd image store on any filesystem. The writable layer grows only
while its Computer is awake: a stop removes the container, for example when
the Computer sits idle, and the space comes back. The disk figure of the
Desk and of the Administration Interface reports what the volumes hold. It
bounds nothing.

The Health view of the Administration Interface reports each answer:

- `volume_quota`: `supported`, `unsupported`, or `unknown` while Docker does
  not answer or `volume_gb` is zero. The daemon asks Docker with a probe
  volume when it has no answer yet.
- `container_quota`: `supported`, `unsupported`, or `unknown` until a
  Computer wakes after the daemon starts, or when `layer_gb` is zero. The
  daemon learns the answer from the container create of a wake and from the
  image store that `docker info` names, and it makes no probe container.
  Read it after the first Computer wakes.

These statements come from the documentation and the source of Docker
Engine 29, and from Docker Engine 29 under Colima with the containerd image
store on ext4. There, Docker refuses a volume with a size (`quota size
requested but no quota support`), and it creates a container with
`--storage-opt size=100M` with no error but does not hold the layer to that
size. `overlay2` on another filesystem refuses the layer size with
`--storage-opt is supported only for overlay over xfs with 'pquota' mount option`.
XFS with `pquota` is not tested on a VM.

Any other failure of the volume create or the container create is a failure
of the wake, and the Agent's Computer says so.

## Backup and restore

An installation is the state directory, the database and the Computer
volumes, and a copy of one of them restores nothing. `pagis backup` takes
the first two together on both Storage Backends, and `deploy/backup.sh`
adds the third. It takes the volumes of the Workspaces in this
installation's database, by the `org.pagis.workspace` label each volume
carries, so a Docker host that also holds another installation keeps that
installation's volumes out of the archive.

**The daemon must be stopped.** A file copied while the daemon writes it is
not a backup, so `pagis backup` takes the daemon's own instance lock and
refuses to run beside a live daemon. The script stops the daemon first and
starts it again whatever happens.

```bash
deploy/backup.sh /var/backups/pagis/nightly
```

That leaves:

```text
nightly/
  installation/
    manifest.json        the layout, the release, the backend, the time
    state/               the state directory, without the logs, the
                         Client Credential and computer-tokens/
    database.dump        the records, from pg_dump --format=custom
  volumes/
    pagis-volume-<workspace_id>-<agent_id>.tar.gz
```

Only the owner can read a Backup. The script sets the destination to mode
700 before a container writes into it. `pagis backup` gives each directory
of `installation/` mode 700, and each file no permission for a group or for
other users. The volume tarballs get the modes that the container gives
them, but no other user can go through the destination to them.

**The archive has no encryption.** It holds the private content of every
Person:

- `state/`: the memory repositories with their full history, the Artifacts,
  the recordings, the screenshots, the Plugins and the Software. Pagis seals
  only the secrets in it, for example `secrets.enc`.
- `database.dump`: the records of every Person. Only the Credential secrets,
  the one-time code seeds and the `brokered` refresh tokens in it are sealed.
  Sessions and passwords are hashes.
- The volume tarballs: the home of each Agent's Computer, with its files and
  the browser profile of the Agent.

[What Pagis encrypts](../docs/DATA-AND-PRIVACY.md#what-pagis-encrypts) names each store.
Encrypt the archive before it leaves the VM, and keep it apart from the Key
File. For example, with age, where `<recipient>` is your age public key and
`<identity>` is the age identity file that decrypts it:

```bash
# Encrypt the archive, and keep nightly.tar.age.
sudo tar -C /var/backups/pagis -cf - nightly | age -r <recipient> > nightly.tar.age
# Decrypt it again before a restore.
age -d -i <identity> nightly.tar.age | sudo tar -C /var/backups/pagis -xf -
```

restic encrypts each repository, so a restic backup of the directory is also
an encrypted copy. The clear archive stays on the disk of the VM until you
remove it, so keep `/var/backups/pagis` on an encrypted disk too.

Without containers, run
`pagis backup <directory>` with the daemon stopped, and copy the Computer
volumes yourself. On a Local Installation of the Client App,
[desktop/README.md](../desktop/README.md#back-up-and-restore) names the
`pagis` command and the steps.

Three things are never in the archive:

- **The Installation Key**, which seals `secrets.enc`. Keep the key where
  you keep your other secrets. The Key File that a local installation
  generates, `installation-key`, stays out of the archive too.
- **The Client Credential.** On a local installation that file trades for
  a Session of the Administrator, and an archive is copied and kept. It
  stays with the machine that made it, and a restored state directory
  writes a new one at its next boot. A server holds none.
- **The Computer tokens**, `computer-tokens/`. The token of a Computer
  opens its control port while that Computer runs, and the Computers run
  on while the daemon is stopped. The daemon writes a new token at each
  start of a Computer. After a restore, it does not adopt a running
  Computer that has no token on the disk, and it starts that Computer
  again at its next wake.

To restore onto a new host, put `compose.yaml`, `egress.sh`, `Caddyfile`
and `.env` in `deploy/` and the Key File in `deploy/secrets/`, on a machine
that has no Pagis volumes, then run:

```bash
deploy/restore.sh /var/backups/pagis/nightly
```

The script restores the volumes in the archive and no others, and it
refuses to start when a volume of that name already exists on the host.

`pagis restore` refuses a state directory that holds an installation and a
database that holds records, because two installations in one place are
neither. It also refuses an archive of the other backend, so the archive
of a server restores onto a Postgres database. The server that opens the
restored data must be the release that the manifest names, or a newer one.

## A restart is the whole server's

Boot recovery fails every unfinished Run of the installation. One daemon
owns every Run and holds each one's mid-turn state in its own memory, so a
process that ends takes every Run with it. Recovery for one Person alone
would leave another Person's Run marked `running` with no process behind
it.

- Restart at a quiet hour, or tell the team first. The restart in the
  Administration Interface is everybody's restart.
- Each failed Run gives its reason: the daemon restarted, which ends every
  Run on the server. A Run that waits on a Request also gets a note in its
  conversation, and its Request expires.
- A Person asks again. Nothing is lost but the turn that was in progress.

A server that must not interrupt one Person for another needs a second
daemon and a second installation, which this design does not offer.

## Upgrading

An upgrade is the `compose.yaml` of the new release, a new image tag, and a
restart of every service:

```bash
cd deploy
# Replace compose.yaml, egress.sh and Caddyfile with the files in deploy/ at
# the tag of the new release.
sed -i 's/^PAGIS_VERSION=.*/PAGIS_VERSION=<the new release>/' .env
docker compose pull
docker compose up -d
docker compose restart egress proxy
```

`docker compose up -d` starts a service again only when its settings in
`compose.yaml` and `.env` or its image change. The `egress` service runs
`egress.sh` and Caddy reads the `Caddyfile` only when they start, so
`docker compose restart egress proxy` applies a new `egress.sh` and a new
`Caddyfile`. `egress.sh` replaces its rules in one step, so the Computers
keep the policy while the service starts again.

Take a backup first, and restart at a quiet hour.

**`postgres`, `klipper-lb` and `caddy` get upstream patches only with a
release of Pagis.** The `compose.yaml` of each release pins their digests,
and Dependabot moves the digests between releases. So take the
`compose.yaml`, the `egress.sh` and the `Caddyfile` of the new release, and
pull and restart every service, not only `pagis`. Keep your settings in
`.env`, because `compose.yaml` holds none of them.

**The release marker is one-way.** The state directory holds a
`runtime-release` file with the newest release that opened it. A server
reads it before it opens or migrates the database, and **refuses to run
when its own release is older**:

```text
Pagis 0.1.0 cannot open data already opened by newer Pagis 0.2.0
```

The newer server may have changed the data, and Pagis supplies no database
rollback. To go back to an older server, restore a backup taken before the
upgrade into a new state directory and a new database. Pin
`PAGIS_VERSION` in `.env` rather than a moving tag, so a restart never
becomes an upgrade.

The Computer image is upgraded with the daemon: the release pins it, and a
daemon whose pin and image disagree boots no container.

## A local installation that serves several People

A local installation can also serve other People: a household, or a small
team, with SQLite. This is the multi-user mode. It follows from the Public
Origin: an installation whose Public Origin host is not loopback is in the
mode, and no other setting says so.

The daemon terminates no TLS, so the owner runs a proxy or tunnel of their
own on the same machine. It holds the certificate, answers on a name that
the other People open, and forwards to the daemon on loopback. Three setups
follow: Caddy, Tailscale Serve and Cloudflare Tunnel. Each one forwards the
WebSocket upgrade, sets `X-Forwarded-Proto` and `X-Forwarded-For`, and passes
on the name that people open in `Host`. Those headers are also what tell the
daemon that a request came through the proxy and not from the owner's own
Client App, so the Client Credential and a `byo` Google Connection stay with
the machine itself (ADR-0025).

### Turn the mode on

1. Set up one of the proxies or tunnels below, and open its name in a
   browser to check that Pagis answers.
2. In the Administration Interface, open **Settings**, and turn on
   **Multi-user mode** under **Network**. A Client App set up for "Several
   people" opens this switch after the installation.
3. Type the address people open as the **Public Origin**, such as
   `https://pagis.example.net`. It is an absolute `https://` or `http://`
   URL whose host is not loopback. Use `https://`: at an `http://` address
   on another machine, the browser gives no microphone for dictation, a
   password crosses the network in clear text, and the Client App of another
   Person refuses to connect.
4. Keep `127.0.0.1` as the **Trusted Proxy**. That is the address a proxy or
   tunnel on this machine reaches the daemon from. The daemon then believes
   its `X-Forwarded-Proto`, so the Session cookie carries `Secure`, and its
   `X-Forwarded-For`, so the sign-in rate limit counts each browser apart.
5. Select **Turn on and restart**. The daemon writes the Public Origin and
   the Trusted Proxy to `config.toml` and starts again.

The switch keeps the Bind Address on loopback. A proxy or tunnel on the same
machine reaches the daemon there, and so does the owner's Client App, while
the plain-HTTP port stays off the network, so nobody reaches Pagis around
the proxy's TLS.

The installation keeps its Client Credential, so the Client App on the
machine stays signed in. The first-run route stays closed, because a
credential exists. The Administrator creates the other People in the
Administration Interface. To sign in from a browser on another machine,
the owner sets their own address and password there too (ADR-0024).

**Turn the mode off** with the same switch and **Turn off and restart**. The
daemon clears the Public Origin and the Trusted Proxy and binds loopback.
With the mode off, the daemon answers only a request from a program on its
own machine: it refuses every request that carries a proxy header, such as
`X-Forwarded-For`, or a `Host` that is not a loopback name. A browser that
opens a page gets a short page that says so, with status 403. A request under
`/api/`, and every request that does not ask for HTML, gets the JSON error.
The People who signed in from other machines keep their accounts and their
Sessions, and they reach nothing until the mode is on again, also while the
proxy or tunnel still runs. Stop the proxy or tunnel as well.

A server is always in the mode. Its deployment names the Public Origin in
`PAGIS_PUBLIC_ORIGIN`, and the Administration Interface shows the mode with
no switch.

### What a Computer reaches here

A Local Installation has no egress rules. A daemon that runs as the owner
cannot write the firewall of this computer or of the Docker Desktop or
Colima VM, so Pagis does not install them. The Computer of each Agent, also
of an Agent of another Person, therefore reaches the public internet, the
LAN, and each service of this computer that listens on a network address.
Under Docker Desktop or Colima it can possibly also reach the services that
listen on this computer's loopback, through the host gateway. An HTTP or SSE
Plugin server sends its requests from the daemon, so it also reaches the
HTTP services on this computer's loopback (`docs/PLUGINS.md`, "Reach"). The
Pagis product port still asks for a password or the Client Credential, and
the control port of each other Computer still asks for its secret. A server
installs the rules ("What a Computer reaches").

### Caddy

For a name that people reach over the internet or the LAN, with a DNS record
that points at this machine and ports 80 and 443 open to it:

```caddyfile
{
	admin off
}

pagis.example.net {
	reverse_proxy 127.0.0.1:4400
}
```

Run it with `caddy run --config Caddyfile`. Caddy gets and renews the
certificate, sets `X-Forwarded-Proto`, appends to `X-Forwarded-For`, keeps
the `Host` that the browser sent, and passes a WebSocket upgrade through with
no configuration. The Public Origin is `https://pagis.example.net`.
`admin off` closes the admin API of Caddy, which listens on the loopback of
this machine by default, where the daemon and each HTTP Plugin server also
send requests.

### Tailscale Serve

For People on the owner's tailnet, with MagicDNS and HTTPS certificates
turned on for the tailnet:

```bash
tailscale serve --bg 4400
```

Tailscale serves `https://<machine>.<tailnet>.ts.net` to the tailnet and
forwards to `http://127.0.0.1:4400`. It sets `X-Forwarded-Proto` to `https`
and `X-Forwarded-For` to the tailnet address of the person's device, keeps
the `Host` that the browser sent, and passes a WebSocket upgrade through.
The Public Origin is the `https://` address that `tailscale serve status`
prints, such as `https://owner-mac.tail1234.ts.net`. `tailscale serve reset`
stops it.

### Cloudflare Tunnel

For a name on a domain whose DNS is on Cloudflare, with no port open on this
machine:

```bash
cloudflared tunnel login
cloudflared tunnel create pagis
cloudflared tunnel route dns pagis pagis.example.net
```

Then write `~/.cloudflared/config.yml`, with the tunnel UUID that `create`
printed:

```yaml
tunnel: <Tunnel-UUID>
credentials-file: /Users/owner/.cloudflared/<Tunnel-UUID>.json
ingress:
  - hostname: pagis.example.net
    service: http://127.0.0.1:4400
  - service: http_status:404
```

Run it with `cloudflared tunnel run pagis`. Cloudflare holds the
certificate, sets `X-Forwarded-Proto`, appends the browser's address to
`X-Forwarded-For`, and carries WebSockets; `cloudflared` keeps the `Host`
that the browser sent. The Public Origin is `https://pagis.example.net`.

### The live screen for other people

The live screen of a Computer does not go through the proxy or tunnel. A
browser sends and receives it over UDP at the Media Relay's address, which
is `[screen] advertise_ip` in `config.toml`, on one port of
`media_port_first` to `media_port_last` (50000 to 50099 by default). A
local installation advertises `127.0.0.1`, so the mode alone keeps the live
screen on this computer: People on other machines see "Live screen
unavailable". The relay listens on every interface of this computer, so one
setting and one firewall rule carry the screen to them. Each setup above
names its own address:

- **Caddy on the LAN.** Set `advertise_ip` to this computer's LAN address,
  such as `192.168.1.20`, and allow the UDP range in this computer's
  firewall. For People on the internet, set the public address and forward
  the UDP range from the router to this computer.
- **Tailscale Serve.** Set `advertise_ip` to this computer's tailnet
  address, which `tailscale ip -4` prints, such as `100.101.102.103`. The
  tailnet carries UDP, so no port is opened to the internet.
- **Cloudflare Tunnel.** The tunnel carries no UDP. Put a TURN server in
  front of the relay (`relay = "turn"`, the variant in
  `docs/SCREEN-RELAY.md`), or advertise a public address and forward the UDP
  range to this computer.

For example, with Tailscale:

```toml
[screen]
advertise_ip = "100.101.102.103"
```

Restart Pagis after the change. `PAGIS_SCREEN_ADVERTISE_IP` sets the same
address for one run.

In Settings, **Multi-user mode** names the address that the running daemon
advertises. It says whether other machines reach the live screen, and for an
address that is not loopback, the UDP range that the firewall must let
through.
