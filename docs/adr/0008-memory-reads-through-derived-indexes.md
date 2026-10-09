# 0008: Memory reads through derived indexes, and a Forget purges every copy

Status: accepted.

## Context

A working memory repository holds thousands of commits and files. A page list
needs the time and author of each file's last change, a search needs ranked
keyword matches, and a forget preview needs the provenance of every file in the
history. A history walk for each costs commits times files, and the cost grows
with every commit. Repository hosts keep the last commit of each path in a
persistent cache, and git keeps a commit-graph file for the same reason.

A Forget must also remove a source item from every derived copy: indexes,
records, conversations and the database files.

## Decision

The memory repository stays the source of truth. The daemon keeps derived
indexes beside it in the installation's database, and they hold nothing the
history does not hold. The database is SQLite or Postgres (ADR-0024), so each
index has two implementations, and this record states what holds on both.

### The Page Index

One row for each file (path, position, time, author) and one row for each
Workspace (the memory revision the rows describe, the position of the next
commit). A position is the place of a commit in the history; a larger position
is newer. Commit times have one-second resolution, so time alone does not
order two commits.

One sync brings the index to HEAD:

- The index revision equals HEAD: nothing to do.
- HEAD descends from the revision: read the commits between, newest first, with
  one tree diff each. The first commit that names a path holds its last change,
  and a path whose last change is a deletion leaves the index.
- The history does not hold the revision: read the full history and replace
  the rows. This is the first build and the case after a forget rebuild.

One rule covers every writer (a commit, a revert, a forget rebuild); no writer
has its own index code. The daemon syncs in the background at boot and after
each commit. A page list syncs before it reads and reads the files at the
returned revision. A forget purge syncs before it reports its end.

A second table holds one row for each link of each page, as repository paths,
written in the same transaction as the page row. It stores no column for
whether the target exists, because that answer changes without a write of the
source page. An edge whose target no page holds is kept: it marks a page worth
writing, and a curation pass reports targets that recur.

The index also holds what a list shows: the title and kind from the front
matter, the connection that wrote most of a Subject Page's Timeline, and the
exposure stamp. A sync reads the file and the stamp at HEAD for each changed
path, and a commit that changes only a stamp updates the stamp.

A page list opens no file and no history. It reads the rows of its scope and
applies the rules of a file read: the reader's access permits the stamp, and
the forget store permits its provenance. No list shows a row with no stamp.
Each distinct stamp asks the forget store one time. A list comes in parts of
100, at most 500, newest change first and then path, with a cursor of the last
position and path. The daemon narrows by kind and by a search of title, path
and entity words, and each part names the full count. A second endpoint gives
the counts of a scope. The access and forget rules run before narrowing and
counting, so a count never names a page the reader cannot open.

### The Brief selection reads the index

The index holds the entity words of every file a Brief can offer. A Subject
Page with a valid layout gives the words of its file name and compiled truth.
Any other file is a fact file and gives its file name, title, aliases and
first paragraph (ADR-0007). A row with no words is an index file, which a
Brief never shows.

In pack mode the rank of a page is the distance of its position from the
newest position. In delta mode a page is changed when its position is among
the last N, where N is the number of commits after the cursor revision. A
cursor revision the history does not hold marks every page as changed. The
daemon selects at most three pages from the rows and reads only those.

The selection then follows one link out of a selected page while the page cap
has room. The target must be a page of the same access, and the hop costs no
page read. It is one hop, not a traversal: a second hop cannot pay for itself
in 8 KB. The budget and the page cap apply to the followed page too.

### Memory Search is a derived full-text index

A full-text index holds the path, the title and the whole text of each memory
file, front matter included, so an alias is searchable. It gives ranked search
and snippets with no repository scan, no model and no external service.
SQLite uses an FTS5 table with the porter tokenizer and BM25. Postgres uses a
`tsvector` column with a GIN index and `ts_rank_cd` under the English
configuration. Each row carries its `workspace_id`, so a search reads one
Workspace's rows.

