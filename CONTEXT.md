# Pagis — Domain Glossary

The canonical term for each concept of the product, in sections, and
alphabetical inside each section. The decisions and their reasons are in
`docs/adr/`.

## Installation and release

### Administration Interface
What an Administrator reads and changes about the installation: the spend
and the last sign-in of each Person, the Sessions, the Hosts, the resources
of each Person, the provider setup, the System Settings, the Plugins, the
restart and the health of the daemon. The Administration Port serves it.
The Product App carries one link to it, and the Client App opens it from
its menu (ADR-0024).
_Avoid_: admin panel, console, dashboard

### Administration Port
The second listener of the daemon, which serves the Administration
Interface and every installation-wide route. It binds loopback by default.
Every route on it needs a signed-in Administrator, except the Server Setup
route and the password sign-in (ADR-0024).
_Avoid_: admin port, management port

### Analytics
The anonymous usage data that a release build of the daemon sends to
PostHog: the first start, an upgrade, and one Installation Report each
day, keyed by the Installation ID. Every property is an enum, a number or
a flag. A build from source sends none. The Analytics System Setting and
`DO_NOT_TRACK` stop it (ADR-0026).
_Avoid_: telemetry, tracking, diagnostics

### Backup
A consistent copy of one installation, taken while the daemon is stopped:
the State Directory and the records, and on a Headless Server one tarball
for each Computer volume. A Backup never holds the Installation Key, the
Client Credential, the access token of a Computer or a Model Request
Capture. It restores onto a new
State Directory and an empty database, on the same Storage Backend, for its
own release or a newer one (ADR-0024).
_Avoid_: database dump

### Bind Address
The address one listener of the daemon binds. The product port and the
Administration Port each have one, and both default to loopback. The Bind
Address says where the daemon listens. It does not decide whether a Local
Installation serves other machines: Remote Access does (ADR-0024,
ADR-0028).
_Avoid_: listen address

### Client App
The signed application a Person opens on their own computer: macOS arm64,
and Linux amd64 and arm64 as an AppImage and a deb. At setup it asks
whether to install on this computer ("just me" or "several people") or to
connect to a server. On this computer it installs and supervises one Server
Runtime. Connected to a server, it keeps the server's origin and opens the
server's own sign-in page, or the page of a Sign-In Link that the Person
pasted at setup. Either way it shows the Product App, stays in
the tray (the menu bar on macOS), and registers its machine as a Host
(ADR-0025).
_Avoid_: Desktop App, desktop client, the shell

### Client Credential
The secret file a Local Installation writes for its own Client App, which
the Client App trades for a Session of the seeded Administrator. It belongs
to the installation, not to a Person, and a browser never receives it.
Every Local Installation holds one and a Server never does: `--local` says
which kind the daemon is. The daemon accepts the trade only from a program
on the same machine, never through a proxy (ADR-0025).

### Compatibility Range
The server releases that a connected Client App accepts: its own release
and every later one that promises the same API. The client checks it on
every start and refuses a server outside it, saying which end to update
(ADR-0025). The Mobile App holds a lower bound only: the first release that
serves Notifications, and every later one. Not built (ADR-0032).

### Computer Image
The container image every Computer and Plugin Computer runs. It has a
version number of its own, and each release pins one digest of it
(ADR-0025).

### Headless Server
The Pagis server as a Linux container image, for a team that runs it on
its own VM, or a household on a Linux machine at home: the same daemon and
Product App as the Server Runtime, with every program the daemon starts. It
refuses to start without a Public Origin whose host is not loopback or
without a Postgres database URL, and it holds no Client Credential. Its
trust root is the registry and the digest that the deployment pins
(ADR-0024).
_Avoid_: server container, docker server, self-hosted build

### Installation ID
The random value that names one installation in its Analytics. The
daemon makes it at the first send and keeps it in `analytics.json` in the
State Directory. It comes from no hardware, host name or Public Origin
(ADR-0026).

### Installation Report
The daily Analytics event of one installation: the kind of installation,
the Storage Backend, whether Remote Access is on, whether Docker answers,
the number of People and of Agents as ranges, and one flag for each
feature in use (ADR-0026).

### Local Installation
An installation on the Person's own computer: the Server Runtime that a
Client App installs. It keeps its data in the State Directory, its records
in SQLite and its Computers on the same machine. It sends model requests to
the configured providers and runs no model. It serves the People at the
machine, and other machines and People too while Remote Access is on. It
always holds a Client Credential (ADR-0025).
_Avoid_: local mode, single-user mode, offline mode

### Mobile App
The Pagis app for iOS and Android. It shows the server's own Product App,
signs in with a Sign-In Link, and shows Notifications with **Approve once**
and **Deny** for an Approval. It connects to a Server, or to a Local
Installation in Remote Access, over `https://`. It installs nothing, and it
is not a Host. Not built (ADR-0032).
_Avoid_: phone app, mobile client, native app

### Onboarding
The first steps the Product App shows a new Local Installation: welcome,
providers, computer. It takes a key for one provider or more, the default
model, and an optional Docker endpoint.
A server, and a Person whom an Administrator creates, skip it (ADR-0025).
_Avoid_: wizard

### Product App
The web application every server serves: the Workspace experience of one
Person. It holds no Client App privilege and no installation-wide route
(ADR-0025, ADR-0024).
_Avoid_: product interface, web UI

### Public Origin
The origin a browser reaches an installation at: the scheme, the host and
the port. On a server it is the name the proxy answers on, over TLS. On a
Local Installation it derives from the Bind Address and the port, unless
Remote Access sets it to the public name of the owner's Tailscale Funnel.
The CORS answer names it (ADR-0024, ADR-0028).
_Avoid_: base URL, external URL, site address

### Push Relay
The service that the project runs for its store apps. It takes a Web Push
for an installation of the Mobile App and forwards the ciphertext to APNs
or FCM. It holds the APNs and FCM keys and never a key that decrypts a
payload. Not built (ADR-0030).

### Release Marker
The file `runtime-release` in the State Directory, which holds the newest
release that opened the data. A server older than the marker refuses to
open the data. Nothing lowers it (ADR-0025).

