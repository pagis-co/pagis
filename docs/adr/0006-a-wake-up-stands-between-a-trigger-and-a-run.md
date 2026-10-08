# 0006: A durable Wake-up stands between a trigger and a proactive Run

Status: accepted.

## Context

An Agent must act on time and on a change at a source, while the Run
lifecycle, the per-Thread serialization, the Grants, the approvals, the audit
trail and the low idle cost stay as they are for a message.

A local daemon is often asleep. To replay every missed tick makes a stampede,
and to drop all missed work makes a reminder unreliable. Three rules on one
Connection must not cause three provider polls, so provider polling,
subscription matching and Run execution need separate seams.

Zapier treats a schedule as approximate. GitHub Actions can delay or drop
scheduled work and keeps one pending run in a concurrency group. n8n has
durable occurrences, misfire policies, deduplication, claims, and one-shot,
cron and interval kinds.

## Decision

### One Trigger module owns proactive routing

One Trigger module sits between provider acquisition and the agent system. It
owns Schedules, Event Subscriptions, source occurrences, provider cursor
commits, deduplication, matching, and the creation, combination, withdrawal and
claim of Wake-ups.

```text
ingest(batch)
process_due(now)
claim_wakeups(agent_id, available_slots)
manage_schedule(...)
manage_subscription(...)
```

A provider collector stops at `ingest`, and the agent system starts at
`claim_wakeups`. Provider transport and Run execution do not cross this seam.
Tests cross the same seams: `process_due` and `ingest` with the public stores,
`claim_wakeups` with the agent system, and the REST and WebSocket interfaces
for a full daemon. Schedule tests mock time and do not test timer internals,
private matchers or SQL.

### Source facts and delivery facts are separate

A **Schedule** is a durable rule with a target Agent, an instruction, a target
Channel and optional Thread, a timing, a creator, a revision, an approval
revision, a state and a next due time.

A **Schedule Occurrence** records one due instant, also when no Run starts. Its
outcome says whether it created or joined a Wake-up, or was skipped.

An **Incoming Event** records one normalized provider occurrence, unique by
source, qualified event kind and provider event id. It stores normalized
metadata, the provider occurrence time, the receive time and the source batch,
and never a body, a snippet, an attachment, a credential or provider stderr.
The declaration of a kind names the synced resource whose Source Items its
occurrences are, and `ingest` drops an occurrence of an item that a Forget
blocks (ADR-0008).

An **Event Subscription** matches one declared event kind from one source
and gives it an Agent, an instruction, a conversation target, a typed filter,
a creator, a revision, an approval revision and a state.

An Incoming Event and an Event Subscription name their source: a Connection,
or a Coding Session (ADR-0033).

A **Wake-up** is the durable delivery decision between the source facts and a
Run. It stores its rule revision, the Agent, the conversation target, the
instruction snapshot, the linked occurrences, a state (`pending`, `started` or
`withdrawn`) and an optional Run id. A started Wake-up points to one Run, and
the outcome stays on the Run. Each pair of an occurrence and a rule joins one
Wake-up, and several occurrences can join one pending Wake-up. A proactive
Run's trigger reference points to its Wake-up, which completes the audit path.

### Schedules have three timing kinds

Five-field cron in an IANA timezone, with no seconds field and no shorthand;
an interval of at least one minute from an anchor; and a one-shot at one
absolute instant.

Each Workspace has a default IANA timezone, the Person's own. The Workspace
that boot seeds takes the host's timezone. A Workspace that an Administrator
makes takes that Administrator's timezone until the Person's first sign-in,
which sets the timezone that their browser or Client App reports. The Person
changes it in Settings. A cron Schedule copies the timezone at creation, and a
later change does not move it. The exception is the Report Schedule
(ADR-0022): it is the Person's morning, so an active Report moves with the
timezone.

Cron keeps wall-clock time. A time in a spring gap runs at the next valid
local instant, and a repeated autumn time runs once, at its first match. An
interval stays at the anchor plus a multiple of the interval, and Run duration
never shifts it; an edit of the interval resets the anchor unless the edit
supplies one. A one-shot resolves to UTC at creation.

Scheduling has minute resolution and no exact-start guarantee. While the
daemon runs, Pagis records an occurrence within 60 seconds. One scheduler loop
sleeps until the earliest next due time or a Schedule change; there is no task
for each Schedule. Due processing inserts uniquely keyed occurrences, advances
each Schedule, and creates or combines Wake-ups in one transaction.

The scheduler claims no Wake-up and no pending review while no provider holds
a key, because a Run then fails and reads as work that needs the Person. The
Wake-up waits for a key.

### Downtime and overlap coalesce

After downtime, an active recurring Schedule creates one late occurrence for
the most recent missed time and records earlier missed times as combined. A
missed one-shot creates one late occurrence whatever its age. The briefing
states the scheduled time, the actual start, the lateness and the combined
count, so the Agent decides whether the work still makes sense.

A rule has at most one active Run and one combined pending Wake-up. A new
occurrence never cancels the active Run and never builds a queue. Across
rules, pending Wake-ups go in due-time order, and a user message takes free
Agent capacity first.

