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
name. An Agent may hold one Agent Mailbox (ADR-0019). A Workspace can hold one
Workspace Number. ADR-0041 defines its ownership and Call routing. An Agent
does not own a telephone number.

The Workspace owns Agents, mailboxes, phone numbers and Credentials. Archiving
an Agent keeps its identity and account history. Pagis never transfers or
reuses an Agent address. A Credential may be granted to another Agent, but the
Grant does not change which Agent's identity made the account, and only an
Agent can create an account with its own identity.

Every Credential records `user_supplied` or `agent_minted` (ADR-0013), and an
`agent_minted` Credential names the Run that made it. Provenance does not
change when the Agent's mailbox changes.

### The Workspace Number

The Person buys, sees and releases the Workspace Number in Settings. It
belongs to the Workspace. The Chief of Staff answers inbound Calls and
receives inbound texts (ADR-0041). A change of designation changes who
answers the next Call. Archiving an Agent does not release the number.
Calls name the Agent that holds them. Text Records name the Workspace Number.
Outbound Calls need a Workspace Number Grant (ADR-0041).
Not built: outbound texts from the Workspace Number.

### Pagis never calls an emergency number

There is no setting, override or approval that permits it. 47 U.S.C.
227(b)(1)(A)(i) makes a call to an emergency line with an artificial or
prerecorded voice unlawful, and a carrier routes an emergency call only from a
caller id enabled for it.

The telephony library contains one emergency-number predicate. It has no
setting, override or approval. Keypad tones are not filtered: a tone does
not start a Call. The outbound Call tool and the dial path both refuse
emergency numbers (ADR-0041).

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
`Ready(display_name, email_address?, phone_number?)` or `Unavailable(reason,
missing_fields)`. A name-only or username-only signup needs no mailbox; an
email needs an available Agent Mailbox; a phone field is unavailable. The
reasons are `agent_inactive`, `email_not_provisioned`, `mailbox_unavailable`,
`phone_unavailable` and `unsupported_identity_field`. A legal name, a birth
date, a home address or a government identifier is unsupported: Pagis blocks
the signup and never invents a value or uses the user's data. The fill path
reports the reason and tries no other address or number, and never takes the
username of a user-supplied Credential as the Agent's address.
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
