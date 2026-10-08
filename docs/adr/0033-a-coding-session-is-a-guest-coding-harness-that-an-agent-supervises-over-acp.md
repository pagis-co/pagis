# 0033: A Coding Session is a guest Coding Harness that an Agent supervises over ACP

Status: accepted.

## Context

A Coding Harness, such as Claude Code or Codex, owns its own loop and writes
code well in a repository on the Person's own machine. Many people pay for
one by subscription.

ADR-0001 keeps ACP as the guest protocol. ACP (Agent Client Protocol,
agentclientprotocol.com, protocol v1) is JSON-RPC over the stdio of the agent
program. The ACP registry
(`https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`) gives
the launch command of each adapter.

Zed and JetBrains drive coding harnesses over ACP. Conductor, Paseo and T3
Code let the person sign in to their own subscription through the app, and
the app runs the vendor's own sign-in. The Claude Code adapter
(`@agentclientprotocol/claude-agent-acp`) offers its sign-in as ACP terminal
authentication methods when the client declares
`clientCapabilities.auth.terminal`: `claude-ai-login` for a Claude
subscription and `console-login` for an Anthropic Console account. A
terminal method runs the agent's own program in an interactive terminal, and
the client never passes it to `authenticate`.

Anthropic's terms for Claude Code
([Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance),
"Authentication and credential use") forbid a third-party developer to offer
Claude.ai login in its own application, to route requests through Free, Pro
or Max plan credentials for its users, and to collect, store or intermediate
Claude.ai credentials or session tokens. The sign-in completes through
Anthropic's own flow. The terms do not prevent a person from signing in to
the unmodified Claude Code program with their own subscription. A product
that offers Claude Code does not modify the program, removes no built-in
authentication method, and lets each person authenticate with their own
credentials.

The nearest shape in Pagis is the Call (ADR-0020): an Agent starts it, a
block that the daemon makes shows it, and its events wake the Agent.

## Decision

### A Coding Harness is a guest, not an Agent

A **Coding Harness** is an external coding agent program that Pagis drives
over ACP: Claude Code (`@agentclientprotocol/claude-agent-acp`), Codex
(`@agentclientprotocol/codex-acp`), OpenCode (`opencode acp`), pi (`pi-acp`),
Gemini CLI (`gemini --acp`), Copilot CLI (`copilot --acp`) and Cursor CLI
(`cursor-agent acp`). It owns its own loop, so memory, the Briefing, Grants
and Threads do not apply inside it (ADR-0001).

### A Coding Session is one ACP session that one Agent owns

A **Coding Session** is one ACP session of one Coding Harness, in one working
directory, on one place: a Host, or the Agent's Computer. One Agent owns and
supervises it. One Thread shows it.

A Coding Session is not a Run. The Run that starts it ends normally, and
session events wake the Agent in new Runs.

The record holds:

- the id (`CodingSessionId` in `pagis-core`), the Workspace and the Agent;
- the harness id and version;
- the place (`host` or `computer`), and the host id for a Host;
- the directory that the Agent named, the directory that the process runs
  in (the worktree path when the session has a worktree), and the worktree
  branch or none;
- the Session Approval Mode;
- the title, the state and the end reason, and for a failed harness the end
  detail: its exit code and the last 4 KB of its stderr, or the message of
  the request that it failed;
- the harness's own ACP session id;
- the Channel and the root message of its Thread, and the message that holds
  its block;
- the Run that started it;
- the usage: the context used and its size, and the cost and its currency
  when the harness reports them;
- the created, updated and ended times.

The tables are `coding_sessions` and `coding_session_events`, in SQLite and
in Postgres.

The states are `starting`, `working` (a turn runs), `needs_decision` (a
permission or a question waits), `idle` (the turn ended, and the session
takes a prompt), `interrupted` (the place was lost, and the session can
resume), and the terminal states `closed` and `failed`.

The transcript is append-only rows of `coding_session_events`: a seq, a time,
a kind and a JSON payload of at most 16 KB, with a truncation marker. The
kinds are `prompt`, `agent_message`, `thought`, `tool_call`,
`tool_call_update`, `plan`, `usage`, `permission`, `decision`, `question`,
`answer` and `turn_end`. Consecutive chunks of one message merge into one
row. A message row holds at most 16 KB of text, and the rest of a longer
message goes on in rows marked `continued`, so the transcript keeps every
word.

