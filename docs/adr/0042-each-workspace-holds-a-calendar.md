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
other attendees.

The `icalendar` library reads and writes iCalendar. The Workspace Mailbox
sends a readable text part and a `text/calendar` part with the iTIP method
REQUEST, REPLY or CANCEL. Only attendees outside the Org receive this mail.
The organizer address is the Workspace Mailbox address. Each Event keeps
its organizer, SEQUENCE and scheduling status with its data. The UID is
unique within its Workspace.

Mail intake matches UID and checks SEQUENCE before a change. A REQUEST
creates a tentative Event with the local attendee at NEEDS-ACTION. A REPLY
changes only the sender's attendee status. A CANCEL marks the Event cancelled;
it does not delete it. Cancelled Events do not block Free/Busy. Older
sequences cannot replace newer sequences. A duplicate REQUEST does not reset
an attendee's response. The sender address must match the organizer for a
REQUEST or CANCEL, or the attendee for a REPLY. This address check does not
make the sender's text trusted input for Agents.

The desk reads INBOX and the Junk folder through the supplied Workspace
Mailbox store, with opaque message ids and page tokens. It reads INBOX
before Junk so an automatic move does not hide an invitation. It reads all
pages and applies messages oldest first. Junk is readable mail and its
invitations apply to the Calendar. Quarantine prevents Calendar intake and
invitation responses. An Administrator read of quarantine does not apply
an invitation. The intake layer wraps every supplied store. It applies calendar
messages when the Person or an Agent opens a message or a thread, and when
the Calendar reads INBOX and Junk. The IMAP store does not own calendar intake.
The Workspace Mailbox record has no calendar cursor. Scheduling UID,
SEQUENCE and occurrence state make repeated reads safe. A failed read can
repeat the pass. Reading a message does not mark it seen.
Accept, maybe and decline in Calendar and Mail send a REPLY through the
Workspace Mailbox and store the attendee's PARTSTAT. These controls also
serve the Mobile App. Responses keep the organizer's SEQUENCE. Changes by
the organizer increase it. A cancellation keeps the Event as a record.
REQUEST messages use DTSTART with DTEND or DURATION. A DATE start with
neither end nor duration lasts one day. Duration days and weeks use local
calendar dates. Hours, minutes and seconds use elapsed time.

The calendar resolves TZID through the IANA zone database first, then through
the CLDR Windows zone mapping. An unknown TZID makes the message invalid.
Stored Events and sent messages use IANA names. The zone database supplies
the offsets. The calendar ignores the VTIMEZONE body.

RECURRENCE-ID identifies the original occurrence by its UTC time or all-day
date. A CANCEL for one occurrence adds that start to the series EXDATE values.
A REPLY for one occurrence is handled without a change to attendee PARTSTAT.
A REQUEST for a known series adds the original start to EXDATE and stores
the moved occurrence as a separate single Event. Its invitation holds the
series UID and RECURRENCE-ID. An unknown series does not create an occurrence.
Each occurrence record keeps its SEQUENCE. Cancelled occurrence records
prevent an older or equal-sequence REQUEST from restoring an occurrence.
A series CANCEL marks the series and its linked occurrence records cancelled.
The store keeps these records. This is not a general recurrence override model.

### Availability and event details have separate access rules

Free/Busy reads return merged busy intervals in a half-open UTC range.
Timed occurrences are busy unless their Event is cancelled. All-day Events
are free. Overlapping and touching
intervals merge. Each Org member can read Free/Busy by a Person's login
email or a Workspace Mailbox address. The answer holds start and end times
only, with no Event details. The planner shows the Free/Busy of attendees
and the Person on an hour axis, with the chosen time and conflicts.
A colleague is in a meeting while a busy interval of their Private Workspace
Calendar contains the current time. The server computes this status on each read;
it takes priority over active client input.
Event details follow Disclosure Rules (ADR-0043). An availability answer
does not open the Event, mail or memory that explains it.
Not built: Disclosure Rules.

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
