# 0040: The Workspace Mailbox starts with Agent triage

Status: accepted.

## Context

An Agent Mailbox is an Agent's own identity (ADR-0019). A Person or team
also needs an address for its Workspace. A shared mailbox needs one store
and clear access rules. Mail filters separate junk from mail before a
reader acts. An Agent needs the same boundary before it reads untrusted
mail.

The Person creates a Workspace Mailbox in Settings and reads it in the
Mail place of the Product App.

Not built: saved drafts.

## Decision

### One Workspace Mailbox uses a store over the mail transport

Each Workspace has one Workspace Mailbox on the Org's mail domain, with at
most one mailbox record. Settings takes the local part and uses the Org
mail domain Connection to create the mailbox. The Address Ledger reserves
the address across Agent Mailboxes and Workspace Mailboxes. The credential
is a Workspace secret.

The active Chief of Staff holds a System default Workspace Mailbox Grant.
A change of Chief of Staff moves the default Grant. Manual Grants stay.
The owner of a Private Workspace or a Manager of a Shared Workspace grants
access to another active Agent. The Grant names the Workspace Mailbox
record, whose primary key is the Workspace id. The designation grants no
power by itself (ADR-0022).

A store interface holds messages, threads, folders, labels and flags. Its
conformance suite checks each operation. The daemon implements the store
over IMAP on an external mail host through the transport of ADR-0019. It
uses the same sessions as Agent Mailboxes. The suite runs with the fake
transport and with the opt-in GreenMail tests. The daemon is not a mail
server. An Embedder supplies `WorkspaceMailHost` to the mailbox desk. Its
check operation validates a request and lists the addresses the host holds.
It declares whether it can create a mailbox without a supplied password.
Its provision and open operations return `WorkspaceMailStore`. Provisioning
reserves the Workspace Mailbox record before it creates the mailbox and
releases the reservation when a failed create leaves the address free.
`ImapWorkspaceMailHost` supplies the daemon's implementation and holds the
mail host, transport and secrets. The routes and Agent tools use the store
through the desk. They need no IMAP login from another implementation.
Separate read and write interfaces keep each concern small. The daemon's
write store sends through SMTP. The transport saves a Sent copy with IMAP
APPEND unless the host already holds that Message-ID. It uses the host's
special-use Sent folder or its Sent name. The Outgoing Cap applies to the
Workspace Mailbox with a daily count on its record.

### A spam verdict comes before any Agent read

Each new message gets a verdict with a score, action and reasons through a
verdict seam. The daemon reads the mail host's verdict from spam headers
and its Junk folder. The Trust Tier applies after the verdict. Only then
does the Chief of Staff read the message. It records an outcome: handled
with a note, replied, assigned, or needs a Person. The last enters the
Needs-You Queue. The verdict seam reads the host's spam headers and
special-use Junk folder. It also recognizes the Junk folder name. The
collector and direct Agent Mailbox reads compute the verdict first, apply
the sender's Trust Tier, and store both in one write before a read or
Wake-up. A source or storage failure stops the pass without advancing the
cursor. The next poll retries. Workspace Mailbox reads use the current
active Chief of Staff's Trust List. The designation is read at each
assessment. Without an active Chief of Staff, the Trust Tier is Unknown.
An Agent that reads under a Workspace Mailbox Grant uses its own Trust
List. Each tool checks for a live Grant. A revocation stops the next
dispatch, including a send that waits for an Approval. The stored record
uses the Message-ID header within its Workspace and mailbox, so a folder
move or UIDVALIDITY change keeps the assessment key. Mail without that
header uses its IMAP id. The host's Rspamd result supplies its score and
weighted symbols as reasons. Mail bodies stay at the host.

Junk goes to the Junk folder at the score threshold. Spam and Not spam
feed corrections through the verdict seam. The daemon moves the message
into or out of the host's Junk folder. Workspace and Org allow and block
lists apply, with the Org block list first. The Org has one sensitivity:
low, normal or high. Malware and high-confidence phishing go to a
quarantine for Administrators.

A model's second opinion on uncertain mail is one weighted input to the
verdict. It never decides alone. Score-only mode records verdicts and
moves nothing. The Workspace Mailbox stores its mode: score-only or move
to Junk. The API and Mail place show the stored score, action and reasons
on each message. Score-only mode moves nothing. The model second opinion
follows ADR-0046. The Org sensitivity selects a score threshold: low at
15.0 points, normal at 8.0 points and high at 4.0 points. The low and high
values are the Junk and Uncertain Spam Band boundaries. Normal is within
the Uncertain band. The host's Junk and Reject actions also mean Junk. The
verdict keeps the host action and the computed Junk decision. The Mail
Opinion stays a bounded input under ADR-0046.

Lists match an exact address or domain without case. A domain entry does
not match subdomains. The Org block list takes priority, then the Org
allow list, then the Workspace block list and allow list. A Person's mark
applies to that Message-ID and overrides list and score decisions. It does
not release quarantine. A host move returns its assigned IMAP id. The
message decision follows this id when the message has no Message-ID
header. Explicit corrections move mail even in score-only mode; automatic
classification in that mode moves nothing.

