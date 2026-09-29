# Data, encryption and privacy

This document states where an installation keeps its data, what Pagis
encrypts, what a Computer reaches, and what a model provider receives.
`CONTEXT.md` defines the terms. ADR-0013 and ADR-0024 hold the decisions.

## Data and configuration

A local installation keeps everything under `~/.pagis`. `PAGIS_HOME` moves
the directory. A Headless Server keeps it at `/var/lib/pagis`
(`docs/DEPLOYING-A-SERVER.md`).

| Path | What it is |
| --- | --- |
| `config.toml` | The configuration. The product port is 4400 by default. |
| `pagis.db` | The records of a local installation, in SQLite. A local installation does not start when `[database] url` or `PAGIS_DATABASE_URL` is set. A server keeps its records in Postgres and does not start without a database URL. |
| `memory/<workspace_id>/` | The memory of one Workspace, as a git repository. |
| `secrets.enc` | The provider keys and the other secrets, sealed with the Installation Key. |
| `installation-key` | The Key File that a local installation generates where the keychain or the Secret Service does not answer. |
| `client-credential` | The Client Credential, which the Client App trades for a Session. |
| `runtime-release` | The Release Marker: the newest release that opened this data. |
| `artifacts/`, `recordings/` | The files that the Agents made, the files that People attach, the screenshots and the call recordings. |
| `screens/` | The last screenshot of each Computer. |
| `computer-tokens/` | The access token of each Computer. The daemon makes a new token at each start of that Computer. |
| `gog/` | The files of `gog`, the Google provider, for each Workspace. |
| `plugins/`, `software/` | The installed Plugins and the Software Packages. |
| `logs/` | The logs of the daemon, and in `logs/plugins/<workspace_id>/` the stderr log of each Plugin in that Workspace. |

Only the OS user that runs Pagis can read the State Directory. The
directory is mode 700, and the files that Pagis writes into it give no
permission to a group or to other users. The Plugin checkouts are the one
exception, because a Computer reads them as a different user. When other
users can enter the directory, Pagis sets it to mode 700 at start and logs
the change. Pagis does not start when a group or another user can read
`client-credential`: remove the file, and the next start writes a new one.

The daemon bind-mounts files from the State Directory into each Computer.
When Docker runs in a VM (Docker Desktop, Colima), the VM must share the
directory. Colima and Docker Desktop share your home directory, and they
do not share `/tmp`. A Computer that gets an empty file from a directory
that the VM does not share fails to wake, and the failure names the
directory.

### Ports and settings

`PAGIS_PORT` and `pagis --port <PORT>` override the product port. When the
port is taken, the daemon names the port and the flag that moves it.

The daemon serves the Administration Interface on a second port, 4401 by
default, bound to loopback. `[administration]` in `config.toml` moves it.
The Client App opens it from its menu, and Settings → Administration in the
Product App links an Administrator to it. An Administrator changes the
port, the Docker endpoint and the log level in its Settings view, and Pagis
writes `config.toml` itself. Each Person sets their own timezone in the
Settings of the Product App.

A provider key in `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` or
`OPENROUTER_API_KEY` wins over the sealed key.

### The Installation Key

The daemon seals its secrets in `secrets.enc` with the Installation Key. A
local installation makes that key itself. It is one item in the keychain on
macOS, or in the desktop keyring through the Secret Service on Linux. Where
that store does not answer, it is the Key File `~/.pagis/installation-key`,
mode 600, which the daemon generates. A Key File that is there wins. The
start log says which one the daemon uses. A server on Linux reads the key
from the Key File that its deployment mounts.

### Docker

Docker is optional. Without it the daemon runs, but the Agents have no
Computers: no browser, no terminal and no screen. Pagis finds Docker
itself. It tries `DOCKER_HOST`, the current docker context, the socket of
Docker Desktop, each Colima socket, then `/var/run/docker.sock`. The
Settings view of the Administration Interface shows what it tried and
takes an endpoint of your own.

## What a Computer reaches

A Computer on a local installation reaches the public internet, your LAN,
and each service of this computer that listens on a network address. Under
Docker Desktop or Colima it can possibly also reach the services that
listen on the loopback of this computer, through the host gateway. Pagis
installs no firewall rules here: a daemon that runs as your user cannot
write the firewall of this computer or of the Docker VM. Text in a web
page, a mail or a document can steer an Agent, so keep the services on your
LAN and on this computer behind their own sign-in.

A Headless Server installs rules that keep each Computer to the public
internet ("What a Computer reaches" in `docs/DEPLOYING-A-SERVER.md`).

## Several People on a local installation

Any installation can hold several People, and the seeded Person is the
Administrator. To let other People reach a local installation, put TLS in
front of it with your own proxy or tunnel (Caddy, Tailscale Serve,
Cloudflare Tunnel), then turn on **Multi-user mode** in the Settings view
of the Administration Interface and type the name that the proxy answers on
as the Public Origin. "A local installation that serves several People" in
`docs/DEPLOYING-A-SERVER.md` holds each setup.

