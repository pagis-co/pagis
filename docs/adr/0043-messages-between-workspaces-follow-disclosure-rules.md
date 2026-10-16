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
history. Each message records its Person or Agent author. An Agent reply
also records its author's Private Workspace and shows `<Agent>, for <Person>`.

Not built: Notifications for an away Person. The Notification path takes
Needs-You items and checks that queue again before each send. Conversation
messages do not enter that queue.

For a conversation between two Workspaces, an active Person receives the
message directly. Otherwise the Chief of Staff answers within the Person's
rules. Delegate everything is a Private Workspace setting that selects the
Chief of Staff even while the Person is active. Delivery uses client input,
not the calendar's in-a-meeting display state.

For the person only is a stored message mark. The Chief of Staff can write
a context note in its DM Channel with the Person, but cannot reply into the
shared conversation for that message. The Person can answer there.

A conversation Run belongs to the recipient's Private Workspace. Its trigger
holds a reference to the one shared message. The model reads shared history
through that reference. The reply goes into the shared conversation, or into
its DM Channel with the Person for a context note. It has no second message copy.
The Run uses the Agent's existing job, Standing Brief, rules and Grants.
Runs in one shared Channel execute one at a time. Input in the Person's
DM Channel starts a separate Run. A resumed conversation Run does not read
private DM Channel input.

The Colleague Trust Tier covers colleagues and their Agents. Their words are
requests, never instructions. The daemon supplies the tier in an envelope
that the message body cannot end, in the same pattern as Team. A request
cannot change the Agent's rules or give it authority.

An Agent reply continues the sending Run's Delegation hop count. The existing
hop cap stops delivery before a new Run enters the queue. There is no second
counter for Workspaces. The stored author Workspace also selects the sending
Run and the live Grants for its Exposure Stamp.

An Agent starts or continues a conversation with `message_colleagues`.
`colleagues_list` supplies enabled colleagues and their Private Workspace ids.
The tool uses the same conversation for the same set of Workspaces. It writes
an Agent message with its author Workspace, Run and Exposure Stamp.

The colleague Grant belongs to the sending Agent's Workspace. Its Allow Rules
name recipient Workspace ids. A send needs an Approval unless every recipient
is covered. Always writes those ids onto the Grant. The broker checks the
live Grant before dispatch and when a parked call resumes. A revoked Grant
blocks a call from a snapshot that holds it.

The Delegation origin crosses Workspaces with the hop count. A reply wakes
the asking Agent, even while its Person is active. A Person's answer uses the
last Agent request from the asking Workspace to keep that origin. A message
from the asking Person ends that delegation. An Agent's final shared reply
starts no new request. That Run reads the shared
conversation and reports back in the Channel and Thread where it owes an
answer. It sends no automatic reply to the colleague. Each Run checks its own
Workspace's Spend Cap and records its Usage Records there.

Not built: group answer rules.

### A Disclosure Rule decides what the Chief of Staff can reveal

The Person writes a profile and manages Disclosure Rules in Settings.
A rule names one asker Workspace, or every Workspace of the Org, and one
category: memory, mail, calendar details or other private data. Both storage
backends keep profiles and rules under their owning Workspace.

The defaults share free and busy times, the Person's profile and Files
attached to a message in the shared Channel. The `file_read` tool reads
these Files without a Disclosure Rule. Other File reads need permission.
Collaborative Artifacts are not built.
A request from a colleague gives no right to read private mail, memory or
Event details. The broker checks disclosure before a private tool reads.
Each participant Workspace other than the answering Workspace is an asker.
Every asker must have a matching rule. Otherwise the broker parks the call
on an Approval to the Person. The Approval names the category and every
asker without a rule. Always stores one rule for each of these askers before
the call resumes. The call still needs its Grants and any action Approval.
Unknown tools also need disclosure permission, because a shell, Plugin or
Coding Session can read private data.

A colleague Run reads shared conversation history and the profile. It does
not load private memory indexes, the Brief or Skill notes into its prompt.
It obtains private data only through the checked tool path. Model
instructions do not decide access.

`conversation_read` and `conversation_search` read the shared Channel by
default. A reference to a private message cannot change that scope. A read
of the Run's private Channel needs Memory permission. Other Channel scopes
are not supported. `conversation_page` reads and writes the private Subject
Page and needs Memory permission.

A parked call stores its resume arguments and resolved action state. The
Run's immutable Capability Snapshot supplies the tool and Grant binding at
resume. The request API hides resume data. A disclosure Approval shows the
tool, category and askers, with no private tool arguments.

### Presence has no stored copy

Active and away come from client activity kept for each Person across all
Workspaces. The conversation reads that state without a stored copy. Host Presence keeps
its separate meaning (ADR-0015).

The server reads the Private Workspace Calendar at the current time. A busy
interval gives the in-a-meeting state, which takes priority over active
client input. It stores no copy of this state (ADR-0042).

## Consequences

- Each participant reads one conversation and one history.
- Each reply shows its Person or Agent author.
- Client activity and Notifications use one rule.
- A group message starts no Run.
