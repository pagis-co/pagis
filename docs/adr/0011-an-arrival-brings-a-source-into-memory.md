# 0011: An arrival brings a source into memory, and a Reflection Filter decides what reflects

Status: accepted.

## Context

A connected account holds years of records. They must reach memory without a
second memory model, a second scheduler or a graph database, and a new account
must not spend a day of model budget on mail nobody reads again.

## Decision

### Sync is an explicit choice for each resource

Sync is chosen for each resource of a Connection. Learning permission and
standing reminder permission are separate: standing permission can authorize a
derived reminder and grants no external tool effect.

An adapter owns the provider checkpoints, the pagination and the handoff from
import to live. The shared interface keeps the record types, the coverage, the
deletions and the loss of access. Source dates and validity dates stay apart
from processing time, and a duplicate or stale pass cannot replace a newer
revision.

### An arrival fails alone

One arrival the source cannot deliver does not stop the others in the pass. A
failure records the reason, the attempt count and a retry time on the
acquisition row. The first failure holds the arrival for two minutes, and each
further failure doubles the wait up to one hour. A pass reads only arrivals
whose retry time has passed.

A permanent rejection, or the fifth attempt, skips the arrival: the row is
acknowledged as `skipped` with its last failure. A source rejects an item when
the same request gives the same answer, such as a thread larger than the
reader accepts. The sync status counts skipped arrivals apart from appended
ones. A lost authorization still stops the whole pass, because every other
arrival then fails the same way.

### Acquisition completes the metadata of a version

Every upsert version keeps metadata. A version with none is incomplete, and
the collect pass asks the provider for the metadata of the resource's
incomplete versions in bounded batches, oldest first. It asks for one item at
a time, so a lost item completes with empty metadata and the pass moves on. A
transient failure stops the pass, and the next one continues. A deletion holds
no metadata and is never incomplete. The sync status counts incomplete
versions. This is a property of acquisition, so any cause of an incomplete
version (a provider gap, a new signal, an interrupted scan) is completed the
same way.

### A Reflection Filter decides what reflects

A **Reflection Filter** is the ordered rule list of one sync resource, and it
applies to live and historical arrivals alike. A rule has a verdict, reflect
or skip, and one or more conditions, which must all hold. Two rules with the
same verdict act as OR. The first rule that holds gives the verdict, and a
default verdict closes the list. A condition on a signal the page lacks is
false.

**Page Signals** are cheap facts about one Subject Page, computed from stored
metadata of the newest acquired version of each item of the page. A label the
owner removed or an item the source deleted leaves no signal. No signal comes
from a model, and no signal reads the source words.

The **Signal Catalogue** is what one resource offers the filter editor: each
signal with its value kind and, for a choice or a tag set, its options. The
daemon validates a filter against it and refuses an unknown signal, an
operator the kind does not allow, a value of the wrong shape, an empty rule and
an option the catalogue does not offer. The refusal names the rule and the
condition.

| Kind | Operators | Value |
| --- | --- | --- |
| Text | contains, does not contain, is, starts with | one text |
| Text | is in, is not in | a list of values |
| DateTime | is after, is before | one instant |
| DateTime | is within the last, is older than | a duration in days or hours |
| Number | is at least, is at most, is | one number |
| Boolean | is | true or false |
| Choice | is, is not | one option |
| Choice | is in | a list of options |
| TagSet | has, lacks | one option |

A text signal holds the value of every record of the page, and a text
condition holds when any value matches, ignoring case. A duration is measured
from the moment of the decision.

A filter change bumps its revision. Decisions of each revision are kept apart,
so a count belongs to one filter.

The default mail filter reflects a record the owner marked important or
starred, a thread the owner replied to, and a sender that the Trust List names
owner or trusted; the default verdict is skip. A page reflects because a rule
names it. A stranger's first mail waits until the owner stars it, replies to
it or trusts the sender.

The scan lists the mailbox without spam and trash, so such a record is an id
the mailbox does not hold: no metadata, no arrival, no rule. The catalogue
offers neither label.

A signal the store cannot read holds back the filter, not the reflection: the
page reflects. A filter saves cost and must never lose knowledge.

### Live arrivals

Acquisition appends the Timeline entries first. The filter then decides about
each page that live arrivals changed, and only the pages that reflect reach
the arrival Run; with none, no Run starts. The daemon records one decision for
each page and filter revision, with the rule and the reason, except for a page
whose signals it cannot read. A page skipped today still reflects on a later
matching live arrival.

### Historical pages reflect in batches

The **Backfill Reflection** reflects the historical Subject Pages that one
resource's filter selects. It runs inside the collect pass after live
delivery, only when the resource is caught up, at most one backfill Run for
each pass, and it stops when the daily budget is spent. There is no worker and
no flag.

It walks the Subject Pages of historical arrivals, newest arrival first, and
only subjects that have metadata. It reads pages the current filter revision
has not decided, computes the Page Signals, selects, and records the verdict,
the rule and the reason. A page whose signals cannot be read reflects, and the
pass records that verdict, so the walk moves on. A filter change decides every
page again. A completed version drops the backfill decisions of its subject
that no Run has reflected yet; a reflected page keeps its record.

Selected pages reflect in batches of at most 20 pages for each Run through the
arrival Run path, with a Wake-up built from the subject paths alone. One
resource starts at most 50 backfill Runs in 24 hours. The sync status shows
how many pages reflected, wait and failed, and whether the budget is spent.
The briefing says the pages are historical: the Run records the facts and sets
a wake-up only where the act-by moment has not passed. A later live arrival on
such a page follows the live path.

Each pass starts by releasing the pages of every backfill Run that failed or
was canceled: the page's record drops its Wake-up and counts one attempt. The
pass reads the budget after the release, so a Run that reflected nothing
spends nothing. After three failed Runs a page stops and counts as failed, and
a new filter revision gives it fresh attempts.

## Consequences

- A new account costs a bounded number of Runs a day, and the owner can watch
  the backlog.
- One rule list explains what the Agent learned from new records and from
  history.
- A lost record is visible as a skip or a failure, never as silence.