### Release Matrix
The four artifacts of one release: the Client App, the Computer Image, the
Server Package of each Client App platform, and the Headless Server image.
All but the Computer Image carry the release number and come from one
commit. A mismatch is refused where it happens: the Runtime Lock on a
download, the Compatibility Range on a connection, the image version label
on a Computer, and the Release Marker on the data (ADR-0025).
_Avoid_: build matrix, artifact list

### Remote Access
How an installation at home serves the owner's other machines and the
other People of the installation: a public name that the owner's
Tailscale Funnel answers on, where another machine signs in with a
Sign-In Link and never with a password. An Administrator of a Local
Installation turns it on with one switch in the Administration Interface,
which turns on the Funnel, sets the Public Origin and the Trusted Proxy,
and restarts the daemon. A Local Installation serves other machines only
while it is on. While it is on, a TURN server in the daemon carries the
live screen to a browser on another machine, through the Funnel on port
8443. A Headless Server at home is in it through the Tailscale service of
its compose deployment, which runs the Funnel in place of a proxy. A
Server behind its own proxy is not in it (ADR-0028).
_Avoid_: tunnel mode, remote mode, pairing, multi-user mode

### Runtime Lock
The Client App's signed statement of the one Server Runtime release it
accepts: the platform and architecture, the Server Package and its
contents, and the Computer Image digest. Each platform has its own lock.
It is the trust root of a download, and it never selects a latest
compatible release (ADR-0025).

### Secret Service
The freedesktop D-Bus interface to the desktop keyring on Linux (GNOME
Keyring, KWallet, KeePassXC). A Local Installation on Linux keeps its
Installation Key there, in the item with service `pagis` and account
`safe-storage` (ADR-0013).
_Avoid_: libsecret, gnome-keyring, Linux keychain

### Server
An installation that serves People on other machines from a machine that
nobody sits at: the Headless Server on a VM or at home. It keeps its
records in Postgres, and a proxy in front of it holds the TLS certificate:
the proxy of its deployment, or at home the Tailscale Funnel of Remote
Access (ADR-0024, ADR-0028).
_Avoid_: hosted installation, cloud

### Server Package
The release asset that holds one Server Runtime release for one platform:
a signed, notarized disk image on macOS arm64, and a gzip tar archive on
Linux amd64 and arm64. The Client App downloads it and checks it against
the Runtime Lock (ADR-0025).
_Avoid_: server DMG, standalone archive

### Server Runtime
The Pagis server that a Client App installs on the Person's computer, with
the programs its release supplies. One Client App release names one Server
Runtime release (ADR-0025).

### Server Setup
How an installation with nobody who can sign in gets its first
Administrator: from the `PAGIS_ADMIN_*` and provider key variables at the
first start, or from the first-run route on the Administration Port. It
runs one time. A Local Installation never runs it, because its Client App
trades the Client Credential (ADR-0024).
_Avoid_: bootstrap, first boot

### State Directory
The directory that holds the files of one installation: the
configuration, the SQLite records, the memory repositories, the sealed
secrets, the Client Credential, the Release Marker, the Installation ID,
the Artifacts, the Plugins, the Software and the logs. `PAGIS_HOME` names it, and the default
is `~/.pagis`. One daemon owns one State Directory (ADR-0024).
_Avoid_: workspace home, workspace directory

### Storage Backend
Where an installation keeps its records: SQLite at `pagis.db` in the State
Directory for a Local Installation, and Postgres for a Server. A Local
Installation refuses to start with a database URL, and a Server refuses to
start without one. The files stay on the disk either way (ADR-0024).

### System Setting
A setting of the installation rather than of a Workspace: the port, the
Docker endpoint, the log level, the data directory, Remote Access of a
Local Installation, the Analytics, the Model Request Capture, and the Home
Exit of a Server, which an Administrator turns off for every Person and
never on for one. An Administrator changes it in the Administration
Interface, and the daemon writes `config.toml` (ADR-0024, ADR-0026,
ADR-0029, ADR-0031).
The timezone is not one: it belongs to each Person (ADR-0006).

### Trusted Proxy
The one address whose `X-Forwarded-For` and `X-Forwarded-Proto` the daemon
believes. The daemon terminates no TLS: the Trusted Proxy holds the
certificate. In Remote Access it is the Tailscale of the same machine, at
loopback (ADR-0024, ADR-0028).
_Avoid_: upstream, front-end proxy, load balancer

### Update
A newer Client App release that the Client App finds, downloads, checks
and installs over itself. Installing an Update on the Client App of a
Local Installation causes an Upgrade at the next start. A connected Client
App takes only the Update to its server's release (ADR-0027).
_Avoid_: auto-update, patch, new version

### Update Key
The Ed25519 key that signs the Linux checksum list of each release for the
Update of a Client App. The Client App embeds its public half and installs
no Update that it did not sign. It signs nothing else (ADR-0027).
_Avoid_: release key (the OpenPGP key that a Person checks a download with)

### Upgrade
The first start of a newer Server Runtime release on the data of an
installation: a Backup of the old release on a Local Installation, the
Release Marker, and the migration. Nothing reverses an Upgrade (ADR-0025,
ADR-0027).
_Avoid_: update (of an installation), migration

## Org, People and access

### Administrator
The role that configures the installation: it manages the People, their
passwords and Spend Caps, reads spend, and sets up the providers, the
System Settings and the Plugins. Every Org has at least one: the seeded
Person of a Local Installation, or the first Person of Server Setup
(ADR-0023).

### Allow Rule
A rule inside a Grant that approves one class of gated actions in advance,
such as a command prefix on one Host or mail to one domain. The Person
makes one from an approval card or in settings. It reaches no further than
its Grant (ADR-0005).

### Exposure Stamp
The record of which Grants the author of a message or a memory file held
when it wrote the words. When the Person revokes one of them, the words are
withheld from every reader (ADR-0004, ADR-0008).

### Grant
A scoped permission that lets one Agent use one Workspace resource: a
Connection with capabilities, the Vault with allowed domains, a Host with
command Allow Rules, or a Plugin. A Grant has numbered revisions. An action
that matches the live revision needs no new approval (ADR-0005).

