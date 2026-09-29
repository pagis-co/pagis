# 0004: A block that asks is a Request, and a block that shows is content

Status: accepted.

## Context

A message carries more than prose: a table, a form, a card of choices, an
approval, progress, an image, a file, a live screen. Slack Block Kit sets the
shape: a union on `type` posted through one call. Adaptive Cards supplies a
declarative table and a fallback for an unknown type. MCP elicitation is the
pattern for a form in the middle of a Run: a call that parks, carries a flat
schema of primitives, and returns the user's answer as the call's result.

## Decision

### The vocabulary is a typed union

The block types are `markdown`, `table`, `form`, `choice_card`,
`approval_card`, `progress`, `image`, `file`, `screen`, `call`, `mail` and
`widget`. The union is a `Block` enum in `pagis-core`, tagged on `type`, so
`blocks` is a `oneOf` in the OpenAPI specification and a discriminated union in
the generated TypeScript. Both sides keep an unknown arm, because a client that
rejects a newer daemon's block breaks for no gain.

### A block that asks is a Request

A Request is a durable row with a `kind` of `tool_action`, `form`, `choice`,
`credential_action` or `widget`. A Run has at most one pending Request, and a
second ask waits behind it. A user message in the Channel supersedes it. There
is no clock deadline: `expired` means that the waiting Run died.

An Approval is the `tool_action` kind. It gates any action that the broker
dispatches. An Approval is a decision the user makes before an Agent may act;
"which date should I book?" is not one.

One body answers every kind: a decision of `approved` or `denied`, an optional
scope of `once` or `always`, and optional values. A form submission is
`approved` with values, and a dismissal is `denied`. The scope applies to
`tool_action` alone, and `always` needs a trusted allow-rule builder.

The server validates the values against the field schema on the Request row,
never against the block's copy, because a client can post back a changed
block. A validation failure is a client error and never wakes the Run.

### Two tools, split by whether the block needs an answer

`add_block` appends a block to the message that the current turn finalizes and
returns an acknowledgement. One turn is one message: the model's prose becomes
`markdown` blocks around the added blocks, in call order.

`ask_user` makes a Request row, posts the `form` or `choice_card` block that
shows it, and parks the Run. The user's answer returns as the tool result.

Both are core tools in one Capability Manifest, dispatched through the broker,
so they are in the Run's snapshot and leave the same audit facts as any tool.
Both have the `free` effect class: writing into one's own message is not a
gated act, and a gate on `ask_user` would be circular. The Request row is its
audit trail. `ask_user` posts into the Run's own Channel with no check that a
user is in it, so it never fails for want of a user. A form that nobody
answers dies with its Run.

### The daemon makes any block that shows a row it owns

An Agent emits `markdown`, `table`, `form`, `choice_card`, `image` and `file`.
The daemon makes `approval_card`, `progress`, `screen`, `call`, `mail` and
`widget`. A block that refers to a durable row the daemon controls comes from
the daemon, because a model that could make an `approval_card` could forge
authority over its own gate. For `ask_user` the Agent supplies the content,
and the daemon makes the row and the block.

### A settled block reads its state from its row

A `form` and a `choice_card` carry the Request id and copies of the display
fields, so the message renders on its own. The live state and the submitted
values come from the Request row. A settled form renders read-only with its
values, and an expired one renders disabled with its reason. A submission is
not a message: the answer belongs to the Run that asked.

### Progress comes from the daemon

A `progress` block carries the Run id and a text that the daemon composes from
facts it holds. No tool sets it, so a model cannot misreport its own progress.
It has no step counter, because the daemon has no fact that supplies one.
Updates travel on ephemeral frames and never enter the event log, so a
finished Run shows the last state.

### The table is declarative and thin

A `table` holds columns (a key, a label, an optional alignment) and rows of
cells. A cell is text, a number, a link or a timestamp. There is no server
pagination and no sort configuration; the client sorts. Past 100 rows the
Agent writes a CSV artifact and posts a `file` block.

### Form fields are flat primitives

`text`, `number`, `select` with options, `checkbox` and `date`, with no
nesting, no conditional visibility and no layout hint. A `choice_card` is its
own type: it is one tap, and a form is fill and then submit.

### Every block projects to plain text

Each type has one plain-text projection, and the message text is their
concatenation. A table projects to its capped markdown, a form or a choice
card to its title and its field labels or options, and an image or a file to
its alt text or file name. One projection serves full-text search, the Agent's
own context on a later turn, and another Agent that reads the Channel.

### Caps are enforced at the tool boundary

20 blocks per message, 100 rows per table, and 256 KB of block JSON per
message. `add_block` checks all three and returns a tool error, so the model
retries smaller. A check at finalize would kill a good turn.

### An exposure stamp is settled by its author's Grants

The daemon stamps every generated message with the source Grants that its
author held when the words were written. When the user revokes the Connection
behind a stamp, the message reads as unavailable because its source access
cannot be verified.

One predicate settles a stamp, and it asks the author's question, never the
reader's. The user's own words carry no source scope. A stamp that names no
Grant is prose of no source. Any other stamp holds while the author's live
Grants still carry every Grant id and revision in it. The API renderer and the
Run transcript both call the predicate, so the user and an Agent in the same
Channel read the same words.

An Agent is a member of the user's staff, and an agent-to-agent Channel is how
work is handed over. What one Agent tells another is a disclosure by that
Agent, as a colleague forwards a mail. A Grant governs which Agent may call a
Connection, not which colleague may hear what was learned from it.

A message whose stamp does not settle wakes no Agent, because its words reach
no reader.

## Consequences

- An Agent can write into a message's blocks, so the caps are a security
  control.
- A Run can park on a question that has no permission part.
- The user can answer a form in an agent-to-agent Channel.
- Revocation withdraws prose everywhere: from the Channel the user reads, and
  from the context of every Agent that reads the same Channel.

## Out of scope

Generative widget authoring beyond ADR-0016, block actions that call back into
a running Agent, message editing, nested or conditional form fields, block
localization, and user-authored blocks.
