# 0040: The Workspace Mailbox starts with Agent triage

Status: accepted.

## Context

An Agent Mailbox is an Agent's own identity (ADR-0019). A Person or team also
needs an address for its Workspace. A shared mailbox needs one store and
clear access rules. Mail filters separate junk from mail before a reader
acts. An Agent needs the same boundary before it reads untrusted mail.

The Person creates a Workspace Mailbox in Settings and reads it in the
Mail place of the Product App.

Not built: Agent triage, Junk handling, verdict filtering, quarantine,
Needs you items, Agent Grants, Shared Workspace visibility levels,
assignments and saved drafts.

## Decision

### One Workspace Mailbox uses a store over the mail transport

Each Workspace has one Workspace Mailbox on the Org's mail domain, with at
most one mailbox record. Settings takes the local part and uses the Org
mail domain Connection to create the mailbox. The Address Ledger reserves
the address across Agent Mailboxes and Workspace Mailboxes. The credential
is a Workspace secret.

Not built: the default Grant to the Chief of Staff and Grants to other
Agents. The designation grants no power (ADR-0022).

A store interface holds messages, threads, folders, labels and flags. Its
conformance suite checks each operation. The daemon implements the store
over IMAP on an external mail host through the transport of ADR-0019. It
uses the same sessions as Agent Mailboxes. The suite runs with the fake
transport and with the opt-in GreenMail tests. The daemon is not a mail
server. Separate read and write interfaces keep each concern small. The
write store sends through SMTP. The transport saves a Sent copy with IMAP
APPEND unless the host already holds that Message-ID. It uses the host's
special-use Sent folder or its Sent name. The Outgoing Cap applies to the
Workspace Mailbox with a daily count on its record.

### A spam verdict comes before any Agent read

Each new message gets a verdict with a score, action and reasons through a
verdict seam. The daemon reads the mail host's verdict from spam headers and
its Junk folder. The Trust Tier applies after the verdict. Only then does
the Chief of Staff read the message. It records an outcome: handled with a
note, replied, assigned, or needs a Person. The last enters the Needs-You
Queue.
Not built: this read order and outcome record.

Junk goes to the Junk folder at the score threshold. Spam and Not spam
feed corrections through the verdict seam. The daemon moves the message
into or out of the host's Junk folder. Workspace and Org allow and block
lists apply, with the Org block list first. The Org has one sensitivity:
low, normal or high. Malware and high-confidence phishing go to a quarantine
for Administrators.

A model's second opinion on uncertain mail is one weighted input to the
verdict. It never decides alone. Score-only mode records verdicts and moves
nothing.
Not built: Junk, corrections, lists, sensitivity, quarantine, model
second opinion and score-only mode.

### The Chief of Staff handles mail within rules

The Person sets what the Chief of Staff can handle alone. It files, answers,
schedules or assigns within those rules. Delegate everything gives it mail
even while the Person is active. Mail in the Product App and Mobile App
shows Needs you, All mail, folders, Junk and search. A Person can compose,
reply, forward and attach Files. Handled mail shows the Agent's note.
The Mail place in the Product App shows an empty Needs you view, All mail,
folders and search. A Person opens a message and reads its thread as text.
The host's labels and flags travel with each message. Opening the message
or its thread marks the displayed messages as seen. A Person composes,
replies, replies to all and forwards mail with Files. The composer uses
plain text. A forward includes the original text and attachments. A reply
keeps the reference headers. Reply all excludes the mailbox's own address.
A Person uploads a File and names its id in the send request. Received
attachments use the same passive content types as File downloads.
The Person creates, renames and deletes folders, moves mail, and applies
or removes labels as IMAP keywords. System folders cannot be renamed or
deleted.
Not built: triage, Needs you items, the Mobile App Mail place, Junk
handling, saved drafts and Agent notes.

A Person's own send needs no Approval. Agents do not use the Person's
send path.

Agents reply from their own Agent Mailbox with the Person in Cc by default.
Sending as the Workspace Mailbox needs an Approval for each message until a
standing Grant allows that kind of mail.
Not built: these Workspace Mailbox
send rules.

### Visibility limits each human read

A Shared Workspace Mailbox has one visibility level: All mail, Escalations
or Agent only. The Org sets the highest level a Manager can select. Each
human read writes an audit entry. At Escalations, a Responder can search all
mail with a stated reason. Workspace roles still apply.
Not built: these
visibility rules and audited reads.

### Assignments use Delegation and keep the mail in its Workspace

A Shared Workspace Agent assigns by Delegation to one or more Persons'
Chiefs of Staff. Each assignment has a role: decide, approve or FYI. The
assignment conversation stays in the Shared Workspace. A Chief of Staff
handles it within its rules or puts it in its Person's Needs-You Queue.
Only that Person's queue makes Notifications.

The Shared Workspace tracks assigned, handled, escalated and timed out
assignments. A timeout reassigns the work. There is no assignment limit by
default; a Manager can set one. FYI adds a feed entry and starts no Run.
Below All mail, an assignment carries a summary and a link, not the mail.
The link passes the visibility check.
Not built: assignments and their
Delegation across Workspaces.

## Consequences

- The mail host holds one message store. Readers do not get separate copies.
- Spam verdicts and Trust Tiers answer different questions in a fixed order.
- The Chief of Staff's designation gives no right to send as the Workspace.
- Assignments can protect attention without opening the full mailbox.