### Member
The role of a Person who changes only their own Workspace. A Member reads
nothing of the Org and gets nothing on the Administration Port (ADR-0023).

### Org
The installation, as the People in it share it. One installation is one
Org. The Org holds what decides what the installation can reach or spend:
the provider keys, the Installation OAuth Client, the Installation
Connections, the installed Plugins, the System Settings, the People with
their roles and Spend Caps, and the Awake Cap (ADR-0023).

### Org's Workspace
The one Workspace that the Org owns and no Person owns. It holds the
installed Plugins with their Bindings and the Installation Connections. No
Session, seed, per-Workspace sweep or Tenant Data Key touches it
(ADR-0023).

### Person
One human in the Org, with a name, a role and one Workspace. A Person signs
in before any client reads or writes their data (ADR-0023).
_Avoid_: user record, account (for a Person; an account is at an external provider)

### Session
What a client holds after a Person signs in: an HTTP-only, host-only,
`SameSite=Strict` cookie that names the Person and the kind of client. A
browser's Session also carries the name of the browser and its system, such
as "Safari on macOS", or "Pagis on macOS" for the product window of a
connected Client App. The Mobile App holds a `browser` Session named "Pagis
on iPhone", "Pagis on iPad" or "Pagis on Android". Not built: the Mobile
App. A Session ends at sign-out, when the Person removes it from their
Sessions list, and 30 days after its last use. A password sign-in, the
trade of a Client Credential and a Sign-In Link hand one out. It grants
nothing on its own (ADR-0023, ADR-0028, ADR-0032).
_Avoid_: bearer, login token

### Sign-In Link
A one-use URL whose secret trades for a Session. There are two kinds. The
start link is the one that the `pagis` binary prints at start on a Local
Installation, good for one minute. It is the one URL of the daemon that
carries a secret in its path. The daemon accepts it from the same machine
alone, and an installation that holds no Client Credential refuses it
(ADR-0025). A link of the Public Origin, `<public origin>/sign-in#<secret>`,
goes with a QR code. A signed-in Person makes one for one more client of
their own, good for five minutes. An Administrator gets one as the invite of
a Person, good for seven days. `pagis pair` prints one on the machine of the
installation, good for five minutes. Its page posts the secret, so opening
the link spends nothing. In Remote Access it is the one way in for another
machine, and the sign-in page there takes a pasted link (ADR-0028). The
Mobile App scans the QR code and spends the link in its web view. Not built
(ADR-0032).
_Avoid_: pairing code, magic link, invite token

### Spend Cap
What one Person may spend on model calls in a calendar month of their own
timezone, in US dollars, set by an Administrator. A Run that would start
over the cap, or on a model with no known price, stops before it asks a
model anything. A Person under no cap is never stopped (ADR-0023).
_Avoid_: quota, spend limit

### Usage Record
What one model call spent: the tokens the provider reported and the cost
that the price list makes of them, with the Workspace and the Run. A model
with no known price has an unknown cost, never zero. It is an accounting
record, not a bill (ADR-0010, ADR-0023).
_Avoid_: usage row, token log, billing record

### Workspace
One Person's private scope: their Agents, memory, conversations,
Connections, Grants, Vault, Trust List, Software List, Agent Phone Numbers,
Agent Mailboxes, Hosts, Retention Policies and timezone. The timezone comes
from the browser or Client App at the first sign-in, and the Person changes
it in Settings. No Person reads another Person's Workspace, and every store
lookup names the Workspace beside the record, except the lookups of the
installation-wide records. A **tenant** is a Workspace seen as the unit of
isolation, as in the Tenant Data Key and the Tenant Network (ADR-0006,
ADR-0023).

## Agents and conversations

### Agent
One of a Person's persistent helpers: a job, a description, a personality,
memory and history, capabilities, a Computer, an Agent Voice, and
optionally an Agent Mailbox and an Agent Phone Number. The job is the first
thing the Agent reads about itself; the description is the one line other
Agents read. An Agent is not a chat session. The product calls an Agent a
**sprite** in the words it shows the Person.
_Avoid_: sprite (in the glossary, code and ADRs), bot

### Agent Voice
How one Agent sounds, the same in a Thread and on a Call. It is one name of
the Provider Voice List. A model that does not have the voice speaks in its
default, says so, and the Call records the voice it used (ADR-0020).

### Briefing
The part of the system prompt that tells a Run about its Channel: who
shares it, whether the Person is one of them, who asked, and which
conversation waits for the answer (ADR-0003).

### Channel
The container for a conversation: the Person with one Agent, the Person
with a group of Agents, or Agents with each other. The direct Channel
between the Person and one Agent is that Agent's **DM Channel**. The
Person reads every Channel. A Channel that Agents opened between
themselves is read-only to the Person (ADR-0003).

### Chief of Staff
The one active Agent a Workspace addresses by default. The Product App
pins it at the top of the sidebar, Home is its Report, and the composer on
Home speaks to it. The seed makes one Agent, Pixie, and names it; the
Person can move the designation to any active Agent. It grants no power
(ADR-0022).

### Compaction
The step that summarizes the older part of a conversation into a
Continuation Record when the complete model request passes its budget. It
can also propose memory changes. The Run stays `running` while it compacts
(ADR-0009).

### Continuation Record
The checkpoint that lets one Agent continue one conversation after older
messages leave the model request: goals, constraints, decisions, work
state, open questions, next steps and references to the evidence. It is
working context, not memory, and no other conversation reads it. A
forgotten source message or a changed Grant revision deletes it
(ADR-0009).

### Delegation
One Agent giving work to another by message, in its DM Channel with that
Agent. Each step between Agents adds one hop, and a chain at the hop cap
(default 8) starts no further Run (ADR-0003).

### Model Alias
A named brain: an ordered list of provider models that a Workspace keeps
and each Agent names. Some aliases are plumbing, such as `transcribe` and
`speak`. The Person chooses the aliases, and the key behind each provider
belongs to the Org. The `default` alias starts as the one model the Person
picks at Onboarding from the Provider Model List (ADR-0023, ADR-0025).
_Avoid_: model group, route list

