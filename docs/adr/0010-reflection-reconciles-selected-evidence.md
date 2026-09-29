# 0010: Reflection reconciles selected evidence

Status: accepted.

## Context

A reply often learns something worth keeping, and most of the time the reply
can write it down. Some evidence needs more: a synced arrival that nobody
discussed, or a conclusion that rests on several messages or sources. A model
request after each reply spends money with no durable reason and ties the
reply to the settlement of memory.

## Decision

Subject Pages, their Facts tables and their Schedules are the one durable
learning model. There is no second store of typed claims.

Three paths change memory:

- **A reply** commits the memory change it staged. It calls no second model
  phase and does not reflect.
- **Compaction** returns proposed memory changes beside its Continuation
  Record (ADR-0009). The Run stays `running`.
- **Reflection** reviews selected evidence: a source arrival that its
  Reflection Filter selects (ADR-0011), or durable Pending Evidence. It runs in
  its own Run, which is `reflecting` for the review, and commits a memory
  change or records a successful no-change result.

### The daemon builds the memory Brief

The daemon builds each reply turn's Brief from the memory files of the
reader's scopes, Subject Pages and fact files, never a scope's index file
(ADR-0007). It calls no model to select pages. It matches normalized entity
words from recent turns and offers at most three pages. A page already shown
in the conversation stays out until its file changes.

The database keeps the conversation's memory revision and shown page paths. A
page changed after that revision enters the next Brief with no entity match. A
conversation with no cursor uses pack mode and gets the most recently changed
pages first, and the daemon then saves the cursor.

The Brief has an 8 KB limit. The selection puts the most relevant page first,
so the builder trims from the end: the last fact of the last page that has one,
then the last Timeline entry, then the last page. A fact file has neither, so
it goes out whole, and the trim always reaches an end. Each page has a path
fence, and the Brief marks all retrieved content as data.

### A reviewed Subject Page can set an Intervention

Reflection can set, move or cancel a wake-only Schedule on a Subject Page. The
daemon owns and renders the page's Schedules section, and a reflection write
cannot forge or replace it.

A wake-only Schedule needs no approval, because it authorizes only a future
Run. That Run gets the compiled truth, the active Facts, the open Schedules and
the latest ten Timeline entries. It can send one message, reschedule or stay
silent. An unmarked reply sends one message; a reply that starts with
`silent:` stays private and records the rest as its reason. A newer Reflection
can move the open Schedule but cannot withdraw a sent message. A 14-day
cooldown starts when the Schedule fires, and Pagis refuses an earlier
reschedule.

### A selected source arrival reflects in its own Run

Acquisition appends Timeline entries before the resource's Reflection Filter
selects changed Subject Pages. One selected acquisition batch starts one
arrival Run, with no Channel and no reply phase. Its request holds the
changed-page Brief and offers only memory and schedule tools. The selection is
already durable work, so an arrival makes no Pending Evidence.

An arrival Run uses the normal memory commit and Learning Feed. A failure or
cancellation keeps the Timeline entries that acquisition committed. A failed
reflection ends the Run with `model_failed` and keeps the error visible.

### Pending Evidence reflects through a durable lease

The Agent calls `memory_review` when current evidence cannot support a durable
conclusion. The tool records the subject, the reason, the urgency, the bounded
conversation range and the source permissions, and runs no model inside the
reply.

Pending Evidence belongs to one Agent and subject. It records its source
range, dependencies, urgency, eligibility time, maximum due time, attempt
state and lease revision. Normal work is eligible five minutes after its newest
evidence and due within 30 minutes of its oldest unprocessed evidence. Urgent
work is eligible at once and due within 60 seconds. A deadline or Schedule
change is urgent.

The daemon claims due work with a lease and a compare-and-swap revision and
gives it to a review Run. Urgent work sorts first, then by due time, creation
time and id. The review uses the arrival slot pool. A waiting Schedule goes
before a normal review, and an urgent review can start while its source
conversation is busy. Idle time can make recorded work due; it does not create
work.

The review Run checks every source and permission before it enters
`reflecting`. A successful review records a committed memory revision or a
no-change result before it completes the lease and advances the learning
cursor. A model, storage, access or cancellation failure advances no cursor.
The daemon retries after one minute and after five minutes, then keeps the
failed work visible for an explicit retry. New evidence makes a new range that
refers to overlapping failed work, and does not delete or revive that work.

Batching combines work only for the same Workspace, Agent and subject. A batch
keeps the earliest lower cursor, the latest upper cursor, all dependencies,
the oldest due time and the highest urgency. Completed foreground writes and
reviewed ranges are excluded by their source cursors and memory revisions.

### Accounting reports the actual job

Each logical model request emits one `model.completed` audit event with the
phase, the request number, the outcome, the duration, the router attempts and
retries when known, the serving provider and model, normalized usage, and a
cost estimate from metadata. Missing provider usage stays unknown. Cache tokens
count as input, and reasoning tokens as output. A successful final attempt does
not invent usage for failed attempts.

Phase reports also count committed memory changes, no-change reviews, commit
failures and unfinished work. Lower token use is not success when the Run stops
before its job is done.

## Consequences

- An ordinary reply uses one model phase unless it needs another turn for
  tools.
- Foreground memory, arrival Reflection and deferred review have separate
  source ranges and success records.
- Reflection has a durable cause, a bounded source range and recoverable work,
  and it keeps the normal Run event, cancellation and recovery paths.
- The Learning Feed shows committed memory changes. A no-change review leaves
  an audit event and no feed entry.
