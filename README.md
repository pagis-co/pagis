# Pagis

A staff of AI Agents that work for you, each with a job, a memory and a
computer of its own.

[![CI](https://github.com/pagis-co/pagis/actions/workflows/ci.yml/badge.svg)](https://github.com/pagis-co/pagis/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Pagis gives you a staff of Agents: AI helpers that work as your virtual
assistants. Each Agent has a job, a personality, its own memory and its own
computer, a container with a browser and a terminal. You work with them in a
chat app, in direct messages, group channels and threads. The interface
calls an Agent a sprite. One Rust daemon serves one person on a laptop or a
team on a server. It sends model requests to the providers that you
configure (Anthropic, OpenAI or OpenRouter), and it runs no model itself.

## Features

- **Agents that remember.** Each Agent keeps its memory as Markdown files
  under git. The Learning Feed shows each memory change, with a revert.
- **A computer for each Agent.** You watch its screen live, take control at
  any time and give control back.
- **Connections with scoped access.** Mail and calendar through Google, a
  Grant for each Agent, approvals for actions that change the world, and a
  Vault whose secrets the model never sees.
- **Proactive work.** Schedules and Event Subscriptions start work while you
  are away.
- **A phone number and a mailbox.** An Agent can hold its own number, make
  and answer calls, and send and receive mail from its own address.
- **Software and Plugins.** Agents build versioned tools that other Agents
  use and fork. An Administrator installs Plugins: Skills and MCP servers.
- **Hosts.** An Agent runs an approved command on your own machine through
  the Client App.
- **One person or a team.** One installation is one Org. Each Person has a
  private Workspace, and an Administrator manages the People, the spend and
  the providers.

## Quick start

A release installs in two ways. The Client App runs Pagis on your own
computer or connects to a server. The Headless Server runs Pagis for a team
on a Linux VM.

### Client App (macOS and Linux)

1. Download the Client App from
   [GitHub Releases](https://github.com/pagis-co/pagis/releases):
   `Pagis-<release>-arm64.dmg` for macOS arm64, or the AppImage or the deb
   for Linux amd64 and arm64. On Ubuntu 24.04 and later, use the deb.
2. Open it and choose **Install on this computer** or **Connect to a Pagis
   server**.
3. On this computer, the onboarding asks for one model provider key and,
   optionally, a Docker endpoint for the Agents' computers. Without Docker,
   the Agents chat and use your Connections, but they cannot browse or run
   commands.

**When you connect to a server, this computer becomes a Host of that
server. The server and its Administrator can then run commands on this
computer, with the same access as you. Connect only to a server whose
Administrator you trust.** The setup page states this in one line under the
Server address field: "Connect only to a server you trust."

[desktop/README.md](desktop/README.md) describes both setups, the data
directory and backup.

### Headless Server (Linux VM with Docker)

```bash
git clone --depth 1 --branch v<release> https://github.com/pagis-co/pagis
cd pagis/deploy
mkdir -p secrets
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > secrets/pagis-secrets-key
chmod 600 secrets/pagis-secrets-key
cp .env.example .env    # set the domain, the public address and the passwords
docker compose up -d
```

The compose deployment runs the server image, Postgres, a Caddy proxy that
holds the TLS certificate, and the egress rules of the Computers. Set the
first Administrator in `.env`, or on the Administration Port over an SSH
tunnel. [docs/DEPLOYING-A-SERVER.md](docs/DEPLOYING-A-SERVER.md) is the
full procedure.

### From source

You need Rust, Node.js (the version in `.nvmrc`) and, for the Agents'
computers, Docker.

```bash
(cd ui && npm ci && npm run build)
cargo run -p pagis -- --local
```

`--local` makes a local installation. At the first run, the daemon prints a
one-time sign-in link and opens the browser on it. `pagis --help` lists
every flag and command.

## Analytics

A release build sends anonymous analytics to PostHog once a day: the
release, the operating system, the kind of installation, the number of
People and Agents as ranges, and which features are in use. The events
carry a random installation ID. They hold no content, no names, no
addresses and no IP address. An Administrator turns them off in System
Settings in the Administration Interface, and `DO_NOT_TRACK=1` in the
environment of the daemon stops them too. A build from source sends
nothing. [ADR-0026](docs/adr/0026-the-daemon-sends-anonymous-analytics-from-a-release-build.md)
holds the details.

## Documentation

| Topic | Document |
| --- | --- |
| The glossary of domain terms | [CONTEXT.md](CONTEXT.md) |
| Product direction and the parts that are not built | [docs/VISION.md](docs/VISION.md) |
| Architecture and decisions | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/adr/](docs/adr) |
| Data, encryption and what a model provider receives | [docs/DATA-AND-PRIVACY.md](docs/DATA-AND-PRIVACY.md) |
| The Client App | [desktop/README.md](desktop/README.md) |
| Run a server for a team | [docs/DEPLOYING-A-SERVER.md](docs/DEPLOYING-A-SERVER.md) |
| The live screen and the TURN variant | [docs/SCREEN-RELAY.md](docs/SCREEN-RELAY.md) |
| Write a Plugin | [docs/PLUGINS.md](docs/PLUGINS.md) |
| Write a Widget | [docs/WIDGETS.md](docs/WIDGETS.md) |
| What belongs on a memory page | [docs/MEMORY-PAGES.md](docs/MEMORY-PAGES.md) |
| The interface design system | [docs/UI-DESIGN.md](docs/UI-DESIGN.md) |
| The LLM router: usage, design, modalities | [docs/USAGE.md](docs/USAGE.md), [docs/DESIGN.md](docs/DESIGN.md), [docs/MODALITIES.md](docs/MODALITIES.md) |
| Release the server and the Client App | [docs/RELEASING-SERVER.md](docs/RELEASING-SERVER.md), [docs/RELEASING-CLIENT.md](docs/RELEASING-CLIENT.md) |

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) describes the development setup, the
checks and how a pull request merges.

## Security

Report a vulnerability through GitHub private vulnerability reporting. Do
not open a public issue for it. [SECURITY.md](SECURITY.md) holds the
policy.

## License

Pagis is licensed under the [MIT License](LICENSE). Each server package
also carries `LICENSE.gog` and `THIRD_PARTY_NOTICES` for the third-party
programs that it ships.