A Coding Session record and its transcript have no retention, and a Backup
holds them, as it holds a Call record. Forget does not reach them: a prompt
is the Agent's own words, and the output of a harness is foreign text that no
Run read from a source.

A lost Host or a restart of the daemon makes each open session of that place
`interrupted`. The Agent resumes it from the stored ACP session id, with ACP
`session/resume` or `session/load` where the harness declares the
capability.

An Agent holds at most four open (non-terminal) Coding Sessions.

### The daemon is the ACP client

The daemon uses the `agent-client-protocol` crate (3.x). In `initialize` it
declares `fs` and `terminal` false, so the harness uses its own tools where
it runs. It declares terminal authentication (`auth.terminal`), so a harness
offers its own sign-in, and it declares form elicitation alone. It passes no
MCP server in `session/new`.

The ACP client keeps no policy. It hands each permission request and each
form question of the harness to the daemon, and it waits for the answer
outside its dispatch loop, so the updates of the harness still arrive. A
cancel of the session, or a request that the harness withdraws, ends the
wait, and the daemon gets an event that the request is withdrawn.

A native driver, such as the Codex app-server or pi RPC, is allowed only
where ACP lacks a capability that Pagis needs.

### The Harness Catalog ships with each release

The **Harness Catalog** ships with each release, as the Provider Catalog does
(ADR-0012). For each Coding Harness it holds:

- an id: `claude`, `codex`, `opencode`, `pi`, `gemini`, `copilot` or
  `cursor`;
- a display name;
- an exact pinned version;
- the launch command for each platform, from the ACP registry:
  `npx <package>@<version>`, or a binary with arguments;
- for a harness that runs in the Agent's Computer, the npm packages that the
  Computer Image installs and the program and arguments that start it there;
- whether the harness asks permission (pi does not);
- the vendor's own sign-in command for each sign-in method (subscription,
  API key), where ACP `initialize` gives no terminal method.

Pagis does not fetch the registry at run time. The Client App declares
`harness:<id>` as a Host capability for each harness whose launcher it finds
on the Person's `PATH`: `npx` for an npx entry, and the binary for a binary
entry. A harness that needs a further program, such as `pi`, needs that
program on the `PATH` too.

The daemon is the one source of the catalog, and the Client App ships no
copy. The answer to a Host registration names, for each harness that has a
launch command for the platform of the Host, its id and the programs it
needs on the `PATH`. The Client App reads the environment of the Person's
login shell and looks for those programs. When the harnesses it finds
differ from the `harness:` capabilities of the answer, it registers again
with its own capabilities and `harness:<id>` for each harness it found, in
the order of the catalog. The client stays the authority for its
capabilities (ADR-0015), and the daemon gets no new frame. The second
registration replaces the presence connection, so a host command that
arrives between the two answers reads the machine as absent, at most once
for each connection of the Host socket. The Client App looks at each
connection of the Host socket, so a person who installs Node sees the
harnesses after the next reconnection. A login shell that fails declares no
harness, and the Client App writes the message to its log. `harness:<id>`
means that the programs are on the `PATH`, not that the Person signed in.

### On a Host, the session socket carries each Coding Session

The **session socket** is a further WebSocket of the Client App at
`/api/v1/hosts/{host_id}/sessions`, a mirror of the exit socket (ADR-0029).
Binary frames carry one byte stream with yamux over it. The daemon opens one
yamux stream for each Coding Session.

The first line on a new stream is the daemon's open request:

```text
{"session_id", "command", "args": [...], "cwd", "env": {name: value},
 "worktree": null | {"repo", "branch", "base"}}
```

`env` never holds a secret.

The Client App answers with one line: `{"ok": true, "cwd": "<the directory
the process runs in>"}`, or `{"ok": false, "error": "<code>", "message":
"..."}`. The codes are `not_found` (the command), `bad_directory`,
`worktree_failed` and `spawn_failed`. After the answer the stream carries the
harness's raw stdin and stdout: newline-delimited JSON-RPC.

