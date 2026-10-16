# 0042: Each Workspace holds a calendar

Status: accepted.

## Context

A Workspace needs its own meeting times and invitations. Agents can arrange
a meeting only when they can check availability and answer invitations.
Mail-based calendar standards let people on separate systems take part in
the same meeting without sharing a calendar server.

Each Workspace holds a Calendar. The Person reads and changes its Events
in the Calendar place of the Product App.

## Decision

### Each Workspace has a calendar store

Each Workspace has a Calendar store interface in the core. SQLite and
Postgres implement it. A stored conformance suite runs against both stores.
It covers create, read, update, delete, range reads, attendees, reminders,
all-day Events, time zones, recurrence across a DST change, EXDATE and the
Workspace boundary. Each Event has one owning Workspace and names its
author. Access follows that owner (ADR-0039).

The store holds one master Event. Each write records its first occurrence
start and its last occurrence end in UTC milliseconds. An endless series
has no end bound. The range query excludes Events outside these bounds
before it reads their data, with an index on Workspace and first start.
The author uses the checked Person, Agent and system columns of the other
records. Calendar input and range errors use the core Calendar error type.
A finite series must fit the recurrence library's bounded expansion; a
series that exceeds it is refused with a validation error. Range reads return at most 1000
occurrences that overlap the half-open range. The `rrule` library expands
RFC 5545 recurrence in the Event's IANA time zone. EXDATE removes an
occurrence. Timed Events use local start and end times; all-day Events use
a date range with an exclusive end. Reminders are minutes before the start,
returned with each occurrence. Not built: reminder delivery as Notifications.

The Calendar place shows day, week and month views. Day and week show an
hour axis, an all-day row and timed Events placed by start and duration.
Overlapping Events use separate columns. The week scrolls inside its own
container at phone width. Month shows up to three Event chips in each day
cell and opens the day from the remaining count. Its editor changes the
complete Event or series, with recurrence presets and a custom RRULE,
attendees and their participation status, reminders and an HTTPS meeting
link. Delete this occurrence adds an EXDATE. Other changes to one occurrence
are not built. The place works at phone width. The Mobile App shows this
Product App place (ADR-0032).

### Invitations travel through the Workspace Mailbox

Invitations, replies and cancellations to and from people outside the Org
travel as iCalendar messages over iMIP (RFC 6047). They use the Workspace
Mailbox and its transport (ADR-0040). The calendar reads invitation state
from these messages. It does not require a calendar server shared with the
other attendees. Not built: iMIP invitation handling.

### Availability and event details have separate access rules

Free/busy is readable inside the Org. Event details follow Disclosure Rules
(ADR-0043). An availability answer does not open the event, mail or memory
that explains it. Not built: free/busy reads and Disclosure Rules.

### Agents use calendar tools under Grants

An Agent needs a Grant to create or change events and answer invitations.
It can arrange meetings by mail through the Workspace Mailbox. Calendar
access does not give mail send authority. Mail Approvals still apply
(ADR-0040). Not built: calendar Grants and tools.

An Event can hold a free HTTPS meeting link to a service the Person chooses.
The store and API keep the link, and the Calendar place opens it.
No video meeting service is built. The calendar
does not serve third-party calendar clients over CalDAV. Booking pages are
not built.

## Consequences

- The calendar store can change without changing the Agent tools.
- Standard mail invitations reach people outside the Org.
- Availability can be shared without disclosing event details.
- A meeting link does not require a video service in Pagis.
