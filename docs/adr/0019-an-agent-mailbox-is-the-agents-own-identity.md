# 0019: An Agent Mailbox is the Agent's own identity

Status: accepted.

## Context

An Agent needs a mailbox that is its own identity on every installation,
including a laptop behind NAT with no public address, so inbound mail must be
pulled or pushed over a connection the daemon starts. Of the three provider
shapes (an inbox API with a webhook or vendor socket, a routing service with no
store, an ordinary mail host with IMAP, SMTP and sometimes a mailbox API), only
the mail host gives an own identity that works behind NAT without a vendor
seam.

## Decision

### A Mailbox Provider is a Connection with two seams

`pagis-mail` holds both.

- **`MailboxHost`** creates and deletes a mailbox on the Connection's domain
  with the host API key, an installation secret. There is an API host, and a
  manual host whose create records the address and password the user typed and
  whose delete forgets the record and tells the user to delete the mailbox at
  the host.
- **`MailTransport`** reads over IMAP, waits with IDLE and sends over SMTP with
  the mailbox's own password, under the mailbox's secret name. The API key
  never enters the transport path.

The Connection stores the provider, the domain, the IMAP and SMTP endpoints
and the alias. An API Connection is connected once its key lists the domain's
mailboxes; a manual one is connected at creation, and each mailbox proves
itself at its first IMAP login. On create the daemon generates the password,
sends it to the host, stores it and never shows it. The first login after a
create retries with backoff for about three minutes, because hosts propagate
slowly.

The transport declares its capabilities: idle, an outgoing cap the host
enforces, mailbox deletion and password reset, and the Connection page shows
what is absent. Any IMAP and SMTP host works through the manual host. An inbox
made outside Pagis and assigned to an Agent must be dedicated to it, and the
collector cursor starts at assignment, so earlier mail never wakes the Agent.

The Mailbox Provider Connection is an Installation Connection in the Org's
Workspace (ADR-0023): one mail domain for the installation, set up by an
Administrator as the `connection` part of the host's catalog entry (ADR-0024).
The mailboxes stay per Workspace, with passwords sealed under names that carry
the Workspace. A person makes, resets and deletes their own Agents' mailboxes
on the product port, and the daemon reads the mail domain from the Org's
Workspace for each act. The Address Ledger is installation-wide.

### A mailbox has five states and one record

An Agent Mailbox is one record, keyed by Agent id and unique for each Agent
while not deleted: the address, the Connection id, the state and its reason,
the Outgoing Cap, the collector cursor, and the created and deleted times. The
Agent's address in the API is a read-through view of it.

- **provisioning**: the ledger reserved the address and the host create
  succeeded; login proof runs in the background, and proof that never comes
  moves the mailbox to unavailable with the reason.
- **active**: IDLE runs, the tools work, and sends count against the Outgoing
  Cap. A transient network failure never leaves active.
- **unavailable**: the host refused the login or reports the mailbox gone. A
  password reset (typed for a manual host, minted where the transport can)
  runs the login proof again and returns to active with the cursor unchanged.
- **dormant**: the Agent is archived. IDLE stops and sends are refused; the
  host mailbox and its mail stay. A mailbox never returns to the Workspace.
- **deleted**: a tombstone that keeps the address, the Agent id and the time,
  so the address is never reused; the password and the host mailbox are gone.

Creating an Agent with a mailbox waits for the host create alone, so a host
failure returns in the form. An Agent holds at most one mailbox at a time and
can get a new one with a new address after a delete; a mailbox never changes
its address.

The Address Ledger holds every mailbox ever made across every Mailbox Provider
Connection and is checked as the user types. The host is asked once at
create; a host conflict is a form error and drops the reservation. The local
part is suggested from the Agent name: lowercase ASCII letters, digits, dot
and hyphen; non-ASCII transliterated, other characters dropped, runs collapsed
to one dot; 1 to 64 characters; a collision appends a number. `postmaster`,
`abuse`, `admin`, `hostmaster`, `webmaster`, `noreply` and `security` are
refused.

Deletion is the user's act alone, from the mailbox panel, on an Agent in any
state, confirmed by typing the address. The daemon deletes the host mailbox
where it can, destroys the password, stops IDLE and writes the tombstone. Pagis
keeps no copy of a message body, so the mail goes with the mailbox, and the
confirmation says so. Incoming Events and Threads stay.

Removing the Connection answers a conflict with the count while any
Workspace's undeleted mailbox points at it. A host key revoked outside Pagis
makes the Connection unavailable, and its mailboxes keep working.

The Outgoing Cap is set at provisioning, 20 a day by default. The daemon
enforces it in the send tool for every host, and also sets it at the host where
the host supports it.

### One mail tool set serves both identities

Mail is one tool set, `mail__*`, with a required mailbox argument: `own`, or a
Connection alias of one of the user's mail accounts. The broker routes `own` to
the Agent's mailbox record with no Grant, and an alias to its Connection under
its Grant. The tool description lists the calling Agent's choices, and an
Agent with neither has no mail tools.