The host's malware symbols and high-confidence phishing symbols select
quarantine before a model call or list decision. Phishing needs a host
score of at least 15.0 points. Quarantine is a read and write
restriction, independent of folder location and scoring mode. Lists,
search, direct message reads, threads, attachments, message writes and
Agent intake enforce it. Only Administrator routes read quarantine
content or release a message. The quarantine read of a Workspace Mailbox
message goes through `WorkspaceMailHost`, and of an Agent Mailbox message
through the Embedder's `Mailboxes`. Release and its Org Audit Log entry share
one storage transaction. The release keeps a durable record so the same
host evidence does not quarantine that message again.

### The Chief of Staff handles mail within rules

The Person sets what the Chief of Staff can handle alone. It files,
answers, schedules or assigns within those rules. Delegate everything
gives it mail even while the Person is active. The Mail place in the
Product App shows Needs you, All mail, folders, Junk and search. Each
message shows its Mail Triage outcome and note. The Mobile App has no
Mail place. A Person opens a message and
reads its thread as text. The host's labels and flags travel with each
message. Opening the message or its thread marks the displayed messages
as seen. A Person composes, replies, replies to all and forwards mail
with Files. The composer uses plain text. A forward includes the
original text and attachments. A reply keeps the reference headers.
Reply all excludes the mailbox's own address. A Person uploads a File
and names its id in the send request. Received attachments use the same
passive content types as File downloads. The Person creates, renames and
deletes folders, moves mail, and applies or removes labels as IMAP
keywords. System folders cannot be renamed or deleted. Deleting a folder
first moves its mail to Trash with its flags and labels. Deleting a
message moves it to Trash. Deleting a message from Trash removes it for
good. The store uses the host's special-use Trash folder, or the folder
named Trash. It creates Trash when neither exists. Folder deletion
refuses a folder that contains quarantined mail before it moves any
message. Not built: saved drafts.

A Mail Triage outcome belongs to one message in one Workspace. Its key
is the Message-ID header, or the host id when the header is absent, as
for the Spam Verdict. It holds handled, replied, assigned or needs_you,
a note of at most 2000 characters, the Agent or System author, the recorded
time and, for assigned, the assignee Agent. The record also keeps the host
id for its Mail place link. The existing mail move path updates that id. A new record replaces the
previous outcome. An assigned outcome names an active Agent of the same
Workspace with a live Workspace Mailbox Grant, or the message's Mail
Assignments to current members of the Shared Workspace.

The triage poll reads INBOX in UID order, at most 50 messages per poll.
Its cursor is separate from the calendar intake cursor. Mailbox provisioning
and connection set the cursor to the current INBOX head. A poll without a
cursor sets it to the head and reads no mail. A changed UIDVALIDITY resets
the cursor to the current head. The Spam Verdict and Trust Tier are stored before triage. Junk
verdicts and quarantine get no outcome, also in score-only mode.

Mail Triage Rules are an ordered list for each Workspace. An entry matches
an exact sender address or exact domain without case; it does not match
subdomains. The first match wins. A needs_you rule, shown as Always show
me, records a System outcome whose note names the rule and starts no Run.
An agent rule lets the Chief of Staff handle mail alone. The Owner of a
Private Workspace or a Manager of a Shared Workspace replaces these rules.

In a Private Workspace, an active Person receives a System needs_you
outcome unless Delegate everything is on or an agent rule matches. Activity
uses the Person's existing hold_until check. A Shared Workspace has no
single Person, so this check does not apply. Without an active Chief of
Staff, the System records needs_you with a note. The Chief of Staff needs
a live Workspace Mailbox Grant.

Otherwise, a System Event Subscription delivers a Wake-up through the
existing mail Incoming Event path. The Agent reads the message under its
Trust Tier with the Workspace Mailbox tool, uses existing tools to act,
and records one outcome through workspace_mail_triage. Any active Agent
with a live Workspace Mailbox Grant can record an outcome. If a Run ends
or fails without one, the daemon records needs_you with the System note
“The Chief of Staff recorded no outcome.” A pending Mail Triage row holds
the message key, host id and Wake-up id. Run completion checks only its
pending rows. Each poll checks at most 50 pending rows and moves its repair
cursor forward. An outcome or a terminal Run removes the pending row. The
poll repairs a missed completion event without a scan of Run history.

Only open needs_you outcomes enter the Private Workspace Person's Needs-You
Queue. Each item opens its message in the Mail place. The Person selects
Done to resolve it. A successful reply, forward, move or delete also resolves
it. The outcome keeps its first resolved time. A resolve publishes the same
Mail Triage event that updates the queue and Notifications. The queue and
Needs you view read bounded pages of open outcomes, newest first. Both store
baselines index these rows. The Needs you view checks message visibility
and quarantine and skips mail that the host no longer holds. Replacing the
outcome changes that same derived queue. Shared Workspace outcomes mark
the message and add no queue item. The existing Needs-You event path is
the only Notification path for mail.

A Person's own send needs no Approval. Agents do not use the Person's send
path.