A Page Index sync writes changed and removed rows to both indexes in one
transaction, and a full replacement first clears both sets of Workspace rows.
The Page Index is a rowid table and an upsert keeps a page's rowid, because
FTS5 finds a row fast only by rowid; the Postgres search table takes the same
id as its primary key.

The words of a query are joined with OR, so a page with more of the words
ranks higher and a word the page lacks does not hide it. Stemming matches word
forms. Hits of the two scopes merge by rank; a smaller rank is better on both
backends.

Relevance is defined by properties, not by one engine's scoring. Each backend
weights a title match about five times a body match. The tests hold both to
these properties: a title match outranks a body match, an entity word matches,
and a search reads no other Workspace's rows. No test asserts an exact order.
On Postgres `ts_rank_cd` reads only the scored row, so another person's corpus
cannot move a rank, and a store test holds that. On SQLite `bm25()` uses the
statistics of the whole table, so on a SQLite installation with several People
another person's corpus changes the numbers. The privacy property holds on
both: another person's pages never appear in the results.

Search limits the index to one Workspace and one scope, and the memory store
then applies the exposure and forget rules of a page list. A refused page
returns no path and no snippet. The Agent tool searches shared memory and the
calling Agent's private memory. Its audit record holds the query, the hit
count and the returned paths, never a snippet or page text.

### A forget preview reads a snapshot

The forget preview says how much a forget would remove and changes nothing. It
takes only the read side of the Workspace swap gate, which is exclusive only
during the directory exchange at the end of a forget rebuild, so commits run
beside it. A read never holds a write lock. The act that follows takes the
write lock, scans again, and returns a conflict when the fresh revision
differs from the one the owner saw.

The scan walks diffs. It walks the commits in topological reverse order with
one snapshot: every memory file with its blob and scope, and every source
stamp by the file it stamps. Each commit starts from its first parent's
snapshot and applies the diff of that parent's tree against its own. A root
commit starts empty. A merge is compared with its first parent alone, so the
snapshot after it is the merge tree. A stamp blob is read once by object id,
equal metadata shares one scope index, and a snapshot lives only while an
unread commit names its commit as first parent.

### The suppression row holds no readable key

A Forget writes one suppression row for each source item it removes. The row
holds the item's identity, the HMAC-SHA256 of the source and the id, so a new
retrieval of the same id finds the row and stays blocked.

The HMAC key of each Workspace derives from its Tenant Data Key (ADR-0013)
with HKDF-SHA256 and the fixed `info` label `pagis forget suppression`. The key
lives only in `secrets.enc`, so a reader of the database, a dump, a Backup or a
disk snapshot without the Installation Key cannot test a known source id. A
change of the label ends every Forget, so the label stays fixed.

The daemon passes its one holder of Tenant Data Keys to each call that
computes an identity: the start of a Forget, an acquisition, an ingest of
Incoming Events, and the check of one source id. A store asks for the key only
when a Forget writes rows or when the source's Connection has a row. Whether a
Workspace forgot anything is a query of the rows and needs no key, so a
Workspace that never forgot makes no Tenant Data Key. The forget rule of a
memory read and a re-opt-in need no key either.

### The suppression row is the only record of a forgotten item

The structured purge deletes every row that holds the item's id or a field of
its record: its current record, every source version and its arrival, the
Incoming Event that an arrival's delivery stored and its link to a Wake-up,
and the Reflection Filter decisions, arrival Wake-up pages and archived
Schedule pages that name the item's thread page. It also clears the target and
plan of the operation.

A new retrieval of a suppressed id writes no row, the Sync checkpoint moves
past it, and acquisition asks for none of its metadata. A re-opt-in therefore
restores nothing; without a suppression row, the next retrieval acquires the
item as new.

The memory purge removes the Subject Page, and the next Page Index sync
deletes its search row. The FTS5 tables have `secure-delete`, so a delete
removes the words at once. On Postgres the search row is an ordinary row.