The Client App reads the whole environment of the Person's login shell and
looks for the command on its `PATH`, because a macOS GUI app does not inherit
it. The harness gets that environment, so it runs as it runs in the Person's
terminal. The variables stay on the machine. When the login shell fails, the
Client App answers with the error and does not use its own environment.

For a request with a `worktree`, the Client App makes a git worktree before
it answers. It runs `git -C <repo> worktree add -b <branch> <path> <base>`
with the git of the login shell, so the repository's own hooks run as in the
Person's terminal. `base` is a ref of the local repository, and the Client
App does not fetch. The worktree is at
`~/.pagis-worktrees/<base name of repo>/<branch with each "/" as "-">`, apart
from the Person's repositories. It is not in the State Directory, because a
Backup copies that directory. The process runs in the worktree, at the same
relative path as `cwd` in `repo`, and the answer gives that directory. A
`repo` that is not an absolute path to a directory, or a `cwd` outside
`repo`, answers `bad_directory`. A failure of git, such as a branch or a
path that exists, answers `worktree_failed` with git's message. Pagis does
not remove a worktree: it holds the Agent's work after the session ends, and
the Person removes it with `git worktree remove`.

When the process exits, the Client App closes the stream and sends a
`session_exit` text frame on the Host socket with the session id, the exit
code and the last 4 KB of stderr.

Each Client App opens the session socket, on a Local Installation too: the
daemon is never a Host (ADR-0015).

### The Client App stays a pipe

The Client App starts what the daemon names and pipes the bytes. All
protocol logic, policy and audit stay in the daemon. The trust statement of
ADR-0015 grows by one clause: a server can start a Coding Harness on the
Host.

### In the Agent's Computer, a session runs in the `auto` mode

The daemon starts the harness with `docker exec` and attached stdio, the path
of the stdio MCP servers of the Plugin Computer (ADR-0017). A Computer
session needs no card, because the container is the sandbox, as for
`computer_shell`.

The Computer Image ships the four harnesses that a Harness Model Endpoint
can serve: Claude Code, Codex, OpenCode and pi. It ships no harness that
needs a subscription sign-in or a Gemini API. The image installs the npm
packages of `computer/harnesses/package-lock.json` with `npm ci`, which
checks each package against its sha512, and the OpenCode release archive of
its architecture, checked against its sha256. The versions are the pins of
the Harness Catalog. Root owns the harnesses, so the `agent` uid cannot
change a harness, and a harness cannot update itself. A Computer launch runs
the installed program and never `npx`, so a session downloads nothing.

The harnesses add about 1 GB to the image, mostly the native programs of
Claude Code and Codex. A Docker host pulls the image once for each release,
and every Computer of the host shares it. A second image or a volume for the
harnesses is one more artifact to pin, pull, scan and match to a release,
for no gain in a product that ships one image.

No credential enters the Computer (ADR-0005). The daemon serves the
**Harness Model Endpoint**: the Anthropic Messages API, the OpenAI Responses
API and the OpenAI Chat Completions API. A per-session token authenticates
each request. The daemon forwards it with the Org's provider key through
`crates/llm-router`, under the Spend Cap, and writes Usage Records for the
Agent. The harness points at the endpoint: `ANTHROPIC_BASE_URL` with
`ANTHROPIC_AUTH_TOKEN`, a Codex model provider, or an OpenCode or pi provider
base URL. The endpoint has a TCP port of its own, `[computer] model_port`, on
every interface of the daemon's host. On a Server the egress rules open that
port to the Computers and close it to everything else (ADR-0014).

A subscription sign-in in a Computer is not in scope.

### Harness Sign-In runs the vendor's own program on the Person's machine

In a **Harness Sign-In** the Person signs in to their own subscription (Claude
Pro or Max, ChatGPT, Copilot and others) or to an API key, and chooses which.
The Client App runs the vendor's own sign-in command, in the vendor's own
program, in a terminal window on that Host: the ACP terminal method that
`initialize` gives, else the command of the Harness Catalog.

