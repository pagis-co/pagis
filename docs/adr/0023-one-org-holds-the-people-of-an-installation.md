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

A Person has a name, an Org role and exactly one Private Workspace. The role
is `administrator` or `member` and applies to the whole installation. An
Administrator changes what the Org holds and what other People may do; a
Member changes their own Private Workspace. A Member also
enters Shared Workspaces through the separate roles Manager, Responder and
Reader (ADR-0039). Every Org has at least one Administrator, its first Person:
the seeded Person of a Local Installation, or the one from Server Setup
(ADR-0024). An Administrator changes a Person's role in the Administration
Interface. A role change keeps at least one enabled Administrator in the Org.
An Administrator can demote themselves when another enabled Administrator
exists. Each role change adds an audit event in the Org's Workspace, with the
Administrator, the Person and both roles.

There is no sign-up. An Administrator creates every account in the
Administration Interface, and also disables and restores an account, resets a
password, reads the roster with each last sign-in, reads each Workspace's
spend, and sets each Workspace's Spend Cap. A disabled account signs in to
nothing and keeps what it owns. Disabling and resetting a password both end
every Session of that Person at once. A new Person gets a seeded Workspace
with the Administrator's timezone until their first sign-in (ADR-0006), and
opens Onboarding in the Product App. The server names the steps: Welcome,
Model, Profile, and Computer when it has Computers. A Person chooses from the
models the installation's keys reach, enters their name and timezone, and can
rename the Chief of Staff. A Person on a Server enters no provider key.
Completion marks their Workspace as onboarded.

### Private and Shared Workspaces have different access rules

A Private Workspace has exactly one Person and never gains others. A Shared
Workspace has no owner. Persons in it have the roles Manager,
Responder or Reader. Each record has one owning Workspace or conversation,
and its owner decides access (ADR-0039). The daemon gives each Person one
Private Workspace. An Administrator creates a Shared Workspace and names
its first Manager. A Manager changes its membership and roles.

A Workspace holds Agents, memory, conversations, Connections, Grants, Vault,
Trust List, Software List, Agent Mailboxes, a Workspace Mailbox, a Workspace
Number, Files and Retention Policies (ADR-0040, ADR-0041). Not built: its
calendar (ADR-0042). A **tenant** is a Workspace as the unit of isolation: a
Tenant Data Key seals its secrets (ADR-0013), a Tenant Network joins its
containers (ADR-0014), and the broker keeps its manifest registry (ADR-0005).

### The Workspace is part of every lookup

A store read or write that names a record also names the Workspace, so the
database refuses another Workspace's record, and a caller that cannot name the
Workspace does not compile. The in-process registries of live records follow
the same rule: the media hubs and tier gates of live Calls are keyed by
Workspace and Call.

The named exceptions are the Workspace store itself; the Org, the Person and
the Session, which sit above a Workspace; the boot sweeps that cross every
Workspace; the Address Ledger, installation-wide so an address is never
reused; and the index of one active Workspace Number for each E.164 number.
Conversation routes read the Org's Workspace after they check that the
Session's Private Workspace is a participant (ADR-0039, ADR-0043).

### The Org's own records live in the Org's Workspace

The Org owns one Workspace that no Person owns: `orgs.workspace_id` names it,
and its `user_id` is null. It holds the installed Plugins with their Bindings
and checkouts, and the Installation Connections: the carrier and the mail
domain. It also holds conversations between Workspaces, each stored once
with its messages (ADR-0043). A Workspace row keeps every store call the
same and the database refusal intact, so no Person reaches it by accident:

- `WorkspaceStore::get` and `list` answer Private and Shared Workspaces. No
  per-Workspace sweep or seed reaches the Org's Workspace.
- The seed never runs for it. It has no Agents, aliases or memory.
- It holds no Google Connection and needs no Tenant Data Key; the keys of
  an Installation Connection are installation secrets.

