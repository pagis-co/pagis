# 0044: A Legal Hold stops each delete in a Workspace

Status: accepted.

## Context

An Administrator needs to preserve the content of a Workspace. Retention
Policies, Forget and explicit deletes must follow the same rule. Google
Vault and Microsoft Purview use a hold to preserve content. A hold does not
remove access to that content.

## Decision

### A Legal Hold preserves a Workspace

An Administrator starts a Legal Hold with a required reason and ends it
through the Administration Interface. The Workspace records the start time
and its author. A hold does not change read access or stop new content.
Only the held Workspace is preserved. A Shared Workspace and a Private
Workspace use the same rule.

Each content delete checks the current hold. A refused delete answers 409
with this message: "This Workspace is under legal hold. Nothing in it can
be deleted until the hold ends." An Agent tool returns the same message as
a tool error. Retention Policies skip held Workspaces before they remove
File blobs. Model Request Capture expiry and the change that turns capture
off keep held captures. Forget cannot begin or continue a purge while the
Workspace is held. A hold does not restore content that a completed delete
removed. Hold changes and external content deletes share a Workspace gate,
so a hold waits for a delete that has started. Database deletes check the
hold in their write transaction.

The boundary includes Files, memory file deletes and reverts, Calendar
Event and occurrence deletes, Workspace Mailbox folder deletion, and Agent
Mailbox deletion. A move to Trash, a folder rename, flags and labels keep
the mail and are permitted. Settings and access records are outside this
boundary: Sessions, Push Subscriptions, Grants, Model Aliases, Trust List
entries, Credentials, Workspace memberships and Connection access can be
removed while the Workspace is held.

Messages between Workspaces belong to the Org Workspace (ADR-0039,
ADR-0043). A message in such a conversation counts as held data while any
participant Workspace is held. Message and conversation delete routes are
not built. Workspace and Person delete routes are not built. Calendar
Agent tools, File delete tools and mail expunge tools are not built.

### The Audit Log belongs to the Org Workspace

Each start and end writes an Audit Entry with an id, the Org Workspace id,
the author, the action, the subject Workspace id, an optional reason and a
time. The hold change and its Audit Entry share one transaction. A repeated
start or end is a conflict and writes no entry. If the entry cannot be
written, the hold does not change.

The Audit Log is append-only. It accepts general actions, not only hold
actions. Only an Administrator lists it, newest first, in pages. Time and
id decide the order. The Audit Log has one owner, the Org Workspace; the
subject Workspace does not own a copy.

### The mail host remains a separate boundary

Mail stays on an external IMAP host. The repository configures Migadu or a
manual mail host; the server deployment runs no mail host and configures no
Trash or Junk purge window. Pagis does not control an external host's own
purge settings. Preservation against a host's automatic purge or a delete
by another mail client is not built. The operator must configure the host
to preserve that mail.

## Consequences

- A hold preserves content without hiding it from its permitted readers.
- One refusal message serves API, storage and Agent tool deletes.
- The Org Audit Log can record other audited actions with the same owner
  and pagination rules.
- Ending a hold permits deletes again. Retention Policies apply their
  configured age window on the next sweep.