The Computers of the Agents of the other People also run on this computer,
and they reach what "What a Computer reaches" states. The providers of the
keys that you store get the model requests of every Person
([What the model provider receives](#what-the-model-provider-receives)).

## What Pagis encrypts

Pagis encrypts secrets and no other content. The Installation Key seals
`secrets.enc` with XChaCha20-Poly1305. The Tenant Data Key of each
Workspace, which `secrets.enc` holds, seals the Credential secrets, the
one-time code seeds and the `brokered` refresh tokens of that Workspace in
the database. The database keeps Sessions, Sign-In Links and passwords as
hashes.

| Store | What it holds | Encrypted |
| --- | --- | --- |
| `secrets.enc` | The provider keys, the other secrets of the installation and of each Workspace, and the Tenant Data Key of each Workspace. | Yes, with the Installation Key. |
| `pagis.db`, or the Postgres database of a server | The records of every Person: the People, the conversations with their tool results, the Runs, the Connections, the Source Items that a Sync copies (for example mail and calendar events), the Grants and the Vault. | Partly. The Tenant Data Keys seal the Credential secrets, the one-time code seeds and the `brokered` refresh tokens. Sessions, Sign-In Links and passwords are hashes. |
| `memory/<workspace_id>/` | The memory of each Workspace, with its full git history. | No. |
| `artifacts/`, `recordings/` | The Artifacts: the files that the Agents made, the files that People attach, the screenshots and the call recordings. | No. |
| `screens/` | The last screenshot of each Computer. | No. |
| `gog/` | The files of `gog` for each Workspace. | Partly. `gog` seals the tokens and the OAuth client secret of a `byo` Google Connection with a password that `secrets.enc` holds. |
| `plugins/`, `software/` | The installed Plugins and the Software Packages. | No. |
| `config.toml`, `runtime-release`, `logs/` | The configuration, the Release Marker, and the logs. A Plugin server can write Person data or a bound value to its stderr. | No. A Backup does not hold the logs. |
| `client-credential`, `computer-tokens/` | The Client Credential, and the access token of each Computer. | No. A Backup holds neither. |
| `installation-key` | The Key File. | No. It is the Installation Key. |
| The Computer volumes | The home of each Computer: its files, and the browser profile with the sign-ins of the Agent. | No. |

The file permissions of the State Directory keep the other users of the
machine out. They do not protect a disk that another computer reads, a
disk image or a volume snapshot. Protect the content with full-disk
encryption: FileVault on macOS, LUKS on Linux, or the disk encryption of
your cloud provider. On a Headless Server, encrypt the disk of
`/var/lib/pagis` and the disk of `/var/lib/docker`, which holds the
Postgres cluster and the Computer volumes.

A Backup holds the same content without encryption. Only the owner can read
it. Encrypt it with your backup tool, for example restic or age, and keep it
apart from the Key File. A person who reads the Backup gets the records,
the memory and the Artifacts of every Person. A Backup never holds the
Installation Key, the Client Credential or `computer-tokens/`, and Sessions
are hashes. So that person gets no secret that Pagis seals, no Session and
no access token of a Computer. The Backup of a local installation does not
hold the Computer volumes. "Backup and restore" in
`docs/DEPLOYING-A-SERVER.md` and "Back up and restore" in
`desktop/README.md` give the procedure for each installation method.

### What the model provider receives

Pagis runs no model. It sends each model request to a provider that holds a
key of the installation: Anthropic, OpenAI or OpenRouter. The provider reads
the content of each request:

- **A Run** sends the system prompt, the conversation and the tool results
  at each Turn. The system prompt holds the name, the job and the
  personality of the Agent, the Briefing, the memory indexes, and the memory
  pages of the Brief or of a Reflection. The conversation holds the
  messages of the Channel or the Thread with their attached images, and the
  Continuation Record after a Compaction. A tool result holds what the tool
  read, for example a memory page, a mail, a web page, the output of a
  command or a screenshot of the Computer. A file attachment goes as its
  name and its type only.
- **Dictation** sends the audio clip. **Speaking** sends the text that it
  reads aloud.
- **A Call** sends the audio of the Call to the voice models of the Call.

No tool returns a Credential secret, and the model gets no screenshot while
the daemon fills a Credential (ADR-0013). The Keypad Code does not go to the
model (ADR-0021). The provider decides what it keeps from a request, and
for how long.

On a server, and on a local installation in Multi-User Mode, the
Administrator stores the provider keys for the whole Org. The providers of
these keys get the model requests of every Person.

## The trust of a connected Client App

When a Client App connects to a server, its machine becomes a Host of that
server. The server and its Administrator can then run commands on that
machine as the OS user who started the client. The approval for a command
lives on the server, and the client runs what the server sends. So a person
who takes control of the server can also run commands on each connected
machine (ADR-0015).

The client connects only to an `https://` address, or to an `http://`
address on loopback, such as an SSH tunnel. It refuses any other `http://`
address before it sends a request, so the password, the Session and the
commands for the machine never cross the network as clear text (ADR-0024).
