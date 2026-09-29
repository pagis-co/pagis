# 0009: Conversation context and learning have separate checkpoints

Status: accepted.

## Context

A conversation needs continuity when its whole history does not fit in a model
request. Facts from it can also belong in durable memory, and evidence from
several messages or sources can need a separate review. These jobs use
different data, permissions and failure rules. A design that ties continuity
to learning spends a model request with no durable reason and fails one job
when the other fails.

## Decision

### Each job has its own checkpoint

A Continuation Record keeps one conversation coherent. A foreground memory
change records a durable fact. Pending Evidence asks Reflection to reconcile
evidence that needs a separate review (ADR-0010). The success or failure of
one job says nothing about another.

The conversation key is `(workspace_id, agent_id, channel_id,
root_message_id)`. A null `root_message_id` names the top-level conversation,
and a value names a Thread. The Agent owns the records and cursors under the
key.

Messages use immutable `MessageId` order. A source range is
`(after_exclusive, through_inclusive]`. A writer fixes the upper cursor before
model work starts, and later messages stay outside the range as verbatim
input. A cursor never splits a tool call from its result within an active Run.
A record refers to durable messages and artifacts where an earlier Run kept no
full tool transcript.

A foreground memory commit records the conversation range that was available
when the Agent staged it. The commit and its `memory.committed` event carry
the key and both bounds. The range is the provenance of one write. It does not
claim that every fact in the range was reconciled, and it advances no review
or compaction cursor.

### The context of a Run is the whole permitted conversation

A Run in a Thread reads the whole Thread. A top-level Run reads the whole
top-level history of its Channel. A valid Continuation Record replaces the
exact older range it covers. The daemon drops a forgotten message and a
message whose exposure stamp does not settle (ADR-0004).

### A Continuation Record is derived working context

A conversation has at most one current Continuation Record: its revision and
source range, the goal, constraints, corrections, decisions, work state, open
questions, the next step, and the references needed to recover omitted
detail. It records each source exposure and Forget dependency it used. Its
text reaches the model inside the untrusted envelope.

The record is not memory and grants no authority; the messages and artifacts
stay the evidence. Replacement is a compare-and-swap on the revision, and the
daemon writes the record and advances its cursor in one transaction. A model,
validation or storage failure keeps the previous record and cursor. A writer
that loses the compare-and-swap reloads once and includes the new messages; a
second conflict ends the attempt.

The daemon checks every exposure and Forget dependency when it reads a record,
and one invalid dependency invalidates the whole record. The daemon rebuilds
from permitted evidence. If that does not fit, the Run stops with a visible
context error rather than use stale text.

The record is one row under the conversation key. The end of a Run or a
conversation does not remove it; a forgotten source message or a changed Grant
revision deletes it. A conversation resumed days later loads the record and
the messages after its upper cursor.

No other conversation, Schedule Run or arrival Run can read the record, and it
is no Brief candidate. So compaction reads the record it has just made, and a
record with open work adds the conversation's Subject Page to the proposed
memory changes (ADR-0007). The page is ordinary memory under ordinary rules. A
Run with foreground memory already staged proposes no page at compaction,
because a compaction proposal cannot commit beside staged work, and the
reply-time writer of that Run owns the page.

### Conversation evidence stays in Run scope

`conversation_search` and `conversation_read` take the Workspace, Agent,
Channel and root message from the Run, and no argument selects another scope.
A review Run can combine several origins, so these tools refuse it.

Search uses the full-text index of each Storage Backend (ADR-0008) and applies
scope, Grant and Forget checks before its limit. It orders results by message
id and stable reference. Read takes one stable reference or a bounded range
and checks access again rather than trust a snippet. Results keep the speaker,
time, text, artifact references and whether each artifact is available.

Pagis keeps exact tool evidence only when a trusted provider opts the read
tool in, the model call has a stable tool-call id, the broker selected one
live Grant revision, and the Run has one message source. The tools are Google
`mail__get_message`, `mail__get_thread` and `google__calendar_events`.
Searches, writes, other providers, core tools and Plugin tools do not opt in.

The broker's 100,000-character cap applies before retention, and the record
says whether it is complete. A structured retention status gives a stable
reference or an unavailable reason; provider requests do not receive it, and
the tool result text keeps its shape. Compaction keeps an active result, or
stops with a context error, unless its retained reference is present and
complete.

Retained text stays data. A Forget or a source Grant change removes the record
and its search row. A Forget of one Source Item removes each record a Run kept
when it read the item, and forgets each message of that Run (ADR-0008). Search
and read also check current source permissions.

### The whole model request has one budget

The budget covers system and user instructions, tools, retrieved memory, the
Continuation Record, messages, images, tool calls and results, and reserved
output. The context and output limits are the smallest in the configured
candidate route. They come from the Provider Model List, then `models.json`,
then a conservative default, so an unknown model still has a budget.

The output reserve is the smallest of 16,384 tokens, the route's output limit
and a quarter of its context limit. The input allowance is the rest. The
daemon estimates the whole encoded request before each model call, including
calls after tool results. An input with no bounded estimate cannot enter the
request.

An image costs the provider's documented formula at its pixel size on the most
expensive candidate. A model with no documented formula takes the largest
current one, and an image of unknown size costs the documented maximum for
one image. When thirteen tool results of a Run hold a full screenshot, all but
the latest three shrink together to 160 by 90 thumbnails. A shrink changes the
prompt prefix and misses the prompt cache after it, so shrinking in chunks
breaks the cache once in ten calls. A shrunk result keeps its thumbnail and
says so, because OpenAI's computer tool refuses a result without a screenshot.

Compaction starts above 80 percent of the input allowance and targets at most
60 percent. It keeps the latest messages up to the smaller of 16,384 tokens
and a quarter of the allowance, and every message of an active tool call and
result. A request that misses the target but fits the allowance goes to the
model. A Run stops with a context error when active work alone exceeds the
allowance. The Run stays `running` while it compacts.

Compaction returns a Continuation Record and zero or more proposed memory
changes, validated apart. The record can commit when a memory change fails.
Failed memory changes stay pending, a failed record advances no cursor, and
neither starts a second review of the range.

## Consequences

- Ordinary replies commit memory with no review call.
- Compaction can fail without deleting history or blocking a memory result.
- Revoked source access invalidates derived context.
- Open work outlives its conversation as ordinary memory.
- The daemon stores more records and cursors, and each has one job and one
  success condition.