### Lifecycle and authority are explicit

Schedule states are `active`, `paused`, `completed`, `blocked` and `archived`.
A one-shot is completed after its Wake-up starts, also when the Run fails. A
delete archives, so audit links survive.

The user and an Agent can create a Schedule. One made in a conversation
targets that conversation; one made in settings or autonomously targets the
user's direct Channel with the Agent. An Agent creates only for itself.

An Agent-created Schedule or revision needs an approval. The gate is the
broker's `tool_action` Request on the Agent's own tool call, so the row exists
only after the user approves: a denial leaves nothing to undo, and a pending
creation is a Request, not a Schedule. The card shows the Agent, the
destination, the instruction, the cadence and the maximum Runs for each day. A
change of the timing, the instruction, the Agent or the destination makes a
revision and needs an approval again. The approval permits future Wake-ups,
model use and conversation posts. It grants no capability and approves no
future tool effect.

An Agent can pause, resume or skip a rule it created for itself whose current
revision is approved. A field change, a delete, or a change to a user-created
rule needs an approval. The user manages every rule directly.

A paused Schedule does not catch up, and a resume starts from the next future
time. A skip records the next occurrence as skipped and advances the Schedule
under the lock of due processing; where due processing already created the
Wake-up, the skip returns a conflict. An edit, a pause or an archive withdraws
the pending Wake-ups of the old revision. A running Run keeps the instruction
and target it started with.

### Event acquisition runs once for each Connection

One provider collector runs for each Connection that has an active Event
Subscription. It acquires changes once and submits normalized batches to
`ingest`, which commits the provider cursor, the deduplicated events, the
matches and the Wake-ups in one transaction. Pull, streaming and webhook
collectors all enter through `ingest`. A provider read may retry before the
cursor commit. A proactive Run never retries by itself, because it may have
had an external effect.

Event Subscription states are `active`, `paused`, `blocked` and `archived`,
with the creator, revision, destination and approval rules of a Schedule. A
new or resumed subscription sets an activation watermark and does not treat
earlier data as new. A paused one does not catch up, and a collector stops
when every subscription on its Connection is paused or blocked.

A filter is typed and evaluated locally after broad collection. Different
fields combine with AND, values inside a field with OR, and text matching
ignores case. There is no regular expression and no expression language.

The first successful collection sets a baseline and emits no events. One batch
creates at most one Wake-up for each matching subscription and links every
matching event of the batch. Events that arrive while the subscription has an
active Run join one pending Wake-up.

The live Grant is checked before every Wake-up. A revocation blocks the
subscription, withdraws its pending Wake-ups and posts one notice; a new Grant
restores delivery from then on. A Connection that needs reauthorization blocks
its subscriptions, posts one notice and keeps the cursor; after
reauthorization they reactivate and one catch-up collection runs.

### A proactive Run uses the normal Run lifecycle

A rule names a Channel and an optional Thread. The Run context is built at
the start: a Thread target gives its root and replies through the wake time,
and a top-level target gives the top-level history of its Channel, through the
context builder of every Run (ADR-0009). The briefing adds the name, the
instruction, the scheduled time, the actual start, the lateness and the
combined count. A source briefing adds the subscription instruction, the
Connection alias and the normalized metadata inside the untrusted envelope. A
body or an attachment enters only through a live granted tool call.

A user message in the same Thread enters the active proactive Run at its next
turn. Another Wake-up never injects its instruction; it stays pending. Pending
Wake-ups survive a restart. Once a Wake-up has started a Run, restart recovery
fails the unfinished Run and Pagis does not start another one.

A future Schedule keeps no actor alive. A pending Wake-up starts an actor only
when capacity can claim it, and an Event Subscription keeps only its
collector active.

### Management and audit

An Agent has six core tools: create, list and update for Schedules and for
Event Subscriptions. The update tools take validated actions (pause, resume,
skip, archive or a field change), omit the Agent id, and take conversation
references, not database ids.

REST reads return the current revision, the state, the creator, the Agent, the
destination, the last Run result, and the paginated occurrence and Wake-up
history. They carry no pending approval, because the gate is at the broker. A
Schedule adds its timing and next due time. A subscription adds the source,
the Connection, the filter, the collector health, the last collection and the
last matched event.

The module publishes an event for each rule creation, update and state
change, each recorded occurrence, each received Incoming Event, each collector
state change, and each creation, combination, withdrawal and start of a
Wake-up, with the Workspace, the revisions, the Agent and a redacted payload.

## Consequences

- Time and provider sources share one delivery model without sharing provider
  details.
- Several subscriptions on one Connection cost one collection pass, and each
  matched instruction stays auditable on its own.
- One Wake-up starts one Run. An external effect is at most once, because a
  failed Run does not replay.
- Catch-up on a sleeping local daemon is one late Wake-up for each rule.

## Out of scope

Automatic Run retries, replay after a pause or a Grant gap, more than one
pending Wake-up for each rule, second-level schedules, exact start times, raw
provider query languages and regular-expression filters.
