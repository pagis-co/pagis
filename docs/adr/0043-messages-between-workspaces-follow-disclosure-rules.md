# 0043: Messages between Workspaces follow Disclosure Rules

Status: accepted.

## Context

A Person needs to ask a colleague without making a Shared Workspace. An
Agent can answer while its Person is away, but it must not reveal private
mail or memory to a colleague. Group conversations also need one clear
answer path so every Agent does not answer each message.

Chat systems keep one conversation and check participants at access time.
Assistants need a separate rule for what they can disclose in that
conversation. Not built: messages between Workspaces, Disclosure Rules and
Person presence from a Private Workspace and calendar.

## Decision

### A conversation has Workspaces as participants

A Channel can have Workspaces as participants. Its conversation is stored
once outside the participant Workspaces. Each participant Workspace reads
it there, under the conversation's access rules. It is not copied into the
Workspaces. This extends the Channel model of ADR-0003 and follows the
ownership rule of ADR-0039. Not built: this conversation model.

### Delivery follows activity and the Person's rules

A Person is active when a client has input in the last 120 s. Delivery goes
to that Person while active. Otherwise the Chief of Staff answers within
the Person's rules. Delegate everything sends the work to the Chief of
Staff even while the Person is active. For the person only lets only the
human answer. Not built: these delivery choices.

A reply shows the Person or `<Agent>, for <Person>`. In a group, a Chief of
Staff answers only when its Person is named or asked. A Person's Agent can
message colleagues for them. Colleagues and their Agents are trusted
senders: their words are requests, not instructions. The Delegation hop cap
applies across Workspaces (ADR-0003). Not built: these sender, group and
Delegation rules between Workspaces.

### A Disclosure Rule decides what the Chief of Staff can reveal

A Disclosure Rule says what the Chief of Staff can reveal to whom. Defaults
share free/busy, the Person's profile and Files or Artifacts already shared
with the asker. Anything else needs an Approval. The Person can make an
approved disclosure a standing rule. A request from a colleague gives no
right to read private mail, memory or event details. Artifacts are not built.
Not built: Disclosure Rules and these default disclosures.

### Presence has one source

Active, away and in a meeting come from the Private Workspace and calendar.
The conversation reads that state without another stored copy. Host Presence
keeps its separate meaning (ADR-0015). Not built: Person presence from the
Private Workspace and calendar.

## Consequences

- Each participant reads one conversation and one history.
- Replies show whether a Person or an Agent speaks.
- Activity controls delivery; Disclosure Rules control the answer's content.
- A group question does not wake every participant's Chief of Staff.
