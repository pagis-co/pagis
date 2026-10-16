# 0043: Messages between Workspaces follow Disclosure Rules

Status: accepted.

## Context

A Person needs to ask a colleague without making a Shared Workspace.
Chat systems keep one conversation and check participants at access time.
An assistant also needs a rule for what it can disclose in a conversation.

## Decision

### A conversation has Workspaces as participants

A Channel of kind `conversation` has Workspaces as participants, beside
Person and Agent participant kinds. The conversation and its messages live
once in the Org's Workspace (ADR-0023). They are never copied into the
participant Workspaces. This extends ADR-0003 and follows ADR-0039.

A Person lists the other enabled Persons of the Org in People. Each entry
names the colleague and their Private Workspace. The Person selects one or
more colleagues to start a conversation. The same set of Workspaces opens
the same conversation, whatever their order. The store creates the Channel
and its participants in one transaction, with a unique set key.

Every conversation route checks that the Session's Private Workspace is a
participant before it reads or writes content. A non-participant gets 404.
The Session gives no access to other records in the Org's Workspace.

### Delivery follows client activity

A Person is active while a visible client had input in the last 120 s.
Presence reads `PersonActivity::hold_until`, the same function and state
that hold Notifications (ADR-0030). A ping or a query does not count.

A new message publishes a live event to each participant Workspace through
the existing event bus. The event carries the conversation and message ids,
with no copy of the message content. Each client reads the one stored
history. Each message records its Person author, and each reply shows that
Person's name. A message starts no Agent Run.

Not built: Notifications for an away Person. The Notification path takes
Needs-You items and checks that queue again before each send. Conversation
messages do not enter that queue.

Not built: the Chief of Staff answering while the Person is away, Delegate
everything, For the person only, Agents messaging colleagues, group answer
rules, and Delegation between Workspaces. An Agent reply needs the name
`<Agent>, for <Person>`. Colleagues' words give no authority to configure an
Agent.

### A Disclosure Rule decides what the Chief of Staff can reveal

Not built: Disclosure Rules and their default disclosures. These rules
must limit disclosure to free and busy times, the Person's profile and
Files or Artifacts already shared with the asker. Other disclosure needs
an Approval. An approved disclosure can become a standing rule. A request
from a colleague gives no right to read private mail, memory or event
details. Artifacts are not built.

### Presence has no stored copy

Active and away come from client activity in the Private Workspace. The
conversation reads that state without a stored copy. Host Presence keeps
its separate meaning (ADR-0015).

The server reads the Private Workspace Calendar at the current time. A busy
interval gives the in-a-meeting state, which takes priority over active
client input. It stores no copy of this state (ADR-0042).

## Consequences

- Each participant reads one conversation and one history.
- Each reply shows its Person author.
- Client activity and Notifications use one rule.
- A group message starts no Run.