### Model Request Capture
A copy of one model request of a Run and of the provider's answer, which
the daemon keeps while an Administrator has the Model Request Capture
System Setting on. Each image in it is a hash and a size. Only an
Administrator reads it, and it expires after the retention of the setting.
Forget deletes it, and a Backup leaves it out (ADR-0031).
_Avoid_: request log, trace, prompt log

### Model Preference
The models the product prefers for a well-known Model Alias, best first,
grouped by provider. A route takes the preferred models of the first
provider that holds a key and serves the alias's Provider Use. For the
`default` alias it selects one model from the Provider Model List and
never replaces the list: the daemon takes the first preferred model whose
provider holds a key and lists it (ADR-0025).
_Avoid_: model profile, recommended models

### Origin
The conversation a delegation chain owes an answer to: the Agent that owes
it, and the Channel and Thread it owes it in. The Run that answers replies
into the Origin (ADR-0003).

### Pointer
A derived entry in a DM Channel: a link to a message that the Agent posted
in another Channel, with what it said. It is computed at read time, not
stored.

### Provider Model List
The models that one provider lists for the installation's key, with the
context window, output limit and prices that the provider reports, and,
where the provider names them, what each model outputs and the voices of
each speech model. The daemon refreshes it every hour and on a key change,
and the Models settings offer it as choices. A model on the list runs even
when the built-in metadata table does not know it. A price that nothing
names is unknown, never zero. Reading the list is also the key check
(`docs/DESIGN.md`, ADR-0025).
_Avoid_: model catalogue, allowed models

### Provider Use
One thing a provider's key does in Pagis: thinking, spoken replies,
dictation or calls. A provider serves a Model Alias only for a use it has.
OpenAI has all four; OpenRouter has thinking, spoken replies and
dictation; Deepgram and ElevenLabs have spoken replies and dictation;
Anthropic has
thinking. Onboarding shows the uses of each provider and what a set of
keys covers (ADR-0025).
_Avoid_: capability (a Grant word), feature

### Provider Voice List
The voices of the model that speaks for a Workspace: the first candidate of
the `speak` alias whose provider holds a key and serves spoken replies. The
daemon reads it from the provider with the Provider Model List; a provider
that lists no voices, such as OpenAI, has a fixed set. ElevenLabs lists
the voices of the account. Each Agent Voice holds the id of one of its
voices, and the first is the default (ADR-0020).
_Avoid_: voice catalogue

### Roster
The Agents of one Workspace. The roster is flat: no Agent has
architectural authority over another (ADR-0022).
_Avoid_: roster of People (say the People of the Org)

### Thread
A reply thread rooted at one message in a Channel: one piece of work or one
topic. Threads are one level deep (ADR-0003).

## Runs and triggers

### Arrival Run
The Run that one selected acquisition batch starts: it reflects the Subject
Pages that the Reflection Filter selected, with no Channel and no reply
phase. Arrival Runs use their own slot pool (ADR-0010, ADR-0011).

### Cooldown
The 14 days after the Schedule of a Subject Page fires, during which the
Agent cannot schedule that page again. A refused reschedule returns the
reason (ADR-0010).

### Event Subscription
A durable rule that matches Incoming Events from one Connection, or from
the Agent's own mailbox or number, and asks one Agent to act in a target
Channel and optional Thread (ADR-0006).

### Incoming Event
A provider occurrence, received through a Connection, an Agent Mailbox or
an Agent Phone Number, that may wake an Agent. It is not an audit event
(ADR-0006).

### Review Run
The Run that reflects one claimed batch of due Pending Evidence. It uses
the arrival slot pool (ADR-0010).

### Run
A unit of work an Agent performs: its loop from a Trigger to the end. A Run
binds to one Channel, and to one Thread when the Trigger is in a Thread. It
is `queued`, then `running`; an Arrival Run and a Review Run are
`reflecting`. It can wait for the Person or for an approval, and it ends
`completed`, `failed` or `canceled` (ADR-0002, ADR-0010).

### Schedule
A durable rule that asks one Agent to act at future times in a target
Channel and optional Thread: a wall-clock cron, an interval, or one
instant. A Schedule that Reflection sets names its Subject Page
(ADR-0006).

### Schedule Occurrence
One time at which a Schedule becomes due. It stays on record when Pagis
skips or combines it and no Run starts (ADR-0006).

### Standing Rule
The Event Subscription the daemon makes with an Agent's own mailbox or
number, which wakes the Agent in its DM Channel:
- the **Standing Mail Rule**, on every inbound mail to an Agent Mailbox.
  The Person pauses, edits or deletes it; the Agent may narrow it and never
  delete it (ADR-0019).
- the **Standing Call Rule**, after every Call the Agent answers. Taking
  the number back archives it (ADR-0020).
- the **Standing Text Rule**, on every text the number receives. Not built
  (ADR-0020).

### Trigger
What starts a Run: a message to the Agent, a Schedule, or an Incoming
Event. A message from another Agent is a message trigger like any other
(ADR-0002).

### Turn
One cycle inside a Run: one model call and the tool calls it asks for. The
Run ends when the model stops without a tool call, or a cap or a
cancellation stops it (ADR-0002).

### Wake-only Schedule
A Schedule that gives no authority in advance: it wakes its Agent into an
ordinary Run, which can send one message, reschedule, or stay silent.
Reflection can set one on a Subject Page with no approval card
(ADR-0010).

### Wake-up
The durable decision that turns a Schedule Occurrence or an Incoming Event
into a Run. An occurrence that is skipped, combined or refused makes none
(ADR-0006).

## Memory and learning

### Alias
Another name of a memory page (a short form, a nickname, an address or a
handle) on the `aliases` line of its Front Matter. An alias is an entity
word of the page (ADR-0007).

### Backfill Reflection
The pass that reflects the historical Subject Pages that the Reflection
Filter selects once a resource is caught up: newest first, at most 20 pages
for each Run, under a daily budget, after live delivery (ADR-0011).

### Brief
The memory an Agent receives for one reply turn or one fired Schedule: at
most three relevant or newly changed pages, inside an envelope that marks
them as data. The daemon selects them with no model (ADR-0010).