A Person-scoped path names the Org's Workspace to read an Installation
Connection, or to read and write a conversation whose participants include
the Person's Private Workspace. The number desk, the mailbox desk, the
endpoint tasks, inbound calls and standing rules read the carrier and the
mail domain from there.

### What belongs to the Org, and what belongs to a Workspace

A setting a Person tunes for their own Agents is theirs. A setting that decides
what the installation reaches or spends is the Org's, and an Administrator
alone reads or writes it, on the Administration Port (ADR-0024).

| The Org's | One Workspace's |
| --- | --- |
| Model provider keys | Model Aliases |
| Installation OAuth Client | Personal Connections |
| Installation Connections | Mailboxes and the Workspace Number |
| Installed Plugins and Bindings | Agent Grants to Plugins |
| System Settings | Timezone |
| People and Org roles | Spend Cap |
| | Agents, memory, Channels, Files, Vault and Software List |
| Awake Cap | Retention windows |
| Home Exit System Setting | The Person's Home Exit Host |

A Person holds and reads no provider key. The Administrator supplies one key
for each provider, and each Person chooses what to think with.

A Spend Cap belongs to a Workspace. It is the most it may spend on model calls
in a calendar month of its timezone, in US dollars (ADR-0039). An
Administrator sets it for each Workspace. The daemon counts spend for each
Workspace. No cap means no stop. A Run checks the Spend Cap
before each model call, in every Turn. It stops before the call that would go
over the cap, or use a model with no known price. It ends `failed` with
`SpendCapReached`. Its earlier replies stay in the conversation, and a note
says why it stopped. The Harness Model Endpoint makes the same check before
each model request of a Coding Session.

Each model call of a Run goes through one function that prepares the
request and checks its budget before it asks the Embedder's
`ModelCallGate`. The gate receives the Run and a `ModelCallBound`: the
model candidates, the input token bound that `model.requested` records,
and the output token limit that the request sends. `AgentDeps` holds the
gate. The daemon uses
`SpendCapGate`, which reads the current Spend Cap and Usage Records with
`cap_stop`. A test or another Embedder supplies its own gate. A gate stop
carries the conversation note and the Run's `FailureKind`.

Each permitted call gives the gate one `after_call` outcome: the provider
usage and the router's cost estimate in USD, or the failure. Cancellation
is a failure. Missing usage or prices stay unknown. The Run uses the same
cost for its Usage Record and its usage events. A call that the gate stops
has no `after_call`. The method has an empty default body; the Spend Cap
gate uses only the model candidates in the bound.

### The Session is the only way in

A Person signs in before any client reads or writes their data. A Session
names the Person and the client kind (`browser` or `desktop`), records when it
was made and last used, ends at sign-out, and expires 30 days after its last
use (ADR-0028). It grants nothing by itself; the role and the Workspace decide.
It stays bound to the Person's Private Workspace. Shared Workspace
requests name `/api/v1/workspaces/:id/...`. The server checks the Session,
then the role at the entrance of each Shared Workspace route (ADR-0039).

The Session travels in an HTTP-only cookie with no `Domain` (host-only) and
`SameSite=Strict`, so the page cannot read it and it never appears in an
address. The record keeps only a hash of the value. The sockets read the same
cookie, and their first frame carries no credential. A listener refuses a
socket upgrade from an origin it does not serve (ADR-0024), because
`SameSite=Strict` still sends the cookie from a same-site sibling.

A password sign-in, the trade of a Client Credential, and a Sign-In Link
(ADR-0025, ADR-0028) hand out a Session. A refused password counts against the
account and against the source address. Another browser or app of a
Person signs in with a Sign-In Link that a signed-in client or `pagis pair`
makes.

## Consequences

- One installation shape serves one person and a team.
- A bug that reads another Workspace's row is refused by the database, and a
  secret read across the tenant line stays ciphertext.
- An Installation Connection is one record and one secret, so a key rotation is
  one write.
