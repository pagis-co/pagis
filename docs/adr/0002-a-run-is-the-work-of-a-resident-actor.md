# 0002: A Run is the work of a resident actor

Status: accepted.

## Context

An Agent must feel like an employee at a desk: a persistent identity, memory
and computer disk, and a reaction to events while the user is away. The
installation must also cost little when idle, on one machine and on a server
that many people share. A container for each Agent that always runs meets
neither need.

## Decision

### An Agent is a resident actor

Each Agent is a small resident actor inside the daemon. The actor holds the
identity and listens for triggers: messages, Schedules and events. The
container and the model loop start only when the Agent has work. The
container disk persists.

### A Run is the unit of work

A Run is what the actor does while it is awake. It carries the identity, the
Grants, the Capability Snapshot, the Channel and the origin. Its states are
`queued`, `running`, `waiting_for_user`, `waiting_for_approval`, `reflecting`,
and the terminal `completed`, `failed` and `canceled`.

A Run is `reflecting` while it reviews memory, and `running` while it answers
and uses tools. Two kinds of Run reflect: the arrival Run of a synced source,
and the review Run of Pending Evidence (ADR-0010). A reply does not reflect,
also when it commits a memory change or compacts its context, so it goes from
`running` to its terminal state. A review stays inside one Run, with one event
stream, one cancellation path and one recovery path.

### Two slot pools

An Agent has two slot pools. The conversation pool holds the Runs that answer
a message or a Schedule in a Channel, and its cap is three. The arrival pool
holds the reflection-only Runs of synced arrivals, and its cap is one. An
arrival Run never takes a conversation slot, so a long background sync cannot
make a user wait. Inside one pool, a user message starts before a proactive
Run. Job systems separate interactive and background pools for this reason: a
backlog of background jobs must not block a request that a person waits for.

### A proactive Run speaks at the top level of its Channel

A Schedule or an event wakes an Agent with no message to answer. Such a Run
binds no Thread root, so its messages are siblings of the user's messages in
the one conversation the user reads. A Run that wakes inside a Thread keeps
that Thread: a Schedule made from a Thread carries its root, and the Run
answers where it was made.

The transcript of a rootless Run is the conversation of its Channel, so a
fired Schedule reads what the user said since the last wake. A reader that
looks for the last thing an Agent wrote on a Run finds it by the Run, not by a
Thread walk.

## Consequences

- An idle Agent costs kilobytes, so a Workspace holds many Agents.
- The Agent still feels always on: it wakes at once, its files stay where it
  left them, and its history and personality persist.
- A proactive Run that has something to say is visible, and the user answers
  it in place. A quiet wake says nothing.
- A Run does not start the Agent's Computer until it calls a computer tool.