### Conversation Page
The Subject Page of one Channel, at
`private/subjects/conversation/<channel>.md`: what the conversation is
about, where it stands, its decisions and its Open Work. The Agent writes
it while it replies, and a Compaction can propose it. A conversation that
ends settled has none (ADR-0007).

### Fact
One row of a Facts Table: a claim, its kind and the source reference it
rests on. A Fact is active, superseded or forgotten. It gives no authority
(ADR-0007).

### Fact File
A small memory file that states one thing an Agent learned: Front Matter
and a body, with no Facts Table, Schedules or Timeline. The index file at
a scope root is not one (ADR-0007).

### Facts Table
The derived index of Facts on a Subject Page, with the columns Claim, Kind
and Source reference. The Agent writes it from the Timeline in a reply
turn, a Compaction or a Reflection. A bad row does not hide a good one
(ADR-0007).

### Forget
The Person's order to remove one source item, or everything from one
account, from what Pagis holds: the structured records, the memory paths
and history derived from it, and each message, tool result and Model
Request Capture of a Run that read it. It also blocks a new retrieval of
that source until the Person opts in again. The block holds only a keyed
hash of the item's id, and the key derives from the Tenant Data Key
(ADR-0008, ADR-0031).
_Avoid_: unlearn

### Front Matter
The `---` block that starts a memory file, with `title`, `kind`, `aliases`
and `links`. It is the only heading form and not compiled truth
(ADR-0007).

### Incomplete Version
A Source Version that holds no metadata yet. The daemon asks the provider
for it in batches, and the Reflection Filter decides again with signals
(ADR-0011).

### Learning Feed
The timeline of committed memory changes across the Workspace, each with
its source and a revert. A Wake-only Schedule is also an entry, with a
cancel. A no-change review makes no entry, and a change that a Forget took
out of the history keeps none (ADR-0007, ADR-0008).

### Link
A `[[<scope-relative path>]]` target on a memory page that names another
page, in the text or on the `links` line. Each link is an edge of the Page
Index, and a link to a page that does not exist is kept (ADR-0007).

### Memory Search
Keyword search over the path, title and whole text of memory files,
through a derived full-text index for each Storage Backend. The read and
Forget rules apply before a result returns (ADR-0008).

### Observation Time
When Pagis first acquired one Source Version (`observed_at`). It can be
later than what the source describes, and it does not decide which Fact is
true (ADR-0011).

### Open Work
A next step or an open question that a conversation leaves. It goes on the
Conversation Page, so another conversation, a Schedule Run or an Arrival
Run can reach it (ADR-0007).

### Page Index
Derived data about the memory files of a Workspace, as of one memory
revision: each file's last change, title, kind, source, Exposure Stamp,
entity words and Links. The repository is the source of truth, and the
daemon rebuilds the index from it (ADR-0008).

### Page Kind
What one memory file is about, on its `kind` line: Person, Organization,
Event, Transaction, Project, Workstream, Concept, Source or Preference. A
word outside the nine is kept (ADR-0007).

### Page Signals
Facts about one Subject Page that its resource computes from stored
metadata, never from a model, and that the Reflection Filter reads
(ADR-0011).

### Pending Evidence
A durable request for an Agent to reconcile evidence that needs more than
one message or source: its subject, source range, urgency and due time.
The `memory_review` tool records it, and a Review Run settles it
(ADR-0010).

### Private Memory
An Agent's own memory: what it learned, its history, its craft. Only that
Agent reads and writes it (ADR-0007).

### Reflection
The review of selected evidence in its own Run: a source arrival that the
Reflection Filter selects, or due Pending Evidence. It commits a memory
change or records a no-change result. A reply and a Compaction do not
reflect (ADR-0010).

### Reflection Filter
The ordered rule list of one sync resource that says whether a Subject
Page reflects. The first rule whose conditions all hold gives the verdict,
reflect or skip, and a default verdict ends the list (ADR-0011).

### Shared Memory
The Workspace memory every Agent can add to. A note that rests on a
restricted source stays limited to the Agents that may read that source
(ADR-0007).

### Signal Catalogue
What the resource of one Connection offers the filter editor: each signal
with its value kind and options (ADR-0011).

### Source Item
One record from a synced resource, such as a message, a calendar event or a
document, with a source identity and a version (ADR-0011).

### Source Read
The record that a Run read synced source content through a Connection: one
Source Item, or each item of one parent, such as the messages of one mail
thread. A Forget of an item forgets each message of a Run that read it
(ADR-0008).

### Source Version
One immutable acquisition of a Source Item: its identity, version,
operation, source time and Observation Time, with no model interpretation
(ADR-0011).

### Subject Page
One Markdown memory file about one entity or matter: Front Matter,
compiled truth, optional open questions, a Facts Table and the Schedules
above a rule, and an append-only Timeline below it (ADR-0007).

### Supersede
To replace a Fact with a newer one. The Facts Table strikes the replaced
row and names the newer row (ADR-0007).

### Sync
The Person's order to acquire and keep selected data from a resource of a
Connection. Sync reports its coverage (ADR-0011).

### Timeline Entry
The record of one source arrival on a Subject Page, or of one turn that
settled something on a Conversation Page: the source reference, the source
time and the source words. Reflection cannot change it (ADR-0007).

## Connections, credentials and secrets

### Account Creation
One Agent's attempt to register one external account with its own
identity. The daemon records the Credential it mints with its provenance.
Not built: the account-creation record and its events (ADR-0018).

### Connection
A Workspace's link to one account at an external provider, such as Google.
The Person connects once, and Agents use it through Grants and name it by
its alias. Its auth mode is `byo`, where the Person supplies the OAuth
client, or `brokered`, where the Installation OAuth Client does and the
Person gives their own consent. A Connection is `disconnected`,
`connecting`, `connected`, `reauth_required` or `unavailable`, and Agents
get its tools at `connected` only (ADR-0012, ADR-0019).
_Avoid_: integration, brokered credential

### Credential
A saved secret for an external account in the Vault, with its owner, its
provenance (the Person supplied it, or an Agent minted it) and the one
address it is used at. Grants decide which Agent uses it; the model never
sees it (ADR-0013, ADR-0018).

