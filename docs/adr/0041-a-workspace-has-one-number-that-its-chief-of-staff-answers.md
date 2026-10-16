# 0041: A Workspace has one number that its Chief of Staff answers

Status: accepted.

## Context

A caller wants to reach a Person or team. A number for each Agent makes the
caller choose the helper before the conversation starts. A shared front
desk answers one line and sends the caller to the right helper. A callback
needs to reach the helper who made the call.

Not built: Workspace Number. The daemon assigns each number to one Agent
(ADR-0020). This decision supersedes the per-Agent number ownership,
assignment, authority and routing parts of ADR-0018, ADR-0020 and ADR-0021.
The SIP and RTP transport of ADR-0020 stays in place.

## Decision

### A Workspace holds at most one Workspace Number

A Workspace Number belongs to its Workspace. It is optional and is made on
request. Private and Shared Workspaces use the same number model. The Org's
carrier Connection serves them (ADR-0023). Text Conversations and Text
Messages belong to the Workspace Number, not an Agent's identity.
Not built: Workspace Number and texts through it.

Per-Agent phone numbers are removed with no compatibility path, fallback,
migration or data move. Not built: this removal. The daemon assigns each
number to one Agent.

### The Chief of Staff answers and can hand off a Call

An inbound Call goes to the Chief of Staff. It can hand the Call to another
Agent of the Workspace. The Chief of Staff gets the Workspace Number
through a default Grant, not through its designation (ADR-0022).
Not built: default number Grants, Chief of Staff routing and Call handoff.

### Outbound Calls need a Grant and callbacks keep their Agent

An Agent with a Grant calls out from the Workspace Number. The number alone
gives no authority to call. A record of recent outbound Calls routes a
callback from the Remote Party to the Agent that called. With no such
record, the Chief of Staff answers. Not built: number Grants and callback
routing.

### Trust remains separate from access

The Trust Tier gates the Remote Party's authority (ADR-0021). The Keypad
Code exists only in a Private Workspace (ADR-0039). A Shared Workspace has
no Keypad Code. Number routing does not grant access or raise trust.
Not built: these Workspace kinds and their number access rules.

## Consequences

- A caller reaches a Workspace without choosing an Agent.
- A Grant decides which Agent can call out from the shared identity.
- A callback can continue with the Agent who made the outbound Call.
- The carrier transport stays separate from number ownership and routing.
