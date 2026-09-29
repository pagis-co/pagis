# 0007: Memory is Markdown files under git

Status: accepted.

## Context

Memory must be shared and private, versioned, auditable and revertible. The
options are a structured store with embeddings, plain files that the Agent
curates, or a mix.

## Decision

### Files are the source of truth

Memory is plain Markdown files under git: one private directory for each
Agent and one shared Workspace directory. The Agent curates its own files: an
index and one file for each fact. A successful reply can commit the memory
change it staged. Reflection commits the changes of a selected arrival or of
Pending Evidence (ADR-0010). The UI shows each commit in the Learning Feed
with a one-tap revert. A skill change with an external effect needs an
approval.

Git gives versions, diffs, blame and revert. Recall depends on curation, and
indexes over the same files add ranked search without moving the source of
truth (ADR-0008). Approval gates stay off the common path, so the learning
loop survives daily use.

### A memory file starts with front matter

A memory file starts with a front matter block: a `---` line, one
`key: value` line for each field, and a second `---` line. The block holds
`title`, `kind`, `aliases` and `links`, and it is the only heading form. The
Memory page and the Page Index read the title and kind from it, and a page
whose block has no title takes its file stem. The block is not compiled
truth, so its words are not entity words of the page, except `aliases`, which
exists to be matched.

The kind is one of nine words: `Person`, `Organization`, `Event`,
`Transaction`, `Project`, `Workstream`, `Concept`, `Source` and `Preference`.
`pagis-core` states the list once, and the reflection prompt reads it from
there. `docs/MEMORY-PAGES.md` says what belongs on a page of each kind. A word
outside the nine is kept by the parse, the render and the Page Index, and a
curation pass reports it, because a vocabulary that refuses an unknown word
makes the model choose a wrong one.

Acquisition writes the block from the source: `title` is the trimmed source
title on one line, and `kind` is `Source`. Reflection replaces the kind when it
reads the page. Acquisition writes no block where the source has no title, and
adds a block only where the page has none. Reflection keeps the current block
unless it supplies a new one.

`aliases` holds the other names of the page: a short form, a nickname, an
address or a handle that a source used, so "the couch" reaches the page called
"sofa". The field is a list of `[[...]]` items, written as `links` is. An alias
is an entity word of its page and is searchable as soon as it is written. It
is a name, not a path, so it is no edge of the Page Index.

### Links name paths

A link is a `[[<scope-relative path>]]` target, for example
`[[private/subjects/gmail/19ce3259c919b4de.md]]` or `[[shared/user.md]]`. It
stands in the text, or on the `links` line for targets the text does not
mention. There is no slug namespace: a page is named by its path, and
`memory_read` takes a link where it takes a path. A `[[...]]` that is not a
valid scope-relative path is prose. Plain code reads the links at each page
write, and the Page Index keeps one edge for each (ADR-0008).

### A Subject Page keeps the arrivals of one matter

An Agent's memory has one Subject Page for each source matter. Above a rule
the page keeps compiled truth, an optional `## Open questions` section and a
derived Facts table. Below the rule it keeps an append-only Timeline. Each
Timeline entry has the source reference, the source time and the source words.

Acquisition appends an entry and calls no model. Reflection can replace the
compiled truth and the Facts table, and the daemon keeps the Timeline
unchanged when it accepts the update.

The private path of a mail page is `subjects/<resource>/<thread_id>.md`, with
the message id where the thread id is missing. An unsafe path byte becomes an
underscore and two lowercase hexadecimal digits. The directory stays flat,
because the path appears in Briefs, prompts and tools, and a shard would move
every page.

`## Open questions` holds one `- ` item for each question the page still has
to answer, because an open question is not a fact and the next Run needs it.
A page with none renders no section.

The Facts table has three columns: Claim, Kind, and the Source reference of the
Timeline entry the claim rests on. It carries no confidence and no validity
dates: no reader needs them, and a model cannot fill them consistently. A date
the source states belongs in the claim. A malformed row does not hide a valid
row. A struck fact is not active; a supersede marker links it to a newer row,
and a forget marker gives the reason.

### A conversation has a Subject Page of its own

A conversation with the owner is a matter, and its page stands at
`private/subjects/conversation/<channel>.md`, one for each Channel. It has the
layout of every Subject Page, so one renderer, parse and Brief serve both, and
recall reaches it as any `subjects/` path.

The Agent writes it with the `conversation_page` tool while it replies, and
the reply's commit carries it. The tool takes the Channel from the Run, so an
Agent writes only the page of the conversation it works in. The compiled truth
says what the conversation is about, its goal and where it stands; the Facts
table holds its decisions; `## Open questions` holds what it still has to
answer. The Timeline takes one entry for each turn that settled something,
names no connection, and one Run leaves one entry. A turn that settles nothing
and leaves nothing open does not call the tool, so a settled conversation has
no file. The tool takes the kinds `Project` (work with an end) and
`Workstream` (work without one) alone. The page links to the pages of the
people and matters the conversation named.

Two causes write the page, and neither is the clock: a reply that settles
something or leaves open work, and a necessary compaction whose Continuation
Record holds open work (ADR-0009). A second session on the same work adds its
entry to the same page.

### A fact file is a Brief candidate

A fact file is one thing the Agent wrote down: front matter, a short body, and
no Subject Page sections. It is a Brief candidate under the same byte budget
and page cap as a Subject Page. Its entity words come from the file name, the
`title`, the `aliases` and the first paragraph. The Brief shows it whole, and
the budget trim drops it whole.

The index file at a scope root, `MEMORY.md`, is never a candidate: it names
the fact files of its scope and holds no fact, and a Run already reads both
indexes in full. The software notes of an Agent follow the same rule.

### One delivery pass makes one commit

A delivery pass reads up to 100 arrivals, reads each from its source on its
own, stages every Timeline append into one changeset, and commits once. The
message names the number of arrivals. Two arrivals of one thread append to the
staged page, so both reach the Timeline in the one commit.

A commit that conflicts reads the indexes and pages again, stages the appends
onto the current content, and commits again, three times in all. A commit that
still fails acknowledges nothing: the pass calls no filter, starts no Run,
reports the arrival error, and leaves the attempt counts untouched, because a
repository that cannot commit is not the fault of one arrival.

### The repository repacks itself

The store counts each Workspace's commits in memory. At the first commit after
boot and every 100 commits after, it runs `git gc --auto` in the repository
with `gc.autoDetach=false`, under the Workspace's write lock. git applies its
own thresholds, so an idle repository costs one process start every 100
commits. libgit2 has no repack, so this is the git binary; without git on the
path the daemon logs one warning for each process and keeps its loose objects.

## Consequences

- A long history and loose objects make every scan and history read more
  expensive, so the commit granularity and the repack are part of the memory
  design.
- Recall does not depend on retrieval infrastructure, and the files stay the
  source of truth however many indexes derive from them.
