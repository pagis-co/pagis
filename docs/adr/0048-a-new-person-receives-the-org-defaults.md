# 0048: A new Person receives the Org defaults

Status: accepted.

## Context

An Administrator gives a new Person an address and access to teams.
Established administration interfaces derive an address from a name and
require an explicit choice when the address is taken.

## Decision

The Org stores Person Defaults: an optional Address Rule with its Mailbox
Provider, and a list of default Shared Workspaces with a role for each.
The Address Rule is one of first name, first name plus last name with a
dot, or first initial plus last name. The first and last words of the
Person's name supply the parts. Address parts use lower-case ASCII letters,
digits and hyphens. Apostrophes are removed. An unsupported or incomplete
name needs an explicit address. This is not a template language.

An Administrator reads and sets these defaults in the New People section
of the Administration Interface. A setting change and its Org Audit Log
entry are one storage transaction. The entry records the Administrator
and the selected settings. A default membership names a Shared Workspace
and a role.
The interface selects Responder when the Administrator adds a Workspace.

Account creation makes the Private Workspace and Chief of Staff through
the Workspace seed. It makes the Workspace Mailbox through the Mailbox
Desk and adds memberships through the Workspace Member store. The mailbox
and memberships record the Administrator as author. The Mailbox Desk
checks the Address Ledger and asks the `WorkspaceMailHost` to check the
request before the Person is stored. It serializes provisioning through the host call. The Address Ledger covers Agent Mailboxes and
Workspace Mailboxes.

A taken address answers with a conflict that names the address and suggests
a free alternative. The Administrator enters an explicit address to retry.
The server never selects the alternative for them. The explicit address
uses the selected Mailbox Provider's domain. The `WorkspaceMailHost` declares
whether it can create a mailbox without a supplied password. The IMAP host
needs both mail endpoints and a minted password. A manual host needs a
password for each mailbox, so it is not offered for an Address Rule. A
supplied store needs no IMAP endpoints or host key.

No Address Rule means no automatic Workspace Mailbox. Default memberships
still apply. A new Sign-In Link for an existing Person changes neither the
mailbox nor memberships. A Workspace Number is made only on request.

## Consequences

- Each new Person starts with the Org's selected resources.
- A conflict leaves the Administrator in control of the address.
- Settings and automatic creation use one mailbox provisioning path.
