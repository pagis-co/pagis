# 0023: One Org holds the People of an installation

Status: accepted.

## Context

One installation serves one person on a laptop or a team on a server. Each
person needs a private place for their Agents, memory and accounts. The
installation also has settings nobody owns alone: model keys, the carrier
account, the mail domain, the installed Plugins. A shared bearer token cannot
say which person asks, so it cannot keep one person's records from another.

Team chat products have one organization with administrators and members,
each with private conversations. A self-hosted server has an administrator
who creates the accounts, with no public sign-up.

## Decision

### One Org, and the People in it

One installation is one Org, and every Person belongs to it. Nothing joins two
Orgs or moves a Person between them. Any installation can hold several People.

A Person has a name, a role and one Workspace. The role is `administrator` or
`member` and applies to the whole installation. An Administrator changes what
the Org holds and what other People may do; a Member changes only their own
Workspace. Every Org has at least one Administrator, its first Person: the
seeded Person of a local installation, or the one from server setup
(ADR-0024).

There is no sign-up. An Administrator creates every account in the
Administration Interface, and also disables and restores an account, resets a
password, reads the roster with each last sign-in and each Person's spend, and
sets each Spend Cap. A disabled account signs in to nothing and keeps what it
owns. Disabling and resetting a password both end every Session of that Person
at once. A new Person gets a seeded Workspace with the Administrator's
timezone until their first sign-in (ADR-0006), and lands in the Product App,
not in the local onboarding.

### A Workspace is one Person's private scope

A Workspace belongs to one Person, and no Person reads another's. It holds the
Person's Agents, memory, conversations, Connections, Grants, Vault, Trust List,
Software List, Agent Phone Numbers, Agent Mailboxes, Hosts and Retention
windows. A **tenant** is a Workspace as the unit of isolation: a Tenant Data
Key seals its secrets (ADR-0013), a Tenant Network joins its containers
(ADR-0014), and the broker keeps a manifest registry for it (ADR-0005).

### The Workspace is part of every lookup

A store read or write that names a record also names the Workspace, so the
database refuses another Workspace's record, and a caller that cannot name the
Workspace does not compile. The in-process registries of live records follow
the same rule: the media hubs and tier gates of live Calls are keyed by
Workspace and Call.

The named exceptions are the Workspace store itself; the Org, the Person and
the Session, which sit above a Workspace; the boot sweeps that cross every
Workspace; the Address Ledger, installation-wide so an address is never
reused; and the index of one live Agent Phone Number for each E.164 number.

### The Org's own records live in the Org's Workspace

The Org owns one Workspace that no Person owns: `orgs.workspace_id` names it,
and its `user_id` is null. It holds the installed Plugins with their Bindings
and checkouts, and the Installation Connections: the carrier and the mail
domain. A Workspace row keeps every store call the same and the database
refusal intact, so no Person may reach it by accident:

- `WorkspaceStore::get` and `list` answer only Workspaces a Person owns. No
  per-Workspace sweep, seed or Session reaches the Org's Workspace.
- The seed never runs for it. It has no Agents, Channels, aliases or memory.
- It holds no `brokered` Connection and needs no Tenant Data Key; the keys of
  an Installation Connection are installation secrets.

A Person-scoped path names the Org's Workspace in one case: to read an
Installation Connection. The number desk, the mailbox desk, the endpoint
tasks, inbound calls and standing rules read the carrier and the mail domain
from there.

### What belongs to the Org, and what belongs to a Workspace

A setting a Person tunes for their own Agents is theirs. A setting that decides
what the installation reaches or spends is the Org's, and an Administrator
alone reads or writes it, on the Administration Port (ADR-0024).

| The Org's | One Workspace's |
| --- | --- |
| The provider keys, one for each model provider | The Model Aliases each Agent thinks on |
| The Installation OAuth Client | The Person's own Connections, their Google account above all |
| The Installation Connections: the carrier and the mail domain | The Agent Phone Numbers and Agent Mailboxes on them |
| The installed Plugins and their Bindings | Which Agent holds a Grant to a Plugin |
| The System Settings: the ports, the Docker endpoint, the log level, the data directory | The timezone |
| The roster, each role, each Spend Cap | The Agents, the memory, the conversations, the files, the Vault, the Software List |
| The Awake Cap on Computers | The Retention windows |

A Person holds and reads no provider key. The Administrator supplies one key
for each provider, and each Person chooses what to think with.

A Spend Cap is what one Person may spend on model calls in a calendar month of
their Workspace timezone, in US dollars. A Person with no cap is never
stopped. A Run that would start over the cap stops before any model call, and
the conversation says so in one sentence.

### The Session is the only way in

A Person signs in before any client reads or writes their data. A Session
names the Person and the client kind (`browser` or `desktop`), records when it
was made and last used, ends at sign-out, and expires 30 days after its last
use (ADR-0028). It grants nothing by itself; the role and the Workspace decide.

The Session travels in an HTTP-only cookie with no `Domain` (host-only) and
`SameSite=Strict`, so the page cannot read it and it never appears in an
address. The record keeps only a hash of the value. The sockets read the same
cookie, and their first frame carries no credential. A listener refuses a
socket upgrade from an origin it does not serve (ADR-0024), because
`SameSite=Strict` still sends the cookie from a same-site sibling.

A password sign-in, the trade of a Client Credential, and a Sign-In Link
(ADR-0025, ADR-0028) hand out a Session. A refused password counts against the
account and against the source address. A second device of a Person signs in
with a Sign-In Link that a signed-in client or `pagis pair` makes.

## Consequences

- One installation shape serves one person and a team.
- A bug that reads another Workspace's row is refused by the database, and a
  secret read across the tenant line stays ciphertext.
- An Installation Connection is one record and one secret, so a key rotation is
  one write.
