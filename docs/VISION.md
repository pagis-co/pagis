# Pagis — Vision

## North star

Pagis lets a person run their life and work with a staff of AI Agents. The
person hires Agents the way a company hires employees. Each Agent has a
job, a personality, its own memory, its own computer, and scoped access to
the person's resources. The person talks to the staff in a chat app, in
direct and group conversations. Agents work while the person is away, make
and answer phone calls, learn from their work, and build their own tools.

The identity of the product is the **hire-an-employee experience**: you
create an Agent, watch it work at its own desk, and trust it more over
time.

## What makes Pagis different

1. **Agents are employees, not sessions.** An Agent is a durable
   colleague: persistent memory, a persistent computer disk, a work record
   and a personality. It reacts to events (mail, Schedules, messages from
   other Agents) without the person present.
2. **A watchable desk.** Every Agent has a real computer. The person can
   watch its screen live, take control at any moment, and give control
   back. The Agent gives control to the person when it needs them.
3. **A phone and a voice.** Each Agent can have its own number, the way an
   employee has a desk line. Agents make and answer real calls. The person
   can speak to Agents, and Agents can speak back.
4. **Connect once.** The person connects an account, saves a Credential or
   adds a number one time. Every Agent with a Grant can then use it.
5. **A software factory.** Agents build their own tools for repeated work:
   scripts and applications for one purpose, versioned, published to the
   Software List of the Workspace, and usable, forkable and changeable by
   any Agent, with changes contributed back to the author. Code replaces
   repeated token spend.
6. **Auditable learning.** An Agent writes down what it learns while it
   works, and reviews synced arrivals and open evidence in the background.
   Every memory change is versioned and can be reverted. Learning is a
   visible feed.
7. **Rich conversation.** Agents answer with structured blocks (tables,
   forms, approval cards, previews), not only with prose.

## Principles

- **Flat roster.** No Agent has architectural authority over another.
  Hierarchy is the person's configuration. The Chief of Staff is a
  designation with no special powers.
- **Fast and small.** The core is a Rust daemon. Idle Agents cost almost
  nothing. Pagis builds a critical part itself when the available one is
  not good enough, for example the screen-streaming pipeline.
- **Own the loop.** The agent loop is Pagis's own, built on its LLM router
  (ADR-0001).
- **One product for one person and for a team.** The same daemon runs on a
  laptop and on a server. One installation is one Org, and each person in
  it keeps a private Workspace.
- **The phone is a pager and a remote.** The person answers Requests,
  gives work, reads the Report, watches a Desk and follows a Call from a
  phone. The work runs in the daemon and on the Agents' Computers, never
  on the phone.
- **Open source.** The daemon, the interface, the Client App and the
  Computer Image are open source.
- **Grow in layers.** Each capability is built on a product that already
  works.

## Not built

- **Texting.** The texting seam exists (ADR-0020), but no path sends,
  receives or shows a text: the collector, the readiness read, the text
  tools, the Standing Text Rule, the `text` block and the Conversation
  Inspector are not built.
- **Structured browser control.** Computer control is pixel computer use.
  A structured browser-automation step in front of it is not built. An
  Agent that turns a repeated flow into a Software Package lowers the need
  for it.
- **A coding harness as a guest.** Handing work to an external coding
  harness, such as Claude Code or Codex, with the person's own
  subscription, is not built. It would sit on top of the Software List,
  watched from a Thread and deployed into the Computer of the parent Agent.
- **The phone as a pager.** A phone opens the Product App in its browser
  through Remote Access and signs in with a Sign-In Link (ADR-0028).
  Notifications through Web Push, the Product App as an installed web
  app, the Push Relay, and the Mobile App for iOS and Android are not
  built.

## Non-goals

- Several people working inside one Workspace.
- A marketplace for Agents.
- An Agent, a model or a Computer that runs on the phone.
- A text message from Pagis to the person as a way to notify them.
