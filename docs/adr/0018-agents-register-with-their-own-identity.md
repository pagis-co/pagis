# 0018: An Agent registers with its own durable identity

Status: accepted.

## Context

An Agent must register an account without an address or a phone number that
belongs to the user, and the identity and the account history must survive the
Agent's archival. Prior art separates a dedicated address from its stable
mailbox record, separates identity fields from login secrets, and stores the
acting identity with the request at event time.

## Decision

The Agent is the identity it registers external accounts with; there is no
separate registration identity record. The Agent name is its external display
name. An Agent may hold one Agent Mailbox (ADR-0019) and one Agent Phone
Number. The assignment of a number carries the authority to use it: there is
no phone Grant.

The Workspace owns Agents, mailboxes, phone numbers and Credentials. Archiving
an Agent keeps its identity and account history. Pagis never transfers or
reuses an Agent address. A Credential may be granted to another Agent, but the
Grant does not change which Agent's identity made the account, and only an
Agent can create an account with its own identity.

Every Credential records `user_supplied` or `agent_minted` (ADR-0013), and an
`agent_minted` Credential names the Run that made it. Provenance does not
change when the Agent's mailbox or phone assignment changes.

### A phone number is a desk line

The carrier Connection backs the numbers. It is an Installation Connection:
the Org owns it in the Org's Workspace (ADR-0023), and one carrier serves every
person. It supplies the credential and the provisioning API, and carries no
authority to call. An Agent Phone Number is its own record, in the Workspace of
the Agent that holds it:

```text
id
workspace_id
connection_id
e164
provider_number_id
agent_id?
status              assigned | unassigned | released
created_at
assigned_at?
```

Pagis stores the number and the provider id together or neither. The model may
know the E.164 number; it never receives the provider id, the SIP credential or
another Agent's number.

The assignment is this record's `agent_id` alone; the Agent has no pointer
back, because two pointers can disagree. A unique index on `agent_id` gives one
number for each Agent and one Agent for each number. A second unique index
holds one live record for each E.164 number in the whole installation, because
one carrier account serves every Workspace. `connection_id` is not a foreign
key, because a released record is a tombstone for its Calls and outlives the
carrier Connection. The record holds no price, because a stored price goes
stale, and no health; registration is live state of the carrier's line, which
the API reports beside the record.

- **Buy.** The daemon writes a purchase intent with an idempotency key before
  it calls the provider, and after a restart compares it with the provider's
  numbers, so it never buys twice.
- **Assign.** Fails when the Agent already holds a number. A replacement is an
  unassign and an assign: two audited acts, and the one-number rule stays in
  one place.
- **Unassign.** The Agent loses the line; the Workspace keeps and pays for the
  number and can assign it again.
- **Release.** Pagis returns the number to the carrier and the charge stops.
  The record stays as `released` for its Calls, and the number never comes
  back. The confirmation names the Agent that loses the line.

Assign, unassign and release refuse while a Call on the number is active.
Removing the carrier Connection answers `409` while any Workspace's unreleased
number points to it. Archiving an Agent unassigns its number. Calls stay with
the Agent that made or answered them.

Search shows a country and an area code or locality and each result's
capabilities. A purchase needs the carrier connected. A person buys, adopts,
assigns, unassigns and releases a number on the page of its Agent, on the
product port. An Administrator sets up the carrier in the Administration
Interface (ADR-0024). An Agent never buys, assigns or releases a number.

### Pagis never calls an emergency number

There is no setting, override or approval that permits it. 47 U.S.C.
227(b)(1)(A)(i) makes a call to an emergency line with an artificial or
prerecorded voice unlawful, and a carrier routes an emergency call only from a
caller id enabled for it.

One predicate decides, and the daemon calls it twice: in the broker before the
call tool acts, and in the dial path, so no caller goes around the broker. The
realtime brief also states the rule, as a courtesy, not a control. Keypad tones
are not filtered: a tone cannot start a call, and a filter would break phone
trees. Every path that starts or redirects a call (a transfer, a forward, a
conference, a click-to-dial) uses the same predicate.

The call tool takes an E.164 number alone. The region comes from the calling
code of the held number, as the territory the source data marks as main for
that code. A number without a leading `+` gets a prefix match, except in
Brazil, Chile and Nicaragua, where the data demands an exact match. A number
with a `+` is tested as one number and as its national form under the region,
both exactly.

A refused call returns `emergency_number_refused` with a message that the
person must dial from a telephone they hold, and writes an audit event with the
number, the Agent and the Run. It makes no Call record.

The data is libphonenumber's short-number and phone-number metadata
(Apache-2.0). `cargo xtask emergency-numbers` converts the emergency patterns
into a generated Rust table in the repository that carries the source version,
so nothing is installed at run time. A check refuses a table that the
generator would not produce.

Two limits are accepted. The statute also names hospital, poison control, fire
and police lines; they look like any number, and Pagis keeps no list, because
an incomplete safety list promises more than it gives. And a few territories
have no emergency pattern in the data, so Pagis refuses nothing there.

## Consequences

- The Agent record is the one durable identity.
- The vault and the Grant model do not depend on the mailbox provider.
- One carrier account serves every person, so two people cannot adopt the same
  number.

## Not built

- **Signup identity resolution.**
  `resolve_signup_identity(agent_id, required_fields)` answers
  `Ready(display_name, email_address?, phone_number?)` or
  `Unavailable(reason, missing_fields)`. A name-only or username-only signup
  needs no mailbox; an email needs an available Agent Mailbox; a phone needs a
  number assigned to the Agent. The reasons are `agent_inactive`,
  `email_not_provisioned`, `mailbox_unavailable`, `phone_not_assigned` and
  `unsupported_identity_field`. A legal name, a birth date, a home address or a
  government identifier is unsupported: Pagis blocks the signup and never
  invents a value or uses the user's data. The fill path reports the reason and
  tries no other address or number, and never takes the username of a
  user-supplied Credential as the Agent's address.
- **Account-creation provenance.** An Agent-made Credential records
  `identity_agent_id` and `account_creation_id`. Both origins record the exact
  ASCII account host (no scheme, port or path, not reduced to a registrable
  domain), the exact username, the creation time and, for an Agent-made
  Credential, the address used.
- **Account-creation events.** Pagis makes an account creation id and writes a
  start event before the first irreversible submission; a failed write blocks
  the submission. A finish event records `succeeded`, `failed` or `unknown`,
  and whether the evidence is agent-reported or system-verified; pixel-based
  work is agent-reported. Both events carry the creation id, the identity
  Agent, the display name, the address, the number, the domain, the username
  and the linked Credential. Recovery after a restart between the events
  appends a finish event with `unknown`.
