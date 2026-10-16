# 0047: Shared Workspace Limits bound Agent configuration

Status: accepted.

## Context

Team products separate the configuration a Manager changes from the limits
an Administrator sets. Configuration history names each author. Access
checks use the current limits so a permission change stops the next use.
These patterns extend the ownership and role rules of ADR-0039.

## Decision

An Administrator sets Shared Workspace Limits in the Administration
Interface. The limits name permitted Plugin ids, permitted Connection ids
and the highest Mail Visibility. An absent permission list permits all. An
empty list permits none. A limits change and its Org Audit Log entry share
one transaction. The Spend Cap is a separate Workspace field. An
Administrator sets it through the Spend Cap route in the Administration
Interface. The limits route does not change the Spend Cap.

A Manager changes Agent configuration within those limits. Each Shared
Workspace configuration change creates an Agent Version with a complete
JSON snapshot of the Agent fields, live Grant records and Agent Mailbox
Approval settings. Each version names the Person author and the time.
Version numbers increase per Agent. The Agent Version and its Org Audit
Log entry share one transaction. Private Workspace Agents have no versions.

Each successful write of Agent configuration explicitly records an Agent
Version. One helper reads the Agent, its live Grants and its mailbox
settings. Agent creation, field changes, appearance changes, archive,
Grant creation, Grant revocation, and standing Approval rule changes use
the helper. Private Workspace Agents have no versions.

The store assigns the next version number in its write transaction.
Postgres locks the Agent row. SQLite starts a write transaction. A unique
constraint prevents duplicate numbers. The fake assigns numbers while it
holds its data lock. These transactions assign unique numbers for
concurrent writes.

A Responder can approve one action. Only a Manager sets standing Approval
rules. A Grant that an Approval creates also has an Agent Version with its
Person author. Only a Manager of the Workspace reads Agent Versions.
An Administrator reads each change in the Org Audit Log.

Grant creation refuses a Plugin or Connection outside the limits. The
resource-specific and Agent-specific live Grant reads apply current limits.
The settings list keeps all unrevoked Grants, so the Administration Interface
can count those outside the limits. Plugin dispatch checks both Plugin and
Connection Grants through those live reads. Lowering limits does not delete
Grants. Removing a restriction permits the existing Grants again.

Mail Visibility is ordered: Agent only, Escalations, All mail. The shared
limit check refuses a selected level above the highest level. A Manager
stores the selected level on the Workspace Mailbox. Each human read uses
the lower of the stored level and the current limit. A lower limit takes
effect without a change to the stored level (ADR-0040).

## Consequences

- The Administrator and Manager use separate write routes.
- The Spend Cap keeps its existing model-call enforcement.
- One Agent history identifies configuration and Person authors.
- Limits stop the next Grant use without a sweep that changes Grants.