`ingest` checks each Incoming Event against the suppression rows as a new
retrieval does: one query of the Connection's rows, and the keyed identity
only when a row exists. A suppressed event writes no row, wakes no Agent, and
the cursor moves past it. An account Forget blocks every event of its
Connection. The identity of an event is the identity of its Source Item: the
declaration of a kind names its synced resource, and the provider event id is
the item id. `mail.message_received` from a Google Connection has the resource
`gmail` and the Gmail message id. `mail.message_received` from an Agent
Mailbox and `call.ended` have no resource, because they live on Installation
Connections that no Sync acquires (ADR-0019, ADR-0020), and the check skips
them.

### The end of a Forget clears the records of a purged page

The memory rebuild drops each file and replaces each commit sentence whose
exposures a Forget does not permit. The step that records the end of the
memory purge also changes, in the same transaction:

- The Learning Feed. A commit entry that the Forget does not permit loses its
  files, titles and sentence; a revert of it loses its files and titles; the
  entry of an archived Schedule loses its name, instruction and Subject Page.
  The step edits the entry and does not delete it, because on SQLite a deleted
  newest row lets the next event reuse its `seq`, and a subscriber past that
  `seq` misses the event. The feed hides an entry that names no file.
- The Brief cursors. Each keeps only the paths that the Page Index holds.

Other event log entries keep what they hold: tool call arguments and failed
commit entries can name a page. Clearing them is not built.

A deleted row stays in the database files until compaction. The structured
purge ends with one, and the last step of a Forget compacts again. On SQLite
this is a WAL checkpoint with `TRUNCATE` under `secure_delete`. On Postgres it
is `VACUUM FULL` of the changed tables. A reader that holds an old snapshot, or
a lock that does not come free, fails the phase, and the owner retries. It is
never a completed erasure.

### A Forget reaches the conversations that read the item

A Run records the synced content it reads in `run_source_reads`: one Source
Item, or a parent such as a mail thread. The provider says what a call read.
For Gmail, `mail__get_message` reads the named message, `mail__get_thread`
each message of the thread, and `mail__search` each listed message. The broker
records the reads before the model reads the result, and a read it cannot
record does not reach the model. While a Forget blocks the Connection the
store refuses the record, so a result in flight does not reach the model. A
Run that an event woke read the events of its Wake-up.

This is run lineage as OpenLineage records it: each output of a Run rests on
each input. Pagis does not know which words came from which read, so each
message of a Run rests on each item the Run read, and a message that quoted a
forgotten item is forgotten whole.

The transaction that blocks a Forget finds each Run that read a forgotten item
or its parent. It marks each message of those Runs forgotten and deletes each
kept tool result and each read record that names the item or its parent. A
message whose stamp names no Grant (the Person's words, a progress line, a
daemon notice) stays (ADR-0004). An account Forget reaches each Run that read
through the Connection and deletes each tool result that a Grant of the
Connection read.

A forgotten message keeps its row, so the conversation keeps its order and
Threads. The Person reads that it is unavailable, and no Agent reads it
(ADR-0004, ADR-0009). The structured purge erases its words. A Run that a
forgotten message started has the first line of that message as its title, so
the purge also writes "This message is unavailable" over that title (ADR-0002).
The SQLite conversation search tables have `secure-delete`; on Postgres the
compaction rewrites the conversation tables.

A Forget does not reach these parts, which are not built: memory page words
that a Brief or a memory read gave a Run; a message of another Run that quoted
a forgotten message; the words of a message that a Run in flight writes after
the purge (the message is forgotten, and its words stay until the next purge);
cards, sent mail and other Run records that are not messages or tool results;
and the audit log of calls, which keeps call arguments.

## Consequences

- A page list, a search and a forget preview cost what the history changed,
  not what it holds.
- The indexes are derived, so a rebuild from git is always possible.
- The memory store applies the exposure and forget rules in one place for
  every reader.
- After a Forget, the database holds no source version, Incoming Event or
  filter decision of the item, Memory Search holds none of its words, no feed
  entry or Brief cursor names a purged page, and the SQLite file and WAL hold
  no removed row. On Postgres each changed table is in a new file, and the
  server's own WAL keeps segments until it recycles them.
- A Forget of one item forgets every message of each Run that read it.
- A copy of the database alone cannot test a source id. A lost `secrets.enc`
  loses the key, so an item row then does not block a retrieval; an account
  row holds no identity and still blocks.
