# 0022: The desk puts one conversation first

Status: accepted.

## Context

The product is used as a group chat with a very capable colleague in it. One
Agent gets almost every message and gives work to the others, so a flat roster
makes the user read the whole list to find the one row that matters.

Prior art divides the interface the same way. An integrations product keeps
each account on a card in one settings destination and moves broken
authentication into a repair queue. An automation product gives durable rules
their own destination, with run history and a pause for each rule, and picks a
cadence from a small typed vocabulary. A chat app lights the conversation that
changed, and follows the user with a huddle only because a huddle is a place
the user speaks.

## Decision

### Layout rules

Chat stays mounted. Above 760 px the inspector is a column beside the
conversation and never covers the composer. At 1100 px and below the Desk
Panel does not open by itself, because the column takes room from the
conversation: Home and the Chief of Staff's direct Channel carry a Desk Panel
toggle, and the address holds the open panel. At 760 px and below the
conversation list goes off-canvas, every full main view carries a control to
open it, and a master-detail body becomes two steps with a back control. At
390 px an inspector is the whole screen.

### The Chief of Staff is a Workspace setting, and the shell pins it

The Workspace names one active Agent as its Chief of Staff; the seed names the
default Agent. The user moves the designation from the roster. Archiving the
Chief of Staff moves it to the next active Agent, oldest first, and a
Workspace with no active Agent has none. The setting grants nothing: the roster
stays flat, and the Chief of Staff delegates through Channels as any Agent
does.

The sidebar has six places at the top: Home, Sprites, Memory, Automations,
Software and Settings. Home carries the count of the Needs-You Queue. Each
place replaces the main pane. Below the places the sidebar lists the
conversations: the Chief of Staff first with its presence and caption, then
the other Agents, then the groups. The composer on Home addresses the Chief of
Staff's direct Channel. On a phone the same column is the drawer.

### Home is the Report

Home renders the Needs-You Queue, then the Report, then the day's work record.
The Report is a message the Chief of Staff writes into its direct Channel on a
Schedule the seed creates, once a day in the Workspace timezone, and on demand
from Home. Its prompt names the questions and their order. Between Reports,
Home shows the last one with its time and a line for every Run since.

### Configuration is settings; a durable rule is a destination

Settings holds the person's own Workspace settings in three groups:

- **Access.** Connections: one card for each, with the alias, the provider,
  the account, the auth mode, the connect time, the absent capabilities, the
  Agents with access, the state and the repair action; add, disconnect and
  reauthorize are here. The Org's Installation Connections show here too,
  flagged as the installation's. Hosts. Vault: one row for each Credential
  with its domain, username, sign-in address, second factor, provenance and
  allowed Agents; the add form is the only place a human types a secret, and
  it says there is no export and no reveal. Trusted contacts: the tier legend,
  the Workspace-wide rows, each Agent's rows, and the keypad-code card, which
  says the code is never spoken to the model and never enters a transcript or
  recording.
- **Models.** The Model Aliases and their ordered candidates, and the person's
  own Usage.
- **System.** Retention, Sound, and, for an Administrator, one link to the
  Administration Interface (ADR-0024). Installation settings are not in the
  Product App.

The Sprites place opens one Agent's page, with the About, Appearance, Desk,
Contact, Access, Memory and Work tabs. Contact holds the phone number card and
the mailbox card; a number is bought, assigned, unassigned and released there,
and the release confirmation names the Agent. Access is the Agent's whole
authority: Connection capabilities, vault domains and host allow rules. A
Connection card lists the Agents with access and grants or removes it, and
edits no capability. Each edit is a Grant revision.

Automations is a place: a Needs-You Queue at the top, then Schedules, then
Event Subscriptions, and a detail body with the current revision, the state,
the creator, the Agent, the destination, the timing or filter, the next due
time, the last occurrence, the last Run result, and the paginated occurrence
and Wake-up history. It has no pending-approval field: an Agent-created rule
that waits for a decision is a pending Request, shown in the Needs-You Queue
alone, until the user approves it.

### Attention has one queue

A Connection that needs reauthorization, a blocked collector and a rule that
waits for approval appear in the Automations Needs-You Queue and on their own
records. The queue is a view, never a second source of truth. A pending
Wake-up appears on its rule and makes no timeline row; once it claims Run
capacity, the timeline uses the daemon's progress block.

### The inspector slot

The Desk Panel is the default tenant. Above 1100 px it opens by itself on Home
and in the Chief of Staff's direct Channel, and the slot stays closed elsewhere
until the user opens a tenant. On Home it lists every Desk, the Chief of
Staff's first and expanded. In the direct Channel it shows the Chief of Staff's
Desk, joined by another Agent's Desk while a delegation that the Chief of Staff
started from that Channel is open, until the answer lands. The Call and the
Mail inspectors are transient tenants that exist only while there is something
to show, and the Desk Panel returns when one closes. A Thread opens in the same
slot.

### A live call is a strip and an inspector, not chrome

While a call runs, its block is one line: the direction, who is on the call,
the elapsed time and the last thing said. It opens the call inspector: the
headline (who, the Agent and its number, the time, the tier), the purpose, the
listen-live control, the running transcript, the controls, and a line that an
action needing approval waits for the end of the call. The inspector stays
open while the user reads elsewhere.

A live call gets no bar over every view: it would take permanent room for a
rare state, show one call where several can run, and cost about 90 px at
390 px. The way back is the strip in the conversation that started the call.
A user who closes the inspector and moves away has no live-call indicator, and
that cost is accepted.

When the call ends, the block settles in place with the outcome, then the
ended reason, the duration and the classification, the transcript behind one
control, and a small player for the recording, the first block that owns a
media player. The settled block also opens in the inspector. An inbound call
renders in the Agent's own Channel with the user, as a line that the Agent
answered its line under its standing rule, then the strip, so strangers add no
conversations.

The strip, the inspector and the settled block show the tier as a chip, and
the inspector states its meaning in one line; on an Unknown call it says that
speech is data, no tool runs, nothing is remembered and no approval is made
from it. **Drop to Unknown** is the only authority control on a live call. It
sits beside hang up and asks for confirmation, because it cannot be undone
during the call.

### The activity ring follows conversation work in its own Channel

A ring turns while an Agent has a queued or running Run in a Channel. A Run
that reflects (ADR-0010) has no Channel and turns no ring; the row still says
in words that memory is updating. A Run that waits for the user or for an
approval keeps its own look everywhere, background included.

A ring belongs to one Channel. The user's own Channel with an Agent shows that
Agent's presence wherever it works. Every other Channel shows the presence of
work in that Channel alone, and a row with several faces rings the face of the
Agent at work there. The roster and the Home thumbnails use the same rule, so
an Agent that settles memory reads as idle everywhere.

## Consequences

- A rule reads across Agents, with its next run, last run and history where
  the user looks for them.
- The presence store answers two questions: an Agent's presence, and its
  presence in one Channel.
- Above 1100 px the inspector's open state follows from the route on the two
  surfaces that own a Desk Panel; at 1100 px and below a toggle controls it.
- At 390 px a user who listens to a call cannot read the conversation at the
  same time.