Pagis never reads, copies, stores or relays the credential. The harness
keeps it in its own store on that machine. The daemon reports a harness that
needs a sign-in.

### The Agent drives a session with core tools

The Agent drives a Coding Session with core tools (ADR-0005):

- `coding_session_start {harness, machine?, directory, worktree?, mode?,
  title, prompt}`. `worktree` is true by default, on the branch
  `pagis/<slug>`. `mode` is `person` by default. The effect class is `host`.
  The candidate machines are the present Hosts of the Person that declare
  `harness:<id>`, chosen as `host_shell` chooses them (ADR-0015). The card
  shows the harness, the machine, the directory, the worktree, the mode and
  the start of the brief. Its allow-rule builder offers "Always allow
  <harness> sessions in <directory> on <machine>", stored on the host Grant.
  The start refuses a mode wider than the Grant allows, and a mode other than
  `auto` for a harness that does not ask permission.
- `coding_session_send {session, prompt}`: a new turn when the session is
  `idle`, and queued until the turn ends when it is `working`, because ACP v1
  has no steering. Free.
- `coding_session_read {session}`: the state, the pending decision, the last
  agent message, the plan, the changed files and the usage. Free.
- `coding_session_cancel {session}` (ACP `session/cancel` of the turn) and
  `coding_session_close {session}`. Free.
- `coding_session_list`: the Agent's own sessions. Free.
- In the `agent` mode, `coding_session_decide {session, decision:
  allow|deny, note}` and `coding_session_escalate {session, note}`. Free,
  because the Person delegated the decision through the mode. A decision
  allows once only, and an Agent never writes an allow rule.
- For a question, `coding_session_answer {session, values}`. Free.

### The Person sets the widest Session Approval Mode

The **Session Approval Mode** says who answers a Harness Permission:

- `person` (UI: "Ask me"): Pagis policy first, then the Person.
- `agent` (UI: "Let the sprite decide"): Pagis policy first, then the
  supervising Agent, which decides or escalates to the Person.
- `auto` (UI: "Allow everything"): the daemon allows each request.

The host Grant holds the widest mode that each Agent may use on each machine,
in the field `session_approval_mode` of its scope. An absent or unknown value,
and a revoked Grant, read as `person`. A change is a Grant revision, and it
withholds no message, because only a Connection Grant is stamped. The Agent
picks a mode for each session within it.

The Person sets the widest mode for an Agent and a machine on the Access tab
of the Agent, with
`PUT /api/v1/agents/{agent_id}/hosts/{host_id}/session-approval-mode`. The tab
shows each machine of the Person that declares a `harness:` capability. When the
Agent holds no live host Grant on that machine, the write makes one with no
Allow Rules. That Grant is the Person's own act, as a Connection Grant is, and
it makes the machine a host candidate of the Agent (ADR-0015).

### A Harness Permission passes Pagis policy first

A **Harness Permission** is an ACP `session/request_permission`. The ACP
client merges the request's tool call onto the tool call that the harness
reported with the same `toolCallId`: a field that the request leaves out
keeps its reported value (ACP `ToolCallUpdate`). The Codex adapter asks for a
started command with no kind, title or locations, because its earlier
`tool_call` update holds them. ACP has no command field. The Claude Code and
Codex adapters put the shell command of an `execute` in `rawInput.command`,
so the command is that value when it is a string, and else there is none.

The daemon reads the live host Grant of the owning Agent on the session's
machine at each permission, so a narrower Grant applies to the next
permission of a running session (ADR-0005). The effective mode is the
narrower of the session's mode and the widest mode on the Grant. With no
live Grant, the mode is `person` and there are no rules.

The broker evaluates the permission with a pure function, in this order, and
stops at the first step that answers:

1. The `auto` mode allows once. The audit decider is `auto`.
2. By ACP tool kind: a `read`, `search`, `think`, `edit`, `delete` or `move`
   with every location inside the session's directory allows. A request with
   no location passes this step only for `think`, which touches no file. The
   session's directory is the directory that the process runs in. The audit
   decider is `scope`.
3. An `execute` whose command a live Host Allow Rule of that machine's Grant
   matches, with the tree-sitter-bash matcher of ADR-0015, allows. The audit
   decider is `rule`.
