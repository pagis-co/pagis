# 0042: Each Workspace holds a calendar

Status: accepted.

## Context

A Workspace needs its own meeting times and invitations. Agents can arrange
a meeting only when they can check availability and answer invitations.
Mail-based calendar standards let people on separate systems take part in
the same meeting without sharing a calendar server.

Each Workspace holds a Calendar. The Person reads its Events in the
Calendar place of the Product App. The Owner changes a Private Workspace
Calendar. Readers read a Shared Workspace Calendar. Responders and Managers
create, change and delete its Events.

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
complete Event or series. The Event view and editor show “Created by”
with the name of its Person or Agent author. The editor has recurrence
presets and a custom RRULE,
attendees and their participation status, reminders and an HTTPS meeting
link. Delete this occurrence adds an EXDATE. Other changes to one occurrence
are not built. The place works at phone width. The Mobile App shows this
Product App place (ADR-0032).

### Member invitations keep one owning Workspace

An enabled Person's login email identifies an inside attendee. An Event
stays in its owning Shared Workspace. A member's Private Calendar also
shows the Shared Workspace Events where that Person is an attendee. Each
occurrence names the owning Workspace and shows its name. The view has no
edit controls. The member can accept, answer maybe or decline on their own
attendee row through the owner Workspace. Each read and response checks
current membership. Removal from the Shared Workspace removes this access.
Readers see no Event edit controls in the Shared Workspace Calendar.

Member invitations need no Workspace Mailbox. Outside attendees receive
iMIP through the Shared Workspace Mailbox. Without a Workspace Mailbox,
an Event with outside attendees is refused.

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
Mailbox store. Each poll reads at most 50 messages per folder above its
calendar intake cursor, in UID order. The Workspace Mailbox record store
keeps one cursor per folder: UIDVALIDITY and the highest UID already read.
An initial cursor reads existing mail from UID 1. A changed UIDVALIDITY
starts the cursor at the folder's current head, as the Agent Mailbox
collector does (ADR-0019). It does not apply the mail already there.
The cursor moves only after every message in its batch is applied. A
failed batch repeats from the stored cursor.

Junk is readable mail and its invitations apply to the Calendar. A move
from INBOX to Junk assigns a new UID in Junk. Its separate cursor reads
the moved message. Scheduling UID, SEQUENCE and occurrence state make
repeated application safe and keep an attendee's response. Quarantine
prevents Calendar intake and invitation responses. An Administrator read
of quarantine does not apply an invitation. The intake layer wraps every
supplied store. It also applies calendar messages when a Person or an Agent
opens a message or a thread. The IMAP store does not own calendar intake.
Reading a message does not mark it seen.
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
A Person's Free/Busy also includes timed Events in Shared Workspaces where
the Person is a member and an attendee who has not declined. These Events
stay in the Shared Workspace. A colleague is in a meeting while a busy
interval of this combined Calendar read contains the current time. The server computes this status on each read;
it takes priority over active client input.
Event details follow Disclosure Rules (ADR-0043). An availability answer
does not open the Event, mail or memory that explains it.

### Agents use calendar tools under Grants

A Calendar Grant gives one active Agent access to the Calendar of its
Workspace. Its resource id is the Workspace id. The Owner of a Private
Workspace or a Manager of a Shared Workspace adds or revokes it in Grant
settings. It has no Allow Rules. Its read tool is `calendar_list_events`
for occurrences that overlap a half-open range. The range read returns at
most 1000 occurrences from the Agent's own Workspace Calendar. The
Free/Busy tool `calendar_free_busy` and the Event read tool `calendar_read`
need no Grant. The change tools are
`calendar_create_event`,
`calendar_update_event`, `calendar_delete_event` and `calendar_respond`.
A response accepts, answers maybe or declines an invitation the Workspace
receives. Each created or changed Event names the Agent as its author.
Each dispatch checks the live Calendar Grant.

A tool change that sends iMIP to outside attendees also needs a live
Workspace Mailbox Grant. It parks the Run on the same tool action Approval
as a Workspace Mailbox send. The card shows the recipients and the Event
summary. Recipient-domain Allow Rules on the Workspace Mailbox Grant
approve a send in advance only when they cover every recipient. Revocation
stops dispatch, including a change that waits for an Approval (ADR-0040).
The Free/Busy tool returns intervals only. The Event and range read tools
check Disclosure Rules for a colleague Run.

The Chief of Staff schedules a meeting by mail for its Person. It reads
Free/Busy and the Calendar range, then proposes times from its own Agent
Mailbox with the Person in Cc. The outside person's reply reaches the
Workspace Mailbox. Mail Triage wakes the Chief of Staff in its DM Channel.
It reads the reply and thread under its Workspace Mailbox Grant and creates
the Event at the selected time under its Calendar Grant. The invitation
uses the Workspace Mailbox as ORGANIZER and needs its send Approval or
Allow Rules. The Chief of Staff records a handled Mail Triage outcome with
a note and reports the meeting in its DM Channel.

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