- `mail__search(mailbox, query, max, page)`: from, to, subject, body text,
  since and before; returns summaries.
- `mail__get_message(mailbox, message_id)`: headers, the text body, HTML as
  text, and attachment names and sizes.
- `mail__get_thread(mailbox, thread_id)`: messages by reference headers, oldest
  first.
- `mail__send(mailbox, to, cc, bcc, subject, body, in_reply_to)`: a reply is a
  send with a reply reference, which sets the headers and subject. There is no
  reply tool and no draft tool; the card's body is the draft.
- `mail__modify_message(mailbox, message_id, mark_read, archive, flag)`:
  reversible state. Deletion is not a tool.

Reads and modification are `Free`, and send is `Outbound`. From the own
mailbox, the card says it sends from that address, shows the subject, and
offers a recipient-domain allow rule built from the registrable domains of all
recipients, stored on the mailbox record. From a Connection, it names the
user's account and alias and offers approve once and deny. An own send over
the cap fails with `outgoing_cap_reached` before any card.

A search returns at most 25 summaries a page with a page token. A message or
thread body is capped at 32 KB of text with a marker, and a thread over 20
messages returns the newest 20 and a page token. An IMAP id is the folder and
UID; a changed validity answers `stale_id`. A dormant or unavailable own
mailbox keeps the tools, and every call answers `mailbox_unavailable` with the
reason. Mailbox reads reach the model inside the untrusted envelope.

### Inbound mail wakes the Agent under a standing rule

`mail.message_received` carries the mailbox, the message id, the thread id,
the reply reference, the sender and sender domain, the recipients, the
subject, whether there are attachments, the receive time, the trust tier,
`sender_verified` and `sender_verification`, and never a body or snippet. Its
filter takes the mailbox, senders as addresses or domains, a subject
substring, a minimum trust and replies only. For an own mailbox the provider
event id is the Message-ID header, or the folder and UID where there is none,
so a resync never wakes twice. A burst joins one pending Wake-up whose briefing
lists every message.

One collector task runs for each active own mailbox: IMAP IDLE on the inbox
with the daemon's own deadline, a full resync from the cursor at each
reconnect, and a five-minute poll where the transport has no idle. The cursor
is the folder's UID high-water mark and validity. Reconnect backoff runs from
one second to five minutes; a refused login makes the mailbox unavailable and
stops the task.

Provisioning creates one Standing Mail Rule for the Agent: the mail kind, a
filter of the own mailbox, an instruction to read the mail and act on its job,
a target of the Agent's own Channel with the user, the user's provenance,
revision one, and the Org's mail domain Connection. The user pauses, edits or
deletes it on the mailbox panel. The Agent may narrow it and never delete it.
No rule is made for a user's account alias.

Where the reply reference matches a message the Agent sent from a Run bound to
a Thread, the Wake-up lands in that Thread; otherwise in the Agent's own
Channel with the user. The daemon keeps each sent message id with its Run's
Thread for this lookup.

The sender address gives a candidate tier from the Trust List (ADR-0021):
Owner for the addresses of the person's own mail Connections and any the
person lists, Trusted for a Trusted address or domain. The candidate holds only
when the topmost Authentication-Results header with the Mailbox Provider's
authserv-id (RFC 8601) shows a DMARC pass aligned with the From domain (RFC
7489). More than one such header, more than one From address, any other
result, and every message on a manual host give Unknown. The Reflection
Filter's `sender_trust` signal follows the same rule. The account address of
the Org's mail domain is the Administrator's host sign-in and is owner for
nobody. The tier stamps the event and decides what the words are worth, never
whether the Agent wakes. Owner mail is instruction, Trusted mail is a request
whose external effect still waits for its card, and Unknown mail is data: it
may still yield a memory write or a Schedule, because the instruction is the
standing rule's, recorded as a fact with its source. A minimum-trust filter
stops unknown mail from waking the Agent.

The Agent is the watcher: the rule's Run reads the mail, writes memory, tells
the user, and creates a one-shot Schedule for later work.

### A mail block fetches its body live

A daemon-made `mail` block renders for an inbound mail that woke the Agent and
for a sent mail: one line with the direction, the counterpart, the subject and
the tier chip. It opens an inspector that fetches the headers and text body
live through the transport, so nothing is stored; it is the user's view, not
the model's. The mailbox field labels the line with the Agent's address or the
user's alias.

The Agent's page has a mailbox card: the address, the state and reason, the
Outgoing Cap with today's count, the standing rule with pause and edit, the
allow rules with remove, a password reset when unavailable, and a delete behind
a typed-address confirmation. It never shows the password or lists mail.

## Consequences

- An Agent with a mailbox wakes on every inbound mail by default, so the
  Outgoing Cap and the send card stop a mail loop; a wake never sends by itself.
- The ledger is the no-reuse guarantee, so a tombstone is never purged.
- A second API host is one more `MailboxHost` implementation.
- A host key rotation is one write, because the Org holds the key.