4. In the `agent` mode, the session goes `needs_decision`, and the Agent
   wakes and decides or escalates.
5. In the `person` mode, or after an escalation: a Request of the kind
   `harness_permission` with no Run, an approval card in the session's
   Thread, an item in the Needs-You Queue and a Notification. "Approve once"
   allows once. "Always allow" for an `execute` writes a Host Allow Rule with
   the existing builder, then allows once. "Deny" rejects once.

The scope check is lexical, because the files are on the Host and the daemon
cannot read them. It removes `.` and resolves `..` in each POSIX path, and it
then requires the directory itself or a path under it. A relative path is
outside. A symbolic link inside the directory that points outside it counts
as inside. The `acceptEdits` mode of Claude Code and the `workspace-write`
mode of Codex trust the working directory in the same way.

The rule step uses the same Host Allow Rules as `host_shell`. A rule that the
Person writes from a harness card also lets the Agent run that command class
with `host_shell` on that machine, and the reverse. ADR-0015 runs a
rule-approved command of `host_shell` under `/bin/sh`. A Coding Harness runs
a command in its own shell, which Pagis does not choose, and a plain word
can have a different meaning in that shell, for example a `zsh` word that
starts with `=`. Pagis accepts this difference for a harness command.

Each decision answers with the offered ACP option of the matching kind,
`allow_once` or `reject_once`. An allow of a request that offers no
`allow_once` option answers `cancelled`. Pagis never answers `allow_always`
or `reject_always` to the harness, so the authority stays in Pagis. A cancel
answers each pending request with `cancelled`.

Each decision is recorded two times. The transcript holds a `permission` row
when the request arrives, with whom the session waits for, and a `decision`
row with its decider when it is answered. The bus holds one audit fact, the event
`coding_session.permission_decided` with no Run. It holds the session, the
Agent, the Host, the tool call id, the tool kind, the command, the
locations, the decider, the outcome (`allowed` or `cancelled`), the selected
option kind, and the revision of the Grant that the evaluator read. A
pending request that a cancel ends, or that the harness withdraws, has the
outcome `cancelled` and no decider.

### A question goes to the supervising Agent

A question of the harness (ACP `elicitation/create`, form mode) goes to the
supervising Agent first, whatever the mode. The Agent answers it, or asks
the Person with `ask_user`. The daemon declines the URL mode.

### A Session Rule wakes the owning Agent

When a session starts, the daemon makes a **Session Rule**: one Event
Subscription for each kind of session event, because a rule matches one kind
(ADR-0006). The rule wakes the owning Agent in the session's Thread, as the
Standing Call Rule does for a Call. The daemon makes it after it posts the
block, because the root of the Thread must exist, and before the stream
opens. A start whose rule is not made fails with `temporarily_unavailable`.
The daemon archives the rule when the session reaches a terminal state,
after it gives the last event to the rule. That archive keeps the pending
Wake-up of the rule, unlike an archive by the Person (ADR-0006), because the
last event of a session is the news that the session ended. The Session Rule
is daemon housekeeping: the REST list, the Automations and
`event_subscription_list` do not show it, and nobody edits it.

Session events enter the Trigger module through `ingest` as Incoming Events
of the kinds `coding_session.turn_ended`, `coding_session.needs_decision`
and `coding_session.ended`. The daemon raises `coding_session.turn_ended` at
each end of a turn, `coding_session.needs_decision` at each move to
`needs_decision`, and `coding_session.ended` at each move to `interrupted`,
`closed` or `failed`. A start that fails raises no event, because the
starting Run reads the failure in its tool result. The metadata names the
session, its title, the harness, the machine and the stop reason, the
decision kind or the end state and reason. It holds no harness text. The id
of an event is `<session id>:<seq>` for an event of a transcript row and
`<session id>:ended:<state>` for an end, so a replay wakes nobody twice. A
failed `ingest` is logged and does not stop the session. The source of these
Incoming Events and of the Session Rule is the Coding Session, and no
Connection. The provider of these kinds is `pagis`, the provider of each
kind that Pagis itself raises, and their declarations name no capability,
because a Grant names a Connection. A Coding Session source has no cursor,
no baseline and no collector: one session event is one batch with one event.

