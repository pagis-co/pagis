# 0041: A Workspace has one number that its Chief of Staff answers

Status: accepted.

## Context

A caller wants to reach a Person or team. A shared front desk answers one
line and sends the caller to the right helper. This decision supersedes the
number ownership, authority, routing and text ownership rules of ADR-0018,
ADR-0020 and ADR-0021. The SIP and RTP transport of ADR-0020 stays in place.

## Decision

### A Workspace holds at most one Workspace Number

A Workspace Number belongs to its Workspace. It is optional. The Person buys
it in Settings, sees its carrier registration and releases it there. The
Org's carrier Connection serves every Workspace (ADR-0023). A unique index
permits at most one active number in each Workspace. A released record stays
for its Calls and Text Records. Number ownership has no Agent field.

Text Conversations and Text Messages belong to the Workspace Number. A Text
Record has no Agent field. The Run names the Agent that handles the message.

### The Chief of Staff answers and can hand off a Call

An inbound Call with no recent callback record reads the Workspace's Chief
of Staff designation at Call time (ADR-0022). A Workspace with no active Chief of Staff refuses the Call
before answer. Its Call Brief uses that Agent's Standing Brief. A change of
Chief of Staff changes the Agent that answers the next Call. No brief or
rule is copied to the new Agent.

The Chief of Staff can use `hand_off_call(agent)` during a Call with a
confirmed Trust Tier. The target must be an active Agent of the same
Workspace. The Call record names the new Agent and its new Run. A new model
session uses its voice and instructions. The Remote Party keeps the same
carrier connection. The Trust Tier stays in force.

Inbound texts are stored as Text Records and wake the Chief of Staff through
a Standing Text Rule. The sender's number supplies the Trust Tier. The words
reach the Run inside the untrusted envelope.

### Outbound Calls and texts need a Grant

Outbound Calls need a live Workspace Number Grant. Its resource is the
active Workspace Number of the same Workspace, and its scope is empty.
The Agent Access tab creates and revokes this Grant. The broker offers
`phone_call` only to an Agent with the Grant and checks it before dispatch.
The tool checks the Grant again before it places the Call. Emergency numbers
are refused. The designation grants no outbound authority.

A callback within seven days reaches the active Agent of the latest outbound
Call from this Workspace Number to the caller's number. With no such Call,
or when that Agent is archived, the Chief of Staff answers. The Call store
supplies the record; routing does not change the Trust Tier.

Not built: outbound texts, default number Grants and the Chief of Staff's
default Grant.

### Trust remains separate from access

The Trust Tier gates the Remote Party's authority (ADR-0021). The Keypad
Code confirms an inbound tier above Unknown. Number routing and hand-off do
not raise trust. Not built: Private and Shared Workspace kinds and their
separate number access rules (ADR-0039). Shared Workspace number routes are
not built. Number handlers use the request's tenant Workspace.

## Consequences

- A caller reaches a Workspace without choosing an Agent.
- The Call holder changes without a new carrier connection.
- The carrier transport stays separate from number ownership and routing.
