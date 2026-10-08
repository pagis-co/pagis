# Pagis — Architecture

Decisions with real trade-offs are recorded in `docs/adr/`. Terms are
defined in `CONTEXT.md`.

## Shape

One Rust daemon (a modular monolith), one TypeScript web UI, and one
Docker container for each Agent's Computer. The same daemon serves one
Person on a laptop and a team on a server: one installation is one Org,
the Org holds its People, and each Person owns one private Workspace
(ADR-0023). A local installation keeps its records in SQLite. A server
keeps them in Postgres, and a reverse proxy in front of it holds the TLS
certificate (ADR-0024). Every lookup names the Workspace beside the
record, so the database refuses a record of another Workspace.

```
┌────────────  Product App (TypeScript)  ────────────┐  ┌── Administration Interface ──┐
│ chat · rich blocks · learning feed · live screens  │  │ People · spend · providers   │
│ approvals · automations                            │  │ settings · Plugins · health  │
└──────────▲───────── product port ──────────▲───────┘  └──────▲── Administration ─────┘
           │ REST + WebSocket, Session cookie │ WebRTC         │   Port (loopback)
┌──────────┴──────────  pagis daemon (Rust)  ─┼────────────────┴───────────────────┐
│                                             │                                    │
│  Identity     Org, People, Sessions, the two listeners                           │
│  Channels     Channels, Threads, messages, blocks, events to the UI              │
│  Runtime      resident Agent actors, Runs, the agent loop                        │
│  LLM Router   (library, crates/llm-router) aliases, models                       │
│  Capability   Grants, Connections, Plugins, Hosts, vault, tool list              │
│   Broker                                                                         │
│  Memory       files under git, Page Index, reflection                            │
│  Triggers     Schedules, Event Subscriptions, Wake-ups                           │
│  Execution    containers, screen pipeline, Media Relay, computer use             │
│   Supervisor                                                                     │
│  Coding       Coding Sessions, ACP client, Harness Catalog                       │
│  Telephony    Agent Phone Numbers, Calls, realtime voice bridge                  │
│  Mail         Agent Mailboxes, mail tools                                        │
│  Audit        append-only event log                                              │
│                                                                                  │
│  Storage: SQLite or Postgres · artifact store · git repositories ·               │
│           secrets.enc, sealed by the Installation Key                            │
│           (a keychain or Secret Service item, or the Key File)                   │
└──────┬──────────────────────────────┬──────────────────────────────┬─────────────┘
       │                              │                              │
 Agent Computers and one        Hosts: the Client App           providers: models,
 Plugin Computer per Workspace  on each Person's machine,       Google, carrier,
 (Docker, Tenant Network)       over its WebSocket              mail host
```

## Installations

A release installs in two ways (ADR-0025):

- **The Client App** on the Person's own computer (macOS arm64, Linux amd64
  and arm64). It either
  installs and supervises the one Server Runtime that its Runtime Lock
  names, or connects to a server that somebody else runs.
- **The Headless Server**, a Linux container image for a team on a VM
  (https://docs.pagis.co/server).

Whether an installation serves the People at its own machine or People on
other machines follows from its Public Origin, not from its Bind Address.
Every local installation holds a Client Credential, which the Client App
on that machine trades for a Session, whatever the Public Origin. The
daemon accepts the trade only from a loopback socket peer and never through
a proxy. A server holds no credential, and every Person signs in with an
address and a password.

## Components

### Identity

One Org holds every Person, with the `administrator` or `member` role. An
Administrator creates the accounts; there is no sign-up. A Session, in an
HTTP-only cookie, is the only way in. A password sign-in, the trade of a
Client Credential, and a one-time sign-in link hand one out.

A Session ends when the Person signs out of it, when an Administrator
disables the Person, resets their password or sets a way in for them, and
at its expiry. Each live connection of the Session ends with it: its
sockets close with code 1008, the Host of such a socket becomes absent, and
its Media Relay paths close. When an Administrator ends every Session of a
Person, each Takeover of their Computers ends too and the input goes back
to the Agent. A sign-out leaves a Takeover in place: another Session of the
Person can hold it, and it ends at its inactivity timeout.

The daemon binds two listeners in one process. The product port serves the
Product App and each Person's own Workspace. The Administration Port, on
loopback by default, serves the Administration Interface: the People,
spend, Sessions, Hosts, resources, provider setup, System Settings,
Plugins, restart and health. The product router carries no
installation-wide route (ADR-0024).

The Org's own records live in the Org's Workspace, a Workspace that no
Person owns: the installed Plugins and the Installation Connections (the
carrier and the mail domain). No Session, seed or per-Workspace sweep
reaches it.

### Channels

Owns Channels, Threads and messages. A Channel connects a Person and one or
more Agents, or Agents with each other. Channels and Threads are the only
agent-to-agent protocol inside a Workspace: observable, persistent, and the
same as the Person's own Channel (ADR-0003). Messages carry blocks: a
curated, typed vocabulary the UI renders natively. A block that asks views
a Request, so an approval card is an approval API (ADR-0004). The daemon
derives the Needs-You Queue on each read from the records, and stores no
copy of it. A daemon-lifetime task derives the queue again after each
event that can change it, and publishes `needs_you.added` or
`needs_you.removed` for each item that enters or leaves it (ADR-0022,
ADR-0030).

### Runtime

Each Agent is a resident actor inside the daemon: a small task that holds
the Agent's identity and listens for triggers (messages, Schedules,
Incoming Events, other Agents). Idle cost is memory only. A trigger starts
a Run: the agent loop executes turns against the LLM router until the work
completes (ADR-0002). Pagis owns the loop, with no external harness under
it (ADR-0001). A Coding Harness runs beside the loop as a guest over ACP
(ADR-0033). Run states: `queued`, `running`, `reflecting`,
`waiting_for_user`, `waiting_for_approval`, and then `completed`, `failed`
or `canceled`. Every step appends to the audit log.