Wake-ups combine as ADR-0006 says: while the Agent's Run for the rule is
active, new events join one pending Wake-up. A Run that a session event
starts is a Run of an Incoming Event: a new chain whose Origin is the
session's Thread (ADR-0003).

### Clients read a session over REST and events

The daemon serves the Coding Sessions of the Workspace and their
transcripts under `/api/v1/coding-sessions`: the list, newest first, one
session, one page of its transcript, and the Person's Stop. Stop cancels
the turn that runs and closes the session with the end reason `stopped`.
The answer about a session also gives the display name of the harness, the
name of the machine, one line about the last row of the transcript, and the
ask that waits while the session is `needs_decision`. The daemon reads them
on each request and does not store them.

The store of the daemon reports each write as a durable event, as a Call
does. `coding_session.changed` reports a write of the record.
`coding_session.transcript` reports a new row at once, and a row that grows
by merges at most once a second. A client then reads the rows from the
highest `seq` that it holds, less one. No event carries the text of a row or
the title of a session.

The Product App shows one session at `/coding/<id>`, in the main pane as a
Run is: its head, its last plan, and its transcript as messages, tool calls
and one line for each ask. An event of the session makes the page read the
record and every page of the transcript again.

### The session's Thread shows a daemon-made block

The daemon posts the session's block in the Channel of the starting Run: in
the Run's Thread when the Run has one, else as a top-level message that
becomes the root of the session's Thread. The block is the daemon-made
`coding_session` type (ADR-0004), because it refers to a row that the daemon
owns.

### Harness output is foreign text

Everything that a harness writes reaches a prompt inside the untrusted
envelope, as `source=coding_session:<id>` (ADR-0005).

Other ways were considered:

- **A native driver for each harness**, such as the Codex app-server or pi
  RPC. One protocol for every harness costs less. A native driver stays
  allowed where ACP lacks a capability.
- **A terminal that runs the harness's TUI**, read from the screen. It gives
  no structured permission request and no structured transcript.
- **The harness's own "always allow".** The authority would leave Pagis.
- **Pagis holds the subscription token, or offers a Claude.ai login of its
  own.** The vendor's terms forbid it, and Pagis would hold a credential
  that the model's tools could reach.
- **A harness process in the daemon.** The daemon is never a Host
  (ADR-0015).

## Consequences

- An Agent gives coding work to the Person's own subscription. On a Host,
  Pagis spends no provider key on the harness.
- A Host session lives while its Client App and its machine are awake. A
  sleeping laptop interrupts it.
- Quit and "Restart to Update" in the Client App ask first while Coding
  Sessions run on this computer, and say how many stop. The Client App
  counts the processes that it runs and asks the server nothing. A quit
  from the Dock, a logout or a signal asks no question.
- Inside a session, Pagis memory, the Briefing and Grants do not apply.
  Pagis policy applies to each Harness Permission.
- An edit inside the session's directory passes with no card in every mode.
- In the `auto` mode, a harness on a Host runs each command as the Person's
  OS user with no card. The Person allows that on the Grant.
- A new harness release changes nothing until a Pagis release pins it.
- A harness that asks no permission runs only in the `auto` mode.
- A Computer session spends the Org's provider key, under the Spend Cap.
- A Request can have no Run: a Harness Permission waits on a Coding Session,
  and it expires when the session ends (ADR-0004).
- An Incoming Event and an Event Subscription name their source: a
  Connection or a Coding Session (ADR-0006).

## Not built

- The core tools other than `coding_session_start`.
- The "Always allow" rule of `coding_session_start`.
- The refusal of a mode other than `auto` for a harness that does not ask
  permission.
- The interruption and the resume.
- The approval card. A Harness Permission that Pagis policy does not allow
  waits until a cancel ends it.
- The `agent` mode.
- The `auto` mode.
- The item in the Needs-You Queue and the Notification.
- The question in the daemon.
- Harness Sign-In.
- The session block in the Product App.
- The diffs on the session page.
- The Coding place.
- Harness Sign-In in Settings.
- The Computer place.