### Installation Connection
A Connection that the Org owns: the carrier and the mail domain. An
Administrator sets it up on the Administration Port, it lives in the Org's
Workspace, and its key is an installation secret. The connection list of
every Person shows it, flagged as the installation's. The Agent Phone
Numbers and Agent Mailboxes on it stay per Workspace (ADR-0012, ADR-0023).
_Avoid_: shared connection, global connection, org connection

### Installation Key
The one key that seals `secrets.enc` and wraps the Tenant Data Key of every
Workspace. A Local Installation keeps it in the keychain on macOS or in the
Secret Service on Linux, or in a Key File where that store does not answer.
A Server reads it from the keychain on macOS or from the Key File on Linux
(ADR-0013).
_Avoid_: safe-storage key, master key

### Installation OAuth Client
The one OAuth client an installation registers with a provider, which an
Administrator sets up and the Org holds. Its presence makes a new
Connection of that provider `brokered` (ADR-0012).
_Avoid_: org client, global client

### Key File
A file that holds the Installation Key: 64 hexadecimal characters, mode
600. A Server on Linux reads it at `/run/secrets/pagis-secrets-key` or the
path in `[secrets] key_file`. A Local Installation reads
`installation-key` in the State Directory first, and generates it where
the keychain or the Secret Service does not answer. The daemon refuses a
file that others can read, and a Backup never holds one (ADR-0013).
_Avoid_: key path, secrets key

### Provider Catalog
The list of providers that Pagis connects: what each one needs, what a
Connection of it gives an Agent, how many Connections a Workspace may
hold, and which installation parts the Administrator sets up (ADR-0012).
_Avoid_: provider list, connector directory

### Tenant Data Key
The key that seals the secrets of one Workspace in the database: Credential
secrets, one-time code seeds and `brokered` refresh tokens. It also derives
the key of the Forget blocks. It is an entry of `secrets.enc` (ADR-0013,
ADR-0008).
_Avoid_: vault data key, workspace key

### VAPID Key
The one P-256 key pair that signs each Web Push of the installation. Its
private half is an entry of `secrets.enc`. Not built (ADR-0030).

### Vault
The store of a Workspace's Credentials. It has no export: a secret leaves
it only into a verified field of its own `https` site, through a browser
channel that the daemon owns (ADR-0013).

## Tools, Software and Plugins

### Capability Manifest
An immutable version of the tools and Incoming Event kinds one source
offers, with the capabilities and approval policy each tool needs
(ADR-0005).

### Capability Snapshot
The immutable set of tools available to one Run, with the manifests and
Grant revisions that made it (ADR-0005).

### Contribution
A proposed change from a Fork to its origin Software Package: a patch, a
summary and a status of open, merged or declined, which the author Agent
decides (ADR-0016).

### Deferred Tool
A tool that the Capability Snapshot holds and the model's tool array does
not. Every Software List tool is one; `tool_search` loads it, and a call of
one that the Run did not load is refused (ADR-0005).

### Effect Class
What a tool call can do to the world, which decides whether it waits for an
approval: `free`, `outbound`, `destructive`, `purchase`,
`credential_release` or `host`. Every class but `free` waits for one
(ADR-0005).
_Avoid_: risk level

### Fork
A Software Package started as a copy of one Version of another, which
records its origin (ADR-0016).

### Loaded Set
The packages one Run loaded with `tool_search`, in load order. It is Run
state, not part of the snapshot (ADR-0005).

### Materialized Copy
The read-only copy of one Version of a Software Package in a calling
Agent's Computer, made at its first call (ADR-0016).

### Plugin
An installed package of Skills and MCP servers in the agent-plugins.org
format. An Administrator installs it for the Org from git or an upload;
each installed state is a commit. Each Workspace uses it with its own
Grants and runs its stdio servers in its own Plugin Computer. An HTTP or
SSE server of a Plugin is an outbound request of the daemon. A Plugin
consumes Connections and secrets through Plugin Bindings and never exposes
one (ADR-0017).

### Plugin Binding
The link from one config field that a Plugin declares to the secret or
Connection that satisfies it, which the Administrator makes at install. A
`connection` binding names an Installation Connection. Not built: the
Grant that lets an Agent use that Connection through a Plugin, so a Plugin
with one does not pass dispatch (ADR-0017).

### Plugin Grant
The Grant that lets one Agent use one Plugin: all of it, or none of it
(ADR-0017).

### Plugin Tool
One tool of an installed Plugin's MCP server, named `<plugin>__<tool>`. A
stdio tool runs in the Workspace's Plugin Computer, never on the daemon
host. An HTTP or SSE tool is a request that the daemon sends. Its Effect
Class is `host` unless the Plugin declared another at install (ADR-0017).

### Sandbox Proxy
The page the daemon serves on the second loopback origin to hold one
Widget in an inner frame at an opaque origin, so a Widget reaches neither
the Product App nor another Widget (ADR-0016).

### Skill
One instruction document that a Plugin ships, with its files. An Agent with
the Plugin Grant sees each Skill's name and loads its body when it needs
it. The Computer Image ships first-party Skills under the name `pagis`
(ADR-0017).

### Software List
The Workspace's catalog of the Software Packages that its Agents built
(ADR-0016).

### Software Package
A directory with a manifest and executables that publishes tools under one
namespace, with one author Agent. Its tools run in the calling Agent's
Computer (ADR-0016).

### Tool
A named action an Agent can call. It comes from Pagis (a core tool), a
Connection, the Software List, a Plugin or a Host. The Agent does not know
where it runs (ADR-0005).

### Version
One immutable published state of a Software Package. Callers use the
latest (ADR-0016).

### Widget
An interactive page that a Software Package ships beside its tools, fed by
one tool result and shown in a sandboxed frame at an opaque origin, with no
persistent browser storage. It needs two loopback origins, so a Product
App on a public name shows its projection instead (ADR-0016).

### Working Copy
The author Agent's editable Software Package in its own Computer
(ADR-0016).

## Computers and Hosts

### Awake Cap
How many Computers may be awake at once, for one Workspace and for the
whole server. The per-Workspace cap counts Agent Computers alone; the
per-server cap also counts Plugin Computers (ADR-0014).

