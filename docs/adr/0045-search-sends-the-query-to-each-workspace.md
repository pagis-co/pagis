# 0045: Search sends the query to each Workspace

Status: accepted.

## Context

A Person needs to find content without knowing its Workspace. Slack
cross-workspace search and Microsoft Search send one query to separate
sources. Each source checks access and the answer combines the results.
Pagis follows this pattern. Each record has one owner that decides access
(ADR-0039). Conversations between Workspaces have one stored history in
the Org's Workspace (ADR-0043).

## Decision

The Product App has one Search box in its header. The Command Palette
also opens Search with its current query. Search sends the query to each
Workspace the signed-in Person can read and merges the results, newest
first. Each result names its Workspace and content kind. Its link selects
that Workspace and opens the item. Search has no central index and stores
no copy of its results.

### Each source checks access

Search uses the same Workspace list as the Workspace switcher. A Private
Workspace permits its Person only. A Shared Workspace permits a current
Reader, Responder or Manager. Each Workspace checks this rule again before
it reads its sources. An Administrator gains no access to another Person's
Private Workspace through Search.

Each Workspace searches these sources through their existing read stores:

- Channel messages and Thread replies use one bounded full-text query per
  Workspace through Conversation Evidence Search, newest first.
  Forgotten messages and content whose source Grant is no longer live are
  excluded. Retained tool results are not Channel messages.
- Memory files use Memory Search for shared memory and each Agent's private
  memory. The memory reader applies current source Grants and Forget rules.
- Calendar Events match a case-insensitive substring in their stored title,
  description or location. Search reads master Events, without recurrence
  expansion, and limits the number returned.
- Workspace Mailbox mail uses the mail desk and the Workspace mail store's
  IMAP search. It runs only when the Workspace has a Workspace Mailbox. A
  Reader can read mail under the same rule as the Workspace mail GET routes.

Search also reads conversations between Workspaces with one bounded
full-text query in the Org's Workspace. The query selects Channels with the
Person's Private Workspace as a participant before it applies the result
limit. It then applies the conversation routes' participation check once
for each distinct Channel in the hits before it returns a hit.
The result names the Person's Private Workspace, where its conversation
link opens. Search gives no access to other records in the Org's Workspace.

### A slow Workspace does not stop the other answers

Workspace queries run concurrently. Each has a three-second time limit.
A failed Workspace or a Workspace that exceeds this limit appears by name
in the incomplete list. The other Workspaces return their results. The
Product App shows the incomplete list and a control to search again.

The Org read route is `GET /api/v1/search`. It accepts a query of 2 to 200
characters. The result limit defaults to 50 and permits 1 to 100. The limit
applies to the combined answer. Ties have a stable order by Workspace id
and item URL. No search table or shared copy of source content is added.

## Consequences

- A Person searches readable Workspaces and participating conversations
  with one query.
- Access removal takes effect at the owning source.
- Search can return partial results when a Workspace is unavailable.
- Search depends on each source's availability and search rules.
