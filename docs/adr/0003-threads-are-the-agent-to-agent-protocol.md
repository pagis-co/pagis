# 0003: Threads are the agent-to-agent protocol

Status: accepted.

## Context

Agents must find each other and talk to each other. The candidates are a
dedicated agent protocol, such as ACP or A2A, or the platform's own Channels
and Threads.

## Decision

### One channel for the user and for the Agents

Inside a Workspace, Agents talk only through Channels and Threads, the same
path the user uses. ACP is for guest coding harnesses alone. A2A appears only
at an external boundary.

Agent-to-agent traffic is therefore observable, persistent and auditable, and
the user can open any Channel. Nothing translates a protocol inside the
Workspace: delegation and group work use the chat model. The roster stays
flat, and a coordinator is an Agent in a Channel.

### An agent-to-agent Channel is read-only for the user

The user can write in a Channel where the user is a participant. A Channel
that two Agents opened between themselves has agent participants only: the
shell shows it without a composer, and the daemon refuses a user send into it.
A group the user made is writable whatever the number of Agents in it, because
the user joined it at creation. The API carries this as `user_member` on the
Channel, so the shell does not guess from the kind and the count of Agents.
The user still steers an exchange between Agents from the Channel they share
with either Agent.

### A Run speaks once in a Channel

Every message an Agent writes in a direct Channel wakes the other Agent in it:
one message, one Run, one answer. `Run::reply_channel_id` names the Channel
where the Run's own reply lands: the waiting conversation when the Run relays
an answer home, and the Run's own Channel otherwise. The message tool refuses
that Channel and tells the model to answer in the reply.

The refusal reaches the model as a tool error inside the turn, so the Run
corrects itself and still answers. The reply carries the answer. The tool
keeps every other Channel, so a Run that relays an answer home can still ask
the Agent it asked for more.

### A delegation chain carries its origin

An Agent delegates by message: its Run posts into its direct Channel with the
target Agent. The answer arrives later in that Channel and starts a new Run.

A Run that another Agent's message starts carries the origin of the
delegation chain: the Agent that owes an answer, and the Channel and Thread
where it owes it. The origin passes down the chain unchanged, however many
Agents the request goes through.

The origin decides two things. A Run whose origin names its own Agent replies
into the origin Channel and Thread, not into the Channel that started it, so
the answer reaches the conversation that waits for it and the exchange ends.
And the system prompt states the Channel and the delegation: who is in the
Channel, whether the user is one of them, who asked, and which conversation
waits for the answer.

A Run that a Schedule or an Incoming Event starts begins a new chain at hop
count zero. Its target Channel and optional Thread are its origin, so work it
delegates returns to the proactive conversation that started it. A Run that
a Coding Session event starts is a Run of an Incoming Event. It begins a new
chain whose origin is the place where it runs: the session's Thread, or, for
the end of the session, the place where the session started (ADR-0033).

## Consequences

- A delegated answer comes home without the model choosing to send it, and a
  delegation ends when it is answered. The hop cap stays as the backstop for a
  true loop.
- An Agent does not address the user in a Channel the user is not in.
- An Agent in a direct Channel answers with its reply. The message tool is for
  an Agent the Run is not already answering.
- A Run persists its origin, and each new trigger kind that starts a chain must
  set it.