### Computer
The container an Agent owns: a persistent disk, a terminal and a browser,
reached through the screen and `computer_shell`. It joins its Tenant
Network and is the sandbox of what runs in it. A Computer is **Awake**
while its container runs and **Asleep** while it is stopped. Between the
two it is **Downloading** its image or **Starting**, and a Computer that
could not start **Needs attention**. These words say nothing about the
Agent's work, which is its Activity. It stops when it sits idle and when
the daemon stops for good, and its disk stays. A restart of the daemon
keeps it running (ADR-0014).

### Exit Proxy
The HTTP proxy on loopback inside every Computer that its browser and its
shells send each connection to. In Direct mode it dials the connection
from the Computer. In Home mode it sends each connection to the exit
listener of the daemon with the Computer's token, and the daemon carries
it through the Person's Home Exit, or from the server while that Host is
absent; a literal private address still leaves from the Computer. An
Agent's Computer on a Server starts in Home mode when its Person has
chosen a Home Exit and the System Setting lets them use it. When the
choice or the System Setting changes, the daemon switches the mode of each
awake Computer with no restart, and each switch closes the connections
that the proxy holds (ADR-0029).
_Avoid_: egress proxy, outbound proxy

### Home Exit
The one Host of a Person through which that Person's Computers on a Server
reach the internet, so that sites see the Person's own connection and not
a data-center address. The Person chooses it in Settings, knowing its
cost, from their own Hosts that declare `exit`, and the Workspace names
it. It is present while the exit socket of its Client App is open: a
second WebSocket that carries one stream for each connection. Only a Host
of the same Person carries that Person's connections. An Administrator
can turn it off for the installation with a System Setting. The view of
an awake Computer in Home mode and its `computer` tool result name the
exit in use: `exit: <Host name>` while the Home Exit is present, and
`exit: server` while it is absent. A Local Installation has none
(ADR-0029).
_Avoid_: residential proxy, exit node

### Host
One machine of one Person that their Agents can act on: the Client App
running there, with its name, platform, capabilities and last-seen time. A
host action runs there as the Person's own OS user, never in the daemon.
Pagis sandboxes nothing on a Host, so the approval stands in its place, and
the Grant names the machine (ADR-0015).
_Avoid_: device, the user's computer

### Media Relay
How the pixels of a screen reach a browser. A Computer publishes no media
port; a browser sends media to the relay's one advertised address and UDP
port range, and the relay forwards it. The `daemon` relay forwards in the
daemon; the `turn` relay puts an external TURN server in front
(ADR-0014). In Remote Access, the TURN server of the daemon carries the
browser leg of another machine over the Funnel, and it relays to the
Media Relay and to nothing else (ADR-0028).

### Plugin Computer
The one container of a Workspace that runs the stdio servers of its
Plugins: the Computer Image, on the Tenant Network, under the limits of a
Computer, with no screen and no Agent that drives it. All its servers run
as one uid, so each server can read the environment and the data of the
others (ADR-0017).

### Presence
Whether a Host is connected now: the life of its client's socket, held in
the daemon's memory and never stored. A host action on an absent Host is
answered at once (ADR-0015).
_Avoid_: online, heartbeat

### Screen Lease
The exclusive right to send input to an Agent's Computer for one batch of
actions. A Takeover denies it (ADR-0013, ADR-0014).

### Takeover
The Person taking live control of an Agent's Computer until they give it
back or stop using it. The daemon also takes the input to fill a
Credential (ADR-0013, ADR-0014).

### Tenant Network
The one Docker network that every container of one Workspace joins. A
container on it reaches its own Workspace's containers, the public internet
and the Media Relay, and on a Headless Server the exit listener of the
daemon (ADR-0029). On a Headless Server it reaches no container of
another Workspace, no other address of the Docker host, no link-local
address and no private address that the Administrator did not allow
(ADR-0014).

## Telephony

### Agent Phone Number
The telephone number of one Agent: its desk line. An Agent holds at most
one, and holding it is the authority to call. The Workspace owns and pays
for it and can move it to another Agent, unassign it or release it. A
number is unique across the installation, and it never reaches emergency
services (ADR-0018, ADR-0020).

### Call
One telephone conversation between an Agent and a Remote Party. The **Call
Record** holds the direction, the Remote Party, the Trust Tier, the times,
the outcome, the ended reason, the recording and the transcript. The
outcome says whether the Call did its job, and the ended reason says why
the audio stopped (ADR-0020).

### Call Brief
What one Call is for, which the Agent writes, and the tools the Call may
use. An Agent answers its desk line under its **Standing Brief**, a field
of the Agent; with none, it takes a message (ADR-0020).

### Keypad Code
The 6 to 8 digits a caller enters to confirm a Trust Tier above Unknown on
a Call that Pagis answers. Each Workspace holds one code, and it confirms
only calls to the Agents of that Workspace. The daemon collects it from
the keypad, so it never reaches the model or the transcript. After 6
failed attempts across the Workspace, the code is refused for a delay that
starts at 1 minute and doubles with each further failure, up to 24 hours.
A correct code or the Person in Settings clears the count (ADR-0021).

### Listen-Live
The Person hearing a Call while it happens, with no way to speak or act on
it (ADR-0020).

### Media Hub
The audio plane of one Call, which fans the frames of both directions out
to the model, the recorder and the Listen-Live subscribers (ADR-0020).

### Realtime Bridge
The task that joins one Call's Media Hub to a live voice model session
until the Call ends (ADR-0020).

### Remote Party
The person or system at the other end of a Call. Its number picks the
candidate Trust Tier (ADR-0020).

### Session Tool
A tool that exists only inside a live Call: hang up, and send keypad
digits (ADR-0020).

### Trust List
The Workspace's list of who is owner and who is trusted: numbers for Calls
and texts, and addresses or domains for mail. Anyone absent is Unknown
(ADR-0021, ADR-0019).

### Trust Tier
What the words of one Remote Party, mail sender or text sender are worth:
owner words are instructions, trusted words are requests, and Unknown words
are data only. A mail sender's tier holds only on an aligned DMARC pass in
the Mailbox Provider's own authentication result, and a message with more
than one such result is Unknown. A tier gates authority,
never access (ADR-0021, ADR-0019).

