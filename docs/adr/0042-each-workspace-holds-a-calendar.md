# 0042: Each Workspace holds a calendar

Status: accepted.

## Context

A Workspace needs its own meeting times and invitations. Agents can arrange
a meeting only when they can check availability and answer invitations.
Mail-based calendar standards let people on separate systems take part in
the same meeting without sharing a calendar server.

Not built: a Workspace calendar, its store, calendar tools and invitations.

## Decision

### Each Workspace has a calendar store

Private and Shared Workspaces each have a calendar store. Its interface
covers events, attendees, recurrence, reminders and free/busy. A conformance
suite checks each of these parts for every store implementation. Each event
has one owning Workspace and access follows that owner (ADR-0039).
Not built: the calendar store and its conformance suite.

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

An event can hold a free meeting link to a service the Person chooses.
Not built: this link field. No video meeting service is built. The calendar
does not serve third-party calendar clients over CalDAV. Booking pages are
not built.

## Consequences

- The calendar store can change without changing the Agent tools.
- Standard mail invitations reach people outside the Org.
- Availability can be shared without disclosing event details.
- A meeting link does not require a video service in Pagis.