A conversation keeps its continuity in a Continuation Record when its
history does not fit one model request, and the complete request has one
token budget (ADR-0009).

### LLM router

The `crates/llm-router` library. Agents name a Model Alias; the Workspace
maps each alias to ordered provider candidates. The Administrator supplies
one key for each provider, and each Person chooses what their Agents think
with. The router also carries speech, realtime voice and the pixel
computer-use protocols.

### Capability broker

The one enforcement point for everything an Agent may touch (ADR-0005).

- **Connections.** Mail, calendar and other provider accounts. A Person
  connects once, and Agents use a Connection through Grants. A Connection
  is `byo`, with the Person's own OAuth client, or `brokered`, through the
  Installation OAuth Client an Administrator set up (ADR-0012). A
  `brokered` refresh token is sealed on its row with the Workspace's Tenant
  Data Key, a `byo` Google token stays in `gog`'s own file store under the
  Workspace's `GOG_HOME`, and provider keys are in `secrets.enc`. No token
  enters a container (ADR-0013).
- **Vault.** Credentials that Grants scope to named Agents and domains. The
  daemon fills them into verified fields of the Credential's own `https` site,
  through a browser channel it owns; the model never sees a secret value.
- **Tools.** Each Run sees one tool list assembled from its Capability
  Snapshot: core tools, Connection tools, Software List tools, Plugin tools
  and Host tools. The Agent does not know where a tool runs.
- **Plugins.** An Administrator installs a Plugin for the Org. Each
  Workspace runs its stdio MCP servers in its own Plugin Computer, and the
  daemon sends the requests to its HTTP and SSE servers. Each Agent needs a
  Plugin Grant (ADR-0017).
- **Hosts.** A Host is one of a Person's own machines, reached through the
  Client App running on it (ADR-0015). A host action never runs in the
  daemon. The Grant names the machine, approval is on by default, and an
  allow rule belongs to that one machine.

### Memory

Plain Markdown files under git (ADR-0007): a private directory for each
Agent and one shared directory for the Workspace. Subject Pages keep the
arrivals of one matter, and fact files state one thing each. A Page Index
and a full-text index are derived from the files (ADR-0008).

Memory changes on three paths (ADR-0010). A reply commits the memory change
it staged, with no second model phase. A compaction proposes memory changes
beside its Continuation Record (ADR-0009). Reflection reviews selected
evidence in its own Run: a synced arrival that its Reflection Filter
selects, or durable Pending Evidence. Commits apply at once, and the
Learning Feed shows each one with a revert.

### Triggers

Durable Schedules and Event Subscriptions, with a Wake-up between a source
occurrence and a Run (ADR-0006). The Automations place shows every rule.

### Execution supervisor

