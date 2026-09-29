# 0005: One broker serves tools from a stable Capability Snapshot

Status: accepted.

## Context

A tool can run in the daemon, in a container or on the person's own machine.
Credentials must never reach a container, and Grants need one enforcement
point. Tool definitions are also the first part of the model prompt, so a
change to their names, order, descriptions or schemas defeats prompt caching.

## Decision

The capability broker is the only module that assembles and invokes tools.
The agent loop sees one flat list and does not know where a tool runs. The
broker routes each call to one of four places:

| Where | Which tools |
| --- | --- |
| The daemon | Core tools. Connection tools, with one provider instance for each Connection that serves every Agent it is granted to. The Plugin tools of an HTTP or SSE server (ADR-0017). |
| The Agent's Computer | `computer_shell`, the screen tools and the Software List tools. |
| The Workspace's Plugin Computer | The Plugin tools of a stdio server (ADR-0017). |
| A Host, a client of the person | `host_shell` (ADR-0015). The daemon never runs a host action. |

### Grants

One Grant joins one Agent to one Workspace resource. A host scope holds
command allow rules, and a connection scope holds named capabilities. Each
scope change increments the Grant revision, and the broker checks the live
revision before every dispatch.

A granted capability permits reads and bounded, reversible changes. Outbound
communication, deletion, purchase, credential release and any other
irreversible action need an approval until an allow rule covers them. Pagis
owns this policy. MCP annotations are not authority.

### The tool catalog

The broker builds tool lists from immutable, versioned Capability Manifests.
It starts no server and calls no `tools/list` when it prepares a Run. A
manifest fixes each tool definition, the capability mapping, the effect class,
the approval presentation and the source version. It can also declare
Incoming Event kinds with normalized metadata, typed subscription filters,
required capabilities and a trusted matcher. A `tools/list_changed`
notification proposes a new manifest version and never changes an installed
one.

Core tools keep reserved bare names. Every other tool is
`<namespace>__<tool>`, which fits the 64-character limit and the character set
that the supported model providers share. The manifest owns an immutable
namespace, and the namespace and the full name use ASCII letters, digits,
underscores or hyphens. Installation refuses another format and a duplicate
qualified name. It never adds a numeric suffix and never puts a server, Grant
or Connection id in a name the model sees. The qualified name is the same from
installation through the model call, dispatch, approval and audit; the router
does not translate it for a provider. An Incoming Event kind keeps its dotted
name, because it is not a function name.

At the start of a Run the broker stores one Capability Snapshot from the
Agent's live Grants. Core tools keep their fixed order, and other tools sort
by qualified name. The names, descriptions, schemas and order stay fixed for
the Run. A new or wider Grant affects the next Run; a revocation blocks the
next dispatch from an existing snapshot.

A provider tool takes an optional Connection alias. Where one granted
Connection can handle a call, omission selects it. Where several can, the
broker returns `connection_required` with their aliases and never guesses. The
snapshot freezes the alias resolution, and dispatch still checks the selected
Connection and Grant.

### One registry for each Workspace

One broker serves every Agent of the installation and keeps one manifest
registry for each Workspace. A Software Package that one Workspace publishes
reaches that Workspace alone, and a tool name that one Workspace claims stays
free for the others. Two Workspaces can hold two versions of one package
namespace.

A Plugin is the Org's, at one version (ADR-0017), and every Workspace registry
gets that version's manifest. The Plugin Computer, the Grants and the registry
stay per Workspace, so a Workspace with no Grant has no such tool.

A Workspace registry starts from the manifests that Pagis ships and every
manifest of a source the installation owns (the vault, a Connection provider),
in install order. The order of boot and of a Workspace's first request
therefore cannot change what the Workspace sees, and an installation manifest
reaches every Workspace, including one made later.

A Plugin uninstall revokes every Plugin Grant in every Workspace, and the
broker reads the Plugin Grant live, so the stop applies to every snapshot at
once. A Pagis release is the stop for a shipped manifest.

### Deferred tools and the search gate

Software List tools are deferred. The snapshot holds every Software List tool
at the version resolved at Run start, flagged deferred and content-addressed
with the rest, but the model's tool array holds them only after the Run loads
them. The `tool_search` core tool is the one gate: a query loads every tool of
each matching package, and an empty query lists every package. Loaded tools
follow the fixed prefix of core and Connection tools, in load order, and stay
loaded for the Run. The prefix never changes during a Run, so one search costs
one prompt-cache break.

The loaded set is Run state, which the audited searches reconstruct; the
snapshot does not change. A dispatch of a deferred tool the Run has not loaded
returns `tool_not_loaded`. Connection tools are not deferred. The search runs
in the daemon for every model provider.

### Approvals

Pagis renders the approval card. A trusted manifest supplies the action title,
the display fields, the effect class and an optional allow-rule builder. The
broker validates the arguments and shows them in full under details. The MCP
server cannot supply or change the card.

