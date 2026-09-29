# Pagis — Memory pages

What belongs on a memory page, kind by kind. The decision that memory is
plain files under git is ADR-0007, and the glossary in `CONTEXT.md` gives
the canonical term for each concept. This document is the reference the
reflection prompt makes short: it says what a page of each kind holds and
what it does not.

## The page kind

A memory file starts with a front matter block, and the `kind` line says
what the page is about. The vocabulary has nine words. Reflection picks
one of them when it writes a page. Acquisition writes `Source` for an
arrival nobody has read yet, and reflection replaces the kind when it
reads the page.

A word outside the nine is a signal, not an error. The page keeps the
word it was given, because a vocabulary that refuses an unknown word makes
the model choose a wrong one. A curation pass that reports such words is
not built.

One page holds one subject. When a page starts to hold two subjects,
write a second page and let each page point at the other.

## Links between pages

A page links to the pages it names. A link is the path of the other page
between double square brackets:
`[[private/subjects/gmail/19ce3259c919b4de.md]]` or `[[shared/user.md]]`.
Write it where the text names the other page. Where the text does not name it,
put the target on a `links` line of the front matter block:

```
---
title: Northwind renewal
kind: Transaction
links: [[shared/user.md]] [[private/subjects/gmail/19ce3259c919b4de.md]]
---
```

A link is a path, not a name: Pagis has no slug namespace, and a Gmail page is
named by its thread id. `memory_read` takes a link where it takes a path, so a
link is the way to open the page it names. A `[[...]]` that is not a path is
ordinary text.

A link to a page that does not exist is worth writing: it marks a page that
memory still needs. An isolated page is invisible, so a page that names others and
links to none is a page recall cannot reach from its neighbours. A first page
about a new subject has nothing to link to, and that is not a fault.

## Aliases

A page has one title and many names. The owner says "the couch" and the
page is called "sofa"; a colleague is "Pri" in speech and
`priya@northwind.example` in a mail header. Recall finds the spelling it
is given, so a name that stands nowhere on the page finds nothing.

Put the other names on an `aliases` line of the front matter block, each
one between double square brackets, as a link target is written:

```
---
title: Sofa
kind: Transaction
aliases: [[the couch]] [[davenport]]
---
```

Write the short form, the nickname, the address and the handle the
source used. An alias is an entity word of the page, so a turn that uses
one of them brings the page into the Brief. Memory Search reads the
whole file, so an alias is searchable as soon as it is written.

An alias is a name, not a path: a `[[...]]` item of the `aliases` line
is never a link.

## A fact file

Not every memory file is a Subject Page. A fact file states one thing an
Agent learned, and it holds no Facts table, no Schedules and no
Timeline: a front matter block and a short body are the whole file.

A fact file is a Brief candidate like a Subject Page. Its entity words
are its file name, its title, its aliases and its first paragraph, so
put the matter of the file in that first paragraph and the detail after
it. A Brief shows the file as it holds it.

The index file at a scope root is not a fact file. `MEMORY.md` names the
fact files of its scope and holds no fact of its own, and a run reads
both indexes in full, so a Brief never volunteers one.

## The page of a conversation

A conversation with the owner is a matter like any other, so it has a
Subject Page of its own. Pagis names the file after the Channel, at
`private/subjects/conversation/<channel>.md`, and the Agent writes it
with the `conversation_page` tool while it replies. There is no path to
give: the tool takes the Channel from the Run, so an Agent writes the
page of the conversation it works in and of no other.

The layout is the layout of every other Subject Page. The compiled truth
says what the conversation is about and where it stands now, and the
Timeline takes one entry for each turn of work, not one for each
message. A decision, a commitment, a result or a change of plan settles
something. Chat does not, and a conversation that ends settled has no
page.

The page also carries the open work of the conversation. The Facts table
holds the decisions the conversation already made. The `## Open
questions` section holds what the work still has to answer, because an
open question is not a fact and the next Run needs it. Give the tool
every question that is still open: the call replaces the section, so a
question this turn answered leaves the page.

Open work is a next step or an open question. A turn that leaves either
one writes the page, and a necessary compaction writes it as well, from
the Continuation Record it makes. One page holds the work, so a task the
owner returns to over a week is one page with a Timeline.

The kind is `Project` when the work has an end, and `Workstream` when it
does not. Standing work with no end date is a Workstream, and a
compaction that starts the page uses that word. Link the page to the
pages of the people and the matters the conversation named, so a later
conversation about the same subject reaches this one.

## The nine kinds

### Person

One human: who they are, how they relate to the owner, and how to reach
them. The page holds names, roles, contact points, relations, and the
preferences of that person that the owner must remember. It does not hold
the record of one meeting with that person, which is an Event, and it
does not hold what the owner wants, which is a Preference.

### Organization

One company, school, clinic, vendor, authority or group. The page holds
what the organization is to the owner, the accounts and reference numbers
that identify the owner to it, and how to reach it. A person at the
organization gets their own Person page. One purchase from the
organization is a Transaction.

### Event

One dated thing that happens once: an appointment, a trip, a delivery, a
hearing, a party. The page holds when and where it happens, who takes
part, what the owner must do before it, and how it ended. Standing work
with no date is a Workstream. Something that repeats without end is not
an Event: write the Workstream that governs it.

### Transaction

One purchase, booking, quote, claim or refund: what was decided and what
it cost. The page holds the amount, the parties, the reference numbers,
the dates that bind, and the state of the exchange. The delivery of the
goods is an Event. The vendor is an Organization.

### Project

Something the owner builds or does that has an end. The page holds the
outcome, the state, what blocks it, and who takes part. When the project
ends, the page stays as the record of it. Work with no end date is a
Workstream.

### Workstream

Standing work with no end date: a duty, a routine, an area the owner
looks after. The page holds what the work is for, how it runs now, and
the open items in it. A piece of that work with an outcome and an end is
a Project.

### Concept

A reusable idea or mental model the owner refers back to. The page holds
the idea, where it comes from, and how the owner applies it. A document
that states the idea is a Source; the Concept page is what the owner
takes from it.

### Source

A document, article, message, reference or unread arrival the agent keeps
and cites. The page holds what the source says and the reference that
finds it again. Acquisition writes this kind for every arrival, so most
pages carry it until reflection reads them. What the owner concludes from
a source belongs on the page of the subject, not here.

### Preference

How the owner wants things done: tone, times, formats, limits, and the
standing instructions that hold across runs. The page holds the
preference and the reason for it, so a later run can tell when it no
longer applies. A preference of another person belongs on that Person
page.
