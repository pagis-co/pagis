# 0015: A host action runs on a present client, never in the daemon

Status: accepted.

## Context

`host_shell` runs a command on the person's own machine. A daemon that ran it
itself would run it as the daemon's user, with no sandbox, on the machine that
holds every Workspace's rows, memory repositories, the secret file and the
Docker socket. On a server that is an escape from one Workspace into all of
them.

The authorization machinery (a Grant, an approval card, a Host Allow Rule, an
effect class, an audit fact) needs a subject: the machine a command runs on,
and whether it is connected. VS Code Remote and JetBrains Gateway make the
client an endpoint the other side dispatches to. Pagis uses that shape: the
daemon dispatches, and the client is the machine.

## Decision

### A Host is a record of one person

A **Host** is one machine of one person: an id, the owner's Workspace, the
name the machine calls itself, its platform, the capabilities its client
declared, and its last-seen time. Every read names the Workspace (ADR-0023),
so another person's Host reads as absent. The name is the machine's identity
inside the Workspace: a client that connects again lands on its own record, so
a Grant survives a restart of the client, the daemon or the machine. Two
people can use the same machine name.

### Presence is memory of the running daemon

A client registers as a Host over its authenticated WebSocket, and the machine
is present while that socket lives. When it closes, the machine is absent and
its last-seen time is written. Presence is never a column: after a daemon
restart nothing is present until clients return.

### The daemon is never a Host

`host_shell` never runs a command in the daemon process. It dispatches to a
Host over the socket and reads the result back, matched by an id the daemon
made. On a local installation the Client App is the Host, on the daemon's
machine. A person with only a browser has no Host, and a host command answers
that no computer is connected and says to open the Client App.

Presence is in memory, so the daemon answers a call for an absent machine at
once and names the machine where there is one. A command runs for at most two
minutes; longer work belongs in the Agent's Computer.

### The Grant and the card name the machine

`Grant.resource_id` carries the host id, and the live-grant index is
`(agent_id, resource_id)`: one live Grant for each machine. The card names the
machine, and the Grant it writes names the machine the card named. A host
Grant that names no machine grants nothing.

Allow rules live on that Grant, so they are scoped to one Agent on one machine
of one person in one installation, and nothing carries them between
installations. A host action's effect class is `host` on every installation,
and the only way past a card is a live allow rule on that machine's Grant.

### A Host Allow Rule names a command class

A Host Allow Rule names the leading words of a command, such as `git status`.
Shell syntax around those words can change what the program does:
`GIT_CONFIG_*` makes `git status` run another program, and `>>` makes `echo`
write a startup file. Claude Code and Codex CLI approve by rule only commands
that parse into plain words, and Pagis uses that model.

The broker parses each command with tree-sitter-bash. A rule matches only a
fully parsed list of simple commands joined by `&&`, `||`, `;`, `|` or a line
break, where each word is a plain word, a single-quoted string, or a
double-quoted string with no expansion. A simple command matches when the
rule's words are its leading words after quote removal, and the command runs
without a card only when every simple command matches. An environment
assignment, a redirection, a here-document, an expansion (parameter,
arithmetic, brace, tilde or glob), a substitution, a subshell or a parse error
matches no rule.

No rule can name a program that runs other programs, such as `env`, `sh`,
`bash`, `xargs`, `find`, `sudo` or `eval`. Derivation proposes none, the
settings refuse one, and the match ignores a stored one. Derivation also
proposes no rule when the second word of a multi-word CLI is a flag, as in
`git -C /repo status`, and none for a command no rule can approve; the card
then offers a one-time approval alone.

A rule-approved command runs under `/bin/sh`, and the dispatch tells the Client
App that a rule approved it, because the matched shape has one meaning in POSIX
`sh` and a shell such as zsh has syntax the check does not model. A command the
person approved on its own card runs in the person's shell, as they read it.

A rule accepts every flag and argument of its program, including one that
writes a file or runs a program, such as `git log --output=<path>`. The broker
does not try to list such flags. Beside "Always allow" the card says so.

### The broker asks rather than choosing

A client declares what it can do, and a machine that declared no shell is
never a candidate for a shell command. Which machine can do what is in the
Capability Snapshot; whether it is present is read at the call. The candidates
are the machines the Agent holds a Grant on that can run the command and are
present. One candidate runs with no new question. None answers that nothing is
connected. Several ask the person which, through a Request, and the approval
names the chosen machine. An Agent with no host Grant takes every machine of
its person as a candidate, because the card writes the first Grant. The audit
fact of every host action, refusals included, carries the host id.

### Connecting to a server grants that server's Administrator the Host

The approval lives on the server. The client receives a `dispatch` frame and
runs it with no check of its own, because the Grant, the effect class, the
allow rules and the audit row are on the server. The trust statement:

**A client that connects to a Pagis server runs what that server dispatches.
The Host trusts the server that TLS authenticates, and a Host registers on no
clear-text connection to another machine. Connecting grants the server's
Administrator, and anybody who takes that server, the ability to run commands
on the client's machine as the OS user who started it.**

So a client connects over `https://` and opens the Host socket over `wss://`,
and accepts `http://` and `ws://` only on a loopback host (ADR-0024). There is
no confirmation on the client: it would ask a question the person cannot answer
better than the server's card, and it would not change the trust. The Client
App's setup states the trust in one line under the Server address field of
"Connect to a Pagis server": "Connect only to a server you trust."
`README.md` and `desktop/README.md` state it in full. On a local installation
the server is the person's own machine.

### A Host is not a Computer

A Computer is Pagis's own machine, sandboxed in a container, so
`computer_shell` needs no approval (ADR-0014). A Host is the person's own
machine: a command runs as the OS user who started the client, with no
container. The approval takes the container's place, so a host action keeps its
effect class, its card, its per-machine rules and its audit fact on every kind
of installation. A Computer runs on the daemon's Docker host; a Host is never
there.

## Consequences

- A person with only a browser cannot run a host command.
- The Client App carries an executor and keeps the socket open. A client that
  is not running is a machine that is not there.
- A person with several machines answers one more question for each command
  class on each machine, until they choose "always".
- A command with shell syntax outside the matched shape shows the card every
  time.
