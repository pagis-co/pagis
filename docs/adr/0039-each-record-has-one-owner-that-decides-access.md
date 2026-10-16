# 0039: Each record has one owner that decides access

Status: accepted.

## Context

A Person needs a private scope for Agents, memory and personal accounts. A
team needs a shared scope for the same work. One Person for each Workspace
cannot express both. Copying records to each reader splits history and makes
access removal hard.

Established team products separate Org roles from roles in a shared scope.
Access follows the scope that owns the record. Shared content keeps one
history, with an author for each change.

The daemon gives each Person one Private Workspace. A Shared Workspace has
a membership list with the roles Manager, Responder and Reader.
Not built: guests and offboarding. This decision
replaces the single-Person Workspace rules of ADR-0023 and the Channel read
rules of ADR-0003.

## Decision

### Each record has exactly one owner

Each record has exactly one owning Workspace or conversation. Its owner
decides access. A conversation between Workspaces lives once outside its
participant Workspaces, in the Org's Workspace (ADR-0043). It is not copied
to each reader. A conversation route checks that the Session's Private
Workspace is a participant before it reads or writes content.

Each record names its author, a Person or Agent. The daemon names itself
on its own records. Messages, Request decisions,
Credentials, Grants, Schedules, Calendar Events and Event Subscriptions record
who makes them.
Each change keeps its author. A Shared Workspace Agent change is versioned
and audited. The Briefing names the Person who asks and the Persons who are
present. An Agent introduces itself with its name and the Shared Workspace
name when it acts for a Shared Workspace: "Aria, from the Acme Support team".
The self-introduction rule is fixed. A personality or message cannot turn
it off. The Agent says it is an AI assistant when asked and never claims
to be human. A Private Workspace keeps the Person's name in the rule.
The Briefing names the triggering Person and role. Present members are
Person participants of the Channel. Client activity is tracked per
Person across all Workspaces and does not supply a connected member roster.

Authorship on messages, Request decisions, Credentials, Grants, Schedules,
Calendar Events and Event Subscriptions is built. Conversation messages live once under their
Channel and record their Person or Agent author. Files are built.
Shared Workspace Agent versioning and auditing are built (ADR-0047).
Not built: collaborative Artifacts, which are documents, sheets, slide decks or sites. An
Artifact belongs to one Workspace, can be stored apart from it, and is shared
by an access grant, never by a copy. Text from outside its Workspace is
untrusted input to Agents.

### Private and Shared Workspaces have separate access rules

Each Person has exactly one Private Workspace, made with the Person. It has
exactly one Person and no membership list. It never gains another Person.

A Shared Workspace has no owner. The product calls it a team.
Persons in it have one of three roles:

| Role | Access |
| --- | --- |
| Reader | Read within the visibility level |
| Responder | Read, send as the Shared Workspace and decide Requests |
| Manager | Responder access; manage People, roles, Agents and settings |

An Administrator creates a Shared Workspace and names its first Manager.
The Administrator sets the Spend Cap.
Shared Workspace Limits name permitted Plugins and Connections and the
highest Mail Visibility (ADR-0047). A Manager works inside those limits. Member
stays an Org role. It does not name a Shared Workspace role.

The Team trust tier replaces Owner for Persons in a Shared Workspace.
Managers' words are instructions. Responders' and Readers' words are
requests. Each member message has a Team envelope with the current role
and authority. Marker lookalikes in its body are removed. A request cannot
change the Agent's job, Standing Brief or rules. The Trust List shows Team
identities from membership. A Manager cannot add an external Owner identity
to a Shared Workspace. A tier decides authority, not access (ADR-0021).

The Org's Workspace stays the scope for Installation Connections and installed Plugins
(ADR-0023).
Not built: its use as an Org-wide Shared Workspace with every
Person in it.

### A Session stays with the Person's Private Workspace

A Session belongs to the Person and stays bound to their Private Workspace.
Private Workspace routes have no Workspace id. A Shared Workspace request
names it in `/api/v1/workspaces/:id/...`. The server checks the Session, then
the role at the entrance of each Shared Workspace route. Only a route with a
finer rule adds its own check.

The Product App lists the Private Workspace and each Shared Workspace the
Person can enter. Query keys and routes name the Workspace. One sign-in
serves them all.
A non-member gets 404, as for an unknown Workspace. A member with too little
access gets 403. A Reader uses GET. A Responder also sends messages and
decides Requests. A Manager makes all other changes. A Manager creates, changes and archives
Agents and chooses the Chief of Staff. The Product App uses the role in the
Workspaces list to show these controls on desktop and phone layouts.
The role for each
method is in the route table. The same handlers serve both route prefixes.

### Personal resources stay private and spend follows the Workspace

Hosts, the Home Exit, personal Connections and the Keypad Code exist only in
a Private Workspace. Shared accounts connect to a Shared Workspace. The
Spend Cap belongs to a Workspace. A Run checks the cap of the Workspace it
runs in. An Administrator sets the cap for each Workspace. Spend is counted
for each Workspace.
A request for a personal resource in a Shared Workspace answers 409 with
the Private Workspace rule. Host registration answers the same conflict on
the socket. The Hosts list is empty in a Shared Workspace.

Google sign-in in a Private Workspace connects the Person's personal account.
A Manager explicitly selects a shared Google account for a Shared Workspace.
The Connection records that choice and belongs to the Shared Workspace,
never to the Manager. Personal Google Connections are refused there. OAuth
state names the Workspace; authorization, the start route and the callback
check the Manager role. Agents use the team account through Connection Grants.

Push Subscriptions and client activity are keyed by Person. Each member's
Notification has its own hold. A member's activity holds no other member's
Notification.

### Departure and guests do not open a Private Workspace

On departure, the Org owns all content of the Private Workspace. An
Administrator names a successor. The daemon removes Hosts, the Home Exit,
the Keypad Code and personal Connection tokens before it changes the Private
Workspace into a Shared Workspace. The successor is its Manager. The
Workspace Mailbox and Workspace Number stay with it.

A guest is a login with the Org role guest. It has no Private Workspace. A
Manager names each Shared Workspace the guest can enter. A new Person gets a
Private Workspace, Chief of Staff, Workspace Mailbox by the Org address rule,
and access to the Org's default Shared Workspaces. A Workspace Number is
made only on request.
Joining uses the Org Address Rule and default Shared Workspace roles
(ADR-0048). Not built: departure and guest rules.

## Consequences

- Org roles and Shared Workspace roles decide different access.
- Access removal changes one owning scope, without copies to remove.
- A departing Person's work continues without their personal resources.
- The ownership rule also serves collaborative Artifacts, which are not built.