Every gated tool offers `Approve once` and `Deny`. `Always allow` appears only
where a trusted Pagis adapter proposes and validates the rule, and the card
states what the rule covers. The adapter can propose no rule for a call, such
as a host command that no rule can approve (ADR-0015). A server without such
an adapter stays at approve once. After an approval the broker checks the live
Grant again before dispatch.

### Dispatch and errors

The daemon builds the provider instance of a Connection at its first call and
keeps it. Every call reads the Connection again first, so a deleted or
disconnected Connection stops the next call, and a changed configuration binds
a new instance. A call times out at 60 seconds unless a trusted manifest sets
another limit.

The broker retries a call only where the provider proves that the request
never left, and never after dispatch. A call that can change something and
that crashes, disconnects or times out returns `outcome_unknown`. A read that
times out returns `temporarily_unavailable`.

A failure returns a tool execution error with a stable code:
`temporarily_unavailable`, `reauth_required`, `permission_revoked`,
`connection_required` or `outcome_unknown`. A result never holds a credential,
provider stderr or process details. The Run fails only where Pagis cannot
enforce authority or write its audit event.

A revocation lets a dispatched call finish, because a cancellation cannot
prove that an external effect did not occur. It blocks a call that has not
reached dispatch, including one that waits on an approval.

The broker truncates every tool result on every route at 100,000 characters,
with a marker that names the full size. A tool's own smaller cap stands.

### Manifest upgrades

An upgrade makes a new immutable version. Active Runs keep their snapshot, and
new Runs use the approved version. The user reviews a change to a name, a
schema, an effect, an approval policy or a capability mapping. Grants carry
forward only where the capability mapping and the effect class stay the same.
Old manifests stay for audit, and old processes stay until their active Runs
finish. A removed name gets no alias.

A revoked Plugin Grant blocks dispatch from an existing snapshot at once and
returns `permission_revoked`, at first dispatch and when an Approval resumes.
An Incoming Event declaration follows the same upgrade rule, and an Event
Subscription carries forward only where the event identity, the metadata, the
filter meaning and the required capability stay the same.

### Audit and interface

Each distinct snapshot is immutable and content-addressed. It holds the
normalized tool definitions, the source versions, the Grant ids, the Grant
revisions and the effective scopes. Each Run event records the snapshot id and
hash. Each tool-call event records the qualified name, the source version, the
selected Connection, the Grant revision, the live authorization result, the
approval or allow rule, and the outcome.

```text
prepare_run(workspace_id, agent_id, run_id) -> CapabilitySnapshot

invoke(workspace_id, run_id, snapshot_id, tool_call)
  -> Completed(tool_result) | Waiting(approval_id) | Rejected(tool_result)

resolve(approval_id, decision)
  -> Completed(tool_result) | Rejected(tool_result)
```

`prepare_run` and `invoke` name the Workspace, because the Run, the Grants and
the Requests behind a snapshot all belong to one Workspace, and a caller that
cannot name it does not compile. Behind this interface the broker owns the
Grant checks, the Connection selection, the approval, the allow-rule match, the
routing, the error normalization and the audit writes. A stored snapshot is
shared by every Workspace with the same capability set, and the store offers no
read of it by id. The only read is `for_run(workspace_id, run_id)`.

### Foreign text arrives inside an envelope

Text that reaches a prompt from outside Pagis arrives inside one envelope. It
opens with a line that starts `[BEGIN UNTRUSTED source=...]` and closes with a
line that starts `[END UNTRUSTED`. A content line that holds a marker anywhere
is removed, so nothing inside can end the envelope.

The layer that knows the source applies the envelope, one time. The broker
wraps the result of a route that reaches outside Pagis, as `tool:<name>`. A
tool that carries its own trust value, such as a call transcript, wraps its
own result. A result that carries the daemon's own state or the user's own
answer is not wrapped. One line of the system prompt says what the markers
mean.

### An absent capability is declared, never emulated

Where a provider lacks a capability, its seam declares it absent and the
interface says so. Pagis does not emulate it and offers no tool that can only
fail. A tool an Agent cannot use is absent from its snapshot.

## Consequences

- No Agent Computer and no Plugin Computer holds a credential. Credentials
  stay with the daemon: sealed in the secret file and the database (ADR-0013),
  and in the file keyring of `gog` under the Workspace's home (ADR-0012). The
  one exception is a Plugin Binding, which the daemon puts into the
  environment of a stdio Plugin server, where every stdio server of that
  Plugin Computer can read it, or into a request header to an HTTP Plugin
  server (ADR-0017).
- One enforcement point serves every installation, local or server.
- The agent loop sees tools, not topology.
- Tool lists change only between Runs; a revocation applies before the next
  dispatch.
- Pagis can answer, for every tool action, what the model could call and what
  the broker authorized.