## Mail and texting

### Address Ledger
The record of every Agent Mailbox address that Pagis made or assigned,
deleted ones too, so no address is used twice (ADR-0019).

### Agent Mailbox
The email address of one Agent, which it reads and sends from as its own
identity with no Grant. It is on the Org's mail domain, belongs to one
Workspace, and never changes its address (ADR-0019).

### Mail Tool
One of the tools an Agent uses to search, read, send and file mail, in its
own Agent Mailbox or in one of the Person's mail Connections under a Grant
(ADR-0019).

### Mailbox Provider
The Installation Connection through which Agent Mailboxes exist: one
account at a mail host that owns a domain, or a manual host where the
Person makes each mailbox (ADR-0019).

### Messaging Readiness
Whether the carrier delivers a text from an Agent Phone Number: `unknown`,
`not_capable`, `unregistered`, `pending`, `ready` or `rejected`. The type
exists. Not built: reading it from the carrier, showing it, and refusing a
send on it (ADR-0020).

### Outgoing Cap
The most messages one Agent Mailbox or one Agent Phone Number may send in a
day, whatever the host or carrier allows. The daemon refuses a mail past
the mailbox's cap. The number's cap counts texts, which are not built
(ADR-0019, ADR-0020).

### Text Conversation
The texts between one Agent Phone Number and one counterpart number: a
pair, not a stored thing. Not built: where an inbound text lands
(ADR-0020).

### Text Message
One SMS between an Agent Phone Number and a counterpart number. Not built:
sending and receiving (ADR-0020).

### Text Record
The one row that holds one Text Message and, for an outbound one, its Run,
Thread and delivery status. The table exists and nothing writes it
(ADR-0020).

### Text Tool
One of the three core tools to send a text, read one Text Conversation, or
list recent ones. Not built (ADR-0020).

## Messages and UI

### Activity
What one Agent does now, in one word: **Working** while a Run is queued or
running in a Channel, **Needs you** while a Run waits for the Person or for
an approval, **On a call** during a Call, and **Idle** when none of these
is true. A Call comes before a wait, and a wait before work. The Activity
follows the Runs and the Calls, never the Computer. The Activity Ring
draws it (ADR-0022).
_Avoid_: asleep, awake, off (for an Agent)

### Activity Ring
The ring that the Product App draws around an Agent's face while it has a
Run queued or running in a Channel. A Run that reflects turns no ring
(ADR-0022).

### Approval
The kind of Request that asks permission to act: a tool action or a
credential action. The approval card is a view of it. Only an Approval
takes a scope, which is how "Always allow" writes an Allow Rule
(ADR-0004).

### Artifact
A binary file that the Workspace stores outside the database, such as a
screenshot, a call recording or a file an Agent made. A client reads it
only through the daemon, which shows only a passive image, audio, video or
plain-text Artifact inline and serves every other one as a download.

### Automations
The place of the Product App that shows every Schedule and Event
Subscription, with a Needs-You Queue at the top (ADR-0022).

### Block
One typed element of a message. It shows content, or it views a Request
and holds only its identifier and display fields. A client shows a
fallback for a type it does not know (ADR-0004).

### Desk
An Agent's Computer as the Person sees it: the live screen, the site, the
step, and the controls to take over or put it to sleep (ADR-0022).

### Desk Panel
The default tenant of the Inspector on Home and in the Chief of Staff's DM
Channel: every Desk on Home, and the Chief of Staff's Desk in its Channel
(ADR-0022).

### Dictation
The Person speaking a message into a Thread with a button held down. It
makes a draft in the composer, and Pagis keeps no audio (ADR-0020).

### Home
The first place of the Product App: the Needs-You Queue, the Chief of
Staff's Report and the work record of the day (ADR-0022).

### Inspector
The slot beside the conversation. The Desk Panel is its default tenant.
The **Call Inspector** (the live or settled Call) and the **Mail
Inspector** (one message, read live) are transient tenants. Not built: the
**Conversation Inspector** of a Text Conversation (ADR-0022, ADR-0020).
_Avoid_: side panel

### Needs-You Queue
What waits for the Person: approvals, questions, failed Runs, missed
Calls, the Keypad Code delay after too many failed attempts, and the rules
that wait for an approval. The daemon derives it, and Home, the sidebar
count, the app badge and each Notification read it. A failed Run or a
missed Call leaves the queue when the Person acts on it or dismisses it;
the record keeps the time of the dismissal. Not built: the derivation in
the daemon. The UI derives the queue (ADR-0022, ADR-0021, ADR-0030).

### Notification
A Web Push to each Push Subscription of the Person when an item enters the
Needs-You Queue. Not built (ADR-0030).
_Avoid_: alert, push message

### Progress
The daemon's derived line for one Run: what it does now, from the Run
state and the tool call in flight (ADR-0004).

### Push Subscription
The push endpoint and the keys of one client. It belongs to one Session
and ends with it. Not built (ADR-0030).

### Report
The message that the Chief of Staff writes for Home in its DM Channel: what
needs the Person, what is running, what got done (ADR-0022).

### Request
Something a Run needs from the Person before it continues: a tool action, a
credential action, a form, a choice, or a Widget answer. It is pending,
approved, denied, expired or superseded, and it parks the Run (ADR-0004).

### Retention Policy
How long one Artifact class is kept in a Workspace. With no policy a class
is kept for ever (ADR-0020, ADR-0022).

### Rich Message
A message made of blocks, not prose alone. An Agent writes the blocks that
show content; the daemon writes the blocks that view its own records
(ADR-0004).

### Speaking
A Thread reading the Agent's replies aloud, one toggle for each Thread.
Only prose is spoken, and Pagis keeps no audio.

### Strip
The one line that a Call or a mail reads as in its Thread, which the daemon
mints and which opens the Inspector. The **Call Strip** shows the live Call
and settles in place with the outcome. The **Mail Strip** marks the subject
and address as foreign text. Not built: the **Text Strip** (ADR-0022,
ADR-0019, ADR-0020).
_Avoid_: call bar
