# 0021: Caller ID proposes a tier, and a keypad code confirms it

Status: accepted.

## Context

A telephone number is public, so a remote party is the first input channel a
stranger opens at will, and speech on a call reaches the model loop that holds
the user's Grants. The surveyed voice-agent stacks have no caller-trust
mechanism. One has a prompt that tells the agent to hide how a message arrived;
a boundary written in the prompt is one the prompt can argue with.

The Capability Snapshot comes from the manifests, the Agent's live Grants and
the Workspace's Connections alone, and is fixed for the Run, so a tier applies
at the realtime session, below the snapshot. Caller id comes from the carrier
and can be spoofed, and attestation reaches no stack.

## Decision

### Three tiers gate authority, not access

Every call has one tier in both directions, chosen by the remote party's
number whoever dialed. A restaurant the Agent dialed is Unknown.

| Tier | What speech is | What the session binds |
| --- | --- | --- |
| Owner | instruction, with the standing of the user's message | the brief's tools, narrowed to `Free` |
| Trusted | a request; an external effect still waits for its card after the call | the brief's tools, narrowed to `Free` |
| Unknown | data only: no tool call, no memory write, no approval made from it | no tools, plus hang-up |

Pagis answers a call from any number. The tier decides whose words can move
the Agent.

On an outbound call Pagis dialed the number, so the listed tier holds. On an
inbound call caller id proposes a candidate tier, and any tier above Unknown
needs the keypad code; without it the call is Unknown for its life. A spoofed
number reaches an Agent that binds no tools.

### A keypad code confirms an inbound tier

Each Workspace holds one code of 6 to 8 digits, ended by a hash key or by its
length. The secret store (ADR-0013) keeps only its Argon2 hash, under a name
that carries the Workspace, so one person's code confirms nothing on another
person's line. The person sets and clears it through the daemon. Only the tier
check reads it, and no API returns it.

The daemon runs the challenge before the realtime session exists:

1. The call is answered, and the caller's number matches a listed number.
2. The daemon plays one fixed prompt and collects keypad events from the media
   hub, with a debounce for each digit and one short overall timeout.
3. The tier is the lower of the candidate and the proved: a Trusted-listed
   number with the right code becomes Trusted, never Owner.
4. The realtime session starts, bound to that tier's tools.

A caller who enters nothing or a wrong code, or has no keypad, proceeds as
Unknown, and a number on no list is never challenged. Three attempts are
allowed in a call; after the third the call stays Unknown. Each Workspace also
counts failures across its calls and lines: after 6, keypad elevation is
suspended for a delay that starts at one minute and doubles with each further
failure, up to 24 hours. The call is still answered as Unknown, so a spoofer
cannot lock the user out of their line. A correct code or the user in Settings
clears the count, and the user gets a notice when a delay starts.

The code is never spoken, because on a call the realtime model is the
transcriber. Keypad tones never enter the model uplink or the recorder's
decoded audio. A press after the session starts reaches only the tier check.
The transcript, every record made from it, and the transcript a reconnected
session receives get one line, `[the caller used the keypad]`, for each run of
presses, with no digit and no count. A carrier that sends inband tones puts
them in the audio, and Pagis accepts that. The daemon detects the code, and no
tool reports it, because a tool that raised a tier is a lever the model can be
talked into pulling.

### The tier can rise, and a person can drop it

A tier rises once, when the code arrives, at the challenge or later in the
call, through one session update that binds the new tier's tools. It never
rises another way. It drops only when the user drops it from the live call
block, never on a heuristic, so a speaker change mid-call is not detected.

### The Trust List

```text
trust_entries
    workspace_id
    agent_id?             NULL for the Workspace-wide list
    subject               number | address | domain
    value                 the E.164 number, the bare address or the bare domain
    tier                  owner | trusted
    label
    created_at
```

A row with no Agent is one of the user's own identities, Workspace-wide, and
reaches Owner. A row with an Agent is on that Agent's Trusted list. The list
serves calls, texts and mail (ADR-0019). No tool writes it; the user edits it
through the daemon, because a list an Agent can extend is a list a caller can
talk their way onto.

### The mechanism is structural, and the prompt only reports it

At Unknown the session binds no tools, so an injection has nothing to reach
for. The brief tells the model its tier in one factual line, so it does not
reach for missing tools in front of a stranger. It never tells the model to
hide how it was reached or to keep a secret. Hang-up is the one exception to
the empty set: its worst case is that a stranger talks the Agent into ending
the call, and an Agent that cannot end an abusive call is worse. The rule is
"the empty set plus hang-up", not a category of harmless tools.

A settled call wakes the Agent under its Standing Call Rule at every tier, with
the transcript inside the untrusted envelope. The instruction is the rule's,
which the user wrote, and the call supplies facts, so the Agent may write
memory from an Unknown call, stamped with the source and tier. Nothing runs
while the stranger speaks. A user who wants less sets a minimum trust on the
rule.

### What a provider must support

A realtime provider that carries calls must run a session with no tools bound
and change the bound tools during a session. A provider without the
second implements a rise as a new session. A provider without the first cannot
carry calls. A tier in the prompt is never a substitute.

## Consequences

- An inbound call from a Trusted number with no code is Unknown, and the Run
  that reads it afterwards reads data.
- The user must reach a keypad to raise an inbound tier.
- The code is a shared secret: a user who gives it away gives a stranger Owner
  authority.
