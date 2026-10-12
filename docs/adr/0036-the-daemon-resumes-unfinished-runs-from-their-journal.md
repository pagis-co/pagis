# 0036: The daemon resumes unfinished Runs from their journal

Status: accepted.

## Context

A Run keeps its transcript, Loaded Set and Turn state in its Run journal.
The daemon and the Client App restart for Updates and Upgrades. A restart
must keep completed work and must not repeat an outside effect whose
outcome is unknown. The Continuation Record keeps context across Runs;
it is not the checkpoint of an active Run (ADR-0009).

Established products keep durable execution state:

- [Temporal](https://docs.temporal.io/encyclopedia/event-history) keeps an
  event history for each workflow and replays it after a worker stops. An
  activity that starts but reports no result has an unknown outcome. The
  workflow's failure and retry policy decides what to do
  ([activity execution](https://docs.temporal.io/activity-execution)).
- [Restate](https://docs.restate.dev/foundations/key-concepts) keeps a
  journal for each invocation. On replay, it gives back the recorded
  result of each step.
- [LangGraph](https://docs.langchain.com/oss/python/langgraph/checkpointers)
  keeps a checkpoint after each step of a graph and resumes a thread from
  its last checkpoint.

## Decision

### A Run journal holds one checkpoint for each Turn

The daemon resumes Runs from a **Run journal**. At the end of each Turn,
the Run writes what the next Turn needs: the Turn number, the transcript
that the Turn added, the Loaded Set and the state of the Run. A Turn is
one model call and the tool calls it asks for (CONTEXT.md, Turn).

The journal keeps the model answer and its call ids before tool dispatch.
This record preserves the identity of each call; it is not a completed Turn.
A Turn that does not end has no checkpoint. Resume restores the last
checkpoint and starts that Turn again. A checkpoint belongs to a Turn,
not to each tool call. Only tool calls with an outside effect have their
own records under the next rule. The Run journal is separate from the
Continuation Record that keeps conversation context across Runs
(ADR-0009).

### A tool call with an outside effect is recorded before it runs

For each tool call whose Effect Class is not `free`, and for each call to
a Computer or a Host, the Run journal records `started` before the call
runs. It records the result after the call. The `started` record is
durable before dispatch. An approval does not mark a call `started`:
dispatch waits for the decision and checks the live Grant (ADR-0005).

On resume, a call with a recorded result gives that result back to the
model and does not run again. A call that is `started` and has no result
also does not run again. The model gets this tool result:

> unknown outcome: the daemon restarted while this call ran. Check the result before you do it again.

The model checks the world, such as the sent mail, before it acts again.
A `free` call runs again, except a call to a Computer or a Host, which
always follows the journal rule. Thus resume never repeats an outside
effect, including a call inside a Turn that has no checkpoint.

### Boot resumes each unfinished Run

Boot applies these rules to each unfinished Run:

- A Run that is `queued` stays in its queue.
- A Run that is `running` resumes from its last checkpoint.
- A Run with a checkpoint that waits for an approval keeps its Request open.
  The Person's decision continues the same Run. A restart does not expire
  that Request.
- A Run that has no checkpoint fails as before, with
  `FailureKind::DaemonRestarted`, except a `queued` Run, which stays in its
  queue. A failed Run that waits for an approval has its Request expired
  and gets the restart note in its Channel.

This rule replaces the restart failure rule of ADR-0006. Pending Wake-ups
survive a restart, and a Run that a Wake-up starts resumes under this
decision. Recovery continues that Run; it does not make another Run from
the Wake-up.

### A Run resumes once

The Run journal records the Turn that recovery resumes before that Turn
starts again. A Run that a second restart stops in the same Turn fails
with `FailureKind::DaemonRestarted`. A Turn that stops the daemon thus
cannot loop. When a Turn ends and writes its checkpoint, the next Turn
has its own single resume attempt.

## Other ways were considered

- **Fail each unfinished Run, as now.** It is simple, but each Update,
  Upgrade and crash loses the work of every Run.
- **Rebuild the Run from its Channel messages.** The messages do not hold
  the transcript of the Run, the Loaded Set or the state of each tool call.
- **A durable execution engine, for example Temporal.** It adds a service
  to each installation, and a Local Installation runs as one process.
- **Run each tool call again on resume.** A send of a mail, a purchase or
  a Call then happens two times.

## Consequences

- A Run with a checkpoint survives an Update, an Upgrade and a crash, and
  continues from its last Turn, subject to the single resume attempt for
  that Turn.
- A tool call with an outside effect never runs two times because of a
  restart. The model reads that its outcome is unknown when no result is
  recorded.
- The Run journal is one more record for each Run. It holds tool results,
  so Forget and a Backup treat it as they treat the conversation that
  holds the same tool results (ADR-0008). A checkpoint that the Run does
  not need after it ends is deleted when the Run ends.
- The restart note in `crates/pagis-agent/src/recover.rs` applies to a Run
  that cannot resume.