Agents reply from their own Agent Mailbox with the Person in Cc by
default. Sending as the Workspace Mailbox needs an Approval for each
message until a standing Grant allows that kind of mail. The broker offers
Workspace Mailbox read, send and reply tools only under a Workspace
Mailbox Grant. A send or reply as the Workspace Mailbox parks the Run on a
tool action Request for an Approval. The card shows the from, to, Cc,
subject and body. Recipient-domain Allow Rules on the Grant approve a
class of sends in advance. Every recipient, including Cc and Bcc, must
match a rule. The Outgoing Cap applies to all sends from the mailbox. An
Agent Mailbox send can name the Workspace Mailbox message it answers. The
daemon reads the original under the Agent's Grant, keeps In-Reply-To and
References, uses a Re: subject and puts the Workspace Mailbox in Cc. The
Agent Mailbox's existing send rules apply.

### Visibility limits each human read and message write

A Shared Workspace Mailbox has one visibility level: All mail, Escalations
or Agent only. The Org sets the highest level a Manager can select. Each
human read writes an Org Audit Log entry with the Person, Workspace and
message identity or search query and reason. At Escalations, each search
needs a stated reason and returns only messages with a needs_you outcome
or a live Mail Assignment to the reader. The reason
records intent and gives no extra access. Global Search omits Escalations
mail unless the request has a stated reason. Workspace roles still apply.
Readers read, Responders also send, and Managers change the selected
level. A Person send records the Person author and sent message identity
in the Org Audit Log. A Manager reads the mailbox entries through the
Shared Workspace route. An Administrator reads them in the Org Audit Log.

The mailbox stores its selected level. Each human read uses the lower of
that level and the current Shared Workspace Limit. All mail permits every
message. Escalations permits messages whose Mail Triage outcome is
needs_you and messages with an assigned or escalated Mail Assignment to
the reader. Agent only hides messages and keeps mailbox settings visible.
Lists, threads, message reads, attachments, reply preparation and invitation
responses from Mail use the same message check. Every route that takes a
message id opens the store and reads the message through one visibility and
quarantine check before it reads or changes the message. A hidden message
returns the same 404 answer as a message that does not exist. Assignment
links use this check too.

A needs_you outcome opens only that message, not the rest of its thread.
Agents read under their Grants at every visibility level.

### Assignments keep the mail in its Workspace

A Mail Assignment belongs to the Shared Workspace that holds the mail.
It records the Mail Triage key and host id, the assignee Person and their
Private Workspace, the role decide, approve or fyi, the state assigned,
handled, escalated or timed_out, the assigning Agent, the conversation
Channel, the summary, the due time, the Chief of Staff's note and the
created and changed times. Both storage baselines index open assignments
by Workspace and due time, and by assignee.

A Team Agent with a live Workspace Mailbox Grant uses
`workspace_mail_triage` with the assigned outcome, current members' Person
ids, a role and a short summary. Each member gets one Mail Assignment and
one conversation in the Shared Workspace. The Shared Workspace and the
member's Private Workspace participate. The message shows
"<Agent>, for <Shared Workspace>", the role, the summary and a link to
the message in the Mail place. It contains no copy of the mail text.
Every human link uses `open_message`, including its visibility and
quarantine checks. At Escalations, an assigned or escalated Mail Assignment
permits its assignee to read that message. At Agent only the link returns
the same 404 as any hidden message.

For decide and approve, cross-Workspace delivery starts a Delegation to
the member's Chief of Staff. It carries the assigning Run's Origin and
hop count. Client activity does not hold an assignment. Each Run spends
from its own Workspace. The Chief of Staff uses `assignment_update` with
a short note to record handled or escalated. A handled answer returns
through the Origin. An escalated assignment makes one derived item in
its Person's Needs-You Queue that opens the assignment conversation. The
Person's answer or Done closes it. A Run with no update leaves the
assignment assigned.

An FYI assignment posts the conversation message and starts no Run. It
adds no Needs-You item and makes no Notification. Home derives the day's
FYI feed entries from open assignment rows. The Person dismisses an FYI
entry to mark it handled. Only a Person's own Needs-You Queue makes
Notifications.

There is no assignment limit by default. A Manager sets or clears the
open assignment limit per member and sets the timeout in minutes, whose
default is 1440. The store checks the limit in the same transaction that
creates the assignment rows. An over-limit tool call tells the Agent to
select another member.

The daemon reads at most 50 overdue assigned rows per sweep through the
due-time index. It marks each timed_out and assigns the same message and
role to the next current Responder or Manager who has not had that message
and is under the limit, ordered by Person id. With no such member, it
records needs_you on the message with a System note. Managers and
Responders list the Shared Workspace's assignments and their states. The
Mail place shows a message's assignments, and Managers set their limit
and timeout in Workspace Mailbox settings.

## Consequences

- The mail host holds one message store. Readers do not get separate copies.
- Spam verdicts and Trust Tiers answer different questions in a fixed order.
- The Chief of Staff's designation gives no right to send as the Workspace.
- Assignments can protect attention without opening the full mailbox.