Owns the Computers: one Docker container for each Agent with a persistent
disk, a terminal and Chromium under a headless compositor (ADR-0014). The
screen pipeline is Pagis's own, in Rust: compositor frame capture,
damage-aware encoding, and WebRTC to the UI, with an input channel back.
Media crosses the Media Relay: a Computer publishes no media port, registers
outbound with the relay for each viewer session, and one advertised address
and one UDP port range serve every screen of the installation
(https://docs.pagis.co/server/live-screen). The Person can take control at any moment, and the
Agent can hand control to the Person to finish a step. Computer control is
model-native pixel computer use through the router. Software List tools
execute inside the calling Agent's Computer; the container is the sandbox.

Each container joins the Tenant Network of its Workspace, and an Awake Cap
limits how many Computers are awake for one Workspace and for the whole
server.

The image ships the runtimes the Agent scripts with, so a task rarely
waits for a download: `python3` with `uv`, Node 24 LTS with `npm` and
`pnpm`, and `bash`. uv and Node come from the vendors' release tarballs,
pinned by version and sha256 in `computer/Dockerfile`, and npm and pnpm
from their npm registry tarballs, pinned by version and sha512. They are
refreshed with the image version. The Agent keeps each package's
dependencies inside `~/software/<name>`.

A system package that is not in the image goes through
`sudo pagis-apt install <pkg>...`. `pagis-apt` is a root-owned wrapper
(`/usr/local/sbin/pagis-apt`, mode 0755) and the only command the Agent
uid may run as root. The wrapper, not the caller, owns every option apt
sees: it accepts one grammar, refuses any package name that is not
`^[a-z0-9][a-z0-9.+-]*$`, unsets `APT_CONFIG`, refreshes the package
lists when they are stale, and logs each install to
`/var/log/pagis-apt.log`. A sudoers rule on `apt-get` itself would not
hold: `-o DPkg::Post-Invoke::=`, `-c`, `--reinstall` and a local `.deb`
path each run an arbitrary command as root. The container stays the
sandbox: the wrapper is a root step inside it, not a way out of it.

The exec seam is the second path into that same container, beside the
screen. `computer_shell` runs one command as the Agent's own uid in
`/data/agent`, in a fixed environment, and it needs no approval: the
container is the sandbox, so a command there has the trust the screen
has. The deadline runs inside the container, because the container API
cannot stop a command that runs; a wrapper stops the command and its
children. The daemon reads the whole output but keeps only a head and a
tail of it, because a command whose output nobody reads stops at its next
write. A command wakes a Computer that sleeps, and holds it awake until
the command ends.

### Coding Sessions

The daemon is the ACP client of a Coding Harness, which runs on a Host
through the session socket of its Client App, or in the Agent's Computer
(ADR-0033). The Agent that started a Coding Session supervises it from the
session's Thread with core tools, and a Session Rule wakes the Agent when a
turn ends, a decision waits or the session ends. Pagis policy answers each
Harness Permission first, then the Session Approval Mode decides who answers.
Coding Sessions are not built.

### Telephony

Each Agent can hold one Agent Phone Number, its desk line, bought on the
Org's carrier account, which an Administrator sets up. The carrier is an
Installation Connection; the number is part of the Agent's identity, so an
Agent that holds a number can call and one that does not cannot. The daemon
is a SIP endpoint (ADR-0020). Calls out and in bridge the carrier's media
to a realtime speech model through the router. The Remote Party's number
proposes a Trust Tier and a Keypad Code confirms it (ADR-0021): the owner
instructs, a trusted number requests, and a stranger is heard but reaches no
tool.

Texting is a third carrier seam beside the number seam and the call seam
(ADR-0020). `TextTransport` has an implementation for Telnyx and Twilio,
and the `text_records` table and the texting fields of `phone_numbers`
exist. No path sends, receives or shows a text: the collector, the
readiness read, the text tools, the Standing Text Rule and the `text` block
are not built.

### Mail

Each Agent can hold one Agent Mailbox on the Org's mail domain, as its own
identity (ADR-0019). One set of mail tools serves the Agent's mailbox and
the Person's own mail accounts. A Standing Mail Rule wakes the Agent on each
inbound mail.

### Notifications

A Notification is a Web Push from the daemon to each Push Subscription of
the Person, when an item enters the Needs-You Queue (ADR-0030). It goes
through the push service of a browser, or through the Push Relay to the
Mobile App, and only the client decrypts it.

A daemon-lifetime task reads each `needs_you.added` event and sends the
Notification of its item to each Push Subscription of that Workspace,
each in a task of its own. No push goes when an item leaves the queue.
The plaintext is the Declarative Web Push JSON: the name of the Agent,
the line of the item, its place on the Public Origin, the queue count as
the badge, and the Pagis fields `{v, item, kind, request?}`. It holds no
content of a message, a tool input or a Credential. An Approval of a
tool action or a credential action carries its Request, with the answers
Approve once and Deny. Each push has `TTL: 86400`, `Urgency: high` for
`approval`, `waiting` and `keypad` and `normal` for the other kinds, and
a `Topic` from the hash of the item id.

A new item waits while the Person is active in a client. A visible
client sends an `activity` frame on the event socket after input, at
most once every 30 s, and the daemon keeps the time of the last one for
each Workspace in memory. An item that enters the queue less than 120 s
after that time waits until 120 s pass with no activity, and then goes
when it is still in the queue. A `needs_you.removed` ends the wait at
once.

A push service that answers `404` or `410` ends the Push Subscription,
and the daemon deletes it. A `429` gets one more send after its
`Retry-After`, at most 300 s later. A delivery keeps the time of the
send. The Person sends a test Notification to one Push Subscription from
Settings, and the route answers what the push service answered.

### Audit

Append-only event log across all components. Every Run step, Grant use,
Call and memory commit lands here.
